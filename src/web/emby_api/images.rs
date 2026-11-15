use super::views::base_library_category;
use crate::smart_views::VirtualLibraryRule;
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::Response,
};
use std::sync::Arc;

pub(super) async fn get_item_image_indexed(
    state: State<Arc<AppState>>,
    Path((item_id, image_type, _image_index)): Path<(String, String, String)>,
) -> Result<Response, (StatusCode, String)> {
    get_item_image(state, Path((item_id, image_type))).await
}

pub(super) async fn get_item_image(
    State(state): State<Arc<AppState>>,
    Path((item_id, image_type)): Path<(String, String)>,
) -> Result<Response, (StatusCode, String)> {
    if let Some(base_cat) = base_library_category(&item_id) {
        let cfg_key = format!("base_lib_cover:{}", item_id);
        let custom_cover: Option<String> =
            sqlx::query_scalar("SELECT config_value FROM system_configs WHERE config_key = $1")
                .bind(&cfg_key)
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None);

        let clean_custom = custom_cover
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let rule = VirtualLibraryRule {
            categories: vec![base_cat.to_string()],
            ..Default::default()
        };
        if let Some(resp) =
            try_serve_dynamic_cover(&state, &item_id, clean_custom, &rule).await
        {
            return Ok(resp);
        }

        let cover_key = clean_custom
            .filter(|k| is_builtin_cover_key(k))
            .unwrap_or(&item_id);
        return serve_embedded_cover(cover_key);
    }

    if let Some(raw_vid) = item_id.strip_prefix("lib_vlib_") {
        let vid = raw_vid
            .parse::<i32>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的智能视图 ID".to_string()))?;

        let row: Option<(String, String, Option<String>, serde_json::Value)> = sqlx::query_as(
            "SELECT name, icon_type, cover_image, filter_rules FROM virtual_libraries WHERE id = $1",
        )
        .bind(vid)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let Some((vname, icon_type, cover_image, rules_val)) = row else {
            return Err((StatusCode::NOT_FOUND, "智能视图不存在".to_string()));
        };

        let custom_cover = cover_image
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let rule: VirtualLibraryRule = serde_json::from_value(rules_val)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("智能视图规则损坏：{}", e)))?;
        if let Some(resp) =
            try_serve_dynamic_cover(&state, &item_id, custom_cover, &rule).await
        {
            return Ok(resp);
        }

        let cover_key = resolve_virtual_library_cover_key(custom_cover, &vname, &icon_type);
        return serve_embedded_cover(cover_key);
    }

    let (size, raw_file): (&str, Option<String>) = if let Some(rest) = item_id.strip_prefix("ep_") {
        let mut parts = rest.rsplitn(3, '_');
        let ep_num = parts
            .next()
            .and_then(|s| s.parse::<i32>().ok())
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "无效的单集集号".to_string()))?;
        let s_num = parts
            .next()
            .and_then(|s| s.parse::<i32>().ok())
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "无效的单集季号".to_string()))?;
        let wid = parts
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "无效的单集所属作品 ID".to_string()))?;

        let ep_paths: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            r#"
            SELECT e.still_path, w.backdrop_path, w.poster_path
            FROM media_works w
            LEFT JOIN work_episodes e
              ON e.work_id = w.id AND e.season_number = $2 AND e.episode_number = $3
            WHERE w.id = $1
            "#,
        )
        .bind(wid)
        .bind(s_num)
        .bind(ep_num)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let Some((still, backdrop, poster)) = ep_paths else {
            return Err((StatusCode::NOT_FOUND, "单集不存在".to_string()));
        };

        ("w500", still.or(backdrop).or(poster))
    } else if let Some(rest) = item_id.strip_prefix("season_") {
        let mut parts = rest.rsplitn(2, '_');
        let snum = parts
            .next()
            .and_then(|s| s.parse::<i32>().ok())
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "无效的季 ID".to_string()))?;
        let wid = parts
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "无效的季所属剧集 ID".to_string()))?;

        let season_poster: Option<String> = sqlx::query_scalar(
            "SELECT poster_path FROM work_seasons WHERE work_id = $1 AND season_number = $2 AND poster_path IS NOT NULL",
        )
        .bind(wid)
        .bind(snum)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

        let final_poster = match season_poster {
            Some(p) => Some(p),
            None => sqlx::query_scalar("SELECT poster_path FROM media_works WHERE id = $1")
                .bind(wid)
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None)
                .flatten(),
        };

        ("w500", final_poster)
    } else {
        let raw_wid = item_id
            .strip_prefix("work_")
            .or_else(|| item_id.strip_prefix("media_"))
            .unwrap_or(&item_id);
        let wid = raw_wid
            .parse::<i64>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "无效的资源 ID".to_string()))?;

        let paths: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT poster_path, backdrop_path FROM media_works WHERE id = $1",
        )
        .bind(wid)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let Some((poster, backdrop)) = paths else {
            return Err((StatusCode::NOT_FOUND, "条目不存在".to_string()));
        };

        if image_type.eq_ignore_ascii_case("Backdrop") {
            ("w1280", backdrop.or(poster))
        } else {
            ("w500", poster)
        }
    };

    let Some(file_path) = raw_file else {
        return Err((StatusCode::NOT_FOUND, "暂无图片".to_string()));
    };

    let clean_name = file_path.trim_start_matches('/');
    let bytes = state
        .get_or_fetch_tmdb_image(size, clean_name)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("图片回源失败: {:#}", e)))?;

    let content_type = if clean_name.ends_with(".png") {
        "image/png"
    } else {
        "image/jpeg"
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=604800")
        .body(axum::body::Body::from(bytes))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}


