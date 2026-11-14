use crate::state::AppState;
use axum::{
    http::header,
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Router,
};
use std::sync::Arc;

pub use crate::smart_views::VirtualLibraryRule;
pub use images::resolve_virtual_library_cover_key;
pub use views::count_virtual_library_matches;

#[cfg(test)]
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
#[cfg(test)]
use images::fetch_virtual_library_top_media_images;
#[cfg(test)]
use items::{get_user_items, ItemsQuery};
#[cfg(test)]
use serde_json::{json, Value};
#[cfg(test)]
use views::get_user_views;

mod auth;
mod images;
mod items;
mod playback;
mod sessions;
mod views;

pub fn router() -> Router<Arc<AppState>> {
    let core = Router::new()
        .route("/System/Info/Public", get(auth::system_info_public))
        .route("/System/Info", get(auth::system_info_public))
        .route("/Users/AuthenticateByName", post(auth::authenticate_by_name))
        .route("/Users/:user_id", get(auth::get_user_info))
        .route("/Users/:user_id/Views", get(views::get_user_views))
        .route("/Users/:user_id/Items", get(items::get_user_items))
        .route("/Users/:user_id/Items/Resume", get(items::get_resume_items))
        .route("/Users/:user_id/Items/Latest", get(items::get_latest_items))
        .route("/Users/:user_id/Items/:item_id", get(items::get_item_detail))
        .route(
            "/Users/:user_id/PlayedItems/:item_id",
            post(sessions::mark_item_played).delete(sessions::unmark_item_played),
        )
        .route(
            "/Users/:user_id/FavoriteItems/:item_id",
            post(sessions::mark_item_favorite).delete(sessions::unmark_item_favorite),
        )
        .route("/Shows/NextUp", get(items::get_next_up_items))
        .route("/Shows/:series_id/Seasons", get(items::get_show_seasons))
        .route("/Shows/:series_id/Episodes", get(items::get_show_episodes))
        .route("/Items/:item_id/PlaybackInfo", post(playback::get_playback_info).get(playback::get_playback_info))
        .route("/Sessions/Playing", post(sessions::session_playing))
        .route("/Sessions/Playing/Progress", post(sessions::session_progress))
        .route("/Sessions/Playing/Stopped", post(sessions::session_stopped))
        .route("/Videos/:item_id/stream", get(playback::stream_video))
        .route("/Videos/:item_id/:file_name", get(playback::stream_video_with_file))
        .route("/d/*alist_path", get(playback::stream_by_d_path))
        .route("/Items/:item_id/Images/:image_type", get(images::get_item_image))
        .route(
            "/Items/:item_id/Images/:image_type/:image_index",
            get(images::get_item_image_indexed),
        );

    Router::new()
        .nest("/emby", core.clone())
        .merge(core)
        .layer(middleware::from_fn(trace_emby_request))
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


#[cfg(test)]
#[path = "../smart_view_tests.rs"]
mod smart_view_tests;
