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
    if state
        .sync_ctrl
        .is_syncing
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Json(json!({
            "status": "already_running",
            "message": "同步任务已在运行中，正在展示实时进度"
        }));
    }

    *state.sync_ctrl.progress.write().await = Some(crate::state::SyncProgress {
        log_id: None,
        stage: "starting".to_string(),
        step: 1,
        total_steps: 5,
        percent: 5,
        message: "正在初始化同步任务…".to_string(),
        scanned_lines: 0,
        valid_records: 0,
        started_at: chrono::Utc::now(),
    });
    state.sync_ctrl.manual_trigger.notify_one();

    // 短暂等待后台任务写入 running 日志行，确保前端立即查询即可看到对应批次
    for _ in 0..10 {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        if state
            .sync_ctrl
            .progress
            .read()
            .await
            .as_ref()
            .and_then(|p| p.log_id)
            .is_some()
        {
            break;
        }
    }

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
            SELECT
                COUNT(*),
                COUNT(*) FILTER (WHERE m.metadata_version >= 2),
                COUNT(*) FILTER (WHERE m.metadata_refresh_requested),
                COUNT(*) FILTER (WHERE m.metadata_refresh_requested AND m.metadata_retry_count >= 3)
            FROM media_works m
            WHERE EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE)
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

    let active_progress = state.sync_ctrl.progress.read().await.clone();
    Ok(Json(json!({
        "logs": logs,
        "metadata": metadata,
        "active_progress": active_progress
    })))
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
        "disk_cache_enabled": cfg.image_disk_cache_enabled,
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
    let scraped_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM media_works w WHERE EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = w.id AND f.is_ignored = FALSE)",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let (verified_folders_count, pending_count, conflict_count, failed_count, ignored_count): (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE NOT is_ignored AND scrape_state = 'verified'),
            COUNT(*) FILTER (WHERE NOT is_ignored AND scrape_state = 'pending'),
            COUNT(*) FILTER (WHERE NOT is_ignored AND scrape_state = 'conflict'),
            COUNT(*) FILTER (WHERE NOT is_ignored AND scrape_state = 'failed'),
            COUNT(*) FILTER (WHERE is_ignored)
        FROM sync_folders
        "#,
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or((0, 0, 0, 0, 0));

    let total_media = scraped_count;

    let total_episodes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE f.is_ignored = FALSE",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    // 2. 五大分类统计
    let cat_rows: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT w.category, COUNT(*)
        FROM media_works w
        WHERE EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = w.id AND f.is_ignored = FALSE)
        GROUP BY w.category
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
            "verified_folders_count": verified_folders_count,
            "pending_count": pending_count,
            "conflict_count": conflict_count,
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

pub(super) async fn get_resource_metrics(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mut recent: Vec<crate::monitor::ResourceSample> = {
        let guard = state.monitor.recent_samples.read().await;
        guard.iter().cloned().collect()
    };

    if recent.is_empty() {
        if let Ok(sample) = crate::monitor::collect_and_record(&state).await {
            recent.push(sample);
        }
    }

    let current = recent.last().cloned();
    let daily_stats: Vec<crate::monitor::ResourceDailyRow> = sqlx::query_as(
        r#"
        SELECT
            stat_date, sample_count,
            app_cpu_avg, app_cpu_max, app_mem_avg_mb, app_mem_max_mb,
            db_cpu_avg, db_cpu_max, db_mem_avg_mb, db_mem_max_mb,
            db_size_max_mb, db_conn_avg, db_conn_max,
            disk_used_max_gb, disk_usage_max_pct,
            peak_cpu_at, peak_cpu_scenario, peak_mem_at, peak_mem_scenario,
            updated_at
        FROM resource_daily_stats
        ORDER BY stat_date DESC
        LIMIT 30
        "#,
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let uptime_sec = (chrono::Utc::now() - state.monitor.started_at)
        .num_seconds()
        .max(0);

    Ok(Json(json!({
        "started_at": state.monitor.started_at,
        "uptime_sec": uptime_sec,
        "current": current,
        "recent_samples": recent,
        "daily_stats": daily_stats
    })))
}

pub(super) async fn trigger_resource_sample(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    crate::monitor::collect_and_record(&state)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    get_resource_metrics(State(state)).await
}

