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
          AND ($3::text IS NULL OR w.title ILIKE $3 ESCAPE '\' OR w.original_title ILIKE $3 ESCAPE '\' OR ($4::int4 IS NOT NULL AND w.tmdb_id = $4))
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
          AND ($3::text IS NULL OR w.title ILIKE $3 ESCAPE '\' OR w.original_title ILIKE $3 ESCAPE '\' OR ($4::int4 IS NOT NULL AND w.tmdb_id = $4))
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

#[derive(Deserialize)]
pub(super) struct UpdateMediaMetadataReq {
    title: String,
    original_title: Option<String>,
    year: Option<i32>,
    media_type: String,
    category: String,
    overview: Option<String>,
    rating: Option<f32>,
    genres: Option<Vec<String>>,
    countries: Option<Vec<String>>,
    networks: Option<Vec<String>>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    is_locked: Option<bool>,
    detach_tmdb: Option<bool>,
}

#[derive(Deserialize, Default)]
pub(super) struct UnbindTmdbReq {
    clear_poster: Option<bool>,
    reset_to_staging: Option<bool>,
}

fn clean_opt_text(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn clean_string_list(v: Option<Vec<String>>) -> Vec<String> {
    v.unwrap_or_default()
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 手动维护正式作品元数据，并默认锁定防止被自动刮削覆盖
pub(super) async fn update_media_metadata(
    State(state): State<Arc<AppState>>,
    Path(work_id): Path<i64>,
    Json(payload): Json<UpdateMediaMetadataReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let title = payload.title.trim().to_string();
    if title.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "片名不能为空".into()));
    }
    let media_type = match payload.media_type.trim() {
        "movie" | "tv" => payload.media_type.trim().to_string(),
        _ => return Err((StatusCode::BAD_REQUEST, "媒体类型仅支持 movie 或 tv".into())),
    };
    let category = match payload.category.trim() {
        "电影" | "电视剧" | "动漫" | "纪录片" | "综艺" => {
            payload.category.trim().to_string()
        }
        _ => return Err((StatusCode::BAD_REQUEST, "无效的媒体分类".into())),
    };

    let original_title = clean_opt_text(payload.original_title);
    let overview = clean_opt_text(payload.overview);
    let poster_path = clean_opt_text(payload.poster_path);
    let backdrop_path = clean_opt_text(payload.backdrop_path);
    let rating = payload.rating.unwrap_or(0.0).clamp(0.0, 10.0);
    let genres = clean_string_list(payload.genres);
    let countries = clean_string_list(payload.countries);
    let networks = clean_string_list(payload.networks);
    let is_locked = payload.is_locked.unwrap_or(true);
    let detach_tmdb = payload.detach_tmdb.unwrap_or(false);

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let updated: Option<i64> = sqlx::query_scalar(
        r#"
        UPDATE media_works
        SET title = $1,
            sort_title = $1,
            original_title = $2,
            year = $3,
            media_type = $4,
            category = $5,
            overview = $6,
            rating = $7,
            genres = $8,
            countries = $9,
            networks = $10,
            poster_path = $11,
            backdrop_path = $12,
            is_locked = $13,
            tmdb_id = CASE WHEN $14 THEN NULL ELSE tmdb_id END,
            metadata_version = 2,
            metadata_refresh_requested = FALSE,
            updated_at = NOW()
        WHERE id = $15
        RETURNING id
        "#,
    )
    .bind(&title)
    .bind(&original_title)
    .bind(payload.year)
    .bind(&media_type)
    .bind(&category)
    .bind(&overview)
    .bind(rating)
    .bind(&genres)
    .bind(&countries)
    .bind(&networks)
    .bind(&poster_path)
    .bind(&backdrop_path)
    .bind(is_locked)
    .bind(detach_tmdb)
    .bind(work_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if updated.is_none() {
        return Err((StatusCode::NOT_FOUND, "作品条目不存在".into()));
    }

    sqlx::query(
        r#"
        UPDATE sync_folders
        SET category = $1,
            inferred_media_type = $2,
            parsed_tmdb_id = CASE WHEN $3 THEN NULL ELSE parsed_tmdb_id END,
            is_locked = CASE WHEN $3 OR $4 THEN TRUE ELSE is_locked END,
            updated_at = NOW()
        WHERE work_id = $5
        "#,
    )
    .bind(&category)
    .bind(&media_type)
    .bind(detach_tmdb)
    .bind(is_locked)
    .bind(work_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    invalidate_admin_stats(&state).await;
    invalidate_work_image_meta(&state, work_id).await;
    broadcast_work_visibility(&state, work_id, true).await;

    Ok(Json(json!({
        "status": "ok",
        "work_id": work_id,
        "message": "已保存手动维护信息并锁定防覆盖"
    })))
}

/// 取消作品的 TMDB 刮削关联：默认保留在海报墙并转为手动维护；可选退回待处理中间表
pub(super) async fn unbind_media_tmdb(
    State(state): State<Arc<AppState>>,
    Path(work_id): Path<i64>,
    Json(payload): Json<UnbindTmdbReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let clear_poster = payload.clear_poster.unwrap_or(false);
    let reset_to_staging = payload.reset_to_staging.unwrap_or(false);

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if reset_to_staging {
        sqlx::query(
            r#"
            UPDATE sync_folders
            SET work_id = NULL,
                parsed_tmdb_id = NULL,
                scrape_state = 'pending',
                is_locked = FALSE,
                retry_count = 0,
                next_retry_at = NULL,
                conflict_reason = NULL,
                updated_at = NOW()
            WHERE work_id = $1
            "#,
        )
        .bind(work_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        invalidate_admin_stats(&state).await;
        invalidate_work_image_meta(&state, work_id).await;
        broadcast_work_visibility(&state, work_id, false).await;

        return Ok(Json(json!({
            "status": "ok",
            "reset_to_staging": true,
            "message": "已取消刮削并将关联目录退回待处理中间表"
        })));
    }

    let updated: Option<i64> = sqlx::query_scalar(
        r#"
        UPDATE media_works
        SET tmdb_id = NULL,
            is_locked = TRUE,
            metadata_version = 2,
            metadata_refresh_requested = FALSE,
            poster_path = CASE WHEN $2 THEN NULL ELSE poster_path END,
            backdrop_path = CASE WHEN $2 THEN NULL ELSE backdrop_path END,
            updated_at = NOW()
        WHERE id = $1
        RETURNING id
        "#,
    )
    .bind(work_id)
    .bind(clear_poster)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if updated.is_none() {
        return Err((StatusCode::NOT_FOUND, "作品条目不存在".into()));
    }

    sqlx::query(
        r#"
        UPDATE sync_folders
        SET parsed_tmdb_id = NULL,
            is_locked = TRUE,
            scrape_state = 'verified',
            conflict_reason = NULL,
            updated_at = NOW()
        WHERE work_id = $1
        "#,
    )
    .bind(work_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    invalidate_admin_stats(&state).await;
    invalidate_work_image_meta(&state, work_id).await;
    broadcast_work_visibility(&state, work_id, true).await;

    Ok(Json(json!({
        "status": "ok",
        "reset_to_staging": false,
        "message": "已取消 TMDB 刮削关联并转为手动维护（保留在海报墙）"
    })))
}

/// 将已合并在多目录作品中的某个物理目录拆分为独立的手动维护作品（保留在海报墙）
pub(super) async fn split_folder_to_new_work(
    State(state): State<Arc<AppState>>,
    Path(folder_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    #[derive(sqlx::FromRow)]
    struct FolderSplitSource {
        work_id: Option<i64>,
        clean_title: String,
        year: Option<i32>,
        inferred_media_type: String,
        category: String,
    }

    let src: Option<FolderSplitSource> = sqlx::query_as(
        "SELECT work_id, clean_title, year, inferred_media_type, category FROM sync_folders WHERE id = $1",
    )
    .bind(folder_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some(src) = src else {
        return Err((StatusCode::NOT_FOUND, "物理目录不存在".into()));
    };
    let Some(old_work_id) = src.work_id else {
        return Err((StatusCode::BAD_REQUEST, "该目录尚未关联正式作品".into()));
    };

    let sibling_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sync_folders WHERE work_id = $1 AND is_ignored = FALSE",
    )
    .bind(old_work_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if sibling_count <= 1 {
        return Err((
            StatusCode::BAD_REQUEST,
            "当前作品仅关联这一个目录，无需拆分；可直接「取消 TMDB 刮削」或「编辑信息」".into(),
        ));
    }

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let new_work_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO media_works (media_type, tmdb_id, title, sort_title, year, category, is_locked, metadata_version)
        VALUES ($1, NULL, $2, $2, $3, $4, TRUE, 2)
        RETURNING id
        "#,
    )
    .bind(&src.inferred_media_type)
    .bind(&src.clean_title)
    .bind(src.year)
    .bind(&src.category)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    sqlx::query(
        r#"
        UPDATE sync_folders
        SET work_id = $1,
            parsed_tmdb_id = NULL,
            scrape_state = 'verified',
            conflict_reason = NULL,
            is_locked = TRUE,
            updated_at = NOW()
        WHERE id = $2
        "#,
    )
    .bind(new_work_id)
    .bind(folder_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    invalidate_admin_stats(&state).await;
    invalidate_work_image_meta(&state, old_work_id).await;
    invalidate_work_image_meta(&state, new_work_id).await;
    broadcast_work_visibility(&state, old_work_id, true).await;
    broadcast_work_visibility(&state, new_work_id, true).await;

    Ok(Json(json!({
        "status": "ok",
        "old_work_id": old_work_id,
        "new_work_id": new_work_id,
        "message": "已将该目录拆分为独立手动维护条目并保留在海报墙"
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

#[derive(Deserialize)]
pub(super) struct UpdateFileEpisodeReq {
    season_number: i32,
    episode_number: i32,
    episode_title: Option<String>,
    convert_to_tv: Option<bool>,
}

/// 手动修正单个物理文件的季/集号（及可选单集标题），多集时自动切换作品为剧集并锁定
pub(super) async fn update_file_episode(
    State(state): State<Arc<AppState>>,
    Path(file_id): Path<i64>,
    Json(payload): Json<UpdateFileEpisodeReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if payload.season_number < 0 || payload.episode_number < 1 {
        return Err((
            StatusCode::BAD_REQUEST,
            "季号必须 >= 0，集号必须 >= 1".into(),
        ));
    }
    let ep_title = clean_opt_text(payload.episode_title);

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let work_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT f.work_id
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        WHERE sf.id = $1
        "#,
    )
    .bind(file_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .flatten();

    let Some(work_id) = work_id else {
        return Err((StatusCode::NOT_FOUND, "物理文件不存在或尚未关联正式作品".into()));
    };

    sqlx::query("UPDATE sync_files SET season_number = $1, episode_number = $2 WHERE id = $3")
        .bind(payload.season_number)
        .bind(payload.episode_number)
        .bind(file_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(ref title) = ep_title {
        sqlx::query(
            r#"
            INSERT INTO work_episodes (work_id, season_number, episode_number, title)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (work_id, season_number, episode_number) DO UPDATE
            SET title = EXCLUDED.title
            "#,
        )
        .bind(work_id)
        .bind(payload.season_number)
        .bind(payload.episode_number)
        .bind(title)
        .execute(&mut *tx)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    let distinct_ep_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(DISTINCT (sf.season_number, sf.episode_number))
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        WHERE f.work_id = $1 AND f.is_ignored = FALSE
        "#,
    )
    .bind(work_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let should_tv = payload.convert_to_tv.unwrap_or(distinct_ep_count > 1);
    if should_tv {
        sqlx::query(
            r#"
            UPDATE media_works
            SET media_type = 'tv',
                is_locked = TRUE,
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(work_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    invalidate_admin_stats(&state).await;
    broadcast_work_visibility(&state, work_id, true).await;

    Ok(Json(json!({
        "status": "ok",
        "work_id": work_id,
        "season_number": payload.season_number,
        "episode_number": payload.episode_number,
        "converted_to_tv": should_tv,
        "message": format!(
            "已将文件归入 S{:02}E{:02}{}",
            payload.season_number,
            payload.episode_number,
            if should_tv { "（已切换为剧集并锁定）" } else { "" }
        )
    })))
}

#[derive(Deserialize)]
pub(super) struct SplitWorkEpisodesReq {
    file_ids: Vec<i64>,
    season_number: Option<i32>,
    start_episode: Option<i32>,
}

/// 将误合并为“同集多版本”的多个文件按顺序拆分为连续剧集（如上下集 -> S01E01, S01E02）
pub(super) async fn split_work_group_to_episodes(
    State(state): State<Arc<AppState>>,
    Path(work_id): Path<i64>,
    Json(payload): Json<SplitWorkEpisodesReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if payload.file_ids.len() < 2 {
        return Err((StatusCode::BAD_REQUEST, "至少需要选择 2 个文件才能拆分为多集".into()));
    }
    let season_number = payload.season_number.unwrap_or(1);
    let start_episode = payload.start_episode.unwrap_or(1);
    if season_number < 0 || start_episode < 1 {
        return Err((StatusCode::BAD_REQUEST, "季号必须 >= 0，起始集号必须 >= 1".into()));
    }

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let valid_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        WHERE f.work_id = $1 AND sf.id = ANY($2)
        "#,
    )
    .bind(work_id)
    .bind(&payload.file_ids)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if valid_count != payload.file_ids.len() as i64 {
        return Err((StatusCode::BAD_REQUEST, "部分文件不属于当前作品".into()));
    }

    for (idx, fid) in payload.file_ids.iter().enumerate() {
        let ep_num = start_episode + idx as i32;
        sqlx::query("UPDATE sync_files SET season_number = $1, episode_number = $2 WHERE id = $3")
            .bind(season_number)
            .bind(ep_num)
            .bind(*fid)
            .execute(&mut *tx)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    sqlx::query(
        r#"
        UPDATE media_works
        SET media_type = 'tv',
            is_locked = TRUE,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(work_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    invalidate_admin_stats(&state).await;
    broadcast_work_visibility(&state, work_id, true).await;

    let end_ep = start_episode + payload.file_ids.len() as i32 - 1;
    Ok(Json(json!({
        "status": "ok",
        "message": format!(
            "已将 {} 个文件拆分为 S{:02}E{:02}-E{:02}，并切换为剧集模式",
            payload.file_ids.len(),
            season_number,
            start_episode,
            end_ep
        )
    })))
}

#[derive(sqlx::FromRow)]
struct ReparseFileRow {
    file_id: i64,
    folder_id: i64,
    file_name: String,
    alist_path: String,
    container: String,
    season_number: i32,
    episode_number: i32,
    version_tag: String,
    priority_score: i32,
}

/// 预览对当前作品关联物理文件的 STRM 重新解析结果（不写入数据库）
pub(super) async fn preview_reparse_media_strm(
    State(state): State<Arc<AppState>>,
    Path(work_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let work: Option<(String, String)> =
        sqlx::query_as("SELECT title, media_type FROM media_works WHERE id = $1")
            .bind(work_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some((work_title, current_media_type)) = work else {
        return Err((StatusCode::NOT_FOUND, "作品不存在".into()));
    };

    let rows: Vec<ReparseFileRow> = sqlx::query_as(
        r#"
        SELECT sf.id AS file_id, sf.folder_id, sf.file_name, sf.alist_path, sf.container,
               sf.season_number, sf.episode_number, sf.version_tag, sf.priority_score
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        WHERE f.work_id = $1 AND f.is_ignored = FALSE
        ORDER BY sf.folder_id ASC, sf.file_name ASC, sf.id ASC
        "#,
    )
    .bind(work_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut folder_counts: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
    for r in &rows {
        *folder_counts.entry(r.folder_id).or_insert(0) += 1;
    }

    let mut distinct_proposed = std::collections::HashSet::new();
    let mut changed_count = 0usize;
    let mut items = Vec::with_capacity(rows.len());

    for r in rows {
        let f_count = folder_counts.get(&r.folder_id).copied().unwrap_or(1);
        let sug = crate::sync::parser::reparse_file_for_manual_preview(
            &r.alist_path,
            &r.file_name,
            &r.container,
            &work_title,
            f_count,
        );
        distinct_proposed.insert((sug.season_number, sug.episode_number));
        let changed = r.season_number != sug.season_number
            || r.episode_number != sug.episode_number
            || r.version_tag != sug.version_tag
            || r.priority_score != sug.priority_score;
        if changed {
            changed_count += 1;
        }
        items.push(json!({
            "file_id": r.file_id,
            "folder_id": r.folder_id,
            "file_name": r.file_name,
            "alist_path": r.alist_path,
            "current": {
                "season_number": r.season_number,
                "episode_number": r.episode_number,
                "version_tag": r.version_tag,
                "priority_score": r.priority_score,
            },
            "proposed": sug,
            "changed": changed,
        }));
    }

    let suggested_media_type = if distinct_proposed.len() > 1 {
        "tv"
    } else {
        current_media_type.as_str()
    };

    Ok(Json(json!({
        "work_id": work_id,
        "current_media_type": current_media_type,
        "suggested_media_type": suggested_media_type,
        "changed_count": changed_count,
        "items": items,
    })))
}

#[derive(Deserialize)]
pub(super) struct ApplyReparseItem {
    file_id: i64,
    season_number: i32,
    episode_number: i32,
    version_tag: Option<String>,
    priority_score: Option<i32>,
}

#[derive(Deserialize)]
pub(super) struct ApplyReparseReq {
    convert_to_tv: Option<bool>,
    items: Vec<ApplyReparseItem>,
}

/// 确认实施手动 STRM 重新解析结果
pub(super) async fn apply_reparse_media_strm(
    State(state): State<Arc<AppState>>,
    Path(work_id): Path<i64>,
    Json(payload): Json<ApplyReparseReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if payload.items.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "未提交任何需要应用的文件项".into()));
    }
    for item in &payload.items {
        if item.season_number < 0 || item.episode_number < 1 {
            return Err((
                StatusCode::BAD_REQUEST,
                "季号必须 >= 0，集号必须 >= 1".into(),
            ));
        }
    }

    let file_ids: Vec<i64> = payload.items.iter().map(|i| i.file_id).collect();
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let valid_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        WHERE f.work_id = $1 AND sf.id = ANY($2)
        "#,
    )
    .bind(work_id)
    .bind(&file_ids)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if valid_count != file_ids.len() as i64 {
        return Err((StatusCode::BAD_REQUEST, "部分文件不属于当前作品或存在重复项".into()));
    }

    for item in &payload.items {
        let vtag = item
            .version_tag
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        sqlx::query(
            r#"
            UPDATE sync_files
            SET season_number = $1,
                episode_number = $2,
                version_tag = COALESCE($3, version_tag),
                priority_score = COALESCE($4, priority_score)
            WHERE id = $5
            "#,
        )
        .bind(item.season_number)
        .bind(item.episode_number)
        .bind(vtag)
        .bind(item.priority_score)
        .bind(item.file_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    let distinct_ep_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(DISTINCT (sf.season_number, sf.episode_number))
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        WHERE f.work_id = $1 AND f.is_ignored = FALSE
        "#,
    )
    .bind(work_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let should_tv = payload.convert_to_tv.unwrap_or(distinct_ep_count > 1);
    if should_tv {
        sqlx::query(
            r#"
            UPDATE media_works
            SET media_type = 'tv',
                is_locked = TRUE,
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(work_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    invalidate_admin_stats(&state).await;
    broadcast_work_visibility(&state, work_id, true).await;

    Ok(Json(json!({
        "status": "ok",
        "updated_count": payload.items.len(),
        "converted_to_tv": should_tv,
        "message": format!(
            "已应用 {} 个文件的 STRM 重新解析结果{}",
            payload.items.len(),
            if should_tv { "（已切换为剧集并锁定）" } else { "" }
        )
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
          AND ($4::text IS NULL OR f.clean_title ILIKE $4 ESCAPE '\' OR f.root_dir ILIKE $4 ESCAPE '\' OR ($5::int4 IS NOT NULL AND f.parsed_tmdb_id = $5))
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
          AND ($4::text IS NULL OR f.clean_title ILIKE $4 ESCAPE '\' OR f.root_dir ILIKE $4 ESCAPE '\' OR ($5::int4 IS NOT NULL AND f.parsed_tmdb_id = $5))
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
    if queued > 0 {
        state
            .scraper
            .runtime
            .add_batch_items("批量刮削指定目录", queued);
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

    let reset_count = res.rows_affected();
    if reset_count > 0 {
        state
            .scraper
            .runtime
            .start_batch("重试全部失败/存疑项", reset_count as usize);
    }
    invalidate_admin_stats(&state).await;

    Ok(Json(json!({
        "status": "ok",
        "reset_count": reset_count,
        "message": format!("已将 {} 个失败/冲突目录重新加入待刮削队列", reset_count)
    })))
}

async fn build_scrape_status_json(state: &AppState) -> Result<Value, (StatusCode, String)> {
    let pending_db: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sync_folders WHERE scrape_state = 'pending' AND is_ignored = FALSE AND is_locked = FALSE",
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let refresh_db: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM media_works WHERE NOT is_locked AND metadata_refresh_requested = TRUE AND metadata_retry_count < 3",
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let auto_bg = state.config.load().scraper_auto_background;
    Ok(state
        .scraper
        .runtime
        .status_json(pending_db, refresh_db, auto_bg))
}

pub(super) async fn get_scrape_task_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    Ok(Json(build_scrape_status_json(&state).await?))
}

pub(super) async fn pause_scrape_task(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.scraper.runtime.pause();
    let mut status = build_scrape_status_json(&state).await?;
    if let Some(obj) = status.as_object_mut() {
        obj.insert("message".to_string(), json!("已暂停刮削任务，当前正在处理的单条请求结束后将停止拉取下一项"));
    }
    Ok(Json(status))
}

pub(super) async fn resume_scrape_task(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.scraper.runtime.resume();
    let mut status = build_scrape_status_json(&state).await?;
    if let Some(obj) = status.as_object_mut() {
        obj.insert("message".to_string(), json!("已恢复后台刮削任务"));
    }
    Ok(Json(status))
}

pub(super) async fn cancel_scrape_task(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.scraper.runtime.cancel();
    sqlx::query(
        "UPDATE media_works SET metadata_refresh_requested = FALSE WHERE metadata_refresh_requested = TRUE",
    )
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    invalidate_admin_stats(&state).await;
    let mut status = build_scrape_status_json(&state).await?;
    if let Some(obj) = status.as_object_mut() {
        obj.insert("message".to_string(), json!("已取消当前刮削队列并停止继续拉取"));
    }
    Ok(Json(status))
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
