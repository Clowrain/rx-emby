use super::auth::resolve_valid_user_id;
use super::playback::{
    build_media_sources_for_episode, build_media_sources_for_movie, PlaybackQuery,
};
use super::sessions::{is_media_favorited, load_media_episode_histories, load_user_favorite_keys};
use super::views::get_user_allowed_categories;
use crate::smart_views::{can_access_view, VirtualLibraryRule};
use crate::state::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::HashSet, sync::Arc};
use uuid::Uuid;

#[derive(Deserialize, Default)]
pub(super) struct ItemsQuery {
    #[serde(rename = "ParentId", alias = "parentId")]
    pub(super) parent_id: Option<String>,
    #[serde(rename = "SearchTerm", alias = "searchTerm")]
    pub(super) search_term: Option<String>,
    #[serde(rename = "Filters", alias = "filters")]
    pub(super) filters: Option<String>,
    #[serde(rename = "IncludeItemTypes", alias = "includeItemTypes")]
    pub(super) include_item_types: Option<String>,
    #[serde(rename = "SortBy", alias = "sortBy")]
    pub(super) sort_by: Option<String>,
    #[serde(rename = "SortOrder", alias = "sortOrder")]
    pub(super) sort_order: Option<String>,
    #[serde(rename = "Ids", alias = "ids")]
    pub(super) ids: Option<String>,
    #[serde(rename = "UserId", alias = "userId")]
    pub(super) user_id: Option<String>,
    #[serde(rename = "StartIndex", alias = "startIndex", default)]
    pub(super) start_index: i64,
    #[serde(rename = "Limit", alias = "limit")]
    pub(super) limit: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct MediaListRow {
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
    created_at: chrono::DateTime<chrono::Utc>,
}

pub(super) async fn get_user_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path_user_id): Path<String>,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id(&state, Some(&path_user_id), &headers).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.start_index.max(0);
    let cfg = state.config.load();

    // 1. 解析 Filters 参数（如 IsFavorite, IsResumable, IsPlayed, IsUnplayed）
    let filter_set: HashSet<&str> = q
        .filters
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if filter_set.contains("IsResumable") {
        return get_resume_items_inner(&state, &user_id, limit).await;
    }

    // 2. 解析 IncludeItemTypes 参数（如 Series, Movie, Episode, Video, Playlist, BoxSet）
    let type_set: HashSet<&str> = q
        .include_item_types
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if !type_set.is_empty() {
        let has_media_type = type_set.contains("Series")
            || type_set.contains("Movie")
            || type_set.contains("Episode")
            || type_set.contains("Video");
        // 若客户端仅请求 Playlist / BoxSet / MusicAlbum 等不支持的集合类型，直接返回空集
        if !has_media_type {
            return Ok(Json(json!({ "Items": [], "TotalRecordCount": 0 })));
        }
        // 若客户端请求收藏的 Episode/Video（我们目前按整部影视维度收藏），直接返回空列表防止把电视剧主条目混入单集列表
        if filter_set.contains("IsFavorite")
            && !type_set.contains("Series")
            && !type_set.contains("Movie")
        {
            return Ok(Json(json!({ "Items": [], "TotalRecordCount": 0 })));
        }
    }

    // 3. 如果是展开某部电视剧的季或单集列表（ParentId 为 "media_123"）
    if let Some(ref pid) = q.parent_id {
        if let Some(raw_id) = pid.strip_prefix("media_") {
            if let Ok(mid) = raw_id.parse::<i64>() {
                return get_episodes_for_media(&state, &user_id, mid).await;
            }
        }
    }

    // 视图授权独立于基础分类；获得某个视图权限即可访问其匹配内容。
    let mut allowed_user_cats = get_user_allowed_categories(&state, &user_id).await;
    let mut vlib_rule: Option<VirtualLibraryRule> = None;
    if let Some(pid) = q.parent_id.as_deref().filter(|p| p.starts_with("lib_vlib_")) {
        let vid = pid.trim_start_matches("lib_vlib_").parse::<i32>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的智能视图 ID".into()))?;
        let row: Option<(String, Value)> = sqlx::query_as(
            "SELECT name, filter_rules FROM virtual_libraries WHERE id=$1 AND is_enabled=TRUE")
            .bind(vid).fetch_optional(&state.db).await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
        let Some((name, rules)) = row else {
            return Err((StatusCode::NOT_FOUND, "智能视图不存在或已停用".into()));
        };
        if !can_access_view(allowed_user_cats.as_deref(), pid, &name) {
            return Err((StatusCode::FORBIDDEN, "没有该智能视图的访问权限".into()));
        }
        let rule: VirtualLibraryRule = serde_json::from_value(rules)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,format!("智能视图规则损坏：{}",e)))?;
        rule.validate().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e))?;
        vlib_rule = Some(rule);
        allowed_user_cats = None;
    }
    let rule_json = vlib_rule.as_ref().map(serde_json::to_value).transpose()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;

    let category_filter = match q.parent_id.as_deref() {
        Some("lib_movie") => Some("电影"),
        Some("lib_tv") => Some("电视剧"),
        Some("lib_anime") => Some("动漫"),
        Some("lib_doc") => Some("纪录片"),
        Some("lib_variety") => Some("综艺"),
        _ => None,
    };

    // 客户端类型与视图规则各自过滤，冲突时结果为空。
    let media_type_filter = match (type_set.contains("Movie"), type_set.contains("Series")) {
        (true, false) => Some("movie"),
        (false, true) => Some("tv"),
        _ => None,
    };

    let only_favorite = filter_set.contains("IsFavorite");
    let only_played = filter_set.contains("IsPlayed");
    let only_unplayed = filter_set.contains("IsUnplayed");

    let search_pattern = q
        .search_term
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{}%", s));

    // 解析指定 Ids 批量查询（如 Ids=media_10,media_20）
    let explicit_ids: Option<Vec<i64>> = q.ids.as_deref().map(|raw| {
        raw.split(',')
            .filter_map(|s| {
                s.trim()
                    .strip_prefix("media_")
                    .or(Some(s.trim()))
                    .and_then(|v| v.parse::<i64>().ok())
            })
            .collect()
    });

    // 4. 组装排序规则（完整支持逗号分隔多字段 SortBy 与 SortOrder），并在同 (tmdb_id, media_type) 多目录间自动折叠去重
    let sort_key = q
        .sort_by
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| vlib_rule.as_ref().and_then(|r| r.sort_by.as_deref()))
        .unwrap_or("");
    let order_clause = build_emby_order_clause(sort_key, q.sort_order.as_deref());

    let sql = format!(
        r#"
        SELECT id, media_type, tmdb_id, title, original_title, year, premiered_at, overview, rating, poster_path, backdrop_path, created_at
        FROM (
            SELECT DISTINCT ON (COALESCE(m.tmdb_id::text || ':' || m.media_type, 'raw:' || m.id::text))
                m.id, m.media_type, m.tmdb_id, m.title, m.original_title, m.year, m.premiered_at, m.overview, m.rating, m.poster_path, m.backdrop_path, m.created_at, m.updated_at, (m.tmdb_metadata->>'popularity')::float8 AS popularity
            FROM media_items m
            WHERE m.is_ignored = FALSE
              AND ($1::text IS NULL OR m.category = $1)
              AND ($2::text IS NULL OR m.title ILIKE $2 OR m.original_title ILIKE $2)
              AND ($3::text IS NULL OR m.media_type = $3)
              AND ($4::int8[] IS NULL OR m.id = ANY($4))
              AND (NOT $5 OR EXISTS (
                  SELECT 1 FROM user_favorites uf
                  WHERE uf.user_id = $6
                    AND ((uf.tmdb_id IS NOT NULL AND uf.tmdb_id = m.tmdb_id AND uf.media_type = m.media_type) OR uf.media_item_id = m.id)
              ))
              AND (NOT $7 OR EXISTS (
                  SELECT 1 FROM play_histories ph
                  WHERE ph.user_id = $6 AND ph.is_played = TRUE
                    AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = m.tmdb_id AND ph.media_type = m.media_type) OR ph.media_item_id = m.id)
              ))
              AND (NOT $8 OR NOT EXISTS (
                  SELECT 1 FROM play_histories ph
                  WHERE ph.user_id = $6 AND ph.is_played = TRUE
                    AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = m.tmdb_id AND ph.media_type = m.media_type) OR ph.media_item_id = m.id)
              ))
              AND ($11::text[] IS NULL OR m.category = ANY($11))
              AND ($12::jsonb IS NULL OR {view_predicate})
            ORDER BY
                COALESCE(m.tmdb_id::text || ':' || m.media_type, 'raw:' || m.id::text),
                (m.scrape_status IN (1, 3)) DESC,
                (m.poster_path IS NOT NULL) DESC,
                m.id ASC
        ) AS deduped
        ORDER BY {}
        LIMIT $9 OFFSET $10
        "#,
        order_clause,
        view_predicate = vlib_rule.as_ref().map(|r| r.sql_predicate("$12::jsonb")).unwrap_or_else(|| "($12::jsonb IS NULL)".into())
    );

    let rows: Vec<MediaListRow> = sqlx::query_as(&sql)
        .bind(category_filter)
        .bind(&search_pattern)
        .bind(&media_type_filter)
        .bind(&explicit_ids)
        .bind(only_favorite)
        .bind(&user_id)
        .bind(only_played)
        .bind(only_unplayed)
        .bind(limit)
        .bind(offset)
        .bind(&allowed_user_cats)
    .bind(&rule_json)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let total: i64 = sqlx::query_scalar(
        &format!(r#"
        SELECT COUNT(DISTINCT COALESCE(m.tmdb_id::text || ':' || m.media_type, 'raw:' || m.id::text))
        FROM media_items m
        WHERE m.is_ignored = FALSE
          AND ($1::text IS NULL OR m.category = $1)
          AND ($2::text IS NULL OR m.title ILIKE $2 OR m.original_title ILIKE $2)
          AND ($3::text IS NULL OR m.media_type = $3)
          AND ($4::int8[] IS NULL OR m.id = ANY($4))
          AND (NOT $5 OR EXISTS (
              SELECT 1 FROM user_favorites uf
              WHERE uf.user_id = $6
                AND ((uf.tmdb_id IS NOT NULL AND uf.tmdb_id = m.tmdb_id AND uf.media_type = m.media_type) OR uf.media_item_id = m.id)
          ))
          AND (NOT $7 OR EXISTS (
              SELECT 1 FROM play_histories ph
              WHERE ph.user_id = $6 AND ph.is_played = TRUE
                AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = m.tmdb_id AND ph.media_type = m.media_type) OR ph.media_item_id = m.id)
          ))
          AND (NOT $8 OR NOT EXISTS (
              SELECT 1 FROM play_histories ph
              WHERE ph.user_id = $6 AND ph.is_played = TRUE
                AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = m.tmdb_id AND ph.media_type = m.media_type) OR ph.media_item_id = m.id)
          ))
          AND ($9::text[] IS NULL OR m.category = ANY($9))
          AND ($10::jsonb IS NULL OR {})
        "#, vlib_rule.as_ref().map(|r| r.sql_predicate("$10::jsonb")).unwrap_or_else(|| "TRUE".into())),
    )
    .bind(category_filter)
    .bind(&search_pattern)
    .bind(&media_type_filter)
    .bind(&explicit_ids)
    .bind(only_favorite)
    .bind(&user_id)
    .bind(only_played)
    .bind(only_unplayed)
    .bind(&allowed_user_cats)
    .bind(&rule_json)
    .fetch_one(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;

    // 批量加载当前用户的收藏集合，填充 UserData
    let fav_set = load_user_favorite_keys(&state, &user_id).await;

    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let emby_type = if r.media_type == "movie" { "Movie" } else { "Series" };
            let is_folder = r.media_type != "movie";
            let mut image_tags = json!({});
            if r.poster_path.is_some() {
                image_tags["Primary"] = json!("v1");
            }
            if r.backdrop_path.is_some() {
                image_tags["Backdrop"] = json!("v1");
            }

            let is_fav = only_favorite || is_media_favorited(&fav_set, r.id, r.tmdb_id, &r.media_type);

            let mut obj = json!({
                "Id": format!("media_{}", r.id),
                "Name": r.title,
                "SortName": r.title,
                "OriginalTitle": r.original_title,
                "ServerId": cfg.server_id,
                "Type": emby_type,
                "IsFolder": is_folder,
                "ProductionYear": r.year,
                "DateCreated": r.created_at.to_rfc3339(),
                "Overview": r.overview,
                "CommunityRating": r.rating,
                "ImageTags": image_tags,
                "UserData": {
                    "PlaybackPositionTicks": 0,
                    "PlayCount": 0,
                    "IsFavorite": is_fav,
                    "Played": false
                }
            });
            if let Some(ref pdate) = r.premiered_at {
                if !pdate.is_empty() {
                    obj["PremiereDate"] = json!(pdate);
                }
            }
            obj
        })
        .collect();

    Ok(Json(json!({
        "Items": items,
        "TotalRecordCount": total
    })))
}

