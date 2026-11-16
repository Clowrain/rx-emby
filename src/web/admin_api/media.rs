use crate::{
    scraper::{promote_folder_manually, scrape_single_folder, scrape_work_seasons_by_id},
    state::{AppState, ScrapeTask},
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Deserialize, Default)]
pub(super) struct AdminMediaQuery {
    category: Option<String>,
    keyword: Option<String>,
    media_type: Option<String>,
    sort_by: Option<String>,
    sort_order: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

fn build_admin_media_order_clause(sort_by: Option<&str>, is_asc: bool) -> &'static str {
    match sort_by.unwrap_or("updated_at") {
        "created_at" => {
            if is_asc {
                "w.created_at ASC, w.id ASC"
            } else {
                "w.created_at DESC, w.id DESC"
            }
        }
        "year" => {
            if is_asc {
                "COALESCE(NULLIF(w.premiered_at, ''), CASE WHEN w.year IS NOT NULL THEN w.year::text || '-01-01' ELSE NULL END) ASC NULLS LAST, w.id ASC"
            } else {
                "COALESCE(NULLIF(w.premiered_at, ''), CASE WHEN w.year IS NOT NULL THEN w.year::text || '-01-01' ELSE NULL END) DESC NULLS LAST, w.id DESC"
            }
        }
        "rating" => {
            if is_asc {
                "w.rating ASC, w.id ASC"
            } else {
                "w.rating DESC, w.id DESC"
            }
        }
        "title" => {
            if is_asc {
                "w.title ASC, w.id ASC"
            } else {
                "w.title DESC, w.id DESC"
            }
        }
        _ => {
            if is_asc {
                "w.updated_at ASC, w.id ASC"
            } else {
                "w.updated_at DESC, w.id DESC"
            }
        }
    }
}

#[derive(Serialize, sqlx::FromRow)]
struct AdminWorkRow {
    id: i64,
    media_type: String,
    tmdb_id: Option<i32>,
    title: String,
    year: Option<i32>,
    rating: f32,
    category: String,
    is_locked: bool,
    poster_path: Option<String>,
    episode_count: i64,
    folder_count: i64,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// 正式作品库列表 (`media_works`)
pub(super) async fn list_admin_media(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AdminMediaQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let limit = q.limit.unwrap_or(30).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);
    let kw_raw = q
        .keyword
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let kw = kw_raw.map(|s| format!("%{}%", s));
    let kw_tmdb_id: Option<i32> = kw_raw.and_then(|s| s.parse::<i32>().ok());

    let is_asc = q
        .sort_order
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case("asc") || s.eq_ignore_ascii_case("ascending"))
        .unwrap_or(false);
    let order_clause = build_admin_media_order_clause(q.sort_by.as_deref(), is_asc);

    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM media_works w
        WHERE EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = w.id AND f.is_ignored = FALSE)
          AND ($1::text IS NULL OR w.category = $1)
          AND ($2::text IS NULL OR w.media_type = $2)
          AND ($3::text IS NULL OR w.title ILIKE $3 OR w.original_title ILIKE $3 OR ($4::int4 IS NOT NULL AND w.tmdb_id = $4))
        "#,
    )
    .bind(&q.category)
    .bind(&q.media_type)
    .bind(&kw)
    .bind(kw_tmdb_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let sql = format!(
        r#"
        SELECT
            w.id, w.media_type, w.tmdb_id, w.title, w.year, w.rating, w.category,
            w.is_locked, w.poster_path, w.updated_at,
            (SELECT COUNT(*) FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE f.work_id = w.id AND f.is_ignored = FALSE) AS episode_count,
            (SELECT COUNT(*) FROM sync_folders f WHERE f.work_id = w.id AND f.is_ignored = FALSE) AS folder_count
        FROM media_works w
        WHERE EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = w.id AND f.is_ignored = FALSE)
          AND ($1::text IS NULL OR w.category = $1)
          AND ($2::text IS NULL OR w.media_type = $2)
          AND ($3::text IS NULL OR w.title ILIKE $3 OR w.original_title ILIKE $3 OR ($4::int4 IS NOT NULL AND w.tmdb_id = $4))
        ORDER BY {order_clause}
        LIMIT $5 OFFSET $6
        "#,
    );

    let items: Vec<AdminWorkRow> = sqlx::query_as(&sql)
        .bind(&q.category)
        .bind(&q.media_type)
        .bind(&kw)
        .bind(kw_tmdb_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "items": items, "total": total })))
}

