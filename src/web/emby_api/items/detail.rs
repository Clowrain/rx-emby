use super::episode_json::{build_episode_item_json, EpisodeJsonInput};
use super::sql::*;
use serde_json::json;
pub async fn get_item_detail(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((path_user_id, item_id)): Path<(String, String)>,
    Query(q): Query<PlaybackQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id_with_query(
        &state,
        Some(&path_user_id),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    Ok(Json(
        get_item_detail_inner(&state, &user_id, &headers, &q, &item_id).await?,
    ))
}

/// 非 user-scoped 别名 `GET /Items/:item_id`
pub async fn get_item_detail_unscoped(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<String>,
    Query(q): Query<PlaybackQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id =
        resolve_valid_user_id_with_query(&state, None, &headers, q.api_key.as_deref()).await?;
    Ok(Json(
        get_item_detail_inner(&state, &user_id, &headers, &q, &item_id).await?,
    ))
}

type EpisodeMetadata = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i32,
);
type SeasonMetadata = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

async fn get_item_detail_inner(
    state: &Arc<AppState>,
    user_id: &str,
    headers: &HeaderMap,
    q: &PlaybackQuery,
    item_id: &str,
) -> Result<Value, (StatusCode, String)> {
    if item_id.starts_with("pl_") || item_id.starts_with("col_") {
        return crate::web::emby_api::playlists::list_detail(
            state,
            user_id,
            item_id,
            item_id.starts_with("col_"),
        )
        .await;
    }
    if let Some(source_id) = item_id
        .strip_prefix("src_")
        .and_then(|id| id.parse::<i64>().ok())
    {
        let source: Option<(i64, i32, i32, String)> = sqlx::query_as("SELECT f.work_id, sf.season_number, sf.episode_number, w.media_type FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id JOIN media_works w ON w.id=f.work_id WHERE sf.id=$1 AND f.work_id IS NOT NULL AND f.is_ignored=FALSE")
            .bind(source_id).fetch_optional(&state.db).await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let Some((wid, season, episode, media_type)) = source else {
            return Err((StatusCode::NOT_FOUND, "媒体源不存在或已被屏蔽".into()));
        };
        ensure_media_accessible(state, user_id, wid).await?;
        let canonical_id = if media_type == "tv" {
            format!("ep_{wid}_{season}_{episode}")
        } else {
            format!("work_{wid}")
        };
        return Box::pin(get_item_detail_inner(
            state,
            user_id,
            headers,
            q,
            &canonical_id,
        ))
        .await;
    }
    let cfg = state.config.load();
    let token = extract_emby_token(headers, q.api_key.as_deref())
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let session_id = Uuid::new_v4().simple().to_string();

    if let Some((wid, snum)) = parse_season_id(item_id) {
        return get_season_detail(state, user_id, wid, snum).await;
    }

    if let Some(raw_wid) = item_id
        .strip_prefix("work_")
        .or_else(|| item_id.strip_prefix("media_"))
    {
        let wid = raw_wid
            .parse::<i64>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的 work_id".to_string()))?;

        ensure_media_accessible(state, user_id, wid).await?;
        state.spawn_prewarm_media_dirs(wid, None);

        let row: Option<WorkDetailRow> = sqlx::query_as(
            r#"
            SELECT media_type, tmdb_id, title, year, overview, rating, poster_path, backdrop_path,
                   (tmdb_metadata->>'number_of_seasons')::int AS num_seasons,
                   (tmdb_metadata->>'number_of_episodes')::int AS num_episodes,
                   original_title, sort_title
            FROM media_works WHERE id = $1
            "#,
        )
        .bind(wid)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("查询作品详情失败 (work_id={}): {}", wid, e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;

        let Some(work) = row else {
            return Err((StatusCode::NOT_FOUND, "条目不存在".to_string()));
        };

        if work.media_type == "tv" {
            if let Err(error) = state
                .scraper
                .priority_tx
                .try_send(ScrapeTask::WorkSeasons(wid))
            {
                tracing::debug!(work_id = wid, %error, "failed to enqueue season metadata prewarm");
            }
        }

        let is_fav: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM user_favorites WHERE user_id=$1 AND ((work_id=$2) OR (tmdb_id IS NOT NULL AND tmdb_id=$3 AND media_type=$4)))",
        )
        .bind(user_id)
        .bind(wid)
        .bind(work.tmdb_id)
        .bind(&work.media_type)
        .fetch_one(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let history_map = crate::web::emby_api::sessions::load_media_episode_user_data(
            state,
            user_id,
            wid,
            work.tmdb_id,
            &work.media_type,
        )
        .await?;
        let user_data = history_map.get(&(0, 1)).cloned().unwrap_or_default();
        let (pos_ticks, dur_ticks, is_played, play_count, last_played_date, unplayed_count) =
            if work.media_type == "tv" {
                let present: Vec<(i32, i32)> = sqlx::query_as(
                    "SELECT DISTINCT sf.season_number, sf.episode_number FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id WHERE f.work_id=$1 AND f.is_ignored=FALSE",
                ).bind(wid).fetch_all(&state.db).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
                let unplayed = present
                    .iter()
                    .filter(|key| !history_map.get(key).is_some_and(|h| h.is_played))
                    .count();
                let count = present
                    .iter()
                    .filter_map(|key| history_map.get(key).map(|h| h.play_count))
                    .max()
                    .unwrap_or(0);
                let last = present
                    .iter()
                    .filter_map(|key| history_map.get(key).and_then(|h| h.last_played_date))
                    .max();
                (
                    0,
                    0,
                    !present.is_empty() && unplayed == 0,
                    count,
                    last,
                    unplayed,
                )
            } else {
                (
                    user_data.position_ticks,
                    user_data.duration_ticks,
                    user_data.is_played,
                    user_data.play_count,
                    user_data.last_played_date,
                    0,
                )
            };

        let mut media_sources = vec![];
        if work.media_type == "movie" {
            media_sources = build_media_sources_for_movie(state, &token, &session_id, wid).await?;
        }

        let mut image_tags = json!({});
        if work.poster_path.is_some() {
            image_tags["Primary"] = json!(crate::web::emby_api::images::image_path_tag(
                work.poster_path.as_deref()
            ));
        }

        let mut resp_obj = json!({
            "Id": format!("work_{}", wid),
            "Name": work.title,
            "OriginalTitle": work.original_title,
            "SortName": work.sort_title.as_deref().filter(|name| !name.is_empty()).unwrap_or(&work.title),
            "ServerId": cfg.server_id,
            "Type": if work.media_type == "movie" { "Movie" } else { "Series" },
            "ProductionYear": work.year,
            "Overview": work.overview,
            "CommunityRating": work.rating,
            "ImageTags": image_tags,
            "MediaSources": media_sources,
            "UserData": {
                "PlaybackPositionTicks": pos_ticks,
                "PlayCount": play_count,
                "LastPlayedDate": last_played_date,
                "UnplayedItemCount": unplayed_count,
                "IsFavorite": is_fav,
                "Played": is_played
            }
        });
        if work.backdrop_path.is_some() {
            resp_obj["BackdropImageTags"] = json!([crate::web::emby_api::images::image_path_tag(
                work.backdrop_path.as_deref()
            )]);
        }
        if let Some(tid) = work.tmdb_id {
            resp_obj["ProviderIds"] = json!({ "Tmdb": tid.to_string() });
        }
        if work.media_type == "tv" {
            if let Some(seasons) = work.num_seasons {
                resp_obj["ChildCount"] = json!(seasons);
            }
            if let Some(episodes) = work.num_episodes {
                resp_obj["RecursiveItemCount"] = json!(episodes);
            }
        } else {
            resp_obj["RecursiveItemCount"] = json!(1);
        }
        if dur_ticks > 0 {
            resp_obj["RunTimeTicks"] = json!(dur_ticks);
        }
        return Ok(resp_obj);
    }

    if let Some((wid, snum, epnum)) = parse_episode_id(item_id) {
        ensure_media_accessible(state, user_id, wid).await?;

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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        if !has_file {
            return Err((StatusCode::NOT_FOUND, "剧集不存在或已被屏蔽".to_string()));
        }

        let ep_meta: Option<EpisodeMetadata> = sqlx::query_as(
            "SELECT title, overview, still_path, air_date, runtime FROM work_episodes WHERE work_id = $1 AND season_number = $2 AND episode_number = $3",
        )
        .bind(wid)
        .bind(snum)
        .bind(epnum)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let (title, overview, still_path, air_date, runtime) =
            ep_meta.unwrap_or((None, None, None, None, 0));

        let media_meta: Option<(Option<i32>, String, String)> =
            sqlx::query_as("SELECT tmdb_id, media_type, title FROM media_works WHERE id = $1")
                .bind(wid)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let (tmdb_id, mtype, series_title) =
            media_meta.unwrap_or((None, "tv".to_string(), String::new()));
        let history_map = crate::web::emby_api::sessions::load_media_episode_user_data(
            state, user_id, wid, tmdb_id, &mtype,
        )
        .await?;
        let user_data = history_map.get(&(snum, epnum)).cloned().unwrap_or_default();
        let (pos_ticks, dur_ticks, is_played) = (
            user_data.position_ticks,
            user_data.duration_ticks,
            user_data.is_played,
        );
        let is_favorite: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM user_favorites WHERE user_id=$1 AND (work_id=$2 OR (tmdb_id IS NOT NULL AND tmdb_id=$3 AND media_type=$4)))",
        )
        .bind(user_id)
        .bind(wid)
        .bind(tmdb_id)
        .bind(&mtype)
        .fetch_one(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let sources =
            build_media_sources_for_episode(state, &token, &session_id, wid, snum, epnum).await?;
        return Ok(build_episode_item_json(EpisodeJsonInput {
            server_id: cfg.server_id.clone(),
            work_id: wid,
            season_number: snum,
            episode_number: epnum,
            title,
            series_name: Some(series_title),
            overview,
            air_date,
            still_path,
            poster_path: None,
            playback_position_ticks: pos_ticks,
            duration_ticks: dur_ticks,
            runtime_seconds: runtime,
            is_played,
            is_favorite,
            play_count: user_data.play_count,
            last_played_date: user_data.last_played_date,
            media_sources: Some(json!(sources)),
            series_primary_image_tag: false,
        }));
    }

    // 基础分类 / 智能视图根条目详情
    if crate::web::emby_api::views::base_library_category(item_id).is_some()
        || item_id.starts_with("lib_vlib_")
    {
        return get_view_detail(state, user_id, &cfg.server_id, item_id).await;
    }

    Err((StatusCode::NOT_FOUND, "未知资源标识".to_string()))
}

