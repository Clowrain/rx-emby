use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest;
use std::sync::Arc;
use uuid::Uuid;

async fn invalidate_user_categories_cache(
    cache: &moka::future::Cache<String, Option<Vec<String>>>,
    uid: &str,
) {
    cache.invalidate(uid).await;
}

async fn invalidate_user_permission_caches(state: &AppState, uid: &str, tokens: &[String]) {
    crate::web::emby_api::invalidate_token_cache(state, tokens).await;
    invalidate_user_categories_cache(&state.caches.user_categories_cache, uid).await;
}

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

#[derive(sqlx::FromRow)]
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
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let sessions: Vec<_> = state
        .sessions
        .iter()
        .filter(|entry| entry.is_active())
        .map(|entry| entry.value().clone())
        .collect();
    let file_ids: Vec<i64> = sessions
        .iter()
        .filter_map(|s| s.current_episode_id.filter(|id| *id > 0))
        .collect();
    let work_ids: Vec<i64> = sessions
        .iter()
        .filter_map(|s| s.current_episode_id.filter(|id| *id < 0).map(|id| -id))
        .collect();
    let file_labels: std::collections::HashMap<i64, String> = sqlx::query_as(
        r#"SELECT sf.id, COALESCE(w.title, f.clean_title, '未知') || ' · S' || LPAD(sf.season_number::text, 2, '0') || 'E' || LPAD(sf.episode_number::text, 2, '0')
           FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id LEFT JOIN media_works w ON w.id = f.work_id
           WHERE sf.id = ANY($1)"#,
    ).bind(&file_ids).fetch_all(&state.db).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?.into_iter().collect();
    let work_labels: std::collections::HashMap<i64, String> =
        sqlx::query_as("SELECT id, title FROM media_works WHERE id = ANY($1)")
            .bind(&work_ids)
            .fetch_all(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .into_iter()
            .collect();
    let mut active_sessions = Vec::new();
    for s in sessions {
        let idle_sec = s.last_activity.elapsed().as_secs();
        let ep_label = match s.current_episode_id {
            Some(id) if id > 0 => file_labels.get(&id).cloned(),
            Some(id) if id < 0 => work_labels.get(&(-id)).cloned(),
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

    let tokens: Vec<Value> = tokens
        .into_iter()
        .map(|t| {
            let digest = sha2::Sha256::digest(t.token.as_bytes());
            let token_id = digest
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            json!({
                "token_id": token_id,
                "token_preview": format!("{}…", &t.token.chars().take(8).collect::<String>()),
                "user_id": t.user_id,
                "username": t.username,
                "device_id": t.device_id,
                "device_name": t.device_name,
                "client_name": t.client_name,
                "last_used_at": t.last_used_at
            })
        })
        .collect();

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
    if payload.password.as_deref() == Some("") {
        return Err((StatusCode::BAD_REQUEST, "密码不能为空".to_string()));
    }
    if payload.id.as_deref().is_none_or(str::is_empty)
        && payload.is_admin
        && payload.password.as_deref().is_none_or(|p| p.is_empty())
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "管理员账号必须设置非空密码".to_string(),
        ));
    }
    let cats = if payload.allowed_categories.is_empty() && payload.is_admin {
        vec!["*".to_string()]
    } else {
        payload.allowed_categories
    };

    if let Some(uid) = payload.id.as_deref().filter(|s| !s.is_empty()) {
        let was_admin: bool = sqlx::query_scalar("SELECT is_admin FROM users WHERE id = $1")
            .bind(uid)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .ok_or((StatusCode::NOT_FOUND, "用户不存在".to_string()))?;
        if let Some(pw) = payload.password.as_deref() {
            let pw_hash = crate::web::password::hash_password_blocking(pw.to_owned()).await?;
            sqlx::query(
                "UPDATE users SET username = $1, password_hash = $2, is_admin = $3, allowed_categories = $4 WHERE id = $5",
            )
            .bind(uname)
            .bind(&pw_hash)
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
        let revoke_sessions = payload.password.is_some() || (was_admin && !payload.is_admin);
        if revoke_sessions {
            let tokens: Vec<String> =
                sqlx::query_scalar("DELETE FROM access_tokens WHERE user_id = $1 RETURNING token")
                    .bind(uid)
                    .fetch_all(&state.db)
                    .await
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            invalidate_user_permission_caches(&state, uid, &tokens).await;
            state.sessions.retain(|_, session| session.user_id != uid);
            state
                .session_hub
                .broadcast(Some(uid), "Sessions", json!([]));
        } else {
            state.caches.user_categories_cache.invalidate(uid).await;
            let tokens: Vec<String> =
                sqlx::query_scalar("SELECT token FROM access_tokens WHERE user_id = $1")
                    .bind(uid)
                    .fetch_all(&state.db)
                    .await
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            for token in tokens {
                state.caches.token_user_cache.invalidate(&token).await;
            }
        }
    } else {
        let new_id = Uuid::new_v4().simple().to_string()[..16].to_string();
        let pw = match payload.password {
            Some(password) => crate::web::password::hash_password_blocking(password).await?,
            None => String::new(),
        };
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

    state.caches.admin_stats_cache.invalidate_all();

    Ok(Json(
        json!({ "status": "ok", "message": "用户账号与权限配置已保存" }),
    ))
}

fn can_delete_admin(is_target_admin: bool, admin_count: i64) -> bool {
    !is_target_admin || admin_count > 1
}

pub(super) async fn delete_user(
    State(state): State<Arc<AppState>>,
    Path(uid): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Lock the administrator rows before counting/deleting so concurrent requests
    // cannot both observe themselves as non-final administrator deletions.
    let admin_ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM users WHERE is_admin = TRUE ORDER BY id FOR UPDATE")
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let is_target_admin = admin_ids.iter().any(|id| id == &uid);
    if !can_delete_admin(is_target_admin, admin_ids.len() as i64) {
        return Err((
            StatusCode::BAD_REQUEST,
            "不能删除系统唯一的管理员账号".to_string(),
        ));
    }

    let user_tokens: Vec<String> =
        sqlx::query_scalar("SELECT token FROM access_tokens WHERE user_id = $1")
            .bind(&uid)
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(&uid)
        .execute(&mut *tx)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state.sessions.retain(|_, s| s.user_id != uid);
    for token in &user_tokens {
        state.client_capabilities.invalidate(token);
    }
    invalidate_user_permission_caches(&state, &uid, &user_tokens).await;
    state
        .session_hub
        .broadcast(Some(&uid), "Sessions", json!([]));
    state.caches.admin_stats_cache.invalidate_all();

    Ok(Json(json!({ "status": "ok", "message": "用户已删除" })))
}

pub(super) async fn revoke_access_token(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let deleted: Option<(String, String, String)> = sqlx::query_as(
        "DELETE FROM access_tokens WHERE encode(sha256(convert_to(token, 'UTF8')), 'hex') = $1 RETURNING token, user_id, device_id",
    )
    .bind(&token)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some((raw_token, uid, device_id)) = deleted {
        state
            .sessions
            .retain(|_, session| session.user_id != uid || session.device_id != device_id);
        state.client_capabilities.invalidate(&raw_token);
        crate::web::emby_api::invalidate_token_cache(&state, std::slice::from_ref(&raw_token))
            .await;
        let remaining_sessions: Vec<Value> = state
            .sessions
            .iter()
            .filter(|entry| entry.is_active() && entry.user_id == uid)
            .map(|entry| {
                let session = entry.value();
                json!({
                    "Id": session.session_id,
                    "UserId": session.user_id,
                    "Client": session.client_name,
                    "DeviceId": session.device_id,
                    "DeviceName": session.device_name,
                    "SupportsRemoteControl": false,
                    "PlayState": {
                        "PositionTicks": session.position_ticks,
                        "CanSeek": true,
                        "IsPaused": false,
                        "IsMuted": false
                    }
                })
            })
            .collect();
        state
            .session_hub
            .broadcast(Some(&uid), "Sessions", json!(remaining_sessions));
    }
    state.caches.admin_stats_cache.invalidate_all();

    Ok(Json(
        json!({ "status": "ok", "message": "已注销该客户端设备" }),
    ))
}

#[cfg(test)]
mod cache_tests {
    use super::{can_delete_admin, invalidate_user_categories_cache};

    #[test]
    fn deleting_the_last_admin_is_rejected() {
        assert!(!can_delete_admin(true, 1));
        assert!(can_delete_admin(true, 2));
        assert!(can_delete_admin(false, 0));
    }

    #[tokio::test]
    async fn invalidating_one_user_keeps_other_users_categories_cached() {
        let cache: moka::future::Cache<String, Option<Vec<String>>> = moka::future::Cache::new(4);
        cache.insert("user-a".into(), Some(vec!["A".into()])).await;
        cache.insert("user-b".into(), Some(vec!["B".into()])).await;

        invalidate_user_categories_cache(&cache, "user-a").await;

        assert!(cache.get(&"user-a".to_string()).await.is_none());
        assert_eq!(
            cache.get(&"user-b".to_string()).await,
            Some(Some(vec!["B".into()]))
        );
    }
}
