use super::auth::resolve_valid_user_id_with_query;
use super::ids::{escape_like, parse_work_id};
use super::items::{ensure_media_accessible, split_metadata_values, DynSql, ItemsQuery};
use super::views::user_access_scope;
use crate::state::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::{HashMap, HashSet}, sync::Arc};

#[derive(sqlx::FromRow)]
struct RecommendationRow {
    id: i64,
    media_type: String,
    title: String,
    year: Option<i32>,
    rating: f32,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
}

#[derive(sqlx::FromRow)]
struct MovieRecommendationRow {
    id: i64,
    title: String,
    year: Option<i32>,
    rating: f32,
    poster_path: Option<String>,
}

#[derive(Deserialize, Default)]
pub(super) struct ListMetaQuery {
    #[serde(rename = "UserId", alias = "userId")]
    user_id: Option<String>,
    #[serde(rename = "SearchTerm", alias = "searchTerm")]
    search_term: Option<String>,
    #[serde(rename = "IsFavorite", alias = "isFavorite")]
    is_favorite: Option<bool>,
    #[serde(rename = "StartIndex", alias = "startIndex", default)]
    start_index: i64,
    #[serde(rename = "Limit", alias = "limit")]
    limit: Option<i64>,
    #[serde(
        rename = "api_key",
        alias = "ApiKey",
        alias = "X-Emby-Token",
        alias = "x-emby-token"
    )]
    api_key: Option<String>,
}

fn empty_query_result(start_index: i64) -> Json<Value> {
    Json(json!({
        "Items": [],
        "TotalRecordCount": 0,
        "StartIndex": start_index.max(0)
    }))
}

fn stable_tag_id(prefix: &str, name: &str) -> String {
    format!(
        "{}_{:016x}",
        prefix,
        xxhash_rust::xxh3::xxh3_64(name.as_bytes())
    )
}

/// `GET /Items/Counts`
pub(super) async fn get_item_counts(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ListMetaQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uid = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    let scope = user_access_scope(&state, &uid).await?;

    let mut sql = DynSql::new(
        "SELECT COUNT(*) FILTER (WHERE m.media_type = 'movie'), COUNT(*) FILTER (WHERE m.media_type = 'tv') FROM media_works m WHERE EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE)",
    );
    if let Some(favorite) = q.is_favorite {
        let user_param = sql.bind(uid.clone())?;
        let favorite_param = sql.bind(favorite)?;
        sql.push(&format!(" AND (EXISTS (SELECT 1 FROM user_favorites uf WHERE uf.user_id = {user_param} AND (uf.work_id = m.id OR (m.tmdb_id IS NOT NULL AND uf.tmdb_id = m.tmdb_id AND uf.media_type = m.media_type))) = {favorite_param})"));
    }
    scope.push_sql_predicate(&mut sql)?;
    let (movie_count, series_count): (i64, i64) = sqlx::query_as_with(&sql.sql, sql.args)
        .fetch_one(&state.db)
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let mut sql = DynSql::new(
        "SELECT COUNT(DISTINCT (f.work_id, sf.season_number, sf.episode_number)) FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id JOIN media_works m ON m.id = f.work_id WHERE f.is_ignored = FALSE AND m.media_type = 'tv'",
    );
    if let Some(favorite) = q.is_favorite {
        let user_param = sql.bind(uid.clone())?;
        let favorite_param = sql.bind(favorite)?;
        sql.push(&format!(" AND (EXISTS (SELECT 1 FROM user_favorites uf WHERE uf.user_id = {user_param} AND (uf.work_id = m.id OR (m.tmdb_id IS NOT NULL AND uf.tmdb_id = m.tmdb_id AND uf.media_type = m.media_type))) = {favorite_param})"));
    }
    scope.push_sql_predicate(&mut sql)?;
    let episode_count: i64 = sqlx::query_scalar_with(&sql.sql, sql.args)
        .fetch_one(&state.db)
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    let box_set_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM collections c WHERE c.user_id = $1 OR c.user_id IS NULL OR EXISTS (SELECT 1 FROM users u WHERE u.id = $1 AND u.is_admin = TRUE)"
    ).bind(&uid).fetch_one(&state.db).await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    Ok(Json(json!({
        "MovieCount": movie_count,
        "SeriesCount": series_count,
        "EpisodeCount": episode_count,
        "GameCount": 0,
        "ArtistCount": 0,
        "ProgramCount": 0,
        "TrailerCount": 0,
        "SongCount": 0,
        "AlbumCount": 0,
        "MusicVideoCount": 0,
        "BoxSetCount": box_set_count,
        "BookCount": 0,
        "ItemCount": movie_count + series_count + episode_count + box_set_count
    })))
}