#[derive(sqlx::FromRow)]
struct EpisodeGroupRow {
    season_number: i32,
    episode_number: i32,
    rep_id: i64,
    title: Option<String>,
    overview: Option<String>,
    still_path: Option<String>,
    air_date: Option<String>,
    runtime: i32,
}

async fn get_episodes_for_media(
    state: &AppState,
    user_id: &str,
    media_id: i64,
) -> Result<Json<Value>, (StatusCode, String)> {
    let cfg = state.config.load();
    // 触发 P0 实时检查（若该剧未刮削，或有新同步集/未完结残缺集已过冷却期，立即异步刮削补齐）
    let _ = state.scraper.priority_tx.try_send(media_id);

    let media_meta: Option<(Option<i32>, String)> =
        sqlx::query_as("SELECT tmdb_id, media_type FROM media_items WHERE id = $1")
            .bind(media_id)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
    let (tmdb_id, mtype) = media_meta.unwrap_or((None, "tv".to_string()));
    let history_map = load_media_episode_histories(state, user_id, media_id, tmdb_id, &mtype).await;
    let sibling_ids = state.resolve_sibling_media_ids(media_id).await;

    // 跨所有同 tmdb_id 兄弟目录，按 (season_number, episode_number) 聚合多版本，取权重最高的一条作为代表单集
    let eps: Vec<EpisodeGroupRow> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (season_number, episode_number)
            season_number, episode_number, id AS rep_id, title, overview, still_path, air_date, runtime
        FROM episodes
        WHERE media_item_id = ANY($1)
        ORDER BY season_number ASC, episode_number ASC, (still_path IS NOT NULL) DESC, priority_score DESC, id ASC
        "#,
    )
    .bind(&sibling_ids)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let items: Vec<Value> = eps
        .into_iter()
        .map(|ep| {
            let (pos_ticks, dur_ticks, is_played) = history_map
                .get(&(ep.season_number, ep.episode_number))
                .copied()
                .unwrap_or((0, 0, false));
            let effective_dur = if dur_ticks > 0 {
                dur_ticks
            } else if ep.runtime > 0 {
                (ep.runtime as i64) * 600_000_000
            } else {
                0
            };

            let mut obj = json!({
                "Id": format!("ep_{}", ep.rep_id),
                "Name": ep.title.unwrap_or_else(|| format!("第 {} 集", ep.episode_number)),
                "ServerId": cfg.server_id,
                "SeriesId": format!("media_{}", media_id),
                "SeasonId": format!("season_{}_{}", media_id, ep.season_number),
                "Type": "Episode",
                "IsFolder": false,
                "IndexNumber": ep.episode_number,
                "ParentIndexNumber": ep.season_number,
                "Overview": ep.overview,
                "PremiereDate": ep.air_date,
                "MediaType": "Video",
                "ImageTags": if ep.still_path.is_some() { json!({"Primary": "v1"}) } else { json!({}) },
                "UserData": {
                    "PlaybackPositionTicks": pos_ticks,
                    "PlayCount": if is_played { 1 } else { 0 },
                    "IsFavorite": false,
                    "Played": is_played
                }
            });
            if effective_dur > 0 {
                obj["RunTimeTicks"] = json!(effective_dur);
            }
            obj
        })
        .collect();

    Ok(Json(json!({
        "Items": items,
        "TotalRecordCount": items.len()
    })))
}

