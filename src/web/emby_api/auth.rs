use super::query::EmbyQuery as Query;
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize, Default)]
pub(super) struct AuthQuery {
    #[serde(
        rename = "api_key",
        alias = "ApiKey",
        alias = "X-Emby-Token",
        alias = "x-emby-token"
    )]
    api_key: Option<String>,
}

pub(super) async fn system_ping() -> (StatusCode, &'static str) {
    (StatusCode::OK, "Emby Server")
}

fn client_ip(headers: &HeaderMap, peer: Option<std::net::SocketAddr>) -> String {
    headers
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
        .or_else(|| {
            headers
                .get("x-forwarded-for")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(',').next())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| peer.map(|address| address.ip().to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

fn local_address(headers: &HeaderMap) -> String {
    headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .filter(|host| {
            !host.is_empty() && !host.contains('@') && !host.contains('/') && !host.contains('\\')
        })
        .map(|host| format!("http://{}", host))
        .unwrap_or_else(|| "http://127.0.0.1:8096".to_string())
}

pub(super) async fn system_info_public(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Json<Value> {
    let cfg = state.config.load();
    Json(json!({
        "Id": cfg.server_id,
        "ServerName": cfg.server_name,
        "Version": "4.8.8.0",
        "OperatingSystem": std::env::consts::OS,
        "LocalAddress": local_address(&headers)
    }))
}

pub(super) async fn system_info(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let _user_id =
        resolve_valid_user_id_with_query(&state, None, &headers, q.api_key.as_deref()).await?;
    let cfg = state.config.load();
    Ok(Json(json!({
        "Id": cfg.server_id,
        "ServerName": cfg.server_name,
        "Version": "4.8.8.0",
        "OperatingSystem": std::env::consts::OS,
        "LocalAddress": local_address(&headers),
        "HasPendingRestart": false,
        "IsShuttingDown": false,
        "SupportsLibraryMonitor": true,
        "WebSocketPortNumber": 8096,
        "CanSelfRestart": false,
        "CanLaunchWebBrowser": false,
        "SupportsAutoRunAtStartup": false,
        "HasUpdateAvailable": false,
        "EncoderLocationType": "System"
    })))
}

pub(super) async fn get_public_users(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let cfg = state.config.load();
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT id, username, password_hash FROM users ORDER BY id ASC")
            .fetch_all(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let users: Vec<Value> = rows
        .into_iter()
        .map(|(id, name, pw_hash)| {
            let has_pw = !pw_hash.is_empty();
            json!({
                "Id": id,
                "Name": name,
                "ServerId": cfg.server_id,
                "HasPassword": has_pw,
                "HasConfiguredPassword": has_pw,
                "EnableAutoLogin": !has_pw,
                "PrimaryImageTag": "default-user-avatar-v1"
            })
        })
        .collect();

    Ok(Json(Value::Array(users)))
}

#[derive(Deserialize)]
pub(super) struct AuthReq {
    #[serde(rename = "Username")]
    username: String,
    #[serde(rename = "Pw", default)]
    pw: String,
}

pub(super) async fn authenticate_by_name(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    peer: Option<axum::extract::ConnectInfo<std::net::SocketAddr>>,
    Json(payload): Json<AuthReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let username = payload.username.trim();
    if username.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "请输入用户名".to_string()));
    }

    let client_ip = client_ip(&headers, peer.map(|axum::extract::ConnectInfo(addr)| addr));
    if state.login_rate_limited(username, &client_ip).await {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            "登录失败次数过多，请稍后重试".to_string(),
        ));
    }

    // PBKDF2 runs before acquiring the bootstrap lock or opening a transaction.
    let user_row: Option<(String, String, bool)> =
        sqlx::query_as("SELECT id, password_hash, is_admin FROM users WHERE username = $1")
            .bind(username)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let prepared = match user_row {
        Some((uid, stored_pw, admin)) => {
            if (admin && stored_pw.is_empty())
                || (!stored_pw.is_empty()
                    && !crate::web::password::verify_password_blocking(
                        stored_pw.clone(),
                        payload.pw.clone(),
                    )
                    .await?)
                || (!admin && stored_pw.is_empty() && !payload.pw.is_empty())
            {
                state.record_failed_login(username, &client_ip).await;
                return Err((StatusCode::UNAUTHORIZED, "用户名或密码错误".to_string()));
            }
            let upgraded = if !stored_pw.is_empty() && !stored_pw.starts_with("$pbkdf2-sha256$") {
                Some(crate::web::password::hash_password_blocking(payload.pw.clone()).await?)
            } else {
                None
            };
            Some((uid, admin, upgraded))
        }
        None => None,
    };
    if prepared.is_none() {
        state.record_failed_login(username, &client_ip).await;
        return Err((StatusCode::UNAUTHORIZED, "用户名或密码错误".to_string()));
    }
    state
        .caches
        .failed_login_attempts
        .invalidate(&format!("user:{}", username.to_lowercase()))
        .await;
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let (user_id, is_admin) = match prepared {
        Some((uid, admin, upgraded)) => {
            if let Some(hash) = upgraded {
                sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
                    .bind(hash)
                    .bind(&uid)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            }
            (uid, admin)
        }
        None => return Err((StatusCode::UNAUTHORIZED, "用户名或密码错误".to_string())),
    };

    let (device_id, device_name, client_name) = parse_emby_client_identity(&headers);

    // Rotate device credentials on every login so a shared device identity cannot recover an active token.
    let removed_tokens: Vec<String> = sqlx::query_scalar(
        "DELETE FROM access_tokens WHERE user_id = $1 AND device_id = $2 RETURNING token",
    )
    .bind(&user_id)
    .bind(&device_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let token = Uuid::new_v4().simple().to_string();
    sqlx::query(
        "INSERT INTO access_tokens (token, user_id, device_id, device_name, client_name) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&token)
    .bind(&user_id)
    .bind(&device_id)
    .bind(&device_name)
    .bind(&client_name)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    invalidate_token_cache(&state, &removed_tokens).await;

    let cfg = state.config.load();
    Ok(Json(json!({
        "User": {
            "Id": user_id,
            "Name": username,
            "ServerId": cfg.server_id,
            "Policy": { "IsAdministrator": is_admin }
        },
        "SessionInfo": {
            "Id": format!("{}:{}", device_id, user_id),
            "UserId": user_id,
            "UserName": username,
            "Client": client_name,
            "DeviceId": device_id,
            "DeviceName": device_name,
            "SupportsRemoteControl": true
        },
        "AccessToken": token,
        "ServerId": cfg.server_id
    })))
}

fn extract_emby_auth_field(auth_header: &str, key: &str) -> Option<String> {
    let bytes = auth_header.as_bytes();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    let mut fields = Vec::new();
    for (i, &byte) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && byte == b'\\' {
            escaped = true;
            continue;
        }
        if byte == b'"' {
            quoted = !quoted;
            continue;
        }
        if byte == b',' && !quoted {
            fields.push(&auth_header[start..i]);
            start = i + 1;
        }
    }
    if !quoted {
        fields.push(&auth_header[start..]);
    }
    for field in fields {
        let Some((name, raw)) = field.trim().split_once('=') else {
            continue;
        };
        let name = name.split_whitespace().last().unwrap_or_default();
        if !name.eq_ignore_ascii_case(key) {
            continue;
        }
        let raw = raw.trim();
        let value = if let Some(rest) = raw.strip_prefix('"') {
            let mut out = String::new();
            let mut chars = rest.chars();
            while let Some(ch) = chars.next() {
                match ch {
                    '"' => break,
                    '\\' => {
                        if let Some(escaped) = chars.next() {
                            out.push(escaped);
                        }
                    }
                    _ => out.push(ch),
                }
            }
            out
        } else {
            raw.trim().to_string()
        };
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
fn hash_password(password: &str) -> String {
    crate::web::password::hash_password_sync(password).unwrap_or_default()
}

#[cfg(test)]
fn verify_password(stored: &str, password: &str) -> bool {
    crate::web::password::verify_password_sync(stored, password)
}

pub(super) fn parse_emby_client_identity(headers: &HeaderMap) -> (String, String, String) {
    let field = |key| {
        ["authorization", "x-emby-authorization"]
            .into_iter()
            .filter_map(|name| headers.get(name).and_then(|v| v.to_str().ok()))
            .find_map(|auth| extract_emby_auth_field(auth, key))
    };

    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("EmbyClient")
        .trim();

    let client = field("Client");
    let version = field("Version");
    let device = field("Device");
    let device_id = field("DeviceId");

    let client_name: String = match (client, version) {
        (Some(c), Some(v)) => format!("{}/{}", c, v),
        (Some(c), None) => c,
        _ => ua.to_string(),
    }
    .chars()
    .take(60)
    .collect();

    let device_name: String = device
        .unwrap_or_else(|| client_name.clone())
        .chars()
        .take(60)
        .collect();

    let stable_device_id: String = device_id
        .unwrap_or_else(|| format!("anonymous:{}", Uuid::new_v4().simple()))
        .chars()
        .take(128)
        .collect();

    (stable_device_id, device_name, client_name)
}

pub(super) async fn get_user_info(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Query(q): Query<AuthQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let valid_uid =
        resolve_valid_user_id_with_query(&state, Some(&user_id), &headers, q.api_key.as_deref())
            .await?;
    let cfg = state.config.load();
    let user_row: Option<(String, bool, bool)> =
        sqlx::query_as("SELECT username, is_admin, password_hash <> '' FROM users WHERE id = $1")
            .bind(&valid_uid)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let (username, is_admin, has_password) =
        user_row.ok_or_else(|| (StatusCode::NOT_FOUND, "用户不存在".to_string()))?;

    Ok(Json(json!({
        "Id": valid_uid,
        "Name": username,
        "ServerId": cfg.server_id,
        "HasPassword": has_password,
        "HasConfiguredPassword": has_password,
        "EnableAutoLogin": !has_password,
        "PrimaryImageTag": "default-user-avatar-v1",
        "Policy": { "IsAdministrator": is_admin }
    })))
}

/// 令牌被注销/吊销后使其内存校验缓存立即失效
async fn invalidate_token_cache_entries(
    cache: &moka::future::Cache<String, Arc<(String, bool)>>,
    tokens: &[String],
) {
    for token in tokens {
        cache.invalidate(token).await;
    }
}

pub(in crate::web) async fn invalidate_token_cache(state: &AppState, tokens: &[String]) {
    state.session_hub.disconnect_tokens(tokens);
    for token in tokens {
        state.client_capabilities.invalidate(token);
    }
    invalidate_token_cache_entries(&state.caches.token_user_cache, tokens).await;
    for token in tokens {
        state.caches.admin_token_user_cache.invalidate(token).await;
        state.caches.token_touch_debounce.invalidate(token).await;
    }
}

pub(super) fn extract_emby_token(headers: &HeaderMap, api_key: Option<&str>) -> Option<String> {
    for key in ["x-emby-token", "x-mediabrowser-token"] {
        if let Some(t) = headers
            .get(key)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(t.to_string());
        }
    }
    for auth in ["authorization", "x-emby-authorization"]
        .into_iter()
        .filter_map(|key| headers.get(key).and_then(|v| v.to_str().ok()))
    {
        if let Some(tok) = extract_emby_auth_field(auth, "Token") {
            return Some(tok);
        }
        let trimmed = auth.trim();
        if let Some(tok) = trimmed
            .split_once(' ')
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
            .map(|(_, token)| token.trim())
            .filter(|s| !s.is_empty())
        {
            return Some(tok.to_string());
        }
    }
    api_key
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

pub(super) struct EmbyAuthContext {
    pub user_id: String,
    pub is_admin: bool,
    pub token: String,
    pub device_id: String,
    pub device_name: String,
    pub client_name: String,
}

pub(super) async fn resolve_auth_context(
    state: &AppState,
    headers: &HeaderMap,
    api_key: Option<&str>,
) -> Result<EmbyAuthContext, (StatusCode, String)> {
    let token = extract_emby_token(headers, api_key).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            "缺少访问令牌，请先登录".to_string(),
        )
    })?;

    // `try_get_with` ensures concurrent cache misses for one token share a single PG lookup.
    let cached = state
        .caches
        .token_user_cache
        .try_get_with(token.clone(), async {
            let token_user: Option<(String, bool)> = sqlx::query_as(
                r#"
                SELECT u.id, u.is_admin
                FROM access_tokens t
                JOIN users u ON u.id = t.user_id
                WHERE t.token = $1
                "#,
            )
            .bind(&token)
            .fetch_optional(&state.db)
            .await?;
            token_user
                .map(std::sync::Arc::new)
                .ok_or(sqlx::Error::RowNotFound)
        })
        .await
        .map_err(|error| {
            if matches!(error.as_ref(), sqlx::Error::RowNotFound) {
                (
                    StatusCode::UNAUTHORIZED,
                    "访问令牌无效或已注销，请重新登录".to_string(),
                )
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            }
        })?;
    let (token_uid, is_admin) = (cached.0.clone(), cached.1);

    // last_used_at 去抖：同一 token 5 分钟窗口内至多触发一次 UPDATE，避免每个请求都写库
    if state
        .caches
        .token_touch_debounce
        .get(&token)
        .await
        .is_none()
    {
        state
            .caches
            .token_touch_debounce
            .insert(token.clone(), ())
            .await;
        let db = state.db.clone();
        let tok = token.clone();
        tokio::spawn(async move {
            if let Err(e) = sqlx::query(
                "UPDATE access_tokens SET last_used_at = NOW() WHERE token = $1 AND last_used_at < NOW() - INTERVAL '5 minutes'",
            )
            .bind(&tok)
            .execute(&db)
            .await
            {
                tracing::warn!("更新访问令牌 last_used_at 失败: {}", e);
            }
        });
    }

    let (device_id, device_name, client_name) = parse_emby_client_identity(headers);
    Ok(EmbyAuthContext {
        user_id: token_uid,
        is_admin,
        token,
        device_id,
        device_name,
        client_name,
    })
}

