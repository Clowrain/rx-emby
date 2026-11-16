use super::auth::{extract_emby_token, resolve_valid_user_id_with_query};
use super::ids::parse_episode_id;
use super::items::ensure_media_accessible;
use crate::state::AppState;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use serde::Deserialize;
use serde_json::{Value, json};
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
    #[serde(rename = "IsPlayback", alias = "isPlayback", alias = "isplayback")]
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
    Path((user_id, item_id)): Path<(String, String)>,
    q: Query<PlaybackQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    get_playback_info_inner(state, headers, Some(user_id), item_id, q).await
}

pub(super) async fn get_playback_info(
    state: State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<String>,
    q: Query<PlaybackQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    get_playback_info_inner(state, headers, None, item_id, q).await
}

async fn get_playback_info_inner(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    explicit_uid: Option<String>,
    item_id: String,
    Query(q): Query<PlaybackQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id_with_query(
        &state,
        explicit_uid.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;

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
    let token = extract_emby_token(&headers, q.api_key.as_deref()).unwrap_or_default();
    let session_id = Uuid::new_v4().simple().to_string();
    let sources = if let Some(raw_wid) = item_id
        .strip_prefix("work_")
        .or_else(|| item_id.strip_prefix("media_"))
    {
        let wid = raw_wid
            .parse::<i64>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的作品 ID".to_string()))?;
        let (mtype, _, _) = ensure_media_accessible(&state, &user_id, wid).await?;
        if mtype == "tv" {
            let first_ep: Option<(i32, i32)> = sqlx::query_as(
                r#"
                SELECT sf.season_number, sf.episode_number
                FROM sync_files sf
                JOIN sync_folders f ON f.id = sf.folder_id
                WHERE f.work_id = $1 AND f.is_ignored = FALSE
                ORDER BY sf.season_number ASC, sf.episode_number ASC
                LIMIT 1
                "#,
            )
            .bind(wid)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            match first_ep {
                Some((snum, epnum)) => {
                    build_media_sources_for_episode(
                        &state,
                        &host,
                        &token,
                        &session_id,
                        wid,
                        snum,
                        epnum,
                    )
                    .await?
                }
                None => return Err((StatusCode::NOT_FOUND, "剧集暂无可播放分集".to_string())),
            }
        } else {
            build_media_sources_for_movie(&state, &host, &token, &session_id, wid).await?
        }
    } else if item_id.starts_with("ep_") {
        let (wid, snum, epnum) = parse_episode_id(&item_id)
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "无效的分集 ID".to_string()))?;
        ensure_media_accessible(&state, &user_id, wid).await?;
        build_media_sources_for_episode(&state, &host, &token, &session_id, wid, snum, epnum)
            .await?
    } else if let Some(raw_sid) = item_id.strip_prefix("src_").or_else(|| {
        item_id
            .chars()
            .all(|c| c.is_ascii_digit())
            .then_some(item_id.as_str())
    }) {
        let sid = raw_sid
            .parse::<i64>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的源 ID".to_string()))?;
        let src_meta: Option<(i64, i32, i32)> = sqlx::query_as(
            "SELECT f.work_id, sf.season_number, sf.episode_number FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE sf.id = $1 AND f.work_id IS NOT NULL AND f.is_ignored = FALSE",
        )
        .bind(sid)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        match src_meta {
            Some((wid, snum, epnum)) => {
                ensure_media_accessible(&state, &user_id, wid).await?;
                build_media_sources_for_episode(
                    &state,
                    &host,
                    &token,
                    &session_id,
                    wid,
                    snum,
                    epnum,
                )
                .await?
            }
            None => return Err((StatusCode::NOT_FOUND, "播放源不存在或已屏蔽".to_string())),
        }
    } else {
        return Err((StatusCode::NOT_FOUND, "未找到播放条目".to_string()));
    };

    if q.is_playback.unwrap_or(true) {
        let requested_source = q
            .media_source_id
            .as_deref()
            .map(|value| value.strip_prefix("src_").unwrap_or(value));
        let target_src_id = if let Some(source_id) = requested_source {
            Some(
                source_id
                    .parse::<i64>()
                    .map_err(|_| (StatusCode::BAD_REQUEST, "无效的媒体源 ID".to_string()))?,
            )
        } else {
            sources
                .first()
                .and_then(|source| source.get("Id"))
                .and_then(Value::as_str)
                .and_then(|source_id| source_id.strip_prefix("src_"))
                .and_then(|source_id| source_id.parse::<i64>().ok())
        };

        if let Some(src_id) = target_src_id {
            let returned_source = sources.iter().any(|source| {
                source
                    .get("Id")
                    .and_then(Value::as_str)
                    .and_then(|value| value.strip_prefix("src_"))
                    .and_then(|value| value.parse::<i64>().ok())
                    == Some(src_id)
            });
            if requested_source.is_some() && !returned_source {
                return Err((
                    StatusCode::NOT_FOUND,
                    "指定的媒体源不属于当前条目或已被屏蔽".to_string(),
                ));
            }
            let prewarm_source: Option<(i64, String, i64)> = sqlx::query_as(
                "SELECT f.work_id, sf.alist_path, sf.path_hash FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE sf.id = $1 AND f.work_id IS NOT NULL AND f.is_ignored = FALSE",
            )
            .bind(src_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            if requested_source.is_some() && prewarm_source.is_none() {
                return Err((
                    StatusCode::NOT_FOUND,
                    "指定的媒体源不属于当前条目或已被屏蔽".to_string(),
                ));
            }
            if let Some((source_work_id, alist_path, path_hash)) = prewarm_source {
                ensure_media_accessible(&state, &user_id, source_work_id).await?;
                let state_clone = state.clone();
                tokio::spawn(async move {
                    let _ = state_clone
                        .resolve_playback_url(path_hash, &alist_path, &client_ua)
                        .await;
                });
            }
        }
    }

    tracing::info!(
        "[PlaybackInfo] item_id={} 返回 {} 个源",
        item_id,
        sources.len()
    );
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
    work_id: i64,
) -> Result<Vec<Value>, (StatusCode, String)> {
    let rows: Vec<SourceRecord> = sqlx::query_as(
        r#"
        SELECT sf.id, sf.version_tag, sf.container, sf.alist_path, sf.file_size_bytes
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        WHERE f.work_id = $1 AND f.is_ignored = FALSE
        ORDER BY COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) DESC, sf.id ASC
        "#,
    )
    .bind(work_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    serialize_media_sources(
        state,
        host,
        token,
        session_id,
        format!("work_{}", work_id),
        rows,
    )
    .await
}

