use super::auth::{resolve_valid_user_id, resolve_valid_user_id_with_query};
use super::ids::{build_search_pattern, numeric_tmdb_id, parse_episode_id, parse_season_id, parse_work_id};
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
use sqlx::{Encode, Postgres, QueryBuilder};
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
    #[serde(rename = "api_key", alias = "ApiKey", alias = "X-Emby-Token")]
    pub(super) api_key: Option<String>,
    #[serde(rename = "StartIndex", alias = "startIndex", default)]
    pub(super) start_index: i64,
    #[serde(rename = "Limit", alias = "limit")]
    pub(super) limit: Option<i64>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
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
    total_count: Option<i64>,
}

/// 动态 SQL 构建器：只拼接实际需要的谓词，绑定值与 `$N` 编号严格一一对应，
/// 避免 `($1 IS NULL OR ...)` 泛化谓词阻断 PostgreSQL 最优查询计划
struct DynSql {
    qb: QueryBuilder<'static, Postgres>,
    n: usize,
}

impl DynSql {
    fn new(initial: &str) -> Self {
        Self {
            qb: QueryBuilder::new(initial),
            n: 0,
        }
    }

    fn push(&mut self, sql: &str) {
        self.qb.push(sql);
    }

    /// 下一个绑定值将获得的占位符编号（供 SQL 文本提前引用）
    fn next_param(&self) -> String {
        format!("${}", self.n + 1)
    }

    /// 绑定一个值并返回其 `$N` 占位符；可在谓词文本中重复引用同一占位符
    fn bind<T>(&mut self, value: T) -> String
    where
        T: Encode<'static, Postgres> + sqlx::Type<Postgres> + 'static,
    {
        self.n += 1;
        self.qb.push_bind(value);
        format!("${}", self.n)
    }

    /// 常用包装：推入前缀 SQL 文本、绑定值、后缀 SQL 文本
    fn push_bind_push<T>(&mut self, prefix: &str, value: T, suffix: &str) -> String
    where
        T: Encode<'static, Postgres> + sqlx::Type<Postgres> + 'static,
    {
        self.push(prefix);
        let p = self.bind(value);
        self.push(suffix);
        p
    }
}

/// get_user_items 主列表的过滤条件集合（主查询与计数兜底查询共用）
struct MediaListFilters {
    user_id: String,
    category_filter: Option<String>,
    media_type_filter: Option<String>,
    search_pattern: Option<String>,
    search_tmdb: Option<i32>,
    explicit_ids: Option<Vec<i64>>,
    only_favorite: bool,
    only_played: bool,
    only_unplayed: bool,
    allowed_cats: Option<Vec<String>>,
    rule_json: Option<Value>,
    rule: Option<VirtualLibraryRule>,
}

/// 把过滤条件以编号绑定参数形式写入 SQL 构建器（作用于 `media_works m` 别名）
fn push_media_filters(sql: &mut DynSql, f: &MediaListFilters) {
    sql.push("EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE)");

    if let Some(cat) = &f.category_filter {
        sql.push_bind_push(" AND m.category = ", cat.clone(), "");
    }

    if let Some(p) = &f.search_pattern {
        // 模式只绑定一次，多处占位符复用同一 $N
        let pat = sql.bind(p.clone());
        let mut pred = format!(
            " AND (m.title ILIKE {pat} OR m.original_title ILIKE {pat} OR m.sort_title ILIKE {pat}"
        );
        pred.push_str(&format!(
            " OR EXISTS (SELECT 1 FROM sync_folders sf2 WHERE sf2.work_id = m.id AND (sf2.clean_title ILIKE {pat} OR sf2.raw_folder_name ILIKE {pat}))"
        ));
        if let Some(tid) = f.search_tmdb {
            let tid_param = sql.bind(tid);
            pred.push_str(&format!(" OR m.tmdb_id = {}", tid_param));
        }
        pred.push(')');
        sql.push(&pred);
    }

    if let Some(mt) = &f.media_type_filter {
        sql.push_bind_push(" AND m.media_type = ", mt.clone(), "");
    }

    if let Some(ids) = &f.explicit_ids {
        sql.push_bind_push(" AND m.id = ANY(", ids.clone(), ")");
    }

    // user_id 只绑定一次，收藏/已看/未看三个 EXISTS 复用同一占位符
    let uid = sql.bind(f.user_id.clone());
    if f.only_favorite {
        sql.push(&format!(
            " AND EXISTS (SELECT 1 FROM user_favorites uf WHERE uf.user_id = {uid} AND ((uf.tmdb_id IS NOT NULL AND uf.tmdb_id = m.tmdb_id AND uf.media_type = m.media_type) OR uf.work_id = m.id))"
        ));
    }
    if f.only_played {
        sql.push(&format!(
            " AND EXISTS (SELECT 1 FROM play_histories ph WHERE ph.user_id = {uid} AND ph.is_played = TRUE AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = m.tmdb_id AND ph.media_type = m.media_type) OR ph.work_id = m.id))"
        ));
    }
    if f.only_unplayed {
        sql.push(&format!(
            " AND NOT EXISTS (SELECT 1 FROM play_histories ph WHERE ph.user_id = {uid} AND ph.is_played = TRUE AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = m.tmdb_id AND ph.media_type = m.media_type) OR ph.work_id = m.id))"
        ));
    }

    if let Some(cats) = &f.allowed_cats {
        sql.push_bind_push(" AND m.category = ANY(", cats.clone(), ")");
    }

    if let Some(rule) = &f.rule {
        let rj = sql.bind(f.rule_json.clone().unwrap_or(Value::Null));
        sql.push(&format!(" AND {}", rule.sql_predicate(&format!("{}::jsonb", rj))));
    }
}