async fn recommendation_rows(
    state: &AppState,
    user_id: &str,
    similar: Option<(i64, &str, &str)>,
    q: &ListMetaQuery,
    default_limit: i64,
) -> Result<(Vec<RecommendationRow>, i64), (StatusCode, String)> {
    let scope = user_access_scope(state, user_id).await?;
    let build = |select: &str| -> Result<DynSql, (StatusCode, String)> {
        let mut sql = DynSql::new(&format!(
            "{select} FROM media_works m WHERE EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE)"
        ));
        scope.push_sql_predicate(&mut sql)?;
        if let Some((id, media_type, category)) = similar {
            sql.push_bind_push(" AND m.id <> ", id, "")?;
            sql.push_bind_push(" AND m.media_type = ", media_type.to_string(), "")?;
            sql.push_bind_push(" AND m.category = ", category.to_string(), "")?;
        }
        if let Some(term) = q
            .search_term
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            sql.push_bind_push(
                " AND m.title ILIKE ",
                format!("%{}%", escape_like(term)),
                "",
            )?;
        }
        Ok(sql)
    };
    let count = build("SELECT COUNT(*)")?;
    let total = sqlx::query_scalar_with(&count.sql, count.args)
        .fetch_one(&state.db)
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let mut sql = build(
        "SELECT m.id, m.media_type, m.title, m.year, m.rating, m.poster_path, m.backdrop_path",
    )?;
    sql.push(" ORDER BY m.rating DESC, m.year DESC NULLS LAST, m.id DESC");
    sql.push_bind_push(" LIMIT ", q.limit.unwrap_or(default_limit).clamp(1, 50), "")?;
    sql.push_bind_push(" OFFSET ", q.start_index.max(0), "")?;
    let rows = sqlx::query_as_with(&sql.sql, sql.args)
        .fetch_all(&state.db)
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok((rows, total))
}

/// `GET /Items/:item_id/Similar` (含 `/Movies/:id/Similar`、`/Shows/:id/Similar` 别名)
pub(super) async fn get_similar_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<String>,
    Query(q): Query<ListMetaQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uid = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;

    let Some(wid) = parse_work_id(&item_id) else {
        return Ok(empty_query_result(q.start_index));
    };

    let (mtype, _, category) = ensure_media_accessible(&state, &uid, wid).await?;
    let cfg = state.config.load();
    let (rows, total) =
        recommendation_rows(&state, &uid, Some((wid, &mtype, &category)), &q, 12).await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(
            |RecommendationRow {
                 id,
                 media_type: mt,
                 title,
                 year,
                 rating,
                 poster_path: poster,
                 backdrop_path: backdrop,
             }| {
                let emby_type = if mt == "tv" { "Series" } else { "Movie" };
                let mut image_tags = serde_json::Map::new();
                if poster.is_some() {
                    image_tags.insert(
                        "Primary".into(),
                        json!(super::images::image_path_tag(poster.as_deref())),
                    );
                }
                let backdrop_tags = backdrop
                    .as_deref()
                    .map(|path| super::images::image_path_tag(Some(path)))
                    .into_iter()
                    .collect::<Vec<_>>();
                json!({
                    "Id": format!("work_{}", id),
                    "Name": title,
                    "ServerId": cfg.server_id,
                    "Type": emby_type,
                    "MediaType": "Video",
                    "IsFolder": mt == "tv",
                    "ProductionYear": year,
                    "CommunityRating": (rating * 10.0).round() / 10.0,
                    "ImageTags": Value::Object(image_tags),
                    "BackdropImageTags": backdrop_tags
                })
            },
        )
        .collect();

    Ok(Json(json!({
        "Items": items,
        "TotalRecordCount": total,
        "StartIndex": q.start_index.max(0)
    })))
}

