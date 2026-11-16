use super::sql::*;
use super::*;
use serde_json::json;
pub async fn get_latest_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id =
        resolve_valid_user_id_with_query(&state, Some(&user_id), &headers, q.api_key.as_deref())
            .await?;
    let mut res = get_user_items_inner(&state, &user_id, &q).await?;
    // 直接取出 Items 数组，避免整份响应 JSON 深拷贝
    let items = res
        .get_mut("Items")
        .map(Value::take)
        .unwrap_or_else(|| json!([]));
    Ok(Json(items))
}

/// 继续观看列表 (`GET /Users/:user_id/Items/Resume`)
pub async fn get_resume_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path_user_id): Path<String>,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id_with_query(
        &state,
        Some(&path_user_id),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    Ok(Json(get_resume_items_inner(&state, &user_id, limit).await?))
}

#[derive(Debug, Clone, sqlx::FromRow)]
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

pub(in crate::web::emby_api) async fn get_resume_items_inner(
    state: &AppState,
    user_id: &str,
    limit: i64,
) -> Result<Value, (StatusCode, String)> {
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

    let allowed_cats = get_user_allowed_categories(state, user_id).await?;
    let rows = batch_build_history_items(
        state,
        &cfg.server_id,
        histories,
        false,
        allowed_cats.as_deref(),
    )
    .await?;

    Ok(json!({
        "Items": rows,
        "TotalRecordCount": rows.len()
    }))
}

/// 接下来观看列表 (`GET /Shows/NextUp`)：
/// 若某部剧最后一集尚未看完，返回该集继续看；若已看完 (>=90%)，自动查找同季下一集或下一季首集返回
pub async fn get_next_up_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let cfg = state.config.load();
    let allowed_cats = get_user_allowed_categories(&state, &user_id).await?;

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

    let rows = batch_build_history_items(
        &state,
        &cfg.server_id,
        latest_tv_histories,
        true,
        allowed_cats.as_deref(),
    )
    .await?;

    Ok(Json(json!({
        "Items": rows,
        "TotalRecordCount": rows.len()
    })))
}

/// 批量解析历史记录对应的媒体与分集（3 条批处理 SQL 替代原先每行 2-3 条的 N+1 查询），
/// 并组装为 Emby 条目 JSON
#[derive(sqlx::FromRow)]
struct MediaSummary {
    hidx: i64,
    work_id: i64,
    media_type: String,
    title: String,
    year: Option<i32>,
    overview: Option<String>,
    poster_path: Option<String>,
    category: String,
}

#[derive(sqlx::FromRow)]
struct EpisodeSummary {
    hidx: i64,
    season_number: i32,
    episode_number: i32,
    title: Option<String>,
    overview: Option<String>,
    still_path: Option<String>,
    air_date: Option<String>,
    runtime: i32,
}

