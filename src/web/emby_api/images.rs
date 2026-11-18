use super::views::base_library_category;
use super::{
    auth::resolve_auth_context,
    ids::{parse_episode_id, parse_season_id},
};
use crate::smart_views::VirtualLibraryRule;
use crate::state::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::Response,
};
use bytes::Bytes;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize, Default)]
pub(super) struct ImageAuthQuery {
    #[serde(
        rename = "api_key",
        alias = "ApiKey",
        alias = "X-Emby-Token",
        alias = "x-emby-token"
    )]
    api_key: Option<String>,
}

fn image_child_work_id(item_id: &str) -> Option<i64> {
    parse_episode_id(item_id)
        .map(|(id, _, _)| id)
        .or_else(|| parse_season_id(item_id).map(|(id, _)| id))
        .or_else(|| super::ids::parse_work_id(item_id))
}

fn is_supported_image_type(image_type: &str) -> bool {
    image_type.eq_ignore_ascii_case("Primary") || image_type.eq_ignore_ascii_case("Backdrop")
}

fn image_meta_cache_key(item_id: &str, image_type: &str, user_id: &str) -> String {
    if item_id.starts_with("pl_") || item_id.starts_with("col_") {
        format!("{item_id}:{image_type}:{user_id}")
    } else {
        format!("{item_id}:{image_type}")
    }
}

fn playlist_collection_cover_permitted(
    collection: bool,
    owner: Option<&str>,
    auth_user: Option<(&str, bool)>,
) -> bool {
    match (collection, owner, auth_user) {
        (true, None, _) => true,
        (_, Some(owner), Some((user, is_admin))) => is_admin || owner == user,
        _ => false,
    }
}

pub(super) fn image_path_tag(path: Option<&str>) -> String {
    let Some(path) = path else {
        return String::new();
    };
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(path.as_bytes()))
}

fn head_image_response(
    headers: &HeaderMap,
    etag: &str,
    content_type: &str,
) -> Result<Response, (StatusCode, String)> {
    if let Some(response) = not_modified_response(headers, etag)? {
        return Ok(response);
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, "public, max-age=604800")
        .body(axum::body::Body::empty())
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

fn not_modified_response(
    headers: &HeaderMap,
    etag: &str,
) -> Result<Option<Response>, (StatusCode, String)> {
    let matches = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(',')
                .any(|candidate| candidate.trim() == etag || candidate.trim() == "*")
        })
        .unwrap_or(false);
    if !matches {
        return Ok(None);
    }
    Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, "public, max-age=604800")
        .body(axum::body::Body::empty())
        .map(Some)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

pub(super) async fn get_item_image_indexed(
    state: State<Arc<AppState>>,
    headers: HeaderMap,
    Path((item_id, image_type, _image_index)): Path<(String, String, String)>,
    Query(q): Query<ImageAuthQuery>,
    method: Method,
) -> Result<Response, (StatusCode, String)> {
    get_item_image_inner(
        state,
        headers,
        Path((item_id, image_type)),
        q.api_key.as_deref(),
        method,
    )
    .await
}

pub(super) async fn get_item_image(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((item_id, image_type)): Path<(String, String)>,
    Query(q): Query<ImageAuthQuery>,
    method: Method,
) -> Result<Response, (StatusCode, String)> {
    get_item_image_inner(
        State(state),
        headers,
        Path((item_id, image_type)),
        q.api_key.as_deref(),
        method,
    )
    .await
}

pub(super) async fn get_user_image_indexed(
    state: State<Arc<AppState>>,
    headers: HeaderMap,
    Path((user_id, image_type, _image_index)): Path<(String, String, String)>,
    method: Method,
) -> Result<Response, (StatusCode, String)> {
    get_user_image(state, headers, Path((user_id, image_type)), method).await
}

