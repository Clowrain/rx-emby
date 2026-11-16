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

async fn ensure_media_accessible(
    state: &AppState,
    user_id: &str,
    work_id: i64,
) -> Result<(), (StatusCode, String)> {
    super::items::ensure_media_accessible(state, user_id, work_id)
        .await
        .map(|_| ())
}

#[derive(Deserialize, Default)]
pub(super) struct MutationQuery {
    #[serde(alias = "api_key", alias = "ApiKey")]
    api_key: Option<String>,
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
    ensure_media_accessible(state, &user_id, target.work_id).await?;
    let media = PlayedHistoryMedia {
        work_id: target.work_id,
        tmdb_id: target.tmdb_id,
        media_type: target.media_type.clone(),
    };
    let scope = played_scope_for_target(item_id, &target);
    set_play_history_scope(state, &user_id, &media, scope, played)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let history = query_play_history_summary(state, &user_id, &media, item_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let favorite: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_favorites WHERE user_id = $1 AND (work_id = $2 OR ($3::int4 IS NOT NULL AND tmdb_id = $3 AND media_type = $4)))",
    ).bind(&user_id).bind(target.work_id).bind(target.tmdb_id).bind(&target.media_type).fetch_one(&state.db).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({
        "PlaybackPositionTicks": history.0,
        "PlayCount": if history.1 { 1 } else { 0 },
        "IsFavorite": favorite,
        "Played": history.1
    })))
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
    ensure_media_accessible(state, &user_id, target.work_id).await?;
    let wid = target.work_id;
    let tmdb_id = target.tmdb_id;
    let mtype = target.media_type;
    {
        if is_fav {
            let updated = sqlx::query(
                r#"
                UPDATE user_favorites
                SET work_id = $2, tmdb_id = COALESCE($3, tmdb_id)
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
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .rows_affected();

            if updated == 0 {
                sqlx::query(
                    "INSERT INTO user_favorites (user_id, work_id, tmdb_id, media_type) VALUES ($1, $2, $3, $4)",
                )
                .bind(&user_id)
                .bind(wid)
                .bind(tmdb_id)
                .bind(&mtype)
                .execute(&state.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            }
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
    let history = query_play_history_summary(state, &user_id, &media, item_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let actual_favorite: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_favorites WHERE user_id = $1 AND (work_id = $2 OR ($3::int4 IS NOT NULL AND tmdb_id = $3 AND media_type = $4)))",
    ).bind(&user_id).bind(wid).bind(tmdb_id).bind(&mtype).fetch_one(&state.db).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({
        "PlaybackPositionTicks": history.0,
        "PlayCount": i32::from(history.1),
        "IsFavorite": actual_favorite,
        "Played": history.1
    })))
}

#[derive(Deserialize, Default)]
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
    #[serde(rename = "api_key", alias = "ApiKey", alias = "X-Emby-Token")]
    api_key: Option<String>,
}

pub(super) async fn session_playing(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q_report): Query<SessionReportPayload>,
    payload: Option<Json<SessionReportPayload>>,
) -> StatusCode {
    state.prune_stale_sessions();
    let body = payload.map(|Json(b)| b).unwrap_or(q_report);
    let Some(ref raw_item_id) = body.item_id else {
        return StatusCode::NO_CONTENT;
    };

    let Ok(user_id) = resolve_valid_user_id_with_query(
        &state,
        body.user_id.as_deref(),
        &headers,
        body.api_key.as_deref(),
    )
    .await
    else {
        return StatusCode::NO_CONTENT;
    };
    let target = match resolve_item_target(&state, raw_item_id).await {
        Ok(target) => target,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
    };
    if let Some(target) = &target {
        if let Err((status, _)) = ensure_media_accessible(&state, &user_id, target.work_id).await {
            return status;
        }
    }

    let ep_id = match normalized_session_id(&state, raw_item_id).await {
        Ok(id) => id,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
    };
    let sid = body
        .play_session_id
        .unwrap_or_else(|| format!("sess_{}", raw_item_id));
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("EmbyClient")
        .to_string();

    if state
        .sessions
        .get(&sid)
        .is_some_and(|s| !session_owned_by(Some(&s.user_id), Some(&user_id)))
    {
        return StatusCode::NO_CONTENT;
    }
    // 同一用户的同一客户端重新开播时，移除旧会话避免残留僵尸会话
    state
        .sessions
        .retain(|k, v| k == &sid || !(v.user_id == user_id && v.client_name == ua));

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
        let target = match resolve_item_target(&state, raw_item_id).await {
            Ok(target) => target,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
        };
        if let Some(ItemTarget {
            work_id: mid,
            tmdb_id,
            media_type: mtype,
            season_number: snum,
            episode_number: epnum,
        }) = target
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

    StatusCode::NO_CONTENT
}

