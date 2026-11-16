use super::sql::*;
use super::*;
use serde_json::json;
pub async fn get_user_items(
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
    Ok(Json(get_user_items_inner(&state, &user_id, &q).await?))
}

/// 用户视图根条目（`GET /Users/:user_id/Items/Root`）：
/// 部分客户端（VidHub / Yamby 等）展开媒体库前会先请求 Root 文件夹
pub async fn get_root_folder(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path_user_id): Path<String>,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let _user_id = resolve_valid_user_id_with_query(
        &state,
        Some(&path_user_id),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    let cfg = state.config.load();
    Ok(Json(json!({
        "Id": "root",
        "Name": "Root",
        "ServerId": cfg.server_id,
        "Type": "Folder",
        "IsFolder": true,
        "UserData": {
            "PlaybackPositionTicks": 0,
            "PlayCount": 0,
            "IsFavorite": false,
            "Played": false
        }
    })))
}

/// 非 user-scoped 别名 `GET /Items`：从令牌 / UserId / api_key 解析用户
pub async fn get_items_unscoped(
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
    Ok(Json(get_user_items_inner(&state, &user_id, &q).await?))
}

pub(in crate::web::emby_api) async fn get_user_items_inner(
    state: &AppState,
    user_id: &str,
    q: &ItemsQuery,
) -> Result<Value, (StatusCode, String)> {
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
        return get_resume_items_inner(state, user_id, limit, q).await;
    }

    // 2. 解析 IncludeItemTypes 参数（如 Series, Movie, Episode, Season, Video）
    let type_set: HashSet<&str> = q
        .include_item_types
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    let has_media_type = type_set
        .iter()
        .any(|t| matches!(*t, "Series" | "Movie" | "Episode" | "Season" | "Video"));

    // 若客户端仅请求 Playlist / BoxSet / MusicAlbum 等不支持的集合类型，直接返回空集
    if !type_set.is_empty() && !has_media_type {
        return Ok(json!({ "Items": [], "TotalRecordCount": 0 }));
    }
    // 收藏的 Episode/Season/Video 按整部影视维度收藏，直接返回空列表防止把剧集主条目混入
    if favorite_filter_has_no_supported_media_type(filter_set.contains("IsFavorite"), &type_set) {
        return Ok(json!({ "Items": [], "TotalRecordCount": 0 }));
    }

    // 3. 展开某部剧/某季的子列表（ParentId = work_123 / media_123 / season_123_2）
    if let Some(ref pid) = q.parent_id {
        let work_parent = pid
            .strip_prefix("work_")
            .or_else(|| pid.strip_prefix("media_"))
            .and_then(parse_work_id);
        if let Some(mid) = work_parent {
            ensure_media_accessible(state, user_id, mid).await?;
            let search_term = q.search_term.as_deref();
            if type_set.contains("Season")
                && !type_set.contains("Episode")
                && !type_set.contains("Video")
            {
                return get_seasons_for_media(state, user_id, mid, search_term).await;
            }
            return get_episodes_for_media(state, user_id, mid, None, search_term).await;
        }
        if let Some((wid, snum)) = parse_season_id(pid) {
            ensure_media_accessible(state, user_id, wid).await?;
            return get_episodes_for_media(
                state,
                user_id,
                wid,
                Some(snum),
                q.search_term.as_deref(),
            )
            .await;
        }
    }

    // 视图授权独立于基础分类；获得某个视图权限即可访问其匹配内容。
    let access_scope = crate::web::emby_api::views::user_access_scope(state, user_id).await?;
    let allowed_user_cats = access_scope.allowed_categories.as_deref();
    let mut vlib_rule: Option<VirtualLibraryRule> = None;
    if let Some(pid) = q
        .parent_id
        .as_deref()
        .filter(|p| p.starts_with("lib_vlib_"))
    {
        let vid = pid
            .trim_start_matches("lib_vlib_")
            .parse::<i32>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的智能视图 ID".into()))?;
        let row: Option<(String, Value)> = sqlx::query_as(
            "SELECT name, filter_rules FROM virtual_libraries WHERE id=$1 AND is_enabled=TRUE",
        )
        .bind(vid)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let Some((name, rules)) = row else {
            return Err((StatusCode::NOT_FOUND, "智能视图不存在或已停用".into()));
        };
        if !can_access_view(allowed_user_cats, pid, &name) {
            return Err((StatusCode::FORBIDDEN, "没有该智能视图的访问权限".into()));
        }
        let rule: VirtualLibraryRule = serde_json::from_value(rules).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("智能视图规则损坏：{}", e),
            )
        })?;
        rule.validate()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        vlib_rule = Some(rule);
    }
    if access_scope.has_no_access() {
        return Ok(json!({ "Items": [], "TotalRecordCount": 0 }));
    }

    let rule_json = vlib_rule
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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

    // 全局单集列表（IncludeItemTypes=Episode 无剧集父级）：真正查询 sync_files + work_episodes
    if type_set.contains("Episode") && !type_set.contains("Series") && !type_set.contains("Movie") {
        return get_episodes_listing(
            state,
            user_id,
            q,
            EpisodeListingOptions {
                category_filter: category_filter.map(str::to_string),
                access_scope: access_scope.clone(),
                vlib_rule: vlib_rule.clone(),
                limit,
                offset,
            },
        )
        .await;
    }

    let only_favorite = filter_set.contains("IsFavorite");
    let only_played = filter_set.contains("IsPlayed");
    let only_unplayed = filter_set.contains("IsUnplayed");

    let search_term = q.search_term.as_deref();
    let search_pattern = build_search_pattern(search_term);
    let search_tmdb = numeric_tmdb_id(search_term);

    // 解析指定 Ids 批量查询：空 Ids= 视为未过滤；支持 work_* / media_* / season_* / ep_* / 纯数字
    let explicit_ids = parse_explicit_work_ids(q.ids.as_deref());

    // 4. 组装排序规则（完整支持逗号分隔多字段 SortBy 与 SortOrder）
    let sort_key = q
        .sort_by
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| vlib_rule.as_ref().and_then(|r| r.sort_by.as_deref()))
        .unwrap_or("");
    let sort_by_popularity = sort_key
        .split(',')
        .any(|f| f.trim().eq_ignore_ascii_case("Popularity"));

    let filters = MediaListFilters {
        user_id,
        access_scope: access_scope.as_ref(),
        category_filter,
        media_type_filter,
        search_pattern: search_pattern.as_deref(),
        search_tmdb,
        explicit_ids: explicit_ids.as_deref(),
        only_favorite,
        only_played,
        only_unplayed,
        rule_json: rule_json.as_ref(),
        rule: vlib_rule.as_ref(),
    };

    // 仅按 Popularity 排序时才解压 tmdb_metadata（避免每行 TOAST 解压热度字段）
    let popularity_select = if sort_by_popularity {
        ", (m.tmdb_metadata->>'popularity')::float8 AS popularity"
    } else {
        ""
    };

    let mut sql = DynSql::new("");
    sql.push(&format!(
        r#"SELECT m.id, m.media_type, m.tmdb_id, m.title, m.original_title, m.year, m.premiered_at, m.overview,
                  m.rating, m.poster_path, m.backdrop_path, m.created_at,
                  CASE WHEN m.media_type = 'tv' THEN (m.tmdb_metadata->>'number_of_seasons')::int END AS num_seasons,
                  CASE WHEN m.media_type = 'tv' THEN (m.tmdb_metadata->>'number_of_episodes')::int END AS num_episodes{popularity_select}
           FROM media_works m
           WHERE "#,
        popularity_select = popularity_select
    ));
    push_media_filters(&mut sql, &filters)?;
    sql.push(" ORDER BY ");

    // DatePlayed / PlayCount 排序子查询需要 user_id：预占下一个参数编号
    let user_param = sql.next_param();
    let (order_clause, user_uses) =
        build_emby_order_clause(sort_key, q.sort_order.as_deref(), Some(&user_param));
    sql.push(&order_clause);
    if user_uses > 0 {
        let _ = sql.bind(user_id.to_string())?;
    }

    sql.push(" LIMIT ");
    let _ = sql.bind(limit)?;
    sql.push(" OFFSET ");
    let _ = sql.bind(offset)?;

    let rows: Vec<MediaListRow> =
        sqlx::query_as_with::<Postgres, MediaListRow, _>(&sql.sql, sql.args)
            .fetch_all(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let total = if !rows.is_empty() && (rows.len() as i64) < limit {
        offset + rows.len() as i64
    } else {
        let mut count_sql = DynSql::new("SELECT COUNT(*) FROM media_works m WHERE ");
        push_media_filters(&mut count_sql, &filters)?;
        sqlx::query_scalar_with::<Postgres, i64, _>(&count_sql.sql, count_sql.args)
            .fetch_one(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };

    // 批量加载当前用户的收藏集合与播放历史，填充 UserData
    let page_mids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let page_tids: Vec<i32> = rows.iter().filter_map(|r| r.tmdb_id).collect();
    let fav_rows: Vec<(Option<i64>, Option<i32>, String)> = if page_mids.is_empty() {
        vec![]
    } else {
        sqlx::query_as(
            "SELECT work_id, tmdb_id, media_type FROM user_favorites WHERE user_id = $1 AND (work_id = ANY($2) OR (tmdb_id IS NOT NULL AND tmdb_id = ANY($3)))",
        )
        .bind(user_id)
        .bind(&page_mids)
        .bind(&page_tids)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };
    let fav_set: (HashSet<i64>, HashSet<(i32, String)>) = (
        fav_rows.iter().filter_map(|(wid, _, _)| *wid).collect(),
        fav_rows
            .iter()
            .filter_map(|(_, tid, mt)| tid.map(|id| (id, mt.clone())))
            .collect(),
    );
    let hist_rows: Vec<PageHistoryRow> = if page_mids.is_empty() {
        vec![]
    } else {
        match sqlx::query_as(
            r#"
            SELECT work_id, tmdb_id, media_type, season_number, episode_number, position_ticks, is_played
            FROM play_histories
            WHERE user_id = $1
              AND (work_id = ANY($2) OR (tmdb_id IS NOT NULL AND tmdb_id = ANY($3)))
            "#,
        )
        .bind(user_id)
        .bind(&page_mids)
        .bind(&page_tids)
        .fetch_all(&state.db)
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!("批量查询页面播放历史失败: {}", e);
                vec![]
            }
        }
    };

    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let emby_type = if r.media_type == "movie" {
                "Movie"
            } else {
                "Series"
            };
            let is_folder = r.media_type != "movie";
            let mut image_tags = json!({});
            if r.poster_path.is_some() {
                image_tags["Primary"] = json!(crate::web::emby_api::images::image_path_tag(
                    r.poster_path.as_deref()
                ));
            }

            let is_fav =
                only_favorite || is_media_favorited(&fav_set, r.id, r.tmdb_id, &r.media_type);
            let (pos_ticks, is_played) = if r.media_type == "movie" {
                hist_rows
                    .iter()
                    .find(|h| {
                        h.season_number == 0
                            && h.episode_number == 1
                            && ((r.tmdb_id.is_some()
                                && h.tmdb_id == r.tmdb_id
                                && h.media_type == "movie")
                                || h.work_id == Some(r.id))
                    })
                    .map(|h| (h.position_ticks, h.is_played))
                    .unwrap_or((0, false))
            } else {
                let any_played = hist_rows.iter().any(|h| {
                    h.is_played
                        && ((r.tmdb_id.is_some() && h.tmdb_id == r.tmdb_id && h.media_type == "tv")
                            || h.work_id == Some(r.id))
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
            if r.backdrop_path.is_some() {
                obj["BackdropImageTags"] = json!([crate::web::emby_api::images::image_path_tag(
                    r.backdrop_path.as_deref()
                )]);
            }
            if let Some(tid) = r.tmdb_id {
                obj["ProviderIds"] = json!({ "Tmdb": tid.to_string() });
            }
            if r.media_type == "tv" {
                if let Some(seasons) = r.num_seasons {
                    obj["ChildCount"] = json!(seasons);
                }
                if let Some(episodes) = r.num_episodes {
                    obj["RecursiveItemCount"] = json!(episodes);
                }
            } else {
                obj["RecursiveItemCount"] = json!(1);
            }
            obj
        })
        .collect();

    Ok(json!({
        "Items": items,
        "StartIndex": offset,
        "TotalRecordCount": total
    }))
}