pub(super) async fn get_item_detail(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((path_user_id, item_id)): Path<(String, String)>,
    Query(q): Query<PlaybackQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id(&state, Some(&path_user_id), &headers).await?;
    let cfg = state.config.load();
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("127.0.0.1:8096")
        .to_string();
    let token = q
        .api_key
        .clone()
        .or_else(|| {
            headers
                .get("x-emby-token")
                .or_else(|| headers.get("x-mediabrowser-token"))
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let session_id = Uuid::new_v4().simple().to_string();

    if let Some(raw_mid) = item_id.strip_prefix("media_") {
        let mid = raw_mid
            .parse::<i64>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的 media_id".to_string()))?;

        let row: Option<(String, Option<i32>, String, Option<i32>, Option<String>, f32, Option<String>, i16)> =
            sqlx::query_as(
                "SELECT media_type, tmdb_id, title, year, overview, rating, poster_path, scrape_status FROM media_items WHERE id = $1",
            )
            .bind(mid)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let Some((mtype, tmdb_id, title, year, overview, rating, poster, scrape_status)) = row else {
            return Err((StatusCode::NOT_FOUND, "条目不存在".to_string()));
        };

        // 若尚未刮削完成，或为电视剧（需检查是否存在未刮削新季/新集/待补齐连载集），发送 P0 实时插队信号
        if scrape_status != 1 || mtype == "tv" {
            let _ = state.scraper.priority_tx.try_send(mid);
        }

        // 零转存浪费：后台对小雅父目录执行只读 fs/list 寻址预热并回填文件大小
        state.spawn_prewarm_media_dirs(mid, None);

        let fav_set = load_user_favorite_keys(&state, &user_id).await;
        let is_fav = is_media_favorited(&fav_set, mid, tmdb_id, &mtype);
        let history_map = load_media_episode_histories(&state, &user_id, mid, tmdb_id, &mtype).await;
        let (pos_ticks, dur_ticks, is_played) = history_map.get(&(0, 1)).copied().unwrap_or((0, 0, false));

        let mut media_sources = vec![];
        if mtype == "movie" {
            media_sources = build_media_sources_for_movie(&state, &host, &token, &session_id, mid).await?;
        }

        let mut resp_obj = json!({
            "Id": item_id,
            "Name": title,
            "ServerId": cfg.server_id,
            "Type": if mtype == "movie" { "Movie" } else { "Series" },
            "ProductionYear": year,
            "Overview": overview,
            "CommunityRating": rating,
            "ImageTags": if poster.is_some() { json!({"Primary": "v1"}) } else { json!({}) },
            "MediaSources": media_sources,
            "UserData": {
                "PlaybackPositionTicks": pos_ticks,
                "PlayCount": if is_played { 1 } else { 0 },
                "IsFavorite": is_fav,
                "Played": is_played
            }
        });
        if dur_ticks > 0 {
            resp_obj["RunTimeTicks"] = json!(dur_ticks);
        }
        return Ok(Json(resp_obj));
    }

    if let Some(raw_eid) = item_id.strip_prefix("ep_") {
        let eid = raw_eid
            .parse::<i64>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的 episode_id".to_string()))?;

        let ep_meta: Option<(i64, i32, i32, Option<String>, Option<String>, Option<String>, Option<String>, i32)> = sqlx::query_as(
            "SELECT media_item_id, season_number, episode_number, title, overview, still_path, air_date, runtime FROM episodes WHERE id = $1",
        )
        .bind(eid)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let Some((mid, snum, epnum, title, overview, still_path, air_date, runtime)) = ep_meta else {
            return Err((StatusCode::NOT_FOUND, "剧集不存在".to_string()));
        };

        let media_meta: Option<(Option<i32>, String, String)> =
            sqlx::query_as("SELECT tmdb_id, media_type, title FROM media_items WHERE id = $1")
                .bind(mid)
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None);
        let (tmdb_id, mtype, series_title) =
            media_meta.unwrap_or((None, "tv".to_string(), String::new()));
        let history_map = load_media_episode_histories(&state, &user_id, mid, tmdb_id, &mtype).await;
        let (pos_ticks, dur_ticks, is_played) = history_map
            .get(&(snum, epnum))
            .copied()
            .unwrap_or((0, 0, false));
        let effective_dur = if dur_ticks > 0 {
            dur_ticks
        } else if runtime > 0 {
            (runtime as i64) * 600_000_000
        } else {
            0
        };

        let sources = build_media_sources_for_episode(&state, &host, &token, &session_id, mid, snum, epnum).await?;
        let mut resp_obj = json!({
            "Id": item_id,
            "Name": title.unwrap_or_else(|| format!("第 {} 集", epnum)),
            "ServerId": cfg.server_id,
            "SeriesId": format!("media_{}", mid),
            "SeriesName": series_title,
            "SeasonId": format!("season_{}_{}", mid, snum),
            "Type": "Episode",
            "IndexNumber": epnum,
            "ParentIndexNumber": snum,
            "Overview": overview,
            "PremiereDate": air_date,
            "ImageTags": if still_path.is_some() { json!({"Primary": "v1"}) } else { json!({}) },
            "MediaSources": sources,
            "UserData": {
                "PlaybackPositionTicks": pos_ticks,
                "PlayCount": if is_played { 1 } else { 0 },
                "IsFavorite": false,
                "Played": is_played
            }
        });
        if effective_dur > 0 {
            resp_obj["RunTimeTicks"] = json!(effective_dur);
        }
        return Ok(Json(resp_obj));
    }

    Err((StatusCode::NOT_FOUND, "未知资源标识".to_string()))
}


#[derive(Deserialize, Default)]
pub(super) struct ShowEpisodesQuery {
    #[serde(rename = "SeasonId", alias = "seasonId")]
    season_id: Option<String>,
    #[serde(rename = "Season", alias = "season")]
    season: Option<i32>,
    #[serde(rename = "UserId", alias = "userId")]
    user_id: Option<String>,
}

#[derive(sqlx::FromRow)]
struct SeasonListRow {
    season_number: i32,
    name: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    air_date: Option<String>,
}

pub(super) async fn get_show_seasons(
    State(state): State<Arc<AppState>>,
    Path(series_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let cfg = state.config.load();
    let mid = series_id
        .strip_prefix("media_")
        .unwrap_or(&series_id)
        .parse::<i64>()
        .map_err(|_| (StatusCode::BAD_REQUEST, "无效的 series_id".to_string()))?;

    // 触发 P0 实时检查（若该剧分集尚未刮削或有连载新集待补齐，立即插队执行）
    let _ = state.scraper.priority_tx.try_send(mid);

    // 零转存浪费：进入剧集季列表时，后台执行只读 fs/list 预热小雅目录树缓存并回填体积
    state.spawn_prewarm_media_dirs(mid, None);
    let sibling_ids = state.resolve_sibling_media_ids(mid).await;

    let series_poster: Option<String> =
        sqlx::query_scalar("SELECT poster_path FROM media_items WHERE id = $1")
            .bind(mid)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None)
            .flatten();

    let season_rows: Vec<SeasonListRow> = sqlx::query_as(
        r#"
        SELECT
            ep_s.season_number,
            s_meta.name,
            s_meta.overview,
            s_meta.poster_path,
            s_meta.air_date
        FROM (
            SELECT DISTINCT season_number
            FROM episodes
            WHERE media_item_id = ANY($1)
        ) ep_s
        LEFT JOIN LATERAL (
            SELECT name, overview, poster_path, air_date
            FROM seasons s
            WHERE s.media_item_id = ANY($1) AND s.season_number = ep_s.season_number
            ORDER BY (s.poster_path IS NOT NULL) DESC, (s.overview IS NOT NULL) DESC, s.episode_count DESC, s.id ASC
            LIMIT 1
        ) s_meta ON TRUE
        ORDER BY ep_s.season_number ASC
        "#,
    )
    .bind(&sibling_ids)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let items: Vec<Value> = season_rows
        .into_iter()
        .map(|row| {
            let snum = row.season_number;
            let name = row.name.unwrap_or_else(|| {
                if snum == 0 {
                    "特别篇".to_string()
                } else {
                    format!("第 {} 季", snum)
                }
            });
            let has_poster = row.poster_path.is_some() || series_poster.is_some();
            json!({
                "Id": format!("season_{}_{}", mid, snum),
                "Name": name,
                "ServerId": cfg.server_id,
                "SeriesId": format!("media_{}", mid),
                "Type": "Season",
                "IsFolder": true,
                "IndexNumber": snum,
                "Overview": row.overview,
                "PremiereDate": row.air_date,
                "ImageTags": if has_poster { json!({"Primary": "v1"}) } else { json!({}) }
            })
        })
        .collect();

    Ok(Json(json!({
        "Items": items,
        "TotalRecordCount": items.len()
    })))
}

pub(super) async fn get_show_episodes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(series_id): Path<String>,
    Query(q): Query<ShowEpisodesQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id(&state, q.user_id.as_deref(), &headers).await?;
    let cfg = state.config.load();
    let mid = series_id
        .strip_prefix("media_")
        .unwrap_or(&series_id)
        .parse::<i64>()
        .map_err(|_| (StatusCode::BAD_REQUEST, "无效的 series_id".to_string()))?;

    // 触发 P0 实时检查（若该季有未刮削分集或连载残缺分集已过 12h 冷却，立即插队刷新）
    let _ = state.scraper.priority_tx.try_send(mid);

    // 兼容 VidHub 传入 SeasonId="season_123_1" 或 Season=1
    let target_season: Option<i32> = q.season.or_else(|| {
        q.season_id
            .as_deref()
            .and_then(|sid| sid.rsplit('_').next())
            .and_then(|n| n.parse::<i32>().ok())
    });

    // 零转存浪费：展开某季分集列表时，确保该季小雅父目录寻址与文件体积已预热
    state.spawn_prewarm_media_dirs(mid, target_season);

    let media_meta: Option<(Option<i32>, String, String)> =
        sqlx::query_as("SELECT tmdb_id, media_type, title FROM media_items WHERE id = $1")
            .bind(mid)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
    let (tmdb_id, mtype, series_title) =
        media_meta.unwrap_or((None, "tv".to_string(), String::new()));
    let history_map = load_media_episode_histories(&state, &user_id, mid, tmdb_id, &mtype).await;
    let sibling_ids = state.resolve_sibling_media_ids(mid).await;

    let eps: Vec<EpisodeGroupRow> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (season_number, episode_number)
            season_number, episode_number, id AS rep_id, title, overview, still_path, air_date, runtime
        FROM episodes
        WHERE media_item_id = ANY($1)
          AND ($2::int4 IS NULL OR season_number = $2)
        ORDER BY season_number ASC, episode_number ASC, (still_path IS NOT NULL) DESC, priority_score DESC, id ASC
        "#,
    )
    .bind(&sibling_ids)
    .bind(target_season)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let items: Vec<Value> = eps
        .into_iter()
        .map(|ep| {
            let (pos_ticks, dur_ticks, is_played) = history_map
                .get(&(ep.season_number, ep.episode_number))
                .copied()
                .unwrap_or((0, 0, false));
            let effective_dur = if dur_ticks > 0 {
                dur_ticks
            } else if ep.runtime > 0 {
                (ep.runtime as i64) * 600_000_000
            } else {
                0
            };

            let mut obj = json!({
                "Id": format!("ep_{}", ep.rep_id),
                "Name": ep.title.unwrap_or_else(|| format!("第 {} 集", ep.episode_number)),
                "ServerId": cfg.server_id,
                "SeriesId": format!("media_{}", mid),
                "SeriesName": series_title,
                "SeasonId": format!("season_{}_{}", mid, ep.season_number),
                "Type": "Episode",
                "IsFolder": false,
                "IndexNumber": ep.episode_number,
                "ParentIndexNumber": ep.season_number,
                "Overview": ep.overview,
                "PremiereDate": ep.air_date,
                "MediaType": "Video",
                "LocationType": "FileSystem",
                "ImageTags": if ep.still_path.is_some() { json!({"Primary": "v1"}) } else { json!({}) },
                "UserData": {
                    "PlaybackPositionTicks": pos_ticks,
                    "PlayCount": if is_played { 1 } else { 0 },
                    "IsFavorite": false,
                    "Played": is_played
                }
            });
            if effective_dur > 0 {
                obj["RunTimeTicks"] = json!(effective_dur);
            }
            obj
        })
        .collect();

    Ok(Json(json!({
        "Items": items,
        "TotalRecordCount": items.len()
    })))
}


