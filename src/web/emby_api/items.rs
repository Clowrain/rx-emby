use super::auth::resolve_valid_user_id;
use super::playback::{
    build_media_sources_for_episode, build_media_sources_for_movie, PlaybackQuery,
};
use super::sessions::{is_media_favorited, load_media_episode_histories, load_user_favorite_keys};
use super::views::get_user_allowed_categories;
use crate::smart_views::{can_access_view, VirtualLibraryRule};
use crate::state::{AppState, ScrapeTask};
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

    // 3. 如果是展开某部电视剧的季或单集列表（ParentId 为 "work_123" 或兼容旧 "media_123"）
    if let Some(ref pid) = q.parent_id {
        if let Some(raw_id) = pid.strip_prefix("work_").or_else(|| pid.strip_prefix("media_")) {
            if let Ok(mid) = raw_id.parse::<i64>() {
                ensure_media_accessible(&state, &user_id, mid).await?;
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

    // 解析指定 Ids 批量查询（如 Ids=work_10,work_20）
    let explicit_ids: Option<Vec<i64>> = q.ids.as_deref().map(|raw| {
        raw.split(',')
            .filter_map(|s| {
                let t = s.trim();
                t.strip_prefix("work_")
                    .or_else(|| t.strip_prefix("media_"))
                    .or(Some(t))
                    .and_then(|v| v.parse::<i64>().ok())
            })
            .collect()
    });

    // 4. 组装排序规则（完整支持逗号分隔多字段 SortBy 与 SortOrder）
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
            SELECT m.id, m.media_type, m.tmdb_id, m.title, m.original_title, m.year, m.premiered_at, m.overview, m.rating, m.poster_path, m.backdrop_path, m.created_at, m.updated_at, (m.tmdb_metadata->>'popularity')::float8 AS popularity
            FROM media_works m
            WHERE EXISTS (
                SELECT 1 FROM sync_folders f
                WHERE f.work_id = m.id AND f.is_ignored = FALSE
            )
              AND ($1::text IS NULL OR m.category = $1)
              AND ($2::text IS NULL OR m.title ILIKE $2 OR m.original_title ILIKE $2)
              AND ($3::text IS NULL OR m.media_type = $3)
              AND ($4::int8[] IS NULL OR m.id = ANY($4))
              AND (NOT $5 OR EXISTS (
                  SELECT 1 FROM user_favorites uf
                  WHERE uf.user_id = $6
                    AND ((uf.tmdb_id IS NOT NULL AND uf.tmdb_id = m.tmdb_id AND uf.media_type = m.media_type) OR uf.work_id = m.id)
              ))
              AND (NOT $7 OR EXISTS (
                  SELECT 1 FROM play_histories ph
                  WHERE ph.user_id = $6 AND ph.is_played = TRUE
                    AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = m.tmdb_id AND ph.media_type = m.media_type) OR ph.work_id = m.id)
              ))
              AND (NOT $8 OR NOT EXISTS (
                  SELECT 1 FROM play_histories ph
                  WHERE ph.user_id = $6 AND ph.is_played = TRUE
                    AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = m.tmdb_id AND ph.media_type = m.media_type) OR ph.work_id = m.id)
              ))
              AND ($11::text[] IS NULL OR m.category = ANY($11))
              AND ($12::jsonb IS NULL OR {view_predicate})
        ) AS m
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
        SELECT COUNT(*)
        FROM media_works m
        WHERE EXISTS (
            SELECT 1 FROM sync_folders f
            WHERE f.work_id = m.id AND f.is_ignored = FALSE
        )
          AND ($1::text IS NULL OR m.category = $1)
          AND ($2::text IS NULL OR m.title ILIKE $2 OR m.original_title ILIKE $2)
          AND ($3::text IS NULL OR m.media_type = $3)
          AND ($4::int8[] IS NULL OR m.id = ANY($4))
          AND (NOT $5 OR EXISTS (
              SELECT 1 FROM user_favorites uf
              WHERE uf.user_id = $6
                AND ((uf.tmdb_id IS NOT NULL AND uf.tmdb_id = m.tmdb_id AND uf.media_type = m.media_type) OR uf.work_id = m.id)
          ))
          AND (NOT $7 OR EXISTS (
              SELECT 1 FROM play_histories ph
              WHERE ph.user_id = $6 AND ph.is_played = TRUE
                AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = m.tmdb_id AND ph.media_type = m.media_type) OR ph.work_id = m.id)
          ))
          AND (NOT $8 OR NOT EXISTS (
              SELECT 1 FROM play_histories ph
              WHERE ph.user_id = $6 AND ph.is_played = TRUE
                AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = m.tmdb_id AND ph.media_type = m.media_type) OR ph.work_id = m.id)
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

    // 批量加载当前用户的收藏集合与播放历史，填充 UserData
    let fav_set = load_user_favorite_keys(&state, &user_id).await;
    let page_mids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let page_tids: Vec<i32> = rows.iter().filter_map(|r| r.tmdb_id).collect();
    let hist_rows: Vec<(Option<i64>, Option<i32>, String, i32, i32, i64, bool)> = if page_mids.is_empty() {
        vec![]
    } else {
        sqlx::query_as(
            r#"
            SELECT work_id, tmdb_id, media_type, season_number, episode_number, position_ticks, is_played
            FROM play_histories
            WHERE user_id = $1
              AND (work_id = ANY($2) OR (tmdb_id IS NOT NULL AND tmdb_id = ANY($3)))
            "#,
        )
        .bind(&user_id)
        .bind(&page_mids)
        .bind(&page_tids)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
    };

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
            let (pos_ticks, is_played) = if r.media_type == "movie" {
                hist_rows
                    .iter()
                    .find(|(mid, tid, mtype, s, e, _, _)| {
                        *s == 0 && *e == 1 && (
                            (r.tmdb_id.is_some() && *tid == r.tmdb_id && mtype == "movie")
                            || *mid == Some(r.id)
                        )
                    })
                    .map(|(_, _, _, _, _, pos, played)| (*pos, *played))
                    .unwrap_or((0, false))
            } else {
                let any_played = hist_rows.iter().any(|(mid, tid, mtype, _, _, _, played)| {
                    *played && (
                        (r.tmdb_id.is_some() && *tid == r.tmdb_id && mtype == "tv")
                        || *mid == Some(r.id)
                    )
                });
                (0, any_played)
            };

            let mut obj = json!({
                "Id": format!("work_{}", r.id),
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
                    "PlaybackPositionTicks": pos_ticks,
                    "PlayCount": if is_played { 1 } else { 0 },
                    "IsFavorite": is_fav,
                    "Played": is_played
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
    title: Option<String>,
    overview: Option<String>,
    still_path: Option<String>,
    air_date: Option<String>,
    runtime: i32,
}

async fn get_episodes_for_media(
    state: &AppState,
    user_id: &str,
    work_id: i64,
) -> Result<Json<Value>, (StatusCode, String)> {
    let cfg = state.config.load();
    let _ = state.scraper.priority_tx.try_send(ScrapeTask::WorkSeasons(work_id));

    let media_meta: Option<(Option<i32>, String)> =
        sqlx::query_as("SELECT tmdb_id, media_type FROM media_works WHERE id = $1")
            .bind(work_id)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
    let (tmdb_id, mtype) = media_meta.unwrap_or((None, "tv".to_string()));
    let history_map = load_media_episode_histories(state, user_id, work_id, tmdb_id, &mtype).await;

    let eps: Vec<EpisodeGroupRow> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (sf.season_number, sf.episode_number)
            sf.season_number,
            sf.episode_number,
            we.title,
            we.overview,
            we.still_path,
            we.air_date,
            COALESCE(we.runtime, 0) AS runtime
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        LEFT JOIN work_episodes we
          ON we.work_id = $1
         AND we.season_number = sf.season_number
         AND we.episode_number = sf.episode_number
        WHERE f.work_id = $1
          AND f.is_ignored = FALSE
        ORDER BY sf.season_number ASC, sf.episode_number ASC, COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) DESC, sf.id ASC
        "#,
    )
    .bind(work_id)
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
                "Id": format!("ep_{}_{}_{}", work_id, ep.season_number, ep.episode_number),
                "Name": ep.title.unwrap_or_else(|| format!("第 {} 集", ep.episode_number)),
                "ServerId": cfg.server_id,
                "SeriesId": format!("work_{}", work_id),
                "SeasonId": format!("season_{}_{}", work_id, ep.season_number),
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

    if let Some(raw_wid) = item_id.strip_prefix("work_").or_else(|| item_id.strip_prefix("media_")) {
        let wid = raw_wid
            .parse::<i64>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的 work_id".to_string()))?;

        ensure_media_accessible(&state, &user_id, wid).await?;

        let row: Option<(String, Option<i32>, String, Option<i32>, Option<String>, f32, Option<String>)> =
            sqlx::query_as(
                "SELECT media_type, tmdb_id, title, year, overview, rating, poster_path FROM media_works WHERE id = $1",
            )
            .bind(wid)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let Some((mtype, tmdb_id, title, year, overview, rating, poster)) = row else {
            return Err((StatusCode::NOT_FOUND, "条目不存在".to_string()));
        };

        if mtype == "tv" {
            let _ = state.scraper.priority_tx.try_send(ScrapeTask::WorkSeasons(wid));
        }

        // 零转存浪费：后台对小雅父目录执行只读 fs/list 寻址预热并回填文件大小
        state.spawn_prewarm_media_dirs(wid, None);

        let fav_set = load_user_favorite_keys(&state, &user_id).await;
        let is_fav = is_media_favorited(&fav_set, wid, tmdb_id, &mtype);
        let history_map = load_media_episode_histories(&state, &user_id, wid, tmdb_id, &mtype).await;
        let (pos_ticks, dur_ticks, is_played) = history_map.get(&(0, 1)).copied().unwrap_or((0, 0, false));

        let mut media_sources = vec![];
        if mtype == "movie" {
            media_sources = build_media_sources_for_movie(&state, &host, &token, &session_id, wid).await?;
        }

        let mut resp_obj = json!({
            "Id": format!("work_{}", wid),
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

    if let Some(raw_ep) = item_id.strip_prefix("ep_") {
        let parts: Vec<&str> = raw_ep.rsplitn(3, '_').collect();
        let (wid, snum, epnum) = if parts.len() == 3 {
            let e = parts[0].parse::<i32>().unwrap_or(0);
            let s = parts[1].parse::<i32>().unwrap_or(0);
            let w = parts[2].parse::<i64>().unwrap_or(0);
            if w <= 0 || e <= 0 {
                return Err((StatusCode::BAD_REQUEST, "无效的 episode_id".to_string()));
            }
            (w, s, e)
        } else {
            return Err((StatusCode::BAD_REQUEST, "无效的 episode_id".to_string()));
        };

        ensure_media_accessible(&state, &user_id, wid).await?;

        let has_file: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM sync_files sf
                JOIN sync_folders f ON f.id = sf.folder_id
                WHERE f.work_id = $1 AND f.is_ignored = FALSE
                  AND sf.season_number = $2 AND sf.episode_number = $3
            )
            "#,
        )
        .bind(wid)
        .bind(snum)
        .bind(epnum)
        .fetch_one(&state.db)
        .await
        .unwrap_or(false);

        if !has_file {
            return Err((StatusCode::NOT_FOUND, "剧集不存在或已被屏蔽".to_string()));
        }

        let ep_meta: Option<(Option<String>, Option<String>, Option<String>, Option<String>, i32)> = sqlx::query_as(
            "SELECT title, overview, still_path, air_date, runtime FROM work_episodes WHERE work_id = $1 AND season_number = $2 AND episode_number = $3",
        )
        .bind(wid)
        .bind(snum)
        .bind(epnum)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

        let (title, overview, still_path, air_date, runtime) =
            ep_meta.unwrap_or((None, None, None, None, 0));

        let media_meta: Option<(Option<i32>, String, String)> =
            sqlx::query_as("SELECT tmdb_id, media_type, title FROM media_works WHERE id = $1")
                .bind(wid)
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None);
        let (tmdb_id, mtype, series_title) =
            media_meta.unwrap_or((None, "tv".to_string(), String::new()));
        let history_map = load_media_episode_histories(&state, &user_id, wid, tmdb_id, &mtype).await;
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

        let sources = build_media_sources_for_episode(&state, &host, &token, &session_id, wid, snum, epnum).await?;
        let mut resp_obj = json!({
            "Id": item_id,
            "Name": title.unwrap_or_else(|| format!("第 {} 集", epnum)),
            "ServerId": cfg.server_id,
            "SeriesId": format!("work_{}", wid),
            "SeriesName": series_title,
            "SeasonId": format!("season_{}_{}", wid, snum),
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

pub(super) async fn ensure_media_accessible(
    state: &AppState,
    user_id: &str,
    work_id: i64,
) -> Result<(String, Option<i32>, String), (StatusCode, String)> {
    let row: Option<(String, Option<i32>, String)> = sqlx::query_as(
        r#"
        SELECT w.media_type, w.tmdb_id, w.category
        FROM media_works w
        WHERE w.id = $1
          AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = w.id AND f.is_ignored = FALSE)
        "#,
    )
    .bind(work_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (media_type, tmdb_id, category) =
        row.ok_or((StatusCode::NOT_FOUND, "媒体不存在或已忽略".to_string()))?;

    if let Some(cats) = get_user_allowed_categories(state, user_id).await {
        let allowed = cats
            .iter()
            .any(|c| c == &category || c.starts_with("lib_vlib_"));
        if !allowed {
            return Err((StatusCode::FORBIDDEN, "无权访问该分类下的媒体".to_string()));
        }
    }

    Ok((media_type, tmdb_id, category))
}

pub(super) async fn get_show_seasons(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(series_id): Path<String>,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id(&state, q.user_id.as_deref(), &headers).await?;
    let cfg = state.config.load();
    let wid = series_id
        .strip_prefix("work_")
        .or_else(|| series_id.strip_prefix("media_"))
        .unwrap_or(&series_id)
        .parse::<i64>()
        .map_err(|_| (StatusCode::BAD_REQUEST, "无效的 series_id".to_string()))?;

    ensure_media_accessible(&state, &user_id, wid).await?;

    // 触发 P0 实时检查（若该剧分集尚未刮削或有连载新集待补齐，立即插队执行）
    let _ = state.scraper.priority_tx.try_send(ScrapeTask::WorkSeasons(wid));

    // 零转存浪费：进入剧集季列表时，后台执行只读 fs/list 预热小雅目录树缓存并回填体积
    state.spawn_prewarm_media_dirs(wid, None);

    let series_poster: Option<String> =
        sqlx::query_scalar("SELECT poster_path FROM media_works WHERE id = $1")
            .bind(wid)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None)
            .flatten();

    let season_rows: Vec<SeasonListRow> = sqlx::query_as(
        r#"
        SELECT
            ep_s.season_number,
            ws.name,
            ws.overview,
            ws.poster_path,
            ws.air_date
        FROM (
            SELECT DISTINCT sf.season_number
            FROM sync_files sf
            JOIN sync_folders f ON f.id = sf.folder_id
            WHERE f.work_id = $1 AND f.is_ignored = FALSE
        ) ep_s
        LEFT JOIN work_seasons ws
          ON ws.work_id = $1 AND ws.season_number = ep_s.season_number
        ORDER BY ep_s.season_number ASC
        "#,
    )
    .bind(wid)
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
                "Id": format!("season_{}_{}", wid, snum),
                "Name": name,
                "ServerId": cfg.server_id,
                "SeriesId": format!("work_{}", wid),
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
    let wid = series_id
        .strip_prefix("work_")
        .or_else(|| series_id.strip_prefix("media_"))
        .unwrap_or(&series_id)
        .parse::<i64>()
        .map_err(|_| (StatusCode::BAD_REQUEST, "无效的 series_id".to_string()))?;

    ensure_media_accessible(&state, &user_id, wid).await?;

    // 触发 P0 实时检查（若该季有未刮削分集或连载残缺分集已过 12h 冷却，立即插队刷新）
    let _ = state.scraper.priority_tx.try_send(ScrapeTask::WorkSeasons(wid));

    // 兼容 VidHub 传入 SeasonId="season_123_1" 或 Season=1
    let target_season: Option<i32> = q.season.or_else(|| {
        q.season_id
            .as_deref()
            .and_then(|sid| sid.rsplit('_').next())
            .and_then(|n| n.parse::<i32>().ok())
    });

    // 零转存浪费：展开某季分集列表时，确保该季小雅父目录寻址与文件体积已预热
    state.spawn_prewarm_media_dirs(wid, target_season);

    let media_meta: Option<(Option<i32>, String, String)> =
        sqlx::query_as("SELECT tmdb_id, media_type, title FROM media_works WHERE id = $1")
            .bind(wid)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
    let (tmdb_id, mtype, series_title) =
        media_meta.unwrap_or((None, "tv".to_string(), String::new()));
    let history_map = load_media_episode_histories(&state, &user_id, wid, tmdb_id, &mtype).await;

    let eps: Vec<EpisodeGroupRow> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (sf.season_number, sf.episode_number)
            sf.season_number,
            sf.episode_number,
            we.title,
            we.overview,
            we.still_path,
            we.air_date,
            COALESCE(we.runtime, 0) AS runtime
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        LEFT JOIN work_episodes we
          ON we.work_id = $1
         AND we.season_number = sf.season_number
         AND we.episode_number = sf.episode_number
        WHERE f.work_id = $1
          AND f.is_ignored = FALSE
          AND ($2::int4 IS NULL OR sf.season_number = $2)
        ORDER BY sf.season_number ASC, sf.episode_number ASC, COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) DESC, sf.id ASC
        "#,
    )
    .bind(wid)
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
                "Id": format!("ep_{}_{}_{}", wid, ep.season_number, ep.episode_number),
                "Name": ep.title.unwrap_or_else(|| format!("第 {} 集", ep.episode_number)),
                "ServerId": cfg.server_id,
                "SeriesId": format!("work_{}", wid),
                "SeriesName": series_title,
                "SeasonId": format!("season_{}_{}", wid, ep.season_number),
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
    work_id: Option<i64>,
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
        SELECT work_id, tmdb_id, media_type, season_number, episode_number, position_ticks, duration_ticks, is_played
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

    let allowed_cats = get_user_allowed_categories(state, user_id).await;
    let mut items = Vec::with_capacity(histories.len());
    for h in histories {
        if let Some(item_json) = build_emby_item_from_history(
            state,
            &cfg.server_id,
            &h,
            false,
            allowed_cats.as_deref(),
        )
        .await
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

/// 接下来观看列表 (`GET /Shows/NextUp`)：
/// 若某部剧最后一集尚未看完，返回该集继续看；若已看完 (>=90%)，自动查找同季下一集或下一季首集返回
pub(super) async fn get_next_up_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id(&state, q.user_id.as_deref(), &headers).await?;
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let cfg = state.config.load();
    let allowed_cats = get_user_allowed_categories(&state, &user_id).await;

    // 按每部剧聚合取最新播放的一集记录，再按 last_played_at 倒序返回
    let latest_tv_histories: Vec<HistoryRecordRow> = sqlx::query_as(
        r#"
        SELECT work_id, tmdb_id, media_type, season_number, episode_number, position_ticks, duration_ticks, is_played
        FROM (
            SELECT DISTINCT ON (COALESCE(tmdb_id::text, work_id::text))
                work_id, tmdb_id, media_type, season_number, episode_number, position_ticks, duration_ticks, is_played, last_played_at
            FROM play_histories
            WHERE user_id = $1 AND media_type = 'tv'
            ORDER BY COALESCE(tmdb_id::text, work_id::text), last_played_at DESC
        ) t
        ORDER BY last_played_at DESC
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
        if let Some(item_json) = build_emby_item_from_history(
            &state,
            &cfg.server_id,
            &h,
            advance_to_next,
            allowed_cats.as_deref(),
        )
        .await
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
    allowed_cats: Option<&[String]>,
) -> Option<Value> {
    let media_row: Option<(i64, String, String, Option<i32>, Option<String>, Option<String>, String)> =
        if let Some(tid) = h.tmdb_id {
            sqlx::query_as(
                r#"
                SELECT w.id, w.media_type, w.title, w.year, w.overview, w.poster_path, w.category
                FROM media_works w
                WHERE EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = w.id AND f.is_ignored = FALSE)
                  AND ((w.tmdb_id = $1 AND w.media_type = $2) OR w.id = $3)
                ORDER BY (w.id = $3) DESC, w.id DESC
                LIMIT 1
                "#,
            )
            .bind(tid)
            .bind(&h.media_type)
            .bind(h.work_id.unwrap_or(0))
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None)
        } else if let Some(wid) = h.work_id {
            sqlx::query_as(
                r#"
                SELECT w.id, w.media_type, w.title, w.year, w.overview, w.poster_path, w.category
                FROM media_works w
                WHERE w.id = $1
                  AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = w.id AND f.is_ignored = FALSE)
                "#,
            )
            .bind(wid)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None)
        } else {
            None
        };

    let (wid, mtype, media_title, year, media_overview, poster, category) = media_row?;
    if let Some(cats) = allowed_cats {
        if !cats.iter().any(|c| c == &category || c.starts_with("lib_vlib_")) {
            return None;
        }
    }

    if mtype == "movie" {
        if advance_to_next_ep {
            return None;
        }
        let mut obj = json!({
            "Id": format!("work_{}", wid),
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

    let target_ep_num = if advance_to_next_ep {
        h.episode_number + 1
    } else {
        h.episode_number
    };

    let mut ep_row: Option<(i32, i32, Option<String>, Option<String>, Option<String>, Option<String>, i32)> = sqlx::query_as(
        r#"
        SELECT
            sf.season_number,
            sf.episode_number,
            we.title,
            we.overview,
            we.still_path,
            we.air_date,
            COALESCE(we.runtime, 0) AS runtime
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        LEFT JOIN work_episodes we
          ON we.work_id = $1
         AND we.season_number = sf.season_number
         AND we.episode_number = sf.episode_number
        WHERE f.work_id = $1 AND f.is_ignored = FALSE
          AND sf.season_number = $2 AND sf.episode_number = $3
        ORDER BY COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) DESC, sf.id ASC
        LIMIT 1
        "#,
    )
    .bind(wid)
    .bind(h.season_number)
    .bind(target_ep_num)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    if ep_row.is_none() && advance_to_next_ep {
        ep_row = sqlx::query_as(
            r#"
            SELECT
                sf.season_number,
                sf.episode_number,
                we.title,
                we.overview,
                we.still_path,
                we.air_date,
                COALESCE(we.runtime, 0) AS runtime
            FROM sync_files sf
            JOIN sync_folders f ON f.id = sf.folder_id
            LEFT JOIN work_episodes we
              ON we.work_id = $1
             AND we.season_number = sf.season_number
             AND we.episode_number = sf.episode_number
            WHERE f.work_id = $1 AND f.is_ignored = FALSE
              AND (sf.season_number > $2 OR (sf.season_number = $2 AND sf.episode_number > $3))
            ORDER BY sf.season_number ASC, sf.episode_number ASC, COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) DESC, sf.id ASC
            LIMIT 1
            "#,
        )
        .bind(wid)
        .bind(h.season_number)
        .bind(h.episode_number)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
    }

    let (snum, epnum, ep_title, ep_overview, still_path, air_date, runtime) = ep_row?;
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
        "Id": format!("ep_{}_{}_{}", wid, snum, epnum),
        "Name": ep_title.unwrap_or_else(|| format!("第 {} 集", epnum)),
        "ServerId": server_id,
        "SeriesId": format!("work_{}", wid),
        "SeriesName": media_title,
        "SeasonId": format!("season_{}_{}", wid, snum),
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
                "(SELECT MAX(ph.last_played_at) FROM play_histories ph WHERE ph.user_id = $6 AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = m.tmdb_id AND ph.media_type = m.media_type) OR ph.work_id = m.id)) {} NULLS LAST",
                dir
            ),
            "PlayCount" => format!(
                "(SELECT COUNT(*) FROM play_histories ph WHERE ph.user_id = $6 AND ph.is_played = TRUE AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = m.tmdb_id AND ph.media_type = m.media_type) OR ph.work_id = m.id)) {}",
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

