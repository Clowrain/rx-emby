use super::auth::resolve_valid_user_id_with_query;
use super::images::compute_cover_image_tag;
use super::playback::PlaybackQuery;
use super::query::EmbyQuery as Query;
use crate::smart_views::{can_access_view, VirtualLibraryRule};
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Debug)]
pub(crate) struct AccessibleVirtualLibrary {
    pub filter_rules: Value,
}

#[derive(Clone, Debug)]
pub(crate) struct UserAccessScope {
    pub allowed_categories: Option<Vec<String>>,
    pub virtual_libraries: Vec<AccessibleVirtualLibrary>,
}

impl UserAccessScope {
    pub(crate) fn has_no_access(&self) -> bool {
        matches!(&self.allowed_categories, Some(categories) if categories.is_empty())
            && self.virtual_libraries.is_empty()
    }
}

pub(crate) async fn user_access_scope(
    state: &AppState,
    user_id: &str,
) -> Result<Arc<UserAccessScope>, (axum::http::StatusCode, String)> {
    let allowed_categories = get_user_allowed_categories(state, user_id).await?;
    let rows = state
        .caches
        .enabled_vlibs_cache
        .try_get_with("enabled".to_string(), async {
            sqlx::query_as::<_, (i32, String, Value)>(
                "SELECT id, name, filter_rules FROM virtual_libraries WHERE is_enabled = TRUE",
            )
            .fetch_all(&state.db)
            .await
        })
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let virtual_libraries = rows
        .into_iter()
        .filter_map(|(id, name, filter_rules)| {
            can_access_view(
                allowed_categories.as_deref(),
                &format!("lib_vlib_{id}"),
                &name,
            )
            .then_some(AccessibleVirtualLibrary { filter_rules })
        })
        .collect();
    Ok(Arc::new(UserAccessScope {
        allowed_categories,
        virtual_libraries,
    }))
}

pub(super) async fn get_user_views(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path_user_id): Path<String>,
    Query(q): Query<PlaybackQuery>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let cfg = state.config.load();
    let user_id = resolve_valid_user_id_with_query(
        &state,
        Some(&path_user_id),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;

    let allowed_cats = get_user_allowed_categories(&state, &user_id).await?;

    let base_libs = vec![
        ("lib_movie", "电影", "movies"),
        ("lib_tv", "电视剧", "tvshows"),
        ("lib_anime", "动漫", "tvshows"),
        ("lib_doc", "纪录片", "movies"),
        ("lib_variety", "综艺", "tvshows"),
    ];

    let base_cover_rows: Vec<(String, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT config_key, config_value, updated_at FROM system_configs WHERE config_key LIKE 'base_lib_cover:%'",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        tracing::warn!("查询基础库封面配置失败: {}", e);
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;
    let base_cover_map: HashMap<String, (String, i64)> = base_cover_rows
        .into_iter()
        .filter_map(|(k, v, updated_at)| {
            k.strip_prefix("base_lib_cover:")
                .map(|id| (id.to_string(), (v, updated_at.timestamp())))
        })
        .collect();

    let mut items: Vec<Value> = base_libs
        .into_iter()
        .filter(|(id, name, _)| match &allowed_cats {
            None => true,
            Some(cats) => cats.iter().any(|c| c == "*" || c == name || c == id),
        })
        .map(|(id, name, ctype)| {
            let (cover, revision) = base_cover_map
                .get(id)
                .map(|(v, r)| (Some(v.as_str()), *r))
                .unwrap_or((None, 1));
            let tag = compute_cover_image_tag(cover, revision);
            json!({
                "Id": id,
                "Name": name,
                "ServerId": cfg.server_id,
                "Type": "CollectionFolder",
                "CollectionType": ctype,
                "IsFolder": true,
                "ImageTags": { "Primary": tag }
            })
        })
        .collect();

    // 追加已启用的自定义虚拟媒体库 (Smart Views)
    let vlibs: Vec<(i32, String, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT id, name, icon_type, cover_image, cover_revision FROM virtual_libraries WHERE is_enabled = TRUE ORDER BY sort_order ASC, id ASC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        tracing::warn!("查询虚拟媒体库列表失败: {}", e);
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

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
        let image_tag = compute_cover_image_tag(cover_image.as_deref(), revision);
        // 视图仅投放 Primary：图片接口对视图的 Backdrop 与 Primary 返回同一资源，
        // 同时投放会诱发客户端冗余拉取
        items.push(json!({
            "Id": view_id,
            "Name": vname,
            "ServerId": cfg.server_id,
            "Type": "CollectionFolder",
            "CollectionType": ctype,
            "IsFolder": true,
            "ImageTags": { "Primary": image_tag }
        }));
    }

    Ok(Json(json!({
        "Items": items,
        "TotalRecordCount": items.len(),
        "StartIndex": 0
    })))
}

/// 获取用户被允许访问的分类集合：`None` 代表不限权限（管理员或包含 `*`），`Some(vec)` 代表受限分类列表
pub(super) async fn get_user_allowed_categories(
    state: &AppState,
    user_id: &str,
) -> Result<Option<Vec<String>>, (axum::http::StatusCode, String)> {
    state
        .caches
        .user_categories_cache
        .try_get_with(user_id.to_owned(), async {
            let row: Option<(bool, Vec<String>)> =
                sqlx::query_as("SELECT is_admin, allowed_categories FROM users WHERE id = $1")
                    .bind(user_id)
                    .fetch_optional(&state.db)
                    .await?;
            let (is_admin, categories) = row.ok_or(sqlx::Error::RowNotFound)?;
            Ok::<Option<Vec<String>>, sqlx::Error>(
                if is_admin || categories.iter().any(|c| c == "*") {
                    None
                } else {
                    Some(normalize_allowed_categories(categories))
                },
            )
        })
        .await
        .map_err(|error| {
            if matches!(error.as_ref(), sqlx::Error::RowNotFound) {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    "用户不存在".to_string(),
                )
            } else {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    error.to_string(),
                )
            }
        })
}

