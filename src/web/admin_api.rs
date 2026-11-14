use crate::{
    config::BootstrapFileConfig,
    image_cache::enforce_image_cache_quota,
    scraper::scrape_single_media,
    state::AppState,
    web::emby_api::{
        count_virtual_library_matches, resolve_virtual_library_cover_key, VirtualLibraryRule,
    },
};
use axum::{
    extract::{Path, Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{delete, get, post},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};
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

async fn require_admin_auth(
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
struct AdminLoginReq {
    username: String,
    #[serde(default)]
    password: String,
}

async fn admin_login(
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

    let token = Uuid::new_v4().simple().to_string();
    let client_ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("Web 管理后台")
        .chars()
        .take(60)
        .collect::<String>();

    sqlx::query(
        "INSERT INTO access_tokens (token, user_id, device_id, device_name, client_name) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&token)
    .bind(&user_id)
    .bind(format!("admin-{}", &token[..8]))
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

async fn admin_logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, String)> {
    if let Some(token) = extract_admin_token(&headers) {
        let _ = sqlx::query("DELETE FROM access_tokens WHERE token = $1")
            .bind(&token)
            .execute(&state.db)
            .await;
    }
    Ok(Json(json!({ "status": "ok" })))
}

async fn admin_me(Extension(user): Extension<AdminUser>) -> Json<Value> {
    Json(json!({ "user": user }))
}

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let public_auth = Router::new()
        .route("/api/admin/auth/login", post(admin_login))
        .route("/api/admin/auth/logout", post(admin_logout));

    let protected = Router::new()
        .route("/api/admin/auth/me", get(admin_me))
        .route("/api/admin/overview", get(get_admin_overview))
        .route("/api/admin/configs", get(list_configs).post(update_configs))
        .route("/api/admin/sync/trigger", post(trigger_sync))
        .route("/api/admin/sync/logs", get(list_sync_logs))
        .route("/api/admin/cache/stats", get(image_cache_stats))
        .route("/api/admin/cache/evict", post(trigger_cache_evict))
        .route("/api/admin/media", get(list_admin_media))
        .route("/api/admin/media/:id/detail", get(get_admin_media_detail))
        .route("/api/admin/media/:id/prewarm_dirs", post(prewarm_admin_media_dirs))
        .route("/api/admin/media/:id/bind_tmdb", post(bind_tmdb_id))
        .route("/api/admin/media/:id/scrape", post(scrape_one_now))
        .route("/api/admin/media/:id/ignore", post(toggle_ignore_media))
        .route("/api/admin/scrape/retry_failed", post(retry_failed_scrapes))
        .route(
            "/api/admin/virtual_libraries",
            get(list_virtual_libraries).post(save_virtual_library),
        )
        .route("/api/admin/virtual_libraries/options", get(smart_view_options))
        .route("/api/admin/virtual_libraries/preview", post(preview_smart_view))
        .route("/api/admin/virtual_libraries/backfill", post(queue_metadata_backfill))
        .route(
            "/api/admin/virtual_libraries/presets",
            post(seed_preset_virtual_libraries),
        )
        .route(
            "/api/admin/virtual_libraries/:id",
            delete(delete_virtual_library),
        )
        .route(
            "/api/admin/base_libraries/covers",
            get(list_base_library_covers).post(save_base_library_cover),
        )
        .route("/api/admin/users", get(list_users_and_sessions).post(save_user))
        .route("/api/admin/users/:id", delete(delete_user))
        .route("/api/admin/tokens/:token", delete(revoke_access_token))
        .route_layer(middleware::from_fn_with_state(state, require_admin_auth));

    Router::new().merge(public_auth).merge(protected)
}

#[derive(Serialize, sqlx::FromRow)]
struct ConfigRow {
    config_key: String,
    config_value: String,
    value_type: String,
    description: Option<String>,
}

async fn list_configs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mut rows: Vec<ConfigRow> = sqlx::query_as(
        "SELECT config_key, config_value, value_type, description FROM system_configs ORDER BY config_key ASC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    for r in &mut rows {
        if r.value_type == "secret" && !r.config_value.is_empty() {
            r.config_value = "********".to_string();
        }
    }

    Ok(Json(json!({ "configs": rows })))
}

#[derive(Deserialize)]
struct UpdateConfigsReq {
    updates: HashMap<String, String>,
}

async fn update_configs(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UpdateConfigsReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    for (k, v) in payload.updates {
        if v == "********" {
            continue;
        }
        sqlx::query(
            "UPDATE system_configs SET config_value = $1, updated_at = NOW() WHERE config_key = $2",
        )
        .bind(&v)
        .bind(&k)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    let base_cfg = BootstrapFileConfig::load_with_env("config.toml")
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let merged = base_cfg
        .into_dynamic()
        .sync_with_db(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state
        .reload_config(merged)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(json!({ "status": "ok", "message": "设置已保存并立即生效" })))
}

async fn trigger_sync(State(state): State<Arc<AppState>>) -> Json<Value> {
    state.sync_ctrl.manual_trigger.notify_one();
    Json(json!({ "status": "triggered", "message": "已启动后台媒体库同步任务" }))
}

#[derive(Serialize, sqlx::FromRow)]
struct SyncLogRow {
    id: i64,
    started_at: chrono::DateTime<chrono::Utc>,
    finished_at: Option<chrono::DateTime<chrono::Utc>>,
    duration_ms: Option<i64>,
    total_lines: i32,
    added_episodes: i32,
    deleted_episodes: i32,
    new_media_items: i32,
    status: String,
    error_message: Option<String>,
}

async fn list_sync_logs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let logs: Vec<SyncLogRow> = sqlx::query_as(
        "SELECT id, started_at, finished_at, duration_ms, total_lines, added_episodes, deleted_episodes, new_media_items, status, error_message FROM sync_logs ORDER BY id DESC LIMIT 30",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "logs": logs })))
}

