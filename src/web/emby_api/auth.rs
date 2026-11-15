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
    let user_row: Option<(String, String, bool)> = sqlx::query_as(
        "SELECT id, password_hash, is_admin FROM users WHERE username = $1",
    )
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
                .unwrap_or(0);
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

    // 按 (user_id, device_id / client_name) 复用已有令牌并清理同客户端历史重复记录
    let existing_token: Option<String> = sqlx::query_scalar(
        r#"
        SELECT token FROM access_tokens
        WHERE user_id = $1 AND (device_id = $2 OR client_name = $3)
        ORDER BY last_used_at DESC
        LIMIT 1
        "#,
    )
    .bind(&user_id)
    .bind(&device_id)
    .bind(&client_name)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    let token = if let Some(tok) = existing_token {
        let _ = sqlx::query(
            "UPDATE access_tokens SET device_id = $2, device_name = $3, client_name = $4, last_used_at = NOW() WHERE token = $1",
        )
        .bind(&tok)
        .bind(&device_id)
        .bind(&device_name)
        .bind(&client_name)
        .execute(&state.db)
        .await;
        let _ = sqlx::query(
            "DELETE FROM access_tokens WHERE user_id = $1 AND token <> $2 AND (device_id = $3 OR client_name = $4)",
        )
        .bind(&user_id)
        .bind(&tok)
        .bind(&device_id)
        .bind(&client_name)
        .execute(&state.db)
        .await;
        tok
    } else {
        let new_tok = Uuid::new_v4().simple().to_string();
        let _ = sqlx::query(
            "INSERT INTO access_tokens (token, user_id, device_id, device_name, client_name) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&new_tok)
        .bind(&user_id)
        .bind(&device_id)
        .bind(&device_name)
        .bind(&client_name)
        .execute(&state.db)
        .await;
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
        .unwrap_or_else(|| format!("client:{}", client_name))
        .chars()
        .take(64)
        .collect();

    (stable_device_id, device_name, client_name)
}

pub(super) async fn get_user_info(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<String>,
) -> Json<Value> {
    let cfg = state.config.load();
    let user_row: Option<(String, bool)> =
        sqlx::query_as("SELECT username, is_admin FROM users WHERE id = $1")
            .bind(&user_id)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
    let (username, is_admin) = user_row.unwrap_or_else(|| ("admin".to_string(), true));

    Json(json!({
        "Id": user_id,
        "Name": username,
        "ServerId": cfg.server_id,
        "Policy": { "IsAdministrator": is_admin }
    }))
}


/// 解析请求对应的合法 user_id（优先校验显式 user_id -> 其次校验 Token -> 兜底取首个管理员账号）
pub(super) async fn resolve_valid_user_id(
    state: &AppState,
    explicit_uid: Option<&str>,
    headers: &HeaderMap,
) -> Result<String, (StatusCode, String)> {
    if let Some(uid) = explicit_uid.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let exists: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE id = $1")
            .bind(uid)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
        if let Some(valid_id) = exists {
            return Ok(valid_id);
        }
    }

    if let Some(token) = headers
        .get("x-emby-token")
        .or_else(|| headers.get("x-mediabrowser-token"))
        .and_then(|v| v.to_str().ok())
    {
        let uid_from_token: Option<String> =
            sqlx::query_scalar("SELECT user_id FROM access_tokens WHERE token = $1")
                .bind(token)
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None);
        if let Some(valid_id) = uid_from_token {
            let db = state.db.clone();
            let tok = token.to_string();
            tokio::spawn(async move {
                let _ = sqlx::query("UPDATE access_tokens SET last_used_at = NOW() WHERE token = $1")
                    .bind(tok)
                    .execute(&db)
                    .await;
            });
            return Ok(valid_id);
        }
    }

    let fallback: Option<String> =
        sqlx::query_scalar("SELECT id FROM users ORDER BY is_admin DESC, created_at ASC LIMIT 1")
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);

    if let Some(uid) = fallback {
        return Ok(uid);
    }

    // 若 users 表完全为空，自动创建默认管理员账号
    let new_id = Uuid::new_v4().simple().to_string()[..16].to_string();
    let _ = sqlx::query(
        "INSERT INTO users (id, username, password_hash, is_admin) VALUES ($1, 'admin', '', TRUE) ON CONFLICT DO NOTHING",
    )
    .bind(&new_id)
    .execute(&state.db)
    .await;
    Ok(new_id)
}