/// 收藏过滤仅在客户端明确指定了不支持的非空类型集合时返回空列表。
fn favorite_filter_has_no_supported_media_type(is_favorite: bool, types: &HashSet<&str>) -> bool {
    is_favorite && !types.is_empty() && !types.contains("Series") && !types.contains("Movie")
}

/// 解析 `Ids=` 参数为 work 主键集合：空串视为未过滤；`season_*` / `ep_*` 归并为所属作品 ID
fn parse_explicit_work_ids(raw: Option<&str>) -> Option<Vec<i64>> {
    let raw = raw.map(str::trim).filter(|s| !s.is_empty())?;
    let mut ids = Vec::new();
    for token in raw.split(',') {
        let t = token.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(wid) = parse_work_id(t) {
            ids.push(wid);
            continue;
        }
        if let Some((wid, _)) = parse_season_id(t) {
            ids.push(wid);
            continue;
        }
        if let Some((wid, _, _)) = parse_episode_id(t) {
            ids.push(wid);
        }
        // 无法识别的 ID 忽略；若全部无法识别则匹配空集
    }
    Some(ids)
}

#[derive(sqlx::FromRow)]
struct PageHistoryRow {
    work_id: Option<i64>,
    tmdb_id: Option<i32>,
    media_type: String,
    season_number: i32,
    episode_number: i32,
    position_ticks: i64,
    is_played: bool,
}

