use super::{
    auth::resolve_auth_context,
    sessions::{self, SessionReportPayload},
};
use crate::state::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{HeaderMap, HeaderValue, StatusCode},
    response::Response,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    hash::{Hash, Hasher},
    sync::Arc,
    time::Duration,
};

#[derive(Deserialize, Default)]
pub(super) struct WsQuery {
    #[serde(
        rename = "api_key",
        alias = "ApiKey",
        alias = "X-Emby-Token",
        alias = "x-emby-token"
    )]
    api_key: Option<String>,
    #[serde(rename = "deviceId", alias = "DeviceId")]
    device_id: Option<String>,
}

fn validated_device_id(
    requested: Option<&str>,
    registered: Option<&str>,
    token: &str,
) -> Result<String, (StatusCode, String)> {
    let valid = |id: &str| {
        !id.is_empty()
            && id.chars().count() <= 128
            && !id
                .chars()
                .any(|c| c.is_control() || matches!(c, ',' | '"' | '\\'))
    };
    if requested.is_some_and(|id| !valid(id)) {
        return Err((StatusCode::BAD_REQUEST, "设备标识无效".into()));
    }
    let registered = registered.filter(|id| valid(id));
    let registered_is_anonymous = registered.is_some_and(|id| id.starts_with("anonymous:"));
    if matches!((requested, registered), (Some(a), Some(b)) if a != b && !registered_is_anonymous) {
        return Err((StatusCode::FORBIDDEN, "设备标识与登录令牌不匹配".into()));
    }
    Ok(registered
        .filter(|_| !registered_is_anonymous)
        .or(requested)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            token.hash(&mut hasher);
            format!("token-device:{:016x}", hasher.finish())
        }))
}

fn websocket_authorization(device_id: &str) -> String {
    format!(
        "Emby DeviceId=\"{}\", Device=\"EmbyClient\", Client=\"EmbyClient\"",
        device_id
    )
}

fn sessions_start_payload(state: &AppState, is_admin: bool, user_id: &str) -> Value {
    if is_admin {
        sessions::active_sessions(state, None)
    } else {
        sessions::active_sessions(state, Some(user_id))
    }
}

fn authenticated_report(
    mut data: Value,
    user_id: &str,
    token: &str,
    is_admin: bool,
) -> Result<SessionReportPayload, serde_json::Error> {
    // 令牌始终来自握手；只有管理员可在报告中指定目标用户。
    if let Some(fields) = data.as_object_mut() {
        let requested_user = ["UserId", "userId"]
            .iter()
            .find_map(|key| fields.get(*key).and_then(Value::as_str).map(str::to_owned));
        for key in [
            "UserId",
            "userId",
            "api_key",
            "ApiKey",
            "X-Emby-Token",
            "x-emby-token",
        ] {
            fields.remove(key);
        }
        fields.insert(
            "UserId".into(),
            json!(if is_admin {
                requested_user.as_deref().unwrap_or(user_id)
            } else {
                user_id
            }),
        );
        fields.insert("api_key".into(), json!(token));
    }
    serde_json::from_value(data)
}

/// `GET /embywebsocket` 与 `GET /socket`。
pub(super) async fn emby_websocket(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    mut headers: HeaderMap,
    Query(q): Query<WsQuery>,
) -> Result<Response, (StatusCode, String)> {
    let context = resolve_auth_context(&state, &headers, q.api_key.as_deref()).await?;
    let identity: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT device_id, device_name, client_name FROM access_tokens WHERE token = $1 AND user_id = $2")
        .bind(&context.token).bind(&context.user_id).fetch_optional(&state.db).await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let (registered_device, device_name, client_name) =
        identity.ok_or((StatusCode::UNAUTHORIZED, "访问令牌已注销".into()))?;
    let device_id = validated_device_id(
        q.device_id.as_deref(),
        registered_device.as_deref(),
        &context.token,
    )?;
    if registered_device
        .as_deref()
        .is_none_or(|id| id.starts_with("anonymous:"))
    {
        sqlx::query("UPDATE access_tokens SET device_id = $1 WHERE token = $2 AND (device_id IS NULL OR device_id LIKE 'anonymous:%')")
            .bind(&device_id)
            .bind(&context.token)
            .execute(&state.db)
            .await
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    }
    headers.remove("authorization");
    headers.remove("x-emby-authorization");
    headers.remove("x-mediabrowser-token");
    headers.remove("x-emby-token");
    // Header values must remain ASCII; preserve the resolved UTF-8 identity separately.
    let authorization = websocket_authorization(&device_id);
    headers.insert(
        "x-emby-authorization",
        HeaderValue::from_str(&authorization)
            .map_err(|_| (StatusCode::BAD_REQUEST, "设备信息无效".into()))?,
    );
    let identity = (
        device_id,
        device_name.unwrap_or(context.device_name),
        client_name.unwrap_or(context.client_name),
    );
    Ok(ws.on_upgrade(move |socket| {
        handle_ws_connection(
            socket,
            state,
            context.user_id,
            context.token,
            context.is_admin,
            identity,
            headers,
        )
    }))
}

