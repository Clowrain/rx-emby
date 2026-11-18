use super::auth::resolve_valid_user_id_with_query;
use super::ids::{parse_episode_id, parse_season_id, parse_work_id};
use crate::state::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

fn favorite_conflict_clause(tmdb_id: Option<i32>) -> &'static str {
    if tmdb_id.is_some() {
        "ON CONFLICT (user_id, tmdb_id, media_type) WHERE tmdb_id IS NOT NULL DO UPDATE"
    } else {
        "ON CONFLICT (user_id, work_id) WHERE tmdb_id IS NULL AND work_id IS NOT NULL DO UPDATE"
    }
}

fn favorite_upsert_sql(tmdb_id: Option<i32>) -> String {
    let conflict_clause = favorite_conflict_clause(tmdb_id);
    format!(
        "INSERT INTO user_favorites (user_id, work_id, tmdb_id, media_type) VALUES ($1, $2, $3, $4) {conflict_clause} SET work_id = EXCLUDED.work_id, tmdb_id = COALESCE(EXCLUDED.tmdb_id, user_favorites.tmdb_id)"
    )
}

fn history_conflict_clause(tmdb_id: Option<i32>) -> &'static str {
    if tmdb_id.is_some() {
        "ON CONFLICT (user_id, tmdb_id, media_type, season_number, episode_number) WHERE tmdb_id IS NOT NULL DO UPDATE"
    } else {
        "ON CONFLICT (user_id, work_id, season_number, episode_number) WHERE tmdb_id IS NULL AND work_id IS NOT NULL DO UPDATE"
    }
}

#[derive(Deserialize, Default)]
pub(super) struct MutationQuery {
    #[serde(
        alias = "api_key",
        alias = "ApiKey",
        alias = "X-Emby-Token",
        alias = "x-emby-token"
    )]
    api_key: Option<String>,
}

/// 声明客户端能力 (`POST /Sessions/Capabilities` & `POST /Sessions/Capabilities/Full`)
pub(super) async fn post_capabilities(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MutationQuery>,
    payload: Option<Json<Value>>,
) -> Result<StatusCode, (StatusCode, String)> {
    let context =
        super::auth::resolve_auth_context(&state, &headers, query.api_key.as_deref()).await?;
    state.client_capabilities.insert(
        context.token,
        json!({
            "UserId": context.user_id, "DeviceId": context.device_id,
            "DeviceName": context.device_name, "Client": context.client_name,
            "Capabilities": payload.map(|Json(value)| value).unwrap_or_else(|| json!({}))
        }),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// 注销当前会话令牌 (`POST /Sessions/Logout`)
pub(super) async fn post_logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MutationQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    if let Some(token) = super::auth::extract_emby_token(&headers, query.api_key.as_deref()) {
        let identity: Option<(String, Option<String>)> = sqlx::query_as(
            "DELETE FROM access_tokens WHERE token = $1 RETURNING user_id, device_id",
        )
        .bind(&token)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if let Some((user_id, device_id)) = identity {
            state.sessions.retain(|_, session| {
                session.user_id != user_id
                    || device_id.as_deref() != Some(session.device_id.as_str())
            });
            broadcast_sessions(&state, &user_id);
        }
        state.client_capabilities.invalidate(&token);
        super::auth::invalidate_token_cache(&state, &[token]).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default)]
pub(super) struct HideFromResumeQuery {
    #[serde(rename = "Hide", alias = "hide", default = "default_true")]
    hide: bool,
    #[serde(
        alias = "api_key",
        alias = "ApiKey",
        alias = "X-Emby-Token",
        alias = "x-emby-token"
    )]
    api_key: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize, Default)]
pub(super) struct UserDataUpdatePayload {
    #[serde(rename = "PlaybackPositionTicks", alias = "playbackPositionTicks")]
    playback_position_ticks: Option<i64>,
    #[serde(rename = "Played", alias = "played")]
    played: Option<bool>,
    #[serde(rename = "IsFavorite", alias = "isFavorite")]
    is_favorite: Option<bool>,
    #[serde(rename = "PlayCount", alias = "playCount")]
    play_count: Option<i32>,
    #[serde(rename = "LastPlayedDate", alias = "lastPlayedDate")]
    last_played_date: Option<chrono::DateTime<chrono::Utc>>,
}