pub(super) async fn build_media_sources_for_episode(
    state: &AppState,
    host: &str,
    token: &str,
    session_id: &str,
    work_id: i64,
    season_num: i32,
    episode_num: i32,
) -> Result<Vec<Value>, (StatusCode, String)> {
    let rows: Vec<SourceRecord> = sqlx::query_as(
        r#"
        SELECT sf.id, sf.version_tag, sf.container, sf.alist_path, sf.file_size_bytes
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        WHERE f.work_id = $1
          AND f.is_ignored = FALSE
          AND sf.season_number = $2
          AND sf.episode_number = $3
        ORDER BY COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) DESC, sf.id ASC
        "#,
    )
    .bind(work_id)
    .bind(season_num)
    .bind(episode_num)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    serialize_media_sources(
        state,
        host,
        token,
        session_id,
        format!("ep_{}_{}_{}", work_id, season_num, episode_num),
        rows,
    )
    .await
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
    let user_id =
        resolve_valid_user_id_with_query(&state, None, &headers, q.api_key.as_deref()).await?;

    let client_ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // 提取具体多版本 Source ID（若未指定则回退到 item_id 对应最高权重源）
    let target_src_id: i64 = if let Some(ref msid) = q.media_source_id {
        msid.strip_prefix("src_")
            .unwrap_or(msid)
            .parse::<i64>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的源 ID".to_string()))?
    } else if let Some(raw_sid) = item_id.strip_prefix("src_") {
        raw_sid
            .parse::<i64>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的源 ID".to_string()))?
    } else if item_id.starts_with("ep_") {
        let (wid, snum, epnum) = parse_episode_id(&item_id)
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "无效的分集 ID".to_string()))?;
        sqlx::query_scalar(
            r#"
            SELECT sf.id
            FROM sync_files sf
            JOIN sync_folders f ON f.id = sf.folder_id
            WHERE f.work_id = $1 AND f.is_ignored = FALSE
              AND sf.season_number = $2 AND sf.episode_number = $3
            ORDER BY COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) DESC, sf.id ASC
            LIMIT 1
            "#,
        )
        .bind(wid)
        .bind(snum)
        .bind(epnum)
        .fetch_optional(&state.db)
        .await
        .map_err(|error| {
            tracing::error!("查询播放源失败: {}", error);
            (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        })?
        .unwrap_or(0)
    } else if let Some(raw_wid) = item_id
        .strip_prefix("work_")
        .or_else(|| item_id.strip_prefix("media_"))
    {
        let wid = raw_wid
            .parse::<i64>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的作品 ID".to_string()))?;
        sqlx::query_scalar(
            r#"
            SELECT sf.id
            FROM sync_files sf
            JOIN sync_folders f ON f.id = sf.folder_id
            WHERE f.work_id = $1 AND f.is_ignored = FALSE
            ORDER BY sf.season_number ASC, sf.episode_number ASC, COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) DESC, sf.id ASC
            LIMIT 1
            "#,
        )
        .bind(wid)
        .fetch_optional(&state.db)
        .await
        .map_err(|error| {
            tracing::error!("查询播放源失败: {}", error);
            (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        })?
        .unwrap_or(0)
    } else if item_id.chars().all(|c| c.is_ascii_digit()) {
        item_id
            .parse::<i64>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的源 ID".to_string()))?
    } else {
        return Err((StatusCode::BAD_REQUEST, "无效的媒体或源 ID".to_string()));
    };

    let row: Option<(i64, String, i64)> = sqlx::query_as(
        "SELECT f.work_id, sf.alist_path, sf.path_hash FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE sf.id = $1 AND f.work_id IS NOT NULL AND f.is_ignored = FALSE",
    )
    .bind(target_src_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some((wid, alist_path, path_hash)) = row else {
        return Err((
            StatusCode::NOT_FOUND,
            "未找到对应的视频播放源或条目已被屏蔽".to_string(),
        ));
    };

    ensure_media_accessible(&state, &user_id, wid).await?;

    let resolved = state
        .resolve_playback_url(path_hash, &alist_path, client_ua)
        .await
        .map_err(|e| {
            tracing::error!(
                "❌ [Stream Error] target_src_id={} alist_path={} 失败: {:#}",
                target_src_id,
                alist_path,
                e
            );
            (
                StatusCode::BAD_GATEWAY,
                format!("获取网盘直链失败: {:#}", e),
            )
        })?;

    // 返回 302 Found 直跳网盘 CDN，并严格禁用浏览器/播放器缓存该重定向
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, resolved.redirect_url)
        .header(header::CACHE_CONTROL, "no-store, no-cache, must-revalidate")
        .body(axum::body::Body::empty())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