#[derive(Serialize, sqlx::FromRow)]
struct AdminWorkFullRow {
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
    is_locked: bool,
}

#[derive(Serialize, sqlx::FromRow)]
struct AdminLinkedFolderRow {
    id: i64,
    folder_path: String,
    parsed_title: String,
    parsed_year: Option<i32>,
    category: String,
    scrape_state: String,
    priority_boost: i32,
    is_locked: bool,
    is_ignored: bool,
    file_count: i64,
}

#[derive(Serialize, sqlx::FromRow)]
struct AdminFileDetailRow {
    id: i64,
    folder_id: i64,
    season_number: i32,
    episode_number: i32,
    title: Option<String>,
    overview: Option<String>,
    still_path: Option<String>,
    air_date: Option<String>,
    runtime: i32,
    version_tag: String,
    priority_score: i32,
    priority_override: Option<i32>,
    folder_priority_boost: i32,
    effective_priority: i32,
    container: String,
    file_name: String,
    alist_path: String,
    file_size_bytes: Option<i64>,
}

/// 获取正式作品详情、关联的物理目录（含目录级权重加成）及所有分集多版本物理源
pub(super) async fn get_admin_media_detail(
    State(state): State<Arc<AppState>>,
    Path(work_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let media: Option<AdminWorkFullRow> = sqlx::query_as(
        r#"
        SELECT id, media_type, tmdb_id, title, original_title, year, premiered_at,
               overview, rating, poster_path, backdrop_path, countries, networks,
               genres, category, is_locked
        FROM media_works
        WHERE id = $1
        "#,
    )
    .bind(work_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some(media) = media else {
        return Err((StatusCode::NOT_FOUND, "作品条目不存在".to_string()));
    };

    if media.media_type == "tv" {
        let _ = state
            .scraper
            .priority_tx
            .try_send(ScrapeTask::WorkSeasons(work_id));
    }

    let folders: Vec<AdminLinkedFolderRow> = sqlx::query_as(
        r#"
        SELECT f.id, f.root_dir AS folder_path, f.clean_title AS parsed_title, f.year AS parsed_year, f.category,
               f.scrape_state, f.priority_boost, f.is_locked, f.is_ignored,
               (SELECT COUNT(*) FROM sync_files sf WHERE sf.folder_id = f.id) AS file_count
        FROM sync_folders f
        WHERE f.work_id = $1
        ORDER BY f.is_ignored ASC, f.id ASC
        "#,
    )
    .bind(work_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut files: Vec<AdminFileDetailRow> = sqlx::query_as(
        r#"
        SELECT
            sf.id, sf.folder_id, sf.season_number, sf.episode_number,
            we.title, we.overview, we.still_path, we.air_date, COALESCE(we.runtime, 0) AS runtime,
            sf.version_tag, sf.priority_score, sf.priority_override,
            f.priority_boost AS folder_priority_boost,
            COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) AS effective_priority,
            sf.container, sf.file_name, sf.alist_path, sf.file_size_bytes
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        LEFT JOIN work_episodes we
          ON we.work_id = $1
         AND we.season_number = sf.season_number
         AND we.episode_number = sf.episode_number
        WHERE f.work_id = $1
        ORDER BY sf.season_number ASC, sf.episode_number ASC,
                 COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) DESC,
                 sf.id ASC
        "#,
    )
    .bind(work_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut idx = 0;
    while idx < files.len() {
        let s_num = files[idx].season_number;
        let e_num = files[idx].episode_number;
        let mut end = idx + 1;
        while end < files.len()
            && files[end].season_number == s_num
            && files[end].episode_number == e_num
        {
            end += 1;
        }

        let slice = &mut files[idx..end];
        let mut tuples: Vec<(String, i32, String, String)> = slice
            .iter()
            .map(|r| {
                (
                    r.version_tag.clone(),
                    r.effective_priority,
                    r.alist_path.clone(),
                    r.container.clone(),
                )
            })
            .collect();
        crate::sync::parser::disambiguate_version_tags(&mut tuples);
        for (f_row, (new_tag, _, _, _)) in slice.iter_mut().zip(tuples.into_iter()) {
            f_row.version_tag = new_tag;
        }
        slice.sort_by(|a, b| {
            b.effective_priority
                .cmp(&a.effective_priority)
                .then(a.id.cmp(&b.id))
        });
        idx = end;
    }

    Ok(Json(json!({
        "media": media,
        "folders": folders,
        "episodes": files
    })))
}

/// 手动触发该作品小雅父目录 `fs/list` 寻址预热并回填文件体积
pub(super) async fn prewarm_admin_media_dirs(
    State(state): State<Arc<AppState>>,
    Path(work_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.spawn_prewarm_media_dirs(work_id, None);
    Ok(Json(json!({
        "status": "ok",
        "message": "已触发小雅目录寻址预热与文件体积回填，请稍候刷新详情查看"
    })))
}

#[derive(Deserialize)]
pub(super) struct BindTmdbReq {
    tmdb_id: Option<i32>,
    media_type: Option<String>,
}

/// 在正式作品上改绑 TMDB ID（会联动重新归并其下关联的物理目录）
pub(super) async fn bind_tmdb_id(
    State(state): State<Arc<AppState>>,
    Path(work_id): Path<i64>,
    Json(payload): Json<BindTmdbReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let Some(tmdb_id) = payload.tmdb_id else {
        return Err((StatusCode::BAD_REQUEST, "请提供有效的 TMDB ID".into()));
    };

    let folder_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM sync_folders WHERE work_id = $1")
        .bind(work_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut target_work_id = work_id;
    for fid in folder_ids {
        target_work_id =
            promote_folder_manually(&state, fid, Some(tmdb_id), payload.media_type.as_deref())
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, format!("改绑 TMDB 失败: {:#}", e)))?;
    }

    let _ = sqlx::query("UPDATE media_works SET is_locked = TRUE WHERE id = $1")
        .bind(target_work_id)
        .execute(&state.db)
        .await;

    state.caches.admin_stats_cache.invalidate_all();
    state.caches.image_meta_cache.invalidate_all();

    Ok(Json(json!({
        "status": "ok",
        "work_id": target_work_id,
        "message": "已成功改绑 TMDB ID 并同步更新关联目录与分集元数据"
    })))
}

/// 单部作品立即刷新分集/季元数据
pub(super) async fn scrape_one_now(
    State(state): State<Arc<AppState>>,
    Path(work_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let _ = sqlx::query("UPDATE work_seasons SET scrape_attempts = 0 WHERE work_id = $1")
        .bind(work_id)
        .execute(&state.db)
        .await;

    scrape_work_seasons_by_id(&state, work_id, true)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("作品分集刮削失败: {:#}", e),
            )
        })?;

    state.caches.admin_stats_cache.invalidate_all();
    state.caches.image_meta_cache.invalidate_all();

    Ok(Json(
        json!({ "status": "ok", "message": "作品及分集元数据刷新成功" }),
    ))
}

