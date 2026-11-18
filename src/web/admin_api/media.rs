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

async fn invalidate_admin_stats(state: &AppState) {
    for key in ["overview", "smart_view_options"] {
        state
            .caches
            .admin_stats_cache
            .invalidate(&key.to_string())
            .await;
    }
}

async fn invalidate_work_image_meta(state: &AppState, work_id: i64) {
    for key in [
        format!("work_{work_id}:Primary"),
        format!("work_{work_id}:Backdrop"),
        format!("media_{work_id}:Primary"),
        format!("media_{work_id}:Backdrop"),
    ] {
        state.caches.image_meta_cache.invalidate(&key).await;
    }
}

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

async fn broadcast_work_visibility(state: &AppState, work_id: i64, visible: bool) {
    let category: Option<String> =
        sqlx::query_scalar("SELECT category FROM media_works WHERE id = $1")
            .bind(work_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let library = match category.as_deref() {
        Some("电影") | Some("movie") => "lib_movie",
        Some("电视剧") | Some("tv") => "lib_tv",
        Some("动漫") | Some("anime") => "lib_anime",
        Some("纪录片") | Some("documentary") => "lib_doc",
        Some("综艺") | Some("variety") => "lib_variety",
        _ => return,
    };
    state.session_hub.broadcast(
        None,
        "LibraryChanged",
        json!({
            "ItemsAdded": if visible { vec![format!("work_{work_id}")] } else { vec![] },
            "ItemsRemoved": if visible { vec![] } else { vec![format!("work_{work_id}")] },
            "ItemsUpdated": if visible { vec![format!("work_{work_id}")] } else { vec![] },
            "FoldersAddedTo": if visible { vec![library] } else { vec![] },
            "FoldersRemovedFrom": if visible { vec![] } else { vec![library] }
        }),
    );
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
    let kw = kw_raw.map(|s| {
        format!(
            "%{}%",
            s.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )
    });
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
          AND ($3::text IS NULL OR w.title ILIKE $3 ESCAPE '\\' OR w.original_title ILIKE $3 ESCAPE '\\' OR ($4::int4 IS NOT NULL AND w.tmdb_id = $4))
        "#,
    )
    .bind(&q.category)
    .bind(&q.media_type)
    .bind(&kw)
    .bind(kw_tmdb_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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
          AND ($3::text IS NULL OR w.title ILIKE $3 ESCAPE '\\' OR w.original_title ILIKE $3 ESCAPE '\\' OR ($4::int4 IS NOT NULL AND w.tmdb_id = $4))
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
        if let Err(error) = state
            .scraper
            .priority_tx
            .try_send(ScrapeTask::WorkSeasons(work_id))
        {
            tracing::debug!(work_id, %error, "failed to enqueue season metadata prewarm");
        }
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

    let old_category: Option<String> =
        sqlx::query_scalar("SELECT category FROM media_works WHERE id = $1")
            .bind(work_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
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

    sqlx::query("UPDATE media_works SET is_locked = TRUE WHERE id = $1")
        .bind(target_work_id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    invalidate_admin_stats(&state).await;
    invalidate_work_image_meta(&state, target_work_id).await;
    broadcast_work_visibility(&state, target_work_id, true).await;
    if target_work_id != work_id {
        let remaining_folders: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sync_folders WHERE work_id = $1 AND is_ignored = FALSE",
        )
        .bind(work_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        invalidate_work_image_meta(&state, work_id).await;
        if remaining_folders == 0 {
            if let Some(category) = old_category.as_deref() {
                let library = match category {
                    "电影" | "movie" => Some("lib_movie"),
                    "电视剧" | "tv" => Some("lib_tv"),
                    "动漫" | "anime" => Some("lib_anime"),
                    "纪录片" | "documentary" => Some("lib_doc"),
                    "综艺" | "variety" => Some("lib_variety"),
                    _ => None,
                };
                if let Some(library) = library {
                    state.session_hub.broadcast(
                        None,
                        "LibraryChanged",
                        json!({
                            "ItemsAdded": [],
                            "ItemsRemoved": [format!("work_{work_id}")],
                            "ItemsUpdated": [],
                            "FoldersAddedTo": [],
                            "FoldersRemovedFrom": [library]
                        }),
                    );
                }
            }
        }
    }

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
    sqlx::query("UPDATE work_seasons SET scrape_attempts = 0 WHERE work_id = $1")
        .bind(work_id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    scrape_work_seasons_by_id(&state, work_id, true)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("作品分集刮削失败: {:#}", e),
            )
        })?;

    invalidate_admin_stats(&state).await;
    invalidate_work_image_meta(&state, work_id).await;
    broadcast_work_visibility(&state, work_id, true).await;

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

    invalidate_admin_stats(&state).await;
    invalidate_work_image_meta(&state, work_id).await;
    broadcast_work_visibility(&state, work_id, !payload.ignore).await;

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
    reason_kind: Option<String>,
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
    next_retry_at: Option<chrono::DateTime<chrono::Utc>>,
    is_locked: bool,
    is_ignored: bool,
    priority_boost: i32,
    work_id: Option<i64>,
    work_title: Option<String>,
    file_count: i64,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize, sqlx::FromRow)]