pub(super) async fn resolve_valid_user_and_admin_with_query(
    state: &AppState,
    explicit_uid: Option<&str>,
    headers: &HeaderMap,
    api_key: Option<&str>,
) -> Result<(String, bool), (StatusCode, String)> {
    let context = resolve_auth_context(state, headers, api_key).await?;
    let Some(uid) = explicit_uid.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok((context.user_id, context.is_admin));
    };
    if uid == context.user_id {
        return Ok((context.user_id, context.is_admin));
    }
    if !context.is_admin {
        return Err((StatusCode::FORBIDDEN, "无权访问其他用户数据".to_string()));
    }
    let target_is_admin: Option<bool> =
        sqlx::query_scalar("SELECT is_admin FROM users WHERE id = $1")
            .bind(uid)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    target_is_admin
        .map(|is_admin| (uid.to_owned(), is_admin))
        .ok_or_else(|| (StatusCode::NOT_FOUND, "目标用户不存在".to_string()))
}

pub(super) async fn resolve_valid_user_id_with_query(
    state: &AppState,
    explicit_uid: Option<&str>,
    headers: &HeaderMap,
    api_key: Option<&str>,
) -> Result<String, (StatusCode, String)> {
    resolve_valid_user_and_admin_with_query(state, explicit_uid, headers, api_key)
        .await
        .map(|(uid, _)| uid)
}

