use super::auth::{resolve_valid_user_and_admin_with_query, resolve_valid_user_id_with_query};
use super::ids::{escape_like, parse_episode_id, parse_season_id, parse_work_id};
use super::images::image_path_tag;
use crate::{smart_views::VirtualLibraryRule, state::AppState};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgConnection;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
type ApiError = (StatusCode, String);
fn db_error(e: sqlx::Error) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

#[derive(Deserialize, Default)]
pub(super) struct PlaylistQuery {
    #[serde(rename = "Name", alias = "name")]
    name: Option<String>,
    #[serde(rename = "Ids", alias = "ids")]
    ids: Option<String>,
    #[serde(rename = "EntryIds", alias = "entryIds")]
    entry_ids: Option<String>,
    #[serde(rename = "UserId", alias = "userId")]
    user_id: Option<String>,
    #[serde(rename = "MediaType", alias = "mediaType")]
    media_type: Option<String>,
    #[serde(rename = "ParentId", alias = "parentId")]
    parent_id: Option<String>,
    #[serde(rename = "IsLocked", alias = "isLocked")]
    is_locked: Option<bool>,
    #[serde(rename = "StartIndex", alias = "startIndex", default)]
    start_index: usize,
    #[serde(rename = "Limit", alias = "limit")]
    limit: Option<usize>,
    #[serde(
        rename = "api_key",
        alias = "ApiKey",
        alias = "X-Emby-Token",
        alias = "x-emby-token"
    )]
    api_key: Option<String>,
}
fn parse_csv_ids(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn delete_target(raw: &str) -> Result<(bool, i64), ApiError> {
    let collection = raw.starts_with("col_");
    let prefix = if collection {
        "col_"
    } else if raw.starts_with("pl_") {
        "pl_"
    } else {
        return Err((StatusCode::BAD_REQUEST, "无效的列表 ID".into()));
    };
    Ok((collection, parse_prefixed_id(raw, prefix)?))
}
fn parse_prefixed_id(raw: &str, prefix: &str) -> Result<i64, ApiError> {
    raw.strip_prefix(prefix)
        .unwrap_or(raw)
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or((StatusCode::BAD_REQUEST, "无效的列表 ID".into()))
}
fn body_ids(query: Option<&str>, body: &Value, key: &str) -> Vec<String> {
    if let Some(value) = query {
        return parse_csv_ids(Some(value));
    }
    match body.get(key) {
        Some(Value::String(value)) => parse_csv_ids(Some(value)),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .flat_map(|s| parse_csv_ids(Some(s)))
            .collect(),
        _ => Vec::new(),
    }
}
fn text<'a>(query: &'a Option<String>, body: &'a Value, key: &str) -> Option<&'a str> {
    query
        .as_deref()
        .or_else(|| body.get(key).and_then(Value::as_str))
}
fn list_access(owner: Option<&str>, user: &str, admin: bool, write: bool) -> bool {
    if write {
        owner == Some(user) || admin
    } else {
        owner.is_none() || owner == Some(user) || admin
    }
}
async fn admin(state: &AppState, uid: &str) -> Result<bool, ApiError> {
    sqlx::query_scalar("SELECT is_admin FROM users WHERE id=$1")
        .bind(uid)
        .fetch_one(&state.db)
        .await
        .map_err(db_error)
}
async fn authorize(
    conn: &mut PgConnection,
    id: i64,
    uid: &str,
    is_admin: bool,
    collection: bool,
    lock: bool,
    write: bool,
) -> Result<Value, ApiError> {
    let table = if collection {
        "collections"
    } else {
        "playlists"
    };
    let sql = format!(
        "SELECT to_jsonb(t) AS data FROM {table} t WHERE id=$1{}",
        if lock { " FOR UPDATE" } else { "" }
    );
    let row: Option<Value> = sqlx::query_scalar(&sql)
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
    let row = row.ok_or((StatusCode::NOT_FOUND, "列表不存在".into()))?;
    if !list_access(row["user_id"].as_str(), uid, is_admin, write) {
        return Err((StatusCode::FORBIDDEN, "无权访问该列表".into()));
    }
    Ok(row)
}
fn target(raw: &str) -> Result<(i64, Option<i32>, Option<i32>), ApiError> {
    if let Some((w, s, e)) = parse_episode_id(raw).filter(|t| t.0 > 0) {
        return Ok((w, Some(s), Some(e)));
    }
    if let Some((w, s)) = parse_season_id(raw).filter(|t| t.0 > 0) {
        return Ok((w, Some(s), None));
    }
    if let Some(w) = parse_work_id(raw).filter(|w| *w > 0) {
        return Ok((w, None, None));
    }
    Err((StatusCode::BAD_REQUEST, format!("无效的媒体 ID: {raw}")))
}

/// 同一批请求共用权限范围，虚拟视图按规则批量匹配作品。
async fn allowed_work_ids(
    state: &AppState,
    uid: &str,
    ids: &[i64],
) -> Result<HashSet<i64>, ApiError> {
    let scope = super::views::user_access_scope(state, uid).await?;
    if scope.allowed_categories.is_none()
        || scope
            .allowed_categories
            .as_ref()
            .is_some_and(|c| c.iter().any(|s| s == "*"))
    {
        return Ok(ids.iter().copied().collect());
    }
    let mut predicates = vec!["m.category=ANY($2)".to_string()];
    let mut rules = Vec::with_capacity(scope.virtual_libraries.len());
    for (index, library) in scope.virtual_libraries.iter().enumerate() {
        let rule: VirtualLibraryRule = serde_json::from_value(library.filter_rules.clone())
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        rule.validate()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        let bind = format!("${}::jsonb", index + 3);
        predicates.push(rule.sql_predicate(&bind));
        rules.push(library.filter_rules.clone());
    }
    let sql = format!(
        "SELECT m.id FROM media_works m WHERE m.id=ANY($1) AND ({})",
        predicates.join(" OR ")
    );
    let mut query = sqlx::query_scalar::<_, i64>(&sql)
        .bind(ids)
        .bind(scope.allowed_categories.as_deref().unwrap_or(&[]));
    for rule in rules {
        query = query.bind(rule);
    }
    Ok(query
        .fetch_all(&state.db)
        .await
        .map_err(db_error)?
        .into_iter()
        .collect())
}

#[derive(sqlx::FromRow)]
struct ResolvedPlaylistRow {
    n: i64,
    title: String,
    media_type: String,
    year: Option<i32>,
    poster_path: Option<String>,
    season_name: Option<String>,
    season_poster: Option<String>,
    episode_title: Option<String>,
    still_path: Option<String>,
    runtime: Option<i32>,
    position_ticks: i64,
    duration_ticks: i64,
    played: bool,
    play_count: i32,
    last_played: Option<chrono::DateTime<chrono::Utc>>,
    favorite: bool,
    present: bool,
    episodes_played: bool,
}

/// 一条批查询解析作品、季、集以及用户收藏/播放状态；保持调用方条目顺序。
async fn resolve_items(
    state: &AppState,
    uid: &str,
    ids: &[String],
    strict: bool,
) -> Result<Vec<Value>, ApiError> {
    let targets = ids
        .iter()
        .map(|s| target(s))
        .collect::<Result<Vec<_>, _>>()?;
    let works: Vec<i64> = targets.iter().map(|t| t.0).collect();
    let seasons: Vec<Option<i32>> = targets.iter().map(|t| t.1).collect();
    let episodes: Vec<Option<i32>> = targets.iter().map(|t| t.2).collect();
    let allowed = allowed_work_ids(state, uid, &works).await?;
    let rows: Vec<ResolvedPlaylistRow> = sqlx::query_as(r#"
        SELECT x.n::bigint AS n, w.title, w.media_type, w.year, w.poster_path,
               s.name AS season_name, s.poster_path AS season_poster, e.title AS episode_title, e.still_path,
               e.runtime, h.position_ticks, h.duration_ticks, h.played, h.play_count, h.last_played,
               EXISTS(SELECT 1 FROM user_favorites uf WHERE uf.user_id=$4 AND
                  (uf.work_id=w.id OR (w.tmdb_id IS NOT NULL AND uf.tmdb_id=w.tmdb_id AND uf.media_type=w.media_type))) AS favorite,
               EXISTS(SELECT 1 FROM sync_folders f WHERE f.work_id=w.id AND NOT f.is_ignored AND
                  (x.s IS NULL OR EXISTS(SELECT 1 FROM sync_files sf WHERE sf.folder_id=f.id AND sf.season_number=x.s AND (x.e IS NULL OR sf.episode_number=x.e)))) AS present,
               CASE WHEN x.s IS NOT NULL AND x.e IS NULL AND w.media_type='tv' THEN
                 EXISTS(SELECT 1 FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id
                   WHERE f.work_id=w.id AND NOT f.is_ignored AND sf.season_number=x.s AND sf.episode_number IS NOT NULL)
                 AND NOT EXISTS(SELECT 1 FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id
                   WHERE f.work_id=w.id AND NOT f.is_ignored AND sf.season_number=x.s AND sf.episode_number IS NOT NULL
                   AND NOT EXISTS(SELECT 1 FROM play_histories ph WHERE ph.user_id=$4 AND
                     (ph.work_id=w.id OR (w.tmdb_id IS NOT NULL AND ph.tmdb_id=w.tmdb_id AND ph.media_type=w.media_type))
                     AND ph.season_number=sf.season_number AND ph.episode_number=sf.episode_number AND ph.is_played))
                 WHEN x.s IS NULL AND w.media_type='tv' THEN
                 EXISTS(SELECT 1 FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id
                   WHERE f.work_id=w.id AND NOT f.is_ignored AND sf.episode_number IS NOT NULL)
                 AND NOT EXISTS(SELECT 1 FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id
                   WHERE f.work_id=w.id AND NOT f.is_ignored AND sf.episode_number IS NOT NULL
                   AND NOT EXISTS(SELECT 1 FROM play_histories ph WHERE ph.user_id=$4 AND
                     (ph.work_id=w.id OR (w.tmdb_id IS NOT NULL AND ph.tmdb_id=w.tmdb_id AND ph.media_type=w.media_type))
                     AND ph.season_number=sf.season_number AND ph.episode_number=sf.episode_number AND ph.is_played))
                 ELSE FALSE END AS episodes_played
        FROM UNNEST($1::bigint[], $2::int[], $3::int[]) WITH ORDINALITY x(w,s,e,n)
        JOIN media_works w ON w.id=x.w
        LEFT JOIN work_seasons s ON s.work_id=x.w AND s.season_number=x.s
        LEFT JOIN work_episodes e ON e.work_id=x.w AND e.season_number=x.s AND e.episode_number=x.e
        LEFT JOIN LATERAL (
           SELECT COALESCE(MAX(position_ticks),0)::bigint AS position_ticks, COALESCE(MAX(duration_ticks),0)::bigint AS duration_ticks,
              COALESCE(BOOL_AND(is_played),FALSE) AS played, COALESCE(MAX(play_count),0)::int AS play_count, MAX(last_played_at) AS last_played
           FROM play_histories ph WHERE ph.user_id=$4 AND
             (ph.work_id=w.id OR (w.tmdb_id IS NOT NULL AND ph.tmdb_id=w.tmdb_id AND ph.media_type=w.media_type))
             AND (x.s IS NULL OR ph.season_number=x.s) AND (x.e IS NULL OR ph.episode_number=x.e)
        ) h ON TRUE ORDER BY x.n
    "#).bind(&works).bind(&seasons).bind(&episodes).bind(uid).fetch_all(&state.db).await.map_err(db_error)?;
    let cfg = state.config.load();
    let mut resolved = HashMap::new();
    for row in rows {
        let ResolvedPlaylistRow {
            n,
            title,
            media_type,
            year,
            poster_path,
            season_name,
            season_poster,
            episode_title,
            still_path,
            runtime,
            position_ticks,
            duration_ticks,
            played: history_played,
            play_count,
            last_played,
            favorite,
            present,
            episodes_played,
        } = row;
        let index = (n - 1) as usize;
        if !present {
            continue;
        }
        if !allowed.contains(&works[index]) {
            if strict {
                return Err((StatusCode::FORBIDDEN, "无权访问列表中的媒体".into()));
            }
            continue;
        }
        let (wid, season, episode) = targets[index];
        if media_type == "movie" && season.is_some() {
            return Err((StatusCode::BAD_REQUEST, "电影不支持季/集目标".into()));
        }
        let kind = if episode.is_some() {
            "Episode"
        } else if season.is_some() {
            "Season"
        } else if media_type == "tv" {
            "Series"
        } else {
            "Movie"
        };
        let name = if let Some(e) = episode {
            episode_title.unwrap_or_else(|| format!("第 {e} 集"))
        } else if let Some(s) = season {
            season_name.unwrap_or_else(|| {
                if s == 0 {
                    "特别篇".into()
                } else {
                    format!("第 {s} 季")
                }
            })
        } else {
            title.clone()
        };
        let poster = if episode.is_some() {
            still_path
        } else if season.is_some() {
            season_poster
        } else {
            poster_path
        };
        let played = if matches!(kind, "Series" | "Season") {
            episodes_played
        } else {
            history_played
        };
        let playback_position_ticks = if kind == "Series" { 0 } else { position_ticks };
        let mut item = json!({"Id":ids[index],"Name":name,"ServerId":cfg.server_id,"Type":kind,"MediaType":"Video",
          "IsFolder":matches!(kind,"Series"|"Season"),"ProductionYear":year,
          "ImageTags":if poster.is_some(){json!({"Primary":image_path_tag(poster.as_deref())})}else{json!({})},
          "UserData":{"PlaybackPositionTicks":playback_position_ticks,"PlayCount":play_count,"Played":played,
            "IsFavorite":favorite,"LastPlayedDate":last_played}});
        if let Some(s) = season {
            item["SeriesId"] = json!(format!("work_{wid}"));
            item["SeriesName"] = json!(title);
            item["IndexNumber"] = json!(episode.unwrap_or(s));
        }
        if let (Some(s), Some(_)) = (season, episode) {
            item["ParentId"] = json!(format!("season_{wid}_{s}"));
            item["ParentIndexNumber"] = json!(s);
        }
        let duration = duration_ticks;
        if duration > 0 {
            item["RunTimeTicks"] = json!(duration);
        } else if let Some(runtime) = runtime.filter(|r| *r > 0) {
            item["RunTimeTicks"] = json!(i64::from(runtime) * 600_000_000);
        }
        resolved.insert(index, item);
    }
    if strict && resolved.len() != ids.len() {
        return Err((StatusCode::NOT_FOUND, "媒体不存在或已被忽略".into()));
    }
    Ok((0..ids.len()).filter_map(|i| resolved.remove(&i)).collect())
}
async fn insert_items(
    conn: &mut PgConnection,
    id: i64,
    ids: &[String],
    collection: bool,
) -> Result<(), ApiError> {
    let sql = if collection {
        "INSERT INTO collection_items(collection_id,item_id) SELECT $1, x FROM UNNEST($2::text[]) x ON CONFLICT DO NOTHING"
    } else {
        "INSERT INTO playlist_items(playlist_id,item_id,sort_order) SELECT $1,x.item_id,(COALESCE((SELECT MAX(sort_order)+1 FROM playlist_items WHERE playlist_id=$1),0)+x.n-1)::int FROM UNNEST($2::text[]) WITH ORDINALITY x(item_id,n)"
    };
    sqlx::query(sql)
        .bind(id)
        .bind(ids)
        .execute(conn)
        .await
        .map_err(db_error)?;
    Ok(())
}
async fn create(
    state: &AppState,
    headers: &HeaderMap,
    q: PlaylistQuery,
    body: Value,
    collection: bool,
) -> Result<Json<Value>, ApiError> {
    let uid = resolve_valid_user_id_with_query(
        state,
        text(&q.user_id, &body, "UserId"),
        headers,
        q.api_key.as_deref(),
    )
    .await?;
    let ids = body_ids(q.ids.as_deref(), &body, "Ids");
    resolve_items(state, &uid, &ids, true).await?;
    let supplied_name = text(&q.name, &body, "Name")
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if text(&q.name, &body, "Name").is_some_and(|s| s.trim().is_empty() || s.chars().count() > 255)
        || text(&q.media_type, &body, "MediaType").is_some_and(|s| s.chars().count() > 32)
        || text(&q.parent_id, &body, "ParentId").is_some_and(|s| s.chars().count() > 64)
    {
        return Err((StatusCode::BAD_REQUEST, "列表字段长度无效".into()));
    }
    let name = supplied_name.unwrap_or(if collection {
        "未命名合集"
    } else {
        "未命名播放列表"
    });
    let mut tx = state.db.begin().await.map_err(db_error)?;
    let id: i64 = if collection {
        sqlx::query_scalar("INSERT INTO collections(user_id,name,parent_id,is_locked) VALUES($1,$2,$3,$4) RETURNING id")
          .bind(&uid).bind(name).bind(text(&q.parent_id,&body,"ParentId")).bind(q.is_locked.or_else(||body["IsLocked"].as_bool()).unwrap_or(false)).fetch_one(&mut *tx).await.map_err(db_error)?
    } else {
        sqlx::query_scalar(
            "INSERT INTO playlists(user_id,name,media_type) VALUES($1,$2,$3) RETURNING id",
        )
        .bind(&uid)
        .bind(name)
        .bind(text(&q.media_type, &body, "MediaType").unwrap_or("Video"))
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?
    };
    insert_items(&mut tx, id, &ids, collection).await?;
    tx.commit().await.map_err(db_error)?;
    let prefix = if collection { "col_" } else { "pl_" };
    state.session_hub.broadcast(
        Some(&uid),
        "LibraryChanged",
        json!({
            "ItemsAdded": [format!("{prefix}{id}")],
            "ItemsUpdated": [],
            "ItemsRemoved": [],
            "FoldersAddedTo": [],
            "FoldersRemovedFrom": []
        }),
    );
    Ok(Json(
        json!({"Id":format!("{}{id}",if collection{"col_"}else{"pl_"}),"Name":name}),
    ))
}
pub(super) async fn create_playlist(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Query(q): Query<PlaylistQuery>,
    b: Option<Json<Value>>,
) -> Result<Json<Value>, ApiError> {
    create(&s, &h, q, b.map(|b| b.0).unwrap_or(Value::Null), false).await
}
pub(super) async fn create_collection(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Query(q): Query<PlaylistQuery>,
    b: Option<Json<Value>>,
) -> Result<Json<Value>, ApiError> {
    create(&s, &h, q, b.map(|b| b.0).unwrap_or(Value::Null), true).await
}

pub(super) async fn list_items(
    state: &AppState,
    uid: &str,
    raw: &str,
    collection: bool,
    offset: usize,
    limit: Option<usize>,
) -> Result<Value, ApiError> {
    let is_admin = admin(state, uid).await?;
    list_items_with_admin(state, uid, raw, collection, offset, limit, is_admin).await
}

async fn filter_visible_item_ids(
    state: &AppState,
    uid: &str,
    ids: &[String],
) -> Result<HashSet<String>, ApiError> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    let mut unique_ids = Vec::with_capacity(ids.len());
    let mut seen = HashSet::with_capacity(ids.len());
    for raw in ids {
        if seen.insert(raw.as_str()) {
            unique_ids.push(raw.as_str());
        }
    }
    let targets = unique_ids
        .iter()
        .map(|s| target(s))
        .collect::<Result<Vec<_>, _>>()?;
    let works: Vec<i64> = targets.iter().map(|t| t.0).collect();
    let seasons: Vec<Option<i32>> = targets.iter().map(|t| t.1).collect();
    let episodes: Vec<Option<i32>> = targets.iter().map(|t| t.2).collect();
    let allowed = allowed_work_ids(state, uid, &works).await?;
    let rows: Vec<(i64, String, bool)> = sqlx::query_as(
        r#"
        SELECT x.n::bigint AS n, w.media_type,
               EXISTS(SELECT 1 FROM sync_folders f WHERE f.work_id=w.id AND NOT f.is_ignored AND
                  (x.s IS NULL OR EXISTS(SELECT 1 FROM sync_files sf WHERE sf.folder_id=f.id AND sf.season_number=x.s AND (x.e IS NULL OR sf.episode_number=x.e)))) AS present
        FROM UNNEST($1::bigint[], $2::int[], $3::int[]) WITH ORDINALITY x(w,s,e,n)
        JOIN media_works w ON w.id=x.w
        "#,
    )
    .bind(&works)
    .bind(&seasons)
    .bind(&episodes)
    .fetch_all(&state.db)
    .await
    .map_err(db_error)?;

    let mut visible = HashSet::with_capacity(rows.len());
    for (n, media_type, present) in rows {
        let index = (n - 1) as usize;
        if !present || !allowed.contains(&works[index]) {
            continue;
        }
        if media_type == "movie" && seasons[index].is_some() {
            return Err((StatusCode::BAD_REQUEST, "电影不支持季/集目标".into()));
        }
        visible.insert(unique_ids[index].to_owned());
    }
    Ok(visible)
}

fn paginate_visible_entries(
    candidates: Vec<(i64, String)>,
    visible_set: &HashSet<String>,
    offset: usize,
    limit: Option<usize>,
) -> (i64, Vec<(i64, String)>, Vec<String>) {
    let visible_entries: Vec<(i64, String)> = candidates
        .into_iter()
        .filter(|(_, item_id)| visible_set.contains(item_id))
        .collect();
    let total = visible_entries.len() as i64;
    let entries: Vec<(i64, String)> = visible_entries
        .into_iter()
        .skip(offset)
        .take(limit.unwrap_or(5000).min(5000))
        .collect();
    let mut unique_page_ids = Vec::with_capacity(entries.len());
    let mut seen_page_ids = HashSet::with_capacity(entries.len());
    for (_, item_id) in &entries {
        if seen_page_ids.insert(item_id.as_str()) {
            unique_page_ids.push(item_id.clone());
        }
    }
    (total, entries, unique_page_ids)
}

async fn list_items_with_admin(
    state: &AppState,
    uid: &str,
    raw: &str,
    collection: bool,
    offset: usize,
    limit: Option<usize>,
    is_admin: bool,
) -> Result<Value, ApiError> {
    let id = parse_prefixed_id(raw, if collection { "col_" } else { "pl_" })?;
    let mut conn = state.db.acquire().await.map_err(db_error)?;
    authorize(&mut conn, id, uid, is_admin, collection, false, false).await?;
    let all_sql = if collection {
        "SELECT id,item_id FROM collection_items WHERE collection_id=$1 ORDER BY id"
    } else {
        "SELECT id,item_id FROM playlist_items WHERE playlist_id=$1 ORDER BY sort_order,id"
    };
    let candidates: Vec<(i64, String)> = sqlx::query_as(all_sql)
        .bind(id)
        .fetch_all(&mut *conn)
        .await
        .map_err(db_error)?;
    drop(conn);
    if candidates.is_empty() {
        return Ok(json!({"Items":[],"TotalRecordCount":0,"StartIndex":offset}));
    }
    let candidate_ids: Vec<String> = candidates.iter().map(|entry| entry.1.clone()).collect();
    let visible_set = filter_visible_item_ids(state, uid, &candidate_ids).await?;
    let (total, entries, unique_page_ids) =
        paginate_visible_entries(candidates, &visible_set, offset, limit);
    let resolved = if unique_page_ids.is_empty() {
        Vec::new()
    } else {
        resolve_items(state, uid, &unique_page_ids, false).await?
    };
    let by_id: HashMap<String, Value> = resolved
        .into_iter()
        .map(|item| (item["Id"].as_str().unwrap_or_default().to_owned(), item))
        .collect();
    let mut items = Vec::with_capacity(entries.len());
    for (entry, item_id) in entries {
        if let Some(mut item) = by_id.get(&item_id).cloned() {
            if !collection {
                item["PlaylistItemId"] = json!(format!("ple_{entry}"));
            }
            items.push(item);
        }
    }
    Ok(json!({"Items":items,"TotalRecordCount":total,"StartIndex":offset}))
}
pub(super) async fn get_playlist_items(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<PlaylistQuery>,
) -> Result<Json<Value>, ApiError> {
    let (uid, is_admin) =
        resolve_valid_user_and_admin_with_query(&s, q.user_id.as_deref(), &h, q.api_key.as_deref())
            .await?;
    Ok(Json(
        list_items_with_admin(&s, &uid, &id, false, q.start_index, q.limit, is_admin).await?,
    ))
}
pub(super) async fn get_collection_items(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<PlaylistQuery>,
) -> Result<Json<Value>, ApiError> {
    let (uid, is_admin) =
        resolve_valid_user_and_admin_with_query(&s, q.user_id.as_deref(), &h, q.api_key.as_deref())
            .await?;
    Ok(Json(
        list_items_with_admin(&s, &uid, &id, true, q.start_index, q.limit, is_admin).await?,
    ))
}
async fn mutate(
    state: &AppState,
    h: &HeaderMap,
    raw: &str,
    q: PlaylistQuery,
    body: Value,
    collection: bool,
    remove: bool,
) -> Result<StatusCode, ApiError> {
    let (uid, is_admin) = resolve_valid_user_and_admin_with_query(
        state,
        text(&q.user_id, &body, "UserId"),
        h,
        q.api_key.as_deref(),
    )
    .await?;
    let id = parse_prefixed_id(raw, if collection { "col_" } else { "pl_" })?;
    let ids = body_ids(q.ids.as_deref(), &body, "Ids");
    if !remove {
        resolve_items(state, &uid, &ids, true).await?;
    }
    let mut tx = state.db.begin().await.map_err(db_error)?;
    let list = authorize(&mut tx, id, &uid, is_admin, collection, true, true).await?;
    let user_scope = list["user_id"].as_str();
    if remove {
        let entries = body_ids(q.entry_ids.as_deref(), &body, "EntryIds")
            .iter()
            .map(|s| parse_prefixed_id(s, "ple_"))
            .collect::<Result<Vec<_>, _>>()?;
        if !collection && !entries.is_empty() {
            sqlx::query("DELETE FROM playlist_items WHERE playlist_id=$1 AND id=ANY($2)")
                .bind(id)
                .bind(entries)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
        } else {
            let sql = if collection {
                "DELETE FROM collection_items WHERE collection_id=$1 AND item_id=ANY($2)"
            } else {
                "DELETE FROM playlist_items WHERE playlist_id=$1 AND item_id=ANY($2)"
            };
            sqlx::query(sql)
                .bind(id)
                .bind(&ids)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
        }
        if !collection {
            normalize_order(&mut tx, id).await?;
        }
    } else {
        insert_items(&mut tx, id, &ids, collection).await?;
    }
    tx.commit().await.map_err(db_error)?;
    invalidate_list_image_cache(state, collection, id).await;
    let prefix = if collection { "col_" } else { "pl_" };
    let list_id = format!("{prefix}{id}");
    state.session_hub.broadcast(
        user_scope,
        "LibraryChanged",
        json!({
            "ItemsAdded": if remove { vec![] } else { vec![list_id.clone()] },
            "ItemsUpdated": [],
            "ItemsRemoved": if remove { vec![list_id] } else { vec![] },
            "FoldersAddedTo": [],
            "FoldersRemovedFrom": []
        }),
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn invalidate_list_image_cache(state: &AppState, collection: bool, id: i64) {
    let prefix = if collection { "col_" } else { "pl_" };
    for image_type in ["Primary", "Backdrop"] {
        state
            .caches
            .image_meta_cache
            .invalidate(&format!("{prefix}{id}:{image_type}"))
            .await;
    }
}

macro_rules! mutation {
    ($name:ident,$collection:expr,$remove:expr) => {
        pub(super) async fn $name(
            State(s): State<Arc<AppState>>,
            h: HeaderMap,
            Path(id): Path<String>,
            Query(q): Query<PlaylistQuery>,
            b: Option<Json<Value>>,
        ) -> Result<StatusCode, ApiError> {
            mutate(
                &s,
                &h,
                &id,
                q,
                b.map(|b| b.0).unwrap_or(Value::Null),
                $collection,
                $remove,
            )
            .await
        }
    };
}
mutation!(add_playlist_items, false, false);
mutation!(remove_playlist_items, false, true);
mutation!(add_collection_items, true, false);
mutation!(remove_collection_items, true, true);
pub(super) async fn delete_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Query(q): Query<PlaylistQuery>,
) -> Result<StatusCode, ApiError> {
    let (collection, _) = delete_target(&raw)?;
    let prefix = if collection { "col_" } else { "pl_" };
    delete_owned(&state, &headers, &raw, q, collection, prefix).await
}

pub(super) async fn delete_item(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Query(q): Query<PlaylistQuery>,
) -> Result<StatusCode, ApiError> {
    let (collection, _) = delete_target(&raw)?;
    let prefix = if collection { "col_" } else { "pl_" };
    delete_owned(&state, &headers, &raw, q, collection, prefix).await
}

async fn delete_owned(
    state: &AppState,
    headers: &HeaderMap,
    raw: &str,
    q: PlaylistQuery,
    collection: bool,
    prefix: &str,
) -> Result<StatusCode, ApiError> {
    let (uid, is_admin) = resolve_valid_user_and_admin_with_query(
        state,
        q.user_id.as_deref(),
        headers,
        q.api_key.as_deref(),
    )
    .await?;
    let id = parse_prefixed_id(raw, prefix)?;
    let mut tx = state.db.begin().await.map_err(db_error)?;
    let list = authorize(&mut tx, id, &uid, is_admin, collection, true, true).await?;
    let user_scope = list["user_id"].as_str();
    let sql = if collection {
        "DELETE FROM collections WHERE id=$1"
    } else {
        "DELETE FROM playlists WHERE id=$1"
    };
    sqlx::query(sql)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    invalidate_list_image_cache(state, collection, id).await;
    state.session_hub.broadcast(
        user_scope,
        "LibraryChanged",
        json!({
            "ItemsAdded": [],
            "ItemsUpdated": [],
            "ItemsRemoved": [format!("{prefix}{id}")],
            "FoldersAddedTo": [],
            "FoldersRemovedFrom": []
        }),
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn normalize_order(conn: &mut PgConnection, id: i64) -> Result<(), ApiError> {
    sqlx::query("UPDATE playlist_items p SET sort_order=x.n-1 FROM (SELECT id,(row_number() OVER (ORDER BY sort_order,id))::int n FROM playlist_items WHERE playlist_id=$1) x WHERE p.id=x.id").bind(id).execute(conn).await.map_err(db_error)?;
    Ok(())
}
fn reorder(entries: &mut Vec<(i64, String)>, raw: &str, index: i32) -> Result<(), ApiError> {
    let entry_id = raw.strip_prefix("ple_").and_then(|s| s.parse::<i64>().ok());
    let pos = entries
        .iter()
        .position(|(id, item)| entry_id.map_or(item == raw, |entry| entry == *id))
        .ok_or((StatusCode::NOT_FOUND, "播放列表条目不存在".into()))?;
    let entry = entries.remove(pos);
    let index = (index.max(0) as usize).min(entries.len());
    entries.insert(index, entry);
    Ok(())
}
pub(super) async fn move_playlist_item(
    State(state): State<Arc<AppState>>,
    h: HeaderMap,
    Path((raw, item, index)): Path<(String, String, i32)>,
    Query(q): Query<PlaylistQuery>,
) -> Result<StatusCode, ApiError> {
    let (uid, is_admin) = resolve_valid_user_and_admin_with_query(
        &state,
        q.user_id.as_deref(),
        &h,
        q.api_key.as_deref(),
    )
    .await?;
    let id = parse_prefixed_id(&raw, "pl_")?;
    let mut tx = state.db.begin().await.map_err(db_error)?;
    let list = authorize(&mut tx, id, &uid, is_admin, false, true, true).await?;
    let owner = list["user_id"].as_str().unwrap_or(&uid).to_owned();
    let mut entries:Vec<(i64,String)>=sqlx::query_as("SELECT id,item_id FROM playlist_items WHERE playlist_id=$1 ORDER BY sort_order,id FOR UPDATE").bind(id).fetch_all(&mut *tx).await.map_err(db_error)?;
    reorder(&mut entries, &item, index)?;
    let ids: Vec<i64> = entries.into_iter().map(|e| e.0).collect();
    sqlx::query("UPDATE playlist_items p SET sort_order=(x.n-1)::int FROM UNNEST($2::bigint[]) WITH ORDINALITY x(id,n) WHERE p.playlist_id=$1 AND p.id=x.id").bind(id).bind(ids).execute(&mut *tx).await.map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    invalidate_list_image_cache(&state, false, id).await;
    state.session_hub.broadcast(
        Some(&owner),
        "LibraryChanged",
        json!({
            "ItemsAdded": [],
            "ItemsUpdated": [format!("pl_{id}")],
            "ItemsRemoved": [],
            "FoldersAddedTo": [],
            "FoldersRemovedFrom": []
        }),
    );
    Ok(StatusCode::NO_CONTENT)
}
pub(super) async fn list_detail(
    state: &AppState,
    uid: &str,
    raw: &str,
    collection: bool,
) -> Result<Value, ApiError> {
    let id = parse_prefixed_id(raw, if collection { "col_" } else { "pl_" })?;
    let is_admin = admin(state, uid).await?;
    let mut conn = state.db.acquire().await.map_err(db_error)?;
    let row = authorize(&mut conn, id, uid, is_admin, collection, false, false).await?;
    let sql = if collection {
        "SELECT COUNT(*) FROM collection_items WHERE collection_id=$1"
    } else {
        "SELECT COUNT(*) FROM playlist_items WHERE playlist_id=$1"
    };
    let count: i64 = sqlx::query_scalar(sql)
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .map_err(db_error)?;
    Ok(
        json!({"Id":format!("{}{id}",if collection{"col_"}else{"pl_"}),"Name":row["name"],"Type":if collection{"BoxSet"}else{"Playlist"},"IsFolder":true,"ServerId":state.config.load().server_id,"ChildCount":count,"ParentId":row["parent_id"],"IsLocked":row["is_locked"],"MediaType":row.get("media_type").cloned().unwrap_or(json!("Video"))}),
    )
}
struct DiscoverQuery<'a> {
    playlists: bool,
    collections: bool,
    offset: usize,
    limit: usize,
    search: Option<&'a str>,
}

/// 根发现批量返回列表及条目数，ACL 与详情一致。
pub(super) async fn discover(
    state: &AppState,
    uid: &str,
    playlists: bool,
    collections: bool,
    offset: usize,
    limit: usize,
    search: Option<&str>,
) -> Result<Value, ApiError> {
    let is_admin = admin(state, uid).await?;
    discover_with_admin(
        state,
        uid,
        DiscoverQuery {
            playlists,
            collections,
            offset,
            limit,
            search,
        },
        is_admin,
    )
    .await
}

async fn discover_with_admin(
    state: &AppState,
    uid: &str,
    query: DiscoverQuery<'_>,
    is_admin: bool,
) -> Result<Value, ApiError> {
    let DiscoverQuery {
        playlists,
        collections,
        offset,
        limit,
        search,
    } = query;
    let term = search.map(|s| format!("%{}%", escape_like(s)));
    let sql = r#"WITH visible AS (
      SELECT jsonb_build_object('Id','pl_'||p.id,'Name',p.name,'Type','Playlist','IsFolder',true,'MediaType',p.media_type,'ChildCount',(SELECT COUNT(*) FROM playlist_items i WHERE i.playlist_id=p.id)) data
      FROM playlists p WHERE $3 AND (p.user_id=$1 OR $2) AND ($5::text IS NULL OR p.name ILIKE $5 ESCAPE '\')
      UNION ALL
      SELECT jsonb_build_object('Id','col_'||c.id,'Name',c.name,'Type','BoxSet','IsFolder',true,'ParentId',c.parent_id,'IsLocked',c.is_locked,'ChildCount',(SELECT COUNT(*) FROM collection_items i WHERE i.collection_id=c.id))
      FROM collections c WHERE $4 AND (c.user_id IS NULL OR c.user_id=$1 OR $2) AND ($5::text IS NULL OR c.name ILIKE $5 ESCAPE '\')
    ), counted AS (SELECT data, COUNT(*) OVER() AS total FROM visible)
    SELECT data,total FROM counted ORDER BY data->>'Name',data->>'Id' LIMIT $6 OFFSET $7"#;
    let rows = sqlx::query_as::<_, (Value, i64)>(sql)
        .bind(uid)
        .bind(is_admin)
        .bind(playlists)
        .bind(collections)
        .bind(term.as_deref())
        .bind(limit.min(i64::MAX as usize) as i64)
        .bind(offset.min(i64::MAX as usize) as i64)
        .fetch_all(&state.db)
        .await
        .map_err(db_error)?;
    let total: i64 = if let Some((_, total)) = rows.first() {
        *total
    } else if let Some(total) = empty_discovery_total(offset) {
        total
    } else {
        sqlx::query_scalar(r#"SELECT
          (SELECT COUNT(*) FROM playlists p WHERE $3 AND (p.user_id=$1 OR $2) AND ($5::text IS NULL OR p.name ILIKE $5 ESCAPE '\')) +
          (SELECT COUNT(*) FROM collections c WHERE $4 AND (c.user_id IS NULL OR c.user_id=$1 OR $2) AND ($5::text IS NULL OR c.name ILIKE $5 ESCAPE '\'))"#)
            .bind(uid).bind(is_admin).bind(playlists).bind(collections).bind(term.as_deref())
            .fetch_one(&state.db).await.map_err(db_error)?
    };
    let server_id = state.config.load().server_id.clone();
    let mut items = Vec::with_capacity(rows.len());
    for (mut row, _) in rows {
        row["ServerId"] = json!(server_id);
        items.push(row);
    }
    Ok(json!({"Items":items,"TotalRecordCount":total,"StartIndex":offset}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_minutes_convert_to_emby_ticks() {
        assert_eq!(i64::from(40_i32) * 600_000_000, 24_000_000_000);
    }
    #[test]
    fn list_identifiers_reject_nonpositive_database_keys() {
        assert!(parse_prefixed_id("pl_-1", "pl_").is_err());
        assert!(parse_prefixed_id("0", "col_").is_err());
    }
    #[test]
    fn delete_targets_parse_playlist_collection_and_reject_unknown_prefixes() {
        assert_eq!(delete_target("pl_12").unwrap(), (false, 12));
        assert_eq!(delete_target("col_34").unwrap(), (true, 34));
        assert_eq!(
            delete_target("work_12").unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            delete_target("pl_0").unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn owner_admin_and_legacy_collections_are_accessible() {
        assert!(list_access(Some("owner"), "owner", false, false));
        assert!(list_access(Some("owner"), "admin", true, false));
        assert!(list_access(None, "any", false, false));
        assert!(!list_access(Some("owner"), "other", false, false));
        assert!(!list_access(None, "any", false, true));
        assert!(!list_access(None, "other", false, true));
        assert!(list_access(None, "any", true, true));
    }
    #[test]
    fn move_entry_id_disambiguates_duplicates_and_keeps_other_entries_in_order() {
        let mut entries = vec![
            (1, "work_7".into()),
            (2, "work_8".into()),
            (3, "work_7".into()),
        ];
        reorder(&mut entries, "ple_3", 0).unwrap();
        assert_eq!(
            entries.iter().map(|e| e.0).collect::<Vec<_>>(),
            vec![3, 1, 2]
        );
        reorder(&mut entries, "work_8", 99).unwrap();
        assert_eq!(
            entries.iter().map(|e| e.0).collect::<Vec<_>>(),
            vec![3, 1, 2]
        );
        assert_eq!(
            reorder(&mut entries, "ple_99", 0).unwrap_err().0,
            StatusCode::NOT_FOUND
        );
    }
    #[test]
    fn json_ids_support_array_csv_and_query_override() {
        let body = json!({"Ids":["work_1", "season_2_0,ep_2_0_1"]});
        assert_eq!(
            body_ids(None, &body, "Ids"),
            vec!["work_1", "season_2_0", "ep_2_0_1"]
        );
        assert_eq!(body_ids(Some("work_3"), &body, "Ids"), vec!["work_3"]);
        assert_eq!(
            body_ids(None, &json!({"EntryIds":"ple_1, ple_2"}), "EntryIds"),
            vec!["ple_1", "ple_2"]
        );
    }
    #[test]
    fn media_target_rejects_unknown_types_and_preserves_specials() {
        assert_eq!(target("ep_2_0_0").unwrap(), (2, Some(0), Some(0)));
        assert_eq!(target("season_2_0").unwrap(), (2, Some(0), None));
        assert_eq!(target("work_2").unwrap(), (2, None, None));
        assert!(target("pl_1").is_err());
        assert!(target("ep_-2_0_0").is_err());
        assert!(target("season_0_1").is_err());
    }
    #[test]
    fn paginate_visible_entries_slices_and_deduplicates_page_ids() {
        let candidates = vec![
            (1, "work_10".into()),
            (2, "work_hidden".into()),
            (3, "work_10".into()),
            (4, "work_20".into()),
            (5, "work_10".into()),
        ];
        let visible: HashSet<String> = ["work_10".into(), "work_20".into()].into_iter().collect();
        let (total, entries, unique_page_ids) =
            paginate_visible_entries(candidates, &visible, 1, Some(3));
        assert_eq!(total, 4);
        assert_eq!(
            entries,
            vec![
                (3, "work_10".into()),
                (4, "work_20".into()),
                (5, "work_10".into()),
            ]
        );
        assert_eq!(unique_page_ids, vec!["work_10", "work_20"]);
    }
}

#[cfg(test)]
mod database_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "需要 TEST_DATABASE_URL；为每次测试创建独立 PostgreSQL schema"]
    async fn collections_and_playlists_acl_atomic_insert_and_reorder() {
        let url = std::env::var("TEST_DATABASE_URL").expect("设置 PostgreSQL 测试库");
        let bootstrap = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("playlist_test_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&bootstrap)
            .await
            .unwrap();
        let search_path = format!("SET search_path TO {schema}, public");
        let db = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .after_connect(move |conn, _| {
                let sql = search_path.clone();
                Box::pin(async move {
                    sqlx::query(&sql).execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&db).await.unwrap();
        sqlx::raw_sql(r#"
            INSERT INTO users(id,username,password_hash,is_admin,allowed_categories) VALUES
              ('owner','owner','',false,ARRAY['电影']),('other','other','',false,ARRAY['电影']),
              ('admin','admin','',true,ARRAY['*']);
            INSERT INTO access_tokens(token,user_id,device_id,device_name,client_name)
              VALUES ('owner-token','owner','test','test','test');
            INSERT INTO media_works(id,media_type,title,rating,category) VALUES
              (10,'movie','电影',8,'电影'),(20,'tv','剧集',8,'电视剧');
            INSERT INTO sync_folders(id,root_dir,raw_folder_name,clean_title,inferred_media_type,category,scrape_state,work_id)
              VALUES (10,'/movie','movie','电影','movie','电影','verified',10),
                     (20,'/tv','tv','剧集','tv','电视剧','verified',20);
            INSERT INTO sync_files(folder_id,alist_path,path_hash,file_name,container,season_number,episode_number)
              VALUES (10,'/movie/1',101,'1.mkv','mkv',0,1),(20,'/tv/1',201,'1.mkv','mkv',0,0);
            INSERT INTO user_favorites(user_id,work_id,media_type) VALUES ('owner',10,'movie');
            INSERT INTO play_histories(user_id,work_id,media_type,play_count,is_played) VALUES ('owner',10,'movie',4,true);
        "#).execute(&db).await.unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let state = AppState::new(
            crate::config::BootstrapFileConfig::default().into_dynamic(),
            db.clone(),
            tx,
        )
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-emby-token", "owner-token".parse().unwrap());
        let created = create(
            &state,
            &headers,
            PlaylistQuery::default(),
            json!({"Name":"合集","Ids":["work_10"],"ParentId":"root","IsLocked":true}),
            true,
        )
        .await
        .unwrap()
        .0;
        let raw = created["Id"].as_str().unwrap();
        let detail = list_detail(&state, "owner", raw, true).await.unwrap();
        assert_eq!(detail["ParentId"], "root");
        assert_eq!(detail["IsLocked"], true);
        assert_eq!(
            list_detail(&state, "other", raw, true).await.unwrap_err().0,
            StatusCode::FORBIDDEN
        );
        assert!(list_detail(&state, "admin", raw, true).await.is_ok());
        assert_eq!(
            list_detail(&state, "owner", "col_999999", true)
                .await
                .unwrap_err()
                .0,
            StatusCode::NOT_FOUND
        );
        let listing = list_items(&state, "owner", raw, true, 0, None)
            .await
            .unwrap();
        assert_eq!(listing["Items"][0]["UserData"]["PlayCount"], 4);
        assert_eq!(listing["Items"][0]["UserData"]["IsFavorite"], true);
        let batch = resolve_items(
            &state,
            "admin",
            &["work_10".into(), "season_20_0".into(), "ep_20_0_0".into()],
            true,
        )
        .await
        .unwrap();
        assert_eq!(
            batch
                .iter()
                .map(|v| v["Type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["Movie", "Season", "Episode"]
        );
        assert_eq!(
            resolve_items(&state, "admin", &["ep_20_0_1".into()], true)
                .await
                .unwrap_err()
                .0,
            StatusCode::NOT_FOUND
        );
        let count_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM collections")
            .fetch_one(&db)
            .await
            .unwrap();
        let rejected = create(
            &state,
            &headers,
            PlaylistQuery::default(),
            json!({"Name":"拒绝","Ids":["work_10","ep_20_0_0"]}),
            true,
        )
        .await
        .unwrap_err();
        assert_eq!(rejected.0, StatusCode::FORBIDDEN);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM collections")
                .fetch_one(&db)
                .await
                .unwrap(),
            count_before
        );
        let legacy: i64 =
            sqlx::query_scalar("INSERT INTO collections(name) VALUES('legacy') RETURNING id")
                .fetch_one(&db)
                .await
                .unwrap();
        assert!(list_detail(&state, "other", &format!("col_{legacy}"), true)
            .await
            .is_ok());
        let pl = create(
            &state,
            &headers,
            PlaylistQuery::default(),
            json!({"Name":"播放","Ids":["work_10","work_10","work_10"]}),
            false,
        )
        .await
        .unwrap()
        .0;
        let raw = pl["Id"].as_str().unwrap();
        let before = list_items(&state, "owner", raw, false, 0, None)
            .await
            .unwrap();
        let entry = before["Items"][2]["PlaylistItemId"].as_str().unwrap();
        move_playlist_item(
            State(state.clone()),
            headers.clone(),
            Path((raw.to_string(), entry.to_string(), 0)),
            Query(PlaylistQuery::default()),
        )
        .await
        .unwrap();
        let after = list_items(&state, "owner", raw, false, 0, None)
            .await
            .unwrap();
        assert_eq!(after["Items"][0]["PlaylistItemId"], entry);
        let orders: Vec<i32> =
            sqlx::query_scalar("SELECT sort_order FROM playlist_items ORDER BY sort_order")
                .fetch_all(&db)
                .await
                .unwrap();
        assert_eq!(orders, vec![0, 1, 2]);
        assert_eq!(
            mutate(
                &state,
                &headers,
                "pl_999999",
                PlaylistQuery::default(),
                json!({"Ids":["work_10"]}),
                false,
                false
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::NOT_FOUND
        );
        let mixed = super::super::items::get_user_items_inner(
            &state,
            "owner",
            &HeaderMap::new(),
            &super::super::items::ItemsQuery {
                include_item_types: Some("Movie,Playlist".into()),
                limit: Some(1),
                start_index: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(mixed["TotalRecordCount"], 2);
        assert_eq!(mixed["Items"][0]["Type"], "Playlist");
        let first = super::super::items::get_user_items_inner(
            &state,
            "owner",
            &HeaderMap::new(),
            &super::super::items::ItemsQuery {
                include_item_types: Some("Movie,Playlist".into()),
                limit: Some(1),
                start_index: 0,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(first["Items"][0]["Type"], "Movie");
        assert_eq!(first["Items"][0]["UserData"]["PlayCount"], 4);
        assert!(first["Items"][0]["UserData"]["LastPlayedDate"].is_string());
        db.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&bootstrap)
            .await
            .unwrap();
        bootstrap.close().await;
    }
}

fn empty_discovery_total(_offset: usize) -> Option<i64> {
    // An empty page says nothing about the total, especially when LIMIT is zero.
    None
}

fn collection_page_window(
    media_total: usize,
    media_page_len: usize,
    offset: usize,
    limit: usize,
) -> (usize, usize) {
    (
        offset.saturating_sub(media_total),
        limit.saturating_sub(media_page_len),
    )
}

pub(super) async fn merge_discovery_page(
    state: &AppState,
    uid: &str,
    mut media: Value,
    playlists: bool,
    collections: bool,
    query: &super::items::ItemsQuery,
) -> Result<Value, ApiError> {
    if !playlists && !collections {
        return Ok(media);
    }
    let offset = query.start_index.max(0) as usize;
    let limit = query.limit.unwrap_or(50).clamp(1, 200) as usize;
    let media_total = media["TotalRecordCount"].as_u64().unwrap_or(0) as usize;
    let media_len = media["Items"].as_array().map_or(0, Vec::len);
    let (collection_offset, collection_limit) =
        collection_page_window(media_total, media_len, offset, limit);
    let discovered = discover(
        state,
        uid,
        playlists,
        collections,
        collection_offset,
        collection_limit,
        query.search_term.as_deref(),
    )
    .await?;
    let total = media_total as u64 + discovered["TotalRecordCount"].as_u64().unwrap_or(0);
    if let Some(items) = media["Items"].as_array_mut() {
        items.extend(
            discovered["Items"]
                .as_array()
                .into_iter()
                .flatten()
                .cloned(),
        );
    }
    media["TotalRecordCount"] = json!(total);
    Ok(media)
}

#[cfg(test)]
mod mixed_page_tests {
    use super::*;
    #[test]
    fn empty_first_discovery_page_requires_real_count() {
        assert_eq!(empty_discovery_total(0), None);
        assert_eq!(empty_discovery_total(1), None);
    }

    #[test]
    fn mixed_page_crosses_media_boundary_without_repeating_collections() {
        assert_eq!(collection_page_window(5, 1, 4, 3), (0, 2));
        assert_eq!(collection_page_window(5, 0, 8, 3), (3, 3));
        assert_eq!(collection_page_window(5, 3, 0, 3), (0, 0));
    }
}
