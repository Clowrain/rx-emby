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

async fn password_matches(stored: String, password: String) -> Result<bool, (StatusCode, String)> {
    crate::web::password::verify_password_blocking(stored, password).await
}

#[derive(Clone, Debug, Serialize)]
pub struct AdminUser {
    pub id: String,
    pub username: String,
    pub is_admin: bool,
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

fn extract_admin_token(headers: &HeaderMap) -> Option<String> {
    if let Some(auth) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
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
    let token = extract_admin_token(req.headers()).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            "未登录，请先使用系统管理员账号登录".to_string(),
        )
    })?;

    let identity = state
        .caches
        .admin_token_user_cache
        .try_get_with(token.clone(), async {
            let row: Option<(String, String, bool)> = sqlx::query_as(
                "SELECT u.id, u.username, u.is_admin FROM access_tokens t JOIN users u ON u.id=t.user_id WHERE t.token=$1",
            )
            .bind(&token)
            .fetch_optional(&state.db)
            .await?;
            row.map(Arc::new).ok_or(sqlx::Error::RowNotFound)
        })
        .await
        .map_err(|error| {
            if matches!(error.as_ref(), sqlx::Error::RowNotFound) {
                (StatusCode::UNAUTHORIZED, "登录凭证无效或已过期，请重新登录".to_string())
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
            }
        })?;
    let (user_id, username, is_admin) = (identity.0.clone(), identity.1.clone(), identity.2);

    if !is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            "无权访问：仅系统管理员权限角色可访问管理后台".to_string(),
        ));
    }

    let touch = state
        .caches
        .token_touch_debounce
        .try_get_with(token.clone(), async {
            sqlx::query("UPDATE access_tokens SET last_used_at = NOW() WHERE token = $1")
                .bind(&token)
                .execute(&state.db)
                .await?;
            Ok::<(), sqlx::Error>(())
        })
        .await;
    if let Err(error) = touch {
        tracing::warn!("更新管理 API 访问令牌 last_used_at 失败: {}", error);
    }

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
    peer: Option<axum::extract::ConnectInfo<std::net::SocketAddr>>,
    Json(payload): Json<AdminLoginReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let username = payload.username.trim();
    if username.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "请输入用户名".to_string()));
    }
    if payload.password.is_empty() {
        return Err((StatusCode::UNAUTHORIZED, "用户名或密码错误".to_string()));
    }

    let client_ip = client_ip(&headers, peer.map(|axum::extract::ConnectInfo(addr)| addr));
    if state.login_rate_limited(username, &client_ip).await {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            "登录失败次数过多，请稍后重试".to_string(),
        ));
    }
    let user_row: Option<(String, String, bool)> =
        sqlx::query_as("SELECT id, password_hash, is_admin FROM users WHERE username = $1")
            .bind(username)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (user_id, is_admin) = if let Some((uid, stored_pw, admin)) = user_row {
        if stored_pw.is_empty()
            || !password_matches(stored_pw.clone(), payload.password.clone()).await?
        {
            state.record_failed_login(username, &client_ip).await;
            return Err((StatusCode::UNAUTHORIZED, "用户名或密码错误".to_string()));
        }
        if !stored_pw.starts_with("$pbkdf2-sha256$") {
            let upgraded =
                crate::web::password::hash_password_blocking(payload.password.clone()).await?;
            sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
                .bind(upgraded)
                .bind(&uid)
                .execute(&state.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
        (uid, admin)
    } else {
        (String::new(), false)
    };

    let (user_id, is_admin) = if user_id.is_empty() {
        let users_exist: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users)")
            .fetch_one(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if users_exist {
            state.record_failed_login(username, &client_ip).await;
            return Err((StatusCode::UNAUTHORIZED, "用户名或密码错误".to_string()));
        }
        let candidate_hash =
            crate::web::password::hash_password_blocking(payload.password.clone()).await?;
        let mut tx = state
            .db
            .begin()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        sqlx::query("SELECT pg_advisory_xact_lock(724981662)")
            .execute(&mut *tx)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if count != 0 {
            return Err((StatusCode::UNAUTHORIZED, "用户名或密码错误".to_string()));
        }
        let new_id = Uuid::new_v4().simple().to_string()[..16].to_string();
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, is_admin) VALUES ($1, $2, $3, TRUE)",
        )
        .bind(&new_id)
        .bind(username)
        .bind(candidate_hash)
        .execute(&mut *tx)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        (new_id, true)
    } else {
        (user_id, is_admin)
    };

    if !is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            "无权登录：仅系统管理员权限角色可登录管理后台".to_string(),
        ));
    }

    state
        .caches
        .failed_login_attempts
        .invalidate(&format!("user:{}", username.to_lowercase()))
        .await;
    let client_ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("Web 管理后台")
        .chars()
        .take(60)
        .collect::<String>();
    let stable_device_id = format!("admin-web-{}", user_id);

    let removed_tokens: Vec<String> = sqlx::query_scalar(
        "DELETE FROM access_tokens WHERE user_id = $1 AND client_name = 'RX-Emby Admin' RETURNING token",
    )
    .bind(&user_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::web::emby_api::invalidate_token_cache(&state, &removed_tokens).await;
    let token = Uuid::new_v4().simple().to_string();
    sqlx::query(
        "INSERT INTO access_tokens (token, user_id, device_id, device_name, client_name) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&token)
    .bind(&user_id)
    .bind(&stable_device_id)
    .bind(&client_ua)
    .bind("RX-Emby Admin")
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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

#[cfg(test)]
mod tests {
    use super::{client_ip, password_matches};

    #[test]
    fn client_ip_uses_connect_info_without_proxy_headers() {
        let ip = client_ip(
            &axum::http::HeaderMap::new(),
            Some("192.0.2.8:1234".parse().unwrap()),
        );
        assert_eq!(ip, "192.0.2.8");
    }

    #[tokio::test]
    async fn admin_login_password_check_accepts_pbkdf2_hashes() {
        let encoded = crate::web::password::hash_password_blocking("admin-secret".into())
            .await
            .unwrap();
        assert!(password_matches(encoded.clone(), "admin-secret".into())
            .await
            .unwrap());
        assert!(!password_matches(encoded, "wrong".into()).await.unwrap());
    }
}