async fn cached_tag_lookup_map(
    state: &AppState,
    col_expr: &str,
    prefix: &str,
) -> Result<Arc<HashMap<String, String>>, (StatusCode, String)> {
    let key = format!("{prefix}:{col_expr}");
    let db = state.db.clone();
    let sql = format!(
        "SELECT DISTINCT val FROM media_works m, unnest({col_expr}) AS val WHERE val IS NOT NULL AND val <> '' AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE)"
    );
    let prefix_owned = prefix.to_string();
    state
        .caches
        .metadata_tag_map_cache
        .try_get_with(key, async move {
            let names: Vec<String> = sqlx::query_scalar(&sql)
                .fetch_all(&db)
                .await
                .map_err(|error| error.to_string())?;
            let mut map = HashMap::with_capacity(names.len());
            for name in names {
                map.insert(stable_tag_id(&prefix_owned, &name), name);
            }
            Ok::<_, String>(Arc::new(map))
        })
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

#[cfg(test)]
fn build_tag_lookup_map(names: &[String], prefix: &str) -> HashMap<String, String> {
    names
        .iter()
        .map(|name| (stable_tag_id(prefix, name), name.clone()))
        .collect()
}

fn matching_metadata_names(
    tag_map: &HashMap<String, String>,
    ids: &str,
    prefix: &str,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for value in split_metadata_values(Some(ids)) {
        let resolved = tag_map
            .get(&value)
            .or_else(|| tag_map.get(&stable_tag_id(prefix, &value)));
        if let Some(name) = resolved {
            if seen.insert(name.as_str()) {
                out.push(name.clone());
            }
        }
    }
    out
}

fn normalize_metadata_names(tag_map: &HashMap<String, String>, values: &str) -> String {
    split_metadata_values(Some(values))
        .into_iter()
        .map(|value| tag_map.get(&value).cloned().unwrap_or(value))
        .collect::<Vec<_>>()
        .join("|")
}

pub(in crate::web::emby_api) async fn normalize_metadata_query(
    state: &AppState,
    _user_id: &str,
    query: &ItemsQuery,
) -> Result<ItemsQuery, (StatusCode, String)> {
    let mut query = query.clone();
    for (raw, column, prefix) in [
        (&mut query.genres, "m.genres", "genre"),
        (&mut query.studios, "m.networks", "studio"),
        (&mut query.tags, TAG_NAMES, "tag"),
        (&mut query.persons, PERSON_NAMES, "person"),
    ] {
        if let Some(values) = raw {
            if split_metadata_values(Some(values))
                .iter()
                .any(|v| v.starts_with(&format!("{prefix}_")))
            {
                let tag_map = cached_tag_lookup_map(state, column, prefix).await?;
                *values = normalize_metadata_names(&tag_map, values);
            }
        }
    }
    Ok(query)
}

pub(in crate::web::emby_api) async fn resolve_metadata_ids(
    state: &AppState,
    _user_id: &str,
    ids: Option<&str>,
    col_expr: &str,
    prefix: &str,
) -> Result<Option<Vec<String>>, (StatusCode, String)> {
    let Some(ids) = ids.filter(|ids| !split_metadata_values(Some(ids)).is_empty()) else {
        return Ok(None);
    };
    let values = split_metadata_values(Some(ids));
    if !values
        .iter()
        .any(|value| value.starts_with(&format!("{prefix}_")))
    {
        return Ok(Some(values));
    }
    let tag_map = cached_tag_lookup_map(state, col_expr, prefix).await?;
    Ok(Some(matching_metadata_names(&tag_map, ids, prefix)))
}

async fn query_distinct_array_column(
    state: &AppState,
    user_id: &str,
    col_expr: &str,
    emby_type: &str,
    id_prefix: &str,
    q: &ListMetaQuery,
) -> Result<Json<Value>, (StatusCode, String)> {
    let scope = user_access_scope(state, user_id).await?;
    let mut sql = DynSql::new(&format!(
        "SELECT val, COUNT(*) OVER() AS total FROM (SELECT DISTINCT val FROM media_works m, unnest({col_expr}) AS val WHERE val IS NOT NULL AND val <> '' AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id=m.id AND f.is_ignored=FALSE)"
    ));
    scope.push_sql_predicate(&mut sql)?;
    if let Some(term) = q
        .search_term
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sql.push_bind_push(" AND val ILIKE ", format!("%{}%", escape_like(term)), "")?;
    }
    sql.push(") names ORDER BY val ASC");
    sql.push_pagination(q.limit.unwrap_or(200).clamp(1, 1000), q.start_index.max(0))?;
    let rows: Vec<(String, i64)> = sqlx::query_as_with(&sql.sql, sql.args)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let total = rows.first().map(|(_, total)| *total).unwrap_or(0);
    let cfg = state.config.load();
    let items = rows
        .into_iter()
        .map(|(name, _)| {
            json!({
                "Name": name, "ServerId": cfg.server_id,
                "Id": stable_tag_id(id_prefix, &name), "Type": emby_type
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(
        json!({"Items": items, "TotalRecordCount": total, "StartIndex": q.start_index.max(0)}),
    ))
}

async fn metadata_by_name(
    state: &AppState,
    user_id: &str,
    name: &str,
    col_expr: &str,
    emby_type: &str,
    prefix: &str,
) -> Result<Json<Value>, (StatusCode, String)> {
    let scope = user_access_scope(state, user_id).await?;
    let mut exact = DynSql::new(&format!(
        "SELECT EXISTS(SELECT 1 FROM media_works m, unnest({col_expr}) AS val WHERE val = "
    ));
    let name_param = exact.bind(name.to_string())?;
    exact.push(&format!("{name_param} AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id=m.id AND f.is_ignored=FALSE)"));
    scope.push_sql_predicate(&mut exact)?;
    exact.push(")");
    let exists: bool = sqlx::query_scalar_with(&exact.sql, exact.args)
        .fetch_one(&state.db)
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let resolved_name = if exists {
        name.to_string()
    } else {
        let tag_map = cached_tag_lookup_map(state, col_expr, prefix).await?;
        tag_map
            .get(name)
            .cloned()
            .ok_or((StatusCode::NOT_FOUND, "元数据条目不存在".to_string()))?
    };
    let name = resolved_name.as_str();
    let mut sql = DynSql::new(
        "SELECT EXISTS(SELECT 1 FROM media_works m JOIN sync_folders f ON f.work_id=m.id AND f.is_ignored=FALSE WHERE ",
    );
    let param = sql.bind(name.to_string())?;
    sql.push(&format!("{param} = ANY({col_expr})"));
    scope.push_sql_predicate(&mut sql)?;
    sql.push(")");
    let exists: bool = sqlx::query_scalar_with(&sql.sql, sql.args)
        .fetch_one(&state.db)
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "元数据条目不存在".to_string()));
    }
    Ok(Json(
        json!({"Name": name, "ServerId": state.config.load().server_id,
        "Id": stable_tag_id(prefix, name), "Type": emby_type}),
    ))
}

