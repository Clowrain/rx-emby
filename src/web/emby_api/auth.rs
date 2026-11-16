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

pub(super) async fn system_info_public(State(state): State<Arc<AppState>>) -> Json<Value> {
    let cfg = state.config.load();
    Json(json!({
        "Id": cfg.server_id,
        "ServerName": cfg.server_name,
        "Version": "4.8.8.0",
        "OperatingSystem": "Linux",
        "LocalAddress": "http://127.0.0.1:8096"
    }))
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
    Json(payload): Json<AuthReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let username = if payload.username.trim().is_empty() {
        "admin"
    } else {
        payload.username.trim()
    };

    // 查询或首次自动初始化管理员账号
    let user_row: Option<(String, String, bool)> =
        sqlx::query_as("SELECT id, password_hash, is_admin FROM users WHERE username = $1")
            .bind(username)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (user_id, is_admin) = match user_row {
        Some((uid, stored_pw, admin)) => {
            if !stored_pw.is_empty() && stored_pw != payload.pw {
                return Err((StatusCode::UNAUTHORIZED, "用户名或密码错误".to_string()));
            }
            (uid, admin)
        }
        None => {
            // 若库中尚无任何用户，自动创建首个管理员账号
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
                .fetch_one(&state.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            if count == 0 {
                let new_id = Uuid::new_v4().simple().to_string()[..16].to_string();
                sqlx::query(
                    "INSERT INTO users (id, username, password_hash, is_admin) VALUES ($1, $2, $3, TRUE)",
                )
                .bind(&new_id)
                .bind(username)
                .bind(&payload.pw)
                .execute(&state.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                (new_id, true)
            } else {
                return Err((StatusCode::UNAUTHORIZED, "用户不存在".to_string()));
            }
        }
    };

    let (device_id, device_name, client_name) = parse_emby_client_identity(&headers);

    // 只按设备标识复用令牌；相同应用名的不同设备必须保留独立会话。
    let existing_token: Option<String> = sqlx::query_scalar(
        r#"
        SELECT token FROM access_tokens
        WHERE user_id = $1 AND device_id = $2
        ORDER BY last_used_at DESC
        LIMIT 1
        "#,
    )
    .bind(&user_id)
    .bind(&device_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        tracing::error!("查询已有访问令牌失败: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    let token = if let Some(tok) = existing_token {
        if let Err(e) = sqlx::query(
            "UPDATE access_tokens SET device_id = $2, device_name = $3, client_name = $4, last_used_at = NOW() WHERE token = $1",
        )
        .bind(&tok)
        .bind(&device_id)
        .bind(&device_name)
        .bind(&client_name)
        .execute(&state.db)
        .await
        {
            tracing::warn!("更新访问令牌设备信息失败: {}", e);
        }
        let removed: Vec<String> = sqlx::query_scalar(
            "DELETE FROM access_tokens WHERE user_id = $1 AND token <> $2 AND device_id = $3 RETURNING token",
        )
        .bind(&user_id)
        .bind(&tok)
        .bind(&device_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("清理同客户端旧访问令牌失败: {}", e);
            Vec::new()
        });
        if !removed.is_empty() {
            invalidate_token_cache(&state, &removed).await;
        }
        tok
    } else {
        let new_tok = Uuid::new_v4().simple().to_string();
        if let Err(e) = sqlx::query(
            "INSERT INTO access_tokens (token, user_id, device_id, device_name, client_name) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&new_tok)
        .bind(&user_id)
        .bind(&device_id)
        .bind(&device_name)
        .bind(&client_name)
        .execute(&state.db)
        .await
        {
            return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
        }
        new_tok
    };

    let cfg = state.config.load();
    Ok(Json(json!({
        "User": {
            "Id": user_id,
            "Name": username,
            "ServerId": cfg.server_id,
            "Policy": { "IsAdministrator": is_admin }
        },
        "AccessToken": token,
        "ServerId": cfg.server_id
    })))
}

fn extract_emby_auth_field(auth_header: &str, key: &str) -> Option<String> {
    let needle = format!("{}=", key);
    let idx = auth_header.find(&needle)?;
    let rest = &auth_header[idx + needle.len()..];
    if let Some(after_quote) = rest.strip_prefix('"') {
        let end = after_quote.find('"')?;
        let val = after_quote[..end].trim();
        return (!val.is_empty()).then(|| val.to_string());
    }
    let val = rest.split(',').next().unwrap_or("").trim();
    (!val.is_empty()).then(|| val.to_string())
}

fn parse_emby_client_identity(headers: &HeaderMap) -> (String, String, String) {
    let auth_header = headers
        .get("x-emby-authorization")
        .or_else(|| headers.get(header::AUTHORIZATION))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("EmbyClient")
        .trim();

    let client = extract_emby_auth_field(auth_header, "Client");
    let version = extract_emby_auth_field(auth_header, "Version");
    let device = extract_emby_auth_field(auth_header, "Device");
    let device_id = extract_emby_auth_field(auth_header, "DeviceId");

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
        .take(64)
        .collect();

    (stable_device_id, device_name, client_name)
}

pub(super) async fn get_user_info(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let valid_uid = resolve_valid_user_id(&state, Some(&user_id), &headers).await?;
    let cfg = state.config.load();
    let user_row: Option<(String, bool)> =
        sqlx::query_as("SELECT username, is_admin FROM users WHERE id = $1")
            .bind(&valid_uid)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let (username, is_admin) =
        user_row.ok_or_else(|| (StatusCode::NOT_FOUND, "用户不存在".to_string()))?;

    Ok(Json(json!({
        "Id": valid_uid,
        "Name": username,
        "ServerId": cfg.server_id,
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
    invalidate_token_cache_entries(&state.caches.token_user_cache, tokens).await;
}

pub(super) fn extract_emby_token(headers: &HeaderMap, api_key: Option<&str>) -> Option<String> {
    if let Some(k) = api_key.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(k.to_string());
    }
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
    if let Some(auth) = headers
        .get("x-emby-authorization")
        .or_else(|| headers.get(header::AUTHORIZATION))
        .and_then(|v| v.to_str().ok())
    {
        if let Some(tok) = extract_emby_auth_field(auth, "Token") {
            return Some(tok);
        }
        let trimmed = auth.trim();
        if let Some(tok) = trimmed
            .strip_prefix("Bearer ")
            .or_else(|| trimmed.strip_prefix("bearer "))
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(tok.to_string());
        }
    }
    None
}

/// 严格校验访问令牌，并校验显式 user_id 是否属于本人或由管理员代查
pub(super) async fn resolve_valid_user_id(
    state: &AppState,
    explicit_uid: Option<&str>,
    headers: &HeaderMap,
) -> Result<String, (StatusCode, String)> {
    resolve_valid_user_id_with_query(state, explicit_uid, headers, None).await
}

pub(super) async fn resolve_valid_user_id_with_query(
    state: &AppState,
    explicit_uid: Option<&str>,
    headers: &HeaderMap,
    api_key: Option<&str>,
) -> Result<String, (StatusCode, String)> {
    let token = extract_emby_token(headers, api_key).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            "缺少访问令牌，请先登录".to_string(),
        )
    })?;

    // 短 TTL 内存校验缓存：大多数请求免 PG JOIN；注销/吊销时通过 invalidate_token_cache 失效
    let token_uid;
    let is_admin;
    if let Some(cached) = state.caches.token_user_cache.get(&token).await {
        (token_uid, is_admin) = (cached.0.clone(), cached.1);
    } else {
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
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let (uid, admin) = token_user.ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                "访问令牌无效或已注销，请重新登录".to_string(),
            )
        })?;
        state
            .caches
            .token_user_cache
            .insert(token.clone(), std::sync::Arc::new((uid.clone(), admin)))
            .await;
        (token_uid, is_admin) = (uid, admin);
    }

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

    if let Some(uid) = explicit_uid.map(str::trim).filter(|s| !s.is_empty()) {
        if uid != token_uid {
            if !is_admin {
                return Err((StatusCode::FORBIDDEN, "无权访问其他用户数据".to_string()));
            }
            let exists: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE id = $1")
                .bind(uid)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            return exists.ok_or_else(|| (StatusCode::NOT_FOUND, "目标用户不存在".to_string()));
        }
    }

    Ok(token_uid)
}

#[cfg(test)]
mod cache_tests {
    use super::{extract_emby_token, invalidate_token_cache_entries, parse_emby_client_identity};
    use axum::http::{HeaderMap, HeaderValue};
    use moka::future::Cache;
    use std::sync::Arc;

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
            Some("query-token")
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
