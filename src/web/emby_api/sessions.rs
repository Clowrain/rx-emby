use super::auth::resolve_valid_user_id;
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

/// 手动标记已看 (`POST /Users/:user_id/PlayedItems/:item_id`)
pub(super) async fn mark_item_played(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((path_user_id, item_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    set_item_played_state(&state, &path_user_id, &headers, &item_id, true).await
}

/// 取消已看标记 (`DELETE /Users/:user_id/PlayedItems/:item_id`)
pub(super) async fn unmark_item_played(
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
            state, &user_id, mid, tmdb_id, &mtype, snum, epnum, 0, 0, played, true,
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
pub(super) async fn mark_item_favorite(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((path_user_id, item_id)): Path<(String, String)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    set_item_favorite_state(&state, &path_user_id, &headers, &item_id, true).await
}

/// 取消收藏 (`DELETE /Users/:user_id/FavoriteItems/:item_id`)
pub(super) async fn unmark_item_favorite(
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
    if let Some((wid, tmdb_id, mtype, _, _)) = resolve_item_target(state, item_id).await {
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
            .map(|r| r.rows_affected())
            .unwrap_or(0);

            if updated == 0 {
                let _ = sqlx::query(
                    "INSERT INTO user_favorites (user_id, work_id, tmdb_id, media_type) VALUES ($1, $2, $3, $4)",
                )
                .bind(&user_id)
                .bind(wid)
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
                  AND (($3::int4 IS NOT NULL AND tmdb_id = $3 AND media_type = $4) OR work_id = $2)
                "#,
            )
            .bind(&user_id)
            .bind(wid)
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

    let Ok(user_id) = resolve_valid_user_id(&state, body.user_id.as_deref(), &headers).await else {
        return StatusCode::NO_CONTENT;
    };

    let ep_id = raw_item_id
        .strip_prefix("src_")
        .and_then(|s| s.parse::<i64>().ok())
        .or_else(|| {
            raw_item_id
                .strip_prefix("work_")
                .or_else(|| raw_item_id.strip_prefix("media_"))
                .and_then(|s| s.parse::<i64>().ok())
                .map(|wid| -wid)
        });
    let sid = body
        .play_session_id
        .unwrap_or_else(|| format!("sess_{}", raw_item_id));
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("EmbyClient")
        .to_string();

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
                false,
            )
            .await;
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

    let Ok(user_id) = resolve_valid_user_id(&state, body.user_id.as_deref(), &headers).await else {
        return StatusCode::NO_CONTENT;
    };

    let ep_id = raw_item_id
        .strip_prefix("src_")
        .and_then(|s| s.parse::<i64>().ok())
        .or_else(|| {
            raw_item_id
                .strip_prefix("work_")
                .or_else(|| raw_item_id.strip_prefix("media_"))
                .and_then(|s| s.parse::<i64>().ok())
                .map(|wid| -wid)
        });
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
                false,
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
        let state_clone = state.clone();
        let raw_item_clone = raw_item_id.clone();
        tokio::spawn(async move {
            if let Some((wid, _, mtype, snum, epnum)) =
                resolve_item_target(&state_clone, &raw_item_clone).await
            {
                if mtype == "tv" {
                    let next_ep: Option<(i64, String, i64)> = sqlx::query_as(
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
                    .await
                    .unwrap_or(None);

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
    let resolved_uid = resolve_valid_user_id(&state, body.user_id.as_deref(), &headers).await.ok();
    if let Some(ref raw_item_id) = body.item_id {
        if body.position_ticks > 0 {
            if let Some(ref user_id) = resolved_uid {
                if let Some((mid, tmdb_id, mtype, snum, epnum)) =
                    resolve_item_target(&state, raw_item_id).await
                {
                    let auto_completed = body.run_time_ticks > 0
                        && (body.position_ticks as f64 / body.run_time_ticks as f64) >= 0.90;
                    let saved_pos = if auto_completed { 0 } else { body.position_ticks };
                    let _ = upsert_play_history(
                        &state,
                        user_id,
                        mid,
                        tmdb_id,
                        &mtype,
                        snum,
                        epnum,
                        saved_pos,
                        body.run_time_ticks,
                        auto_completed,
                        false,
                    )
                    .await;
                }
            }
        }
    }
    if let Some(ref sid) = body.play_session_id {
        state.sessions.remove(sid);
    }
    if let Some(ref raw_item_id) = body.item_id {
        let fallback_sid = format!("sess_{}", raw_item_id);
        state.sessions.remove(&fallback_sid);
        if let Some(ep_id) = raw_item_id
            .strip_prefix("ep_")
            .or_else(|| raw_item_id.strip_prefix("src_"))
            .and_then(|s| s.parse::<i64>().ok())
        {
            if let Some(ref uid) = resolved_uid {
                state
                    .sessions
                    .retain(|_, v| !(v.user_id == *uid && v.current_episode_id == Some(ep_id)));
            }
        }
    }
    StatusCode::NO_CONTENT
}


/// 将 `work_123` / `ep_123_1_2` / `src_456` 解析为 `(work_id, tmdb_id, media_type, season_number, episode_number)`
async fn resolve_item_target(
    state: &AppState,
    item_id: &str,
) -> Option<(i64, Option<i32>, String, i32, i32)> {
    if let Some(raw_wid) = item_id
        .strip_prefix("work_")
        .or_else(|| item_id.strip_prefix("media_"))
    {
        let wid = raw_wid.parse::<i64>().ok()?;
        let (tmdb_id, mtype): (Option<i32>, String) =
            sqlx::query_as("SELECT tmdb_id, media_type FROM media_works WHERE id = $1")
                .bind(wid)
                .fetch_optional(&state.db)
                .await
                .ok()??;
        return Some((wid, tmdb_id, mtype, 0, 1));
    }

    if let Some(rest) = item_id.strip_prefix("ep_") {
        let mut parts = rest.rsplitn(3, '_');
        let epnum = parts.next()?.parse::<i32>().ok()?;
        let snum = parts.next()?.parse::<i32>().ok()?;
        let wid = parts.next()?.parse::<i64>().ok()?;
        let (tmdb_id, mtype): (Option<i32>, String) =
            sqlx::query_as("SELECT tmdb_id, media_type FROM media_works WHERE id = $1")
                .bind(wid)
                .fetch_optional(&state.db)
                .await
                .ok()??;
        return Some((wid, tmdb_id, mtype, snum, epnum));
    }

    let sid = item_id
        .strip_prefix("src_")
        .unwrap_or(item_id)
        .parse::<i64>()
        .ok()?;

    let (wid, snum, epnum): (i64, i32, i32) = sqlx::query_as(
        "SELECT f.work_id, sf.season_number, sf.episode_number FROM sync_files sf JOIN sync_folders f ON f.id = sf.folder_id WHERE sf.id = $1 AND f.work_id IS NOT NULL",
    )
    .bind(sid)
    .fetch_optional(&state.db)
    .await
    .ok()??;

    let (tmdb_id, mtype): (Option<i32>, String) =
        sqlx::query_as("SELECT tmdb_id, media_type FROM media_works WHERE id = $1")
            .bind(wid)
            .fetch_optional(&state.db)
            .await
            .ok()??;

    Some((wid, tmdb_id, mtype, snum, epnum))
}

/// 写入或更新播放进度（同时兼容有/无 tmdb_id 的条目）
#[allow(clippy::too_many_arguments)]
async fn upsert_play_history(
    state: &AppState,
    user_id: &str,
    work_id: i64,
    tmdb_id: Option<i32>,
    media_type: &str,
    season_number: i32,
    episode_number: i32,
    position_ticks: i64,
    duration_ticks: i64,
    is_played: bool,
    overwrite_played: bool,
) -> Result<(), sqlx::Error> {
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
        sqlx::query(
            r#"
            INSERT INTO play_histories (
                user_id, work_id, tmdb_id, media_type,
                season_number, episode_number, position_ticks, duration_ticks, is_played, last_played_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
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
        .execute(&state.db)
        .await?;
    }

    Ok(())
}

pub(super) async fn load_user_favorite_keys(state: &AppState, user_id: &str) -> (HashSet<i64>, HashSet<(i32, String)>) {
    let rows: Vec<(Option<i64>, Option<i32>, String)> = sqlx::query_as(
        "SELECT work_id, tmdb_id, media_type FROM user_favorites WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut wids = HashSet::new();
    let mut tmdbs = HashSet::new();
    for (wid, tid, mtype) in rows {
        if let Some(w) = wid {
            wids.insert(w);
        }
        if let Some(t) = tid {
            tmdbs.insert((t, mtype));
        }
    }
    (wids, tmdbs)
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
) -> HashMap<(i32, i32), (i64, i64, bool)> {
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
    .unwrap_or_default();

    rows.into_iter()
        .map(|(s, e, pos, dur, played)| ((s, e), (pos, dur, played)))
        .collect()
}