async fn image_cache_stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let cfg = state.config.load();
    let (file_count, total_bytes): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0)::bigint FROM image_cache_index",
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let max_bytes = (cfg.image_max_size_mb * 1024 * 1024) as f64;
    let usage_ratio = if max_bytes > 0.0 {
        (total_bytes as f64 / max_bytes) * 100.0
    } else {
        0.0
    };

    Ok(Json(json!({
        "file_count": file_count,
        "used_mb": total_bytes / 1024 / 1024,
        "max_mb": cfg.image_max_size_mb,
        "usage_percent": format!("{:.2}%", usage_ratio),
        "high_watermark": cfg.image_evict_high_watermark,
        "low_watermark": cfg.image_evict_low_watermark
    })))
}

async fn trigger_cache_evict(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let freed = enforce_image_cache_quota(&state)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({
        "status": "ok",
        "freed_mb": freed / 1024 / 1024
    })))
}

#[derive(Deserialize, Default)]
struct AdminMediaQuery {
    scrape_status: Option<i16>,
    category: Option<String>,
    keyword: Option<String>,
    is_ignored: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
struct AdminMediaRow {
    id: i64,
    media_type: String,
    tmdb_id: Option<i32>,
    title: String,
    year: Option<i32>,
    rating: f32,
    category: String,
    scrape_status: i16,
    is_locked: bool,
    is_ignored: bool,
    root_dir: String,
    poster_path: Option<String>,
    episode_count: i64,
    sibling_count: i64,
}

async fn get_admin_overview(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let cfg = state.config.load();

    // 1. 统计去重后在库影视总数及各刮削状态占比
    let (total_media, scraped_count, pending_count, failed_count, ignored_count): (i64, i64, i64, i64, i64) =
        sqlx::query_as(
            r#"
            SELECT
                COUNT(DISTINCT CASE WHEN NOT is_ignored THEN COALESCE(tmdb_id::text || ':' || media_type, 'raw:' || id::text) END),
                COUNT(DISTINCT CASE WHEN NOT is_ignored AND scrape_status IN (1, 3) THEN COALESCE(tmdb_id::text || ':' || media_type, 'raw:' || id::text) END),
                COUNT(DISTINCT CASE WHEN NOT is_ignored AND scrape_status = 0 THEN COALESCE(tmdb_id::text || ':' || media_type, 'raw:' || id::text) END),
                COUNT(DISTINCT CASE WHEN NOT is_ignored AND scrape_status = 2 THEN COALESCE(tmdb_id::text || ':' || media_type, 'raw:' || id::text) END),
                COUNT(DISTINCT CASE WHEN is_ignored THEN COALESCE(tmdb_id::text || ':' || media_type, 'raw:' || id::text) END)
            FROM media_items
            "#,
        )
        .fetch_one(&state.db)
        .await
        .unwrap_or((0, 0, 0, 0, 0));

    let total_episodes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM episodes e JOIN media_items m ON m.id = e.media_item_id WHERE m.is_ignored = FALSE",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    // 2. 五大分类统计
    let cat_rows: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT category, COUNT(DISTINCT COALESCE(tmdb_id::text || ':' || media_type, 'raw:' || id::text))
        FROM media_items
        WHERE is_ignored = FALSE
        GROUP BY category
        ORDER BY 2 DESC
        "#,
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let categories: Vec<Value> = cat_rows
        .into_iter()
        .map(|(cat, count)| json!({ "category": cat, "count": count }))
        .collect();

    // 3. 图片缓存与最近同步
    let (cache_files, cache_bytes): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0)::bigint FROM image_cache_index",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or((0, 0));