pub(in crate::web::emby_api) const PERSON_NAMES: &str = "ARRAY(SELECT p->>'name' FROM jsonb_array_elements(COALESCE(m.tmdb_metadata->'cast', '[]') || COALESCE(m.tmdb_metadata->'directors', '[]')) p)";
const TAG_NAMES: &str = "ARRAY(SELECT val FROM (SELECT unnest(COALESCE(m.tags, ARRAY[]::text[])) AS val UNION SELECT k->>'name' AS val FROM jsonb_array_elements(COALESCE(m.tmdb_metadata->'keyword_items', '[]'::jsonb)) k) tags WHERE val IS NOT NULL AND val <> '')";
const RATING_NAMES: &str = "ARRAY(SELECT c->>'rating' FROM jsonb_array_elements(COALESCE(m.tmdb_metadata->'certifications', '[]')) c)";

/// `GET /Genres`
pub(super) async fn get_genres(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ListMetaQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uid = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    query_distinct_array_column(&state, &uid, "m.genres", "Genre", "genre", &q).await
}

/// `GET /Genres/:name`
pub(super) async fn get_genre_by_name(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(q): Query<ListMetaQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uid = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    metadata_by_name(&state, &uid, &name, "m.genres", "Genre", "genre").await
}

pub(super) async fn get_studio_by_name(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(q): Query<ListMetaQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uid = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    metadata_by_name(&state, &uid, &name, "m.networks", "Studio", "studio").await
}