/// 解析 Emby 客户端逗号分隔的 `SortBy` 与 `SortOrder` 参数，生成稳定的 PostgreSQL ORDER BY 子句。
/// `user_param` 为 user_id 绑定参数占位符；返回值附带该占位符出现的次数（调用方按此数量绑定 user_id）。
fn build_emby_order_clause(
    sort_by: &str,
    sort_order: Option<&str>,
    user_param: Option<&str>,
) -> (String, usize) {
    let orders: Vec<bool> = sort_order
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.eq_ignore_ascii_case("Ascending") || s.eq_ignore_ascii_case("asc"))
        .collect();

    let uid = user_param.unwrap_or("$1");
    let mut user_uses = 0usize;
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
            "DatePlayed" => {
                user_uses += 1;
                format!(
                    "(SELECT MAX(ph.last_played_at) FROM play_histories ph WHERE ph.user_id = {uid} AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = m.tmdb_id AND ph.media_type = m.media_type) OR ph.work_id = m.id)) {} NULLS LAST",
                    dir
                )
            }
            "PlayCount" => {
                user_uses += 1;
                format!(
                    "(SELECT COUNT(*) FROM play_histories ph WHERE ph.user_id = {uid} AND ph.is_played = TRUE AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = m.tmdb_id AND ph.media_type = m.media_type) OR ph.work_id = m.id)) {}",
                    dir
                )
            }
            "Random" => "RANDOM()".to_string(),
            _ => continue,
        };
        clauses.push(expr);
    }

    if clauses.is_empty() {
        let fallback_asc = orders.first().copied().unwrap_or(false);
        let clause = if fallback_asc {
            "year ASC NULLS LAST, id ASC".to_string()
        } else {
            "year DESC NULLS LAST, id DESC".to_string()
        };
        return (clause, user_uses);
    }

    let tie_dir = if first_is_asc.unwrap_or(false) {
        "ASC"
    } else {
        "DESC"
    };
    clauses.push(format!("id {}", tie_dir));
    (clauses.join(", "), user_uses)
}