struct StagingReasonCounts {
    type_conflict: i64,
    title_conflict: i64,
    year_conflict: i64,
    has_hint: i64,
    no_hint: i64,
    no_reason: i64,
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
    let kw = kw_raw.map(|s| {
        format!(
            "%{}%",
            s.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )
    });
    let kw_tmdb_id: Option<i32> = kw_raw.and_then(|s| s.parse::<i32>().ok());
    let reason_kind = q
        .reason_kind
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM sync_folders f
        WHERE f.is_ignored = $1
          AND ($2::text IS NULL OR f.scrape_state = $2)
          AND ($3::text IS NULL OR f.category = $3)
          AND ($4::text IS NULL OR f.clean_title ILIKE $4 ESCAPE '\\' OR f.root_dir ILIKE $4 ESCAPE '\\' OR ($5::int4 IS NOT NULL AND f.parsed_tmdb_id = $5))
          AND (
            $6::text IS NULL
            OR ($6 = 'type_conflict' AND f.conflict_reason LIKE '类型冲突%')
            OR ($6 = 'title_conflict' AND f.conflict_reason LIKE '片名冲突%')
            OR ($6 = 'year_conflict' AND f.conflict_reason LIKE '年份冲突%')
            OR ($6 = 'has_hint' AND f.parsed_tmdb_id IS NOT NULL)
            OR ($6 = 'no_hint' AND f.parsed_tmdb_id IS NULL)
            OR ($6 = 'no_reason' AND f.conflict_reason IS NULL)
          )
        "#,
    )
    .bind(ignored_filter)
    .bind(&state_filter)
    .bind(&q.category)
    .bind(&kw)
    .bind(kw_tmdb_id)
    .bind(&reason_kind)
    .fetch_one(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let items: Vec<StagingFolderRow> = sqlx::query_as(
        r#"
        SELECT
            f.id, f.root_dir AS folder_path, f.clean_title AS parsed_title, f.year AS parsed_year, f.parsed_tmdb_id AS hint_tmdb_id,
            f.inferred_media_type AS media_type, f.category, f.scrape_state, f.conflict_reason,
            f.retry_count::int4 AS retry_count, f.next_retry_at, f.is_locked, f.is_ignored, f.priority_boost,
            f.work_id, w.title AS work_title,
            (SELECT COUNT(*) FROM sync_files sf WHERE sf.folder_id = f.id) AS file_count,
            f.updated_at
        FROM sync_folders f
        LEFT JOIN media_works w ON w.id = f.work_id
        WHERE f.is_ignored = $1
          AND ($2::text IS NULL OR f.scrape_state = $2)
          AND ($3::text IS NULL OR f.category = $3)
          AND ($4::text IS NULL OR f.clean_title ILIKE $4 ESCAPE '\\' OR f.root_dir ILIKE $4 ESCAPE '\\' OR ($5::int4 IS NOT NULL AND f.parsed_tmdb_id = $5))
          AND (
            $8::text IS NULL
            OR ($8 = 'type_conflict' AND f.conflict_reason LIKE '类型冲突%')
            OR ($8 = 'title_conflict' AND f.conflict_reason LIKE '片名冲突%')
            OR ($8 = 'year_conflict' AND f.conflict_reason LIKE '年份冲突%')
            OR ($8 = 'has_hint' AND f.parsed_tmdb_id IS NOT NULL)
            OR ($8 = 'no_hint' AND f.parsed_tmdb_id IS NULL)
            OR ($8 = 'no_reason' AND f.conflict_reason IS NULL)
          )
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
    .bind(&reason_kind)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let reason_counts: Option<StagingReasonCounts> = if matches!(state_filter.as_deref(), Some("conflict" | "failed")) {
        Some(
            sqlx::query_as(
                r#"
                SELECT
                    COUNT(*) FILTER (WHERE f.conflict_reason LIKE '类型冲突%') AS type_conflict,
                    COUNT(*) FILTER (WHERE f.conflict_reason LIKE '片名冲突%') AS title_conflict,
                    COUNT(*) FILTER (WHERE f.conflict_reason LIKE '年份冲突%') AS year_conflict,
                    COUNT(*) FILTER (WHERE f.parsed_tmdb_id IS NOT NULL) AS has_hint,
                    COUNT(*) FILTER (WHERE f.parsed_tmdb_id IS NULL) AS no_hint,
                    COUNT(*) FILTER (WHERE f.conflict_reason IS NULL) AS no_reason
                FROM sync_folders f
                WHERE f.is_ignored = $1
                  AND f.scrape_state = $2
                  AND ($3::text IS NULL OR f.category = $3)
                "#,
            )
            .bind(ignored_filter)
            .bind(&state_filter)
            .bind(&q.category)
            .fetch_one(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        )
    } else {
        None
    };

    Ok(Json(json!({ "items": items, "total": total, "reason_counts": reason_counts })))
}

#[derive(Serialize, sqlx::FromRow)]
struct StagingFileRow {
    id: i64,
    file_name: String,
    alist_path: String,
    season_number: i32,
    episode_number: i32,
    version_tag: String,
    container: String,
    file_size_bytes: Option<i64>,
    subtitles: Vec<String>,
}

pub(super) async fn list_staging_folder_files(
    State(state): State<Arc<AppState>>,
    Path(folder_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let files: Vec<StagingFileRow> = sqlx::query_as(
        r#"
        SELECT id, file_name, alist_path, season_number, episode_number,
               version_tag, container, file_size_bytes, subtitles
        FROM sync_files
        WHERE folder_id = $1
        ORDER BY season_number ASC, episode_number ASC, id ASC
        LIMIT 500
        "#,
    )
    .bind(folder_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "files": files })))
}