struct WsPlaybackContext<'a> {
    user_id: &'a str,
    token: &'a str,
    is_admin: bool,
    identity: &'a (String, String, String),
}

async fn dispatch_playback_report(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    kind: &str,
    data: Value,
    context: WsPlaybackContext<'_>,
) -> Option<StatusCode> {
    let WsPlaybackContext {
        user_id,
        token,
        is_admin,
        identity,
    } = context;
    let report = authenticated_report(data.clone(), user_id, token, is_admin).ok()?;
    let state = State(state.clone());
    let headers = headers.clone();
    let query = Query(SessionReportPayload::default());
    let body = Some(Json(report));
    let identity = Some(identity.clone());
    let status = match kind {
        "ReportPlaybackStart" => {
            sessions::session_playing_with_identity(
                state.clone(),
                headers.clone(),
                query,
                body,
                identity,
            )
            .await
        }
        "ReportPlaybackProgress" => {
            sessions::session_progress_with_identity(
                state.clone(),
                headers.clone(),
                query,
                body,
                identity,
            )
            .await
        }
        "ReportPlaybackStopped" => {
            sessions::session_stopped_with_identity(
                state.clone(),
                headers.clone(),
                query,
                body,
                identity,
            )
            .await
        }
        _ => return None,
    };
    Some(status)
}