pub(super) async fn get_latest_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let res = get_user_items(State(state), headers, Path(user_id), Query(q)).await?;
    Ok(Json(res.0["Items"].clone()))
}

/// 继续观看列表 (`GET /Users/:user_id/Items/Resume`)
pub(super) async fn get_resume_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path_user_id): Path<String>,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id(&state, Some(&path_user_id), &headers).await?;
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    get_resume_items_inner(&state, &user_id, limit).await
}

#[derive(sqlx::FromRow)]
struct HistoryRecordRow {
    media_item_id: Option<i64>,
    tmdb_id: Option<i32>,
    media_type: String,
    season_number: i32,
    episode_number: i32,
    position_ticks: i64,
    duration_ticks: i64,
    is_played: bool,
}

async fn get_resume_items_inner(
    state: &AppState,
    user_id: &str,
    limit: i64,
) -> Result<Json<Value>, (StatusCode, String)> {
    let cfg = state.config.load();
    let histories: Vec<HistoryRecordRow> = sqlx::query_as(
        r#"
        SELECT media_item_id, tmdb_id, media_type, season_number, episode_number, position_ticks, duration_ticks, is_played
        FROM play_histories
        WHERE user_id = $1 AND is_played = FALSE AND position_ticks > 0
        ORDER BY last_played_at DESC
        LIMIT $2
        "#,
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut items = Vec::with_capacity(histories.len());
    for h in histories {
        if let Some(item_json) = build_emby_item_from_history(state, &cfg.server_id, &h, false).await {
            items.push(item_json);
        }
    }

    let total = items.len();
    Ok(Json(json!({
        "Items": items,
        "TotalRecordCount": total
    })))
}

/// 接下来观看列表 (`GET /Shows/NextUp`)：
/// 若某部剧最后一集尚未看完，返回该集继续看；若已看完 (>=90%)，自动查找同季下一集返回
pub(super) async fn get_next_up_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id(&state, q.user_id.as_deref(), &headers).await?;
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let cfg = state.config.load();

    // 按每部剧聚合取最新播放的一集记录
    let latest_tv_histories: Vec<HistoryRecordRow> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (COALESCE(tmdb_id::text, media_item_id::text))
            media_item_id, tmdb_id, media_type, season_number, episode_number, position_ticks, duration_ticks, is_played
        FROM play_histories
        WHERE user_id = $1 AND media_type = 'tv'
        ORDER BY COALESCE(tmdb_id::text, media_item_id::text), last_played_at DESC
        LIMIT $2
        "#,
    )
    .bind(&user_id)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut items = Vec::new();
    for h in latest_tv_histories {
        let advance_to_next = h.is_played;
        if let Some(item_json) =
            build_emby_item_from_history(&state, &cfg.server_id, &h, advance_to_next).await
        {
            items.push(item_json);
        }
    }

    let total = items.len();
    Ok(Json(json!({
        "Items": items,
        "TotalRecordCount": total
    })))
}