pub(super) async fn get_person_by_name(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(q): Query<ListMetaQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uid = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    metadata_by_name(&state, &uid, &name, PERSON_NAMES, "Person", "person").await
}

pub(super) async fn get_artist_by_name(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(_name): Path<String>,
    Query(q): Query<ListMetaQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    resolve_valid_user_id_with_query(&state, q.user_id.as_deref(), &headers, q.api_key.as_deref())
        .await?;
    Err((StatusCode::NOT_FOUND, "艺术家不存在".to_string()))
}

/// `GET /Studios`
pub(super) async fn get_studios(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ListMetaQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uid = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    query_distinct_array_column(&state, &uid, "m.networks", "Studio", "studio", &q).await
}

/// `GET /Tags`
pub(super) async fn get_tags(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ListMetaQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uid = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    query_distinct_array_column(&state, &uid, TAG_NAMES, "Tag", "tag", &q).await
}

/// `GET /Years`
pub(super) async fn get_years(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ListMetaQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uid = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    let scope = user_access_scope(&state, &uid).await?;
    let mut sql = DynSql::new(
        "SELECT year, COUNT(*) OVER() AS total FROM (SELECT DISTINCT m.year AS year FROM media_works m WHERE m.year IS NOT NULL AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE)",
    );
    scope.push_sql_predicate(&mut sql)?;
    if let Some(term) = q
        .search_term
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sql.push_bind_push(
            " AND m.year::text ILIKE ",
            format!("%{}%", escape_like(term)),
            "",
        )?;
    }
    sql.push(") years ORDER BY year DESC");
    sql.push_pagination(q.limit.unwrap_or(200).clamp(1, 500), q.start_index.max(0))?;
    let years: Vec<(i32, i64)> = sqlx::query_as_with(&sql.sql, sql.args)
        .fetch_all(&state.db)
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let total = years.first().map(|(_, total)| *total).unwrap_or(0);
    let cfg = state.config.load();
    let items: Vec<Value> = years.into_iter().map(|(year, _)| json!({"Name": year.to_string(), "Id": format!("year_{year}"), "Type": "Year", "ServerId": cfg.server_id})).collect();
    Ok(Json(
        json!({"Items": items, "TotalRecordCount": total, "StartIndex": q.start_index.max(0)}),
    ))
}

/// 列出演职人员；当前媒体库没有音乐艺术家条目。
pub(super) async fn get_empty_people_or_artists(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: axum::extract::OriginalUri,
    Query(q): Query<ListMetaQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uid = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    if uri.path().ends_with("/Persons") {
        query_distinct_array_column(&state, &uid, PERSON_NAMES, "Person", "person", &q).await
    } else {
        Ok(empty_query_result(q.start_index))
    }
}