pub(super) async fn get_user_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path_user_id): Path<String>,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id =
        resolve_valid_user_id_with_query(&state, Some(&path_user_id), &headers, q.api_key.as_deref())
            .await?;
    Ok(Json(get_user_items_inner(&state, &user_id, &q).await?))
}

/// 用户视图根条目（`GET /Users/:user_id/Items/Root`）：
/// 部分客户端（VidHub / Yamby 等）展开媒体库前会先请求 Root 文件夹
pub(super) async fn get_root_folder(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path_user_id): Path<String>,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let _user_id =
        resolve_valid_user_id_with_query(&state, Some(&path_user_id), &headers, q.api_key.as_deref())
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
pub(super) async fn get_items_unscoped(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id_with_query(&state, q.user_id.as_deref(), &headers, q.api_key.as_deref())
        .await?;
    Ok(Json(get_user_items_inner(&state, &user_id, &q).await?))
}

async fn get_user_items_inner(
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
        return get_resume_items_inner(state, user_id, limit).await;
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
    if favorite_filter_has_no_supported_media_type(
        filter_set.contains("IsFavorite"),
        &type_set,
    ) {
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
            if type_set.contains("Season") && !type_set.contains("Episode") && !type_set.contains("Video") {
                return get_seasons_for_media(state, user_id, mid, search_term).await;
            }
            return get_episodes_for_media(state, user_id, mid, None, search_term).await;
        }
        if let Some((wid, snum)) = parse_season_id(pid) {
            ensure_media_accessible(state, user_id, wid).await?;
            return get_episodes_for_media(state, user_id, wid, Some(snum), q.search_term.as_deref()).await;
        }
    }

    // 视图授权独立于基础分类；获得某个视图权限即可访问其匹配内容。
    let mut allowed_user_cats = get_user_allowed_categories(state, user_id).await;
    let mut vlib_rule: Option<VirtualLibraryRule> = None;
    if let Some(pid) = q.parent_id.as_deref().filter(|p| p.starts_with("lib_vlib_")) {
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
        if !can_access_view(allowed_user_cats.as_deref(), pid, &name) {
            return Err((StatusCode::FORBIDDEN, "没有该智能视图的访问权限".into()));
        }
        let rule: VirtualLibraryRule = serde_json::from_value(rules)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("智能视图规则损坏：{}", e)))?;
        rule.validate().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        vlib_rule = Some(rule);
        allowed_user_cats = None;
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
            category_filter.map(str::to_string),
            allowed_user_cats.clone(),
            vlib_rule.clone(),
            limit,
            offset,
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
        user_id: user_id.to_string(),
        category_filter: category_filter.map(str::to_string),
        media_type_filter: media_type_filter.map(str::to_string),
        search_pattern: search_pattern.clone(),
        search_tmdb,
        explicit_ids: explicit_ids.clone(),
        only_favorite,
        only_played,
        only_unplayed,
        allowed_cats: allowed_user_cats.clone(),
        rule_json: rule_json.clone(),
        rule: vlib_rule.clone(),
    };

    // 仅按 Popularity 排序时才解压 tmdb_metadata（避免每行 TOAST 解压热度字段）
    let popularity_select = if sort_by_popularity {
        ", (m.tmdb_metadata->>'popularity')::float8 AS popularity"
    } else {
        ""
    };

    let mut sql = DynSql::new("");
    sql.push(&format!(
        r#"SELECT id, media_type, tmdb_id, title, original_title, year, premiered_at, overview,
                  rating, poster_path, backdrop_path, created_at, total_count
           FROM (
               SELECT m.id, m.media_type, m.tmdb_id, m.title, m.original_title, m.year, m.premiered_at,
                      m.overview, m.rating, m.poster_path, m.backdrop_path, m.created_at, m.updated_at,
                      COUNT(*) OVER() AS total_count{popularity_select}
               FROM media_works m
               WHERE "#,
        popularity_select = popularity_select
    ));
    push_media_filters(&mut sql, &filters);
    sql.push(") AS m ORDER BY ");

    // DatePlayed / PlayCount 排序子查询需要 user_id：预占下一个参数编号
    let user_param = sql.next_param();
    let (order_clause, user_uses) = build_emby_order_clause(sort_key, q.sort_order.as_deref(), Some(&user_param));
    sql.push(&order_clause);
    for _ in 0..user_uses {
        let _ = sql.bind(user_id.to_string());
    }

    sql.push(" LIMIT ");
    let _ = sql.bind(limit);
    sql.push(" OFFSET ");
    let _ = sql.bind(offset);

    let rows: Vec<MediaListRow> = sql
        .qb
        .build_query_as()
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // 空页 + StartIndex > 0 边界：窗口计数不会出现在空结果集中，退回一次精确计数查询
    if rows.is_empty() && offset > 0 {
        let mut count_sql = DynSql::new("SELECT COUNT(*) FROM media_works m WHERE ");
        push_media_filters(&mut count_sql, &filters);
        let total: i64 = count_sql
            .qb
            .build_query_scalar()
            .fetch_one(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        return Ok(json!({
            "Items": [],
            "StartIndex": offset,
            "TotalRecordCount": total
        }));
    }

    let total = rows.first().and_then(|r| r.total_count).unwrap_or(0);

    // 批量加载当前用户的收藏集合与播放历史，填充 UserData
    let fav_set = load_user_favorite_keys(state, user_id).await;
    let page_mids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let page_tids: Vec<i32> = rows.iter().filter_map(|r| r.tmdb_id).collect();
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

    // 分页后的补充元数据查询：ChildCount / RecursiveItemCount 仅对本页 ≤200 行解压 JSON，
    // 主扫描保持零 TOAST 解压
    let meta_map: std::collections::HashMap<i64, (i32, i32)> = if page_mids.is_empty() {
        std::collections::HashMap::new()
    } else {
        match sqlx::query_as::<_, (i64, Option<i32>, Option<i32>)>(
            "SELECT id, (tmdb_metadata->>'number_of_seasons')::int, (tmdb_metadata->>'number_of_episodes')::int FROM media_works WHERE id = ANY($1)",
        )
        .bind(&page_mids)
        .fetch_all(&state.db)
        .await
        {
            Ok(rows) => rows
                .into_iter()
                .map(|(id, s, e)| (id, (s.unwrap_or(0), e.unwrap_or(0))))
                .collect(),
            Err(e) => {
                tracing::warn!("批量查询分页元数据失败: {}", e);
                std::collections::HashMap::new()
            }
        }
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

            let is_fav = only_favorite || is_media_favorited(&fav_set, r.id, r.tmdb_id, &r.media_type);
            let (pos_ticks, is_played) = if r.media_type == "movie" {
                hist_rows
                    .iter()
                    .find(|h| {
                        h.season_number == 0
                            && h.episode_number == 1
                            && ((r.tmdb_id.is_some() && h.tmdb_id == r.tmdb_id && h.media_type == "movie")
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
                obj["BackdropImageTags"] = json!(["v1"]);
            }
            if let Some(tid) = r.tmdb_id {
                obj["ProviderIds"] = json!({ "Tmdb": tid.to_string() });
            }
            if let Some((seasons, episodes)) = meta_map.get(&r.id) {
                if r.media_type == "tv" {
                    obj["ChildCount"] = json!(seasons);
                    obj["RecursiveItemCount"] = json!(episodes);
                } else {
                    obj["RecursiveItemCount"] = json!(1);
                }
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
fn favorite_filter_has_no_supported_media_type(
    is_favorite: bool,
    types: &HashSet<&str>,
) -> bool {
    is_favorite
        && !types.is_empty()
        && !types.contains("Series")
        && !types.contains("Movie")
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

/// 某部剧的季列表（供 `ParentId=work_x` + IncludeItemTypes=Season 与 `/Shows/:id/Seasons` 复用）
async fn get_seasons_for_media(
    state: &AppState,
    _user_id: &str,
    work_id: i64,
    search_term: Option<&str>,
) -> Result<Value, (StatusCode, String)> {
    let cfg = state.config.load();
    let _ = state.scraper.priority_tx.try_send(ScrapeTask::WorkSeasons(work_id));

    let series_poster: Option<String> =
        sqlx::query_scalar("SELECT poster_path FROM media_works WHERE id = $1")
            .bind(work_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .flatten();

    let search_pattern = build_search_pattern(search_term);
    let season_rows: Vec<SeasonListRow> = if let Some(ref p) = search_pattern {
        sqlx::query_as(
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
            WHERE COALESCE(ws.name, '') ILIKE $2
               OR EXISTS (
                    SELECT 1 FROM sync_files sf2
                    JOIN sync_folders f2 ON f2.id = sf2.folder_id
                    WHERE f2.work_id = $1 AND f2.is_ignored = FALSE
                      AND sf2.season_number = ep_s.season_number AND sf2.file_name ILIKE $2
               )
            ORDER BY ep_s.season_number ASC
            "#,
        )
        .bind(work_id)
        .bind(p)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        sqlx::query_as(
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
        .bind(work_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };

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
                "ImageTags": if has_poster { json!({"Primary": "v1"}) } else { json!({}) }
            });
            if has_poster {
                obj["BackdropImageTags"] = json!([]);
            }
            obj
        })
        .collect();

    Ok(json!({
        "Items": items,
        "TotalRecordCount": items.len()
    }))
}

/// 某部剧/某季的分集列表；`season_number=None` 返回全剧，`Some(n)` 仅返回该季；
/// `search_term` 按 TMDB 分集标题与本地文件名过滤
async fn get_episodes_for_media(
    state: &AppState,
    user_id: &str,
    work_id: i64,
    season_number: Option<i32>,
    search_term: Option<&str>,
) -> Result<Value, (StatusCode, String)> {
    let cfg = state.config.load();
    let _ = state.scraper.priority_tx.try_send(ScrapeTask::WorkSeasons(work_id));

    let media_meta: Option<(Option<i32>, String)> =
        match sqlx::query_as("SELECT tmdb_id, media_type FROM media_works WHERE id = $1")
            .bind(work_id)
            .fetch_optional(&state.db)
            .await
        {
            Ok(row) => row,
            Err(e) => {
                tracing::warn!("查询媒体元数据失败 (work_id={}): {}", work_id, e);
                None
            }
        };
    let (tmdb_id, _mtype) = media_meta.unwrap_or((None, "tv".to_string()));
    let history_map = load_media_episode_histories(state, user_id, work_id, tmdb_id, &_mtype).await;

    let search_pattern = build_search_pattern(search_term);

    let eps: Vec<EpisodeGroupRow> = if let Some(ref p) = search_pattern {
        let base_sql = |season_clause: &str| -> String {
            format!(
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
                  {season_clause}
                  AND (we.title ILIKE $2 OR sf.file_name ILIKE $2)
                ORDER BY sf.season_number ASC, sf.episode_number ASC, COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) DESC, sf.id ASC
                "#,
                season_clause = season_clause
            )
        };
        match season_number {
            Some(snum) => sqlx::query_as(&base_sql("AND sf.season_number = $3"))
                .bind(work_id)
                .bind(p)
                .bind(snum)
                .fetch_all(&state.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            None => sqlx::query_as(&base_sql(""))
                .bind(work_id)
                .bind(p)
                .fetch_all(&state.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        }
    } else {
        let base_sql = |season_clause: &str| -> String {
            format!(
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
                  {season_clause}
                ORDER BY sf.season_number ASC, sf.episode_number ASC, COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) DESC, sf.id ASC
                "#,
                season_clause = season_clause
            )
        };
        match season_number {
            Some(snum) => sqlx::query_as(&base_sql("AND sf.season_number = $2"))
                .bind(work_id)
                .bind(snum)
                .fetch_all(&state.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            None => sqlx::query_as(&base_sql(""))
                .bind(work_id)
                .fetch_all(&state.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        }
    };

    let items: Vec<Value> = eps
        .into_iter()
        .map(|ep| {
            episode_item_json(
                &cfg.server_id,
                work_id,
                None,
                &ep,
                history_map.get(&(ep.season_number, ep.episode_number)).copied(),
            )
        })
        .collect();

    Ok(json!({
        "Items": items,
        "TotalRecordCount": items.len()
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

fn episode_item_json(
    server_id: &str,
    work_id: i64,
    series_name: Option<&str>,
    ep: &EpisodeGroupRow,
    history: Option<(i64, i64, bool)>,
) -> Value {
    let (pos_ticks, dur_ticks, is_played) = history.unwrap_or((0, 0, false));
    let effective_dur = if dur_ticks > 0 {
        dur_ticks
    } else if ep.runtime > 0 {
        (ep.runtime as i64) * 600_000_000
    } else {
        0
    };

    let mut obj = json!({
        "Id": format!("ep_{}_{}_{}", work_id, ep.season_number, ep.episode_number),
        "Name": ep.title.clone().unwrap_or_else(|| format!("第 {} 集", ep.episode_number)),
        "ServerId": server_id,
        "SeriesId": format!("work_{}", work_id),
        "SeasonId": format!("season_{}_{}", work_id, ep.season_number),
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
    if let Some(name) = series_name {
        obj["SeriesName"] = json!(name);
    }
    if effective_dur > 0 {
        obj["RunTimeTicks"] = json!(effective_dur);
    }
    obj
}

/// 全局单集列表（IncludeItemTypes=Episode 且无剧集父级）：
/// 从 sync_files + media_works + work_episodes 分页查询，搜索覆盖分集标题与剧名
#[allow(clippy::too_many_arguments)]
async fn get_episodes_listing(
    state: &AppState,
    _user_id: &str,
    q: &ItemsQuery,
    category_filter: Option<String>,
    allowed_cats: Option<Vec<String>>,
    vlib_rule: Option<VirtualLibraryRule>,
    limit: i64,
    offset: i64,
) -> Result<Value, (StatusCode, String)> {
    let search_term = q.search_term.as_deref();
    let search_pattern = build_search_pattern(search_term);
    let rule_json = vlib_rule
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let build = |with_paging: bool| -> DynSql {
        let mut sql = DynSql::new("");
        sql.push(
            r#"
            SELECT sf.work_id, m.title AS series_title, sf.season_number, sf.episode_number,
                   we.title AS episode_title, we.overview, we.still_path, we.air_date,
                   COALESCE(we.runtime, 0) AS runtime, COUNT(*) OVER() AS total_count
            FROM (
                SELECT DISTINCT ON (sfx.work_id, sfx.season_number, sfx.episode_number)
                    sfx.work_id, sfx.season_number, sfx.episode_number
                FROM sync_files sfx
                JOIN sync_folders fx ON fx.id = sfx.folder_id
                WHERE fx.work_id IS NOT NULL AND fx.is_ignored = FALSE
                ORDER BY sfx.work_id, sfx.season_number, sfx.episode_number,
                         COALESCE(sfx.priority_override, sfx.priority_score + fx.priority_boost) DESC, sfx.id ASC
            ) sf
            JOIN media_works m ON m.id = sf.work_id
            LEFT JOIN work_episodes we
              ON we.work_id = sf.work_id AND we.season_number = sf.season_number AND we.episode_number = sf.episode_number
            WHERE EXISTS (
                SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE
            )
            "#,
        );
        if let Some(cat) = &category_filter {
            sql.push_bind_push(" AND m.category = ", cat.clone(), "");
        }
        if let Some(cats) = &allowed_cats {
            sql.push_bind_push(" AND m.category = ANY(", cats.clone(), ")");
        }
        if let Some(rule) = &vlib_rule {
            let rj = sql.bind(rule_json.clone().unwrap_or(Value::Null));
            sql.push(&format!(" AND {}", rule.sql_predicate(&format!("{}::jsonb", rj))));
        }
        if let Some(p) = &search_pattern {
            let pat = sql.bind(p.clone());
            sql.push(&format!(
                " AND (we.title ILIKE {pat} OR m.title ILIKE {pat} OR m.original_title ILIKE {pat} OR m.sort_title ILIKE {pat})"
            ));
        }
        sql.push(
            " ORDER BY m.year DESC NULLS LAST, sf.work_id DESC, sf.season_number ASC, sf.episode_number ASC",
        );
        if with_paging {
            sql.push(" LIMIT ");
            let _ = sql.bind(limit);
            sql.push(" OFFSET ");
            let _ = sql.bind(offset);
        }
        sql
    };

    let rows: Vec<EpisodeListRow> = build(true)
        .qb
        .build_query_as()
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // 空页 + StartIndex > 0：退回一次分页前查询取真实总数
    let total = if rows.is_empty() && offset > 0 {
        let mut sql = build(false);
        let head: Option<EpisodeListRow> = sql
            .qb
            .build_query_as()
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        head.and_then(|r| r.total_count).unwrap_or(0)
    } else {
        rows.first().and_then(|r| r.total_count).unwrap_or(0)
    };

    let cfg = state.config.load();
    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let ep = EpisodeGroupRow {
                season_number: r.season_number,
                episode_number: r.episode_number,
                title: r.episode_title,
                overview: r.overview,
                still_path: r.still_path,
                air_date: r.air_date,
                runtime: r.runtime,
            };
            episode_item_json(
                &cfg.server_id,
                r.work_id,
                Some(r.series_title.as_str()),
                &ep,
                None,
            )
        })
        .collect();

    Ok(json!({
        "Items": items,
        "StartIndex": offset,
        "TotalRecordCount": total
    }))
}

#[derive(sqlx::FromRow)]
struct EpisodeListRow {
    work_id: i64,
    series_title: String,
    season_number: i32,
    episode_number: i32,
    episode_title: Option<String>,
    overview: Option<String>,
    still_path: Option<String>,
    air_date: Option<String>,
    runtime: i32,
    total_count: Option<i64>,
}

pub(super) async fn get_item_detail(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((path_user_id, item_id)): Path<(String, String)>,
    Query(q): Query<PlaybackQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id =
        resolve_valid_user_id_with_query(&state, Some(&path_user_id), &headers, q.api_key.as_deref())
            .await?;
    // 零转存浪费：进入详情页时后台对小雅父目录执行只读 fs/list 寻址预热并回填体积
    if let Some((wid, _)) = parse_season_id(&item_id) {
        state.spawn_prewarm_media_dirs(wid, None);
    } else if let Some(wid) = parse_work_id(&item_id) {
        state.spawn_prewarm_media_dirs(wid, None);
    }
    Ok(Json(get_item_detail_inner(&state, &user_id, &headers, &item_id).await?))
}

/// 非 user-scoped 别名 `GET /Items/:item_id`
pub(super) async fn get_item_detail_unscoped(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<String>,
    Query(q): Query<PlaybackQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id_with_query(&state, None, &headers, q.api_key.as_deref()).await?;
    if let Some((wid, _)) = parse_season_id(&item_id) {
        state.spawn_prewarm_media_dirs(wid, None);
    } else if let Some(wid) = parse_work_id(&item_id) {
        state.spawn_prewarm_media_dirs(wid, None);
    }
    Ok(Json(get_item_detail_inner(&state, &user_id, &headers, &item_id).await?))
}

#[allow(clippy::type_complexity)]
async fn get_item_detail_inner(
    state: &AppState,
    user_id: &str,
    headers: &HeaderMap,
    item_id: &str,
) -> Result<Value, (StatusCode, String)> {
    let cfg = state.config.load();
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("127.0.0.1:8096")
        .to_string();
    let token = headers
        .get("x-emby-token")
        .or_else(|| headers.get("x-mediabrowser-token"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let session_id = Uuid::new_v4().simple().to_string();

    if let Some((wid, snum)) = parse_season_id(item_id) {
        return get_season_detail(state, user_id, wid, snum).await;
    }

    if let Some(raw_wid) = item_id.strip_prefix("work_").or_else(|| item_id.strip_prefix("media_")) {
        let wid = raw_wid
            .parse::<i64>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的 work_id".to_string()))?;

        ensure_media_accessible(state, user_id, wid).await?;

        let row: Option<WorkDetailRow> = sqlx::query_as(
            r#"
            SELECT media_type, tmdb_id, title, year, overview, rating, poster_path, backdrop_path,
                   (tmdb_metadata->>'number_of_seasons')::int, (tmdb_metadata->>'number_of_episodes')::int,
                   original_title
            FROM media_works WHERE id = $1
            "#,
        )
        .bind(wid)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let Some(work) = row else {
            return Err((StatusCode::NOT_FOUND, "条目不存在".to_string()));
        };

        if work.media_type == "tv" {
            let _ = state.scraper.priority_tx.try_send(ScrapeTask::WorkSeasons(wid));
        }

        let fav_set = load_user_favorite_keys(state, user_id).await;
        let is_fav = is_media_favorited(&fav_set, wid, work.tmdb_id, &work.media_type);
        let history_map = load_media_episode_histories(state, user_id, wid, work.tmdb_id, &work.media_type).await;
        let (pos_ticks, dur_ticks, is_played) = history_map
            .get(&(0, 1))
            .copied()
            .unwrap_or((0, 0, false));

        let mut media_sources = vec![];
        if work.media_type == "movie" {
            media_sources = build_media_sources_for_movie(state, &host, &token, &session_id, wid).await?;
        }

        let mut image_tags = json!({});
        if work.poster_path.is_some() {
            image_tags["Primary"] = json!("v1");
        }

        let mut resp_obj = json!({
            "Id": format!("work_{}", wid),
            "Name": work.title,
            "OriginalTitle": work.original_title,
            "SortName": work.title,
            "ServerId": cfg.server_id,
            "Type": if work.media_type == "movie" { "Movie" } else { "Series" },
            "ProductionYear": work.year,
            "Overview": work.overview,
            "CommunityRating": work.rating,
            "ImageTags": image_tags,
            "MediaSources": media_sources,
            "UserData": {
                "PlaybackPositionTicks": pos_ticks,
                "PlayCount": if is_played { 1 } else { 0 },
                "IsFavorite": is_fav,
                "Played": is_played
            }
        });
        if work.backdrop_path.is_some() {
            resp_obj["BackdropImageTags"] = json!(["v1"]);
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

        let ep_meta: Option<(Option<String>, Option<String>, Option<String>, Option<String>, i32)> = sqlx::query_as(
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

        let media_meta: Option<(Option<i32>, String, String)> = sqlx::query_as(
            "SELECT tmdb_id, media_type, title FROM media_works WHERE id = $1",
        )
        .bind(wid)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let (tmdb_id, mtype, series_title) =
            media_meta.unwrap_or((None, "tv".to_string(), String::new()));
        let history_map = load_media_episode_histories(state, user_id, wid, tmdb_id, &mtype).await;
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

        let sources = build_media_sources_for_episode(state, &host, &token, &session_id, wid, snum, epnum).await?;
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
        return Ok(resp_obj);
    }

    // 基础分类 / 智能视图根条目详情
    if super::views::base_library_category(item_id).is_some() || item_id.starts_with("lib_vlib_") {
        return get_view_detail(state, &cfg.server_id, item_id).await;
    }

    Err((StatusCode::NOT_FOUND, "未知资源标识".to_string()))
}

/// `season_{wid}_{snum}` 详情（季号 0 = 特别篇）
#[allow(clippy::type_complexity)]
async fn get_season_detail(
    state: &AppState,
    user_id: &str,
    wid: i64,
    snum: i32,
) -> Result<Value, (StatusCode, String)> {
    let cfg = state.config.load();
    ensure_media_accessible(state, user_id, wid).await?;

    let row: Option<(Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as(
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

    let child_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(DISTINCT (sf.season_number, sf.episode_number))
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        WHERE f.work_id = $1 AND f.is_ignored = FALSE AND sf.season_number = $2
        "#,
    )
    .bind(wid)
    .bind(snum)
    .fetch_one(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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
        "ImageTags": if poster.is_some() { json!({"Primary": "v1"}) } else { json!({}) }
    });
    Ok(obj)
}

/// `lib_*` / `lib_vlib_*` 根条目详情（CollectionFolder）
async fn get_view_detail(
    state: &AppState,
    server_id: &str,
    item_id: &str,
) -> Result<Value, (StatusCode, String)> {
    if let Some(cat) = super::views::base_library_category(item_id) {
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
        .trim_start_matches("lib_vlib_")
        .parse::<i32>()
        .map_err(|_| (StatusCode::BAD_REQUEST, "无效的智能视图 ID".to_string()))?;
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT name, icon_type FROM virtual_libraries WHERE id = $1 AND is_enabled = TRUE")
            .bind(vid)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some((name, icon_type)) = row else {
        return Err((StatusCode::NOT_FOUND, "智能视图不存在或已停用".to_string()));
    };
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
    let user_id = resolve_valid_user_id_with_query(&state, q.user_id.as_deref(), &headers, q.api_key.as_deref())
        .await?;
    let wid = parse_work_id(&series_id)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "无效的 series_id".to_string()))?;

    ensure_media_accessible(&state, &user_id, wid).await?;

    // 零转存浪费：进入剧集季列表时，后台执行只读 fs/list 预热小雅目录树缓存并回填体积
    state.spawn_prewarm_media_dirs(wid, None);

    let search_term = q.search_term.as_deref();
    Ok(Json(get_seasons_for_media(&state, &user_id, wid, search_term).await?))
}

#[derive(Deserialize, Default)]
pub(super) struct ShowEpisodesQuery {
    #[serde(rename = "SeasonId", alias = "seasonId")]
    season_id: Option<String>,
    #[serde(rename = "Season", alias = "season")]
    season: Option<i32>,
    #[serde(rename = "UserId", alias = "userId")]
    user_id: Option<String>,
    #[serde(rename = "SearchTerm", alias = "searchTerm")]
    search_term: Option<String>,
}

pub(super) async fn get_show_episodes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(series_id): Path<String>,
    Query(q): Query<ShowEpisodesQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id(&state, q.user_id.as_deref(), &headers).await?;
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

    let res = get_episodes_for_media(&state, &user_id, wid, target_season, q.search_term.as_deref()).await?;
    Ok(Json(res))
}

pub(super) async fn get_latest_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id =
        resolve_valid_user_id_with_query(&state, Some(&user_id), &headers, q.api_key.as_deref()).await?;
    let mut res = get_user_items_inner(&state, &user_id, &q).await?;
    // 直接取出 Items 数组，避免整份响应 JSON 深拷贝
    let items = res.get_mut("Items").map(Value::take).unwrap_or_else(|| json!([]));
    Ok(Json(items))
}

/// 继续观看列表 (`GET /Users/:user_id/Items/Resume`)
pub(super) async fn get_resume_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path_user_id): Path<String>,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id =
        resolve_valid_user_id_with_query(&state, Some(&path_user_id), &headers, q.api_key.as_deref())
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

async fn get_resume_items_inner(
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

    let allowed_cats = get_user_allowed_categories(state, user_id).await;
    let rows = batch_build_history_items(state, &cfg.server_id, histories, false, allowed_cats.as_deref()).await?;

    Ok(json!({
        "Items": rows,
        "TotalRecordCount": rows.len()
    }))
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
#[allow(clippy::type_complexity)]
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
    let media_rows: Vec<(i64, i64, String, String, Option<i32>, Option<String>, Option<String>, String)> =
        sqlx::query_as(
            r#"
            SELECT t.hidx, w.id, w.media_type, w.title, w.year, w.overview, w.poster_path, w.category
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

    let mut media_by_idx: std::collections::HashMap<i64, _> = media_rows
        .into_iter()
        .map(|(hidx, wid, mtype, title, year, overview, poster, category)| {
            (hidx, (wid, mtype, title, year, overview, poster, category))
        })
        .collect();

    // 分类权限过滤
    media_by_idx.retain(|_, (_, _, _, _, _, _, category)| match allowed_cats {
        None => true,
        Some(cats) => cats.iter().any(|c| c == category || c.starts_with("lib_vlib_")),
    });

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
        .filter_map(|idx| media_by_idx.get(&(*idx as i64)).map(|(wid, ..)| *wid))
        .collect();
    let ep_snums: Vec<i32> = tv_idx.iter().map(|idx| histories[*idx].season_number).collect();
    let ep_nums: Vec<i32> = tv_idx.iter().map(|idx| histories[*idx].episode_number).collect();
    let advances: Vec<bool> = tv_idx
        .iter()
        .map(|idx| next_up_mode && histories[*idx].is_played)
        .collect();

    let mut ep_by_idx: std::collections::HashMap<i64, (i32, i32, Option<String>, Option<String>, Option<String>, Option<String>, i32)> =
        std::collections::HashMap::new();
    if !ep_wids.is_empty() {
        let rows: Vec<(
            i64,
            i32,
            i32,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            i32,
        )> = sqlx::query_as(
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
            ep_by_idx.insert(row.0, (row.1, row.2, row.3, row.4, row.5, row.6, row.7));
        }
    }

    // 3. 组装条目（保持 histories 原始顺序）
    let mut items = Vec::with_capacity(n);
    for (idx, h) in histories.iter().enumerate() {
        let Some((wid, mtype, media_title, year, media_overview, poster, _)) =
            media_by_idx.get(&(idx as i64))
        else {
            continue;
        };
        let (wid, mtype) = (*wid, mtype.clone());

        if mtype == "movie" {
            if next_up_mode {
                continue;
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
            if poster.is_some() {
                obj["BackdropImageTags"] = json!(["v1"]);
            }
            if h.duration_ticks > 0 {
                obj["RunTimeTicks"] = json!(h.duration_ticks);
            }
            items.push(obj);
            continue;
        }

        let Some((snum, epnum, ep_title, ep_overview, still_path, air_date, runtime)) =
            ep_by_idx.get(&(idx as i64))
        else {
            continue;
        };
        let advance = next_up_mode && h.is_played;
        let pos_ticks = if advance { 0 } else { h.position_ticks };
        let played = if advance { false } else { h.is_played };
        let effective_dur = if !advance && h.duration_ticks > 0 {
            h.duration_ticks
        } else if *runtime > 0 {
            (*runtime as i64) * 600_000_000
        } else {
            0
        };

        let mut obj = json!({
            "Id": format!("ep_{}_{}_{}", wid, snum, epnum),
            "Name": ep_title.clone().unwrap_or_else(|| format!("第 {} 集", epnum)),
            "ServerId": server_id,
            "SeriesId": format!("work_{}", wid),
            "SeriesName": media_title,
            "SeasonId": format!("season_{}_{}", wid, snum),
            "Type": "Episode",
            "IsFolder": false,
            "IndexNumber": epnum,
            "ParentIndexNumber": snum,
            "Overview": ep_overview.clone().or(media_overview.clone()),
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
        items.push(obj);
    }

    Ok(items)
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

    let tie_dir = if first_is_asc.unwrap_or(false) { "ASC" } else { "DESC" };
    clauses.push(format!("id {}", tie_dir));
    (clauses.join(", "), user_uses)
}

#[cfg(test)]
mod tests {
    use super::{build_emby_order_clause, favorite_filter_has_no_supported_media_type, parse_explicit_work_ids};
    use std::collections::HashSet;

    #[test]
    fn favorite_filter_only_short_circuits_when_types_are_explicitly_unsupported() {
        let empty = HashSet::new();
        let episode = HashSet::from(["Episode"]);
        let movie = HashSet::from(["Movie"]);
        assert!(!favorite_filter_has_no_supported_media_type(true, &empty));
        assert!(favorite_filter_has_no_supported_media_type(true, &episode));
        assert!(!favorite_filter_has_no_supported_media_type(true, &movie));
        assert!(!favorite_filter_has_no_supported_media_type(false, &episode));
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

        let (clause, uses) = build_emby_order_clause("Popularity,SortName", Some("Descending"), None);
        assert_eq!(clause, "popularity DESC NULLS LAST, title DESC, id DESC");
        assert_eq!(uses, 0);

        let (clause, _) =
            build_emby_order_clause("CommunityRating,DateCreated", Some("Descending,Ascending"), None);
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
        assert_eq!(parse_explicit_work_ids(Some("lib_movie,lib_vlib_9")), Some(vec![]));
    }
}
