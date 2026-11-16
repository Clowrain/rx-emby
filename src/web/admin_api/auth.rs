use crate::state::AppState;
use axum::{
    extract::{Extension, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub struct AdminUser {
    pub id: String,
    pub username: String,
    pub is_admin: bool,
}

fn extract_admin_token(headers: &HeaderMap) -> Option<String> {
    if let Some(auth) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        let trimmed = auth.trim();
        if let Some(token) = trimmed
            .strip_prefix("Bearer ")
            .or_else(|| trimmed.strip_prefix("bearer "))
        {
            let t = token.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    for key in ["X-Admin-Token", "X-Emby-Token"] {
        if let Some(val) = headers.get(key).and_then(|v| v.to_str().ok()) {
            let t = val.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

pub(super) async fn require_admin_auth(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, (StatusCode, String)> {
    let token = extract_admin_token(req.headers())
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "未登录，请先使用系统管理员账号登录".to_string()))?;

    let row: Option<(String, String, bool)> = sqlx::query_as(
        r#"
        SELECT u.id, u.username, u.is_admin
        FROM access_tokens t
        JOIN users u ON u.id = t.user_id
        WHERE t.token = $1
        "#,
    )
    .bind(&token)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (user_id, username, is_admin) = row.ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            "登录凭证无效或已过期，请重新登录".to_string(),
        )
    })?;

    if !is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            "无权访问：仅系统管理员权限角色可访问管理后台".to_string(),
        ));
    }

    let db = state.db.clone();
    let tok = token.clone();
    tokio::spawn(async move {
        let _ = sqlx::query("UPDATE access_tokens SET last_used_at = NOW() WHERE token = $1")
            .bind(&tok)
            .execute(&db)
            .await;
    });

    req.extensions_mut().insert(AdminUser {
        id: user_id,
        username,
        is_admin,
    });

    Ok(next.run(req).await)
}

#[derive(Deserialize)]
pub(super) struct AdminLoginReq {
    username: String,
    #[serde(default)]
    password: String,
}

pub(super) async fn admin_login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<AdminLoginReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let username = payload.username.trim();
    if username.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "请输入用户名".to_string()));
    }

    let user_row: Option<(String, String, bool)> = sqlx::query_as(
        "SELECT id, password_hash, is_admin FROM users WHERE username = $1",
    )
    .bind(username)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (user_id, is_admin) = match user_row {
        Some((uid, stored_pw, admin)) => {
            if !stored_pw.is_empty() && stored_pw != payload.password {
                return Err((StatusCode::UNAUTHORIZED, "用户名或密码错误".to_string()));
            }
            (uid, admin)
        }
        None => {
            // 若库中尚无任何用户，自动初始化首个系统管理员账号
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
                .bind(&payload.password)
                .execute(&state.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                (new_id, true)
            } else {
                return Err((StatusCode::UNAUTHORIZED, "用户名或密码错误".to_string()));
            }
        }
    };

    if !is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            "无权登录：仅系统管理员权限角色可登录管理后台".to_string(),
        ));
    }

    let client_ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("Web 管理后台")
        .chars()
        .take(60)
        .collect::<String>();
    let stable_device_id = format!("admin-web-{}", user_id);

    let existing_token: Option<String> = sqlx::query_scalar(
        r#"
        SELECT token FROM access_tokens
        WHERE user_id = $1 AND client_name = 'RX-Emby Admin'
        ORDER BY last_used_at DESC
        LIMIT 1
        "#,
    )
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    let token = if let Some(tok) = existing_token {
        sqlx::query(
            "UPDATE access_tokens SET device_id = $2, device_name = $3, last_used_at = NOW() WHERE token = $1",
        )
        .bind(&tok)
        .bind(&stable_device_id)
        .bind(&client_ua)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let _ = sqlx::query(
            "DELETE FROM access_tokens WHERE user_id = $1 AND client_name = 'RX-Emby Admin' AND token <> $2",
        )
        .bind(&user_id)
        .bind(&tok)
        .execute(&state.db)
        .await;
        tok
    } else {
        let new_tok = Uuid::new_v4().simple().to_string();
        sqlx::query(
            "INSERT INTO access_tokens (token, user_id, device_id, device_name, client_name) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&new_tok)
        .bind(&user_id)
        .bind(&stable_device_id)
        .bind(&client_ua)
        .bind("RX-Emby Admin")
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        new_tok
    };

    Ok(Json(json!({
        "token": token,
        "user": {
            "id": user_id,
            "username": username,
            "is_admin": true
        }
    })))
}

pub(super) async fn admin_logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, String)> {
    if let Some(token) = extract_admin_token(&headers) {
        let _ = sqlx::query("DELETE FROM access_tokens WHERE token = $1")
            .bind(&token)
            .execute(&state.db)
            .await;
        crate::web::emby_api::invalidate_token_cache(&state, std::slice::from_ref(&token)).await;
    }
    Ok(Json(json!({ "status": "ok" })))
}

pub(super) async fn admin_me(Extension(user): Extension<AdminUser>) -> Json<Value> {
    Json(json!({ "user": user }))
}