/// `GET /OfficialRatings`
pub(super) async fn get_official_ratings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ListMetaQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uid = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    let scope = user_access_scope(&state, &uid).await?;
    let mut sql = DynSql::new(&format!(
        "SELECT val, COUNT(*) OVER() AS total FROM (SELECT DISTINCT val FROM media_works m, unnest({RATING_NAMES}) AS val WHERE val IS NOT NULL AND val <> '' AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id=m.id AND f.is_ignored=FALSE)"
    ));
    scope.push_sql_predicate(&mut sql)?;
    if let Some(term) = q
        .search_term
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sql.push_bind_push(" AND val ILIKE ", format!("%{}%", escape_like(term)), "")?;
    }
    sql.push(") names ORDER BY val ASC");
    sql.push_pagination(q.limit.unwrap_or(200).clamp(1, 1000), q.start_index.max(0))?;
    let rows: Vec<(String, i64)> = sqlx::query_as_with(&sql.sql, sql.args)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let total = rows.first().map(|(_, total)| *total).unwrap_or(0);
    let items = rows
        .into_iter()
        .map(|(name, _)| json!({"Name": name, "Value": name}))
        .collect::<Vec<_>>();
    Ok(Json(
        json!({"Items": items, "TotalRecordCount": total, "StartIndex": q.start_index.max(0)}),
    ))
}

/// `GET /Localization/ParentalRatings`
pub(super) async fn get_parental_ratings() -> Json<Value> {
    Json(json!([
        { "Name": "G", "Value": 1 },
        { "Name": "PG", "Value": 5 },
        { "Name": "PG-13", "Value": 7 },
        { "Name": "R", "Value": 9 },
        { "Name": "NC-17", "Value": 10 },
        { "Name": "TV-Y", "Value": 1 },
        { "Name": "TV-G", "Value": 1 },
        { "Name": "TV-PG", "Value": 5 },
        { "Name": "TV-14", "Value": 8 },
        { "Name": "TV-MA", "Value": 9 }
    ]))
}

/// `GET /Users/:user_id/Suggestions`
pub(super) async fn get_user_suggestions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Query(q): Query<ListMetaQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uid =
        resolve_valid_user_id_with_query(&state, Some(&user_id), &headers, q.api_key.as_deref())
            .await?;
    let cfg = state.config.load();
    let (rows, total) = recommendation_rows(&state, &uid, None, &q, 20).await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(
            |RecommendationRow {
                 id,
                 media_type: mt,
                 title,
                 year,
                 rating,
                 poster_path: poster,
                 backdrop_path: backdrop,
             }| {
                let emby_type = if mt == "tv" { "Series" } else { "Movie" };
                let mut image_tags = serde_json::Map::new();
                if poster.is_some() {
                    image_tags.insert(
                        "Primary".into(),
                        json!(super::images::image_path_tag(poster.as_deref())),
                    );
                }
                let backdrop_tags = backdrop
                    .as_deref()
                    .map(|path| super::images::image_path_tag(Some(path)))
                    .into_iter()
                    .collect::<Vec<_>>();
                json!({
                    "Id": format!("work_{}", id),
                    "Name": title,
                    "ServerId": cfg.server_id,
                    "Type": emby_type,
                    "MediaType": "Video",
                    "IsFolder": mt == "tv",
                    "ProductionYear": year,
                    "CommunityRating": (rating * 10.0).round() / 10.0,
                    "ImageTags": Value::Object(image_tags),
                    "BackdropImageTags": backdrop_tags
                })
            },
        )
        .collect();

    Ok(Json(json!({
        "Items": items,
        "TotalRecordCount": total,
        "StartIndex": q.start_index.max(0)
    })))
}