#[cfg(test)]
mod tests {
    use super::{
        build_emby_order_clause, favorite_filter_has_no_supported_media_type,
        parse_explicit_work_ids, DynSql,
    };
    use std::collections::HashSet;

    #[test]
    fn favorite_filter_only_short_circuits_when_types_are_explicitly_unsupported() {
        let empty = HashSet::new();
        let episode = HashSet::from(["Episode"]);
        let movie = HashSet::from(["Movie"]);
        assert!(!favorite_filter_has_no_supported_media_type(true, &empty));
        assert!(favorite_filter_has_no_supported_media_type(true, &episode));
        assert!(!favorite_filter_has_no_supported_media_type(true, &movie));
        assert!(!favorite_filter_has_no_supported_media_type(
            false, &episode
        ));
    }

    #[test]
    fn main_media_list_uses_flat_top_n_query() {
        let source = include_str!("list.rs");
        let select = source.find("SELECT m.id, m.media_type").unwrap();
        let from = source[select..].find("FROM media_works m").unwrap() + select;
        let limit = source[from..].find("sql.push(\" LIMIT \")").unwrap() + from;
        let query_shape = &source[select..limit];
        assert!(!query_shape.contains("FROM ("));
        assert!(query_shape.contains("ORDER BY"));
        assert!(source[limit..].contains("sql.push(\" OFFSET \")"));
    }