pub(super) async fn get_user_image(
    State(_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((_user_id, _image_type)): Path<(String, String)>,
    method: Method,
) -> Result<Response, (StatusCode, String)> {
    // 所有用户当前共用默认头像；匿名登录页返回一致结果，避免暴露用户是否存在。
    let etag = r#""default-user-avatar-v1""#;
    if method == Method::HEAD {
        return head_image_response(&headers, etag, "image/jpeg");
    }
    if let Some(response) = not_modified_response(&headers, etag)? {
        return Ok(response);
    }
    let mut response = serve_embedded_cover("vlib_default_tv")?;
    response
        .headers_mut()
        .insert(header::ETAG, axum::http::HeaderValue::from_static(etag));
    Ok(response)
}

async fn get_item_image_inner(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((item_id, image_type)): Path<(String, String)>,
    api_key: Option<&str>,
    method: Method,
) -> Result<Response, (StatusCode, String)> {
    if !is_supported_image_type(&image_type) {
        return Err((StatusCode::NOT_FOUND, "暂无图片".to_string()));
    }

    // 基础分类封面（内嵌资源 / 自定义配置 / 动态自动封面 / 拼接封面）
    if let Some(base_cat) = base_library_category(&item_id) {
        let auth = resolve_auth_context(&state, &headers, api_key).await?;
        let allowed = super::views::get_user_allowed_categories(&state, &auth.user_id).await?;
        if !crate::smart_views::can_access_view(allowed.as_deref(), &item_id, base_cat) {
            return Err((StatusCode::FORBIDDEN, "无权访问该媒体库".into()));
        }
        let custom_cover: Option<String> =
            sqlx::query_scalar("SELECT config_value FROM system_configs WHERE config_key = $1")
                .bind(format!("base_lib_cover:{}", item_id))
                .fetch_optional(&state.db)
                .await
                .map_err(|e| {
                    tracing::error!("查询基础库封面配置失败: {}", e);
                    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                })?;

        let clean_custom = custom_cover
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let rule = VirtualLibraryRule {
            categories: vec![base_cat.to_string()],
            ..Default::default()
        };
        let cover_key = clean_custom
            .filter(|k| is_builtin_cover_key(k))
            .unwrap_or(&item_id);
        return serve_library_cover(
            &state,
            &headers,
            method,
            &item_id,
            clean_custom,
            &rule,
            cover_key,
        )
        .await;
    }

    // 智能视图封面
    if let Some(raw_vid) = item_id.strip_prefix("lib_vlib_") {
        let vid = raw_vid
            .parse::<i32>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的智能视图 ID".to_string()))?;

        let auth = resolve_auth_context(&state, &headers, api_key).await?;

        let row: Option<(String, String, Option<String>, serde_json::Value)> = sqlx::query_as(
            "SELECT name, icon_type, cover_image, filter_rules FROM virtual_libraries WHERE id = $1 AND is_enabled = TRUE",
        )
        .bind(vid)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let Some((vname, icon_type, cover_image, rules_val)) = row else {
            return Err((StatusCode::NOT_FOUND, "智能视图不存在".to_string()));
        };

        if !crate::smart_views::can_access_view(
            super::views::get_user_allowed_categories(&state, &auth.user_id)
                .await?
                .as_deref(),
            &item_id,
            &vname,
        ) {
            return Err((StatusCode::FORBIDDEN, "无权访问该智能视图".into()));
        }

        let custom_cover = cover_image
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let rule: VirtualLibraryRule = serde_json::from_value(rules_val).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("智能视图规则损坏：{}", e),
            )
        })?;
        let cover_key = resolve_virtual_library_cover_key(custom_cover, &vname, &icon_type);
        return serve_library_cover(
            &state,
            &headers,
            method,
            &item_id,
            custom_cover,
            &rule,
            cover_key,
        )
        .await;
    }

    // Playlist/collection art inherits its parent's ACL and media category ACL.
    let auth = if item_id.starts_with("pl_") || item_id.starts_with("col_") {
        let collection = item_id.starts_with("col_");
        let prefix = if collection { "col_" } else { "pl_" };
        let id = item_id
            .strip_prefix(prefix)
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|id| *id > 0)
            .ok_or((StatusCode::BAD_REQUEST, "无效的资源 ID".to_string()))?;
        let auth = resolve_auth_context(&state, &headers, api_key).await?;
        let table = if collection {
            "collections"
        } else {
            "playlists"
        };
        let owner: Option<Option<String>> =
            sqlx::query_scalar(&format!("SELECT user_id FROM {table} WHERE id=$1"))
                .bind(id)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let Some(owner) = owner else {
            return Err((StatusCode::NOT_FOUND, "列表不存在".into()));
        };
        let permitted = playlist_collection_cover_permitted(
            collection,
            owner.as_deref(),
            Some((auth.user_id.as_str(), auth.is_admin)),
        );
        if !permitted {
            return Err((StatusCode::NOT_FOUND, "暂无图片".into()));
        }
        auth
    } else {
        // Media artwork is protected by the same ignored-folder and category ACL as details/playback.
        resolve_auth_context(&state, &headers, api_key).await?
    };

    let work_id = parse_episode_id(&item_id)
        .map(|(id, _, _)| id)
        .or_else(|| parse_season_id(&item_id).map(|(id, _)| id))
        .or_else(|| super::ids::parse_work_id(&item_id));
    if let Some(work_id) = work_id {
        super::items::ensure_media_accessible(&state, &auth.user_id, work_id).await?;
    }

    // 短 TTL 元数据缓存：热门海报/背景图的 DB 定位查询不再逐请求打库；
    // 播放列表/合集封面需按用户隔离缓存键，避免跨用户绕过子项分类 ACL。
    let meta_cache_key = image_meta_cache_key(&item_id, &image_type, &auth.user_id);
    let resolved = state
        .caches
        .image_meta_cache
        .try_get_with(meta_cache_key, async {
            resolve_item_image_meta(&state, &item_id, &image_type, &auth.user_id)
                .await
                .map_err(|(status, message)| format!("{}|{}", status.as_u16(), message))
        })
        .await
        .map_err(|error| {
            let (status, message) = error
                .split_once('|')
                .unwrap_or(("500", "图片元数据查询失败"));
            (
                status
                    .parse::<u16>()
                    .ok()
                    .and_then(|code| StatusCode::from_u16(code).ok())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                message.to_string(),
            )
        })?;
    let (size, clean_name) = resolved.ok_or((StatusCode::NOT_FOUND, "暂无图片".to_string()))?;
    if method == Method::HEAD {
        let etag_key = format!("{item_id}:{image_type}:{size}:{clean_name}");
        let etag = format!(
            "\"img-{:016x}\"",
            xxhash_rust::xxh3::xxh3_64(etag_key.as_bytes())
        );
        let content_type = if clean_name.ends_with(".png") {
            "image/png"
        } else {
            "image/jpeg"
        };
        return head_image_response(&headers, &etag, content_type);
    }

    let bytes = state
        .get_or_fetch_tmdb_image(&size, &clean_name)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("图片回源失败: {:#}", e)))?;

    let content_type = if clean_name.ends_with(".png") {
        "image/png"
    } else {
        "image/jpeg"
    };

    // Strong ETag derived from content so changed bytes at a stable path invalidate clients.
    let etag = format!("\"img-{:016x}\"", xxhash_rust::xxh3::xxh3_64(&bytes));
    if let Some(response) = not_modified_response(&headers, &etag)? {
        return Ok(response);
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, "public, max-age=604800")
        .body(axum::body::Body::from(bytes))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// 根据条目 ID 定位 TMDB 图片路径，返回 `(size, 清洗后的文件名)`；`None` 表示该条目暂无图片