#[cfg(test)]
mod cache_tests {
    use super::{
        client_ip, extract_emby_token, invalidate_token_cache_entries, parse_emby_client_identity,
    };

    #[test]
    fn client_ip_uses_connect_info_without_proxy_headers() {
        let ip = client_ip(
            &HeaderMap::new(),
            Some("[2001:db8::1]:1234".parse().unwrap()),
        );
        assert_eq!(ip, "2001:db8::1");
    }
    use axum::http::{HeaderMap, HeaderValue};
    use moka::future::Cache;
    use std::sync::Arc;

    #[test]
    fn bearer_scheme_is_case_insensitive() {
        let headers = HeaderMap::from_iter([(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("bEaReR my-token"),
        )]);
        assert_eq!(
            extract_emby_token(&headers, None).as_deref(),
            Some("my-token")
        );
    }

    #[test]
    fn auth_fields_require_token_boundary_and_match_key_case_insensitively() {
        assert_eq!(
            super::extract_emby_auth_field("MediaBrowser client=Infuse, tOkEn=abc", "Token")
                .as_deref(),
            Some("abc")
        );
        assert_eq!(
            super::extract_emby_auth_field("MediaBrowser NotToken=wrong, Client=Infuse", "Token"),
            None
        );
        assert_eq!(
            super::extract_emby_auth_field("DeviceId=phone-1, device=Phone", "DeviceId").as_deref(),
            Some("phone-1")
        );
        assert_eq!(
            super::extract_emby_auth_field("MediaBrowser token=first, TOKEN=second", "Token")
                .as_deref(),
            Some("first")
        );
        assert_eq!(
            super::extract_emby_auth_field("NotDeviceId=wrong", "DeviceId"),
            None
        );
        assert_eq!(
            super::extract_emby_auth_field("Token=\"abc,def\"", "Token").as_deref(),
            Some("abc,def")
        );
    }

