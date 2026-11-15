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
    let (username, is_admin) = user_row.ok_or_else(|| (StatusCode::NOT_FOUND, "用户不存在".to_string()))?;

    Ok(Json(json!({
        "Id": valid_uid,
        "Name": username,
        "ServerId": cfg.server_id,
        "Policy": { "IsAdministrator": is_admin }
    })))
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
        (StatusCode::UNAUTHORIZED, "缺少访问令牌，请先登录".to_string())
    })?;

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

    let (token_uid, is_admin) = token_user.ok_or_else(|| {
        (StatusCode::UNAUTHORIZED, "访问令牌无效或已注销，请重新登录".to_string())
    })?;

    let db = state.db.clone();
    let tok = token.clone();
    tokio::spawn(async move {
        let _ = sqlx::query("UPDATE access_tokens SET last_used_at = NOW() WHERE token = $1")
            .bind(tok)
            .execute(&db)
            .await;
    });

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