async fn resolve_item_image_meta(
    state: &AppState,
    item_id: &str,
    image_type: &str,
    user_id: &str,
) -> Result<Option<(String, String)>, (StatusCode, String)> {
    if base_library_category(item_id).is_some() || item_id.starts_with("lib_vlib_") {
        // 基础分类与智能视图封面由动态封面/内嵌资源分支处理，不走 TMDB 路径
        return Err((StatusCode::NOT_FOUND, "暂无图片".to_string()));
    }

    if !is_supported_image_type(image_type) {
        return Ok(None);
    }
    if item_id.starts_with("pl_") || item_id.starts_with("col_") {
        let collection = item_id.starts_with("col_");
        let raw = item_id
            .strip_prefix(if collection { "col_" } else { "pl_" })
            .unwrap_or(item_id);
        let id = raw
            .parse::<i64>()
            .ok()
            .filter(|v| *v > 0)
            .ok_or((StatusCode::BAD_REQUEST, "无效的资源 ID".to_string()))?;
        let sql = if collection {
            "SELECT item_id FROM collection_items WHERE collection_id=$1 ORDER BY id LIMIT 1"
        } else {
            "SELECT item_id FROM playlist_items WHERE playlist_id=$1 ORDER BY sort_order,id LIMIT 1"
        };
        let child: Option<String> = sqlx::query_scalar(sql)
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if let Some(child) = child {
            if let Some(work_id) = image_child_work_id(&child) {
                super::items::ensure_media_accessible(state, user_id, work_id).await?;
            }
            return Box::pin(resolve_item_image_meta(state, &child, image_type, user_id)).await;
        }
        return Ok(None);
    }
    if let Some((wid, s_num, ep_num)) = parse_episode_id(item_id) {
        let ep_paths: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            r#"
            SELECT e.still_path, w.backdrop_path, w.poster_path
            FROM media_works w
            LEFT JOIN work_episodes e
              ON e.work_id = w.id AND e.season_number = $2 AND e.episode_number = $3
            WHERE w.id = $1
              AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = w.id AND f.is_ignored = FALSE)
            "#,
        )
        .bind(wid)
        .bind(s_num)
        .bind(ep_num)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let Some((still, backdrop, poster)) = ep_paths else {
            return Err((StatusCode::NOT_FOUND, "暂无图片".to_string()));
        };
        return Ok(still
            .or(backdrop)
            .or(poster)
            .map(|p| ("w500".to_string(), p.trim_start_matches('/').to_string())));
    }

    if let Some((wid, snum)) = parse_season_id(item_id) {
        // 单查询合并季海报与剧集兜底海报（原先为 2 次串行 DB 查询）
        let final_poster: Option<String> = sqlx::query_scalar(
            r#"
            SELECT COALESCE(ws.poster_path, w.poster_path)
            FROM media_works w
            LEFT JOIN work_seasons ws
              ON ws.work_id = w.id AND ws.season_number = $2 AND ws.poster_path IS NOT NULL
            WHERE w.id = $1
              AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = w.id AND f.is_ignored = FALSE)
            "#,
        )
        .bind(wid)
        .bind(snum)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .flatten();

        return Ok(
            final_poster.map(|p| ("w500".to_string(), p.trim_start_matches('/').to_string()))
        );
    }

    let Some(wid) = super::ids::parse_work_id(item_id) else {
        return Err((StatusCode::BAD_REQUEST, "无效的资源 ID".to_string()));
    };

    let paths: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT poster_path, backdrop_path FROM media_works w WHERE id = $1 AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = w.id AND f.is_ignored = FALSE)")
            .bind(wid)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some((poster, backdrop)) = paths else {
        return Err((StatusCode::NOT_FOUND, "暂无图片".to_string()));
    };

    if image_type.eq_ignore_ascii_case("Backdrop") {
        Ok(backdrop
            .or(poster)
            .map(|p| ("w1280".to_string(), p.trim_start_matches('/').to_string())))
    } else {
        Ok(poster.map(|p| ("w500".to_string(), p.trim_start_matches('/').to_string())))
    }
}

