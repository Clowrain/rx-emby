use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use tower::ServiceExt;

fn lazy_test_state() -> Arc<AppState> {
    let db = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:5432/rx_emby")
        .expect("lazy PgPool should construct without connecting");
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    AppState::new(
        crate::config::BootstrapFileConfig::default().into_dynamic(),
        db,
        tx,
    )
    .expect("test AppState should construct")
}

async fn send(app: &Router, method: Method, uri: &str, body: Body) -> (StatusCode, bytes::Bytes) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(body)
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, bytes)
}

#[tokio::test]
async fn public_p0_to_p3_routes_respond_on_both_root_and_emby_prefix() {
    let app = crate::web::build_router(lazy_test_state());

    for prefix in ["", "/emby"] {
        let (status, body) = send(
            &app,
            Method::GET,
            &format!("{prefix}/System/Ping"),
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"Emby Server");

        let (status, _) = send(
            &app,
            Method::HEAD,
            &format!("{prefix}/System/Ping"),
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = send(
            &app,
            Method::POST,
            &format!("{prefix}/System/Ping"),
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = send(
            &app,
            Method::GET,
            &format!("{prefix}/Playback/BitrateTest?Size=256"),
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.len(), 256);

        let (status, _) = send(
            &app,
            Method::GET,
            &format!("{prefix}/Localization/ParentalRatings"),
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = send(
            &app,
            Method::GET,
            &format!("{prefix}/Branding/Configuration"),
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
}

#[tokio::test]
async fn protected_p0_to_p3_routes_are_mounted_and_require_auth() {
    let app = crate::web::build_router(lazy_test_state());

    let protected_routes = [
        (Method::GET, "/emby/System/Info"),
        (Method::GET, "/emby/Sessions"),
        (Method::POST, "/emby/Sessions/Capabilities"),
        (Method::POST, "/emby/Sessions/Capabilities/Full"),
        (Method::GET, "/emby/Items/Counts"),
        (Method::GET, "/emby/Genres"),
        (Method::GET, "/emby/Studios"),
        (Method::GET, "/emby/Tags"),
        (Method::GET, "/emby/Years"),
        (Method::GET, "/emby/Persons"),
        (Method::GET, "/emby/Artists"),
        (Method::GET, "/emby/OfficialRatings"),
        (Method::GET, "/emby/DisplayPreferences/home"),
        (Method::POST, "/emby/Playlists"),
        (Method::POST, "/emby/Collections"),
        (Method::GET, "/emby/Movies/Recommendations"),
        (Method::POST, "/emby/Library/Refresh"),
        (Method::GET, "/emby/ScheduledTasks"),
        (Method::GET, "/emby/System/ActivityLog/Entries"),
        (Method::GET, "/emby/System/Endpoint"),
        (Method::GET, "/emby/LiveTv/Info"),
        (Method::DELETE, "/emby/Videos/ActiveEncodings"),
        (Method::POST, "/emby/Videos/ActiveEncodings/Delete"),
    ];

    for (method, path) in protected_routes {
        let (status, _) = send(&app, method.clone(), path, Body::from("{}")).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} should be mounted and reject missing token with 401"
        );
    }
}

#[test]
fn token_headers_override_query_and_authorization_headers_fall_back() {
    use axum::http::{HeaderMap, HeaderValue};
    let mut headers = HeaderMap::new();
    headers.insert("x-emby-token", HeaderValue::from_static("header"));
    assert_eq!(
        auth::extract_emby_token(&headers, Some("query")).as_deref(),
        Some("header")
    );
    headers.remove("x-emby-token");
    headers.insert(
        "x-emby-authorization",
        HeaderValue::from_static("Emby Client=Test"),
    );
    headers.insert(
        "authorization",
        HeaderValue::from_static("Emby Token=standard"),
    );
    assert_eq!(
        auth::extract_emby_token(&headers, None).as_deref(),
        Some("standard")
    );
}

#[test]
fn trace_uri_redacts_all_token_aliases() {
    let uri = "/Items?api_key=secret&X-Emby-Token=secret&ApiKey=secret&Limit=4"
        .parse()
        .unwrap();
    let redacted = redacted_uri(&uri);
    assert!(!redacted.contains("secret"));
    assert!(redacted.contains("Limit=4"));
}

#[tokio::test]
async fn bitrate_head_preserves_length_without_body() {
    let app = crate::web::build_router(lazy_test_state());
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::HEAD)
                .uri("/emby/Playback/BitrateTest?Size=10000000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["content-length"], "10000000");
    assert!(to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn non_admin_sessions_are_private() {
    let state = lazy_test_state();
    state
        .caches
        .token_user_cache
        .insert("test-token".into(), Arc::new(("alice".into(), false)))
        .await;
    for uid in ["alice", "bob"] {
        state.sessions.insert(
            uid.into(),
            crate::state::PlaybackSession {
                session_id: uid.into(),
                user_id: uid.into(),
                device_id: uid.into(),
                device_name: "test".into(),
                client_name: "test".into(),
                current_episode_id: None,
                position_ticks: 0,
                duration_ticks: 0,
                next_ep_prewarmed: false,
                completed_in_session: false,
                last_activity: std::time::Instant::now(),
            },
        );
    }
    let app = crate::web::build_router(state);
    let (_, body) = send(
        &app,
        Method::GET,
        "/Sessions?api_key=test-token",
        Body::empty(),
    )
    .await;
    let sessions: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert_eq!(sessions[0]["UserId"], "alice");
}

#[tokio::test]
async fn anonymous_user_image_is_consistent_and_cacheable() {
    let app = crate::web::build_router(lazy_test_state());
    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/Users/unknown/Images/Primary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let content_type = first
        .headers()
        .get("content-type")
        .expect("avatar Content-Type")
        .clone();
    let etag = first.headers().get("etag").expect("avatar ETag").clone();
    let head = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::HEAD)
                .uri("/Users/unknown/Images/Primary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers().get("content-type"), Some(&content_type));
    assert_eq!(head.headers().get("etag"), Some(&etag));
    let cached = app
        .oneshot(
            Request::builder()
                .uri("/Users/another/Images/Primary")
                .header("if-none-match", etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn batch2_closed_loop_routes_require_auth_on_both_prefixes() {
    let app = crate::web::build_router(lazy_test_state());
    for prefix in ["", "/emby"] {
        for (method, path) in [
            (Method::GET, "/Studios/missing"),
            (Method::GET, "/Persons/missing"),
            (Method::GET, "/Artists/missing"),
            (Method::GET, "/Collections/col_1/Items"),
            (Method::POST, "/Collections/col_1/Items/Delete"),
            (Method::POST, "/Playlists/pl_1/Items/Delete"),
        ] {
            let (status, _) =
                send(&app, method, &format!("{prefix}{path}"), Body::from("{}")).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{prefix}{path}");
        }
    }
}