async fn build_emby_item_from_history(
    state: &AppState,
    server_id: &str,
    h: &HistoryRecordRow,
    advance_to_next_ep: bool,
) -> Option<Value> {
    // 优先通过 tmdb_id 定位当前有效 media_items（防 /每日更新 目录迁移后失效），其次回退 media_item_id
    let media_row: Option<(i64, String, String, Option<i32>, Option<String>, Option<String>)> =
        if let Some(tid) = h.tmdb_id {
            sqlx::query_as(
                r#"
                SELECT id, media_type, title, year, overview, poster_path
                FROM media_items
                WHERE is_ignored = FALSE AND ((tmdb_id = $1 AND media_type = $2) OR id = $3)
                ORDER BY (id = $3) DESC, id DESC
                LIMIT 1
                "#,
            )
            .bind(tid)
            .bind(&h.media_type)
            .bind(h.media_item_id.unwrap_or(0))
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None)
        } else if let Some(mid) = h.media_item_id {
            sqlx::query_as(
                "SELECT id, media_type, title, year, overview, poster_path FROM media_items WHERE id = $1 AND is_ignored = FALSE",
            )
            .bind(mid)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None)
        } else {
            None
        };

    let (mid, mtype, media_title, year, media_overview, poster) = media_row?;

    if mtype == "movie" {
        if advance_to_next_ep {
            return None;
        }
        let mut obj = json!({
            "Id": format!("media_{}", mid),
            "Name": media_title,
            "ServerId": server_id,
            "Type": "Movie",
            "IsFolder": false,
            "ProductionYear": year,
            "Overview": media_overview,
            "MediaType": "Video",
            "ImageTags": if poster.is_some() { json!({"Primary": "v1"}) } else { json!({}) },
            "UserData": {
                "PlaybackPositionTicks": h.position_ticks,
                "PlayCount": if h.is_played { 1 } else { 0 },
                "IsFavorite": false,
                "Played": h.is_played
            }
        });
        if h.duration_ticks > 0 {
            obj["RunTimeTicks"] = json!(h.duration_ticks);
        }
        return Some(obj);
    }

    // 电视剧：跨所有同 tmdb_id 兄弟目录定位当前集或下一集
    let target_ep_num = if advance_to_next_ep {
        h.episode_number + 1
    } else {
        h.episode_number
    };
    let sibling_ids = state.resolve_sibling_media_ids(mid).await;

    let ep_row: Option<(i64, i32, i32, Option<String>, Option<String>, Option<String>, Option<String>, i32)> = sqlx::query_as(
        r#"
        SELECT id, season_number, episode_number, title, overview, still_path, air_date, runtime
        FROM episodes
        WHERE media_item_id = ANY($1) AND season_number = $2 AND episode_number = $3
        ORDER BY (still_path IS NOT NULL) DESC, priority_score DESC, id ASC
        LIMIT 1
        "#,
    )
    .bind(&sibling_ids)
    .bind(h.season_number)
    .bind(target_ep_num)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    let (ep_id, snum, epnum, ep_title, ep_overview, still_path, air_date, runtime) = ep_row?;
    let pos_ticks = if advance_to_next_ep { 0 } else { h.position_ticks };
    let played = if advance_to_next_ep { false } else { h.is_played };
    let effective_dur = if !advance_to_next_ep && h.duration_ticks > 0 {
        h.duration_ticks
    } else if runtime > 0 {
        (runtime as i64) * 600_000_000
    } else {
        0
    };

    let mut obj = json!({
        "Id": format!("ep_{}", ep_id),
        "Name": ep_title.unwrap_or_else(|| format!("第 {} 集", epnum)),
        "ServerId": server_id,
        "SeriesId": format!("media_{}", mid),
        "SeriesName": media_title,
        "SeasonId": format!("season_{}_{}", mid, snum),
        "Type": "Episode",
        "IsFolder": false,
        "IndexNumber": epnum,
        "ParentIndexNumber": snum,
        "Overview": ep_overview.or(media_overview),
        "PremiereDate": air_date,
        "MediaType": "Video",
        "ImageTags": if still_path.is_some() || poster.is_some() { json!({"Primary": "v1"}) } else { json!({}) },
        "SeriesPrimaryImageTag": if poster.is_some() { "v1" } else { "" },
        "UserData": {
            "PlaybackPositionTicks": pos_ticks,
            "PlayCount": if played { 1 } else { 0 },
            "IsFavorite": false,
            "Played": played
        }
    });
    if effective_dur > 0 {
        obj["RunTimeTicks"] = json!(effective_dur);
    }
    Some(obj)
}