/// `season_{wid}_{snum}` 详情（季号 0 = 特别篇）
async fn get_season_detail(
    state: &Arc<AppState>,
    user_id: &str,
    wid: i64,
    snum: i32,
) -> Result<Value, (StatusCode, String)> {
    let cfg = state.config.load();
    let (media_type, tmdb_id, _) = ensure_media_accessible(state, user_id, wid).await?;
    let histories = crate::web::emby_api::sessions::load_media_episode_user_data(
        state,
        user_id,
        wid,
        tmdb_id,
        &media_type,
    )
    .await?;
    let season_histories: Vec<_> = histories
        .iter()
        .filter(|((season, _), _)| *season == snum)
        .map(|(_, value)| value)
        .collect();
    let present: Vec<i32> = sqlx::query_scalar("SELECT DISTINCT sf.episode_number FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id WHERE f.work_id=$1 AND f.is_ignored=FALSE AND sf.season_number=$2")
        .bind(wid).bind(snum).fetch_all(&state.db).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let is_fav: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM user_favorites WHERE user_id=$1 AND (work_id=$2 OR (tmdb_id IS NOT NULL AND tmdb_id=$3 AND media_type=$4)))")
        .bind(user_id).bind(wid).bind(tmdb_id).bind(&media_type).fetch_one(&state.db).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let played = !present.is_empty()
        && present.iter().all(|episode| {
            histories
                .get(&(snum, *episode))
                .is_some_and(|history| history.is_played)
        });
    let user_data = json!({
        "PlaybackPositionTicks": season_histories.iter().map(|h| h.position_ticks).max().unwrap_or(0),
        "PlayCount": season_histories.iter().map(|h| h.play_count).max().unwrap_or(0),
        "Played": played,
        "IsFavorite": is_fav,
        "LastPlayedDate": season_histories.iter().filter_map(|h| h.last_played_date).max()
    });
    state.spawn_prewarm_media_dirs(wid, None);

    let row: Option<SeasonMetadata> = sqlx::query_as(
        r#"
            SELECT ws.name, ws.overview, ws.poster_path, ws.air_date, w.title
            FROM media_works w
            LEFT JOIN work_seasons ws
              ON ws.work_id = w.id AND ws.season_number = $2
            WHERE w.id = $1
            "#,
    )
    .bind(wid)
    .bind(snum)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some((name, overview, poster, air_date, series_title)) = row else {
        return Err((StatusCode::NOT_FOUND, "条目不存在".to_string()));
    };

    let child_count = present.len() as i64;

    let season_name = name.unwrap_or_else(|| {
        if snum == 0 {
            "特别篇".to_string()
        } else {
            format!("第 {} 季", snum)
        }
    });

    let obj = json!({
        "Id": format!("season_{}_{}", wid, snum),        "Name": season_name,
        "SortName": season_name,
        "ServerId": cfg.server_id,
        "SeriesId": format!("work_{}", wid),
        "SeriesName": series_title,
        "Type": "Season",
        "IsFolder": true,
        "IndexNumber": snum,
        "Overview": overview,
        "PremiereDate": air_date,
        "ChildCount": child_count,
        "UserData": user_data,
        "ImageTags": if poster.is_some() { json!({"Primary": crate::web::emby_api::images::image_path_tag(poster.as_deref())}) } else { json!({}) }
    });
    Ok(obj)
}