    let last_sync: Option<SyncLogRow> = sqlx::query_as(
        "SELECT id, started_at, finished_at, duration_ms, total_lines, added_episodes, deleted_episodes, new_media_items, status, error_message FROM sync_logs ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    let vlib_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM virtual_libraries WHERE is_enabled = TRUE")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db)
        .await
        .unwrap_or(1);

    Ok(Json(json!({
        "media": {
            "total_media": total_media,
            "total_episodes": total_episodes,
            "scraped_count": scraped_count,
            "pending_count": pending_count,
            "failed_count": failed_count,
            "ignored_count": ignored_count,
            "categories": categories
        },
        "cache": {
            "file_count": cache_files,
            "used_mb": cache_bytes / 1024 / 1024,
            "max_mb": cfg.image_max_size_mb
        },
        "system": {
            "server_name": cfg.server_name,
            "server_id": cfg.server_id,
            "alist_base_url": cfg.alist_base_url,
            "auto_background_scraper": cfg.scraper_auto_background,
            "tmdb_rate_limit": cfg.tmdb_rate_limit_per_sec,
            "active_sessions": state.sessions.len(),
            "vlib_count": vlib_count,
            "user_count": user_count
        },
        "last_sync": last_sync
    })))
}

async fn list_admin_media(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AdminMediaQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let limit = q.limit.unwrap_or(30).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);
    let ignored_filter = q.is_ignored.unwrap_or(false);
    let kw = q
        .keyword
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{}%", s));

    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(DISTINCT COALESCE(m.tmdb_id::text || ':' || m.media_type, 'raw:' || m.id::text))
        FROM media_items m
        WHERE m.is_ignored = $1
          AND ($2::int2 IS NULL OR m.scrape_status = $2)
          AND ($3::text IS NULL OR m.category = $3)
          AND ($4::text IS NULL OR m.title ILIKE $4 OR m.root_dir ILIKE $4)
        "#,
    )
    .bind(ignored_filter)
    .bind(q.scrape_status)
    .bind(&q.category)
    .bind(&kw)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let items: Vec<AdminMediaRow> = sqlx::query_as(
        r#"
        SELECT
            id, media_type, tmdb_id, title, year, rating, category,
            scrape_status, is_locked, is_ignored, root_dir, poster_path,
            episode_count, sibling_count
        FROM (
            SELECT DISTINCT ON (COALESCE(m.tmdb_id::text || ':' || m.media_type, 'raw:' || m.id::text))
                m.id, m.media_type, m.tmdb_id, m.title, m.year, m.rating, m.category,
                m.scrape_status, m.is_locked, m.is_ignored, m.root_dir, m.poster_path, m.updated_at,
                CASE
                    WHEN m.tmdb_id IS NOT NULL THEN (
                        SELECT COUNT(*) FROM episodes e
                        JOIN media_items sm ON sm.id = e.media_item_id
                        WHERE sm.tmdb_id = m.tmdb_id AND sm.media_type = m.media_type AND sm.is_ignored = m.is_ignored
                    )
                    ELSE (SELECT COUNT(*) FROM episodes e WHERE e.media_item_id = m.id)
                END AS episode_count,
                CASE
                    WHEN m.tmdb_id IS NOT NULL THEN (
                        SELECT COUNT(*) FROM media_items s
                        WHERE s.tmdb_id = m.tmdb_id AND s.media_type = m.media_type AND s.is_ignored = m.is_ignored
                    )
                    ELSE 1
                END AS sibling_count
            FROM media_items m
            WHERE m.is_ignored = $1
              AND ($2::int2 IS NULL OR m.scrape_status = $2)
              AND ($3::text IS NULL OR m.category = $3)
              AND ($4::text IS NULL OR m.title ILIKE $4 OR m.root_dir ILIKE $4)
            ORDER BY
                COALESCE(m.tmdb_id::text || ':' || m.media_type, 'raw:' || m.id::text),
                (m.scrape_status IN (1, 3)) DESC,
                (m.poster_path IS NOT NULL) DESC,
                m.id ASC
        ) AS deduped
        ORDER BY updated_at DESC, id DESC
        LIMIT $5 OFFSET $6
        "#,
    )
    .bind(ignored_filter)
    .bind(q.scrape_status)
    .bind(&q.category)
    .bind(&kw)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "items": items, "total": total })))
}