pub(super) async fn session_progress(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q_report): Query<SessionReportPayload>,
    payload: Option<Json<SessionReportPayload>>,
) -> StatusCode {
    state.prune_stale_sessions();
    let body = payload.map(|Json(b)| b).unwrap_or(q_report);
    let Some(ref raw_item_id) = body.item_id else {
        return StatusCode::NO_CONTENT;
    };

    let Ok(user_id) = resolve_valid_user_id_with_query(
        &state,
        body.user_id.as_deref(),
        &headers,
        body.api_key.as_deref(),
    )
    .await
    else {
        return StatusCode::NO_CONTENT;
    };
    let target = match resolve_item_target(&state, raw_item_id).await {
        Ok(target) => target,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
    };
    if let Some(target) = &target {
        if let Err((status, _)) = ensure_media_accessible(&state, &user_id, target.work_id).await {
            return status;
        }
    }

    let ep_id = match normalized_session_id(&state, raw_item_id).await {
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
        return StatusCode::NO_CONTENT;
    }
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
        let target = match resolve_item_target(&state, raw_item_id).await {
            Ok(target) => target,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
        };
        if let Some(ItemTarget {
            work_id: mid,
            tmdb_id,
            media_type: mtype,
            season_number: snum,
            episode_number: epnum,
        }) = target
        {
            let saved_pos = if auto_completed {
                0
            } else {
                body.position_ticks
            };
            if upsert_play_history(
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
            )
            .await
            .is_err()
            {
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
                    device_name: client_ua.clone(),
                    client_name: client_ua.clone(),
                    current_episode_id: ep_id,
                    position_ticks: body.position_ticks,
                    next_ep_prewarmed: false,
                    last_activity: std::time::Instant::now(),
                });
        if entry.user_id != user_id {
            return StatusCode::NO_CONTENT;
        }
        entry.position_ticks = body.position_ticks;
        entry.last_activity = std::time::Instant::now();
        if reached_85_percent && !entry.next_ep_prewarmed {
            entry.next_ep_prewarmed = true;
            should_prewarm_next = true;
        }
    }

    if should_prewarm_next {
        let state_clone = state.clone();
        let raw_item_clone = raw_item_id.clone();
        tokio::spawn(async move {
            if let Ok(Some(target)) = resolve_item_target(&state_clone, &raw_item_clone).await {
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
                    .bind(epnum + 1)
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
                            epnum + 1,
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

    StatusCode::NO_CONTENT
}

pub(super) async fn session_stopped(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q_report): Query<SessionReportPayload>,
    payload: Option<Json<SessionReportPayload>>,
) -> StatusCode {
    state.prune_stale_sessions();
    let body = payload.map(|Json(b)| b).unwrap_or(q_report);
    let resolved_uid = resolve_valid_user_id_with_query(
        &state,
        body.user_id.as_deref(),
        &headers,
        body.api_key.as_deref(),
    )
    .await
    .ok();
    let Some(ref authenticated_uid) = resolved_uid else {
        return StatusCode::NO_CONTENT;
    };
    if let Some(ref sid) = body.play_session_id {
        if state.sessions.get(sid).is_some_and(|session| {
            !session_owned_by(Some(&session.user_id), Some(authenticated_uid))
        }) {
            return StatusCode::NO_CONTENT;
        }
    }
    if let Some(ref raw_item_id) = body.item_id {
        match resolve_item_target(&state, raw_item_id).await {
            Ok(Some(target)) => {
                if let Err((status, _)) =
                    ensure_media_accessible(&state, authenticated_uid, target.work_id).await
                {
                    return status;
                }
            }
            Ok(None) => {}
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
    if let Some(ref raw_item_id) = body.item_id {
        if body.position_ticks > 0 {
            if let Some(ref user_id) = resolved_uid {
                let target = match resolve_item_target(&state, raw_item_id).await {
                    Ok(target) => target,
                    Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
                };
                if let Some(ItemTarget {
                    work_id: mid,
                    tmdb_id,
                    media_type: mtype,
                    season_number: snum,
                    episode_number: epnum,
                }) = target
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
    }
    let Some(ref uid) = resolved_uid else {
        return StatusCode::NO_CONTENT;
    };
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
        if let Some(ref uid) = resolved_uid {
            let matching_ids = match matching_session_ids(&state, raw_item_id).await {
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
) -> Result<HashSet<i64>, sqlx::Error> {
    let mut ids = HashSet::new();
    let Some(target) = resolve_item_target(state, item_id).await? else {
        return Ok(ids);
    };
    let wid = target.work_id;
    ids.insert(-wid);
    let scope = item_history_scope(item_id).unwrap_or(PlayedHistoryScope::Work);
    let (scope_sql, season, episode) = match scope {
        PlayedHistoryScope::Work => ("TRUE", None, None),
        PlayedHistoryScope::Season(season) => ("sf.season_number = $2", Some(season), None),
        PlayedHistoryScope::Episode(season, episode) => (
            "sf.season_number = $2 AND sf.episode_number = $3",
            Some(season),
            Some(episode),
        ),
    };
    let sql = format!("SELECT sf.id FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE f.work_id = $1 AND ({scope_sql})");
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
) -> Result<Option<i64>, sqlx::Error> {
    let Some(target) = resolve_item_target(state, item_id).await? else {
        return Ok(parse_work_id(item_id).map(|wid| -wid));
    };
    let wid = target.work_id;
    let scope = item_history_scope(item_id).unwrap_or(PlayedHistoryScope::Work);
    let (scope_sql, season, episode) = match scope {
        PlayedHistoryScope::Work => ("TRUE", None, None),
        PlayedHistoryScope::Season(season) => ("sf.season_number = $2", Some(season), None),
        PlayedHistoryScope::Episode(season, episode) => (
            "sf.season_number = $2 AND sf.episode_number = $3",
            Some(season),
            Some(episode),
        ),
    };
    let sql = format!("SELECT sf.id FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE f.work_id = $1 AND f.is_ignored = FALSE AND ({scope_sql}) ORDER BY COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) DESC, sf.id ASC LIMIT 1");
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
    let Some((wid, snum, epnum)): Option<(i64, i32, i32)> = sqlx::query_as(
        "SELECT f.work_id, sf.season_number, sf.episode_number FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE sf.id = $1 AND f.work_id IS NOT NULL AND f.is_ignored = FALSE",
    ).bind(sid).fetch_optional(&state.db).await? else { return Ok(None) };
    let Some((tmdb_id, mtype)): Option<(Option<i32>, String)> =
        sqlx::query_as("SELECT tmdb_id, media_type FROM media_works WHERE id = $1")
            .bind(wid)
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

async fn query_play_history_summary(
    state: &AppState,
    user_id: &str,
    media: &PlayedHistoryMedia,
    item_id: &str,
) -> Result<(i64, bool), sqlx::Error> {
    let scope = item_history_scope(item_id).unwrap_or(PlayedHistoryScope::Work);
    let scope_sql = match scope {
        PlayedHistoryScope::Work if media.media_type == "tv" => "TRUE",
        PlayedHistoryScope::Work => "season_number = 0 AND episode_number = 1",
        PlayedHistoryScope::Season(_) => "season_number = $5",
        PlayedHistoryScope::Episode(_, _) => "season_number = $5 AND episode_number = $6",
    };
    let sql = format!("SELECT COALESCE(MAX(position_ticks), 0), COALESCE(BOOL_AND(is_played), FALSE) FROM play_histories WHERE user_id = $1 AND (work_id = $2 OR ($3::int4 IS NOT NULL AND tmdb_id = $3 AND media_type = $4)) AND ({scope_sql})");
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
    sqlx::query(
        "UPDATE play_histories SET is_played = $5, position_ticks = CASE WHEN $5 THEN 0 ELSE position_ticks END, last_played_at = NOW() WHERE user_id = $1 AND (work_id = $2 OR ($3::int4 IS NOT NULL AND tmdb_id = $3 AND media_type = $4)) AND ($7 OR season_number = $6)",
    ).bind(user_id).bind(media.work_id).bind(media.tmdb_id).bind(&media.media_type).bind(played).bind(season_filter).bind(matches!(scope, PlayedHistoryScope::Work)).execute(&state.db).await?;

    let pair_scope = match scope {
        PlayedHistoryScope::Work if media.media_type == "tv" => "SELECT 0::int4 AS season_number, 1::int4 AS episode_number UNION SELECT DISTINCT sf.season_number, sf.episode_number FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE f.work_id = $2 AND f.is_ignored = FALSE",
        PlayedHistoryScope::Work => "SELECT 0::int4 AS season_number, 1::int4 AS episode_number",
        PlayedHistoryScope::Season(_) => "SELECT $6::int4 AS season_number, 0::int4 AS episode_number UNION SELECT DISTINCT sf.season_number, sf.episode_number FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE f.work_id = $2 AND f.is_ignored = FALSE AND sf.season_number = $6",
        PlayedHistoryScope::Episode(_, _) => unreachable!(),
    };
    let insert_sql = format!(
        "INSERT INTO play_histories (user_id, work_id, tmdb_id, media_type, season_number, episode_number, position_ticks, duration_ticks, is_played, last_played_at) SELECT $1, $2, $3, $4, pairs.season_number, pairs.episode_number, 0, 0, $5, NOW() FROM ({pair_scope}) pairs WHERE NOT EXISTS (SELECT 1 FROM play_histories ph WHERE ph.user_id = $1 AND ph.work_id = $2 AND ph.season_number = pairs.season_number AND ph.episode_number = pairs.episode_number) ON CONFLICT DO NOTHING"
    );
    let mut insert_query = sqlx::query(&insert_sql)
        .bind(user_id)
        .bind(media.work_id)
        .bind(media.tmdb_id)
        .bind(&media.media_type)
        .bind(played);
    if matches!(scope, PlayedHistoryScope::Season(_)) {
        insert_query = insert_query.bind(season_filter);
    }
    insert_query.execute(&state.db).await?;
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
    let updated = sqlx::query(
        r#"
        UPDATE play_histories
        SET work_id = $2,
            tmdb_id = COALESCE($3, tmdb_id),
            position_ticks = $7,
            duration_ticks = CASE WHEN $8 > 0 THEN $8 ELSE duration_ticks END,
            is_played = CASE WHEN $10 THEN $9 ELSE ($9 OR is_played) END,
            last_played_at = NOW()
        WHERE user_id = $1
          AND season_number = $5
          AND episode_number = $6
          AND (
              ($3::int4 IS NOT NULL AND tmdb_id = $3 AND media_type = $4)
              OR work_id = $2
          )
        "#,
    )
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
    .execute(&state.db)
    .await?
    .rows_affected();

    if updated == 0 {
        let conflict_clause = if tmdb_id.is_some() {
            "ON CONFLICT (user_id, tmdb_id, media_type, season_number, episode_number) WHERE tmdb_id IS NOT NULL DO UPDATE"
        } else {
            "ON CONFLICT (user_id, work_id, season_number, episode_number) WHERE tmdb_id IS NULL AND work_id IS NOT NULL DO UPDATE"
        };
        let insert_sql = format!(
            "INSERT INTO play_histories (user_id, work_id, tmdb_id, media_type, season_number, episode_number, position_ticks, duration_ticks, is_played, last_played_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,NOW()) {conflict_clause} SET work_id = EXCLUDED.work_id, tmdb_id = COALESCE(EXCLUDED.tmdb_id, play_histories.tmdb_id), position_ticks = EXCLUDED.position_ticks, duration_ticks = CASE WHEN EXCLUDED.duration_ticks > 0 THEN EXCLUDED.duration_ticks ELSE play_histories.duration_ticks END, is_played = CASE WHEN $10 THEN EXCLUDED.is_played ELSE (EXCLUDED.is_played OR play_histories.is_played) END, last_played_at = NOW()"
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
            .execute(&state.db)
            .await?;
    }

    Ok(())
}

pub(super) async fn load_user_favorite_keys(
    state: &AppState,
    user_id: &str,
) -> Result<(HashSet<i64>, HashSet<(i32, String)>), (StatusCode, String)> {
    let rows: Vec<(Option<i64>, Option<i32>, String)> = sqlx::query_as(
        "SELECT work_id, tmdb_id, media_type FROM user_favorites WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await
    .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    let (mut wids, mut tmdbs) = (HashSet::new(), HashSet::new());
    for (wid, tid, mtype) in rows {
        if let Some(work_id) = wid {
            wids.insert(work_id);
        }
        if let Some(tmdb_id) = tid {
            tmdbs.insert((tmdb_id, mtype));
        }
    }
    Ok((wids, tmdbs))
}

pub(super) fn is_media_favorited(
    fav_keys: &(HashSet<i64>, HashSet<(i32, String)>),
    work_id: i64,
    tmdb_id: Option<i32>,
    media_type: &str,
) -> bool {
    if fav_keys.0.contains(&work_id) {
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
pub(super) async fn load_media_episode_histories(
    state: &AppState,
    user_id: &str,
    work_id: i64,
    tmdb_id: Option<i32>,
    media_type: &str,
) -> Result<HashMap<(i32, i32), (i64, i64, bool)>, (StatusCode, String)> {
    let rows: Vec<(i32, i32, i64, i64, bool)> = sqlx::query_as(
        r#"
        SELECT season_number, episode_number, position_ticks, duration_ticks, is_played
        FROM play_histories
        WHERE user_id = $1
          AND (
              ($3::int4 IS NOT NULL AND tmdb_id = $3 AND media_type = $4)
              OR work_id = $2
          )
        "#,
    )
    .bind(user_id)
    .bind(work_id)
    .bind(tmdb_id)
    .bind(media_type)
    .fetch_all(&state.db)
    .await
    .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|(s, e, pos, dur, played)| ((s, e), (pos, dur, played)))
        .collect())
}

#[cfg(test)]
mod session_fix_tests {
    use super::*;

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
            ..tv_target.clone()
        };
        assert_eq!(
            played_scope_for_target("src_123", &tv_target),
            PlayedHistoryScope::Episode(2, 3)
        );
        assert_eq!(
            played_scope_for_target("src_123", &movie_target),
            PlayedHistoryScope::Work
        );
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
