use crate::{
    config::BootstrapFileConfig,
    image_cache::enforce_image_cache_quota,
    state::AppState,
};
use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};

#[derive(Serialize, sqlx::FromRow)]
struct ConfigRow {
    config_key: String,
    config_value: String,
    value_type: String,
    description: Option<String>,
}

pub(super) async fn list_configs(
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
pub(super) struct UpdateConfigsReq {
    updates: HashMap<String, String>,
}

pub(super) async fn update_configs(
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

pub(super) async fn trigger_sync(State(state): State<Arc<AppState>>) -> Json<Value> {
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

pub(super) async fn list_sync_logs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let logs: Vec<SyncLogRow> = sqlx::query_as(
        "SELECT id, started_at, finished_at, duration_ms, total_lines, added_episodes, deleted_episodes, new_media_items, status, error_message FROM sync_logs ORDER BY id DESC LIMIT 30",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // 复用 10s 缓存提供轻量级元数据扩展覆盖率统计，避免前端额外触发 smart_view_options 的 7 次 JSONB 全表扫描
    let metadata = if let Some(cached) = state.caches.admin_stats_cache.get("sync_metadata_stats").await {
        cached
    } else {
        let stats: (i64, i64, i64, i64) = sqlx::query_as(
            r#"
            WITH active AS (
                SELECT COALESCE(tmdb_id::text || ':' || media_type, 'raw:' || id::text) AS dedup_key,
                       metadata_version, metadata_refresh_requested, metadata_retry_count
                FROM media_items
                WHERE is_ignored = FALSE
            ),
            grouped AS (
                SELECT dedup_key, MAX(metadata_version) AS max_ver
                FROM active
                GROUP BY dedup_key
            )
            SELECT
                (SELECT COUNT(*) FROM grouped),
                (SELECT COUNT(*) FROM grouped WHERE max_ver >= 2),
                (SELECT COUNT(*) FROM active WHERE metadata_refresh_requested),
                (SELECT COUNT(*) FROM active WHERE metadata_refresh_requested AND metadata_retry_count >= 3)
            "#,
        )
        .fetch_one(&state.db)
        .await
        .unwrap_or((0, 0, 0, 0));

        let val = json!({
            "total": stats.0,
            "enriched": stats.1,
            "queued": stats.2,
            "failed": stats.3
        });
        state
            .caches
            .admin_stats_cache
            .insert("sync_metadata_stats".to_string(), val.clone())
            .await;
        val
    };

    Ok(Json(json!({ "logs": logs, "metadata": metadata })))
}

pub(super) async fn image_cache_stats(
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

pub(super) async fn trigger_cache_evict(
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


pub(super) async fn get_admin_overview(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.prune_stale_sessions();
    let cfg = state.config.load();
    let active_sessions = state.sessions.len();

    if let Some(mut cached) = state.caches.admin_stats_cache.get("overview").await {
        if let Some(sys) = cached.get_mut("system").and_then(|v| v.as_object_mut()) {
            sys.insert("active_sessions".to_string(), json!(active_sessions));
        }
        return Ok(Json(cached));
    }

    // 1. 统计去重后在库影视总数及各刮削状态占比（单次扫描 + 分组去重）
    let (total_media, scraped_count, pending_count, failed_count, ignored_count): (i64, i64, i64, i64, i64) =
        sqlx::query_as(
            r#"
            WITH dedup AS (
                SELECT DISTINCT ON (is_ignored, COALESCE(tmdb_id::text || ':' || media_type, 'raw:' || id::text))
                    is_ignored, scrape_status
                FROM media_items
                ORDER BY is_ignored, COALESCE(tmdb_id::text || ':' || media_type, 'raw:' || id::text), id ASC
            )
            SELECT
                COUNT(*) FILTER (WHERE NOT is_ignored),
                COUNT(*) FILTER (WHERE NOT is_ignored AND scrape_status IN (1, 3)),
                COUNT(*) FILTER (WHERE NOT is_ignored AND scrape_status = 0),
                COUNT(*) FILTER (WHERE NOT is_ignored AND scrape_status = 2),
                COUNT(*) FILTER (WHERE is_ignored)
            FROM dedup
            "#,
        )
        .fetch_one(&state.db)
        .await
        .unwrap_or((0, 0, 0, 0, 0));

    let total_episodes: i64 = sqlx::query_scalar(
        "SELECT GREATEST(0, (SELECT COUNT(*) FROM episodes) - (SELECT COUNT(*) FROM episodes e JOIN media_items m ON m.id = e.media_item_id WHERE m.is_ignored = TRUE))",
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

    let payload = json!({
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
            "active_sessions": active_sessions,
            "vlib_count": vlib_count,
            "user_count": user_count
        },
        "last_sync": last_sync
    });
    state
        .caches
        .admin_stats_cache
        .insert("overview".to_string(), payload.clone())
        .await;

    Ok(Json(payload))
}