#[derive(Serialize, sqlx::FromRow)]
struct AdminMediaFullRow {
    id: i64,
    media_type: String,
    tmdb_id: Option<i32>,
    title: String,
    original_title: Option<String>,
    year: Option<i32>,
    premiered_at: Option<String>,
    overview: Option<String>,
    rating: f32,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    countries: Vec<String>,
    networks: Vec<String>,
    genres: Vec<String>,
    category: String,
    scrape_status: i16,
    is_locked: bool,
    is_ignored: bool,
    root_dir: String,
}

#[derive(Serialize, sqlx::FromRow)]
struct AdminSiblingDirRow {
    id: i64,
    root_dir: String,
    scrape_status: i16,
    episode_count: i64,
}

#[derive(Serialize, sqlx::FromRow)]
struct AdminEpisodeDetailRow {
    id: i64,
    media_item_id: i64,
    season_number: i32,
    episode_number: i32,
    title: Option<String>,
    overview: Option<String>,
    still_path: Option<String>,
    air_date: Option<String>,
    runtime: i32,
    meta_incomplete: bool,
    version_tag: String,
    priority_score: i32,
    container: String,
    file_name: String,
    alist_path: String,
    file_size_bytes: Option<i64>,
}

/// 获取某部影视的完整详情、跨目录同 TMDB 聚合目录列表以及所有分集多版本源明细
async fn get_admin_media_detail(
    State(state): State<Arc<AppState>>,
    Path(media_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let media: Option<AdminMediaFullRow> = sqlx::query_as(
        r#"
        SELECT id, media_type, tmdb_id, title, original_title, year, premiered_at,
               overview, rating, poster_path, backdrop_path, countries, networks,
               genres, category, scrape_status, is_locked, is_ignored, root_dir
        FROM media_items
        WHERE id = $1
        "#,
    )
    .bind(media_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some(media) = media else {
        return Err((StatusCode::NOT_FOUND, "影视条目不存在".to_string()));
    };

    if media.scrape_status != 1 || media.media_type == "tv" {
        let _ = state.scraper.priority_tx.try_send(media_id);
    }

    let sibling_ids = state.resolve_sibling_media_ids(media_id).await;

    let sibling_dirs: Vec<AdminSiblingDirRow> = sqlx::query_as(
        r#"
        SELECT m.id, m.root_dir, m.scrape_status,
               (SELECT COUNT(*) FROM episodes e WHERE e.media_item_id = m.id) AS episode_count
        FROM media_items m
        WHERE m.id = ANY($1)
        ORDER BY (m.id = $2) DESC, m.id ASC
        "#,
    )
    .bind(&sibling_ids)
    .bind(media_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut episodes: Vec<AdminEpisodeDetailRow> = sqlx::query_as(
        r#"
        SELECT id, media_item_id, season_number, episode_number, title,
               overview, still_path, air_date, runtime, meta_incomplete,
               version_tag, priority_score, container, file_name, alist_path, file_size_bytes
        FROM episodes
        WHERE media_item_id = ANY($1)
        ORDER BY season_number ASC, episode_number ASC, priority_score DESC, id ASC
        "#,
    )
    .bind(&sibling_ids)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // 按同集 (season_number, episode_number) 分组执行多维标签重算与重名自动消歧
    let mut idx = 0;
    while idx < episodes.len() {
        let s_num = episodes[idx].season_number;
        let e_num = episodes[idx].episode_number;
        let mut end = idx + 1;
        while end < episodes.len()
            && episodes[end].season_number == s_num
            && episodes[end].episode_number == e_num
        {
            end += 1;
        }

        let slice = &mut episodes[idx..end];
        let mut tuples: Vec<(String, i32, String, String)> = slice
            .iter()
            .map(|r| {
                (
                    r.version_tag.clone(),
                    r.priority_score,
                    r.alist_path.clone(),
                    r.container.clone(),
                )
            })
            .collect();
        crate::sync::parser::disambiguate_version_tags(&mut tuples);
        for (ep, (new_tag, new_score, _, _)) in slice.iter_mut().zip(tuples.into_iter()) {
            ep.version_tag = new_tag;
            ep.priority_score = new_score;
        }
        slice.sort_by(|a, b| b.priority_score.cmp(&a.priority_score).then(a.id.cmp(&b.id)));
        idx = end;
    }

    Ok(Json(json!({
        "media": media,
        "sibling_dirs": sibling_dirs,
        "episodes": episodes
    })))
}

/// 手动触发该影视小雅父目录 `fs/list` 寻址预热并回填文件体积
async fn prewarm_admin_media_dirs(
    State(state): State<Arc<AppState>>,
    Path(media_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.spawn_prewarm_media_dirs(media_id, None);
    Ok(Json(json!({
        "status": "ok",
        "message": "已触发小雅目录寻址预热与文件体积回填，请稍候刷新详情查看"
    })))
}

#[derive(Deserialize)]
struct BindTmdbReq {
    tmdb_id: i32,
    media_type: Option<String>,
}

async fn bind_tmdb_id(
    State(state): State<Arc<AppState>>,
    Path(media_id): Path<i64>,
    Json(payload): Json<BindTmdbReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    sqlx::query(
        r#"
        UPDATE media_items SET
            tmdb_id = $1,
            media_type = COALESCE($2, media_type),
            is_locked = FALSE,
            scrape_status = 0
        WHERE id = $3
        "#,
    )
    .bind(payload.tmdb_id)
    .bind(&payload.media_type)
    .bind(media_id)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let sibling_ids = state.resolve_sibling_media_ids(media_id).await;
    let _ = sqlx::query("UPDATE seasons SET scrape_attempts = 0 WHERE media_item_id = ANY($1)")
        .bind(&sibling_ids)
        .execute(&state.db)
        .await;

    scrape_single_media(&state, media_id)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("绑定后刮削 TMDB 失败: {:#}", e)))?;

    sqlx::query("UPDATE media_items SET is_locked = TRUE, scrape_status = 3 WHERE id = $1")
        .bind(media_id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "status": "ok", "message": "已成功绑定 TMDB ID 并刷新锁定主条目与分集元数据" })))
}

