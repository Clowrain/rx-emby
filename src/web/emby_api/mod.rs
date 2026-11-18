use crate::state::AppState;
use axum::{
    http::header,
    middleware::{self, Next},
    response::Response,
    routing::{delete, get, post},
    Router,
};
use std::sync::Arc;
use std::time::Instant;
use tower_http::cors::{Any, CorsLayer};

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
use items::{get_item_detail, get_user_items, ItemsQuery};
#[cfg(test)]
use serde_json::{json, Value};
#[cfg(test)]
use views::get_user_views;

mod auth;
mod display_prefs;
mod ids;
mod images;
mod items;
mod metadata_lists;
mod playback;
mod playlists;
mod search;
mod sessions;
mod subtitles;
mod system_ops;
mod views;
mod ws;

pub(in crate::web) use auth::invalidate_token_cache;

pub fn router() -> Router<Arc<AppState>> {
    let core = Router::new()
        .route(
            "/System/Ping",
            get(auth::system_ping).post(auth::system_ping),
        )
        .route("/System/Info/Public", get(auth::system_info_public))
        .route("/System/Info", get(auth::system_info))
        .route("/Users/Public", get(auth::get_public_users))
        .route(
            "/Users/AuthenticateByName",
            post(auth::authenticate_by_name),
        )
        .route("/Users/:user_id", get(auth::get_user_info))
        .route("/Users/:user_id/Views", get(views::get_user_views))
        .route("/Users/:user_id/Items", get(items::get_user_items))
        .route("/Users/:user_id/Items/Resume", get(items::get_resume_items))
        .route("/Users/:user_id/Items/Latest", get(items::get_latest_items))
        .route("/Users/:user_id/Items/Root", get(items::get_root_folder))
        .route(
            "/Users/:user_id/Items/:item_id",
            get(items::get_item_detail),
        )
        .route(
            "/Users/:user_id/PlayedItems/:item_id",
            post(sessions::mark_item_played).delete(sessions::unmark_item_played),
        )
        .route(
            "/Users/:user_id/FavoriteItems/:item_id",
            post(sessions::mark_item_favorite).delete(sessions::unmark_item_favorite),
        )
        .route(
            "/Users/:user_id/Items/:item_id/UserData",
            post(sessions::post_user_item_data),
        )
        .route(
            "/Users/:user_id/Items/:item_id/HideFromResume",
            post(sessions::post_hide_from_resume),
        )
        // 非 user-scoped 别名：官方客户端与第三方（Infuse/VidHub）常直接请求 /Items 前缀
        .route("/Items", get(items::get_items_unscoped))
        .route("/Items/Counts", get(metadata_lists::get_item_counts))
        .route(
            "/Items/:item_id",
            get(items::get_item_detail_unscoped).delete(playlists::delete_item),
        )
        .route("/Playlists/:id", delete(playlists::delete_list))
        .route("/Collections/:id", delete(playlists::delete_list))
        .route(
            "/Items/:item_id/Similar",
            get(metadata_lists::get_similar_items),
        )
        .route(
            "/Movies/:item_id/Similar",
            get(metadata_lists::get_similar_items),
        )
        .route(
            "/Shows/:item_id/Similar",
            get(metadata_lists::get_similar_items),
        )
        .route("/Genres", get(metadata_lists::get_genres))
        .route("/Genres/:name", get(metadata_lists::get_genre_by_name))
        .route("/Studios", get(metadata_lists::get_studios))
        .route("/Studios/:name", get(metadata_lists::get_studio_by_name))
        .route("/Persons/:name", get(metadata_lists::get_person_by_name))
        .route("/Artists/:name", get(metadata_lists::get_artist_by_name))
        .route("/Tags", get(metadata_lists::get_tags))
        .route("/Years", get(metadata_lists::get_years))
        .route("/Persons", get(metadata_lists::get_empty_people_or_artists))
        .route("/Artists", get(metadata_lists::get_empty_people_or_artists))
        .route(
            "/Artists/AlbumArtists",
            get(metadata_lists::get_empty_people_or_artists),
        )
        .route(
            "/OfficialRatings",
            get(metadata_lists::get_official_ratings),
        )
        .route(
            "/Localization/ParentalRatings",
            get(metadata_lists::get_parental_ratings),
        )
        .route("/Search/Hints", get(search::get_search_hints))
        .route("/Shows/NextUp", get(items::get_next_up_items))
        .route("/Shows/:series_id/Seasons", get(items::get_show_seasons))
        .route("/Shows/:series_id/Episodes", get(items::get_show_episodes))
        .route(
            "/Items/:item_id/PlaybackInfo",
            post(playback::get_playback_info).get(playback::get_playback_info),
        )
        .route(
            "/Users/:user_id/Items/:item_id/PlaybackInfo",
            post(playback::get_user_playback_info).get(playback::get_user_playback_info),
        )
        .route("/Playback/BitrateTest", get(playback::bitrate_test))
        .route("/MediaSegments/:item_id", get(playback::get_media_segments))
        .route("/Sessions", get(sessions::get_sessions))
        .route("/Sessions/Capabilities", post(sessions::post_capabilities))
        .route(
            "/Sessions/Capabilities/Full",
            post(sessions::post_capabilities),
        )
        .route("/Sessions/Logout", post(sessions::post_logout))
        .route("/Sessions/Playing", post(sessions::session_playing))
        .route(
            "/Sessions/Playing/Progress",
            post(sessions::session_progress),
        )
        .route("/Sessions/Playing/Stopped", post(sessions::session_stopped))
        .route(
            "/Videos/:item_id/stream",
            get(playback::stream_video).head(playback::stream_video),
        )
        .route(
            "/Videos/:item_id/:file_name",
            get(playback::stream_video_with_file).head(playback::stream_video_with_file),
        )
        .route(
            "/Videos/:item_id/:media_source_id/Subtitles/:index/:stream_file",
            get(subtitles::get_subtitle_stream).head(subtitles::get_subtitle_stream),
        )
        .route(
            "/Items/:item_id/:media_source_id/Subtitles/:index/:stream_file",
            get(subtitles::get_subtitle_stream).head(subtitles::get_subtitle_stream),
        )
        .route(
            "/Videos/:item_id/:media_source_id/Subtitles/:index/:start_position_ticks/:stream_file",
            get(subtitles::get_subtitle_stream_at).head(subtitles::get_subtitle_stream_at),
        )
        .route(
            "/Items/:item_id/:media_source_id/Subtitles/:index/:start_position_ticks/:stream_file",
            get(subtitles::get_subtitle_stream_at).head(subtitles::get_subtitle_stream_at),
        )
        .route("/d/*alist_path", get(playback::stream_by_d_path))
        .route(
            "/Users/:user_id/Images/:image_type",
            get(images::get_user_image).head(images::get_user_image),
        )
        .route(
            "/Users/:user_id/Images/:image_type/:image_index",
            get(images::get_user_image_indexed).head(images::get_user_image_indexed),
        )
        .route(
            "/Items/:item_id/Images/:image_type",
            get(images::get_item_image).head(images::get_item_image),
        )
        .route(
            "/Items/:item_id/Images/:image_type/:image_index",
            get(images::get_item_image_indexed).head(images::get_item_image_indexed),
        )
        .route(
            "/DisplayPreferences/:display_preferences_id",
            get(display_prefs::get_display_preferences)
                .post(display_prefs::post_display_preferences),
        )
        .route("/Playlists", post(playlists::create_playlist))
        .route(
            "/Playlists/:id/Items",
            get(playlists::get_playlist_items)
                .post(playlists::add_playlist_items)
                .delete(playlists::remove_playlist_items),
        )
        .route(
            "/Playlists/:id/Items/:item_id/Move/:new_index",
            post(playlists::move_playlist_item),
        )
        .route(
            "/Playlists/:id/Items/Delete",
            post(playlists::remove_playlist_items),
        )
        .route(
            "/Collections/:id/Items/Delete",
            post(playlists::remove_collection_items),
        )
        .route("/Collections", post(playlists::create_collection))
        .route(
            "/Collections/:id/Items",
            get(playlists::get_collection_items)
                .post(playlists::add_collection_items)
                .delete(playlists::remove_collection_items),
        )
        .route(
            "/Users/:user_id/Suggestions",
            get(metadata_lists::get_user_suggestions),
        )
        .route(
            "/Movies/Recommendations",
            get(metadata_lists::get_movie_recommendations),
        )
        .route("/embywebsocket", get(ws::emby_websocket))
        .route("/socket", get(ws::emby_websocket))
        .route("/Library/Refresh", post(system_ops::post_library_refresh))
        .route("/ScheduledTasks", get(system_ops::get_scheduled_tasks))
        .route(
            "/ScheduledTasks/Running/:id",
            post(system_ops::post_run_scheduled_task),
        )
        .route(
            "/System/ActivityLog/Entries",
            get(system_ops::get_activity_log_entries),
        )
        .route("/System/Endpoint", get(system_ops::get_system_endpoint))
        .route(
            "/Branding/Configuration",
            get(system_ops::get_branding_configuration),
        )
        .route("/LiveTv/Info", get(system_ops::get_livetv_info))
        .route(
            "/Videos/ActiveEncodings",
            delete(system_ops::delete_active_encodings),
        )
        .route(
            "/Videos/ActiveEncodings/Delete",
            post(system_ops::delete_active_encodings),
        );

    Router::new()
        .nest("/emby", core.clone())
        .merge(core)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(middleware::from_fn(trace_emby_request))
}

