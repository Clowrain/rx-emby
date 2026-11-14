use crate::state::AppState;
use axum::{
    middleware::{self, Next},
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use uuid::Uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VirtualLibraryRule {
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub countries: Vec<String>,
    #[serde(default)]
    pub networks: Vec<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub year_min: Option<i32>,
    #[serde(default)]
    pub year_max: Option<i32>,
    #[serde(default)]
    pub min_rating: Option<f32>,
    #[serde(default)]
    pub keyword: Option<String>,
}

pub fn router() -> Router<Arc<AppState>> {
    let core = Router::new()
        .route("/System/Info/Public", get(system_info_public))
        .route("/System/Info", get(system_info_public))
        .route("/Users/AuthenticateByName", post(authenticate_by_name))
        .route("/Users/:user_id", get(get_user_info))
        .route("/Users/:user_id/Views", get(get_user_views))
        .route("/Users/:user_id/Items", get(get_user_items))
        .route("/Users/:user_id/Items/Resume", get(get_resume_items))
        .route("/Users/:user_id/Items/Latest", get(get_latest_items))
        .route("/Users/:user_id/Items/:item_id", get(get_item_detail))
        .route(
            "/Users/:user_id/PlayedItems/:item_id",
            post(mark_item_played).delete(unmark_item_played),
        )
        .route(
            "/Users/:user_id/FavoriteItems/:item_id",
            post(mark_item_favorite).delete(unmark_item_favorite),
        )
        .route("/Shows/NextUp", get(get_next_up_items))
        .route("/Shows/:series_id/Seasons", get(get_show_seasons))
        .route("/Shows/:series_id/Episodes", get(get_show_episodes))
        .route("/Items/:item_id/PlaybackInfo", post(get_playback_info).get(get_playback_info))
        .route("/Sessions/Playing", post(session_playing))
        .route("/Sessions/Playing/Progress", post(session_progress))
        .route("/Sessions/Playing/Stopped", post(session_stopped))
        .route("/Videos/:item_id/stream", get(stream_video))
        .route("/Videos/:item_id/:file_name", get(stream_video_with_file))
        .route("/d/*alist_path", get(stream_by_d_path))
        .route("/Items/:item_id/Images/:image_type", get(get_item_image))
        .route(
            "/Items/:item_id/Images/:image_type/:image_index",
            get(get_item_image_indexed),
        );

    Router::new()
        .nest("/emby", core.clone())
        .merge(core)
        .layer(middleware::from_fn(trace_emby_request))
}

async fn system_info_public(State(state): State<Arc<AppState>>) -> Json<Value> {
    let cfg = state.config.load();
    Json(json!({
        "Id": cfg.server_id,
        "ServerName": cfg.server_name,
        "Version": "4.8.8.0",
        "OperatingSystem": "Linux",
        "LocalAddress": "http://127.0.0.1:8096"
    }))
}

#[derive(Deserialize)]
struct AuthReq {
    #[serde(rename = "Username")]
    username: String,
    #[serde(rename = "Pw", default)]
    pw: String,
}

async fn authenticate_by_name(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<AuthReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let username = if payload.username.trim().is_empty() {
        "admin"
    } else {
        payload.username.trim()
    };

    // 查询或首次自动初始化管理员账号
    let user_row: Option<(String, String, bool)> = sqlx::query_as(
        "SELECT id, password_hash, is_admin FROM users WHERE username = $1",
    )
    .bind(username)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (user_id, is_admin) = match user_row {
        Some((uid, stored_pw, admin)) => {
            if !stored_pw.is_empty() && stored_pw != payload.pw {
                return Err((StatusCode::UNAUTHORIZED, "用户名或密码错误".to_string()));
            }
            (uid, admin)
        }
        None => {
            // 若库中尚无任何用户，自动创建首个管理员账号
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
                .fetch_one(&state.db)
                .await
                .unwrap_or(0);
            if count == 0 {
                let new_id = Uuid::new_v4().simple().to_string()[..16].to_string();
                sqlx::query(
                    "INSERT INTO users (id, username, password_hash, is_admin) VALUES ($1, $2, $3, TRUE)",
                )
                .bind(&new_id)
                .bind(username)
                .bind(&payload.pw)
                .execute(&state.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                (new_id, true)
            } else {
                return Err((StatusCode::UNAUTHORIZED, "用户不存在".to_string()));
            }
        }
    };

    let token = Uuid::new_v4().simple().to_string();
    let client_ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("EmbyClient")
        .chars()
        .take(60)
        .collect::<String>();

    let _ = sqlx::query(
        "INSERT INTO access_tokens (token, user_id, device_id, device_name, client_name) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&token)
    .bind(&user_id)
    .bind(&token[..12])
    .bind(&client_ua)
    .bind(&client_ua)
    .execute(&state.db)
    .await;

    let cfg = state.config.load();
    Ok(Json(json!({
        "User": {
            "Id": user_id,
            "Name": username,
            "ServerId": cfg.server_id,
            "Policy": { "IsAdministrator": is_admin }
        },
        "AccessToken": token,
        "ServerId": cfg.server_id
    })))
}

async fn get_user_info(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<String>,
) -> Json<Value> {
    let cfg = state.config.load();
    let user_row: Option<(String, bool)> =
        sqlx::query_as("SELECT username, is_admin FROM users WHERE id = $1")
            .bind(&user_id)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
    let (username, is_admin) = user_row.unwrap_or_else(|| ("admin".to_string(), true));

    Json(json!({
        "Id": user_id,
        "Name": username,
        "ServerId": cfg.server_id,
        "Policy": { "IsAdministrator": is_admin }
    }))
}

async fn get_user_views(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path_user_id): Path<String>,
) -> Json<Value> {
    let cfg = state.config.load();
    let user_id = resolve_valid_user_id(&state, Some(&path_user_id), &headers)
        .await
        .unwrap_or(path_user_id);

    let allowed_cats = get_user_allowed_categories(&state, &user_id).await;

    let base_libs = vec![
        ("lib_movie", "电影", "movies"),
        ("lib_tv", "电视剧", "tvshows"),
        ("lib_anime", "动漫", "tvshows"),
        ("lib_doc", "纪录片", "movies"),
        ("lib_variety", "综艺", "tvshows"),
    ];

    let mut items: Vec<Value> = base_libs
        .into_iter()
        .filter(|(id, name, _)| {
            match &allowed_cats {
                None => true,
                Some(cats) => cats.iter().any(|c| c == "*" || c == name || c == id),
            }
        })
        .map(|(id, name, ctype)| {
            json!({
                "Id": id,
                "Name": name,
                "ServerId": cfg.server_id,
                "Type": "CollectionFolder",
                "CollectionType": ctype,
                "IsFolder": true
            })
        })
        .collect();

    // 追加已启用的自定义虚拟媒体库 (Smart Views)
    let vlibs: Vec<(i32, String, String)> = sqlx::query_as(
        "SELECT id, name, icon_type FROM virtual_libraries WHERE is_enabled = TRUE ORDER BY sort_order ASC, id ASC",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    for (vid, vname, icon_type) in vlibs {
        let view_id = format!("lib_vlib_{}", vid);
        let allowed = match &allowed_cats {
            None => true,
            Some(cats) => cats
                .iter()
                .any(|c| c == "*" || c == "vlib_*" || c == &view_id || c == &vname),
        };
        if !allowed {
            continue;
        }
        let ctype = if icon_type == "movies" || icon_type == "movie" {
            "movies"
        } else {
            "tvshows"
        };
        items.push(json!({
            "Id": view_id,
            "Name": vname,
            "ServerId": cfg.server_id,
            "Type": "CollectionFolder",
            "CollectionType": ctype,
            "IsFolder": true
        }));
    }

    Json(json!({
        "Items": items,
        "TotalRecordCount": items.len()
    }))
}

/// 获取用户被允许访问的分类集合：`None` 代表不限权限（管理员或包含 `*`），`Some(vec)` 代表受限分类列表
async fn get_user_allowed_categories(state: &AppState, user_id: &str) -> Option<Vec<String>> {
    let row: Option<(bool, Vec<String>)> =
        sqlx::query_as("SELECT is_admin, allowed_categories FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);

    match row {
        Some((is_admin, cats)) => {
            if is_admin || cats.is_empty() || cats.iter().any(|c| c == "*") {
                None
            } else {
                Some(cats)
            }
        }
        None => None,
    }
}

#[derive(Deserialize, Default)]
struct ItemsQuery {
    #[serde(rename = "ParentId", alias = "parentId")]
    parent_id: Option<String>,
    #[serde(rename = "SearchTerm", alias = "searchTerm")]
    search_term: Option<String>,
    #[serde(rename = "Filters", alias = "filters")]
    filters: Option<String>,
    #[serde(rename = "IncludeItemTypes", alias = "includeItemTypes")]
    include_item_types: Option<String>,
    #[serde(rename = "SortBy", alias = "sortBy")]
    sort_by: Option<String>,
    #[serde(rename = "SortOrder", alias = "sortOrder")]
    sort_order: Option<String>,
    #[serde(rename = "Ids", alias = "ids")]
    ids: Option<String>,
    #[serde(rename = "UserId", alias = "userId")]
    user_id: Option<String>,
    #[serde(rename = "StartIndex", alias = "startIndex", default)]
    start_index: i64,
    #[serde(rename = "Limit", alias = "limit")]
    limit: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct MediaListRow {
    id: i64,
    media_type: String,
    tmdb_id: Option<i32>,
    title: String,
    original_title: Option<String>,
    year: Option<i32>,
    overview: Option<String>,
    rating: f32,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
}

async fn get_user_items(
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

    // 3.5 解析自定义虚拟媒体库 (ParentId 为 "lib_vlib_1") 及用户分类访问权限
    let mut vlib_rule: Option<VirtualLibraryRule> = None;
    if let Some(ref pid) = q.parent_id {
        if let Some(raw_vid) = pid.strip_prefix("lib_vlib_") {
            if let Ok(vid) = raw_vid.parse::<i32>() {
                let raw_json: Option<Value> = sqlx::query_scalar(
                    "SELECT filter_rules FROM virtual_libraries WHERE id = $1 AND is_enabled = TRUE",
                )
                .bind(vid)
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None);
                if let Some(val) = raw_json {
                    vlib_rule = serde_json::from_value::<VirtualLibraryRule>(val).ok();
                } else {
                    return Ok(Json(json!({ "Items": [], "TotalRecordCount": 0 })));
                }
            }
        }
    }

    let allowed_user_cats = get_user_allowed_categories(&state, &user_id).await;

    let category_filter = match q.parent_id.as_deref() {
        Some("lib_movie") => Some("电影"),
        Some("lib_tv") => Some("电视剧"),
        Some("lib_anime") => Some("动漫"),
        Some("lib_doc") => Some("纪录片"),
        Some("lib_variety") => Some("综艺"),
        _ => None,
    };

    let media_type_filter = if !type_set.is_empty() {
        match (type_set.contains("Movie"), type_set.contains("Series")) {
            (true, false) => Some("movie".to_string()),
            (false, true) => Some("tv".to_string()),
            _ => vlib_rule.as_ref().and_then(|r| r.media_type.clone()).filter(|s| !s.is_empty()),
        }
    } else {
        vlib_rule.as_ref().and_then(|r| r.media_type.clone()).filter(|s| !s.is_empty())
    };

    let vlib_categories: Option<Vec<String>> = vlib_rule
        .as_ref()
        .map(|r| r.categories.clone())
        .filter(|v| !v.is_empty());
    let vlib_countries: Option<Vec<String>> = vlib_rule
        .as_ref()
        .map(|r| r.countries.clone())
        .filter(|v| !v.is_empty());
    let vlib_networks: Option<Vec<String>> = vlib_rule
        .as_ref()
        .map(|r| r.networks.clone())
        .filter(|v| !v.is_empty());
    let vlib_genres: Option<Vec<String>> = vlib_rule
        .as_ref()
        .map(|r| r.genres.clone())
        .filter(|v| !v.is_empty());
    let vlib_year_min: Option<i32> = vlib_rule.as_ref().and_then(|r| r.year_min);
    let vlib_year_max: Option<i32> = vlib_rule.as_ref().and_then(|r| r.year_max);
    let vlib_min_rating: Option<f32> = vlib_rule.as_ref().and_then(|r| r.min_rating);
    let vlib_kw: Option<String> = vlib_rule
        .as_ref()
        .and_then(|r| r.keyword.as_deref())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{}%", s));

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

    // 4. 组装排序规则（兼容 SortBy 与 SortOrder），并在同 (tmdb_id, media_type) 多目录间自动折叠去重
    let is_asc = q
        .sort_order
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case("Ascending"))
        .unwrap_or(false);
    let sort_key = q.sort_by.as_deref().unwrap_or("");
    let order_clause = if sort_key.contains("DateCreated") {
        if is_asc {
            "created_at ASC, id ASC"
        } else {
            "created_at DESC, id DESC"
        }
    } else if sort_key.contains("CommunityRating") {
        if is_asc {
            "rating ASC, id DESC"
        } else {
            "rating DESC, id DESC"
        }
    } else if sort_key.contains("SortName") || sort_key.contains("Name") {
        if is_asc {
            "title ASC, id DESC"
        } else {
            "title DESC, id DESC"
        }
    } else if is_asc {
        "year ASC NULLS LAST, id ASC"
    } else {
        "year DESC NULLS LAST, id DESC"
    };

    let sql = format!(
        r#"
        SELECT id, media_type, tmdb_id, title, original_title, year, overview, rating, poster_path, backdrop_path
        FROM (
            SELECT DISTINCT ON (COALESCE(m.tmdb_id::text || ':' || m.media_type, 'raw:' || m.id::text))
                m.id, m.media_type, m.tmdb_id, m.title, m.original_title, m.year, m.overview, m.rating, m.poster_path, m.backdrop_path, m.created_at
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
              AND ($12::text[] IS NULL OR m.category = ANY($12))
              AND ($13::text[] IS NULL OR m.countries && $13)
              AND ($14::text[] IS NULL OR m.networks && $14)
              AND ($15::text[] IS NULL OR m.genres && $15)
              AND ($16::int4 IS NULL OR m.year >= $16)
              AND ($17::int4 IS NULL OR m.year <= $17)
              AND ($18::float4 IS NULL OR m.rating >= $18)
              AND ($19::text IS NULL OR m.title ILIKE $19 OR m.original_title ILIKE $19 OR m.root_dir ILIKE $19)
            ORDER BY
                COALESCE(m.tmdb_id::text || ':' || m.media_type, 'raw:' || m.id::text),
                (m.scrape_status IN (1, 3)) DESC,
                (m.poster_path IS NOT NULL) DESC,
                m.id ASC
        ) AS deduped
        ORDER BY {}
        LIMIT $9 OFFSET $10
        "#,
        order_clause
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
        .bind(&vlib_categories)
        .bind(&vlib_countries)
        .bind(&vlib_networks)
        .bind(&vlib_genres)
        .bind(vlib_year_min)
        .bind(vlib_year_max)
        .bind(vlib_min_rating)
        .bind(&vlib_kw)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let total: i64 = sqlx::query_scalar(
        r#"
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
          AND ($10::text[] IS NULL OR m.category = ANY($10))
          AND ($11::text[] IS NULL OR m.countries && $11)
          AND ($12::text[] IS NULL OR m.networks && $12)
          AND ($13::text[] IS NULL OR m.genres && $13)
          AND ($14::int4 IS NULL OR m.year >= $14)
          AND ($15::int4 IS NULL OR m.year <= $15)
          AND ($16::float4 IS NULL OR m.rating >= $16)
          AND ($17::text IS NULL OR m.title ILIKE $17 OR m.original_title ILIKE $17 OR m.root_dir ILIKE $17)
        "#,
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
    .bind(&vlib_categories)
    .bind(&vlib_countries)
    .bind(&vlib_networks)
    .bind(&vlib_genres)
    .bind(vlib_year_min)
    .bind(vlib_year_max)
    .bind(vlib_min_rating)
    .bind(&vlib_kw)
    .fetch_one(&state.db)
    .await
    .unwrap_or(rows.len() as i64);

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

            json!({
                "Id": format!("media_{}", r.id),
                "Name": r.title,
                "OriginalTitle": r.original_title,
                "ServerId": cfg.server_id,
                "Type": emby_type,
                "IsFolder": is_folder,
                "ProductionYear": r.year,
                "Overview": r.overview,
                "CommunityRating": r.rating,
                "ImageTags": image_tags,
                "UserData": {
                    "PlaybackPositionTicks": 0,
                    "PlayCount": 0,
                    "IsFavorite": is_fav,
                    "Played": false
                }
            })
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

async fn get_item_detail(
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
struct PlaybackQuery {
    #[serde(
        rename = "MediaSourceId",
        alias = "mediaSourceId",
        alias = "mediasourceid"
    )]
    media_source_id: Option<String>,
    #[serde(
        rename = "IsPlayback",
        alias = "isPlayback",
        alias = "isplayback"
    )]
    is_playback: Option<bool>,
    #[serde(
        rename = "api_key",
        alias = "ApiKey",
        alias = "X-Emby-Token",
        alias = "x-emby-token"
    )]
    api_key: Option<String>,
}

async fn get_playback_info(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<String>,
    Query(q): Query<PlaybackQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("127.0.0.1:8096")
        .to_string();
    let client_ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
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
    let sources = if let Some(raw_mid) = item_id.strip_prefix("media_") {
        let mid = raw_mid.parse::<i64>().unwrap_or(0);
        build_media_sources_for_movie(&state, &host, &token, &session_id, mid).await?
    } else if let Some(raw_eid) = item_id.strip_prefix("ep_") {
        let eid = raw_eid.parse::<i64>().unwrap_or(0);
        let ep_meta: Option<(i64, i32, i32)> = sqlx::query_as(
            "SELECT media_item_id, season_number, episode_number FROM episodes WHERE id = $1",
        )
        .bind(eid)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        match ep_meta {
            Some((mid, snum, epnum)) => {
                build_media_sources_for_episode(&state, &host, &token, &session_id, mid, snum, epnum).await?
            }
            None => vec![],
        }
    } else {
        vec![]
    };

    // 当客户端明确处于点击播放状态 (IsPlayback=true) 时，立即启动 Singleflight 异步换链，
    // 与客户端解析 JSON 及初始化播放器解码器并行执行，后续 stream.mp4 直接复用该换链任务
    if q.is_playback.unwrap_or(true) {
        let target_src_id = q
            .media_source_id
            .as_deref()
            .and_then(|s| s.strip_prefix("src_").or(Some(s)))
            .and_then(|s| s.parse::<i64>().ok())
            .or_else(|| {
                sources
                    .first()
                    .and_then(|v| v.get("Id"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.strip_prefix("src_"))
                    .and_then(|s| s.parse::<i64>().ok())
            });

        if let Some(ep_id) = target_src_id {
            let state_clone = state.clone();
            tokio::spawn(async move {
                if let Ok(Some((alist_path, path_hash))) = sqlx::query_as::<_, (String, i64)>(
                    "SELECT alist_path, path_hash FROM episodes WHERE id = $1",
                )
                .bind(ep_id)
                .fetch_optional(&state_clone.db)
                .await
                {
                    let _ = state_clone
                        .resolve_playback_url(path_hash, &alist_path, &client_ua)
                        .await;
                }
            });
        }
    }

    tracing::info!("🎬 [PlaybackInfo] item_id={} 返回 {} 个源: {}", item_id, sources.len(), serde_json::to_string(&sources).unwrap_or_default());
    Ok(Json(json!({
        "MediaSources": sources,
        "PlaySessionId": session_id
    })))
}

#[derive(sqlx::FromRow)]
struct SourceRecord {
    id: i64,
    version_tag: String,
    container: String,
    alist_path: String,
    file_size_bytes: Option<i64>,
}

async fn build_media_sources_for_movie(
    state: &AppState,
    host: &str,
    token: &str,
    session_id: &str,
    media_id: i64,
) -> Result<Vec<Value>, (StatusCode, String)> {
    let sibling_ids = state.resolve_sibling_media_ids(media_id).await;
    let rows: Vec<SourceRecord> = sqlx::query_as(
        r#"
        SELECT id, version_tag, container, alist_path, file_size_bytes
        FROM episodes
        WHERE media_item_id = ANY($1)
        ORDER BY priority_score DESC, id ASC
        "#,
    )
    .bind(&sibling_ids)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    serialize_media_sources(state, host, token, session_id, format!("media_{}", media_id), rows).await
}

async fn build_media_sources_for_episode(
    state: &AppState,
    host: &str,
    token: &str,
    session_id: &str,
    media_id: i64,
    season_num: i32,
    episode_num: i32,
) -> Result<Vec<Value>, (StatusCode, String)> {
    let sibling_ids = state.resolve_sibling_media_ids(media_id).await;
    let rows: Vec<SourceRecord> = sqlx::query_as(
        r#"
        SELECT id, version_tag, container, alist_path, file_size_bytes
        FROM episodes
        WHERE media_item_id = ANY($1) AND season_number = $2 AND episode_number = $3
        ORDER BY priority_score DESC, id ASC
        "#,
    )
    .bind(&sibling_ids)
    .bind(season_num)
    .bind(episode_num)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let rep_id = rows.first().map(|r| r.id).unwrap_or(0);
    serialize_media_sources(state, host, token, session_id, format!("ep_{}", rep_id), rows).await
}

async fn serialize_media_sources(
    _state: &AppState,
    host: &str,
    token: &str,
    session_id: &str,
    item_id: String,
    mut rows: Vec<SourceRecord>,
) -> Result<Vec<Value>, (StatusCode, String)> {
    // 实时用增强版多维规则重算版本标签并自动消除同集重名
    let mut tag_tuples: Vec<(String, i32, String, String)> = rows
        .iter()
        .map(|r| {
            (
                r.version_tag.clone(),
                0,
                r.alist_path.clone(),
                r.container.clone(),
            )
        })
        .collect();
    crate::sync::parser::disambiguate_version_tags(&mut tag_tuples);
    for (row, (new_tag, _, _, _)) in rows.iter_mut().zip(tag_tuples.into_iter()) {
        row.version_tag = new_tag;
    }

    let sources: Vec<Value> = rows
        .into_iter()
        .map(|s| {
            let display_name = match s.file_size_bytes {
                Some(sz) if sz > 0 => {
                    let gb = sz as f64 / 1_073_741_824.0;
                    format!("{} ({:.2} GB)", s.version_tag, gb)
                }
                _ => s.version_tag,
            };

            let ext = if s.container.is_empty() { "mp4" } else { s.container.as_str() };
            let direct_url = format!(
                "http://{}/emby/Videos/{}/stream.{}?MediaSourceId=src_{}&Static=true&PlaySessionId={}&api_key={}",
                host, item_id, ext, s.id, session_id, token
            );
            let display_path = format!("/d{}", s.alist_path);

            json!({
                "Id": format!("src_{}", s.id),
                "Name": display_name,
                "Type": "Default",
                "Protocol": "File",
                "Container": s.container,
                "Size": s.file_size_bytes.unwrap_or(0),
                "SupportsDirectPlay": true,
                "SupportsDirectStream": true,
                "SupportsTranscoding": false,
                "IsRemote": false,
                "RequiresOpening": false,
                "RequiresClosing": false,
                "MediaStreams": [
                    {
                        "Type": "Video",
                        "Index": 0,
                        "Codec": "h264",
                        "IsDefault": true,
                        "IsExternal": false
                    },
                    {
                        "Type": "Audio",
                        "Index": 1,
                        "Codec": "aac",
                        "IsDefault": true,
                        "IsExternal": false
                    }
                ],
                "DirectStreamUrl": direct_url,
                "Path": display_path
            })
        })
        .collect();

    Ok(sources)
}

async fn stream_video_with_file(
    state: State<Arc<AppState>>,
    Path((item_id, _file_name)): Path<(String, String)>,
    q: Query<PlaybackQuery>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    stream_video(state, Path(item_id), q, headers).await
}

async fn stream_video(
    State(state): State<Arc<AppState>>,
    Path(item_id): Path<String>,
    Query(q): Query<PlaybackQuery>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    let client_ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // 提取具体多版本 Source ID（若未指定则回退到 item_id 对应默认源）
    let target_ep_id: i64 = if let Some(ref msid) = q.media_source_id {
        msid.strip_prefix("src_")
            .or_else(|| msid.strip_prefix("ep_"))
            .unwrap_or(msid)
            .parse::<i64>()
            .unwrap_or(0)
    } else if let Some(raw_sid) = item_id.strip_prefix("src_") {
        raw_sid.parse::<i64>().unwrap_or(0)
    } else if let Some(raw_eid) = item_id.strip_prefix("ep_") {
        raw_eid.parse::<i64>().unwrap_or(0)
    } else if let Some(raw_mid) = item_id.strip_prefix("media_") {
        let mid = raw_mid.parse::<i64>().unwrap_or(0);
        sqlx::query_scalar(
            "SELECT id FROM episodes WHERE media_item_id = $1 ORDER BY priority_score DESC LIMIT 1",
        )
        .bind(mid)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None)
        .unwrap_or(0)
    } else {
        item_id.parse::<i64>().unwrap_or(0)
    };

    let row: Option<(String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT alist_path, path_hash, file_size_bytes FROM episodes WHERE id = $1",
    )
    .bind(target_ep_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some((alist_path, path_hash, existing_size)) = row else {
        return Err((StatusCode::NOT_FOUND, "未找到对应的视频播放源".to_string()));
    };

    let resolved = state
        .resolve_playback_url(path_hash, &alist_path, client_ua)
        .await
        .map_err(|e| { tracing::error!("❌ [Stream Error] target_ep_id={} alist_path={} 失败: {:#}", target_ep_id, alist_path, e); (StatusCode::BAD_GATEWAY, format!("获取网盘直链失败: {:#}", e)) })?;

    // 顺手异步回填文件体积
    if existing_size.is_none() {
        if let Some(sz) = resolved.file_size {
            let db = state.db.clone();
            tokio::spawn(async move {
                let _ = sqlx::query("UPDATE episodes SET file_size_bytes = $1 WHERE id = $2")
                    .bind(sz)
                    .bind(target_ep_id)
                    .execute(&db)
                    .await;
            });
        }
    }

    // 返回 302 Found 直跳网盘 CDN，并严格禁用浏览器/播放器缓存该重定向
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, resolved.redirect_url)
        .header(header::CACHE_CONTROL, "no-store, no-cache, must-revalidate")
        .body(axum::body::Body::empty())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// 兼容按 `/d/*alist_path` 逻辑路径直接请求播放，内部换链并 302 跳转真实网盘直链
async fn stream_by_d_path(
    State(state): State<Arc<AppState>>,
    Path(raw_path): Path<String>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    let client_ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let decoded = crate::sync::parser::lossy_url_decode(&raw_path);
    let alist_path = format!("/{}", decoded.trim_start_matches('/'));
    let path_hash = xxhash_rust::xxh3::xxh3_64(alist_path.as_bytes()) as i64;

    let resolved = state
        .resolve_playback_url(path_hash, &alist_path, client_ua)
        .await
        .map_err(|e| {
            tracing::error!(
                "❌ [Stream /d/ Error] alist_path={} 失败: {:#}",
                alist_path,
                e
            );
            (
                StatusCode::BAD_GATEWAY,
                format!("获取网盘直链失败: {:#}", e),
            )
        })?;

    if let Some(sz) = resolved.file_size {
        let db = state.db.clone();
        tokio::spawn(async move {
            let _ = sqlx::query(
                "UPDATE episodes SET file_size_bytes = $1 WHERE path_hash = $2 AND file_size_bytes IS NULL",
            )
            .bind(sz)
            .bind(path_hash)
            .execute(&db)
            .await;
        });
    }

    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, resolved.redirect_url)
        .header(header::CACHE_CONTROL, "no-store, no-cache, must-revalidate")
        .body(axum::body::Body::empty())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn get_item_image_indexed(
    state: State<Arc<AppState>>,
    Path((item_id, image_type, _image_index)): Path<(String, String, String)>,
) -> Result<Response, (StatusCode, String)> {
    get_item_image(state, Path((item_id, image_type))).await
}

async fn get_item_image(
    State(state): State<Arc<AppState>>,
    Path((item_id, image_type)): Path<(String, String)>,
) -> Result<Response, (StatusCode, String)> {
    let (size, raw_file): (&str, Option<String>) = if let Some(raw_eid) = item_id.strip_prefix("ep_") {
        let eid = raw_eid
            .parse::<i64>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的单集 ID".to_string()))?;

        let ep_paths: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            r#"
            SELECT e.still_path, m.backdrop_path, m.poster_path
            FROM episodes e
            JOIN media_items m ON m.id = e.media_item_id
            WHERE e.id = $1
            "#,
        )
        .bind(eid)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let Some((still, backdrop, poster)) = ep_paths else {
            return Err((StatusCode::NOT_FOUND, "单集不存在".to_string()));
        };

        ("w500", still.or(backdrop).or(poster))
    } else if let Some(rest) = item_id.strip_prefix("season_") {
        let mut parts = rest.rsplitn(2, '_');
        let snum = parts
            .next()
            .and_then(|s| s.parse::<i32>().ok())
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "无效的季 ID".to_string()))?;
        let mid = parts
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "无效的季所属剧集 ID".to_string()))?;

        let sibling_ids = state.resolve_sibling_media_ids(mid).await;
        let season_poster: Option<String> = sqlx::query_scalar(
            r#"
            SELECT poster_path FROM seasons
            WHERE media_item_id = ANY($1) AND season_number = $2 AND poster_path IS NOT NULL
            ORDER BY episode_count DESC, id ASC
            LIMIT 1
            "#,
        )
        .bind(&sibling_ids)
        .bind(snum)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

        let final_poster = match season_poster {
            Some(p) => Some(p),
            None => sqlx::query_scalar("SELECT poster_path FROM media_items WHERE id = $1")
                .bind(mid)
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None)
                .flatten(),
        };

        ("w500", final_poster)
    } else {
        let raw_mid = item_id.strip_prefix("media_").unwrap_or(&item_id);
        let mid = raw_mid
            .parse::<i64>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的资源 ID".to_string()))?;

        let paths: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT poster_path, backdrop_path FROM media_items WHERE id = $1",
        )
        .bind(mid)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let Some((poster, backdrop)) = paths else {
            return Err((StatusCode::NOT_FOUND, "条目不存在".to_string()));
        };

        if image_type.eq_ignore_ascii_case("Backdrop") {
            ("w1280", backdrop.or(poster))
        } else {
            ("w500", poster)
        }
    };

    let Some(file_path) = raw_file else {
        return Err((StatusCode::NOT_FOUND, "暂无图片".to_string()));
    };

    let clean_name = file_path.trim_start_matches('/');
    let bytes = state
        .get_or_fetch_tmdb_image(size, clean_name)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("图片回源失败: {:#}", e)))?;

    let content_type = if clean_name.ends_with(".png") {
        "image/png"
    } else {
        "image/jpeg"
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=604800")
        .body(axum::body::Body::from(bytes))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[derive(Deserialize, Default)]
struct ShowEpisodesQuery {
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

async fn get_show_seasons(
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

async fn get_show_episodes(
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

async fn trace_emby_request(req: axum::extract::Request, next: Next) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    // 跳过图片请求减少刷屏，重点追踪 PlaybackInfo、Items、Videos 播放全链路
    let is_image = uri.path().contains("/Images/");
    if !is_image {
        let ua = req.headers().get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("-");
        let range = req.headers().get(header::RANGE).and_then(|v| v.to_str().ok()).unwrap_or("-");
        tracing::info!(
            "➡️  [EMBY REQ] {} {} | UA=\"{}\" | Range={}",
            method, uri, ua, range
        );
    }

    let resp = next.run(req).await;

    if !is_image {
        let status = resp.status();
        let loc = resp.headers().get(header::LOCATION).and_then(|v| v.to_str().ok()).unwrap_or("-");
        tracing::info!(
            "⬅️  [EMBY RESP] {} {} -> HTTP {} | Location={}",
            method, uri, status, loc
        );
    }
    resp
}

async fn get_latest_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Query(q): Query<ItemsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let res = get_user_items(State(state), headers, Path(user_id), Query(q)).await?;
    Ok(Json(res.0["Items"].clone()))
}

/// 继续观看列表 (`GET /Users/:user_id/Items/Resume`)
async fn get_resume_items(
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
async fn get_next_up_items(
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

/// 手动标记已看 (`POST /Users/:user_id/PlayedItems/:item_id`)
async fn mark_item_played(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((path_user_id, item_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    set_item_played_state(&state, &path_user_id, &headers, &item_id, true).await
}

/// 取消已看标记 (`DELETE /Users/:user_id/PlayedItems/:item_id`)
async fn unmark_item_played(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((path_user_id, item_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    set_item_played_state(&state, &path_user_id, &headers, &item_id, false).await
}

async fn set_item_played_state(
    state: &AppState,
    path_user_id: &str,
    headers: &HeaderMap,
    item_id: &str,
    played: bool,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id(state, Some(path_user_id), headers).await?;
    if let Some((mid, tmdb_id, mtype, snum, epnum)) = resolve_item_target(state, item_id).await {
        let _ = upsert_play_history(
            state, &user_id, mid, tmdb_id, &mtype, snum, epnum, 0, 0, played,
        )
        .await;
    }

    Ok(Json(json!({
        "PlaybackPositionTicks": 0,
        "PlayCount": if played { 1 } else { 0 },
        "IsFavorite": false,
        "Played": played
    })))
}

/// 添加收藏 (`POST /Users/:user_id/FavoriteItems/:item_id`)
async fn mark_item_favorite(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((path_user_id, item_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    set_item_favorite_state(&state, &path_user_id, &headers, &item_id, true).await
}

/// 取消收藏 (`DELETE /Users/:user_id/FavoriteItems/:item_id`)
async fn unmark_item_favorite(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((path_user_id, item_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    set_item_favorite_state(&state, &path_user_id, &headers, &item_id, false).await
}

async fn set_item_favorite_state(
    state: &AppState,
    path_user_id: &str,
    headers: &HeaderMap,
    item_id: &str,
    is_fav: bool,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id(state, Some(path_user_id), headers).await?;
    if let Some((mid, tmdb_id, mtype, _, _)) = resolve_item_target(state, item_id).await {
        if is_fav {
            // 先尝试更新已有同 tmdb_id 或 media_item_id 的收藏记录，避免重复插入
            let updated = sqlx::query(
                r#"
                UPDATE user_favorites
                SET media_item_id = $2, tmdb_id = COALESCE($3, tmdb_id)
                WHERE user_id = $1
                  AND (($3::int4 IS NOT NULL AND tmdb_id = $3 AND media_type = $4) OR media_item_id = $2)
                "#,
            )
            .bind(&user_id)
            .bind(mid)
            .bind(tmdb_id)
            .bind(&mtype)
            .execute(&state.db)
            .await
            .map(|r| r.rows_affected())
            .unwrap_or(0);

            if updated == 0 {
                let _ = sqlx::query(
                    "INSERT INTO user_favorites (user_id, media_item_id, tmdb_id, media_type) VALUES ($1, $2, $3, $4)",
                )
                .bind(&user_id)
                .bind(mid)
                .bind(tmdb_id)
                .bind(&mtype)
                .execute(&state.db)
                .await;
            }
        } else {
            let _ = sqlx::query(
                r#"
                DELETE FROM user_favorites
                WHERE user_id = $1
                  AND (($3::int4 IS NOT NULL AND tmdb_id = $3 AND media_type = $4) OR media_item_id = $2)
                "#,
            )
            .bind(&user_id)
            .bind(mid)
            .bind(tmdb_id)
            .bind(&mtype)
            .execute(&state.db)
            .await;
        }
    }

    Ok(Json(json!({
        "PlaybackPositionTicks": 0,
        "PlayCount": 0,
        "IsFavorite": is_fav,
        "Played": false
    })))
}

#[derive(Deserialize, Default)]
struct SessionReportPayload {
    #[serde(rename = "ItemId", alias = "itemId")]
    item_id: Option<String>,
    #[serde(rename = "UserId", alias = "userId")]
    user_id: Option<String>,
    #[serde(rename = "PlaySessionId", alias = "playSessionId")]
    play_session_id: Option<String>,
    #[serde(rename = "PositionTicks", alias = "positionTicks", default)]
    position_ticks: i64,
    #[serde(rename = "RunTimeTicks", alias = "runTimeTicks", default)]
    run_time_ticks: i64,
}

async fn session_playing(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q_report): Query<SessionReportPayload>,
    payload: Option<Json<SessionReportPayload>>,
) -> StatusCode {
    let body = payload.map(|Json(b)| b).unwrap_or(q_report);
    let Some(ref raw_item_id) = body.item_id else {
        return StatusCode::NO_CONTENT;
    };

    let Ok(user_id) = resolve_valid_user_id(&state, body.user_id.as_deref(), &headers).await else {
        return StatusCode::NO_CONTENT;
    };

    let ep_id = raw_item_id
        .strip_prefix("ep_")
        .or_else(|| raw_item_id.strip_prefix("src_"))
        .and_then(|s| s.parse::<i64>().ok());
    let sid = body
        .play_session_id
        .unwrap_or_else(|| format!("sess_{}", raw_item_id));
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("EmbyClient")
        .to_string();

    state.sessions.insert(
        sid.clone(),
        crate::state::PlaybackSession {
            session_id: sid,
            user_id: user_id.clone(),
            device_name: ua.clone(),
            client_name: ua,
            current_episode_id: ep_id,
            position_ticks: body.position_ticks,
            next_ep_prewarmed: false,
            last_activity: std::time::Instant::now(),
        },
    );

    if body.position_ticks > 0 {
        if let Some((mid, tmdb_id, mtype, snum, epnum)) =
            resolve_item_target(&state, raw_item_id).await
        {
            let _ = upsert_play_history(
                &state,
                &user_id,
                mid,
                tmdb_id,
                &mtype,
                snum,
                epnum,
                body.position_ticks,
                body.run_time_ticks,
                false,
            )
            .await;
        }
    }

    StatusCode::NO_CONTENT
}

async fn session_progress(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q_report): Query<SessionReportPayload>,
    payload: Option<Json<SessionReportPayload>>,
) -> StatusCode {
    let body = payload.map(|Json(b)| b).unwrap_or(q_report);
    let Some(ref raw_item_id) = body.item_id else {
        return StatusCode::NO_CONTENT;
    };

    let Ok(user_id) = resolve_valid_user_id(&state, body.user_id.as_deref(), &headers).await else {
        return StatusCode::NO_CONTENT;
    };

    let ep_id = raw_item_id
        .strip_prefix("ep_")
        .or_else(|| raw_item_id.strip_prefix("src_"))
        .and_then(|s| s.parse::<i64>().ok());
    let sid = body
        .play_session_id
        .unwrap_or_else(|| format!("sess_{}", raw_item_id));
    let client_ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // 1. 判定是否已达 85%（触发下一集预热）与 90%（自动标记已看完并推进下一集）
    let ratio = if body.run_time_ticks > 0 {
        body.position_ticks as f64 / body.run_time_ticks as f64
    } else {
        0.0
    };
    let reached_85_percent = if body.run_time_ticks > 0 {
        ratio >= 0.85
    } else {
        body.position_ticks >= 30 * 60 * 10_000_000
    };
    let auto_completed = ratio >= 0.90;

    // 2. 持久化播放进度到 play_histories 表
    if body.position_ticks > 0 {
        if let Some((mid, tmdb_id, mtype, snum, epnum)) =
            resolve_item_target(&state, raw_item_id).await
        {
            let saved_pos = if auto_completed { 0 } else { body.position_ticks };
            let _ = upsert_play_history(
                &state,
                &user_id,
                mid,
                tmdb_id,
                &mtype,
                snum,
                epnum,
                saved_pos,
                body.run_time_ticks,
                auto_completed,
            )
            .await;
        }
    }

    // 3. 更新内存会话并在达到 85% 时静默预热下一集
    let mut should_prewarm_next = false;
    {
        let mut entry = state.sessions.entry(sid.clone()).or_insert_with(|| {
            crate::state::PlaybackSession {
                session_id: sid.clone(),
                user_id: user_id.clone(),
                device_name: client_ua.clone(),
                client_name: client_ua.clone(),
                current_episode_id: ep_id,
                position_ticks: body.position_ticks,
                next_ep_prewarmed: false,
                last_activity: std::time::Instant::now(),
            }
        });
        entry.position_ticks = body.position_ticks;
        entry.last_activity = std::time::Instant::now();
        if reached_85_percent && !entry.next_ep_prewarmed {
            entry.next_ep_prewarmed = true;
            should_prewarm_next = true;
        }
    }

    if should_prewarm_next {
        if let Some(eid) = ep_id {
            let state_clone = state.clone();
            tokio::spawn(async move {
                let cur: Option<(i64, i32, i32)> = sqlx::query_as(
                    "SELECT media_item_id, season_number, episode_number FROM episodes WHERE id = $1",
                )
                .bind(eid)
                .fetch_optional(&state_clone.db)
                .await
                .unwrap_or(None);

                if let Some((mid, snum, epnum)) = cur {
                    let sibling_ids = state_clone.resolve_sibling_media_ids(mid).await;
                    let next_ep: Option<(i64, String, i64)> = sqlx::query_as(
                        r#"
                        SELECT id, alist_path, path_hash
                        FROM episodes
                        WHERE media_item_id = ANY($1) AND season_number = $2 AND episode_number = $3
                        ORDER BY priority_score DESC, id ASC
                        LIMIT 1
                        "#,
                    )
                    .bind(&sibling_ids)
                    .bind(snum)
                    .bind(epnum + 1)
                    .fetch_optional(&state_clone.db)
                    .await
                    .unwrap_or(None);

                    if let Some((next_id, next_path, next_hash)) = next_ep {
                        tracing::info!(
                            "⏭️ [NextEpPrewarm] 当前集 ep_{} 进度已达 85%+，后台提前预热下一集 S{:02}E{:02} (ep_{})",
                            eid,
                            snum,
                            epnum + 1,
                            next_id
                        );
                        let _ = state_clone
                            .resolve_playback_url(next_hash, &next_path, &client_ua)
                            .await;
                    }
                }
            });
        }
    }

    StatusCode::NO_CONTENT
}

async fn session_stopped(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q_report): Query<SessionReportPayload>,
    payload: Option<Json<SessionReportPayload>>,
) -> StatusCode {
    let body = payload.map(|Json(b)| b).unwrap_or(q_report);
    if let Some(ref raw_item_id) = body.item_id {
        if body.position_ticks > 0 {
            if let Ok(user_id) = resolve_valid_user_id(&state, body.user_id.as_deref(), &headers).await {
                if let Some((mid, tmdb_id, mtype, snum, epnum)) =
                    resolve_item_target(&state, raw_item_id).await
                {
                    let auto_completed = body.run_time_ticks > 0
                        && (body.position_ticks as f64 / body.run_time_ticks as f64) >= 0.90;
                    let saved_pos = if auto_completed { 0 } else { body.position_ticks };
                    let _ = upsert_play_history(
                        &state,
                        &user_id,
                        mid,
                        tmdb_id,
                        &mtype,
                        snum,
                        epnum,
                        saved_pos,
                        body.run_time_ticks,
                        auto_completed,
                    )
                    .await;
                }
            }
        }
    }
    if let Some(sid) = body.play_session_id {
        state.sessions.remove(&sid);
    }
    StatusCode::NO_CONTENT
}

/// 解析请求对应的合法 user_id（优先校验显式 user_id -> 其次校验 Token -> 兜底取首个管理员账号）
async fn resolve_valid_user_id(
    state: &AppState,
    explicit_uid: Option<&str>,
    headers: &HeaderMap,
) -> Result<String, (StatusCode, String)> {
    if let Some(uid) = explicit_uid.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let exists: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE id = $1")
            .bind(uid)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
        if let Some(valid_id) = exists {
            return Ok(valid_id);
        }
    }

    if let Some(token) = headers
        .get("x-emby-token")
        .or_else(|| headers.get("x-mediabrowser-token"))
        .and_then(|v| v.to_str().ok())
    {
        let uid_from_token: Option<String> =
            sqlx::query_scalar("SELECT user_id FROM access_tokens WHERE token = $1")
                .bind(token)
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None);
        if let Some(valid_id) = uid_from_token {
            let db = state.db.clone();
            let tok = token.to_string();
            tokio::spawn(async move {
                let _ = sqlx::query("UPDATE access_tokens SET last_used_at = NOW() WHERE token = $1")
                    .bind(tok)
                    .execute(&db)
                    .await;
            });
            return Ok(valid_id);
        }
    }

    let fallback: Option<String> =
        sqlx::query_scalar("SELECT id FROM users ORDER BY is_admin DESC, created_at ASC LIMIT 1")
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);

    if let Some(uid) = fallback {
        return Ok(uid);
    }

    // 若 users 表完全为空，自动创建默认管理员账号
    let new_id = Uuid::new_v4().simple().to_string()[..16].to_string();
    let _ = sqlx::query(
        "INSERT INTO users (id, username, password_hash, is_admin) VALUES ($1, 'admin', '', TRUE) ON CONFLICT DO NOTHING",
    )
    .bind(&new_id)
    .execute(&state.db)
    .await;
    Ok(new_id)
}

/// 将 `media_123` 或 `ep_456` 解析为 `(media_item_id, tmdb_id, media_type, season_number, episode_number)`
async fn resolve_item_target(
    state: &AppState,
    item_id: &str,
) -> Option<(i64, Option<i32>, String, i32, i32)> {
    if let Some(raw_mid) = item_id.strip_prefix("media_") {
        let mid = raw_mid.parse::<i64>().ok()?;
        let (tmdb_id, mtype): (Option<i32>, String) =
            sqlx::query_as("SELECT tmdb_id, media_type FROM media_items WHERE id = $1")
                .bind(mid)
                .fetch_optional(&state.db)
                .await
                .ok()??;
        return Some((mid, tmdb_id, mtype, 0, 1));
    }

    let eid = item_id
        .strip_prefix("ep_")
        .or_else(|| item_id.strip_prefix("src_"))
        .unwrap_or(item_id)
        .parse::<i64>()
        .ok()?;

    let (mid, snum, epnum): (i64, i32, i32) = sqlx::query_as(
        "SELECT media_item_id, season_number, episode_number FROM episodes WHERE id = $1",
    )
    .bind(eid)
    .fetch_optional(&state.db)
    .await
    .ok()??;

    let (tmdb_id, mtype): (Option<i32>, String) =
        sqlx::query_as("SELECT tmdb_id, media_type FROM media_items WHERE id = $1")
            .bind(mid)
            .fetch_optional(&state.db)
            .await
            .ok()??;

    Some((mid, tmdb_id, mtype, snum, epnum))
}

/// 写入或更新播放进度（同时兼容有/无 tmdb_id 的条目，支持目录迁移后按 tmdb_id 自动继承进度）
async fn upsert_play_history(
    state: &AppState,
    user_id: &str,
    media_item_id: i64,
    tmdb_id: Option<i32>,
    media_type: &str,
    season_number: i32,
    episode_number: i32,
    position_ticks: i64,
    duration_ticks: i64,
    is_played: bool,
) -> Result<(), sqlx::Error> {
    let updated = sqlx::query(
        r#"
        UPDATE play_histories
        SET media_item_id = $2,
            tmdb_id = COALESCE($3, tmdb_id),
            position_ticks = $7,
            duration_ticks = CASE WHEN $8 > 0 THEN $8 ELSE duration_ticks END,
            is_played = $9 OR is_played,
            last_played_at = NOW()
        WHERE user_id = $1
          AND season_number = $5
          AND episode_number = $6
          AND (
              ($3::int4 IS NOT NULL AND tmdb_id = $3 AND media_type = $4)
              OR media_item_id = $2
          )
        "#,
    )
    .bind(user_id)
    .bind(media_item_id)
    .bind(tmdb_id)
    .bind(media_type)
    .bind(season_number)
    .bind(episode_number)
    .bind(position_ticks)
    .bind(duration_ticks)
    .bind(is_played)
    .execute(&state.db)
    .await?
    .rows_affected();

    if updated == 0 {
        sqlx::query(
            r#"
            INSERT INTO play_histories (
                user_id, media_item_id, tmdb_id, media_type,
                season_number, episode_number, position_ticks, duration_ticks, is_played, last_played_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
            "#,
        )
        .bind(user_id)
        .bind(media_item_id)
        .bind(tmdb_id)
        .bind(media_type)
        .bind(season_number)
        .bind(episode_number)
        .bind(position_ticks)
        .bind(duration_ticks)
        .bind(is_played)
        .execute(&state.db)
        .await?;
    }

    Ok(())
}

async fn load_user_favorite_keys(state: &AppState, user_id: &str) -> (HashSet<i64>, HashSet<(i32, String)>) {
    let rows: Vec<(Option<i64>, Option<i32>, String)> = sqlx::query_as(
        "SELECT media_item_id, tmdb_id, media_type FROM user_favorites WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut mids = HashSet::new();
    let mut tmdbs = HashSet::new();
    for (mid, tid, mtype) in rows {
        if let Some(m) = mid {
            mids.insert(m);
        }
        if let Some(t) = tid {
            tmdbs.insert((t, mtype));
        }
    }
    (mids, tmdbs)
}

fn is_media_favorited(
    fav_keys: &(HashSet<i64>, HashSet<(i32, String)>),
    media_id: i64,
    tmdb_id: Option<i32>,
    media_type: &str,
) -> bool {
    if fav_keys.0.contains(&media_id) {
        return true;
    }
    if let Some(tid) = tmdb_id {
        if fav_keys.1.contains(&(tid, media_type.to_string())) {
            return true;
        }
    }
    false
}

/// 加载某部影视下所有单集的播放进度 `(season_number, episode_number) -> (position_ticks, duration_ticks, is_played)`
async fn load_media_episode_histories(
    state: &AppState,
    user_id: &str,
    media_id: i64,
    tmdb_id: Option<i32>,
    media_type: &str,
) -> HashMap<(i32, i32), (i64, i64, bool)> {
    let rows: Vec<(i32, i32, i64, i64, bool)> = sqlx::query_as(
        r#"
        SELECT season_number, episode_number, position_ticks, duration_ticks, is_played
        FROM play_histories
        WHERE user_id = $1
          AND (
              ($3::int4 IS NOT NULL AND tmdb_id = $3 AND media_type = $4)
              OR media_item_id = $2
          )
        "#,
    )
    .bind(user_id)
    .bind(media_id)
    .bind(tmdb_id)
    .bind(media_type)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    rows.into_iter()
        .map(|(s, e, pos, dur, played)| ((s, e), (pos, dur, played)))
        .collect()
}

/// 统计某个虚拟媒体库规则当前在库内命中的影视条目数（同 TMDB 自动去重）
pub async fn count_virtual_library_matches(state: &AppState, rule: &VirtualLibraryRule) -> i64 {
    let media_type = rule.media_type.clone().filter(|s| !s.is_empty());
    let categories = if rule.categories.is_empty() {
        None
    } else {
        Some(rule.categories.clone())
    };
    let countries = if rule.countries.is_empty() {
        None
    } else {
        Some(rule.countries.clone())
    };
    let networks = if rule.networks.is_empty() {
        None
    } else {
        Some(rule.networks.clone())
    };
    let genres = if rule.genres.is_empty() {
        None
    } else {
        Some(rule.genres.clone())
    };
    let kw = rule
        .keyword
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{}%", s));

    sqlx::query_scalar(
        r#"
        SELECT COUNT(DISTINCT COALESCE(m.tmdb_id::text || ':' || m.media_type, 'raw:' || m.id::text))
        FROM media_items m
        WHERE m.is_ignored = FALSE
          AND ($1::text IS NULL OR m.media_type = $1)
          AND ($2::text[] IS NULL OR m.category = ANY($2))
          AND ($3::text[] IS NULL OR m.countries && $3)
          AND ($4::text[] IS NULL OR m.networks && $4)
          AND ($5::text[] IS NULL OR m.genres && $5)
          AND ($6::int4 IS NULL OR m.year >= $6)
          AND ($7::int4 IS NULL OR m.year <= $7)
          AND ($8::float4 IS NULL OR m.rating >= $8)
          AND ($9::text IS NULL OR m.title ILIKE $9 OR m.original_title ILIKE $9 OR m.root_dir ILIKE $9)
        "#,
    )
    .bind(media_type)
    .bind(categories)
    .bind(countries)
    .bind(networks)
    .bind(genres)
    .bind(rule.year_min)
    .bind(rule.year_max)
    .bind(rule.min_rating)
    .bind(kw)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0)
}