    #[test]
    fn quoted_fields_ignore_commas_and_fake_fields_inside_quotes() {
        assert_eq!(
            super::extract_emby_auth_field(r#"Client="x, Token=fake", Token="real""#, "Token")
                .as_deref(),
            Some("real")
        );
        assert_eq!(
            super::extract_emby_auth_field(r#"Client="x, \"q\", Token=fake", Token=real"#, "Token")
                .as_deref(),
            Some("real")
        );
        assert_eq!(
            super::extract_emby_auth_field(r#"Token="unfinished, Token=real"#, "Token"),
            None
        );
    }

    #[test]
    fn password_hashes_verify_and_upgrade_legacy_values() {
        let encoded = super::hash_password("secret");
        assert!(super::verify_password(&encoded, "secret"));
        assert!(!super::verify_password(&encoded, "wrong"));
        assert!(super::verify_password("legacy", "legacy"));
        assert!(encoded.starts_with("$pbkdf2-sha256$"));
        assert_eq!(super::hash_password(""), "");
    }

    #[test]
    fn detail_token_extraction_supports_emby_authorization_and_api_key() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-emby-authorization",
            HeaderValue::from_static("MediaBrowser Token=authorization-token, Client=Test"),
        );
        assert_eq!(
            extract_emby_token(&headers, None).as_deref(),
            Some("authorization-token")
        );
        assert_eq!(
            extract_emby_token(&headers, Some("query-token")).as_deref(),
            Some("authorization-token")
        );
    }

    #[test]
    fn devices_sharing_a_client_name_have_distinct_identities() {
        let mut device_a = HeaderMap::new();
        device_a.insert(
            "x-emby-authorization",
            HeaderValue::from_static("MediaBrowser Client=Infuse, DeviceId=device-a"),
        );
        let mut device_b = HeaderMap::new();
        device_b.insert(
            "x-emby-authorization",
            HeaderValue::from_static("MediaBrowser Client=Infuse, DeviceId=device-b"),
        );
        let (a, _, client_a) = parse_emby_client_identity(&device_a);
        let (b, _, client_b) = parse_emby_client_identity(&device_b);
        assert_eq!(client_a, client_b);
        assert_ne!(a, b);

        let without_device_id = HeaderMap::from_iter([(
            axum::http::HeaderName::from_static("x-emby-authorization"),
            HeaderValue::from_static("MediaBrowser Client=Infuse"),
        )]);
        let (fallback_a, _, _) = parse_emby_client_identity(&without_device_id);
        let (fallback_b, _, _) = parse_emby_client_identity(&without_device_id);
        assert_ne!(fallback_a, fallback_b);
    }

    #[tokio::test]
    async fn revoking_a_token_invalidates_only_that_token_entry() {
        let cache: Cache<String, Arc<(String, bool)>> = Cache::new(4);
        cache
            .insert("revoked".into(), Arc::new(("user-a".into(), false)))
            .await;
        cache
            .insert("retained".into(), Arc::new(("user-b".into(), true)))
            .await;

        invalidate_token_cache_entries(&cache, &["revoked".to_string()]).await;

        assert!(cache.get(&"revoked".to_string()).await.is_none());
        assert!(cache.get(&"retained".to_string()).await.is_some());
    }
}
