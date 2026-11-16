pub mod admin_api;
pub mod emby_api;

use crate::state::AppState;
use axum::{
    extract::{Path, Request},
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

pub(crate) fn get_embedded_asset(path: &str) -> Option<rust_embed::EmbeddedFile> {
    WebAssets::get(path)
}

pub fn build_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/", get(serve_index))
        .route("/favicon.ico", get(serve_favicon))
        .route("/assets/*path", get(serve_assets))
        .route("/covers/*path", get(serve_covers))
        .route("/web/*path", get(serve_static))
        .merge(emby_api::router())
        .merge(admin_api::router(state.clone()))
        .fallback(log_unmatched_404)
        .layer(cors)
        .with_state(state)
}

async fn log_unmatched_404(req: Request) -> StatusCode {
    let ua = req
        .headers()
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    tracing::warn!("⚠️ [HTTP 404] {} {} | UA={:?}", req.method(), req.uri(), ua);
    StatusCode::NOT_FOUND
}

async fn serve_index() -> impl IntoResponse {
    match WebAssets::get("index.html") {
        Some(content) => {
            Html(String::from_utf8_lossy(content.data.as_ref()).into_owned()).into_response()
        }
        None => (StatusCode::NOT_FOUND, "index.html 未找到").into_response(),
    }
}

async fn serve_favicon() -> Response {
    serve_embedded_file("favicon.ico").await
}

async fn serve_assets(Path(path): Path<String>) -> Response {
    let asset_path = format!("assets/{}", path.trim_start_matches('/'));
    serve_embedded_file(&asset_path).await
}

async fn serve_covers(Path(path): Path<String>) -> Response {
    let cover_path = format!("covers/{}", path.trim_start_matches('/'));
    serve_embedded_file(&cover_path).await
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
            } else if clean_path.ends_with(".jpg") || clean_path.ends_with(".jpeg") {
                "image/jpeg"
            } else if clean_path.ends_with(".png") {
                "image/png"
            } else if clean_path.ends_with(".webp") {
                "image/webp"
            } else if clean_path.ends_with(".json") {
                "application/json"
            } else {
                "text/html; charset=utf-8"
            };
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime)
                .body(axum::body::Body::from(content.data.into_owned()))
                .unwrap_or_else(|error| {
                    tracing::error!("构建静态资源响应失败: {}", error);
                    (StatusCode::INTERNAL_SERVER_ERROR, "静态资源响应构建失败").into_response()
                })
        }
        None => serve_index().await.into_response(),
    }
}
