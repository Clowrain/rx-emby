use crate::state::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize, Default)]
pub(super) struct PlaybackQuery {
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
    pub(super) api_key: Option<String>,
}

pub(super) async fn get_user_playback_info(
    state: State<Arc<AppState>>,
    headers: HeaderMap,
    Path((_user_id, item_id)): Path<(String, String)>,
    q: Query<PlaybackQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    get_playback_info(state, headers, Path(item_id), q).await
}

pub(super) async fn get_playback_info(
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
    } else if let Some(raw_eid) = item_id
        .strip_prefix("ep_")
        .or_else(|| item_id.strip_prefix("src_"))
        .or_else(|| item_id.chars().all(|c| c.is_ascii_digit()).then_some(item_id.as_str()))
    {
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

pub(super) async fn build_media_sources_for_movie(
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

pub(super) async fn build_media_sources_for_episode(
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
    _host: &str,
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

            // 使用相对 Emby 路径且不带视频文件后缀，避免反代丢失协议/端口或被 Nginx 静态资源正则规则拦截 404
            let direct_url = format!(
                "/Videos/{}/stream?MediaSourceId=src_{}&Static=true&PlaySessionId={}&api_key={}",
                item_id, s.id, session_id, token
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

pub(super) async fn stream_video_with_file(
    state: State<Arc<AppState>>,
    Path((item_id, _file_name)): Path<(String, String)>,
    q: Query<PlaybackQuery>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    stream_video(state, Path(item_id), q, headers).await
}

pub(super) async fn stream_video(
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
pub(super) async fn stream_by_d_path(
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

pub(super) async fn get_media_segments() -> Json<Value> {
    Json(json!({
        "Items": [],
        "TotalRecordCount": 0,
        "StartIndex": 0
    }))
}

