use super::auth::resolve_valid_user_id;
use super::images::compute_cover_image_tag;
use crate::smart_views::{can_access_view, VirtualLibraryRule};
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};

pub(super) async fn get_user_views(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path_user_id): Path<String>,
) -> Json<Value> {
    let cfg = state.config.load();
    let user_id = resolve_valid_user_id(&state, Some(&path_user_id), &headers)
        .await
        .unwrap_or(path_user_id);

    let allowed_cats = get_user_allowed_categories(&state, &user_id).await;

    let base_libs = vec![
        ("lib_movie", "电影", "movies"),
        ("lib_tv", "电视剧", "tvshows"),
        ("lib_anime", "动漫", "tvshows"),
        ("lib_doc", "纪录片", "movies"),
        ("lib_variety", "综艺", "tvshows"),
    ];

    let base_cover_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT config_key, config_value FROM system_configs WHERE config_key LIKE 'base_lib_cover:%'",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let base_cover_map: HashMap<String, String> = base_cover_rows
        .into_iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("base_lib_cover:")
                .map(|id| (id.to_string(), v))
        })
        .collect();

    let mut items: Vec<Value> = base_libs
        .into_iter()
        .filter(|(id, name, _)| {
            match &allowed_cats {
                None => true,
                Some(cats) => cats.iter().any(|c| c == "*" || c == name || c == id),
            }
        })
        .map(|(id, name, ctype)| {
            let tag = compute_cover_image_tag(base_cover_map.get(id).map(|s| s.as_str()));
            json!({
                "Id": id,
                "Name": name,
                "ServerId": cfg.server_id,
                "Type": "CollectionFolder",
                "CollectionType": ctype,
                "IsFolder": true,
                "ImageTags": { "Primary": tag },
                "BackdropImageTags": [tag]
            })
        })
        .collect();

    // 追加已启用的自定义虚拟媒体库 (Smart Views)
    let vlibs: Vec<(i32, String, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT id, name, icon_type, cover_image, cover_revision FROM virtual_libraries WHERE is_enabled = TRUE ORDER BY sort_order ASC, id ASC",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    for (vid, vname, icon_type, cover_image, revision) in vlibs {
        let view_id = format!("lib_vlib_{}", vid);
        let allowed = can_access_view(allowed_cats.as_deref(), &view_id, &vname);
        if !allowed {
            continue;
        }
        let ctype = if icon_type == "movies" || icon_type == "movie" {
            "movies"
        } else {
            "tvshows"
        };
        let image_tag = format!("{}_{}", compute_cover_image_tag(cover_image.as_deref()), revision);
        items.push(json!({
            "Id": view_id,
            "Name": vname,
            "ServerId": cfg.server_id,
            "Type": "CollectionFolder",
            "CollectionType": ctype,
            "IsFolder": true,
            "ImageTags": { "Primary": image_tag },
            "BackdropImageTags": [image_tag]
        }));
    }

    Json(json!({
        "Items": items,
        "TotalRecordCount": items.len()
    }))
}

/// 获取用户被允许访问的分类集合：`None` 代表不限权限（管理员或包含 `*`），`Some(vec)` 代表受限分类列表
pub(super) async fn get_user_allowed_categories(state: &AppState, user_id: &str) -> Option<Vec<String>> {
    let row: Option<(bool, Vec<String>)> =
        sqlx::query_as("SELECT is_admin, allowed_categories FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);

    match row {
        Some((is_admin, cats)) => {
            if is_admin || cats.is_empty() || cats.iter().any(|c| c == "*") {
                None
            } else {
                Some(cats)
            }
        }
        None => None,
    }
}


/// 统计某个虚拟媒体库规则当前在库内命中的影视作品数
pub async fn count_virtual_library_matches(state: &AppState, rule: &VirtualLibraryRule) -> Result<i64, sqlx::Error> {
    let sql = format!(
        "SELECT COUNT(*) FROM media_works m WHERE EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE) AND $1::jsonb IS NOT NULL AND {}",
        rule.sql_predicate("$1::jsonb")
    );
    sqlx::query_scalar(&sql)
        .bind(serde_json::to_value(rule).expect("已校验的规则"))
        .fetch_one(&state.db).await
}

pub(super) fn base_library_category(item_id: &str) -> Option<&'static str> {
    match item_id {
        "lib_movie" => Some("电影"),
        "lib_tv" => Some("电视剧"),
        "lib_anime" => Some("动漫"),
        "lib_doc" => Some("纪录片"),
        "lib_variety" => Some("综艺"),
        _ => None,
    }
}