fn ensure_base_library_access(
    category: &str,
    allowed_categories: Option<&[String]>,
) -> Result<(), (StatusCode, String)> {
    if allowed_categories.is_some_and(|categories| {
        !categories
            .iter()
            .any(|allowed| allowed == "*" || allowed == category)
    }) {
        return Err((StatusCode::FORBIDDEN, "没有该媒体库的访问权限".to_string()));
    }
    Ok(())
}

/// `lib_*` / `lib_vlib_*` 根条目详情（CollectionFolder）
async fn get_view_detail(
    state: &AppState,
    user_id: &str,
    server_id: &str,
    item_id: &str,
) -> Result<Value, (StatusCode, String)> {
    if let Some(cat) = crate::web::emby_api::views::base_library_category(item_id) {
        let allowed_categories = get_user_allowed_categories(state, user_id).await?;
        ensure_base_library_access(cat, allowed_categories.as_deref())?;
        let collection_type = match item_id {
            "lib_movie" | "lib_doc" => "movies",
            _ => "tvshows",
        };
        return Ok(json!({
            "Id": item_id,
            "Name": cat,
            "ServerId": server_id,
            "Type": "CollectionFolder",
            "CollectionType": collection_type,
            "IsFolder": true
        }));
    }

    let vid = item_id
        .strip_prefix("lib_vlib_")
        .unwrap_or(item_id)
        .parse::<i32>()
        .map_err(|_| (StatusCode::BAD_REQUEST, "无效的智能视图 ID".to_string()))?;
    let row: Option<(String, String, Value)> =
        sqlx::query_as("SELECT name, icon_type, filter_rules FROM virtual_libraries WHERE id = $1 AND is_enabled = TRUE")
            .bind(vid)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some((name, icon_type, rules)) = row else {
        return Err((StatusCode::NOT_FOUND, "智能视图不存在或已停用".to_string()));
    };
    let allowed_cats = get_user_allowed_categories(state, user_id).await?;
    if !can_access_view(allowed_cats.as_deref(), item_id, &name) {
        return Err((
            StatusCode::FORBIDDEN,
            "没有该智能视图的访问权限".to_string(),
        ));
    }
    let rule: VirtualLibraryRule = serde_json::from_value(rules).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("智能视图规则损坏：{}", e),
        )
    })?;
    rule.validate()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let collection_type = if icon_type == "movies" || icon_type == "movie" {
        "movies"
    } else {
        "tvshows"
    };
    Ok(json!({
        "Id": item_id,
        "Name": name,
        "ServerId": server_id,
        "Type": "CollectionFolder",
        "CollectionType": collection_type,
        "IsFolder": true
    }))
}