async fn process_reports_in_order<F>(
    mut reports: tokio::sync::mpsc::Receiver<(String, Value)>,
    mut process: F,
) where
    F: FnMut(String, Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
{
    while let Some((kind, data)) = reports.recv().await {
        process(kind, data).await;
    }
}

async fn handle_ws_connection(
    mut socket: WebSocket,
    state: Arc<AppState>,
    user_id: String,
    token: String,
    is_admin: bool,
    identity: (String, String, String),
    headers: HeaderMap,
) {
    let (connection_id, mut outbound, revoked) =
        state
            .session_hub
            .register_with_role(token.clone(), user_id.clone(), is_admin);
    let (report_tx, report_rx) = tokio::sync::mpsc::channel::<(String, Value)>(64);
    let worker_state = state.clone();
    let worker_headers = headers.clone();
    let worker_user_id = user_id.clone();
    let worker_token = token.clone();
    let worker_identity = identity.clone();
    let mut report_worker = tokio::spawn(process_reports_in_order(report_rx, move |kind, data| {
        let state = worker_state.clone();
        let headers = worker_headers.clone();
        let user_id = worker_user_id.clone();
        let token = worker_token.clone();
        let identity = worker_identity.clone();
        Box::pin(async move {
            if let Some(status) = dispatch_playback_report(
                &state,
                &headers,
                &kind,
                data,
                WsPlaybackContext {
                    user_id: &user_id,
                    token: &token,
                    is_admin,
                    identity: &identity,
                },
            )
            .await
            {
                if status.is_server_error() {
                    tracing::warn!("WebSocket 播放上报失败: {}", status);
                }
            }
        })
    }));
    let connection = async {
        if socket
            .send(Message::Text(
                json!({"MessageType":"ForceKeepAlive","Data":60}).to_string(),
            ))
            .await
            .is_err()
        {
            return;
        }
        let mut sessions_subscribed = false;
        loop {
            tokio::select! {
                outgoing = outbound.recv() => {
                    let Some(outgoing) = outgoing else { break; };
                    if !sessions_subscribed && outgoing.kind == crate::state::HubEventKind::Sessions { continue; }
                    if socket.send(Message::Text(outgoing.message)).await.is_err() { break; }
                }
                incoming = socket.recv() => {
                    let Some(Ok(message)) = incoming else { break; };
                    match message {
                        Message::Text(text) => {
                            let Ok(message) = serde_json::from_str::<Value>(&text) else { continue; };
                            let kind = message.get("MessageType").and_then(Value::as_str).unwrap_or("");
                            match kind {
                                "KeepAlive" => {
                                    if socket.send(Message::Text(json!({"MessageType":"KeepAlive"}).to_string())).await.is_err() { break; }
                                }
                                "SessionsStart" => {
                                    sessions_subscribed = true;
                                    let sessions = sessions_start_payload(&state, is_admin, &user_id);
                                    if socket.send(Message::Text(json!({"MessageType":"Sessions", "Data":sessions}).to_string())).await.is_err() { break; }
                                }
                                "SessionsStop" => sessions_subscribed = false,
                                "ReportPlaybackStart" | "ReportPlaybackProgress" | "ReportPlaybackStopped" => {
                                    let Some(data) = message.get("Data") else { continue; };
                                    if report_tx.send((kind.to_owned(), data.clone())).await.is_err() {
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                        Message::Ping(bytes) => { if socket.send(Message::Pong(bytes)).await.is_err() { break; } }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
            }
        }
    };
    tokio::select! {
        biased;
        _ = state.shutdown_token.cancelled() => {},
        _ = revoked.cancelled() => {},
        _ = connection => {},
    }
    drop(report_tx);
    if tokio::time::timeout(Duration::from_secs(1), &mut report_worker)
        .await
        .is_err()
    {
        report_worker.abort();
    }
    state.session_hub.unregister(&connection_id);
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.send(Message::Close(None))).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn playback_progress_is_processed_before_a_later_stop_frame() {
        let (tx, rx) = tokio::sync::mpsc::channel(2);
        tx.send((
            "ReportPlaybackProgress".into(),
            json!({"PositionTicks": 80}),
        ))
        .await
        .unwrap();
        tx.send(("ReportPlaybackStopped".into(), json!({"PositionTicks": 95})))
            .await
            .unwrap();
        drop(tx);
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = order.clone();
        process_reports_in_order(rx, move |kind, _| {
            let observed = observed.clone();
            Box::pin(async move {
                tokio::task::yield_now().await;
                observed.lock().unwrap().push(kind);
            })
        })
        .await;
        assert_eq!(
            *order.lock().unwrap(),
            ["ReportPlaybackProgress", "ReportPlaybackStopped"]
        );
    }

    #[test]
    fn handshake_device_uses_registered_identity_and_rejects_spoofing() {
        assert_eq!(
            validated_device_id(None, Some("registered"), "token").unwrap(),
            "registered"
        );
        let anonymous_fallback = validated_device_id(None, Some("anonymous:abc"), "token").unwrap();
        assert!(!anonymous_fallback.starts_with("anonymous:"));
        assert_eq!(
            validated_device_id(Some("client-id"), Some("anonymous:abc"), "token").unwrap(),
            "client-id"
        );
        assert_eq!(
            validated_device_id(Some(&"x".repeat(128)), None, "token")
                .unwrap()
                .len(),
            128
        );
        assert!(validated_device_id(Some("other"), Some("registered"), "token").is_err());
        assert!(validated_device_id(Some("bad,device"), None, "token").is_err());
        let fallback = validated_device_id(None, None, "token").unwrap();
        assert_eq!(fallback, validated_device_id(None, None, "token").unwrap());
        assert_ne!(
            fallback,
            validated_device_id(None, None, "other-token").unwrap()
        );
        assert!(!fallback.contains("token-device:token"));
    }

    #[test]
    fn playback_report_cannot_replace_handshake_authority() {
        let report = authenticated_report(
            json!({"ItemId":"work_1", "UserId":"victim", "api_key":"other"}),
            "owner",
            "token",
            false,
        )
        .unwrap();
        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["UserId"], "owner");
        assert_eq!(value["api_key"], "token");
        let report = authenticated_report(
            json!({"ItemId":"work_1", "UserId":"target", "x-emby-token":"spoof"}),
            "admin",
            "admin-token",
            true,
        )
        .unwrap();
        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["UserId"], "target");
        assert_eq!(value["api_key"], "admin-token");
        assert!(websocket_authorization("device").is_ascii());
    }
    #[tokio::test]
    async fn unmapped_ws_item_ids_are_rejected_with_not_found() {
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
        state
            .caches
            .token_user_cache
            .insert("token".into(), Arc::new(("owner".into(), false)))
            .await;
        state
            .caches
            .token_touch_debounce
            .insert("token".into(), ())
            .await;
        let headers = HeaderMap::from_iter([(
            "x-emby-token".parse().unwrap(),
            HeaderValue::from_static("token"),
        )]);
        let report = json!({"ItemId":"unmapped", "PlaySessionId":"play", "PositionTicks":1, "RunTimeTicks":100});
        assert_eq!(
            dispatch_playback_report(
                &state,
                &headers,
                "ReportPlaybackStart",
                report,
                WsPlaybackContext {
                    user_id: "owner",
                    token: "token",
                    is_admin: false,
                    identity: &("device".into(), "设备".into(), "客户端".into())
                }
            )
            .await,
            Some(StatusCode::NOT_FOUND)
        );
        assert!(!state.sessions.contains_key("play"));
    }
    #[tokio::test]
    #[ignore = "设置 TEST_DATABASE_URL 验证 WS 上报复用 HTTP 持久化、预热和完成逻辑"]
    async fn ws_reports_persist_progress_and_completion() {
        let url = std::env::var("TEST_DATABASE_URL").expect("需要 PostgreSQL 测试连接");
        let db = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql(r#"
            CREATE TEMP TABLE media_works(id BIGINT PRIMARY KEY, tmdb_id INT, media_type TEXT, category TEXT);
            CREATE TEMP TABLE sync_folders(id BIGINT PRIMARY KEY, work_id BIGINT, is_ignored BOOL, priority_boost INT);
            CREATE TEMP TABLE sync_files(id BIGINT PRIMARY KEY, folder_id BIGINT, season_number INT, episode_number INT, priority_override INT, priority_score INT, alist_path TEXT, path_hash BIGINT);
            CREATE TEMP TABLE user_favorites(user_id TEXT, work_id BIGINT, tmdb_id INT, media_type TEXT);
            CREATE TEMP TABLE play_histories(user_id TEXT, work_id BIGINT, tmdb_id INT, media_type TEXT,
                season_number INT, episode_number INT, position_ticks BIGINT, duration_ticks BIGINT,
                is_played BOOL, play_count INT, last_played_at TIMESTAMPTZ, hidden_from_resume BOOL DEFAULT FALSE);
            CREATE UNIQUE INDEX ON play_histories(user_id, tmdb_id, media_type, season_number, episode_number) WHERE tmdb_id IS NOT NULL;
            CREATE UNIQUE INDEX ON play_histories(user_id, work_id, season_number, episode_number) WHERE tmdb_id IS NULL AND work_id IS NOT NULL;
            INSERT INTO media_works VALUES(1, 11, 'tv', 'tv');
            INSERT INTO sync_folders VALUES(1, 1, FALSE, 0);
            INSERT INTO sync_files VALUES(1, 1, 1, 1, NULL, 0, '/episode', 1);
        "#).execute(&db).await.unwrap();
        let (tx, _) = tokio::sync::mpsc::channel(1);
        let state = AppState::new(
            crate::config::BootstrapFileConfig::default().into_dynamic(),
            db.clone(),
            tx,
        )
        .unwrap();
        state
            .caches
            .token_user_cache
            .insert("token".into(), Arc::new(("owner".into(), false)))
            .await;
        state
            .caches
            .token_touch_debounce
            .insert("token".into(), ())
            .await;
        state
            .caches
            .user_categories_cache
            .insert("owner".into(), None)
            .await;
        state
            .caches
            .enabled_vlibs_cache
            .insert("enabled".into(), vec![])
            .await;
        let headers = HeaderMap::from_iter([
            (
                "x-emby-token".parse().unwrap(),
                HeaderValue::from_static("token"),
            ),
            (
                "x-emby-authorization".parse().unwrap(),
                HeaderValue::from_static("Emby DeviceId=\"device\""),
            ),
        ]);
        let report = |position| json!({"ItemId":"ep_1_1_1", "PlaySessionId":"play", "PositionTicks":position, "RunTimeTicks":100});
        for (kind, position) in [
            ("ReportPlaybackStart", 10),
            ("ReportPlaybackProgress", 85),
            ("ReportPlaybackProgress", 95),
            ("ReportPlaybackStopped", 95),
        ] {
            assert_eq!(
                dispatch_playback_report(
                    &state,
                    &headers,
                    kind,
                    report(position),
                    WsPlaybackContext {
                        user_id: "owner",
                        token: "token",
                        is_admin: false,
                        identity: &("device".into(), "设备".into(), "客户端".into())
                    }
                )
                .await,
                Some(StatusCode::NO_CONTENT)
            );
            let row: (i64, bool, i32) = sqlx::query_as("SELECT position_ticks, is_played, play_count FROM play_histories WHERE user_id='owner'").fetch_one(&db).await.unwrap();
            assert_eq!(
                row,
                if position >= 90 {
                    (0, true, 1)
                } else {
                    (position, false, 0)
                }
            );
            if kind == "ReportPlaybackProgress" {
                assert!(state.sessions.get("play").unwrap().next_ep_prewarmed);
            }
        }
        assert!(state.sessions.is_empty());
        db.close().await;
    }
}
