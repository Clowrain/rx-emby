use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Serialize, sqlx::FromRow)]
struct AdminUserRow {
    id: String,
    username: String,
    has_password: bool,
    is_admin: bool,
    allowed_categories: Vec<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    history_count: i64,
    favorite_count: i64,
}

#[derive(Serialize, sqlx::FromRow)]
struct AdminTokenRow {
    token: String,
    user_id: String,
    username: String,
    device_id: String,
    device_name: String,
    client_name: String,
    last_used_at: chrono::DateTime<chrono::Utc>,
}

pub(super) async fn list_users_and_sessions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    // 若库中尚无任何用户，自动初始化默认管理员账号
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);
    if count == 0 {
        let new_id = Uuid::new_v4().simple().to_string()[..16].to_string();
        let _ = sqlx::query(
            "INSERT INTO users (id, username, password_hash, is_admin) VALUES ($1, 'admin', '', TRUE) ON CONFLICT DO NOTHING",
        )
        .bind(&new_id)
        .execute(&state.db)
        .await;
    }

    let users: Vec<AdminUserRow> = sqlx::query_as(
        r#"
        SELECT u.id, u.username,
               (u.password_hash <> '') AS has_password,
               u.is_admin, u.allowed_categories, u.created_at,
               (SELECT COUNT(*) FROM play_histories ph WHERE ph.user_id = u.id) AS history_count,
               (SELECT COUNT(*) FROM user_favorites uf WHERE uf.user_id = u.id) AS favorite_count
        FROM users u
        ORDER BY u.is_admin DESC, u.created_at ASC
        "#,
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let tokens: Vec<AdminTokenRow> = sqlx::query_as(
        r#"
        SELECT t.token, t.user_id, u.username, t.device_id, t.device_name, t.client_name, t.last_used_at
        FROM access_tokens t
        JOIN users u ON u.id = t.user_id
        ORDER BY t.last_used_at DESC
        LIMIT 30
        "#,
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    state.prune_stale_sessions();

    let mut active_sessions = Vec::new();
    for entry in state.sessions.iter() {
        let s = entry.value().clone();
        let idle_sec = s.last_activity.elapsed().as_secs();
        let ep_label: Option<String> = match s.current_episode_id {
            Some(eid) if eid > 0 => {
                sqlx::query_scalar(
                    r#"
                    SELECT m.title || ' · S' || LPAD(e.season_number::text, 2, '0') || 'E' || LPAD(e.episode_number::text, 2, '0')
                    FROM episodes e
                    JOIN media_items m ON m.id = e.media_item_id
                    WHERE e.id = $1
                    "#,
                )
                .bind(eid)
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None)
            }
            Some(neg_mid) if neg_mid < 0 => {
                sqlx::query_scalar("SELECT title FROM media_items WHERE id = $1")
                    .bind(-neg_mid)
                    .fetch_optional(&state.db)
                    .await
                    .unwrap_or(None)
            }
            _ => None,
        };

        active_sessions.push(json!({
            "session_id": s.session_id,
            "user_id": s.user_id,
            "device_name": s.device_name,
            "client_name": s.client_name,
            "current_episode_id": s.current_episode_id,
            "playing_title": ep_label.unwrap_or_else(|| "未知剧集".to_string()),
            "position_sec": s.position_ticks / 10_000_000,
            "next_ep_prewarmed": s.next_ep_prewarmed,
            "idle_sec": idle_sec
        }));
    }

    Ok(Json(json!({
        "users": users,
        "tokens": tokens,
        "active_sessions": active_sessions
    })))
}

#[derive(Deserialize)]
pub(super) struct SaveUserReq {
    id: Option<String>,
    username: String,
    password: Option<String>,
    #[serde(default)]
    is_admin: bool,
    #[serde(default)]
    allowed_categories: Vec<String>,
}

pub(super) async fn save_user(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SaveUserReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uname = payload.username.trim();
    if uname.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "用户名不能为空".to_string()));
    }
    let cats = if payload.allowed_categories.is_empty() {
        vec!["*".to_string()]
    } else {
        payload.allowed_categories
    };

    if let Some(uid) = payload.id.as_deref().filter(|s| !s.is_empty()) {
        if let Some(pw) = payload.password.as_deref() {
            sqlx::query(
                "UPDATE users SET username = $1, password_hash = $2, is_admin = $3, allowed_categories = $4 WHERE id = $5",
            )
            .bind(uname)
            .bind(pw)
            .bind(payload.is_admin)
            .bind(&cats)
            .bind(uid)
            .execute(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        } else {
            sqlx::query(
                "UPDATE users SET username = $1, is_admin = $2, allowed_categories = $3 WHERE id = $4",
            )
            .bind(uname)
            .bind(payload.is_admin)
            .bind(&cats)
            .bind(uid)
            .execute(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
    } else {
        let new_id = Uuid::new_v4().simple().to_string()[..16].to_string();
        let pw = payload.password.unwrap_or_default();
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, is_admin, allowed_categories) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&new_id)
        .bind(uname)
        .bind(&pw)
        .bind(payload.is_admin)
        .bind(&cats)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("创建用户失败: {}", e)))?;
    }

    Ok(Json(json!({ "status": "ok", "message": "用户账号与权限配置已保存" })))
}

pub(super) async fn delete_user(
    State(state): State<Arc<AppState>>,
    Path(uid): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let is_target_admin: bool =
        sqlx::query_scalar("SELECT is_admin FROM users WHERE id = $1")
            .bind(&uid)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None)
            .unwrap_or(false);

    if is_target_admin {
        let admin_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE is_admin = TRUE")
                .fetch_one(&state.db)
                .await
                .unwrap_or(1);
        if admin_count <= 1 {
            return Err((StatusCode::BAD_REQUEST, "不能删除系统唯一的管理员账号".to_string()));
        }
    }

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(&uid)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state.sessions.retain(|_, s| s.user_id != uid);
    state.caches.admin_stats_cache.invalidate_all();

    Ok(Json(json!({ "status": "ok", "message": "用户已删除" })))
}

pub(super) async fn revoke_access_token(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let deleted: Option<(String, String)> = sqlx::query_as(
        "DELETE FROM access_tokens WHERE token = $1 RETURNING user_id, device_name",
    )
    .bind(&token)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some((uid, dev)) = deleted {
        state
            .sessions
            .retain(|_, s| !(s.user_id == uid && s.device_name == dev));
    }
    state.caches.admin_stats_cache.invalidate_all();

    Ok(Json(json!({ "status": "ok", "message": "已注销该客户端设备" })))
}

