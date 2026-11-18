use super::sql::*;
use super::*;
use serde_json::json;
/// 某部剧的季列表（供 `ParentId=work_x` + IncludeItemTypes=Season 与 `/Shows/:id/Seasons` 复用）
pub(in crate::web::emby_api) async fn get_seasons_for_media(
    state: &AppState,
    user_id: &str,
    work_id: i64,
    search_term: Option<&str>,
    start_index: i64,
    limit: i64,
) -> Result<Value, (StatusCode, String)> {
    let cfg = state.config.load();
    if let Err(error) = state
        .scraper
        .priority_tx
        .try_send(ScrapeTask::WorkSeasons(work_id))
    {
        tracing::debug!(work_id, %error, "failed to enqueue season metadata prewarm");
    }

    let series_poster: Option<String> =
        sqlx::query_scalar("SELECT poster_path FROM media_works WHERE id = $1")
            .bind(work_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .flatten();

    let search_pattern = build_search_pattern(search_term);
    let mut seasons_sql = DynSql::new(
        r#"
        SELECT ep_s.season_number, ws.name, ws.overview, ws.poster_path, ws.air_date, COUNT(*) OVER() AS total_count
        FROM (
            SELECT DISTINCT sf.season_number
            FROM sync_files sf
            JOIN sync_folders f ON f.id = sf.folder_id
            WHERE f.work_id = $1 AND f.is_ignored = FALSE
        ) ep_s
        LEFT JOIN work_seasons ws
          ON ws.work_id = $1 AND ws.season_number = ep_s.season_number
        WHERE TRUE
        "#,
    );
    seasons_sql.bind(work_id)?;
    if let Some(pattern) = search_pattern {
        let pattern_param = seasons_sql.bind(pattern)?;
        seasons_sql.push(&format!(
            " AND (COALESCE(ws.name, '') ILIKE {pattern_param} OR EXISTS (SELECT 1 FROM sync_files sf2 JOIN sync_folders f2 ON f2.id = sf2.folder_id WHERE f2.work_id = $1 AND f2.is_ignored = FALSE AND sf2.season_number = ep_s.season_number AND sf2.file_name ILIKE {pattern_param}))"
        ));
    }
    seasons_sql.push(" ORDER BY ep_s.season_number ASC");
    seasons_sql.push_pagination(limit.clamp(1, 200), start_index.max(0))?;
    let season_rows: Vec<SeasonListRow> =
        sqlx::query_as_with::<Postgres, SeasonListRow, _>(&seasons_sql.sql, seasons_sql.args)
            .fetch_all(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let total_count = season_rows.first().map(|row| row.total_count).unwrap_or(0);
    let media_meta: Option<(Option<i32>, String)> =
        sqlx::query_as("SELECT tmdb_id, media_type FROM media_works WHERE id = $1")
            .bind(work_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let (tmdb_id, media_type) = media_meta.unwrap_or((None, "tv".to_string()));
    let history_map = crate::web::emby_api::sessions::load_media_episode_user_data(
        state,
        user_id,
        work_id,
        tmdb_id,
        &media_type,
    )
    .await?;
    let present: Vec<(i32, i32)> = sqlx::query_as(
        "SELECT DISTINCT sf.season_number, sf.episode_number FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE f.work_id = $1 AND f.is_ignored = FALSE",
    )
    .bind(work_id)
    .fetch_all(&state.db)
    .await
    .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
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
            let season_episodes: Vec<_> = present
                .iter()
                .filter(|(season, _)| *season == snum)
                .collect();
            let played_count = season_episodes
                .iter()
                .filter(|(_, episode)| {
                    history_map
                        .get(&(snum, *episode))
                        .is_some_and(|history| history.is_played)
                })
                .count();
            let season_histories: Vec<_> = season_episodes
                .iter()
                .filter_map(|(_, episode)| history_map.get(&(snum, *episode)))
                .collect();
            let mut obj = json!({
                "Id": format!("season_{}_{}", work_id, snum),
                "Name": name,
                "ServerId": cfg.server_id,
                "SeriesId": format!("work_{}", work_id),
                "Type": "Season",
                "IsFolder": true,
                "IndexNumber": snum,
                "Overview": row.overview,
                "PremiereDate": row.air_date,
                "UserData": {
                    "PlaybackPositionTicks": season_histories.iter().map(|history| history.position_ticks).max().unwrap_or(0),
                    "PlayCount": season_histories.iter().map(|history| history.play_count).max().unwrap_or(0),
                    "Played": !season_episodes.is_empty() && played_count == season_episodes.len(),
                    "IsFavorite": false
                },
                "ImageTags": if has_poster { json!({"Primary": crate::web::emby_api::images::image_path_tag(row.poster_path.as_deref().or(series_poster.as_deref()))}) } else { json!({}) }
            });
            if has_poster {
                obj["BackdropImageTags"] = json!([]);
            }
            obj
        })
        .collect();

    Ok(json!({
        "Items": items,
        "StartIndex": start_index.max(0),
        "TotalRecordCount": total_count
    }))
}

/// 某部剧/某季的分集列表；`season_number=None` 返回全剧，`Some(n)` 仅返回该季；
/// `search_term` 按 TMDB 分集标题与本地文件名过滤
pub(in crate::web::emby_api) async fn get_episodes_for_media(
    state: &AppState,
    user_id: &str,
    work_id: i64,
    season_number: Option<i32>,
    search_term: Option<&str>,
    start_index: i64,
    limit: i64,
) -> Result<Value, (StatusCode, String)> {
    let cfg = state.config.load();
    if let Err(error) = state
        .scraper
        .priority_tx
        .try_send(ScrapeTask::WorkSeasons(work_id))
    {
        tracing::debug!(work_id, %error, "failed to enqueue season metadata prewarm");
    }

    let media_meta: Option<(Option<i32>, String, String)> =
        sqlx::query_as("SELECT tmdb_id, media_type, title FROM media_works WHERE id = $1")
            .bind(work_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let (tmdb_id, _mtype, series_title) =
        media_meta.unwrap_or((None, "tv".to_string(), String::new()));
    let history_map = crate::web::emby_api::sessions::load_media_episode_user_data(
        state, user_id, work_id, tmdb_id, &_mtype,
    )
    .await?;

    let favorite: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM user_favorites WHERE user_id=$1 AND (work_id=$2 OR (tmdb_id IS NOT NULL AND tmdb_id=$3 AND media_type=$4)))")
        .bind(user_id).bind(work_id).bind(tmdb_id).bind(&_mtype).fetch_one(&state.db).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
    let search_pattern = build_search_pattern(search_term);
    let mut episodes_sql = DynSql::new(
        r#"
        SELECT DISTINCT ON (sf.season_number, sf.episode_number)
            sf.season_number, sf.episode_number, we.title, we.overview, we.still_path,
            we.air_date, COALESCE(we.runtime, 0) AS runtime
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        LEFT JOIN work_episodes we
          ON we.work_id = $1
         AND we.season_number = sf.season_number
         AND we.episode_number = sf.episode_number
        WHERE f.work_id = $1 AND f.is_ignored = FALSE
        "#,
    );
    episodes_sql.bind(work_id)?;
    if let Some(season) = season_number {
        let season_param = episodes_sql.bind(season)?;
        episodes_sql.push(&format!(" AND sf.season_number = {season_param}"));
    }
    if let Some(pattern) = search_pattern {
        let pattern_param = episodes_sql.bind(pattern)?;
        episodes_sql.push(&format!(
            " AND (we.title ILIKE {pattern_param} OR sf.file_name ILIKE {pattern_param})"
        ));
    }
    episodes_sql.push(" ORDER BY sf.season_number ASC, sf.episode_number ASC, COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) DESC, sf.id ASC");
    let total_sql = format!("SELECT COUNT(*) FROM ({}) episodes", episodes_sql.sql);
    let total_args = episodes_sql.args.clone();
    let total: i64 = sqlx::query_scalar_with::<Postgres, i64, _>(&total_sql, total_args)
        .fetch_one(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    episodes_sql.push_pagination(limit.clamp(1, 200), start_index.max(0))?;
    let eps: Vec<EpisodeGroupRow> =
        sqlx::query_as_with::<Postgres, EpisodeGroupRow, _>(&episodes_sql.sql, episodes_sql.args)
            .fetch_all(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let items: Vec<Value> = eps
        .into_iter()
        .map(|ep| {
            let history = history_map
                .get(&(ep.season_number, ep.episode_number))
                .cloned()
                .unwrap_or_default();
            build_episode_item_json(EpisodeJsonInput {
                server_id: cfg.server_id.clone(),
                work_id,
                season_number: ep.season_number,
                episode_number: ep.episode_number,
                title: ep.title,
                series_name: Some(series_title.clone()),
                overview: ep.overview,
                air_date: ep.air_date,
                still_path: ep.still_path,
                poster_path: None,
                playback_position_ticks: history.position_ticks,
                duration_ticks: history.duration_ticks,
                runtime_seconds: ep.runtime,
                is_played: history.is_played,
                is_favorite: favorite,
                play_count: history.play_count,
                last_played_date: history.last_played_date,
                media_sources: None,
                series_primary_image_tag: false,
            })
        })
        .collect();

    Ok(json!({
        "Items": items,
        "StartIndex": start_index.max(0),
        "TotalRecordCount": total
    }))
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

/// 全局单集列表（IncludeItemTypes=Episode 且无剧集父级）：
/// 从 sync_files + media_works + work_episodes 分页查询，搜索覆盖分集标题与剧名
pub(in crate::web::emby_api) struct EpisodeListingOptions {
    pub(super) category_filter: Option<String>,
    pub(super) access_scope: Arc<crate::web::emby_api::views::UserAccessScope>,
    pub(super) vlib_rule: Option<VirtualLibraryRule>,
    pub(super) limit: i64,
    pub(super) offset: i64,
}

pub(in crate::web::emby_api) async fn get_episodes_listing(
    state: &AppState,
    user_id: &str,
    q: &ItemsQuery,
    options: EpisodeListingOptions,
) -> Result<Value, (StatusCode, String)> {
    let EpisodeListingOptions {
        category_filter,
        access_scope,
        vlib_rule,
        limit,
        offset,
    } = options;
    let search_term = q.search_term.as_deref();
    let search_pattern = build_search_pattern(search_term);
    let rule_json = vlib_rule
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let build = async |count_only: bool| -> Result<DynSql, (StatusCode, String)> {
        let mut sql = DynSql::new(
            r#"
            WITH filtered_episodes AS (
                SELECT DISTINCT ON (fx.work_id, sfx.season_number, sfx.episode_number)
                       fx.work_id, m.tmdb_id, m.title AS series_title, m.year,
                       sfx.season_number, sfx.episode_number,
                       we.title AS episode_title, we.overview, we.still_path, we.air_date,
                       COALESCE(we.runtime, 0) AS runtime
                FROM sync_files sfx
                JOIN sync_folders fx ON fx.id = sfx.folder_id
                JOIN media_works m ON m.id = fx.work_id
                LEFT JOIN work_episodes we
                  ON we.work_id = fx.work_id AND we.season_number = sfx.season_number AND we.episode_number = sfx.episode_number
                WHERE fx.work_id IS NOT NULL AND fx.is_ignored = FALSE
                  AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE)
            "#,
        );
        push_metadata_query_filters(state, user_id, q, &mut sql).await?;
        if let Some(cat) = &category_filter {
            sql.push_bind_push(" AND m.category = ", cat.clone(), "")?;
        }
        push_user_access_scope(&mut sql, &access_scope)?;
        if let Some(rule) = &vlib_rule {
            let rj = sql.bind(rule_json.clone().unwrap_or(Value::Null))?;
            sql.push(&format!(
                " AND {}",
                rule.sql_predicate(&format!("{}::jsonb", rj))
            ));
        }
        if let Some(p) = &search_pattern {
            let pat = sql.bind(p.clone())?;
            sql.push(&format!(
                " AND (we.title ILIKE {pat} OR m.title ILIKE {pat} OR m.original_title ILIKE {pat} OR m.sort_title ILIKE {pat} OR EXISTS (SELECT 1 FROM sync_folders sf_kw WHERE sf_kw.work_id = m.id AND sf_kw.is_ignored = FALSE AND (sf_kw.clean_title ILIKE {pat} OR sf_kw.raw_folder_name ILIKE {pat})))"
            ));
        }
        sql.push(" ORDER BY fx.work_id, sfx.season_number, sfx.episode_number, COALESCE(sfx.priority_override, sfx.priority_score + fx.priority_boost) DESC, sfx.id ASC) ");
        if count_only {
            sql.push("SELECT COUNT(*) FROM filtered_episodes");
        } else {
            sql.push("SELECT work_id, tmdb_id, series_title, season_number, episode_number, episode_title, overview, still_path, air_date, runtime, COUNT(*) OVER() AS total_count FROM filtered_episodes ORDER BY year DESC NULLS LAST, work_id DESC, season_number ASC, episode_number ASC");
            sql.push_pagination(limit, offset)?;
        }
        Ok(sql)
    };

    let page_sql = build(false).await?;
    let rows: Vec<EpisodeListRow> =
        sqlx::query_as_with::<Postgres, EpisodeListRow, _>(&page_sql.sql, page_sql.args)
            .fetch_all(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let total = if let Some(row) = rows.first() {
        row.total_count
    } else if offset == 0 {
        0
    } else {
        let count_sql = build(true).await?;
        sqlx::query_scalar_with::<Postgres, i64, _>(&count_sql.sql, count_sql.args)
            .fetch_one(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };

    let page_wids: Vec<i64> = rows.iter().map(|r| r.work_id).collect();
    let page_tids: Vec<i32> = rows.iter().filter_map(|r| r.tmdb_id).collect();
    let episode_histories: Vec<EpisodeListingHistoryRow> = if page_wids.is_empty() {
        vec![]
    } else {
        sqlx::query_as(
            "SELECT work_id, tmdb_id, season_number, episode_number, position_ticks, duration_ticks, is_played, play_count, last_played_at AS last_played_date FROM play_histories WHERE user_id=$1 AND (work_id = ANY($2) OR (tmdb_id IS NOT NULL AND tmdb_id = ANY($3) AND media_type = 'tv')) ORDER BY (work_id = ANY($2)) DESC, (work_id IS NOT NULL) DESC",
        )
        .bind(user_id)
        .bind(&page_wids)
        .bind(&page_tids)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };
    let mut episode_history_by_work = std::collections::HashMap::new();
    let mut episode_history_by_tmdb = std::collections::HashMap::new();
    for history in episode_histories {
        let value = (
            history.position_ticks,
            history.duration_ticks,
            history.is_played,
            history.play_count,
            history.last_played_date,
        );
        if let Some(work_id) = history.work_id {
            episode_history_by_work
                .entry((work_id, history.season_number, history.episode_number))
                .or_insert(value);
        }
        if let Some(tmdb_id) = history.tmdb_id {
            // Query order prefers rows linked to a current work, but legacy rows
            // with an older work_id remain eligible as a TMDB fallback.
            episode_history_by_tmdb
                .entry((tmdb_id, history.season_number, history.episode_number))
                .or_insert(value);
        }
    }

    let favorites = load_user_favorite_keys(state, user_id).await?;
    let cfg = state.config.load();
    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let history = episode_history_by_work
                .get(&(r.work_id, r.season_number, r.episode_number))
                .or_else(|| {
                    r.tmdb_id.and_then(|tmdb_id| {
                        episode_history_by_tmdb.get(&(tmdb_id, r.season_number, r.episode_number))
                    })
                })
                .copied();
            let ep = EpisodeGroupRow {
                season_number: r.season_number,
                episode_number: r.episode_number,
                title: r.episode_title,
                overview: r.overview,
                still_path: r.still_path,
                air_date: r.air_date,
                runtime: r.runtime,
            };
            let (position, duration, played, play_count, last_played_date) =
                history.unwrap_or((0, 0, false, 0, None));
            build_episode_item_json(EpisodeJsonInput {
                server_id: cfg.server_id.clone(),
                work_id: r.work_id,
                season_number: ep.season_number,
                episode_number: ep.episode_number,
                title: ep.title,
                series_name: Some(r.series_title),
                overview: ep.overview,
                air_date: ep.air_date,
                still_path: ep.still_path,
                poster_path: None,
                playback_position_ticks: position,
                duration_ticks: duration,
                runtime_seconds: ep.runtime,
                is_played: played,
                is_favorite: is_media_favorited(&favorites, r.work_id, r.tmdb_id, "tv"),
                play_count,
                last_played_date,
                media_sources: None,
                series_primary_image_tag: false,
            })
        })
        .collect();

    Ok(json!({
        "Items": items,
        "StartIndex": offset,
        "TotalRecordCount": total
    }))
}

#[cfg(test)]
mod sql_tests {
    #[test]
    fn global_episode_history_falls_back_by_tmdb_even_with_legacy_work_id() {
        let source = include_str!("episodes.rs");
        let implementation = source.split("#[cfg(test)]").next().unwrap();
        assert!(implementation.contains("tmdb_id = ANY($3) AND media_type = 'tv'"));
        assert!(implementation
            .contains("ORDER BY (work_id = ANY($2)) DESC, (work_id IS NOT NULL) DESC"));
        assert!(!implementation.contains("work_id IS NULL AND tmdb_id IS NOT NULL"));
    }

    #[test]
    fn global_episode_sql_uses_flat_top_n_and_folder_work_id() {
        let source = include_str!("episodes.rs");
        let implementation = source.split("#[cfg(test)]").next().unwrap();
        assert!(implementation
            .contains("SELECT DISTINCT ON (fx.work_id, sfx.season_number, sfx.episode_number)"));
        assert!(!implementation.contains("sfx.work_id"));
        assert!(implementation.contains(
            "ORDER BY year DESC NULLS LAST, work_id DESC, season_number ASC, episode_number ASC"
        ));
        let selection = implementation
            .find("SELECT DISTINCT ON (fx.work_id, sfx.season_number, sfx.episode_number)")
            .unwrap();
        let dedup_sort = implementation
            .find("ORDER BY fx.work_id, sfx.season_number, sfx.episode_number")
            .unwrap();
        assert!(selection < dedup_sort);
        assert!(implementation[selection..dedup_sort]
            .contains("JOIN media_works m ON m.id = fx.work_id"));
    }
}

#[derive(sqlx::FromRow)]
struct EpisodeListingHistoryRow {
    work_id: Option<i64>,
    tmdb_id: Option<i32>,
    season_number: i32,
    episode_number: i32,
    position_ticks: i64,
    duration_ticks: i64,
    is_played: bool,
    play_count: i32,
    last_played_date: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(sqlx::FromRow)]
struct EpisodeListRow {
    total_count: i64,
    work_id: i64,
    tmdb_id: Option<i32>,
    series_title: String,
    season_number: i32,
    episode_number: i32,
    episode_title: Option<String>,
    overview: Option<String>,
    still_path: Option<String>,
    air_date: Option<String>,
    runtime: i32,
}

pub async fn get_show_seasons(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(series_id): Path<String>,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    let wid = parse_work_id(&series_id)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "无效的 series_id".to_string()))?;

    ensure_media_accessible(&state, &user_id, wid).await?;

    // 零转存浪费：进入剧集季列表时，后台执行只读 fs/list 预热小雅目录树缓存并回填体积
    state.spawn_prewarm_media_dirs(wid, None);

    let search_term = q.search_term.as_deref();
    Ok(Json(
        get_seasons_for_media(
            &state,
            &user_id,
            wid,
            search_term,
            q.start_index,
            q.limit.unwrap_or(50).clamp(1, 200),
        )
        .await?,
    ))
}

#[derive(Deserialize, Default)]
pub(in crate::web::emby_api) struct ShowEpisodesQuery {
    #[serde(rename = "SeasonId", alias = "seasonId")]
    season_id: Option<String>,
    #[serde(rename = "Season", alias = "season")]
    season: Option<i32>,
    #[serde(rename = "UserId", alias = "userId")]
    user_id: Option<String>,
    #[serde(rename = "SearchTerm", alias = "searchTerm")]
    search_term: Option<String>,
    #[serde(
        rename = "api_key",
        alias = "ApiKey",
        alias = "X-Emby-Token",
        alias = "x-emby-token"
    )]
    api_key: Option<String>,
    #[serde(rename = "StartIndex", alias = "startIndex", default)]
    start_index: i64,
    #[serde(rename = "Limit", alias = "limit")]
    limit: Option<i64>,
}

pub async fn get_show_episodes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(series_id): Path<String>,
    Query(q): Query<ShowEpisodesQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    let wid = parse_work_id(&series_id)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "无效的 series_id".to_string()))?;

    ensure_media_accessible(&state, &user_id, wid).await?;

    // 兼容 VidHub 传入 SeasonId="season_123_1" 或 Season=1
    let target_season: Option<i32> = q.season.or_else(|| {
        q.season_id
            .as_deref()
            .and_then(|sid| parse_season_id(sid).map(|(_, snum)| snum))
    });

    // 零转存浪费：展开某季分集列表时，确保该季小雅父目录寻址与文件体积已预热
    state.spawn_prewarm_media_dirs(wid, target_season);

    let res = get_episodes_for_media(
        &state,
        &user_id,
        wid,
        target_season,
        q.search_term.as_deref(),
        q.start_index,
        q.limit.unwrap_or(50).clamp(1, 200),
    )
    .await?;
    Ok(Json(res))
}