/// 单部影视立即实时刮削（不开启全库后台扫描时用于手动测试单条，同时强制刷新其下季与分集）
async fn scrape_one_now(
    State(state): State<Arc<AppState>>,
    Path(media_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let sibling_ids = state.resolve_sibling_media_ids(media_id).await;
    sqlx::query("UPDATE media_items SET is_locked = FALSE, scrape_status = 0 WHERE id = ANY($1)")
        .bind(&sibling_ids)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let _ = sqlx::query("UPDATE seasons SET scrape_attempts = 0 WHERE media_item_id = ANY($1)")
        .bind(&sibling_ids)
        .execute(&state.db)
        .await;

    scrape_single_media(&state, media_id)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("单条刮削失败: {:#}", e)))?;

    Ok(Json(json!({ "status": "ok", "message": "单条及分集元数据刮削成功" })))
}

#[derive(Deserialize)]
struct ToggleIgnoreReq {
    ignore: bool,
}

/// 将不想要的影视移出或恢复到媒体库（软屏蔽，增量同步不会重新导入；同时联动同 tmdb_id 兄弟目录）
async fn toggle_ignore_media(
    State(state): State<Arc<AppState>>,
    Path(media_id): Path<i64>,
    Json(payload): Json<ToggleIgnoreReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    sqlx::query(
        r#"
        UPDATE media_items AS target
        SET is_ignored = $1, updated_at = NOW()
        FROM (SELECT id, tmdb_id, media_type FROM media_items WHERE id = $2) AS anchor
        WHERE target.id = anchor.id
           OR (anchor.tmdb_id IS NOT NULL AND target.tmdb_id = anchor.tmdb_id AND target.media_type = anchor.media_type)
        "#,
    )
    .bind(payload.ignore)
    .bind(media_id)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({
        "status": "ok",
        "message": if payload.ignore { "已屏蔽该影视，后续同步将自动跳过（可在「已屏蔽」中恢复）" } else { "已恢复显示到媒体库" }
    })))
}

async fn retry_failed_scrapes(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let res = sqlx::query(
        "UPDATE media_items SET scrape_status = 0, retry_count = 0, next_retry_at = NULL WHERE scrape_status = 2 AND is_locked = FALSE",
    )
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({
        "status": "ok",
        "reset_count": res.rows_affected(),
        "message": format!("已将 {} 部未匹配影视重新加入待刮削队列", res.rows_affected())
    })))
}

#[derive(Serialize, sqlx::FromRow)]
struct VirtualLibDbRow {
    id: i32,
    name: String,
    icon_type: String,
    sort_order: i32,
    filter_rules: Value,
    is_enabled: bool,
    cover_image: Option<String>,
}