/// 解析 Emby 客户端逗号分隔的 `SortBy` 与 `SortOrder` 参数，生成稳定的 PostgreSQL ORDER BY 子句
fn build_emby_order_clause(sort_by: &str, sort_order: Option<&str>) -> String {
    let orders: Vec<bool> = sort_order
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.eq_ignore_ascii_case("Ascending") || s.eq_ignore_ascii_case("asc"))
        .collect();

    let mut clauses: Vec<String> = Vec::new();
    let mut first_is_asc: Option<bool> = None;

    for (idx, raw_field) in sort_by
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("IsFolder"))
        .enumerate()
    {
        if clauses.len() >= 3 {
            break;
        }
        let default_asc = matches!(raw_field, "SortName" | "Name" | "OriginalTitle");
        let is_asc = orders
            .get(idx)
            .copied()
            .or_else(|| orders.last().copied())
            .unwrap_or(default_asc);
        if first_is_asc.is_none() {
            first_is_asc = Some(is_asc);
        }
        let dir = if is_asc { "ASC" } else { "DESC" };

        let expr = match raw_field {
            "DateCreated" => format!("created_at {}", dir),
            "DateLastContentAdded" => format!("updated_at {}", dir),
            "PremiereDate" => format!(
                "COALESCE(NULLIF(premiered_at, ''), CASE WHEN year IS NOT NULL THEN year::text || '-01-01' ELSE NULL END) {} NULLS LAST",
                dir
            ),
            "ProductionYear" => format!("year {} NULLS LAST", dir),
            "Popularity" => format!("popularity {} NULLS LAST", dir),
            "CommunityRating" | "CriticRating" => format!("rating {}", dir),
            "SortName" | "Name" | "OriginalTitle" => format!("title {}", dir),
            "DatePlayed" => format!(
                "(SELECT MAX(ph.last_played_at) FROM play_histories ph WHERE ph.user_id = $6 AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = deduped.tmdb_id AND ph.media_type = deduped.media_type) OR ph.media_item_id = deduped.id)) {} NULLS LAST",
                dir
            ),
            "PlayCount" => format!(
                "(SELECT COUNT(*) FROM play_histories ph WHERE ph.user_id = $6 AND ph.is_played = TRUE AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = deduped.tmdb_id AND ph.media_type = deduped.media_type) OR ph.media_item_id = deduped.id)) {}",
                dir
            ),
            "Random" => "RANDOM()".to_string(),
            _ => continue,
        };
        clauses.push(expr);
    }

    if clauses.is_empty() {
        let fallback_asc = orders.first().copied().unwrap_or(false);
        return if fallback_asc {
            "year ASC NULLS LAST, id ASC".to_string()
        } else {
            "year DESC NULLS LAST, id DESC".to_string()
        };
    }

    let tie_dir = if first_is_asc.unwrap_or(false) { "ASC" } else { "DESC" };
    clauses.push(format!("id {}", tie_dir));
    clauses.join(", ")
}

#[cfg(test)]
mod tests {
    use super::build_emby_order_clause;

    #[test]
    fn test_emby_sort_priority_with_multi_fields() {
        assert_eq!(
            build_emby_order_clause("PremiereDate,SortName", Some("Descending")),
            "COALESCE(NULLIF(premiered_at, ''), CASE WHEN year IS NOT NULL THEN year::text || '-01-01' ELSE NULL END) DESC NULLS LAST, title DESC, id DESC"
        );
        assert_eq!(
            build_emby_order_clause("Popularity,SortName", Some("Descending")),
            "popularity DESC NULLS LAST, title DESC, id DESC"
        );
        assert_eq!(
            build_emby_order_clause("CommunityRating,DateCreated", Some("Descending,Ascending")),
            "rating DESC, created_at ASC, id DESC"
        );
        assert_eq!(
            build_emby_order_clause("IsFolder,SortName", None),
            "title ASC, id ASC"
        );
        assert_eq!(
            build_emby_order_clause("DateLastContentAdded", Some("Descending")),
            "updated_at DESC, id DESC"
        );
        assert_eq!(
            build_emby_order_clause("Random", None),
            "RANDOM(), id DESC"
        );
    }
}