pub(super) async fn scrape_staging_folder(
    State(state): State<Arc<AppState>>,
    Path(folder_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    sqlx::query(
        "UPDATE sync_folders SET scrape_state = 'pending', is_locked = FALSE, retry_count = 0, next_retry_at = NULL, conflict_reason = NULL WHERE id = $1",
    )
    .bind(folder_id)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    scrape_single_folder(&state, folder_id)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("目录刮削失败: {:#}", e)))?;

    let state_after: Option<(String, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT scrape_state, conflict_reason, work_id FROM sync_folders WHERE id = $1",
    )
    .bind(folder_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    invalidate_admin_stats(&state).await;

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

    invalidate_admin_stats(&state).await;
    invalidate_work_image_meta(&state, work_id).await;
    broadcast_work_visibility(&state, work_id, true).await;

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
    let linked_work: Option<i64> =
        sqlx::query_scalar("SELECT work_id FROM sync_folders WHERE id = $1")
            .bind(folder_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .flatten();
    sqlx::query("UPDATE sync_folders SET is_ignored = $1, updated_at = NOW() WHERE id = $2")
        .bind(payload.ignore)
        .bind(folder_id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    invalidate_admin_stats(&state).await;
    if let Some(work_id) = linked_work {
        invalidate_work_image_meta(&state, work_id).await;
        broadcast_work_visibility(&state, work_id, !payload.ignore).await;
    }

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
    let mut queued = 0;
    let mut rejected = 0;
    for fid in payload.folder_ids {
        match state.scraper.priority_tx.try_send(ScrapeTask::Folder(fid)) {
            Ok(()) => queued += 1,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => rejected += 1,
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!("刮削队列已关闭，成功加入 {} 个目录，剩余任务未提交", queued),
                ));
            }
        }
    }
    Ok(Json(json!({
        "status": if rejected == 0 { "ok" } else { "partial" },
        "queued": queued,
        "rejected": rejected,
        "message": format!("已将 {} 个目录加入高优先级刮削队列，{} 个因队列已满未提交", queued, rejected)
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

    invalidate_admin_stats(&state).await;

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
