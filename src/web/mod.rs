pub mod admin_api;
pub mod emby_api;

use crate::state::AppState;
use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use rust_embed::Embed;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

#[derive(Embed)]
#[folder = "web_ui/"]
struct WebAssets;

pub fn build_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/", get(serve_index))
        .route("/assets/*path", get(serve_assets))
        .route("/web/*path", get(serve_static))
        .merge(emby_api::router())
        .merge(admin_api::router())
        .layer(cors)
        .with_state(state)
}

async fn serve_index() -> impl IntoResponse {
    match WebAssets::get("index.html") {
        Some(content) => Html(String::from_utf8_lossy(content.data.as_ref()).into_owned()).into_response(),
        None => (StatusCode::NOT_FOUND, "index.html 未找到").into_response(),
    }
}

async fn serve_assets(Path(path): Path<String>) -> Response {
    let asset_path = format!("assets/{}", path.trim_start_matches('/'));
    serve_embedded_file(&asset_path).await
}

async fn serve_static(Path(path): Path<String>) -> Response {
    let clean_path = path.trim_start_matches('/');
    serve_embedded_file(clean_path).await
}

async fn serve_embedded_file(clean_path: &str) -> Response {
    match WebAssets::get(clean_path) {
        Some(content) => {
            let mime = if clean_path.ends_with(".js") || clean_path.ends_with(".mjs") {
                "application/javascript; charset=utf-8"
            } else if clean_path.ends_with(".css") {
                "text/css; charset=utf-8"
            } else if clean_path.ends_with(".svg") {
                "image/svg+xml"
            } else if clean_path.ends_with(".json") {
                "application/json"
            } else {
                "text/html; charset=utf-8"
            };
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime)
                .body(axum::body::Body::from(content.data.into_owned()))
                .unwrap()
        }
        None => serve_index().await.into_response(),
    }
}