async fn list_virtual_libraries(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let rows: Vec<VirtualLibDbRow> = sqlx::query_as(
        "SELECT id, name, icon_type, sort_order, filter_rules, is_enabled, cover_image FROM virtual_libraries ORDER BY sort_order ASC, id ASC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut libraries = Vec::with_capacity(rows.len());
    for r in rows {
        let rule: VirtualLibraryRule = serde_json::from_value(r.filter_rules.clone())
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,format!("视图 {} 规则损坏：{}",r.id,e)))?;
        let matched_count = count_virtual_library_matches(&state, &rule).await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
        let resolved_cover = resolve_virtual_library_cover_key(
            r.cover_image.as_deref(),
            &r.name,
            &r.icon_type,
        );
        libraries.push(json!({
            "id": r.id,
            "view_id": format!("lib_vlib_{}", r.id),
            "name": r.name,
            "icon_type": r.icon_type,
            "sort_order": r.sort_order,
            "filter_rules": rule,
            "is_enabled": r.is_enabled,
            "cover_image": r.cover_image,
            "resolved_cover": resolved_cover,
            "matched_count": matched_count
        }));
    }

    Ok(Json(json!({ "libraries": libraries })))
}

#[derive(Deserialize)]
struct SaveVirtualLibReq {
    id: Option<i32>,
    name: String,
    icon_type: String,
    #[serde(default)]
    sort_order: i32,
    filter_rules: VirtualLibraryRule,
    #[serde(default = "default_true")]
    is_enabled: bool,
    #[serde(default)]
    cover_image: Option<String>,
}

fn default_true() -> bool {
    true
}

async fn save_virtual_library(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SaveVirtualLibReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let name = payload.name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err((StatusCode::BAD_REQUEST, "智能视图名称须为 1～64 个字符".to_string()));
    }
    if !["movies", "tvshows"].contains(&payload.icon_type.as_str()) {
        return Err((StatusCode::BAD_REQUEST, "请选择电影或剧集展示样式".into()));
    }
    payload.filter_rules.validate().map_err(|e| (StatusCode::BAD_REQUEST,e))?;
    let rules_json = serde_json::to_value(&payload.filter_rules)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let cover_image = payload
        .cover_image
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    if let Some(vid) = payload.id {
        let result = sqlx::query(
            r#"
            UPDATE virtual_libraries
            SET name = $1, icon_type = $2, sort_order = $3, filter_rules = $4, is_enabled = $5, cover_image = $6, cover_revision = cover_revision + 1
            WHERE id = $7
            "#,
        )
        .bind(name)
        .bind(&payload.icon_type)
        .bind(payload.sort_order)
        .bind(&rules_json)
        .bind(payload.is_enabled)
        .bind(&cover_image)
        .bind(vid)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if result.rows_affected() == 0 {
            return Err((StatusCode::NOT_FOUND, "智能视图已被删除，请刷新列表".into()));
        }
    } else {
        sqlx::query(
            r#"
            INSERT INTO virtual_libraries (name, icon_type, sort_order, filter_rules, is_enabled, cover_image)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(name)
        .bind(&payload.icon_type)
        .bind(payload.sort_order)
        .bind(&rules_json)
        .bind(payload.is_enabled)
        .bind(&cover_image)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    Ok(Json(json!({ "status": "ok", "message": "智能视图已保存并立即生效" })))
}

async fn seed_preset_virtual_libraries(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let presets: Vec<(&str, &str, &str, i32, VirtualLibraryRule)> = vec![
        (
            "🔥 TMDB 8.0+ 高分佳作",
            "tvshows",
            "vlib_top_rated",
            10,
            VirtualLibraryRule {
                min_rating: Some(8.0),
                ..Default::default()
            },
        ),
        (
            "🎬 Netflix / HBO 精品剧场",
            "tvshows",
            "vlib_streaming",
            20,
            VirtualLibraryRule {
                media_type: Some("tv".to_string()),
                networks: vec![
                    "Netflix".to_string(),
                    "HBO".to_string(),
                    "HBO Max".to_string(),
                    "Apple TV+".to_string(),
                    "Disney+".to_string(),
                ],
                ..Default::default()
            },
        ),
        (
            "🇨🇳 华语热播剧场",
            "tvshows",
            "vlib_chinese",
            30,
            VirtualLibraryRule {
                media_type: Some("tv".to_string()),
                categories: vec!["电视剧".to_string()],
                countries: vec!["CN".to_string(), "HK".to_string(), "TW".to_string()],
                ..Default::default()
            },
        ),
        (
            "🇰🇷 日韩剧集精选",
            "tvshows",
            "vlib_kr_jp",
            40,
            VirtualLibraryRule {
                media_type: Some("tv".to_string()),
                categories: vec!["电视剧".to_string()],
                countries: vec!["KR".to_string(), "JP".to_string()],
                ..Default::default()
            },
        ),
        (
            "🎞️ 2025-2026 最新院线与新番",
            "movies",
            "vlib_latest",
            50,
            VirtualLibraryRule {
                year_min: Some(2025),
                ..Default::default()
            },
        ),
        ("高分口碑电影", "movies", "vlib_top_rated", 60, VirtualLibraryRule {
            media_type: Some("movie".into()), min_rating: Some(8.0), min_votes: Some(1000), ..Default::default()
        }),
        ("已完结短剧", "tvshows", "vlib_default_tv", 70, VirtualLibraryRule {
            media_type: Some("tv".into()), statuses: vec!["Ended".into()], episodes_max: Some(12), ..Default::default()
        }),
        ("90 分钟内电影", "movies", "vlib_default_movies", 80, VirtualLibraryRule {
            media_type: Some("movie".into()), runtime_max: Some(90), ..Default::default()
        }),
        ("4K 杜比视界", "movies", "vlib_default_movies", 90, VirtualLibraryRule {
            version_tags: vec!["4K".into(), "杜比视界".into()], ..Default::default()
        }),
    ];

    let mut tx = state.db.begin().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
    sqlx::query("SELECT pg_advisory_xact_lock(728314)").execute(&mut *tx).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
    let mut inserted = 0;
    for (name, icon_type, cover_image, sort_order, rule) in presets {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM virtual_libraries WHERE name=$1)")
            .bind(name).fetch_one(&mut *tx).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
        if exists { continue; }
        sqlx::query("INSERT INTO virtual_libraries (name,icon_type,sort_order,filter_rules,is_enabled,cover_image) VALUES ($1,$2,$3,$4,TRUE,$5)")
            .bind(name).bind(icon_type).bind(sort_order).bind(serde_json::to_value(rule).unwrap()).bind(cover_image)
            .execute(&mut *tx).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
        inserted += 1;
    }
    tx.commit().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;

    Ok(Json(json!({
        "status": "ok",
        "inserted": inserted,
        "message": format!("已创建 {} 个预设智能视图", inserted)
    })))
}