#[cfg(test)]
mod access_tests {
    use super::{ensure_base_library_access, StatusCode};

    #[test]
    fn forbidden_for_base_library_outside_allowed_categories() {
        let allowed = vec!["电视剧".to_string()];
        let error = ensure_base_library_access("电影", Some(&allowed)).unwrap_err();
        assert_eq!(error.0, StatusCode::FORBIDDEN);
    }

    #[test]
    fn work_detail_query_aliases_jsonb_season_and_episode_counts() {
        let source = include_str!("detail.rs");
        assert!(source.contains("AS num_seasons"));
        assert!(source.contains("AS num_episodes"));
    }
}

#[derive(sqlx::FromRow)]
struct WorkDetailRow {
    media_type: String,
    tmdb_id: Option<i32>,
    title: String,
    year: Option<i32>,
    overview: Option<String>,
    rating: f32,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    num_seasons: Option<i32>,
    num_episodes: Option<i32>,
    original_title: Option<String>,
    sort_title: Option<String>,
}

#[derive(sqlx::FromRow)]
pub(in crate::web::emby_api) struct SeasonListRow {
    pub(super) total_count: i64,
    pub(super) season_number: i32,
    pub(super) name: Option<String>,
    pub(super) overview: Option<String>,
    pub(super) poster_path: Option<String>,
    pub(super) air_date: Option<String>,
}