fn indexed_path_work_id(row: Option<(Option<i64>, bool)>) -> Result<i64, (StatusCode, String)> {
    let Some((work_id, is_ignored)) = row else {
        return Err((StatusCode::NOT_FOUND, "未找到已索引的播放源".to_string()));
    };
    if is_ignored {
        return Err((StatusCode::NOT_FOUND, "条目已被屏蔽".to_string()));
    }
    work_id.ok_or((
        StatusCode::NOT_FOUND,
        "播放源未关联到已刮削媒体".to_string(),
    ))
}

fn canonicalize_alist_path(path: &str) -> Result<String, (StatusCode, String)> {
    let parts: Vec<&str> = path
        .trim()
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty()
        || parts
            .iter()
            .any(|segment| *segment == "." || *segment == "..")
    {
        return Err((StatusCode::BAD_REQUEST, "非法播放路径".to_string()));
    }
    Ok(format!("/{}", parts.join("/")))
}

/// 兼容按 `/d/*alist_path` 逻辑路径直接请求播放，内部换链并 302 跳转真实网盘直链
pub(super) async fn stream_by_d_path(
    State(state): State<Arc<AppState>>,
    Path(raw_path): Path<String>,
    Query(q): Query<PlaybackQuery>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    let user_id =
        resolve_valid_user_id_with_query(&state, None, &headers, q.api_key.as_deref()).await?;
    let client_ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // Axum has already percent-decoded Path captures. Decoding again would turn
    // a stored literal "%2e%2e" segment into traversal after validation.
    let alist_path = canonicalize_alist_path(&raw_path)?;
    let path_hash = xxhash_rust::xxh3::xxh3_64(alist_path.as_bytes()) as i64;

    let path_row = sqlx::query_as::<_, (Option<i64>, bool)>(
        "SELECT f.work_id, f.is_ignored FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE sf.path_hash = $1 AND sf.alist_path = $2 LIMIT 1",
    )
    .bind(path_hash)
    .bind(&alist_path)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let wid = indexed_path_work_id(path_row)?;
    ensure_media_accessible(&state, &user_id, wid).await?;

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

#[cfg(test)]
mod path_tests {
    use super::{StatusCode, canonicalize_alist_path, indexed_path_work_id};

    #[test]
    fn d_path_rejects_missing_ignored_and_unindexed_sources() {
        assert_eq!(
            indexed_path_work_id(None).unwrap_err().0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            indexed_path_work_id(Some((Some(7), true))).unwrap_err().0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            indexed_path_work_id(Some((None, false))).unwrap_err().0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(indexed_path_work_id(Some((Some(7), false))).unwrap(), 7);
    }

    #[test]
    fn canonical_path_rejects_encoded_traversal_without_decoding_twice() {
        assert_eq!(
            canonicalize_alist_path("Movies/../secret.mkv")
                .unwrap_err()
                .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            canonicalize_alist_path("Movies/%2e%2e/secret.mkv").unwrap(),
            "/Movies/%2e%2e/secret.mkv"
        );
    }

    #[test]
    fn canonical_path_rejects_dot_segments_and_normalizes_leading_slashes() {
        assert_eq!(
            canonicalize_alist_path("//Movies///a.mkv").unwrap(),
            "/Movies/a.mkv"
        );
        assert_eq!(
            canonicalize_alist_path("/电影/庆余年 S01 E01.mkv").unwrap(),
            "/电影/庆余年 S01 E01.mkv"
        );
        // Axum decodes %2520 once to the literal text %20; do not decode again.
        assert_eq!(
            canonicalize_alist_path("/Movies/100%20real.mkv").unwrap(),
            "/Movies/100%20real.mkv"
        );
        assert_eq!(
            canonicalize_alist_path("/Movies/100%2520name.mkv").unwrap(),
            "/Movies/100%2520name.mkv"
        );
        assert_eq!(
            canonicalize_alist_path("/Movies/literal%XX.mkv").unwrap(),
            "/Movies/literal%XX.mkv"
        );
        assert_eq!(
            canonicalize_alist_path("Movies/../secret.mkv")
                .unwrap_err()
                .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            canonicalize_alist_path("./secret.mkv").unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
    }
}