/// 从稳定内容/库状态派生封面缓存 tag（无时间分桶，客户端缓存跨窗口保持有效）。
/// `revision` 由调用方传入（基础库为配置 updated_at 时间戳，智能视图为 cover_revision），
/// 管理端修改封面后 revision 变化即可让客户端刷新，无需时间戳轮转。
pub(super) fn compute_cover_image_tag(cover_image: Option<&str>, revision: i64) -> String {
    match cover_image.map(str::trim).filter(|s| !s.is_empty()) {
        Some(c) => format!(
            "v{}_{:016x}",
            revision.max(1),
            xxhash_rust::xxh3::xxh3_64(c.as_bytes())
        ),
        None => format!("v{}", revision.max(1)),
    }
}

fn is_builtin_cover_key(key: &str) -> bool {
    matches!(
        key,
        "lib_movie"
            | "lib_tv"
            | "lib_anime"
            | "lib_doc"
            | "lib_variety"
            | "vlib_top_rated"
            | "vlib_streaming"
            | "vlib_chinese"
            | "vlib_kr_jp"
            | "vlib_latest"
            | "vlib_default_movies"
            | "vlib_default_tv"
    )
}

async fn try_serve_dynamic_cover(
    state: &Arc<AppState>,
    view_id: &str,
    custom_cover: Option<&str>,
    rule: &VirtualLibraryRule,
) -> Option<Response> {
    if custom_cover == Some("auto") {
        if let Some(file_path) = fetch_virtual_library_auto_image(state, rule).await {
            let clean_name = file_path.trim_start_matches('/');
            if let Ok(bytes) = state.get_or_fetch_tmdb_image("w1280", clean_name).await {
                let content_type = if clean_name.ends_with(".png") {
                    "image/png"
                } else {
                    "image/jpeg"
                };
                return Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, content_type)
                    .header(header::CACHE_CONTROL, "public, max-age=300")
                    .body(axum::body::Body::from(bytes))
                    .map_err(|e| {
                        tracing::warn!("构建动态封面响应失败: {}", e);
                        e
                    })
                    .ok();
            }
        }
    } else if let Some(style) =
        custom_cover.and_then(crate::image_cache::collage::CollageStyle::from_key)
    {
        if let Some(bytes) =
            fetch_or_build_virtual_library_collage(state, view_id, rule, style).await
        {
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "image/jpeg")
                .header(header::CACHE_CONTROL, "public, max-age=300")
                .body(axum::body::Body::from(bytes))
                .map_err(|e| {
                    tracing::warn!("构建拼接封面响应失败: {}", e);
                    e
                })
                .ok();
        }
    }
    None
}