#[derive(Deserialize)]
pub(super) struct ToggleIgnoreReq {
    ignore: bool,
}

/// 屏蔽或恢复整部作品关联的所有物理目录
pub(super) async fn toggle_ignore_media(
    State(state): State<Arc<AppState>>,
    Path(work_id): Path<i64>,
    Json(payload): Json<ToggleIgnoreReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    sqlx::query("UPDATE sync_folders SET is_ignored = $1, updated_at = NOW() WHERE work_id = $2")
        .bind(payload.ignore)
        .bind(work_id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state.caches.admin_stats_cache.invalidate_all();
    state.caches.image_meta_cache.invalidate_all();

    Ok(Json(json!({
        "status": "ok",
        "message": if payload.ignore { "已屏蔽该作品关联目录，后续同步将自动跳过" } else { "已恢复显示到媒体库" }
    })))
}

// =========================================================================
// 多版本权重调整 (目录级 priority_boost / 单集文件级 priority_override)
// =========================================================================

#[derive(Deserialize)]
pub(super) struct FolderPriorityReq {
    priority_boost: i32,
}

pub(super) async fn update_folder_priority(
    State(state): State<Arc<AppState>>,
    Path(folder_id): Path<i64>,
    Json(payload): Json<FolderPriorityReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let res = sqlx::query(
        "UPDATE sync_folders SET priority_boost = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(payload.priority_boost)
    .bind(folder_id)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if res.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, "物理目录不存在".into()));
    }

    Ok(Json(json!({
        "status": "ok",
        "message": format!("已将目录权重加成调整为 {}", payload.priority_boost)
    })))
}

#[derive(Deserialize)]
pub(super) struct FilePriorityReq {
    priority_override: Option<i32>,
}