pub(super) fn compute_cover_image_tag(cover_image: Option<&str>) -> String {
    match cover_image.map(str::trim).filter(|s| !s.is_empty()) {
        Some(c) => {
            let clean: String = c
                .chars()
                .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                .take(24)
                .collect();
            if clean.is_empty() {
                "v1".to_string()
            } else {
                if c == "auto" || c.starts_with("collage_") {
                    format!("v1_{}_{}", clean, chrono::Utc::now().timestamp() / 300)
                } else { format!("v1_{}", clean) }
            }
        }
        None => "v1".to_string(),
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
    state: &AppState,
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
                return Some(
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, content_type)
                        .header(header::CACHE_CONTROL, "public, max-age=300")
                        .body(axum::body::Body::from(bytes))
                        .unwrap(),
                );
            }
        }
    } else if let Some(style) = custom_cover.and_then(crate::image_cache::collage::CollageStyle::from_key) {
        if let Some(bytes) = fetch_or_build_virtual_library_collage(state, view_id, rule, style).await {
            return Some(
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "image/jpeg")
                    .header(header::CACHE_CONTROL, "public, max-age=300")
                    .body(axum::body::Body::from(bytes))
                    .unwrap(),
            );
        }
    }
    None
}

fn serve_embedded_cover(cover_key: &str) -> Result<Response, (StatusCode, String)> {
    let asset_path = format!("covers/{}.jpg", cover_key);
    let asset = crate::web::get_embedded_asset(&asset_path)
        .or_else(|| crate::web::get_embedded_asset("covers/vlib_default_tv.jpg"))
        .ok_or_else(|| (StatusCode::NOT_FOUND, "封面资源未找到".to_string()))?;

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .body(axum::body::Body::from(asset.data.into_owned()))
        .unwrap())
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
    sqlx::query_as(&format!(
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
    .unwrap_or_default()
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

async fn fetch_or_build_virtual_library_collage(
    state: &AppState,
    view_id: &str,
    rule: &VirtualLibraryRule,
    style: crate::image_cache::collage::CollageStyle,
) -> Option<Vec<u8>> {
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

    if let Ok(bytes) = tokio::fs::read(&cache_path).await {
        return Some(bytes);
    }

    let inflight_key = format!("collage:{}:{}:{:016x}", view_id, style.as_str(), hash);
    let _guard = state
        .caches
        .image_fetch_inflight
        .get_with(inflight_key, async { () })
        .await;

    if let Ok(bytes) = tokio::fs::read(&cache_path).await {
        return Some(bytes);
    }

    let mut posters: Vec<Vec<u8>> = Vec::with_capacity(rows.len());
    let mut primary_backdrop: Option<Vec<u8>> = None;

    for (backdrop, poster) in &rows {
        if primary_backdrop.is_none() {
            if let Some(b_path) = backdrop {
                let clean = b_path.trim_start_matches('/');
                if let Ok(bytes) = state.get_or_fetch_tmdb_image("w1280", clean).await {
                    primary_backdrop = Some(bytes);
                }
            }
        }
        if let Some(p_path) = poster.as_ref().or(backdrop.as_ref()) {
            let clean = p_path.trim_start_matches('/');
            if let Ok(bytes) = state.get_or_fetch_tmdb_image("w500", clean).await {
                posters.push(bytes);
            }
        }
    }

    if posters.is_empty() && primary_backdrop.is_none() {
        return None;
    }

    let rendered = tokio::task::spawn_blocking(move || {
        crate::image_cache::collage::render_collage(style, &posters, primary_backdrop.as_deref())
    })
    .await
    .ok()?
    .ok()?;

    if tokio::fs::create_dir_all(&covers_dir).await.is_ok() {
        let _ = tokio::fs::write(&cache_path, &rendered).await;
    }

    Some(rendered)
}