fn normalize_allowed_categories(categories: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::with_capacity(categories.len());
    for category in categories {
        let value = match category.as_str() {
            "lib_movie" => "电影",
            "lib_tv" => "电视剧",
            "lib_anime" => "动漫",
            "lib_doc" => "纪录片",
            "lib_variety" => "综艺",
            _ => category.as_str(),
        };
        if !normalized.iter().any(|existing| existing == value) {
            normalized.push(value.to_owned());
        }
    }
    normalized
}

/// 统计某个虚拟媒体库规则当前在库内命中的影视作品数
pub async fn count_virtual_library_matches(
    state: &AppState,
    rule: &VirtualLibraryRule,
) -> Result<i64, sqlx::Error> {
    let sql = format!(
        "SELECT COUNT(*) FROM media_works m WHERE EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE) AND $1::jsonb IS NOT NULL AND {}",
        rule.sql_predicate("$1::jsonb")
    );
    let rule_json = serde_json::to_value(rule).map_err(|e| sqlx::Error::Configuration(e.into()))?;
    sqlx::query_scalar(&sql)
        .bind(rule_json)
        .fetch_one(&state.db)
        .await
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

#[cfg(test)]
mod access_scope_tests {
    use super::{normalize_allowed_categories, AccessibleVirtualLibrary, UserAccessScope};

    #[test]
    fn category_ids_and_display_names_normalize_to_same_acl_value() {
        assert_eq!(
            normalize_allowed_categories(vec!["lib_movie".into(), "电影".into()]),
            vec!["电影"]
        );
    }

    #[test]
    fn empty_restricted_categories_and_no_libraries_means_no_access() {
        assert!(UserAccessScope {
            allowed_categories: Some(vec![]),
            virtual_libraries: vec![],
        }
        .has_no_access());
        assert!(!UserAccessScope {
            allowed_categories: None,
            virtual_libraries: vec![],
        }
        .has_no_access());
        assert!(!UserAccessScope {
            allowed_categories: Some(vec![]),
            virtual_libraries: vec![AccessibleVirtualLibrary {
                filter_rules: serde_json::json!({"media_type":"movie"}),
            }],
        }
        .has_no_access());
    }
}