/// 更新条目用户数据 (`POST /Users/:user_id/Items/:item_id/UserData`)
pub(super) async fn post_user_item_data(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MutationQuery>,
    Path((path_user_id, item_id)): Path<(String, String)>,
    Json(payload): Json<UserDataUpdatePayload>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id_with_query(
        &state,
        Some(&path_user_id),
        &headers,
        query.api_key.as_deref(),
    )
    .await?;
    let target = resolve_item_target(&state, &item_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "媒体不存在".to_string()))?;
    super::items::ensure_media_accessible(&state, &user_id, target.work_id).await?;

    let scope = played_scope_for_target(&item_id, &target);
    let supports_single_row = supports_single_row_progress(scope, &target.media_type);

    if let Some(pos) = payload.playback_position_ticks {
        if supports_single_row {
            let (season_number, episode_number) = match scope {
                PlayedHistoryScope::Work => (0, 1),
                PlayedHistoryScope::Season(season) => (season, 0),
                PlayedHistoryScope::Episode(season, episode) => (season, episode),
            };
            upsert_play_history(
                &state,
                PlayHistoryUpsert {
                    user_id: &user_id,
                    work_id: target.work_id,
                    tmdb_id: target.tmdb_id,
                    media_type: &target.media_type,
                    season_number,
                    episode_number,
                    position_ticks: if payload.played == Some(true) {
                        0
                    } else {
                        pos.max(0)
                    },
                    duration_ticks: 0,
                    is_played: payload.played.unwrap_or(false),
                    overwrite_played: payload.played.is_some(),
                },
            )
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        } else if let Some(played) = payload.played {
            let media = PlayedHistoryMedia {
                work_id: target.work_id,
                tmdb_id: target.tmdb_id,
                media_type: target.media_type.clone(),
            };
            set_play_history_scope(&state, &user_id, &media, scope, played)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
    } else if let Some(played) = payload.played {
        let media = PlayedHistoryMedia {
            work_id: target.work_id,
            tmdb_id: target.tmdb_id,
            media_type: target.media_type.clone(),
        };
        set_play_history_scope(&state, &user_id, &media, scope, played)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    } else if (payload.play_count.is_some() || payload.last_played_date.is_some())
        && supports_single_row
    {
        let (season_number, episode_number) = match scope {
            PlayedHistoryScope::Work => (0, 1),
            PlayedHistoryScope::Season(season) => (season, 0),
            PlayedHistoryScope::Episode(season, episode) => (season, episode),
        };
        upsert_play_history(
            &state,
            PlayHistoryUpsert {
                user_id: &user_id,
                work_id: target.work_id,
                tmdb_id: target.tmdb_id,
                media_type: &target.media_type,
                season_number,
                episode_number,
                position_ticks: 0,
                duration_ticks: 0,
                is_played: false,
                overwrite_played: false,
            },
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    if payload.play_count.is_some() || payload.last_played_date.is_some() {
        let scope = played_scope_for_target(&item_id, &target);
        let (season, episode) = match scope {
            PlayedHistoryScope::Work => (None, None),
            PlayedHistoryScope::Season(season) => (Some(season), None),
            PlayedHistoryScope::Episode(season, episode) => (Some(season), Some(episode)),
        };
        sqlx::query(
            "UPDATE play_histories SET play_count = COALESCE($5, play_count), last_played_at = COALESCE($6, last_played_at) WHERE user_id=$1 AND (work_id=$2 OR ($3::int4 IS NOT NULL AND tmdb_id=$3 AND media_type=$4)) AND ($7::int4 IS NULL OR season_number=$7) AND ($8::int4 IS NULL OR episode_number=$8)",
        )
        .bind(&user_id).bind(target.work_id).bind(target.tmdb_id).bind(&target.media_type)
        .bind(payload.play_count).bind(payload.last_played_date.as_ref()).bind(season).bind(episode)
        .execute(&state.db).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    if let Some(is_fav) = payload.is_favorite {
        return set_item_favorite_state(
            &state,
            &path_user_id,
            &headers,
            query.api_key.as_deref(),
            &item_id,
            is_fav,
        )
        .await;
    }

    let media = PlayedHistoryMedia {
        work_id: target.work_id,
        tmdb_id: target.tmdb_id,
        media_type: target.media_type.clone(),
    };
    user_data_response(
        &state,
        &user_id,
        &item_id,
        &media,
        played_scope_for_target(&item_id, &target),
    )
    .await
}

/// 从“继续观看”中隐藏 (`POST /Users/:user_id/Items/:item_id/HideFromResume`)
pub(super) async fn post_hide_from_resume(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<HideFromResumeQuery>,
    Path((path_user_id, item_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id_with_query(
        &state,
        Some(&path_user_id),
        &headers,
        query.api_key.as_deref(),
    )
    .await?;
    let target = resolve_item_target(&state, &item_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "媒体不存在".to_string()))?;
    super::items::ensure_media_accessible(&state, &user_id, target.work_id).await?;

    let media = PlayedHistoryMedia {
        work_id: target.work_id,
        tmdb_id: target.tmdb_id,
        media_type: target.media_type.clone(),
    };
    set_resume_hidden_scope(
        &state,
        &user_id,
        &media,
        played_scope_for_target(&item_id, &target),
        query.hide,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    user_data_response(
        &state,
        &user_id,
        &item_id,
        &media,
        played_scope_for_target(&item_id, &target),
    )
    .await
}

/// 查询活跃播放会话列表 (`GET /Sessions`)
pub(super) async fn get_sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MutationQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let context =
        super::auth::resolve_auth_context(&state, &headers, query.api_key.as_deref()).await?;

    Ok(Json(active_sessions(
        &state,
        if context.is_admin {
            None
        } else {
            Some(&context.user_id)
        },
    )))
}

pub(super) fn active_sessions(state: &AppState, user_id: Option<&str>) -> Value {
    Value::Array(state.sessions.iter()
        .filter(|entry| entry.is_active() && user_id.is_none_or(|id| entry.user_id == id))
        .map(|entry| {
            let s = entry.value();
            let capabilities = state.client_capabilities.iter()
                .find(|(_token, cap)| cap["UserId"] == s.user_id && cap["DeviceId"] == s.device_id)
                .map(|(_token, cap)| cap["Capabilities"].clone())
                .unwrap_or_else(|| json!({}));
            json!({"Id": s.session_id, "UserId": s.user_id, "Client": s.client_name,
                "DeviceId": s.device_id, "DeviceName": s.device_name,
                "SupportsRemoteControl": capabilities["SupportsRemoteControl"].as_bool().unwrap_or(false),
                "SupportedCommands": capabilities["SupportedCommands"].as_array().cloned().unwrap_or_default(),
                "PlayableMediaTypes": capabilities["PlayableMediaTypes"].as_array().cloned().unwrap_or_default(),
                "PlayState": {"PositionTicks": s.position_ticks, "CanSeek": true, "IsPaused": false, "IsMuted": false}})
        }).collect())
}

pub(super) fn broadcast_sessions(state: &AppState, user_id: &str) {
    state.session_hub.broadcast(
        Some(user_id),
        "Sessions",
        active_sessions(state, Some(user_id)),
    );
    state
        .session_hub
        .broadcast_admins("Sessions", active_sessions(state, None));
}

fn broadcast_user_data(state: &AppState, user_id: &str, item_id: &str, user_data: &Value) {
    let mut data = user_data.clone();
    if let Some(fields) = data.as_object_mut() {
        fields.insert("Key".into(), json!(item_id));
        fields.insert("ItemId".into(), json!(item_id));
    }
    state.session_hub.broadcast(
        Some(user_id),
        "UserDataChanged",
        json!({"UserId": user_id, "UserDataList": [data]}),
    );
}

async fn broadcast_playback_user_data(
    state: &AppState,
    user_id: &str,
    item_id: Option<&str>,
    target: Option<&ItemTarget>,
    _position_ticks: i64,
) {
    let (Some(item_id), Some(target)) = (item_id, target) else {
        return;
    };
    if !state.session_hub.has_user_connections(user_id) {
        return;
    }
    let media = PlayedHistoryMedia {
        work_id: target.work_id,
        tmdb_id: target.tmdb_id,
        media_type: target.media_type.clone(),
    };
    match item_user_data(
        state,
        user_id,
        &media,
        played_scope_for_target(item_id, target),
    )
    .await
    {
        Ok(data) => broadcast_user_data(state, user_id, item_id, &data),
        Err(error) => tracing::warn!("读取播放进度推送数据失败: {}", error),
    }
}

async fn user_data_response(
    state: &AppState,
    user_id: &str,
    item_id: &str,
    media: &PlayedHistoryMedia,
    scope: PlayedHistoryScope,
) -> Result<Json<Value>, (StatusCode, String)> {
    let data = item_user_data(state, user_id, media, scope)
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    broadcast_user_data(state, user_id, item_id, &data);
    Ok(Json(data))
}

/// 手动标记已看 (`POST /Users/:user_id/PlayedItems/:item_id`)
pub(super) async fn mark_item_played(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MutationQuery>,
    Path((path_user_id, item_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    set_item_played_state(
        &state,
        &path_user_id,
        &headers,
        query.api_key.as_deref(),
        &item_id,
        true,
    )
    .await
}

/// 取消已看标记 (`DELETE /Users/:user_id/PlayedItems/:item_id`)
pub(super) async fn unmark_item_played(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MutationQuery>,
    Path((path_user_id, item_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    set_item_played_state(
        &state,
        &path_user_id,
        &headers,
        query.api_key.as_deref(),
        &item_id,
        false,
    )
    .await
}

async fn set_item_played_state(
    state: &AppState,
    path_user_id: &str,
    headers: &HeaderMap,
    api_key: Option<&str>,
    item_id: &str,
    played: bool,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id =
        resolve_valid_user_id_with_query(state, Some(path_user_id), headers, api_key).await?;
    let target = resolve_item_target(state, item_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "媒体不存在".to_string()))?;
    super::items::ensure_media_accessible(state, &user_id, target.work_id).await?;
    let media = PlayedHistoryMedia {
        work_id: target.work_id,
        tmdb_id: target.tmdb_id,
        media_type: target.media_type.clone(),
    };
    let scope = played_scope_for_target(item_id, &target);
    set_play_history_scope(state, &user_id, &media, scope, played)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    user_data_response(
        state,
        &user_id,
        item_id,
        &media,
        played_scope_for_target(item_id, &target),
    )
    .await
}

/// 添加收藏 (`POST /Users/:user_id/FavoriteItems/:item_id`)
pub(super) async fn mark_item_favorite(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MutationQuery>,
    Path((path_user_id, item_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    set_item_favorite_state(
        &state,
        &path_user_id,
        &headers,
        query.api_key.as_deref(),
        &item_id,
        true,
    )
    .await
}

/// 取消收藏 (`DELETE /Users/:user_id/FavoriteItems/:item_id`)
pub(super) async fn unmark_item_favorite(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MutationQuery>,
    Path((path_user_id, item_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    set_item_favorite_state(
        &state,
        &path_user_id,
        &headers,
        query.api_key.as_deref(),
        &item_id,
        false,
    )
    .await
}

async fn set_item_favorite_state(
    state: &AppState,
    path_user_id: &str,
    headers: &HeaderMap,
    api_key: Option<&str>,
    item_id: &str,
    is_fav: bool,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id =
        resolve_valid_user_id_with_query(state, Some(path_user_id), headers, api_key).await?;
    let target = resolve_item_target(state, item_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "媒体不存在".to_string()))?;
    super::items::ensure_media_accessible(state, &user_id, target.work_id).await?;
    let wid = target.work_id;
    let tmdb_id = target.tmdb_id;
    let mtype = target.media_type.clone();
    {
        if is_fav {
            let mut tx = state
                .db
                .begin()
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            if let Some(tmdb_id) = tmdb_id {
                // Remove a legacy work-only duplicate before changing the canonical TMDB row's
                // work_id, otherwise both partial unique indexes can reject the same update.
                sqlx::query(
                    "DELETE FROM user_favorites legacy WHERE legacy.user_id = $1 AND legacy.work_id = $2 AND legacy.tmdb_id IS NULL AND EXISTS (SELECT 1 FROM user_favorites canonical WHERE canonical.user_id = $1 AND canonical.tmdb_id = $3 AND canonical.media_type = $4)",
                )
                .bind(&user_id)
                .bind(wid)
                .bind(tmdb_id)
                .bind(&mtype)
                .execute(&mut *tx)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            }
            let sql = favorite_upsert_sql(tmdb_id);
            sqlx::query(&sql)
                .bind(&user_id)
                .bind(wid)
                .bind(tmdb_id)
                .bind(&mtype)
                .execute(&mut *tx)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            tx.commit()
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        } else {
            sqlx::query(
                r#"
                DELETE FROM user_favorites
                WHERE user_id = $1
                  AND (($3::int4 IS NOT NULL AND tmdb_id = $3 AND media_type = $4) OR work_id = $2)
                "#,
            )
            .bind(&user_id)
            .bind(wid)
            .bind(tmdb_id)
            .bind(&mtype)
            .execute(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
    }

    let media = PlayedHistoryMedia {
        work_id: wid,
        tmdb_id,
        media_type: mtype.clone(),
    };
    user_data_response(
        state,
        &user_id,
        item_id,
        &media,
        played_scope_for_target(item_id, &target),
    )
    .await
}

#[derive(serde::Serialize, Deserialize, Default)]
pub(super) struct SessionReportPayload {
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
    #[serde(
        rename = "api_key",
        alias = "ApiKey",
        alias = "X-Emby-Token",
        alias = "x-emby-token"
    )]
    api_key: Option<String>,
}

fn merge_session_report(
    body: Option<SessionReportPayload>,
    query: &SessionReportPayload,
) -> SessionReportPayload {
    let mut body = body.unwrap_or_default();
    if body.item_id.is_none() {
        body.item_id.clone_from(&query.item_id);
    }
    if body.user_id.is_none() {
        body.user_id.clone_from(&query.user_id);
    }
    if body.play_session_id.is_none() {
        body.play_session_id.clone_from(&query.play_session_id);
    }
    if body.api_key.is_none() {
        body.api_key.clone_from(&query.api_key);
    }
    if body.position_ticks == 0 {
        body.position_ticks = query.position_ticks;
    }
    if body.run_time_ticks == 0 {
        body.run_time_ticks = query.run_time_ticks;
    }
    body
}

pub(super) async fn session_playing(
    state: State<Arc<AppState>>,
    headers: HeaderMap,
    query: Query<SessionReportPayload>,
    payload: Option<Json<SessionReportPayload>>,
) -> StatusCode {
    session_playing_with_identity(state, headers, query, payload, None).await
}

pub(super) async fn session_playing_with_identity(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q_report): Query<SessionReportPayload>,
    payload: Option<Json<SessionReportPayload>>,
    identity_override: Option<(String, String, String)>,
) -> StatusCode {
    let body = merge_session_report(payload.map(|Json(b)| b), &q_report);
    let Some(ref raw_item_id) = body.item_id else {
        return StatusCode::NO_CONTENT;
    };

    let user_id = match resolve_valid_user_id_with_query(
        &state,
        body.user_id.as_deref(),
        &headers,
        body.api_key.as_deref(),
    )
    .await
    {
        Ok(user_id) => user_id,
        Err((status, _)) => return status,
    };
    let target = match resolve_item_target(&state, raw_item_id).await {
        Ok(Some(target)) => Some(target),
        Ok(None) => return StatusCode::NOT_FOUND,
        Err(error) => {
            tracing::error!("解析播放条目失败: {}", error);
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };
    if let Some(target) = &target {
        if let Err((status, _)) =
            super::items::ensure_media_accessible(&state, &user_id, target.work_id).await
        {
            return status;
        }
    }

    let ep_id = match normalized_session_id(&state, raw_item_id, target.as_ref()).await {
        Ok(id) => id,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
    };
    let sid = body
        .play_session_id
        .unwrap_or_else(|| format!("sess_{}", raw_item_id));
    let (device_id, device_name, client_name) =
        identity_override.unwrap_or_else(|| super::auth::parse_emby_client_identity(&headers));

    if state
        .sessions
        .get(&sid)
        .is_some_and(|s| !session_owned_by(Some(&s.user_id), Some(&user_id)))
    {
        return StatusCode::FORBIDDEN;
    }
    // 同一用户同一设备重新开播时，移除旧会话避免残留僵尸会话。
    state.sessions.retain(|key, session| {
        key == &sid || !(session.user_id == user_id && session.device_id == device_id)
    });

    state.sessions.insert(
        sid.clone(),
        crate::state::PlaybackSession {
            session_id: sid,
            user_id: user_id.clone(),
            device_id,
            device_name,
            client_name,
            current_episode_id: ep_id,
            position_ticks: body.position_ticks,
            duration_ticks: body.run_time_ticks.max(0),
            next_ep_prewarmed: false,
            completed_in_session: false,
            last_activity: std::time::Instant::now(),
        },
    );

    if body.position_ticks > 0 {
        if let Some(ItemTarget {
            work_id: mid,
            tmdb_id,
            media_type: mtype,
            season_number: snum,
            episode_number: epnum,
        }) = target.clone()
        {
            if upsert_play_history(
                &state,
                PlayHistoryUpsert {
                    user_id: &user_id,
                    work_id: mid,
                    tmdb_id,
                    media_type: &mtype,
                    season_number: snum,
                    episode_number: epnum,
                    position_ticks: body.position_ticks,
                    duration_ticks: body.run_time_ticks,
                    is_played: false,
                    overwrite_played: false,
                },
            )
            .await
            .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
        }
    }

    broadcast_playback_user_data(
        &state,
        &user_id,
        body.item_id.as_deref(),
        target.as_ref(),
        body.position_ticks,
    )
    .await;
    broadcast_sessions(&state, &user_id);
    StatusCode::NO_CONTENT
}

pub(super) async fn session_progress(
    state: State<Arc<AppState>>,
    headers: HeaderMap,
    query: Query<SessionReportPayload>,
    payload: Option<Json<SessionReportPayload>>,
) -> StatusCode {
    session_progress_with_identity(state, headers, query, payload, None).await
}

pub(super) async fn session_progress_with_identity(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q_report): Query<SessionReportPayload>,
    payload: Option<Json<SessionReportPayload>>,
    identity_override: Option<(String, String, String)>,
) -> StatusCode {
    let body = merge_session_report(payload.map(|Json(b)| b), &q_report);
    let Some(ref raw_item_id) = body.item_id else {
        return StatusCode::NO_CONTENT;
    };

    let user_id = match resolve_valid_user_id_with_query(
        &state,
        body.user_id.as_deref(),
        &headers,
        body.api_key.as_deref(),
    )
    .await
    {
        Ok(user_id) => user_id,
        Err((status, _)) => return status,
    };
    let target = match resolve_item_target(&state, raw_item_id).await {
        Ok(Some(target)) => Some(target),
        Ok(None) => return StatusCode::NOT_FOUND,
        Err(error) => {
            tracing::error!("解析播放条目失败: {}", error);
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };
    if let Some(target) = &target {
        if let Err((status, _)) =
            super::items::ensure_media_accessible(&state, &user_id, target.work_id).await
        {
            return status;
        }
    }

    let ep_id = match normalized_session_id(&state, raw_item_id, target.as_ref()).await {
        Ok(id) => id,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
    };
    let sid = body
        .play_session_id
        .unwrap_or_else(|| format!("sess_{}", raw_item_id));
    if state
        .sessions
        .get(&sid)
        .is_some_and(|session| !session_owned_by(Some(&session.user_id), Some(&user_id)))
    {
        return StatusCode::FORBIDDEN;
    }
    let client_ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let (device_id, device_name, client_name) =
        identity_override.unwrap_or_else(|| super::auth::parse_emby_client_identity(&headers));

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

    // A completed episode from an earlier session is a rewatch; count it only on
    // this session's first completion, never on repeated >=90% heartbeats.
    let claimed_completion = if auto_completed && body.position_ticks > 0 && target.is_some() {
        let mut entry = state
            .sessions
            .entry(sid.clone())
            .or_insert_with(|| crate::state::PlaybackSession {
                session_id: sid.clone(),
                user_id: user_id.clone(),
                device_id: device_id.clone(),
                device_name: device_name.clone(),
                client_name: client_name.clone(),
                current_episode_id: ep_id,
                position_ticks: body.position_ticks,
                duration_ticks: body.run_time_ticks.max(0),
                next_ep_prewarmed: false,
                completed_in_session: false,
                last_activity: std::time::Instant::now(),
            });
        if entry.user_id != user_id {
            return StatusCode::FORBIDDEN;
        }
        if entry.completed_in_session {
            false
        } else {
            entry.completed_in_session = true;
            true
        }
    } else {
        false
    };

    // 2. 持久化播放进度到 play_histories 表
    if body.position_ticks > 0 {
        if let Some(ItemTarget {
            work_id: mid,
            tmdb_id,
            media_type: mtype,
            season_number: snum,
            episode_number: epnum,
        }) = target.clone()
        {
            let saved_pos = if auto_completed {
                0
            } else {
                body.position_ticks
            };
            if upsert_play_history_with_rewatch(
                &state,
                PlayHistoryUpsert {
                    user_id: &user_id,
                    work_id: mid,
                    tmdb_id,
                    media_type: &mtype,
                    season_number: snum,
                    episode_number: epnum,
                    position_ticks: saved_pos,
                    duration_ticks: body.run_time_ticks,
                    is_played: auto_completed,
                    overwrite_played: false,
                },
                claimed_completion,
            )
            .await
            .is_err()
            {
                if claimed_completion {
                    if let Some(mut entry) = state.sessions.get_mut(&sid) {
                        entry.completed_in_session = false;
                    }
                }
                tracing::error!("写入播放进度失败");
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
        }
    }

    // 3. 更新内存会话并在达到 85% 时静默预热下一集
    let mut should_prewarm_next = false;
    {
        let mut entry =
            state
                .sessions
                .entry(sid.clone())
                .or_insert_with(|| crate::state::PlaybackSession {
                    session_id: sid.clone(),
                    user_id: user_id.clone(),
                    device_id: device_id.clone(),
                    device_name: device_name.clone(),
                    client_name: client_name.clone(),
                    current_episode_id: ep_id,
                    position_ticks: body.position_ticks,
                    duration_ticks: body.run_time_ticks.max(0),
                    next_ep_prewarmed: false,
                    completed_in_session: false,
                    last_activity: std::time::Instant::now(),
                });
        if entry.user_id != user_id {
            return StatusCode::FORBIDDEN;
        }
        entry.device_id.clone_from(&device_id);
        entry.device_name.clone_from(&device_name);
        entry.client_name.clone_from(&client_name);
        entry.position_ticks = body.position_ticks;
        if body.run_time_ticks > 0 {
            entry.duration_ticks = body.run_time_ticks;
        }
        entry.last_activity = std::time::Instant::now();
        if auto_completed {
            entry.completed_in_session = true;
        }
        if reached_85_percent && !entry.next_ep_prewarmed {
            entry.next_ep_prewarmed = true;
            should_prewarm_next = true;
        }
    }

    if should_prewarm_next {
        let state_clone = state.clone();
        let target = target.clone();
        let raw_item_clone = raw_item_id.clone();
        tokio::spawn(async move {
            if let Some(target) = target {
                let ItemTarget {
                    work_id: wid,
                    media_type: mtype,
                    season_number: snum,
                    episode_number: epnum,
                    ..
                } = target;
                if mtype == "tv" {
                    let next_ep_result: Result<Option<(i64, String, i64)>, sqlx::Error> = sqlx::query_as(
                        r#"
                        SELECT sf.id, sf.alist_path, sf.path_hash
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
                    .bind(epnum.saturating_add(1))
                    .fetch_optional(&state_clone.db)
                    .await;
                    let next_ep = match next_ep_result {
                        Ok(next_ep) => next_ep,
                        Err(error) => {
                            tracing::error!("查询下一集预热源失败: {}", error);
                            return;
                        }
                    };

                    if let Some((next_src_id, next_path, next_hash)) = next_ep {
                        tracing::info!(
                            "⏭️ [NextEpPrewarm] 当前集 {} 进度已达 85%+，后台提前预热下一集 S{:02}E{:02} (src_{})",
                            raw_item_clone,
                            snum,
                            epnum.saturating_add(1),
                            next_src_id
                        );
                        let _ = state_clone
                            .resolve_playback_url(next_hash, &next_path, &client_ua)
                            .await;
                    }
                }
            }
        });
    }

    broadcast_playback_user_data(
        &state,
        &user_id,
        body.item_id.as_deref(),
        target.as_ref(),
        body.position_ticks,
    )
    .await;
    broadcast_sessions(&state, &user_id);
    StatusCode::NO_CONTENT
}

pub(super) async fn session_stopped(
    state: State<Arc<AppState>>,
    headers: HeaderMap,
    query: Query<SessionReportPayload>,
    payload: Option<Json<SessionReportPayload>>,
) -> StatusCode {
    session_stopped_with_identity(state, headers, query, payload, None).await
}

pub(super) async fn session_stopped_with_identity(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q_report): Query<SessionReportPayload>,
    payload: Option<Json<SessionReportPayload>>,
    _identity_override: Option<(String, String, String)>,
) -> StatusCode {
    let body = merge_session_report(payload.map(|Json(b)| b), &q_report);
    let resolved_uid = resolve_valid_user_id_with_query(
        &state,
        body.user_id.as_deref(),
        &headers,
        body.api_key.as_deref(),
    )
    .await;
    let authenticated_uid = match resolved_uid {
        Ok(user_id) => user_id,
        Err((status, _)) => return status,
    };
    let resolved_uid = authenticated_uid;
    if let Some(ref sid) = body.play_session_id {
        if state
            .sessions
            .get(sid)
            .is_some_and(|session| !session_owned_by(Some(&session.user_id), Some(&resolved_uid)))
        {
            return StatusCode::FORBIDDEN;
        }
    }
    let target = if let Some(raw_item_id) = body.item_id.as_deref() {
        let target = match resolve_item_target(&state, raw_item_id).await {
            Ok(Some(target)) => Some(target),
            Ok(None) => return StatusCode::NOT_FOUND,
            Err(error) => {
                tracing::error!("解析停止播放条目失败: {}", error);
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
        };
        if let Some(target) = &target {
            if let Err((status, _)) =
                super::items::ensure_media_accessible(&state, &resolved_uid, target.work_id).await
            {
                return status;
            }
        }
        target
    } else {
        None
    };
    if body.item_id.is_some() && body.position_ticks >= 0 {
        {
            let user_id = &resolved_uid;
            if let Some(ItemTarget {
                work_id: mid,
                tmdb_id,
                media_type: mtype,
                season_number: snum,
                episode_number: epnum,
            }) = target.clone()
            {
                let auto_completed = body.run_time_ticks > 0
                    && (body.position_ticks as f64 / body.run_time_ticks as f64) >= 0.90;
                let saved_pos = if auto_completed {
                    0
                } else {
                    body.position_ticks
                };
                if upsert_play_history(
                    &state,
                    PlayHistoryUpsert {
                        user_id,
                        work_id: mid,
                        tmdb_id,
                        media_type: &mtype,
                        season_number: snum,
                        episode_number: epnum,
                        position_ticks: saved_pos,
                        duration_ticks: body.run_time_ticks,
                        is_played: auto_completed,
                        overwrite_played: false,
                    },
                )
                .await
                .is_err()
                {
                    return StatusCode::INTERNAL_SERVER_ERROR;
                }
            }
        }
    }
    let uid = &resolved_uid;
    if let Some(ref sid) = body.play_session_id {
        if state
            .sessions
            .get(sid)
            .is_some_and(|s| session_owned_by(Some(&s.user_id), Some(uid)))
        {
            state.sessions.remove(sid);
        }
    }
    if let Some(ref raw_item_id) = body.item_id {
        let fallback_sid = format!("sess_{}", raw_item_id);
        if state
            .sessions
            .get(&fallback_sid)
            .is_some_and(|s| session_owned_by(Some(&s.user_id), Some(uid)))
        {
            state.sessions.remove(&fallback_sid);
        }
        {
            let uid = &resolved_uid;
            let matching_ids =
                match matching_session_ids(&state, raw_item_id, target.as_ref()).await {
                    Ok(ids) => ids,
                    Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
                };
            state.sessions.retain(|key, session| {
                if session.user_id != uid.as_str() {
                    return true;
                }
                if matching_ids.contains(&session.current_episode_id.unwrap_or(i64::MIN)) {
                    return false;
                }
                let Some(session_item) = key.strip_prefix("sess_") else {
                    return true;
                };
                !session_item_matches(session_item, raw_item_id)
            });
        }
    }
    broadcast_playback_user_data(
        &state,
        uid,
        body.item_id.as_deref(),
        target.as_ref(),
        body.position_ticks,
    )
    .await;
    broadcast_sessions(&state, uid);
    StatusCode::NO_CONTENT
}

/// 将 `work_123` / `ep_123_1_2` / `src_456` 解析为 `(work_id, tmdb_id, media_type, season_number, episode_number)`
fn history_key(item_id: &str) -> Option<(i64, i32, i32)> {
    if let Some((wid, season, episode)) = parse_episode_id(item_id) {
        return Some((wid, season, episode));
    }
    if let Some((wid, season)) = parse_season_id(item_id) {
        return Some((wid, season, 0));
    }
    parse_work_id(item_id).map(|wid| (wid, 0, 1))
}

fn session_owned_by(owner: Option<&str>, authenticated_user: Option<&str>) -> bool {
    matches!((owner, authenticated_user), (Some(owner), Some(user)) if owner == user)
}

fn session_item_matches(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let (Some((lw, _, _)), Some((rw, _, _))) = (history_key(left), history_key(right)) else {
        return false;
    };
    if lw != rw {
        return false;
    }
    let (Some(ls), Some(rs)) = (item_history_scope(left), item_history_scope(right)) else {
        return false;
    };
    let overlaps = |a: PlayedHistoryScope, b: PlayedHistoryScope| match (a, b) {
        (PlayedHistoryScope::Work, _) | (_, PlayedHistoryScope::Work) => true,
        (PlayedHistoryScope::Season(a), PlayedHistoryScope::Season(b)) => a == b,
        (PlayedHistoryScope::Season(s), PlayedHistoryScope::Episode(es, _))
        | (PlayedHistoryScope::Episode(es, _), PlayedHistoryScope::Season(s)) => s == es,
        (PlayedHistoryScope::Episode(as_, ae), PlayedHistoryScope::Episode(bs, be)) => {
            as_ == bs && ae == be
        }
    };
    overlaps(ls, rs)
}

async fn matching_session_ids(
    state: &AppState,
    item_id: &str,
    target: Option<&ItemTarget>,
) -> Result<HashSet<i64>, sqlx::Error> {
    let mut ids = HashSet::new();
    let Some(target) = target else {
        if let Some(work_id) = parse_work_id(item_id) {
            ids.insert(-work_id);
        }
        return Ok(ids);
    };
    let wid = target.work_id;
    ids.insert(-wid);
    let scope = played_scope_for_target(item_id, target);
    let (scope_sql, season, episode) = match scope {
        PlayedHistoryScope::Work => ("TRUE", None, None),
        PlayedHistoryScope::Season(season) => ("sf.season_number = $2", Some(season), None),
        PlayedHistoryScope::Episode(season, episode) => (
            "sf.season_number = $2 AND sf.episode_number = $3",
            Some(season),
            Some(episode),
        ),
    };
    let sql = format!(
        "SELECT sf.id FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE f.work_id = $1 AND f.is_ignored = FALSE AND ({scope_sql})"
    );
    let mut query = sqlx::query_scalar(&sql).bind(wid);
    if let Some(season) = season {
        query = query.bind(season);
    }
    if let Some(episode) = episode {
        query = query.bind(episode);
    }
    let source_ids: Vec<i64> = query.fetch_all(&state.db).await?;
    ids.extend(source_ids);
    Ok(ids)
}

async fn normalized_session_id(
    state: &AppState,
    item_id: &str,
    target: Option<&ItemTarget>,
) -> Result<Option<i64>, sqlx::Error> {
    let Some(target) = target else {
        return Ok(parse_work_id(item_id).map(|work_id| -work_id));
    };
    if item_id.starts_with("src_") {
        // resolve_item_target has already validated this exact source as linked and non-ignored.
        return Ok(item_id.strip_prefix("src_").and_then(|id| id.parse().ok()));
    }
    let wid = target.work_id;
    let scope = played_scope_for_target(item_id, target);
    let (scope_sql, season, episode) = match scope {
        PlayedHistoryScope::Work => ("TRUE", None, None),
        PlayedHistoryScope::Season(season) => ("sf.season_number = $2", Some(season), None),
        PlayedHistoryScope::Episode(season, episode) => (
            "sf.season_number = $2 AND sf.episode_number = $3",
            Some(season),
            Some(episode),
        ),
    };
    let sql = format!(
        "SELECT sf.id FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE f.work_id = $1 AND f.is_ignored = FALSE AND ({scope_sql}) ORDER BY COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) DESC, sf.id ASC LIMIT 1"
    );
    let mut query = sqlx::query_scalar(&sql).bind(wid);
    if let Some(season) = season {
        query = query.bind(season);
    }
    if let Some(episode) = episode {
        query = query.bind(episode);
    }
    let source_id: Option<i64> = query.fetch_optional(&state.db).await?;
    Ok(source_id.or(Some(-wid)))
}

#[derive(Debug, Clone)]
struct ItemTarget {
    work_id: i64,
    tmdb_id: Option<i32>,
    media_type: String,
    season_number: i32,
    episode_number: i32,
}

async fn resolve_item_target(
    state: &AppState,
    item_id: &str,
) -> Result<Option<ItemTarget>, sqlx::Error> {
    if item_id.starts_with("src_") {
        return resolve_source_item_target(state, item_id).await;
    }
    if let Some((wid, snum, epnum)) = history_key(item_id) {
        let Some((tmdb_id, mtype)): Option<(Option<i32>, String)> =
            sqlx::query_as("SELECT tmdb_id, media_type FROM media_works WHERE id = $1")
                .bind(wid)
                .fetch_optional(&state.db)
                .await?
        else {
            return Ok(None);
        };
        return Ok(Some(ItemTarget {
            work_id: wid,
            tmdb_id,
            media_type: mtype,
            season_number: snum,
            episode_number: epnum,
        }));
    }
    Ok(None)
}

async fn resolve_source_item_target(
    state: &AppState,
    item_id: &str,
) -> Result<Option<ItemTarget>, sqlx::Error> {
    let Some(sid) = item_id
        .strip_prefix("src_")
        .and_then(|s| s.parse::<i64>().ok())
    else {
        return Ok(None);
    };
    let Some((wid, snum, epnum, tmdb_id, mtype)): Option<(i64, i32, i32, Option<i32>, String)> =
        sqlx::query_as(
            "SELECT f.work_id, sf.season_number, sf.episode_number, m.tmdb_id, m.media_type FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id JOIN media_works m ON m.id = f.work_id WHERE sf.id = $1 AND f.work_id IS NOT NULL AND f.is_ignored = FALSE",
        )
        .bind(sid)
        .fetch_optional(&state.db)
        .await?
    else {
        return Ok(None);
    };
    Ok(Some(ItemTarget {
        work_id: wid,
        tmdb_id,
        media_type: mtype,
        season_number: snum,
        episode_number: epnum,
    }))
}

struct PlayedHistoryMedia {
    work_id: i64,
    tmdb_id: Option<i32>,
    media_type: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlayedHistoryScope {
    Work,
    Season(i32),
    Episode(i32, i32),
}

fn played_scope_for_target(item_id: &str, target: &ItemTarget) -> PlayedHistoryScope {
    item_history_scope(item_id).unwrap_or_else(|| {
        if item_id.starts_with("src_") && target.media_type == "tv" {
            PlayedHistoryScope::Episode(target.season_number, target.episode_number)
        } else {
            PlayedHistoryScope::Work
        }
    })
}

fn supports_single_row_progress(scope: PlayedHistoryScope, media_type: &str) -> bool {
    matches!(scope, PlayedHistoryScope::Episode(_, _))
        || (matches!(scope, PlayedHistoryScope::Work) && media_type == "movie")
}

fn item_history_scope(item_id: &str) -> Option<PlayedHistoryScope> {
    if parse_work_id(item_id).is_some() {
        Some(PlayedHistoryScope::Work)
    } else if let Some((_, season)) = parse_season_id(item_id) {
        Some(PlayedHistoryScope::Season(season))
    } else {
        parse_episode_id(item_id)
            .map(|(_, season, episode)| PlayedHistoryScope::Episode(season, episode))
    }
}

async fn set_resume_hidden_scope(
    state: &AppState,
    user_id: &str,
    media: &PlayedHistoryMedia,
    scope: PlayedHistoryScope,
    hide: bool,
) -> Result<(), sqlx::Error> {
    let (season, episode) = match scope {
        PlayedHistoryScope::Work => (None, None),
        PlayedHistoryScope::Season(s) => (Some(s), None),
        PlayedHistoryScope::Episode(s, e) => (Some(s), Some(e)),
    };
    let mut tx = state.db.begin().await?;
    // NextUp 的下一集可能还没有播放历史，为现有目标分集创建隐藏状态。
    if hide {
        sqlx::query(
            "INSERT INTO play_histories (user_id,work_id,tmdb_id,media_type,season_number,episode_number,hidden_from_resume)
             SELECT $1,$2,$3,$4,p.season_number,p.episode_number,TRUE FROM (
               SELECT DISTINCT sf.season_number,sf.episode_number FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id
               WHERE f.work_id=$2 AND f.is_ignored=FALSE AND ($5::int4 IS NULL OR sf.season_number=$5) AND ($6::int4 IS NULL OR sf.episode_number=$6)
               UNION SELECT COALESCE($5,0),COALESCE($6,1) WHERE $4='movie' OR $6 IS NOT NULL
             ) p ON CONFLICT DO NOTHING",
        ).bind(user_id).bind(media.work_id).bind(media.tmdb_id).bind(&media.media_type)
            .bind(season).bind(episode).execute(&mut *tx).await?;
    }
    sqlx::query(
        "UPDATE play_histories SET hidden_from_resume=$5 WHERE user_id=$1 AND (work_id=$2 OR ($3::int4 IS NOT NULL AND tmdb_id=$3 AND media_type=$4)) AND ($6::int4 IS NULL OR season_number=$6) AND ($7::int4 IS NULL OR episode_number=$7)",
    ).bind(user_id).bind(media.work_id).bind(media.tmdb_id).bind(&media.media_type)
        .bind(hide).bind(season).bind(episode).execute(&mut *tx).await?;
    tx.commit().await
}

async fn item_user_data(
    state: &AppState,
    user_id: &str,
    media: &PlayedHistoryMedia,
    scope: PlayedHistoryScope,
) -> Result<Value, sqlx::Error> {
    let history = query_play_history_summary(state, user_id, media, scope).await?;
    let favorite: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_favorites WHERE user_id=$1 AND (work_id=$2 OR ($3::int4 IS NOT NULL AND tmdb_id=$3 AND media_type=$4)))",
    ).bind(user_id).bind(media.work_id).bind(media.tmdb_id).bind(&media.media_type)
        .fetch_one(&state.db).await?;
    let unplayed: i64 = if media.media_type == "tv" {
        let season = match scope {
            PlayedHistoryScope::Season(season) => Some(season),
            _ => None,
        };
        sqlx::query_scalar(
            "SELECT COUNT(DISTINCT (sf.season_number,sf.episode_number)) FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id WHERE f.work_id=$1 AND f.is_ignored=FALSE AND sf.episode_number>0 AND ($2::int4 IS NULL OR sf.season_number=$2) AND NOT EXISTS (SELECT 1 FROM play_histories ph WHERE ph.user_id=$3 AND (ph.work_id=$1 OR ($4::int4 IS NOT NULL AND ph.tmdb_id=$4 AND ph.media_type=$5)) AND ph.season_number=sf.season_number AND ph.episode_number=sf.episode_number AND ph.is_played=TRUE)",
        ).bind(media.work_id).bind(season).bind(user_id).bind(media.tmdb_id).bind(&media.media_type)
            .fetch_one(&state.db).await?
    } else {
        0
    };
    Ok(
        json!({"PlaybackPositionTicks":history.0,"Played":history.1,"PlayCount":history.2,
        "LastPlayedDate":history.3,"IsFavorite":favorite,"UnplayedItemCount":unplayed}),
    )
}

async fn query_play_history_summary(
    state: &AppState,
    user_id: &str,
    media: &PlayedHistoryMedia,
    scope: PlayedHistoryScope,
) -> Result<(i64, bool, i32, Option<chrono::DateTime<chrono::Utc>>), sqlx::Error> {
    let scope_sql = match scope {
        PlayedHistoryScope::Work if media.media_type == "tv" => "TRUE",
        PlayedHistoryScope::Work => "season_number = 0 AND episode_number = 1",
        PlayedHistoryScope::Season(_) => "season_number = $5",
        PlayedHistoryScope::Episode(_, _) => "season_number = $5 AND episode_number = $6",
    };
    let played_sql = match scope {
        PlayedHistoryScope::Work if media.media_type == "tv" => {
            "(EXISTS (SELECT 1 FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id WHERE f.work_id=$2 AND f.is_ignored=FALSE) AND NOT EXISTS (SELECT 1 FROM (SELECT DISTINCT sf.season_number,sf.episode_number FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id WHERE f.work_id=$2 AND f.is_ignored=FALSE) episodes WHERE NOT EXISTS (SELECT 1 FROM play_histories ph WHERE ph.user_id=$1 AND (ph.work_id=$2 OR ($3::int4 IS NOT NULL AND ph.tmdb_id=$3 AND ph.media_type=$4)) AND ph.season_number=episodes.season_number AND ph.episode_number=episodes.episode_number AND ph.is_played=TRUE)))"
        }
        PlayedHistoryScope::Season(_) if media.media_type == "tv" => {
            "(EXISTS (SELECT 1 FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id WHERE f.work_id=$2 AND f.is_ignored=FALSE AND sf.season_number=$5) AND NOT EXISTS (SELECT 1 FROM (SELECT DISTINCT sf.season_number,sf.episode_number FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id WHERE f.work_id=$2 AND f.is_ignored=FALSE AND sf.season_number=$5) episodes WHERE NOT EXISTS (SELECT 1 FROM play_histories ph WHERE ph.user_id=$1 AND (ph.work_id=$2 OR ($3::int4 IS NOT NULL AND ph.tmdb_id=$3 AND ph.media_type=$4)) AND ph.season_number=episodes.season_number AND ph.episode_number=episodes.episode_number AND ph.is_played=TRUE)))"
        }
        _ => "COALESCE(BOOL_AND(is_played), FALSE)",
    };
    let sql = format!(
        "SELECT COALESCE(MAX(position_ticks), 0), {played_sql}, COALESCE(MAX(play_count), 0), MAX(last_played_at) FROM play_histories WHERE user_id = $1 AND (work_id = $2 OR ($3::int4 IS NOT NULL AND tmdb_id = $3 AND media_type = $4)) AND ({scope_sql})"
    );
    let mut query = sqlx::query_as(&sql)
        .bind(user_id)
        .bind(media.work_id)
        .bind(media.tmdb_id)
        .bind(&media.media_type);
    match scope {
        PlayedHistoryScope::Work => {}
        PlayedHistoryScope::Season(season) => query = query.bind(season),
        PlayedHistoryScope::Episode(season, episode) => {
            query = query.bind(season).bind(episode);
        }
    }
    query.fetch_one(&state.db).await
}

async fn set_play_history_scope(
    state: &AppState,
    user_id: &str,
    media: &PlayedHistoryMedia,
    scope: PlayedHistoryScope,
    played: bool,
) -> Result<(), sqlx::Error> {
    let season_filter = match scope {
        PlayedHistoryScope::Work => 0,
        PlayedHistoryScope::Season(season) => season,
        PlayedHistoryScope::Episode(season, episode) => {
            return upsert_play_history(
                state,
                PlayHistoryUpsert {
                    user_id,
                    work_id: media.work_id,
                    tmdb_id: media.tmdb_id,
                    media_type: &media.media_type,
                    season_number: season,
                    episode_number: episode,
                    position_ticks: 0,
                    duration_ticks: 0,
                    is_played: played,
                    overwrite_played: true,
                },
            )
            .await;
        }
    };
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE play_histories SET play_count = LEAST(play_count::bigint + CASE WHEN $5 AND NOT is_played THEN 1 ELSE 0 END, 2147483647)::int, is_played = $5, position_ticks = CASE WHEN $5 THEN 0 ELSE position_ticks END, last_played_at = NOW() WHERE user_id = $1 AND (work_id = $2 OR ($3::int4 IS NOT NULL AND tmdb_id = $3 AND media_type = $4)) AND ($7 OR season_number = $6)",
    ).bind(user_id).bind(media.work_id).bind(media.tmdb_id).bind(&media.media_type).bind(played).bind(season_filter).bind(matches!(scope, PlayedHistoryScope::Work)).execute(&mut *tx).await?;

    let pair_scope = match scope {
        PlayedHistoryScope::Work if media.media_type == "tv" => {
            "SELECT DISTINCT sf.season_number, sf.episode_number FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE f.work_id = $2 AND f.is_ignored = FALSE AND sf.episode_number > 0"
        }
        PlayedHistoryScope::Work => "SELECT 0::int4 AS season_number, 1::int4 AS episode_number",
        PlayedHistoryScope::Season(_) => {
            "SELECT DISTINCT sf.season_number, sf.episode_number FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE f.work_id = $2 AND f.is_ignored = FALSE AND sf.season_number = $6 AND sf.episode_number > 0"
        }
        PlayedHistoryScope::Episode(season, episode) => {
            let _ = (season, episode);
            "SELECT $6::int4 AS season_number, $7::int4 AS episode_number"
        }
    };
    let insert_sql = format!(
        "INSERT INTO play_histories (user_id, work_id, tmdb_id, media_type, season_number, episode_number, position_ticks, duration_ticks, is_played, play_count, last_played_at) SELECT $1, $2, $3, $4, pairs.season_number, pairs.episode_number, 0, 0, $5, CASE WHEN $5 THEN 1 ELSE 0 END, NOW() FROM ({pair_scope}) pairs WHERE NOT EXISTS (SELECT 1 FROM play_histories ph WHERE ph.user_id = $1 AND ph.work_id = $2 AND ph.season_number = pairs.season_number AND ph.episode_number = pairs.episode_number) ON CONFLICT DO NOTHING"
    );
    let mut insert_query = sqlx::query(&insert_sql)
        .bind(user_id)
        .bind(media.work_id)
        .bind(media.tmdb_id)
        .bind(&media.media_type)
        .bind(played);
    if matches!(
        scope,
        PlayedHistoryScope::Season(_) | PlayedHistoryScope::Episode(_, _)
    ) {
        insert_query = insert_query.bind(season_filter);
    }
    if let PlayedHistoryScope::Episode(_, episode) = scope {
        insert_query = insert_query.bind(episode);
    }
    insert_query.execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

struct PlayHistoryUpsert<'a> {
    user_id: &'a str,
    work_id: i64,
    tmdb_id: Option<i32>,
    media_type: &'a str,
    season_number: i32,
    episode_number: i32,
    position_ticks: i64,
    duration_ticks: i64,
    is_played: bool,
    overwrite_played: bool,
}

/// 写入或更新播放进度（同时兼容有/无 tmdb_id 的条目）
async fn upsert_play_history(
    state: &AppState,
    input: PlayHistoryUpsert<'_>,
) -> Result<(), sqlx::Error> {
    upsert_play_history_with_rewatch(state, input, false).await
}

async fn upsert_play_history_with_rewatch(
    state: &AppState,
    input: PlayHistoryUpsert<'_>,
    increment_rewatch: bool,
) -> Result<(), sqlx::Error> {
    let PlayHistoryUpsert {
        user_id,
        work_id,
        tmdb_id,
        media_type,
        season_number,
        episode_number,
        position_ticks,
        duration_ticks,
        is_played,
        overwrite_played,
    } = input;
    {
        let mut tx = state.db.begin().await?;
        if let Some(tmdb_id) = tmdb_id {
            let has_legacy_row: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM play_histories WHERE user_id = $1 AND work_id = $2 AND tmdb_id IS NULL AND season_number = $3 AND episode_number = $4)",
            )
            .bind(user_id)
            .bind(work_id)
            .bind(season_number)
            .bind(episode_number)
            .fetch_one(&mut *tx)
            .await?;
            if has_legacy_row {
                // Reconcile an old work-only row before promoting its identity. When both
                // identities exist, preserve the richer history on the canonical TMDB row.
                sqlx::query(
                "UPDATE play_histories canonical SET play_count = GREATEST(canonical.play_count, legacy.play_count), position_ticks = GREATEST(canonical.position_ticks, legacy.position_ticks), duration_ticks = GREATEST(canonical.duration_ticks, legacy.duration_ticks), is_played = canonical.is_played OR legacy.is_played, last_played_at = GREATEST(canonical.last_played_at, legacy.last_played_at) FROM play_histories legacy WHERE canonical.user_id = $1 AND canonical.tmdb_id = $3 AND canonical.media_type = $4 AND canonical.season_number = $5 AND canonical.episode_number = $6 AND legacy.user_id = $1 AND legacy.work_id = $2 AND legacy.tmdb_id IS NULL AND legacy.season_number = $5 AND legacy.episode_number = $6 AND EXISTS (SELECT 1 FROM play_histories legacy_check WHERE legacy_check.user_id=$1 AND legacy_check.work_id=$2 AND legacy_check.tmdb_id IS NULL AND legacy_check.season_number=$5 AND legacy_check.episode_number=$6)",
            )
            .bind(user_id)
            .bind(work_id)
            .bind(tmdb_id)
            .bind(media_type)
            .bind(season_number)
            .bind(episode_number)
            .execute(&mut *tx)
            .await?;
                sqlx::query(
                "DELETE FROM play_histories legacy WHERE legacy.user_id = $1 AND legacy.work_id = $2 AND legacy.tmdb_id IS NULL AND legacy.season_number = $5 AND legacy.episode_number = $6 AND EXISTS (SELECT 1 FROM play_histories canonical WHERE canonical.user_id = $1 AND canonical.tmdb_id = $3 AND canonical.media_type = $4 AND canonical.season_number = $5 AND canonical.episode_number = $6)",
            )
            .bind(user_id)
            .bind(work_id)
            .bind(tmdb_id)
            .bind(media_type)
            .bind(season_number)
            .bind(episode_number)
            .execute(&mut *tx)
            .await?;
                sqlx::query(
                "UPDATE play_histories SET tmdb_id = $3, media_type = $4 WHERE user_id = $1 AND work_id = $2 AND tmdb_id IS NULL AND season_number = $5 AND episode_number = $6 AND NOT EXISTS (SELECT 1 FROM play_histories canonical WHERE canonical.user_id = $1 AND canonical.tmdb_id = $3 AND canonical.media_type = $4 AND canonical.season_number = $5 AND canonical.episode_number = $6)",
            )
            .bind(user_id)
            .bind(work_id)
            .bind(tmdb_id)
            .bind(media_type)
            .bind(season_number)
            .bind(episode_number)
            .execute(&mut *tx)
            .await?;
            }
        }
        let conflict_clause = history_conflict_clause(tmdb_id);
        let insert_sql = format!(
            "INSERT INTO play_histories (user_id, work_id, tmdb_id, media_type, season_number, episode_number, position_ticks, duration_ticks, is_played, play_count, last_played_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,CASE WHEN $9 THEN 1 ELSE 0 END,NOW()) {conflict_clause} SET hidden_from_resume = FALSE, play_count = LEAST(play_histories.play_count::bigint + CASE WHEN EXCLUDED.is_played AND (NOT play_histories.is_played OR $11) THEN 1 ELSE 0 END, 2147483647)::int, work_id = EXCLUDED.work_id, tmdb_id = COALESCE(EXCLUDED.tmdb_id, play_histories.tmdb_id), position_ticks = EXCLUDED.position_ticks, duration_ticks = CASE WHEN EXCLUDED.duration_ticks > 0 THEN EXCLUDED.duration_ticks ELSE play_histories.duration_ticks END, is_played = CASE WHEN $10 THEN EXCLUDED.is_played ELSE (EXCLUDED.is_played OR play_histories.is_played) END, last_played_at = NOW()"
        );
        sqlx::query(&insert_sql)
            .bind(user_id)
            .bind(work_id)
            .bind(tmdb_id)
            .bind(media_type)
            .bind(season_number)
            .bind(episode_number)
            .bind(position_ticks)
            .bind(duration_ticks)
            .bind(is_played)
            .bind(overwrite_played)
            .bind(increment_rewatch)
            .execute(&mut *tx)
            .await?;
        if media_type == "tv" && overwrite_played && !is_played && episode_number > 0 {
            sqlx::query(
                "UPDATE play_histories SET is_played = FALSE, position_ticks = 0 WHERE user_id = $1 AND (work_id = $2 OR ($3::int4 IS NOT NULL AND tmdb_id = $3 AND media_type = $4)) AND ((season_number = 0 AND episode_number = 1) OR (season_number = $5 AND episode_number = 0))",
            )
            .bind(user_id).bind(work_id).bind(tmdb_id).bind(media_type)
            .bind(season_number).execute(&mut *tx).await?;
        }
        tx.commit().await?;
    }

    Ok(())
}

pub(super) type FavoriteKeys = (HashSet<i64>, HashMap<String, HashSet<i32>>);

pub(super) async fn load_user_favorite_keys(
    state: &AppState,
    user_id: &str,
) -> Result<FavoriteKeys, (StatusCode, String)> {
    let rows: Vec<(Option<i64>, Option<i32>, String)> = sqlx::query_as(
        "SELECT work_id, tmdb_id, media_type FROM user_favorites WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await
    .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    let mut wids = HashSet::new();
    let mut tmdbs: HashMap<String, HashSet<i32>> = HashMap::new();
    for (wid, tid, mtype) in rows {
        if let Some(work_id) = wid {
            wids.insert(work_id);
        }
        if let Some(tmdb_id) = tid {
            tmdbs.entry(mtype).or_default().insert(tmdb_id);
        }
    }
    Ok((wids, tmdbs))
}

pub(super) fn is_media_favorited(
    fav_keys: &FavoriteKeys,
    work_id: i64,
    tmdb_id: Option<i32>,
    media_type: &str,
) -> bool {
    if fav_keys.0.contains(&work_id) {
        return true;
    }
    if let Some(tid) = tmdb_id {
        if fav_keys
            .1
            .get(media_type)
            .is_some_and(|ids| ids.contains(&tid))
        {
            return true;
        }
    }
    false
}

#[derive(Clone, Default, sqlx::FromRow)]
pub(super) struct HistoryUserData {
    pub(super) position_ticks: i64,
    pub(super) duration_ticks: i64,
    pub(super) is_played: bool,
    pub(super) play_count: i32,
    pub(super) last_played_date: Option<chrono::DateTime<chrono::Utc>>,
}

pub(super) async fn load_media_episode_user_data(
    state: &AppState,
    user_id: &str,
    work_id: i64,
    tmdb_id: Option<i32>,
    media_type: &str,
) -> Result<HashMap<(i32, i32), HistoryUserData>, (StatusCode, String)> {
    type EpisodeUserDataRow = (
        i32,
        i32,
        i64,
        i64,
        bool,
        i32,
        Option<chrono::DateTime<chrono::Utc>>,
    );
    let rows: Vec<EpisodeUserDataRow> =
        sqlx::query_as("SELECT season_number, episode_number, position_ticks, duration_ticks, is_played, play_count, last_played_at FROM play_histories WHERE user_id=$1 AND (work_id=$2 OR ($3::int4 IS NOT NULL AND tmdb_id=$3 AND media_type=$4)) ORDER BY (work_id=$2) DESC, last_played_at DESC")
        .bind(user_id).bind(work_id).bind(tmdb_id).bind(media_type).fetch_all(&state.db).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
    let mut result = HashMap::new();
    for (s, e, position_ticks, duration_ticks, is_played, play_count, last_played_date) in rows {
        result.entry((s, e)).or_insert(HistoryUserData {
            position_ticks,
            duration_ticks,
            is_played,
            play_count,
            last_played_date,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod session_fix_tests {
    use super::*;

    #[tokio::test]
    async fn session_and_user_data_pushes_filter_users_and_expired_entries() {
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/test")
            .unwrap();
        let (tx, _) = tokio::sync::mpsc::channel(1);
        let state = AppState::new(
            crate::config::BootstrapFileConfig::default().into_dynamic(),
            db,
            tx,
        )
        .unwrap();
        let session = crate::state::PlaybackSession {
            session_id: "active".into(),
            user_id: "alice".into(),
            device_id: "device".into(),
            device_name: "device".into(),
            client_name: "test".into(),
            current_episode_id: None,
            position_ticks: 0,
            duration_ticks: 0,
            next_ep_prewarmed: false,
            completed_in_session: false,
            last_activity: std::time::Instant::now(),
        };
        state.sessions.insert("active".into(), session.clone());
        state.client_capabilities.insert(
            "alice-token".into(),
            json!({"UserId":"alice","DeviceId":"device","Capabilities":{
                "SupportsRemoteControl":true,"SupportedCommands":["Play"],"PlayableMediaTypes":["Video"]
            }}),
        );
        let active = active_sessions(&state, Some("alice"));
        assert_eq!(active[0]["SupportsRemoteControl"], true);
        assert_eq!(active[0]["SupportedCommands"][0], "Play");
        assert_eq!(active[0]["PlayableMediaTypes"][0], "Video");
        state.sessions.insert(
            "expired".into(),
            crate::state::PlaybackSession {
                session_id: "expired".into(),
                last_activity: std::time::Instant::now() - std::time::Duration::from_secs(61),
                ..session.clone()
            },
        );
        state.sessions.insert(
            "bob".into(),
            crate::state::PlaybackSession {
                session_id: "bob".into(),
                user_id: "bob".into(),
                ..session
            },
        );
        assert_eq!(
            active_sessions(&state, Some("alice"))
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(active_sessions(&state, None).as_array().unwrap().len(), 2);
        assert!(
            state.sessions.contains_key("expired"),
            "读取活跃列表不扫描清理共享状态"
        );
        let (_, mut alice, _) = state.session_hub.register("a".into(), "alice".into());
        let (_, mut bob, _) = state.session_hub.register("b".into(), "bob".into());
        broadcast_user_data(
            &state,
            "alice",
            "ep_1_1_1",
            &json!({"IsFavorite": true, "Played": true, "PlaybackPositionTicks":42}),
        );
        let message: Value = serde_json::from_str(&alice.recv().await.unwrap().message).unwrap();
        assert_eq!(message["MessageType"], "UserDataChanged");
        assert_eq!(message["Data"]["UserId"], "alice");
        assert_eq!(message["Data"]["UserDataList"][0]["ItemId"], "ep_1_1_1");
        assert_eq!(message["Data"]["UserDataList"][0]["IsFavorite"], true);
        assert!(bob.try_recv().is_err());
    }

    #[test]
    fn session_body_inherits_query_authentication_fields() {
        let query = SessionReportPayload {
            user_id: Some("query-user".into()),
            api_key: Some("query-token".into()),
            ..Default::default()
        };
        let body = SessionReportPayload {
            item_id: Some("work_1".into()),
            ..Default::default()
        };
        let merged = merge_session_report(Some(body), &query);
        assert_eq!(merged.user_id.as_deref(), Some("query-user"));
        assert_eq!(merged.api_key.as_deref(), Some("query-token"));
    }

    #[test]
    fn favorites_lookup_is_keyed_by_media_type() {
        let favorites = (
            HashSet::new(),
            HashMap::from([("tv".to_string(), HashSet::from([77]))]),
        );
        assert!(is_media_favorited(&favorites, 1, Some(77), "tv"));
        assert!(!is_media_favorited(&favorites, 1, Some(77), "movie"));
    }

    #[test]
    fn favorite_upsert_updates_only_favorite_identity() {
        for tmdb_id in [None, Some(77)] {
            let sql = favorite_upsert_sql(tmdb_id);
            let update = sql.split(" DO UPDATE SET ").nth(1).unwrap();
            assert_eq!(
                update,
                "work_id = EXCLUDED.work_id, tmdb_id = COALESCE(EXCLUDED.tmdb_id, user_favorites.tmdb_id)"
            );
        }
    }

    #[test]
    fn partial_unique_conflict_clauses_match_migration_indexes() {
        assert!(favorite_conflict_clause(Some(1)).contains("WHERE tmdb_id IS NOT NULL"));
        assert!(favorite_conflict_clause(None)
            .contains("WHERE tmdb_id IS NULL AND work_id IS NOT NULL"));
        assert!(history_conflict_clause(Some(1))
            .contains("season_number, episode_number) WHERE tmdb_id IS NOT NULL"));
        assert!(history_conflict_clause(None).contains(
            "season_number, episode_number) WHERE tmdb_id IS NULL AND work_id IS NOT NULL"
        ));
    }

    #[test]
    fn source_ids_use_episode_scope_for_tv_and_work_scope_for_movies() {
        let tv_target = ItemTarget {
            work_id: 7,
            tmdb_id: Some(77),
            media_type: "tv".into(),
            season_number: 2,
            episode_number: 3,
        };
        let movie_target = ItemTarget {
            media_type: "movie".into(),
            ..tv_target
        };
        assert_eq!(
            played_scope_for_target("src_123", &tv_target),
            PlayedHistoryScope::Episode(2, 3)
        );
        assert_eq!(
            played_scope_for_target("src_123", &movie_target),
            PlayedHistoryScope::Work
        );
        assert!(supports_single_row_progress(
            PlayedHistoryScope::Episode(2, 3),
            "tv"
        ));
        assert!(supports_single_row_progress(
            PlayedHistoryScope::Work,
            "movie"
        ));
        assert!(!supports_single_row_progress(
            PlayedHistoryScope::Work,
            "tv"
        ));
        assert!(!supports_single_row_progress(
            PlayedHistoryScope::Season(1),
            "tv"
        ));
    }

    #[test]
    fn session_removal_requires_matching_authenticated_owner() {
        assert!(session_owned_by(Some("alice"), Some("alice")));
        assert!(!session_owned_by(Some("bob"), Some("alice")));
        assert!(!session_owned_by(None, Some("alice")));
        assert!(!session_owned_by(Some("alice"), None));
    }

    #[test]
    fn session_item_ids_match_by_normalized_work_and_episode() {
        assert!(session_item_matches("ep_7_2_3", "work_7"));
        assert!(session_item_matches("season_7_2", "ep_7_2_3"));
        assert!(session_item_matches("src_91", "src_91"));
        assert!(!session_item_matches("src_91", "src_92"));
        assert!(!session_item_matches("work_8", "ep_7_2_3"));
    }

    #[test]
    fn work_history_uses_work_level_key_and_season_uses_season_key() {
        assert_eq!(history_key("work_7"), Some((7, 0, 1)));
        assert_eq!(history_key("season_7_2"), Some((7, 2, 0)));
        assert_eq!(history_key("ep_7_2_3"), Some((7, 2, 3)));
    }

    #[test]
    fn played_history_scope_preserves_zero_season_and_episode_one() {
        assert_eq!(
            item_history_scope("work_123"),
            Some(PlayedHistoryScope::Work)
        );
        assert_eq!(
            item_history_scope("season_123_0"),
            Some(PlayedHistoryScope::Season(0))
        );
        assert_eq!(
            item_history_scope("ep_123_0_2"),
            Some(PlayedHistoryScope::Episode(0, 2))
        );
        assert_eq!(
            item_history_scope("ep_123_1_1"),
            Some(PlayedHistoryScope::Episode(1, 1))
        );
    }

    #[test]
    fn session_item_matching_does_not_treat_zero_season_as_wildcard() {
        assert!(!session_item_matches("season_7_0", "ep_7_1_2"));
        assert!(session_item_matches("season_7_0", "ep_7_0_2"));
        assert!(!session_item_matches("ep_7_1_1", "ep_7_1_2"));
    }
}

#[cfg(test)]
mod history_database_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "设置 TEST_DATABASE_URL，使用单连接临时表验证事务和播放历史"]
    async fn history_mutations_preserve_scope_progress_and_atomicity() {
        let url = std::env::var("TEST_DATABASE_URL").expect("需要 PostgreSQL 测试连接");
        let db = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql(r#"
            CREATE TEMP TABLE users(id TEXT PRIMARY KEY,username TEXT UNIQUE,password_hash TEXT,is_admin BOOL);
            CREATE TEMP TABLE access_tokens(token TEXT CHECK(token=''),user_id TEXT,device_id TEXT,device_name TEXT,client_name TEXT,last_used_at TIMESTAMPTZ DEFAULT NOW());
            CREATE TEMP TABLE media_works(id BIGINT PRIMARY KEY);
            CREATE TEMP TABLE sync_folders(id BIGINT PRIMARY KEY,work_id BIGINT,is_ignored BOOL DEFAULT FALSE);
            CREATE TEMP TABLE sync_files(folder_id BIGINT,season_number INT,episode_number INT);
            CREATE TEMP TABLE play_histories(
                user_id TEXT,work_id BIGINT,tmdb_id INT,media_type TEXT,season_number INT DEFAULT 0,
                episode_number INT DEFAULT 1,position_ticks BIGINT DEFAULT 0,duration_ticks BIGINT DEFAULT 0,
                is_played BOOL DEFAULT FALSE,last_played_at TIMESTAMPTZ DEFAULT NOW(),
                play_count INT DEFAULT 0,hidden_from_resume BOOL DEFAULT FALSE);
            CREATE UNIQUE INDEX ON play_histories(user_id,tmdb_id,media_type,season_number,episode_number) WHERE tmdb_id IS NOT NULL;
            CREATE UNIQUE INDEX ON play_histories(user_id,work_id,season_number,episode_number) WHERE tmdb_id IS NULL AND work_id IS NOT NULL;
        "#).execute(&db).await.unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let state = AppState::new(
            crate::config::BootstrapFileConfig::default().into_dynamic(),
            db.clone(),
            tx,
        )
        .unwrap();

        let login = super::super::auth::authenticate_by_name(
            State(state.clone()),
            HeaderMap::new(),
            None,
            Json(serde_json::from_value(json!({"Username":"first","Pw":""})).unwrap()),
        )
        .await;
        assert!(login.is_err());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(count, 0, "令牌写入失败必须回滚首次管理员创建");

        sqlx::raw_sql(r#"
            INSERT INTO users VALUES('u','user','',FALSE);
            INSERT INTO media_works VALUES(1);
            INSERT INTO sync_folders VALUES(1,1,FALSE);
            INSERT INTO sync_files VALUES(1,0,2),(1,1,1),(1,1,9);
            INSERT INTO play_histories(user_id,work_id,media_type,season_number,episode_number,position_ticks,duration_ticks,play_count)
            VALUES('u',1,'tv',0,2,42,100,3);
        "#).execute(&db).await.unwrap();
        let media = PlayedHistoryMedia {
            work_id: 1,
            tmdb_id: None,
            media_type: "tv".into(),
        };
        set_resume_hidden_scope(&state, "u", &media, PlayedHistoryScope::Episode(0, 2), true)
            .await
            .unwrap();
        let history: (i64,i32,bool) = sqlx::query_as("SELECT position_ticks,play_count,hidden_from_resume FROM play_histories WHERE season_number=0")
            .fetch_one(&db).await.unwrap();
        assert_eq!(history, (42, 3, true));
        set_resume_hidden_scope(
            &state,
            "u",
            &media,
            PlayedHistoryScope::Episode(0, 2),
            false,
        )
        .await
        .unwrap();
        let visible: bool = sqlx::query_scalar(
            "SELECT NOT hidden_from_resume FROM play_histories WHERE season_number=0",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert!(visible);
        set_resume_hidden_scope(&state, "u", &media, PlayedHistoryScope::Episode(1, 1), true)
            .await
            .unwrap();
        let hidden: bool = sqlx::query_scalar("SELECT hidden_from_resume FROM play_histories WHERE season_number=1 AND episode_number=1").fetch_one(&db).await.unwrap();
        assert!(hidden, "尚无历史的NextUp分集必须能隐藏");
        upsert_play_history(
            &state,
            PlayHistoryUpsert {
                user_id: "u",
                work_id: 1,
                tmdb_id: None,
                media_type: "tv",
                season_number: 1,
                episode_number: 1,
                position_ticks: 50,
                duration_ticks: 100,
                is_played: false,
                overwrite_played: false,
            },
        )
        .await
        .unwrap();
        let visible: (i64,bool) = sqlx::query_as("SELECT position_ticks,hidden_from_resume FROM play_histories WHERE season_number=1 AND episode_number=1").fetch_one(&db).await.unwrap();
        assert_eq!(visible, (50, false));
        let untouched: (i64, bool) = sqlx::query_as(
            "SELECT position_ticks,hidden_from_resume FROM play_histories WHERE season_number=0",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(untouched, (42, false));

        for (work_id, tmdb_id) in [(3, None), (2, Some(77))] {
            sqlx::query("INSERT INTO play_histories(user_id,work_id,tmdb_id,media_type,season_number,episode_number,is_played,position_ticks) VALUES('u',$2,$1,'tv',0,1,TRUE,99),('u',$2,$1,'tv',1,0,TRUE,88)")
                .bind(tmdb_id).bind(work_id).execute(&db).await.unwrap();
            upsert_play_history(
                &state,
                PlayHistoryUpsert {
                    user_id: "u",
                    work_id,
                    tmdb_id,
                    media_type: "tv",
                    season_number: 1,
                    episode_number: 1,
                    position_ticks: 0,
                    duration_ticks: 0,
                    is_played: false,
                    overwrite_played: true,
                },
            )
            .await
            .unwrap();
            let markers: Vec<(i32, i32, bool, i64)> = sqlx::query_as("SELECT season_number,episode_number,is_played,position_ticks FROM play_histories WHERE user_id='u' AND work_id=$2 AND (($1::int4 IS NULL AND tmdb_id IS NULL) OR tmdb_id=$1) AND ((season_number=0 AND episode_number=1) OR (season_number=1 AND episode_number=0))")
                .bind(tmdb_id).bind(work_id).fetch_all(&db).await.unwrap();
            assert_eq!(markers.len(), 2);
            assert!(markers
                .iter()
                .all(|(_, _, played, position)| !played && *position == 0));
        }

        sqlx::raw_sql(r#"
            CREATE FUNCTION pg_temp.reject_history_insert() RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN RAISE EXCEPTION 'test insert failure'; END $$;
            CREATE TRIGGER reject_insert BEFORE INSERT ON play_histories FOR EACH ROW EXECUTE FUNCTION pg_temp.reject_history_insert();
        "#).execute(&db).await.unwrap();
        let result =
            set_play_history_scope(&state, "u", &media, PlayedHistoryScope::Season(1), true).await;
        assert!(result.is_err());
        let rolled_back: (i64,bool,i32) = sqlx::query_as("SELECT position_ticks,is_played,play_count FROM play_histories WHERE season_number=1 AND episode_number=1").fetch_one(&db).await.unwrap();
        assert_eq!(
            rolled_back,
            (50, false, 0),
            "补齐行失败时必须回滚此前UPDATE"
        );
        db.close().await;
    }
}