async fn delete_virtual_library(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
) -> Result<Json<Value>, (StatusCode, String)> {
    sqlx::query("DELETE FROM virtual_libraries WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "status": "ok", "message": "智能视图已删除" })))
}

async fn list_base_library_covers(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT config_key, config_value FROM system_configs WHERE config_key LIKE 'base_lib_cover:%'",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let cover_map: std::collections::HashMap<String, String> = rows
        .into_iter()
        .filter_map(|(k, v)| k.strip_prefix("base_lib_cover:").map(|id| (id.to_string(), v)))
        .collect();

    let base_libs = [
        ("lib_movie", "电影"),
        ("lib_tv", "电视剧"),
        ("lib_anime", "动漫"),
        ("lib_doc", "纪录片"),
        ("lib_variety", "综艺"),
    ];

    let libraries: Vec<Value> = base_libs
        .into_iter()
        .map(|(id, name)| {
            let cover_image = cover_map.get(id).cloned().filter(|s| !s.trim().is_empty());
            json!({
                "id": id,
                "name": name,
                "cover_image": cover_image
            })
        })
        .collect();

    Ok(Json(json!({ "libraries": libraries })))
}

#[derive(Deserialize)]
struct SaveBaseLibCoverReq {
    id: String,
    #[serde(default)]
    cover_image: Option<String>,
}

async fn save_base_library_cover(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SaveBaseLibCoverReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if !matches!(
        payload.id.as_str(),
        "lib_movie" | "lib_tv" | "lib_anime" | "lib_doc" | "lib_variety"
    ) {
        return Err((StatusCode::BAD_REQUEST, "无效的基础媒体库 ID".to_string()));
    }

    let cfg_key = format!("base_lib_cover:{}", payload.id);
    let clean_cover = payload
        .cover_image
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if let Some(cover) = clean_cover {
        sqlx::query(
            r#"
            INSERT INTO system_configs (config_key, config_value, value_type, description, updated_at)
            VALUES ($1, $2, 'string', '基础媒体库封面配置', NOW())
            ON CONFLICT (config_key) DO UPDATE SET
                config_value = EXCLUDED.config_value,
                updated_at = NOW()
            "#,
        )
        .bind(&cfg_key)
        .bind(cover)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    } else {
        sqlx::query("DELETE FROM system_configs WHERE config_key = $1")
            .bind(&cfg_key)
            .execute(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    Ok(Json(json!({ "status": "ok", "message": "基础媒体库封面已更新" })))
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

async fn list_users_and_sessions(
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

    let mut active_sessions = Vec::new();
    for entry in state.sessions.iter() {
        let s = entry.value().clone();
        let idle_sec = s.last_activity.elapsed().as_secs();
        let ep_label: Option<String> = if let Some(eid) = s.current_episode_id {
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
        } else {
            None
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
struct SaveUserReq {
    id: Option<String>,
    username: String,
    password: Option<String>,
    #[serde(default)]
    is_admin: bool,
    #[serde(default)]
    allowed_categories: Vec<String>,
}

async fn save_user(
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

async fn delete_user(
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

    Ok(Json(json!({ "status": "ok", "message": "用户已删除" })))
}

async fn revoke_access_token(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    sqlx::query("DELETE FROM access_tokens WHERE token = $1")
        .bind(&token)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "status": "ok", "message": "已注销该客户端设备" })))
}

async fn preview_smart_view(
    State(state): State<Arc<AppState>>,
    Json(rule): Json<VirtualLibraryRule>,
) -> Result<Json<Value>, (StatusCode, String)> {
    rule.validate().map_err(|e| (StatusCode::BAD_REQUEST,e))?;
    let count = count_virtual_library_matches(&state,&rule).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
    Ok(Json(json!({"matched_count": count})))
}

async fn smart_view_options(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode,String)> {
    let mut result = json!({});
    for (key,expr) in [
        ("networks", "m.tmdb_metadata->'networks'"),
        ("companies", "m.tmdb_metadata->'production_companies'"),
        ("collections", "CASE WHEN m.tmdb_metadata->'collection'->>'id' IS NOT NULL THEN jsonb_build_array(m.tmdb_metadata->'collection') ELSE '[]'::jsonb END"),
        ("keywords", "m.tmdb_metadata->'keyword_items'"),
        ("directors", "m.tmdb_metadata->'directors'"),
        ("cast", "m.tmdb_metadata->'cast'"),
        ("watch_providers", "m.tmdb_metadata->'watch_providers'"),
    ] {
        let sql = format!("SELECT DISTINCT (v->>'id')::int AS id, v->>'name' AS name FROM media_items m CROSS JOIN LATERAL jsonb_array_elements(COALESCE({},'[]')) v WHERE NOT m.is_ignored AND v->>'id' IS NOT NULL ORDER BY name,id LIMIT 500",expr);
        let rows: Vec<(i32,String)> = sqlx::query_as(&sql).fetch_all(&state.db).await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
        result[key] = json!(rows.into_iter().map(|(id,name)|json!({"id":id,"name":name})).collect::<Vec<_>>());
    }
    let stats: (i64,i64,i64,i64) = sqlx::query_as("SELECT COUNT(DISTINCT COALESCE(tmdb_id::text||':'||media_type,'raw:'||id::text)), COUNT(DISTINCT COALESCE(tmdb_id::text||':'||media_type,'raw:'||id::text)) FILTER(WHERE metadata_version>=2), COUNT(*) FILTER(WHERE metadata_refresh_requested), COUNT(*) FILTER(WHERE metadata_refresh_requested AND metadata_retry_count>=3) FROM media_items WHERE NOT is_ignored")
        .fetch_one(&state.db).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
    result["metadata"] = json!({"total":stats.0,"enriched":stats.1,"queued":stats.2,"failed":stats.3});
    Ok(Json(result))
}

async fn queue_metadata_backfill(State(state): State<Arc<AppState>>) -> Result<Json<Value>,(StatusCode,String)> {
    let cfg = state.config.load();
    if cfg.tmdb_api_key.trim().is_empty() || cfg.tmdb_api_key == "your_tmdb_api_key" {
        return Err((StatusCode::BAD_REQUEST,"请先在系统设置中配置 TMDB API Key".into()));
    }
    let result = sqlx::query("UPDATE media_items SET metadata_refresh_requested=TRUE, metadata_retry_count=0, metadata_retry_at=NULL WHERE NOT is_locked AND NOT is_ignored AND scrape_status IN (1,3) AND (metadata_version<2 OR metadata_retry_count>0)")
        .execute(&state.db).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
    Ok(Json(json!({"queued":result.rows_affected(),"message":format!("已将 {} 条已刮削目录加入补齐队列，同作品共享更新结果",result.rows_affected())})))
}