pub(super) async fn update_file_priority(
    State(state): State<Arc<AppState>>,
    Path(file_id): Path<i64>,
    Json(payload): Json<FilePriorityReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let res = sqlx::query("UPDATE sync_files SET priority_override = $1 WHERE id = $2")
        .bind(payload.priority_override)
        .bind(file_id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if res.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, "物理文件不存在".into()));
    }

    Ok(Json(json!({
        "status": "ok",
        "message": match payload.priority_override {
            Some(v) => format!("已将该版本文件覆盖权重设为 {}", v),
            None => "已恢复默认自动权重计算".to_string(),
        }
    })))
}

// =========================================================================
// Staging 中间表管理工作台 (`sync_folders`: pending / conflict / failed / ignored)
// =========================================================================

#[derive(Deserialize, Default)]
pub(super) struct StagingFoldersQuery {
    scrape_state: Option<String>,
    is_ignored: Option<bool>,
    category: Option<String>,
    keyword: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
struct StagingFolderRow {
    id: i64,
    folder_path: String,
    parsed_title: String,
    parsed_year: Option<i32>,
    hint_tmdb_id: Option<i32>,
    media_type: String,
    category: String,
    scrape_state: String,
    conflict_reason: Option<String>,
    retry_count: i32,
    is_locked: bool,
    is_ignored: bool,
    priority_boost: i32,
    work_id: Option<i64>,
    work_title: Option<String>,
    file_count: i64,
    updated_at: chrono::DateTime<chrono::Utc>,
}

pub(super) async fn list_staging_folders(
    State(state): State<Arc<AppState>>,
    Query(q): Query<StagingFoldersQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let limit = q.limit.unwrap_or(30).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);
    let ignored_filter = q.is_ignored.unwrap_or(false);
    let state_filter = q
        .scrape_state
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "all")
        .map(str::to_string);
    let kw_raw = q
        .keyword
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let kw = kw_raw.map(|s| format!("%{}%", s));
    let kw_tmdb_id: Option<i32> = kw_raw.and_then(|s| s.parse::<i32>().ok());

    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM sync_folders f
        WHERE f.is_ignored = $1
          AND ($2::text IS NULL OR f.scrape_state = $2)
          AND ($3::text IS NULL OR f.category = $3)
          AND ($4::text IS NULL OR f.clean_title ILIKE $4 OR f.root_dir ILIKE $4 OR ($5::int4 IS NOT NULL AND f.parsed_tmdb_id = $5))
        "#,
    )
    .bind(ignored_filter)
    .bind(&state_filter)
    .bind(&q.category)
    .bind(&kw)
    .bind(kw_tmdb_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let items: Vec<StagingFolderRow> = sqlx::query_as(
        r#"
        SELECT
            f.id, f.root_dir AS folder_path, f.clean_title AS parsed_title, f.year AS parsed_year, f.parsed_tmdb_id AS hint_tmdb_id,
            f.inferred_media_type AS media_type, f.category, f.scrape_state, f.conflict_reason,
            f.retry_count::int4 AS retry_count, f.is_locked, f.is_ignored, f.priority_boost,
            f.work_id, w.title AS work_title,
            (SELECT COUNT(*) FROM sync_files sf WHERE sf.folder_id = f.id) AS file_count,
            f.updated_at
        FROM sync_folders f
        LEFT JOIN media_works w ON w.id = f.work_id
        WHERE f.is_ignored = $1
          AND ($2::text IS NULL OR f.scrape_state = $2)
          AND ($3::text IS NULL OR f.category = $3)
          AND ($4::text IS NULL OR f.clean_title ILIKE $4 OR f.root_dir ILIKE $4 OR ($5::int4 IS NOT NULL AND f.parsed_tmdb_id = $5))
        ORDER BY f.updated_at DESC, f.id DESC
        LIMIT $6 OFFSET $7
        "#,
    )
    .bind(ignored_filter)
    .bind(&state_filter)
    .bind(&q.category)
    .bind(&kw)
    .bind(kw_tmdb_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "items": items, "total": total })))
}