/// `GET /Movies/Recommendations`
pub(super) async fn get_movie_recommendations(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ListMetaQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uid = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    let scope = user_access_scope(&state, &uid).await?;
    let cfg = state.config.load();

    let mut sql = DynSql::new(
        "SELECT m.id, m.title, m.year, m.rating, m.poster_path FROM media_works m WHERE m.media_type = 'movie' AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE)",
    );
    scope.push_sql_predicate(&mut sql)?;
    sql.push(" ORDER BY m.rating DESC, m.year DESC NULLS LAST, m.id DESC LIMIT 12");
    let rows: Vec<MovieRecommendationRow> = sqlx::query_as_with(&sql.sql, sql.args)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if rows.is_empty() {
        return Ok(Json(json!([])));
    }

    let items: Vec<Value> = rows
        .into_iter()
        .map(
            |MovieRecommendationRow {
                 id,
                 title,
                 year,
                 rating,
                 poster_path: poster,
             }| {
                let mut image_tags = serde_json::Map::new();
                if poster.is_some() {
                    image_tags.insert(
                        "Primary".into(),
                        json!(super::images::image_path_tag(poster.as_deref())),
                    );
                }
                json!({
                    "Id": format!("work_{}", id),
                    "Name": title,
                    "ServerId": cfg.server_id,
                    "Type": "Movie",
                    "MediaType": "Video",
                    "ProductionYear": year,
                    "CommunityRating": (rating * 10.0).round() / 10.0,
                    "ImageTags": Value::Object(image_tags)
                })
            },
        )
        .collect();

    Ok(Json(json!([
        {
            "CategoryId": "rec_top_movies",
            "RecommendationType": "SimilarToRecentlyPlayed",
            "BaselineItemName": "高分精选",
            "Items": items
        }
    ])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_counts_query_accepts_favorite_filter_in_both_casings() {
        let upper: ListMetaQuery = serde_json::from_value(json!({"IsFavorite": true})).unwrap();
        let lower: ListMetaQuery = serde_json::from_value(json!({"isFavorite": false})).unwrap();
        assert_eq!(upper.is_favorite, Some(true));
        assert_eq!(lower.is_favorite, Some(false));
    }

    #[test]
    fn metadata_names_accept_plain_values_and_generated_ids_together() {
        for prefix in ["genre", "studio", "tag"] {
            let names = vec!["A".to_string(), "B".to_string()];
            let tag_map = build_tag_lookup_map(&names, prefix);
            let values = format!("A|{},unknown", stable_tag_id(prefix, "B"));
            assert_eq!(
                normalize_metadata_names(&tag_map, &values),
                "A|B|unknown"
            );
            assert_eq!(
                matching_metadata_names(&tag_map, &values, prefix),
                vec!["A", "B"]
            );
        }
    }

    #[test]
    fn metadata_ids_match_list_ids_and_unknown_ids_stay_empty() {
        let names = vec!["剧情".to_string(), "喜剧".to_string()];
        let tag_map = build_tag_lookup_map(&names, "genre");
        let ids = format!("{}|unknown", stable_tag_id("genre", "剧情"));
        assert_eq!(
            matching_metadata_names(&tag_map, &ids, "genre"),
            vec!["剧情"]
        );
        assert!(matching_metadata_names(&tag_map, "unknown", "genre").is_empty());
    }

    #[test]
    fn metadata_lookup_map_resolves_beyond_5000_entries_without_truncation() {
        let names: Vec<String> = (0..5500).map(|i| format!("演员_{i:04}")).collect();
        let tag_map = build_tag_lookup_map(&names, "person");
        let tail_id = stable_tag_id("person", "演员_5499");
        assert_eq!(
            matching_metadata_names(&tag_map, &tail_id, "person"),
            vec!["演员_5499"]
        );
        assert_eq!(
            normalize_metadata_names(&tag_map, &tail_id),
            "演员_5499"
        );
    }
}