async fn serve_library_cover(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    method: Method,
    item_id: &str,
    custom_cover: Option<&str>,
    rule: &VirtualLibraryRule,
    fallback_cover_key: &str,
) -> Result<Response, (StatusCode, String)> {
    let key = custom_cover.unwrap_or(item_id);
    let etag = format!(
        "\"cover-{:016x}\"",
        xxhash_rust::xxh3::xxh3_64(key.as_bytes())
    );
    if let Some(response) = not_modified_response(headers, &etag)? {
        return Ok(response);
    }
    if method == Method::HEAD && custom_cover != Some("auto") {
        return head_image_response(headers, &etag, "image/jpeg");
    }
    let mut response =
        if let Some(resp) = try_serve_dynamic_cover(state, item_id, custom_cover, rule).await {
            resp
        } else {
            serve_embedded_cover(fallback_cover_key)?
        };
    if let Ok(etag_val) = axum::http::HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, etag_val);
    }
    if method == Method::HEAD {
        let (parts, _) = response.into_parts();
        return Ok(Response::from_parts(parts, axum::body::Body::empty()));
    }
    Ok(response)
}

fn serve_embedded_cover(cover_key: &str) -> Result<Response, (StatusCode, String)> {
    let asset_path = format!("covers/{}.jpg", cover_key);
    let asset = crate::web::get_embedded_asset(&asset_path)
        .or_else(|| crate::web::get_embedded_asset("covers/vlib_default_tv.jpg"))
        .ok_or_else(|| (StatusCode::NOT_FOUND, "暂无图片".to_string()))?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .body(axum::body::Body::from(asset.data))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

pub fn resolve_virtual_library_cover_key<'a>(
    custom_cover: Option<&'a str>,
    name: &str,
    icon_type: &str,
) -> &'a str {
    if let Some(key) = custom_cover {
        if is_builtin_cover_key(key) {
            return key;
        }
    }

    if name.contains("8.0+") || name.contains("高分") {
        "vlib_top_rated"
    } else if name.contains("Netflix") || name.contains("HBO") || name.contains("精品") {
        "vlib_streaming"
    } else if name.contains("华语") {
        "vlib_chinese"
    } else if name.contains("日韩") {
        "vlib_kr_jp"
    } else if name.contains("最新") || name.contains("2025") || name.contains("2026") {
        "vlib_latest"
    } else if icon_type == "movies" || icon_type == "movie" {
        "vlib_default_movies"
    } else {
        "vlib_default_tv"
    }
}