    #[test]
    fn test_emby_sort_priority_with_multi_fields() {
        let (clause, uses) =
            build_emby_order_clause("PremiereDate,SortName", Some("Descending"), Some("$9"));
        assert_eq!(
            clause,
            "COALESCE(NULLIF(premiered_at, ''), CASE WHEN year IS NOT NULL THEN year::text || '-01-01' ELSE NULL END) DESC NULLS LAST, title DESC, id DESC"
        );
        assert_eq!(uses, 0);

        let (clause, uses) =
            build_emby_order_clause("Popularity,SortName", Some("Descending"), None);
        assert_eq!(clause, "popularity DESC NULLS LAST, title DESC, id DESC");
        assert_eq!(uses, 0);

        let (clause, _) = build_emby_order_clause(
            "CommunityRating,DateCreated",
            Some("Descending,Ascending"),
            None,
        );
        assert_eq!(clause, "rating DESC, created_at ASC, id DESC");

        let (clause, _) = build_emby_order_clause("IsFolder,SortName", None, None);
        assert_eq!(clause, "title ASC, id ASC");

        let (clause, _) = build_emby_order_clause("DateLastContentAdded", Some("Descending"), None);
        assert_eq!(clause, "updated_at DESC, id DESC");

        let (clause, _) = build_emby_order_clause("Random", None, None);
        assert_eq!(clause, "RANDOM(), id DESC");

        let (clause, uses) = build_emby_order_clause("DatePlayed,PlayCount", None, Some("$7"));
        assert!(clause.contains("ph.user_id = $7"));
        assert_eq!(uses, 2);
    }

    #[test]
    fn date_played_and_play_count_share_one_user_bind() {
        let mut sql = DynSql::new("SELECT m.id FROM media_works m ORDER BY ");
        let user_param = sql.next_param();
        let (order, user_uses) =
            build_emby_order_clause("DatePlayed,PlayCount", None, Some(&user_param));
        sql.push(&order);
        if user_uses > 0 {
            sql.bind("user-a".to_string()).unwrap();
        }
        sql.push(" LIMIT ");
        sql.bind(20_i64).unwrap();
        assert_eq!(user_uses, 2);
        assert_eq!(sql.n, 2);
        assert_eq!(sql.sql.matches("ph.user_id = $1").count(), 2);
    }

    #[test]
    fn test_parse_explicit_work_ids_supports_typed_ids() {
        // 空 Ids= 不应匹配空集
        assert_eq!(parse_explicit_work_ids(Some("")), None);
        assert_eq!(parse_explicit_work_ids(Some("   ")), None);
        assert_eq!(parse_explicit_work_ids(None), None);

        // 各类型 ID 均归并为 work 主键
        assert_eq!(
            parse_explicit_work_ids(Some("work_10,media_20,30,season_40_2,ep_50_1_3")),
            Some(vec![10, 20, 30, 40, 50])
        );
        // 全部无法识别 -> 空集（Ids 显式指定但无有效条目）
        assert_eq!(
            parse_explicit_work_ids(Some("lib_movie,lib_vlib_9")),
            Some(vec![])
        );
    }
}