pub(super) async fn scrape_staging_folder(
    State(state): State<Arc<AppState>>,
    Path(folder_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let _ = sqlx::query(
        "UPDATE sync_folders SET scrape_state = 'pending', is_locked = FALSE, retry_count = 0, next_retry_at = NULL, conflict_reason = NULL WHERE id = $1",
    )
    .bind(folder_id)
    .execute(&state.db)
    .await;

    scrape_single_folder(&state, folder_id)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("目录刮削失败: {:#}", e)))?;

    let state_after: Option<(String, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT scrape_state, conflict_reason, work_id FROM sync_folders WHERE id = $1",
    )
    .bind(folder_id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    state.caches.admin_stats_cache.invalidate_all();
    state.caches.image_meta_cache.invalidate_all();

    let (scrape_state, conflict_reason, work_id) =
        state_after.unwrap_or(("pending".into(), None, None));
    Ok(Json(json!({
        "status": "ok",
        "scrape_state": scrape_state,
        "conflict_reason": conflict_reason,
        "work_id": work_id,
        "message": match scrape_state.as_str() {
            "verified" => "刮削并通过四维校验，已自动入库正式作品表".to_string(),
            "conflict" => format!("刮削存在校验冲突：{}", conflict_reason.unwrap_or_default()),
            _ => "刮削执行完成，请查看最新状态".to_string(),
        }
    })))
}

/// 人工裁决 / 绑定 TMDB ID / 无 TMDB 直接发布到正式作品库
pub(super) async fn promote_staging_folder(
    State(state): State<Arc<AppState>>,
    Path(folder_id): Path<i64>,
    Json(payload): Json<BindTmdbReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let work_id = promote_folder_manually(
        &state,
        folder_id,
        payload.tmdb_id,
        payload.media_type.as_deref(),
    )
    .await
    .map_err(|e| (StatusCode::BAD_GATEWAY, format!("手动发布失败: {:#}", e)))?;

    state.caches.admin_stats_cache.invalidate_all();
    state.caches.image_meta_cache.invalidate_all();

    Ok(Json(json!({
        "status": "ok",
        "work_id": work_id,
        "message": if payload.tmdb_id.is_some() {
            "已绑定 TMDB ID 并直接发布至正式作品库"
        } else {
            "已作为自定义条目直接发布至正式作品库"
        }
    })))
}

pub(super) async fn toggle_ignore_staging_folder(
    State(state): State<Arc<AppState>>,
    Path(folder_id): Path<i64>,
    Json(payload): Json<ToggleIgnoreReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    sqlx::query("UPDATE sync_folders SET is_ignored = $1, updated_at = NOW() WHERE id = $2")
        .bind(payload.ignore)
        .bind(folder_id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state.caches.admin_stats_cache.invalidate_all();
    state.caches.image_meta_cache.invalidate_all();

    Ok(Json(json!({
        "status": "ok",
        "message": if payload.ignore { "已屏蔽该物理目录" } else { "已取消屏蔽该物理目录" }
    })))
}

#[derive(Deserialize)]
pub(super) struct BatchScrapeReq {
    folder_ids: Vec<i64>,
}

pub(super) async fn batch_scrape_staging_folders(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BatchScrapeReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let count = payload.folder_ids.len();
    for fid in payload.folder_ids {
        let _ = state.scraper.priority_tx.try_send(ScrapeTask::Folder(fid));
    }
    Ok(Json(json!({
        "status": "ok",
        "queued": count,
        "message": format!("已将 {} 个目录加入高优先级刮削队列", count)
    })))
}

pub(super) async fn retry_failed_scrapes(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let res = sqlx::query(
        r#"
        UPDATE sync_folders
        SET scrape_state = 'pending', retry_count = 0, next_retry_at = NULL, conflict_reason = NULL
        WHERE scrape_state IN ('failed', 'conflict') AND is_locked = FALSE AND is_ignored = FALSE
        "#,
    )
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state.caches.admin_stats_cache.invalidate_all();
    state.caches.image_meta_cache.invalidate_all();

    Ok(Json(json!({
        "status": "ok",
        "reset_count": res.rows_affected(),
        "message": format!("已将 {} 个失败/冲突目录重新加入待刮削队列", res.rows_affected())
    })))
}

#[cfg(test)]
mod tests {
    use super::build_admin_media_order_clause;

    #[test]
    fn year_sort_prefers_premiered_at_with_year_fallback() {
        assert_eq!(
            build_admin_media_order_clause(Some("year"), false),
            "COALESCE(NULLIF(w.premiered_at, ''), CASE WHEN w.year IS NOT NULL THEN w.year::text || '-01-01' ELSE NULL END) DESC NULLS LAST, w.id DESC"
        );
        assert_eq!(
            build_admin_media_order_clause(Some("year"), true),
            "COALESCE(NULLIF(w.premiered_at, ''), CASE WHEN w.year IS NOT NULL THEN w.year::text || '-01-01' ELSE NULL END) ASC NULLS LAST, w.id ASC"
        );
    }
}