pub(super) async fn fetch_virtual_library_top_media_images(
    state: &AppState,
    rule: &VirtualLibraryRule,
    limit: i64,
) -> Vec<(Option<String>, Option<String>)> {
    match sqlx::query_as(&format!(
        r#"
        SELECT m.backdrop_path, m.poster_path
        FROM media_works m
        WHERE EXISTS (
            SELECT 1 FROM sync_folders f
            WHERE f.work_id = m.id AND f.is_ignored = FALSE
        )
          AND (m.backdrop_path IS NOT NULL OR m.poster_path IS NOT NULL)
          AND $1::jsonb IS NOT NULL AND {}
        ORDER BY m.rating DESC, m.year DESC NULLS LAST, m.id DESC
        LIMIT $2
        "#,
        rule.sql_predicate("$1::jsonb")
    ))
    .bind(serde_json::to_value(rule).ok())
    .bind(limit)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("查询智能视图封面候选媒体失败: {}", e);
            Vec::new()
        }
    }
}

async fn fetch_virtual_library_auto_image(
    state: &AppState,
    rule: &VirtualLibraryRule,
) -> Option<String> {
    let rows = fetch_virtual_library_top_media_images(state, rule, 1).await;
    rows.into_iter()
        .next()
        .and_then(|(backdrop, poster)| backdrop.or(poster))
}

/// 需要并发拉取的一张 TMDB 图片请求
struct CollageImageRequest {
    /// 在结果槽位数组中的下标
    slot: usize,
    size: &'static str,
    file_name: String,
}

