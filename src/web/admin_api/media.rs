use crate::{scraper::scrape_single_media, state::AppState};
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


pub(super) async fn list_admin_media(
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
pub(super) async fn get_admin_media_detail(
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
pub(super) async fn prewarm_admin_media_dirs(
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
pub(super) struct BindTmdbReq {
    tmdb_id: i32,
    media_type: Option<String>,
}

pub(super) async fn bind_tmdb_id(
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
pub(super) async fn scrape_one_now(
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
pub(super) struct ToggleIgnoreReq {
    ignore: bool,
}

/// 将不想要的影视移出或恢复到媒体库（软屏蔽，增量同步不会重新导入；同时联动同 tmdb_id 兄弟目录）
pub(super) async fn toggle_ignore_media(
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

pub(super) async fn retry_failed_scrapes(
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

