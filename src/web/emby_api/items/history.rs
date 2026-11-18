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
    let mut latest_query = q.clone();
    if latest_query.limit.is_none() {
        latest_query.limit = Some(20);
    }
    if latest_query.sort_by.is_none() {
        latest_query.sort_by = Some("DateCreated".to_string());
    }
    if latest_query.sort_order.is_none() {
        latest_query.sort_order = Some("Descending".to_string());
    }
    let mut res = get_user_items_inner(&state, &user_id, &headers, &latest_query).await?;
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
    Ok(Json(
        get_resume_items_inner(&state, &user_id, limit, &q).await?,
    ))
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
    play_count: i32,
    last_played_date: Option<chrono::DateTime<chrono::Utc>>,
    total_count: i64,
}

pub(in crate::web::emby_api) async fn get_resume_items_inner(
    state: &AppState,
    user_id: &str,
    limit: i64,
    q: &ItemsQuery,
) -> Result<Value, (StatusCode, String)> {
    let cfg = state.config.load();
    let access_scope = user_access_scope(state, user_id).await?;
    let mut history_sql = DynSql::new(
        r#"
        SELECT ph.work_id, ph.tmdb_id, ph.media_type, ph.season_number, ph.episode_number,
               ph.position_ticks, ph.duration_ticks, ph.is_played, ph.play_count, ph.last_played_at AS last_played_date,
               COUNT(*) OVER() AS total_count
        FROM play_histories ph
        JOIN LATERAL (
            SELECT m.id, m.media_type, m.tmdb_id, m.title, m.year, m.overview, m.poster_path, m.category,
                   m.genres, m.networks, m.tags, m.rating
            FROM media_works m
            WHERE m.id = ph.work_id
               OR (ph.tmdb_id IS NOT NULL AND m.tmdb_id = ph.tmdb_id AND m.media_type = ph.media_type)
            ORDER BY (m.id = ph.work_id) DESC, m.id DESC
            LIMIT 1
        ) m ON TRUE
        WHERE ph.hidden_from_resume = FALSE AND ph.user_id = $1 AND ph.is_played = FALSE AND ph.position_ticks > 0
          AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE)
          AND (m.media_type = 'movie' OR EXISTS (
              SELECT 1 FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id
              WHERE f.work_id = m.id AND f.is_ignored = FALSE
                AND sf.season_number = ph.season_number AND sf.episode_number = ph.episode_number
          ))
        "#,
    );
    history_sql.bind(user_id.to_string())?;
    access_scope.push_sql_predicate(&mut history_sql)?;
    push_metadata_query_filters(state, user_id, q, &mut history_sql).await?;
    if let Some(parent_id) = q.parent_id.as_deref() {
        if let Some(work_id) = parse_work_id(parent_id) {
            let parent_param = history_sql.bind(work_id)?;
            history_sql.push(&format!(" AND m.id = {parent_param}"));
        } else if let Some((work_id, season)) = parse_season_id(parent_id) {
            let work_param = history_sql.bind(work_id)?;
            let season_param = history_sql.bind(season)?;
            history_sql.push(&format!(
                " AND m.id = {work_param} AND ph.season_number = {season_param}"
            ));
        }
    }
    if let Some(types) = q.include_item_types.as_deref() {
        let types: Vec<_> = types.split(',').map(str::trim).collect();
        if types.contains(&"Movie")
            && !types
                .iter()
                .any(|kind| matches!(*kind, "Series" | "Episode"))
        {
            history_sql.push(" AND m.media_type = 'movie'");
        } else if types
            .iter()
            .any(|kind| matches!(*kind, "Series" | "Episode"))
            && !types.contains(&"Movie")
        {
            history_sql.push(" AND m.media_type = 'tv'");
        }
    }
    if let Some(pattern) = build_search_pattern(q.search_term.as_deref()) {
        let pattern_param = history_sql.bind(pattern)?;
        history_sql.push(&format!(
            " AND (m.title ILIKE {pattern_param} OR EXISTS (SELECT 1 FROM work_episodes we WHERE we.work_id = m.id AND we.season_number = ph.season_number AND we.episode_number = ph.episode_number AND we.title ILIKE {pattern_param}) OR EXISTS (SELECT 1 FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE f.work_id = m.id AND f.is_ignored = FALSE AND sf.season_number = ph.season_number AND sf.episode_number = ph.episode_number AND sf.file_name ILIKE {pattern_param}))"
        ));
    }
    history_sql.push(" ORDER BY ph.last_played_at DESC");
    history_sql.push_pagination(limit, q.start_index.max(0))?;
    let histories: Vec<HistoryRecordRow> =
        sqlx::query_as_with::<sqlx::Postgres, HistoryRecordRow, _>(
            &history_sql.sql,
            history_sql.args,
        )
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let total = histories.first().map(|row| row.total_count).unwrap_or(0);
    let rows = batch_build_history_items(state, user_id, &cfg.server_id, histories, false).await?;

    Ok(json!({
        "Items": rows,
        "StartIndex": q.start_index.max(0),
        "TotalRecordCount": total
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
    let access_scope = user_access_scope(&state, &user_id).await?;

    // 先按当前库可访问性与非忽略来源筛选，再按剧集选最新历史并分页。
    let mut history_sql = DynSql::new(
        r#"
        SELECT work_id, tmdb_id, media_type, season_number, episode_number,
               position_ticks, duration_ticks, is_played, play_count, last_played_date, COUNT(*) OVER() AS total_count
        FROM (
            SELECT DISTINCT ON (COALESCE('t:' || ph.tmdb_id::text, 'w:' || ph.work_id::text))
                ph.work_id, ph.tmdb_id, ph.media_type, ph.season_number, ph.episode_number,
                ph.position_ticks, ph.duration_ticks, ph.is_played, ph.play_count, ph.last_played_at AS last_played_date, ph.last_played_at, ph.hidden_from_resume
            FROM play_histories ph
            JOIN LATERAL (
                SELECT m.*
                FROM media_works m
                WHERE m.id = ph.work_id
                   OR (ph.tmdb_id IS NOT NULL AND m.tmdb_id = ph.tmdb_id AND m.media_type = ph.media_type)
                ORDER BY (m.id = ph.work_id) DESC, m.id DESC
                LIMIT 1
            ) m ON TRUE
            WHERE ph.user_id = $1 AND ph.media_type = 'tv' AND m.media_type = 'tv'
              AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE)
              AND EXISTS (
                  SELECT 1 FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id
                  WHERE f.work_id = m.id AND f.is_ignored = FALSE
                    AND sf.season_number = ph.season_number AND sf.episode_number = ph.episode_number
              )
        "#,
    );
    history_sql.bind(user_id.clone())?;
    access_scope.push_sql_predicate(&mut history_sql)?;
    push_metadata_query_filters(&state, &user_id, &q, &mut history_sql).await?;
    history_sql.push(" ORDER BY COALESCE('t:' || ph.tmdb_id::text, 'w:' || ph.work_id::text), ph.last_played_at DESC) t WHERE hidden_from_resume = FALSE ORDER BY last_played_at DESC");
    history_sql.push_pagination(limit, q.start_index.max(0))?;
    let latest_tv_histories: Vec<HistoryRecordRow> = sqlx::query_as_with::<
        sqlx::Postgres,
        HistoryRecordRow,
        _,
    >(&history_sql.sql, history_sql.args)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let total = latest_tv_histories
        .first()
        .map(|row| row.total_count)
        .unwrap_or(0);
    let rows =
        batch_build_history_items(&state, &user_id, &cfg.server_id, latest_tv_histories, true)
            .await?;

    Ok(Json(json!({
        "Items": rows,
        "StartIndex": q.start_index.max(0),
        "TotalRecordCount": total
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
    position_ticks: i64,
    duration_ticks: i64,
    is_played: bool,
    play_count: i32,
    last_played_date: Option<chrono::DateTime<chrono::Utc>>,
}

async fn batch_build_history_items(
    state: &AppState,
    user_id: &str,
    server_id: &str,
    histories: Vec<HistoryRecordRow>,
    next_up_mode: bool,
) -> Result<Vec<Value>, (StatusCode, String)> {
    if histories.is_empty() {
        return Ok(vec![]);
    }

    let favorites = load_user_favorite_keys(state, user_id).await?;
    let n = histories.len();
    let hidx: Vec<i64> = (0..n as i64).collect();
    let wids: Vec<i64> = histories.iter().map(|h| h.work_id.unwrap_or(0)).collect();
    let tids: Vec<i32> = histories.iter().map(|h| h.tmdb_id.unwrap_or(0)).collect();
    let mtypes: Vec<String> = histories.iter().map(|h| h.media_type.clone()).collect();

    // 1. 批量定位媒体条目：优先 work_id 精确匹配，否则按 (tmdb_id, media_type) 匹配
    let media_rows: Vec<MediaSummary> =
        sqlx::query_as(
            r#"
            SELECT t.hidx, w.id AS work_id, w.media_type, w.title, w.year, w.overview, w.poster_path
            FROM unnest($1::int8[], $2::int8[], $3::int4[], $4::text[]) AS t(hidx, wid, tid, mtype)
            CROSS JOIN LATERAL (
                SELECT w2.id, w2.media_type, w2.title, w2.year, w2.overview, w2.poster_path
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

    let media_by_idx: std::collections::HashMap<i64, MediaSummary> = media_rows
        .into_iter()
        .map(|summary| (summary.hidx, summary))
        .collect();

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
                   COALESCE(we.runtime, 0) AS runtime,
                   COALESCE(ph.position_ticks,0) AS position_ticks, COALESCE(ph.duration_ticks,0) AS duration_ticks,
                   COALESCE(ph.is_played,FALSE) AS is_played, COALESCE(ph.play_count,0) AS play_count,
                   ph.last_played_at AS last_played_date
            FROM unnest($1::int8[], $2::int8[], $3::int4[], $4::int4[], $5::bool[])
                 AS t(hidx, wid, snum, epnum, advance)
            CROSS JOIN LATERAL (
                SELECT sf.season_number, sf.episode_number
                FROM sync_files sf
                JOIN sync_folders f ON f.id = sf.folder_id
                WHERE f.work_id = t.wid AND f.is_ignored = FALSE
                  AND NOT EXISTS (
                    SELECT 1 FROM play_histories hidden
                    JOIN media_works hw ON hw.id = t.wid
                    WHERE hidden.user_id = $6 AND hidden.hidden_from_resume = TRUE
                      AND (hidden.work_id=t.wid OR (hidden.tmdb_id=hw.tmdb_id AND hidden.media_type=hw.media_type))
                      AND hidden.season_number=sf.season_number AND hidden.episode_number=sf.episode_number
                  )
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
            LEFT JOIN LATERAL (
                SELECT p.* FROM play_histories p JOIN media_works mw ON mw.id=t.wid
                WHERE p.user_id=$6 AND (p.work_id=t.wid OR (p.tmdb_id=mw.tmdb_id AND p.media_type=mw.media_type))
                  AND p.season_number=x.season_number AND p.episode_number=x.episode_number
                ORDER BY (p.work_id=t.wid) DESC, p.last_played_at DESC LIMIT 1
            ) ph ON TRUE
            "#,
        )
        .bind(&ep_hidx)
        .bind(&ep_wids)
        .bind(&ep_snums)
        .bind(&ep_nums)
        .bind(&advances)
        .bind(user_id)
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
                    "PlayCount": h.play_count,
                    "LastPlayedDate": h.last_played_date,
                    "IsFavorite": is_media_favorited(&favorites, wid, h.tmdb_id, mtype),
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
        let pos_ticks = if advance {
            episode.position_ticks
        } else {
            h.position_ticks
        };
        let played = if advance {
            episode.is_played
        } else {
            h.is_played
        };
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
            duration_ticks: if advance {
                episode.duration_ticks
            } else {
                h.duration_ticks
            },
            runtime_seconds: episode.runtime,
            is_played: played,
            is_favorite: is_media_favorited(&favorites, wid, h.tmdb_id, mtype),
            play_count: if advance {
                episode.play_count
            } else {
                h.play_count
            },
            last_played_date: if advance {
                episode.last_played_date
            } else {
                h.last_played_date
            },
            media_sources: None,
            series_primary_image_tag: true,
        }));
    }

    Ok(items)
}

#[cfg(test)]
mod tests {
    #[test]
    fn next_up_deduplicates_by_resolved_work_and_orders_distinct_selection() {
        let source = include_str!("history.rs");
        let next_up = source.find("pub async fn get_next_up_items").unwrap();
        let sql = &source[next_up..source.find("async fn batch_build_history_items").unwrap()];
        let dedup_key = "COALESCE('t:' || ph.tmdb_id::text, 'w:' || ph.work_id::text)";
        assert!(sql.contains(&format!("DISTINCT ON ({dedup_key})")));
        assert!(sql.contains(&format!("ORDER BY {dedup_key}, ph.last_played_at DESC")));
        assert!(!sql.contains("m.media_type = 'movie'"));
    }

    #[test]
    fn resume_and_next_up_apply_access_scope_before_limit() {
        let source = include_str!("history.rs");
        let resume = source
            .find("pub(in crate::web::emby_api) async fn get_resume_items_inner")
            .unwrap();
        let next_up = source.find("pub async fn get_next_up_items").unwrap();
        let resume_sql = &source[resume..next_up];
        let next_up_sql =
            &source[next_up..source.find("async fn batch_build_history_items").unwrap()];
        for query in [resume_sql, next_up_sql] {
            assert!(query.contains("f.is_ignored = FALSE"));
            assert!(
                query.find("push_sql_predicate").unwrap() < query.rfind("push_pagination").unwrap()
            );
        }
    }
}