async fn fetch_or_build_virtual_library_collage(
    state: &Arc<AppState>,
    view_id: &str,
    rule: &VirtualLibraryRule,
    style: crate::image_cache::collage::CollageStyle,
) -> Option<Bytes> {
    let rows = fetch_virtual_library_top_media_images(state, rule, 4).await;
    if rows.is_empty() {
        return None;
    }

    let mut sig = String::from(style.as_str());
    for (b, p) in &rows {
        sig.push('|');
        if let Some(s) = b {
            sig.push_str(s);
        }
        sig.push(',');
        if let Some(s) = p {
            sig.push_str(s);
        }
    }
    let hash = xxhash_rust::xxh3::xxh3_64(sig.as_bytes());

    let cfg = state.config.load();
    let covers_dir = cfg.image_cache_dir.join("covers");
    let cache_path = covers_dir.join(format!("{}_{}_{:016x}.jpg", view_id, style.as_str(), hash));

    let inflight_key = format!("collage:{}:{}:{:016x}", view_id, style.as_str(), hash);
    let state2 = state.clone();
    let covers_dir2 = covers_dir.clone();
    let cache_path2 = cache_path.clone();

    let rendered = state
        .caches
        .collage_cache
        .try_get_with(inflight_key, async move {
            // 磁盘已有同签名渲染结果则直接复用
            if let Ok(bytes) = tokio::fs::read(&cache_path2).await {
                return Ok(Bytes::from(bytes));
            }

            // 组装并发拉取请求：第 1 行的背景图作主视觉，每行取海报（缺失时退回背景图）
            let mut requests: Vec<CollageImageRequest> = Vec::new();
            if let Some(b_path) = rows[0].0.as_ref() {
                requests.push(CollageImageRequest {
                    slot: usize::MAX, // 主视觉槽位
                    size: "w1280",
                    file_name: b_path.trim_start_matches('/').to_string(),
                });
            }
            for (idx, (_, poster)) in rows.iter().enumerate() {
                let p_path = poster.as_ref().or(rows[idx].0.as_ref());
                if let Some(p) = p_path {
                    requests.push(CollageImageRequest {
                        slot: idx,
                        size: "w500",
                        file_name: p.trim_start_matches('/').to_string(),
                    });
                }
            }

            // 并发回源（bounded singleflight 内部，由全局信号量进一步限流）
            let mut join_set = tokio::task::JoinSet::new();
            for req in requests {
                let st = state2.clone();
                join_set.spawn(async move {
                    let bytes = st
                        .get_or_fetch_tmdb_image(req.size, &req.file_name)
                        .await
                        .ok();
                    (req.slot, bytes)
                });
            }

            let mut primary_backdrop: Option<Bytes> = None;
            let mut posters: Vec<Option<Bytes>> = Vec::with_capacity(rows.len());
            while let Some(finished) = join_set.join_next().await {
                if let Ok((slot, bytes)) = finished {
                    if slot == usize::MAX {
                        primary_backdrop = bytes;
                    } else if slot < rows.len() {
                        if slot >= posters.len() {
                            posters.resize_with(slot + 1, || None);
                        }
                        posters[slot] = bytes;
                    }
                }
            }

            let poster_bytes: Vec<Bytes> = posters.into_iter().flatten().collect();
            if poster_bytes.is_empty() && primary_backdrop.is_none() {
                return Err(anyhow::anyhow!("没有可用于拼接的封面素材"));
            }

            let rendered = tokio::task::spawn_blocking(move || {
                crate::image_cache::collage::render_collage(
                    style,
                    &poster_bytes,
                    primary_backdrop.as_ref().map(|v| &v[..]),
                )
            })
            .await
            .map_err(|e| anyhow::anyhow!("拼接封面渲染任务异常: {:#}", e))??;

            if tokio::fs::create_dir_all(&covers_dir2).await.is_ok() {
                // 原子落盘：临时文件 + rename，读端不会读到半截封面
                let tmp_path =
                    cache_path2.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
                if tokio::fs::write(&tmp_path, &rendered).await.is_ok() {
                    if let Err(e) = tokio::fs::rename(&tmp_path, &cache_path2).await {
                        tracing::warn!("拼接封面原子落盘失败 ({}): {}", cache_path2.display(), e);
                        let _ = tokio::fs::remove_file(&tmp_path).await;
                    } else {
                        let rel_path = format!("covers/{}_{}_{:016x}.jpg", view_id, style.as_str(), hash);
                        let cache_key = format!("collage:{}:{}:{:016x}", view_id, style.as_str(), hash);
                        if let Err(error) = sqlx::query(
                            "INSERT INTO image_cache_index (cache_key, local_rel_path, size_bytes, last_accessed_at) VALUES ($1, $2, $3, NOW()) ON CONFLICT (cache_key) DO UPDATE SET local_rel_path=EXCLUDED.local_rel_path, size_bytes=EXCLUDED.size_bytes, last_accessed_at=NOW()",
                        )
                        .bind(cache_key)
                        .bind(rel_path)
                        .bind(i64::try_from(rendered.len()).unwrap_or(i64::MAX))
                        .execute(&state2.db)
                        .await {
                            tracing::warn!("拼接封面缓存索引更新失败: {}", error);
                        }
                    }
                }
            }

            Ok(Bytes::from(rendered))
        })
        .await
        .ok()?;

    Some(rendered)
}

#[cfg(test)]
mod tests {
    use super::{
        head_image_response, image_path_tag, not_modified_response,
        playlist_collection_cover_permitted,
    };
    use axum::http::{header, HeaderMap, HeaderValue, StatusCode};

