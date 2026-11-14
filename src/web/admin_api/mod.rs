use crate::state::AppState;
use axum::{
    middleware,
    routing::{delete, get, post},
    Router,
};
use std::sync::Arc;

mod auth;
mod media;
mod system;
mod users;
mod virtual_libraries;

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let public_auth = Router::new()
        .route("/api/admin/auth/login", post(auth::admin_login))
        .route("/api/admin/auth/logout", post(auth::admin_logout));

    let protected = Router::new()
        .route("/api/admin/auth/me", get(auth::admin_me))
        .route("/api/admin/overview", get(system::get_admin_overview))
        .route("/api/admin/configs", get(system::list_configs).post(system::update_configs))
        .route("/api/admin/sync/trigger", post(system::trigger_sync))
        .route("/api/admin/sync/logs", get(system::list_sync_logs))
        .route("/api/admin/cache/stats", get(system::image_cache_stats))
        .route("/api/admin/cache/evict", post(system::trigger_cache_evict))
        .route("/api/admin/media", get(media::list_admin_media))
        .route("/api/admin/media/:id/detail", get(media::get_admin_media_detail))
        .route("/api/admin/media/:id/prewarm_dirs", post(media::prewarm_admin_media_dirs))
        .route("/api/admin/media/:id/bind_tmdb", post(media::bind_tmdb_id))
        .route("/api/admin/media/:id/scrape", post(media::scrape_one_now))
        .route("/api/admin/media/:id/ignore", post(media::toggle_ignore_media))
        .route("/api/admin/scrape/retry_failed", post(media::retry_failed_scrapes))
        .route(
            "/api/admin/virtual_libraries",
            get(virtual_libraries::list_virtual_libraries)
                .post(virtual_libraries::save_virtual_library),
        )
        .route("/api/admin/virtual_libraries/options", get(virtual_libraries::smart_view_options))
        .route("/api/admin/virtual_libraries/preview", post(virtual_libraries::preview_smart_view))
        .route("/api/admin/virtual_libraries/backfill", post(virtual_libraries::queue_metadata_backfill))
        .route(
            "/api/admin/virtual_libraries/presets",
            post(virtual_libraries::seed_preset_virtual_libraries),
        )
        .route(
            "/api/admin/virtual_libraries/:id",
            delete(virtual_libraries::delete_virtual_library),
        )
        .route(
            "/api/admin/base_libraries/covers",
            get(virtual_libraries::list_base_library_covers)
                .post(virtual_libraries::save_base_library_cover),
        )
        .route("/api/admin/users", get(users::list_users_and_sessions).post(users::save_user))
        .route("/api/admin/users/:id", delete(users::delete_user))
        .route("/api/admin/tokens/:token", delete(users::revoke_access_token))
        .route_layer(middleware::from_fn_with_state(state, auth::require_admin_auth));

    Router::new().merge(public_auth).merge(protected)
}