pub(in crate::web) fn redacted_uri(uri: &axum::http::Uri) -> String {
    let Some(query) = uri.query() else {
        return uri.path().to_owned();
    };
    let query: String = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(
            url::form_urlencoded::parse(query.as_bytes()).map(|(key, value)| {
                let value = if ["api_key", "x-emby-token", "apikey"]
                    .iter()
                    .any(|alias| key.eq_ignore_ascii_case(alias))
                {
                    std::borrow::Cow::Borrowed("[REDACTED]")
                } else {
                    value
                };
                (key, value)
            }),
        )
        .finish();
    format!("{}?{}", uri.path(), query)
}

async fn trace_emby_request(req: axum::extract::Request, next: Next) -> Response {
    let method = req.method().clone();
    let is_image = req.uri().path().contains("/Images/");
    let uri = redacted_uri(req.uri());
    let ua = req
        .headers()
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    let range = req
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    // 图片请求降级为 debug，但保留慢请求可见性与完整链路追踪
    if !is_image {
        tracing::info!(
            "➡️  [EMBY REQ] {} {} | UA=\"{}\" | Range={}",
            method,
            uri,
            ua,
            range
        );
    }

    let started = Instant::now();
    let resp = next.run(req).await;

    let status = resp.status();
    let elapsed = started.elapsed();
    let loc = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    if is_image {
        if elapsed.as_millis() > 2000 {
            tracing::warn!(
                "🐢 [EMBY SLOW IMG] {} {} -> HTTP {} | 耗时={}ms",
                method,
                uri,
                status,
                elapsed.as_millis()
            );
        } else {
            tracing::debug!(
                "[EMBY IMG] {} {} -> HTTP {} | 耗时={}ms",
                method,
                uri,
                status,
                elapsed.as_millis()
            );
        }
    } else {
        tracing::info!(
            "⬅️  [EMBY RESP] {} {} -> HTTP {} | 耗时={}ms | Location={}",
            method,
            uri,
            status,
            elapsed.as_millis(),
            loc
        );
    }
    resp
}

#[cfg(test)]
#[path = "../smart_view_tests.rs"]
mod smart_view_tests;

#[cfg(test)]
mod compat_tests;