async fn batch_build_history_items(
    state: &AppState,
    server_id: &str,
    histories: Vec<HistoryRecordRow>,
    next_up_mode: bool,
    allowed_cats: Option<&[String]>,
) -> Result<Vec<Value>, (StatusCode, String)> {
    if histories.is_empty() {
        return Ok(vec![]);
    }

    let n = histories.len();
    let hidx: Vec<i64> = (0..n as i64).collect();
    let wids: Vec<i64> = histories.iter().map(|h| h.work_id.unwrap_or(0)).collect();
    let tids: Vec<i32> = histories.iter().map(|h| h.tmdb_id.unwrap_or(0)).collect();
    let mtypes: Vec<String> = histories.iter().map(|h| h.media_type.clone()).collect();

    // 1. 批量定位媒体条目：优先 work_id 精确匹配，否则按 (tmdb_id, media_type) 匹配
    let media_rows: Vec<MediaSummary> =
        sqlx::query_as(
            r#"
            SELECT t.hidx, w.id AS work_id, w.media_type, w.title, w.year, w.overview, w.poster_path, w.category
            FROM unnest($1::int8[], $2::int8[], $3::int4[], $4::text[]) AS t(hidx, wid, tid, mtype)
            CROSS JOIN LATERAL (
                SELECT w2.id, w2.media_type, w2.title, w2.year, w2.overview, w2.poster_path, w2.category
                FROM media_works w2
                WHERE (w2.id = t.wid OR (t.tid <> 0 AND w2.tmdb_id = t.tid AND w2.media_type = t.mtype))
                  AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = w2.id AND f.is_ignored = FALSE)
                ORDER BY (w2.id = t.wid) DESC, w2.id DESC
                LIMIT 1
            ) w
            "#,
        )
        .bind(&hidx)
        .bind(&wids)
        .bind(&tids)
        .bind(&mtypes)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut media_by_idx: std::collections::HashMap<i64, MediaSummary> = media_rows
        .into_iter()
        .map(|summary| (summary.hidx, summary))
        .collect();

    // 按真实分类或已授权且规则匹配的虚拟视图过滤历史记录。
    let mut access_checks = Vec::with_capacity(media_by_idx.len());
    for summary in media_by_idx.values() {
        access_checks.push((summary.hidx, summary.work_id, summary.category.clone()));
    }
    for (idx, wid, category) in access_checks {
        if !work_access_allowed(state, wid, &category, allowed_cats).await? {
            media_by_idx.remove(&idx);
        }
    }

    // 2. 批量解析目标分集：resume 取 (s, e) 精确集；next_up 取 (s, e) 之后的下一集
    let tv_idx: Vec<usize> = histories
        .iter()
        .enumerate()
        .filter(|(idx, h)| h.media_type == "tv" && media_by_idx.contains_key(&(*idx as i64)))
        .map(|(idx, _)| idx)
        .collect();
    let ep_hidx: Vec<i64> = tv_idx.iter().map(|idx| *idx as i64).collect();
    let ep_wids: Vec<i64> = tv_idx
        .iter()
        .filter_map(|idx| {
            media_by_idx
                .get(&(*idx as i64))
                .map(|summary| summary.work_id)
        })
        .collect();
    let ep_snums: Vec<i32> = tv_idx
        .iter()
        .map(|idx| histories[*idx].season_number)
        .collect();
    let ep_nums: Vec<i32> = tv_idx
        .iter()
        .map(|idx| histories[*idx].episode_number)
        .collect();
    let advances: Vec<bool> = tv_idx
        .iter()
        .map(|idx| next_up_mode && histories[*idx].is_played)
        .collect();

    let mut ep_by_idx: std::collections::HashMap<i64, EpisodeSummary> =
        std::collections::HashMap::new();
    if !ep_wids.is_empty() {
        let rows: Vec<EpisodeSummary> = sqlx::query_as(
            r#"
            SELECT t.hidx, x.season_number, x.episode_number, we.title, we.overview, we.still_path, we.air_date,
                   COALESCE(we.runtime, 0) AS runtime
            FROM unnest($1::int8[], $2::int8[], $3::int4[], $4::int4[], $5::bool[])
                 AS t(hidx, wid, snum, epnum, advance)
            CROSS JOIN LATERAL (
                SELECT sf.season_number, sf.episode_number
                FROM sync_files sf
                JOIN sync_folders f ON f.id = sf.folder_id
                WHERE f.work_id = t.wid AND f.is_ignored = FALSE
                  AND CASE WHEN t.advance
                        THEN (sf.season_number, sf.episode_number) > (t.snum, t.epnum)
                        ELSE (sf.season_number, sf.episode_number) = (t.snum, t.epnum)
                   END
                ORDER BY sf.season_number ASC, sf.episode_number ASC,
                         COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) DESC, sf.id ASC
                LIMIT 1
            ) x
            LEFT JOIN work_episodes we
              ON we.work_id = t.wid AND we.season_number = x.season_number AND we.episode_number = x.episode_number
            "#,
        )
        .bind(&ep_hidx)
        .bind(&ep_wids)
        .bind(&ep_snums)
        .bind(&ep_nums)
        .bind(&advances)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        for row in rows {
            ep_by_idx.insert(row.hidx, row);
        }
    }

    // 3. 组装条目（保持 histories 原始顺序）
    let mut items = Vec::with_capacity(n);
    for (idx, h) in histories.iter().enumerate() {
        let Some(media) = media_by_idx.get(&(idx as i64)) else {
            continue;
        };
        let wid = media.work_id;
        let mtype = media.media_type.as_str();

        if mtype == "movie" {
            if next_up_mode {
                continue;
            }
            let mut obj = json!({
                "Id": format!("work_{}", wid),
                "Name": media.title,
                "ServerId": server_id,
                "Type": "Movie",
                "IsFolder": false,
                "ProductionYear": media.year,
                "Overview": media.overview,
                "MediaType": "Video",
                "ImageTags": if media.poster_path.is_some() { json!({"Primary": crate::web::emby_api::images::image_path_tag(media.poster_path.as_deref())}) } else { json!({}) },
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
            items.push(obj);
            continue;
        }

        let Some(episode) = ep_by_idx.get(&(idx as i64)) else {
            continue;
        };
        let advance = next_up_mode && h.is_played;
        let pos_ticks = if advance { 0 } else { h.position_ticks };
        let played = if advance { false } else { h.is_played };
        items.push(build_episode_item_json(EpisodeJsonInput {
            server_id: server_id.to_string(),
            work_id: wid,
            season_number: episode.season_number,
            episode_number: episode.episode_number,
            title: episode.title.clone(),
            series_name: Some(media.title.clone()),
            overview: episode.overview.clone().or_else(|| media.overview.clone()),
            air_date: episode.air_date.clone(),
            still_path: episode.still_path.clone(),
            poster_path: media.poster_path.clone(),
            playback_position_ticks: pos_ticks,
            duration_ticks: if advance { 0 } else { h.duration_ticks },
            runtime_seconds: episode.runtime,
            is_played: played,
            media_sources: None,
            series_primary_image_tag: true,
        }));
    }

    Ok(items)
}
