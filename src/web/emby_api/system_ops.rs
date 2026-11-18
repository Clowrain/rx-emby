use super::auth::resolve_valid_user_id_with_query;
use crate::state::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{atomic::Ordering, Arc};

#[derive(Deserialize, Default)]
pub(super) struct OpsQuery {
    #[serde(rename = "StartIndex", alias = "startIndex", default)]
    start_index: i64,
    #[serde(rename = "Limit", alias = "limit")]
    limit: Option<i64>,
    #[serde(
        rename = "api_key",
        alias = "ApiKey",
        alias = "X-Emby-Token",
        alias = "x-emby-token"
    )]
    api_key: Option<String>,
}

async fn require_admin(
    state: &AppState,
    headers: &HeaderMap,
    api_key: Option<&str>,
) -> Result<String, (StatusCode, String)> {
    let context = super::auth::resolve_auth_context(state, headers, api_key).await?;
    if !context.is_admin {
        return Err((StatusCode::FORBIDDEN, "仅管理员可执行该操作".to_string()));
    }
    Ok(context.user_id)
}

/// `POST /Library/Refresh` —— 桥接触发 rx-emby 增量同步
pub(super) async fn post_library_refresh(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<OpsQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_admin(&state, &headers, q.api_key.as_deref()).await?;
    crate::sync::try_trigger_sync(&state);
    tracing::info!("🔄 [Emby API] 管理员触发 /Library/Refresh 增量同步");
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /ScheduledTasks`
pub(super) async fn get_scheduled_tasks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<OpsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    require_admin(&state, &headers, q.api_key.as_deref()).await?;
    let is_syncing = state.sync_ctrl.is_syncing.load(Ordering::Relaxed);
    let last_sync = *state.sync_ctrl.last_sync_time.read().await;
    let progress = state.sync_ctrl.progress.read().await.clone();

    let scrape = state.scraper.runtime.snapshot();

    let sync_state = if is_syncing { "Running" } else { "Idle" };
    let sync_pct = progress.as_ref().map(|p| p.percent as f64).unwrap_or(0.0);

    Ok(Json(json!([
        {
            "Name": "扫描媒体库 (Xiaoya STRM 增量同步)",
            "State": sync_state,
            "CurrentProgressPercentage": if is_syncing { Some(sync_pct) } else { None::<f64> },
            "Id": "rx_sync_strm",
            "Category": "Library",
            "Key": "RefreshLibrary",
            "Description": "解析 strm.txt 并增量更新本地媒体索引",
            "LastExecutionResult": last_sync.map(|t| json!({
                "StartTimeUtc": t.to_rfc3339(),
                "EndTimeUtc": t.to_rfc3339(),
                "Status": "Completed"
            }))
        },
        {
            "Name": "TMDB 元数据与海报刮削",
            "State": if scrape.running { "Running" } else { "Idle" },
            "CurrentProgressPercentage": if scrape.running { scrape.current_progress_percentage } else { None },
            "Id": "rx_scrape_tmdb",
            "Category": "Library",
            "Key": "RefreshMetadata",
            "Description": "后台自动补全 TMDB 影视信息与封面缓存",
            "LastExecutionResult": scrape.last_execution_finished.map(|t| json!({
                "StartTimeUtc": t.to_rfc3339(),
                "EndTimeUtc": t.to_rfc3339(),
                "Status": "Completed"
            }))
        }
    ])))
}

/// `POST /ScheduledTasks/Running/:id`
pub(super) async fn post_run_scheduled_task(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
    Query(q): Query<OpsQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_admin(&state, &headers, q.api_key.as_deref()).await?;
    if task_id == "rx_sync_strm" {
        crate::sync::try_trigger_sync(&state);
        Ok(StatusCode::NO_CONTENT)
    } else if task_id == "rx_scrape_tmdb" {
        crate::scraper::request_scheduled_scrape(&state)
            .await
            .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((StatusCode::NOT_FOUND, "未找到指定的计划任务".to_string()))
    }
}

/// `GET /System/ActivityLog/Entries`
pub(super) async fn get_activity_log_entries(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<OpsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    require_admin(&state, &headers, q.api_key.as_deref()).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.start_index.max(0);

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_logs")
        .fetch_one(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    #[derive(sqlx::FromRow)]
    struct ActivityRow {
        id: i64,
        started_at: chrono::DateTime<chrono::Utc>,
        total_lines: i32,
        added_episodes: i32,
        deleted_episodes: i32,
        new_media_items: i32,
        status: String,
        error_message: Option<String>,
    }
    let rows: Vec<ActivityRow> = sqlx::query_as(
        r#"
        SELECT id, started_at, total_lines, added_episodes, deleted_episodes, new_media_items, status, error_message
        FROM sync_logs
        ORDER BY id DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let items: Vec<Value> = rows
        .into_iter()
        .map(
            |ActivityRow {
                 id,
                 started_at,
                 total_lines: lines,
                 added_episodes: added,
                 deleted_episodes: deleted,
                 new_media_items: new_items,
                 status,
                 error_message: err,
             }| {
                let severity = if status == "failed" { "Error" } else { "Info" };
                let overview = match err {
                    Some(ref msg) if !msg.is_empty() => msg.clone(),
                    _ => format!(
                        "扫描 {} 行，新增剧集 {}，删除 {}，新作品 {}",
                        lines, added, deleted, new_items
                    ),
                };
                json!({
                    "Id": id,
                    "Name": format!("媒体库增量同步 ({})", status),
                    "Overview": overview,
                    "ShortOverview": overview,
                    "Type": "LibrarySync",
                    "Date": started_at.to_rfc3339(),
                    "Severity": severity
                })
            },
        )
        .collect();

    Ok(Json(json!({
        "Items": items,
        "TotalRecordCount": total,
        "StartIndex": offset
    })))
}

/// `DELETE /Videos/ActiveEncodings` & `POST /Videos/ActiveEncodings/Delete` —— 纯 302 直链架构兜底
pub(super) async fn delete_active_encodings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<OpsQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    let _uid =
        resolve_valid_user_id_with_query(&state, None, &headers, q.api_key.as_deref()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /LiveTv/Info` —— 最小状态桩（未启用 Live TV）
pub(super) async fn get_livetv_info(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<OpsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let _uid =
        resolve_valid_user_id_with_query(&state, None, &headers, q.api_key.as_deref()).await?;
    Ok(Json(json!({
        "Services": [],
        "IsEnabled": false,
        "EnabledUsers": []
    })))
}

/// `GET /Branding/Configuration`
pub(super) async fn get_branding_configuration() -> Json<Value> {
    Json(json!({
        "LoginDisclaimer": "",
        "CustomCss": ""
    }))
}

/// `GET /System/Endpoint`
pub(super) async fn get_system_endpoint(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<OpsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let _uid =
        resolve_valid_user_id_with_query(&state, None, &headers, q.api_key.as_deref()).await?;
    Ok(Json(json!({
        "IsLocal": true,
        "IsInNetwork": true
    })))
}

#[cfg(test)]
mod scheduled_task_tests {
    use super::*;

    async fn test_state(admin: bool) -> Arc<AppState> {
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@127.0.0.1:5432/rx_emby")
            .unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let state = AppState::new(
            crate::config::BootstrapFileConfig::default().into_dynamic(),
            db,
            tx,
        )
        .unwrap();
        state
            .caches
            .token_user_cache
            .insert("scheduled-test".into(), Arc::new(("admin".into(), admin)))
            .await;
        state
            .caches
            .token_touch_debounce
            .insert("scheduled-test".into(), ())
            .await;
        state
    }

    fn query() -> Query<OpsQuery> {
        Query(OpsQuery {
            api_key: Some("scheduled-test".into()),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn scheduled_scrape_requires_admin_before_any_queue_action() {
        let result = post_run_scheduled_task(
            State(test_state(false).await),
            HeaderMap::new(),
            Path("rx_scrape_tmdb".into()),
            query(),
        )
        .await;
        assert_eq!(result.unwrap_err().0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn scheduled_scrape_without_tmdb_key_reports_unavailable() {
        let result = post_run_scheduled_task(
            State(test_state(true).await),
            HeaderMap::new(),
            Path("rx_scrape_tmdb".into()),
            query(),
        )
        .await;
        assert_eq!(result.unwrap_err().0, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn scheduled_tasks_idle_scrape_has_unknown_progress() {
        let Json(tasks) =
            get_scheduled_tasks(State(test_state(true).await), HeaderMap::new(), query())
                .await
                .unwrap();
        assert_eq!(tasks[1]["State"], "Idle");
        assert!(tasks[1]["CurrentProgressPercentage"].is_null());
    }
}