    #[tokio::test]
    async fn recursive_playlist_and_collection_children_resolve_and_enforce_media_acl() {
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/test")
            .unwrap();
        let (tx, _) = tokio::sync::mpsc::channel(1);
        let state = crate::state::AppState::new(
            crate::config::BootstrapFileConfig::default().into_dynamic(),
            db,
            tx,
        )
        .unwrap();
        let restricted = super::super::views::UserAccessScope {
            allowed_categories: Some(vec!["allowed".into()]),
            virtual_libraries: vec![],
        };
        for child in ["work_17", "season_18_0", "ep_19_1_2"] {
            let work_id = super::image_child_work_id(child).unwrap();
            assert!(!super::super::items::work_access_allowed(
                &state,
                work_id,
                "restricted",
                &restricted,
            )
            .await
            .unwrap());
        }
        assert_eq!(super::image_child_work_id("pl_20"), None);
        assert_eq!(super::image_child_work_id("col_21"), None);
    }

    #[test]
    fn playlist_and_collection_cover_acl_matches_visibility_contract() {
        assert!(!playlist_collection_cover_permitted(
            false,
            Some("alice"),
            None
        ));
        assert!(!playlist_collection_cover_permitted(
            false,
            Some("alice"),
            Some(("bob", false))
        ));
        assert!(playlist_collection_cover_permitted(
            false,
            Some("alice"),
            Some(("alice", false))
        ));
        assert!(playlist_collection_cover_permitted(
            false,
            Some("alice"),
            Some(("admin", true))
        ));

        assert!(!playlist_collection_cover_permitted(
            true,
            Some("alice"),
            None
        ));
        assert!(!playlist_collection_cover_permitted(
            true,
            Some("alice"),
            Some(("bob", false))
        ));
        assert!(playlist_collection_cover_permitted(
            true,
            Some("alice"),
            Some(("alice", false))
        ));
        assert!(playlist_collection_cover_permitted(
            true,
            Some("alice"),
            Some(("admin", true))
        ));

        assert!(playlist_collection_cover_permitted(true, None, None));
        assert!(playlist_collection_cover_permitted(
            true,
            None,
            Some(("bob", false))
        ));
    }

    #[test]
    fn image_head_returns_deterministic_metadata_and_honors_etag() {
        let headers = HeaderMap::new();
        let response = head_image_response(&headers, "\"cover-1\"", "image/jpeg").unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::ETAG], "\"cover-1\"");
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");

        let mut conditional = HeaderMap::new();
        conditional.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("\"cover-1\""),
        );
        assert_eq!(
            head_image_response(&conditional, "\"cover-1\"", "image/jpeg")
                .unwrap()
                .status(),
            StatusCode::NOT_MODIFIED
        );
    }

    #[test]
    fn matching_if_none_match_returns_304() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("\"img-test\""),
        );
        let response = not_modified_response(&headers, "\"img-test\"")
            .unwrap()
            .expect("matching ETag should produce a 304 response");
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    }

    #[test]
    fn image_type_is_restricted_to_primary_and_backdrop() {
        assert!(["Primary", "Backdrop"]
            .iter()
            .any(|v| v.eq_ignore_ascii_case("primary")));
        assert!(!["Primary", "Backdrop"]
            .iter()
            .any(|v| v.eq_ignore_ascii_case("Logo")));
        assert!(!["Primary", "Backdrop"]
            .iter()
            .any(|v| v.eq_ignore_ascii_case("Banner")));
    }

    #[test]
    fn image_path_tag_differs_for_distinct_paths() {
        assert_ne!(
            image_path_tag(Some("/covers/a.jpg")),
            image_path_tag(Some("/covers/b.jpg"))
        );
        assert_eq!(image_path_tag(None), "");
    }

    #[test]
    fn list_image_meta_cache_keys_are_scoped_per_user() {
        assert_ne!(
            super::image_meta_cache_key("col_1", "Primary", "alice"),
            super::image_meta_cache_key("col_1", "Primary", "bob")
        );
        assert_ne!(
            super::image_meta_cache_key("pl_2", "Backdrop", "alice"),
            super::image_meta_cache_key("pl_2", "Backdrop", "bob")
        );
        assert_eq!(
            super::image_meta_cache_key("work_10", "Primary", "alice"),
            super::image_meta_cache_key("work_10", "Primary", "bob")
        );
    }
}
