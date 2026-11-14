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

    let token = Uuid::new_v4().simple().to_string();
    let client_ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("EmbyClient")
        .chars()
        .take(60)
        .collect::<String>();

    let _ = sqlx::query(
        "INSERT INTO access_tokens (token, user_id, device_id, device_name, client_name) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&token)
    .bind(&user_id)
    .bind(&token[..12])
    .bind(&client_ua)
    .bind(&client_ua)
    .execute(&state.db)
    .await;

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

