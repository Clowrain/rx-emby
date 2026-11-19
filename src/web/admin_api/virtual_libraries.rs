use super::smart_view_bundle::{
    build_smart_view_export_zip, dynamic_cover_label, is_custom_cover_key,
    parse_and_validate_import_bundle, ExportableView,
};
use crate::{
    state::AppState,
    web::emby_api::{
        count_virtual_library_matches, is_builtin_cover_key, resolve_virtual_library_cover_key,
        VirtualLibraryRule,
    },
};
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::Response,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashSet, path::Path as FsPath, sync::Arc};

type EnabledVlibsCache = moka::future::Cache<String, Vec<(i32, String, Value)>>;

async fn load_smart_view_facet(db: &sqlx::PgPool, expression: &str) -> Result<Value, String> {
    let sql = format!(
        "SELECT DISTINCT (v->>'id')::int AS id, v->>'name' AS name FROM media_works m CROSS JOIN LATERAL jsonb_array_elements(COALESCE({},'[]')) v WHERE EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE) AND m.metadata_version >= 2 AND v->>'id' IS NOT NULL ORDER BY name,id LIMIT 500",
        expression
    );
    let rows: Vec<(i32, String)> = sqlx::query_as(&sql)
        .fetch_all(db)
        .await
        .map_err(|error| error.to_string())?;
    Ok(json!(rows
        .into_iter()
        .map(|(id, name)| json!({"id": id, "name": name}))
        .collect::<Vec<_>>()))
}

async fn load_smart_view_metadata_stats(db: &sqlx::PgPool) -> Result<Value, String> {
    let stats: (i64, i64, i64, i64) = sqlx::query_as("SELECT COUNT(*), COUNT(*) FILTER(WHERE m.metadata_version>=2), COUNT(*) FILTER(WHERE m.metadata_refresh_requested), COUNT(*) FILTER(WHERE m.metadata_refresh_requested AND m.metadata_retry_count>=3) FROM media_works m WHERE EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE)")
        .fetch_one(db)
        .await
        .map_err(|error| error.to_string())?;
    Ok(json!({"total": stats.0, "enriched": stats.1, "queued": stats.2, "failed": stats.3}))
}

fn invalidate_enabled_vlibs_cache(cache: &EnabledVlibsCache) {
    cache.invalidate_all();
}

async fn invalidate_vlib_image_meta(state: &AppState, id: i32) {
    for key in [
        format!("lib_vlib_{id}:Primary"),
        format!("lib_vlib_{id}:Backdrop"),
    ] {
        state.caches.image_meta_cache.invalidate(&key).await;
    }
    state.caches.collage_cache.invalidate_all();
}

async fn invalidate_base_library_image_meta(state: &AppState, id: &str) {
    for key in [format!("{id}:Primary"), format!("{id}:Backdrop")] {
        state.caches.image_meta_cache.invalidate(&key).await;
    }
    state.caches.collage_cache.invalidate_all();
}

#[derive(Serialize, sqlx::FromRow)]
struct VirtualLibDbRow {
    id: i32,
    name: String,
    icon_type: String,
    sort_order: i32,
    filter_rules: Value,
    is_enabled: bool,
    cover_image: Option<String>,
}

pub(super) async fn list_virtual_libraries(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let rows: Vec<VirtualLibDbRow> = sqlx::query_as(
        "SELECT id, name, icon_type, sort_order, filter_rules, is_enabled, cover_image FROM virtual_libraries ORDER BY sort_order ASC, id ASC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut tasks = tokio::task::JoinSet::new();
    for (index, r) in rows.into_iter().enumerate() {
        let state = state.clone();
        tasks.spawn(async move {
            let rule: VirtualLibraryRule =
                serde_json::from_value(r.filter_rules.clone()).map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("视图 {} 规则损坏：{}", r.id, e),
                    )
                })?;
            let matched_count = count_virtual_library_matches(&state, &rule)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let resolved_cover =
                resolve_virtual_library_cover_key(r.cover_image.as_deref(), &r.name, &r.icon_type);
            Ok::<(usize, Value), (StatusCode, String)>((
                index,
                json!({
                    "id": r.id, "view_id": format!("lib_vlib_{}", r.id), "name": r.name,
                    "icon_type": r.icon_type, "sort_order": r.sort_order, "filter_rules": rule,
                    "is_enabled": r.is_enabled, "cover_image": r.cover_image,
                    "resolved_cover": resolved_cover, "matched_count": matched_count
                }),
            ))
        });
    }
    let mut ordered = Vec::new();
    while let Some(result) = tasks.join_next().await {
        let (index, library) =
            result.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
        ordered.push((index, library));
    }
    ordered.sort_by_key(|(index, _)| *index);
    let libraries: Vec<Value> = ordered.into_iter().map(|(_, library)| library).collect();
    Ok(Json(json!({ "libraries": libraries })))
}

#[derive(Deserialize)]
pub(super) struct SaveVirtualLibReq {
    id: Option<i32>,
    name: String,
    icon_type: String,
    #[serde(default)]
    sort_order: i32,
    filter_rules: VirtualLibraryRule,
    #[serde(default = "default_true")]
    is_enabled: bool,
    #[serde(default)]
    cover_image: Option<String>,
}

fn default_true() -> bool {
    true
}

pub(super) async fn save_virtual_library(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SaveVirtualLibReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let name = payload.name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err((
            StatusCode::BAD_REQUEST,
            "智能视图名称须为 1～64 个字符".to_string(),
        ));
    }
    if !["movies", "tvshows"].contains(&payload.icon_type.as_str()) {
        return Err((StatusCode::BAD_REQUEST, "请选择电影或剧集展示样式".into()));
    }
    payload
        .filter_rules
        .validate()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let rules_json = serde_json::to_value(&payload.filter_rules)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let cover_image = payload
        .cover_image
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let custom_covers_dir = state.config.load().image_cache_dir.join("custom_covers");
    validate_cover_image_value(cover_image.as_deref(), &custom_covers_dir)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let vlib_id = if let Some(vid) = payload.id {
        let result = sqlx::query(
            r#"
            UPDATE virtual_libraries
            SET name = $1, icon_type = $2, sort_order = $3, filter_rules = $4, is_enabled = $5, cover_image = $6, cover_revision = cover_revision + 1
            WHERE id = $7
            "#,
        )
        .bind(name)
        .bind(&payload.icon_type)
        .bind(payload.sort_order)
        .bind(&rules_json)
        .bind(payload.is_enabled)
        .bind(&cover_image)
        .bind(vid)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if result.rows_affected() == 0 {
            return Err((StatusCode::NOT_FOUND, "智能视图已被删除，请刷新列表".into()));
        }
        vid
    } else {
        sqlx::query_scalar(
            r#"
            INSERT INTO virtual_libraries (name, icon_type, sort_order, filter_rules, is_enabled, cover_image)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id
            "#,
        )
        .bind(name)
        .bind(&payload.icon_type)
        .bind(payload.sort_order)
        .bind(&rules_json)
        .bind(payload.is_enabled)
        .bind(&cover_image)
        .fetch_one(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };

    cleanup_orphan_custom_covers(&state.db, &custom_covers_dir).await;
    invalidate_enabled_vlibs_cache(&state.caches.enabled_vlibs_cache);
    state.caches.admin_stats_cache.invalidate_all();
    invalidate_vlib_image_meta(&state, vlib_id).await;

    Ok(Json(
        json!({ "status": "ok", "message": "智能视图已保存并立即生效" }),
    ))
}

fn validate_cover_image_value(
    cover: Option<&str>,
    custom_covers_dir: &FsPath,
) -> Result<(), String> {
    let Some(c) = cover else {
        return Ok(());
    };
    if is_builtin_cover_key(c) || dynamic_cover_label(c).is_some() {
        return Ok(());
    }
    if is_custom_cover_key(c) && custom_covers_dir.join(format!("{}.jpg", c)).is_file() {
        return Ok(());
    }
    Err(format!("不支持的客户端封面标识：{}", c))
}

async fn cleanup_unreferenced_custom_cover_files(
    custom_covers_dir: &FsPath,
    referenced_keys: &HashSet<String>,
) {
    let Ok(mut entries) = tokio::fs::read_dir(custom_covers_dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Some(fname) = entry.file_name().to_str() {
            if let Some(stem) = fname.strip_suffix(".jpg") {
                if is_custom_cover_key(stem) && !referenced_keys.contains(stem) {
                    let _ = tokio::fs::remove_file(entry.path()).await;
                }
            }
        }
    }
}

async fn cleanup_orphan_custom_covers(db: &sqlx::PgPool, custom_covers_dir: &FsPath) {
    let Ok(rows) = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT cover_image FROM virtual_libraries WHERE cover_image LIKE 'custom_%'",
    )
    .fetch_all(db)
    .await
    else {
        return;
    };
    let active: HashSet<String> = rows.into_iter().collect();
    cleanup_unreferenced_custom_cover_files(custom_covers_dir, &active).await;
}

pub(super) async fn export_virtual_libraries(
    State(state): State<Arc<AppState>>,
) -> Result<Response, (StatusCode, String)> {
    let rows: Vec<VirtualLibDbRow> = sqlx::query_as(
        "SELECT id, name, icon_type, sort_order, filter_rules, is_enabled, cover_image FROM virtual_libraries ORDER BY sort_order ASC, id ASC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut views = Vec::with_capacity(rows.len());
    for r in rows {
        let rule: VirtualLibraryRule = serde_json::from_value(r.filter_rules).unwrap_or_default();
        views.push(ExportableView {
            name: r.name,
            icon_type: r.icon_type,
            sort_order: r.sort_order,
            is_enabled: r.is_enabled,
            cover_image: r.cover_image,
            filter_rules: rule,
        });
    }

    let Json(facets) = smart_view_options(State(state.clone())).await?;
    let custom_covers_dir = state.config.load().image_cache_dir.join("custom_covers");
    let zip_bytes = build_smart_view_export_zip(&views, &facets, &custom_covers_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"rx-emby-smart-views.zip\"",
        )
        .body(axum::body::Body::from(zip_bytes))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

pub(super) async fn preview_import_virtual_libraries(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, String)> {
    let custom_covers_dir = state.config.load().image_cache_dir.join("custom_covers");
    let parsed = tokio::task::spawn_blocking(move || {
        parse_and_validate_import_bundle(&body, &custom_covers_dir)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let existing_names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM virtual_libraries ORDER BY sort_order ASC, id ASC")
            .fetch_all(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let existing_set: HashSet<String> = existing_names.into_iter().collect();

    let mut create_count = 0usize;
    let mut update_count = 0usize;
    let mut custom_cover_count = 0usize;
    let mut imported_names = HashSet::with_capacity(parsed.views.len());
    let mut items = Vec::with_capacity(parsed.views.len());

    for v in parsed.views {
        imported_names.insert(v.name.clone());
        let action = if existing_set.contains(&v.name) {
            update_count += 1;
            "update"
        } else {
            create_count += 1;
            "create"
        };
        if v.cover_type == "custom" {
            custom_cover_count += 1;
        }
        items.push(json!({
            "name": v.name,
            "action": action,
            "icon_type": v.icon_type,
            "sort_order": v.sort_order,
            "is_enabled": v.is_enabled,
            "cover_image": v.cover_image,
            "cover_type": v.cover_type,
            "cover_label": v.cover_label,
            "filter_rules": v.filter_rules,
        }));
    }

    let unmatched_existing_count = existing_set.difference(&imported_names).count();

    Ok(Json(json!({
        "total_count": items.len(),
        "create_count": create_count,
        "update_count": update_count,
        "custom_cover_count": custom_cover_count,
        "unmatched_existing_count": unmatched_existing_count,
        "items": items,
    })))
}

#[derive(Deserialize, Default)]
pub(super) struct ApplyImportQuery {
    #[serde(default)]
    prune_missing: bool,
}

pub(super) async fn apply_import_virtual_libraries(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ApplyImportQuery>,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, String)> {
    let custom_covers_dir = state.config.load().image_cache_dir.join("custom_covers");
    let custom_covers_dir_clone = custom_covers_dir.clone();
    let parsed = tokio::task::spawn_blocking(move || {
        parse_and_validate_import_bundle(&body, &custom_covers_dir_clone)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    if !parsed.custom_covers.is_empty() {
        tokio::fs::create_dir_all(&custom_covers_dir)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("创建自定义封面目录失败：{}", e),
                )
            })?;
        for (key, jpg_bytes) in &parsed.custom_covers {
            let final_path = custom_covers_dir.join(format!("{}.jpg", key));
            let tmp_path = custom_covers_dir.join(format!(
                "{}.tmp-{}",
                key,
                uuid::Uuid::new_v4().simple()
            ));
            tokio::fs::write(&tmp_path, jpg_bytes).await.map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("写入自定义封面失败：{}", e),
                )
            })?;
            tokio::fs::rename(&tmp_path, &final_path)
                .await
                .map_err(|e| {
                    let _ = std::fs::remove_file(&tmp_path);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("保存自定义封面失败：{}", e),
                    )
                })?;
        }
    }

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    sqlx::query("SELECT pg_advisory_xact_lock(728314)")
        .execute(&mut *tx)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let existing_rows: Vec<(i32, String)> =
        sqlx::query_as("SELECT id, name FROM virtual_libraries ORDER BY id ASC")
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut existing_by_name = std::collections::HashMap::with_capacity(existing_rows.len());
    for (id, name) in existing_rows {
        existing_by_name.entry(name).or_insert(id);
    }

    let mut created = 0usize;
    let mut updated = 0usize;
    let mut touched_ids = Vec::new();
    let mut imported_names = HashSet::with_capacity(parsed.views.len());

    for v in parsed.views {
        imported_names.insert(v.name.clone());
        let rules_json = serde_json::to_value(&v.filter_rules)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if let Some(&existing_id) = existing_by_name.get(&v.name) {
            sqlx::query(
                r#"
                UPDATE virtual_libraries
                SET icon_type = $1, sort_order = $2, filter_rules = $3, is_enabled = $4, cover_image = $5, cover_revision = cover_revision + 1
                WHERE id = $6
                "#,
            )
            .bind(&v.icon_type)
            .bind(v.sort_order)
            .bind(&rules_json)
            .bind(v.is_enabled)
            .bind(&v.cover_image)
            .bind(existing_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            updated += 1;
            touched_ids.push(existing_id);
        } else {
            let new_id: i32 = sqlx::query_scalar(
                r#"
                INSERT INTO virtual_libraries (name, icon_type, sort_order, filter_rules, is_enabled, cover_image)
                VALUES ($1, $2, $3, $4, $5, $6)
                RETURNING id
                "#,
            )
            .bind(&v.name)
            .bind(&v.icon_type)
            .bind(v.sort_order)
            .bind(&rules_json)
            .bind(v.is_enabled)
            .bind(&v.cover_image)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            created += 1;
            touched_ids.push(new_id);
        }
    }

    let mut deleted = 0usize;
    if query.prune_missing {
        for (old_name, old_id) in &existing_by_name {
            if !imported_names.contains(old_name) {
                sqlx::query("DELETE FROM virtual_libraries WHERE id = $1")
                    .bind(*old_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                let grant_key = format!("lib_vlib_{}", old_id);
                sqlx::query(
                    "UPDATE users SET allowed_categories = array_remove(allowed_categories, $1) WHERE $1 = ANY(allowed_categories)",
                )
                .bind(&grant_key)
                .execute(&mut *tx)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                deleted += 1;
                touched_ids.push(*old_id);
            }
        }
    }

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if deleted > 0 {
        state.caches.user_categories_cache.invalidate_all();
    }
    cleanup_orphan_custom_covers(&state.db, &custom_covers_dir).await;
    invalidate_enabled_vlibs_cache(&state.caches.enabled_vlibs_cache);
    state.caches.admin_stats_cache.invalidate_all();
    for id in touched_ids {
        invalidate_vlib_image_meta(&state, id).await;
    }

    let msg = if deleted > 0 {
        format!(
            "已应用视图配置：新增 {} 项，更新 {} 项，移除旧视图 {} 项",
            created, updated, deleted
        )
    } else {
        format!("已应用视图配置：新增 {} 项，更新 {} 项", created, updated)
    };

    Ok(Json(json!({
        "status": "ok",
        "created": created,
        "updated": updated,
        "deleted": deleted,
        "message": msg,
    })))
}

pub(super) async fn delete_virtual_library(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    sqlx::query("DELETE FROM virtual_libraries WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let grant_key = format!("lib_vlib_{}", id);
    sqlx::query(
        r#"
        UPDATE users
        SET allowed_categories = array_remove(allowed_categories, $1)
        WHERE $1 = ANY(allowed_categories)
        "#,
    )
    .bind(&grant_key)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    state.caches.user_categories_cache.invalidate_all();

    let cfg = state.config.load();
    let covers_dir = cfg.image_cache_dir.join("covers");
    let custom_covers_dir = cfg.image_cache_dir.join("custom_covers");
    let prefix = format!("vlib_{}", id);
    if let Ok(mut entries) = tokio::fs::read_dir(&covers_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Some(fname) = entry.file_name().to_str() {
                if fname.starts_with(&format!("{}_", prefix)) {
                    let _ = tokio::fs::remove_file(entry.path()).await;
                }
            }
        }
    }

    cleanup_orphan_custom_covers(&state.db, &custom_covers_dir).await;
    invalidate_enabled_vlibs_cache(&state.caches.enabled_vlibs_cache);
    state.caches.admin_stats_cache.invalidate_all();
    invalidate_vlib_image_meta(&state, id).await;

    Ok(Json(json!({ "status": "ok", "message": "智能视图已删除" })))
}

pub(super) async fn list_base_library_covers(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT config_key, config_value FROM system_configs WHERE config_key LIKE 'base_lib_cover:%'",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let cover_map: std::collections::HashMap<String, String> = rows
        .into_iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("base_lib_cover:")
                .map(|id| (id.to_string(), v))
        })
        .collect();

    let base_libs = [
        ("lib_movie", "电影"),
        ("lib_tv", "电视剧"),
        ("lib_anime", "动漫"),
        ("lib_doc", "纪录片"),
        ("lib_variety", "综艺"),
    ];

    let libraries: Vec<Value> = base_libs
        .into_iter()
        .map(|(id, name)| {
            let cover_image = cover_map.get(id).cloned().filter(|s| !s.trim().is_empty());
            json!({
                "id": id,
                "name": name,
                "cover_image": cover_image
            })
        })
        .collect();

    Ok(Json(json!({ "libraries": libraries })))
}

#[derive(Deserialize)]
pub(super) struct SaveBaseLibCoverReq {
    id: String,
    #[serde(default)]
    cover_image: Option<String>,
}

pub(super) async fn save_base_library_cover(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SaveBaseLibCoverReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if !matches!(
        payload.id.as_str(),
        "lib_movie" | "lib_tv" | "lib_anime" | "lib_doc" | "lib_variety"
    ) {
        return Err((StatusCode::BAD_REQUEST, "无效的基础媒体库 ID".to_string()));
    }

    let cfg_key = format!("base_lib_cover:{}", payload.id);
    let clean_cover = payload
        .cover_image
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if let Some(cover) = clean_cover {
        sqlx::query(
            r#"
            INSERT INTO system_configs (config_key, config_value, value_type, description, updated_at)
            VALUES ($1, $2, 'string', '基础媒体库封面配置', NOW())
            ON CONFLICT (config_key) DO UPDATE SET
                config_value = EXCLUDED.config_value,
                updated_at = NOW()
            "#,
        )
        .bind(&cfg_key)
        .bind(cover)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    } else {
        sqlx::query("DELETE FROM system_configs WHERE config_key = $1")
            .bind(&cfg_key)
            .execute(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    invalidate_base_library_image_meta(&state, &payload.id).await;

    Ok(Json(
        json!({ "status": "ok", "message": "基础媒体库封面已更新" }),
    ))
}

pub(super) async fn preview_smart_view(
    State(state): State<Arc<AppState>>,
    Json(rule): Json<VirtualLibraryRule>,
) -> Result<Json<Value>, (StatusCode, String)> {
    rule.validate().map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let count = count_virtual_library_matches(&state, &rule)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({"matched_count": count})))
}

pub(super) async fn smart_view_options(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let db = state.db.clone();
    let result = state
        .caches
        .admin_stats_cache
        .try_get_with("smart_view_options".to_string(), async move {
            let (networks, companies, collections, keywords, directors, cast, watch_providers, metadata) =
                tokio::try_join!(
                    load_smart_view_facet(&db, "m.tmdb_metadata->'networks'"),
                    load_smart_view_facet(&db, "m.tmdb_metadata->'production_companies'"),
                    load_smart_view_facet(&db, "CASE WHEN m.tmdb_metadata->'collection'->>'id' IS NOT NULL THEN jsonb_build_array(m.tmdb_metadata->'collection') ELSE '[]'::jsonb END"),
                    load_smart_view_facet(&db, "m.tmdb_metadata->'keyword_items'"),
                    load_smart_view_facet(&db, "m.tmdb_metadata->'directors'"),
                    load_smart_view_facet(&db, "m.tmdb_metadata->'cast'"),
                    load_smart_view_facet(&db, "m.tmdb_metadata->'watch_providers'"),
                    load_smart_view_metadata_stats(&db),
                )?;
            Ok::<_, String>(json!({
                "networks": networks,
                "companies": companies,
                "collections": collections,
                "keywords": keywords,
                "directors": directors,
                "cast": cast,
                "watch_providers": watch_providers,
                "metadata": metadata
            }))
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(result))
}

pub(super) async fn queue_metadata_backfill(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let cfg = state.config.load();
    if cfg.tmdb_api_key.trim().is_empty() || cfg.tmdb_api_key == "your_tmdb_api_key" {
        return Err((
            StatusCode::BAD_REQUEST,
            "请先在系统设置中配置 TMDB API Key".into(),
        ));
    }
    let result = sqlx::query("UPDATE media_works SET metadata_refresh_requested=TRUE, metadata_retry_count=0, metadata_retry_at=NULL WHERE NOT is_locked AND tmdb_id IS NOT NULL AND (metadata_version<2 OR metadata_retry_count>0)")
        .execute(&state.db).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
    if result.rows_affected() > 0 {
        state
            .scraper
            .runtime
            .start_batch("深度元数据补齐", result.rows_affected() as usize);
    }
    invalidate_enabled_vlibs_cache(&state.caches.enabled_vlibs_cache);
    state.caches.admin_stats_cache.invalidate_all();
    state.caches.collage_cache.invalidate_all();
    Ok(Json(
        json!({"queued":result.rows_affected(),"message":format!("已将 {} 部作品加入深度元数据补齐队列",result.rows_affected())}),
    ))
}

/// 手动使全部动态封面缓存（1天TTL）立即失效，用于后台调整后立刻预览最新封面
pub(super) async fn refresh_dynamic_covers(
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    state.caches.collage_cache.invalidate_all();
    state.caches.image_meta_cache.invalidate_all();
    Json(json!({
        "status": "ok",
        "message": "已刷新动态封面缓存，将按最新发布时间重新检查并生成预览"
    }))
}

#[cfg(test)]
mod cache_tests {
    use super::{
        cleanup_unreferenced_custom_cover_files, invalidate_enabled_vlibs_cache,
        validate_cover_image_value,
    };
    use moka::future::Cache;
    use std::collections::HashSet;

    #[tokio::test]
    async fn virtual_library_mutations_invalidate_enabled_library_cache() {
        let cache: Cache<String, Vec<(i32, String, serde_json::Value)>> = Cache::new(1);
        cache
            .insert(
                "enabled".into(),
                vec![(1, "view".into(), serde_json::json!({"media_type":"movie"}))],
            )
            .await;
        invalidate_enabled_vlibs_cache(&cache);
        assert!(cache.get(&"enabled".to_string()).await.is_none());
    }

    #[tokio::test]
    async fn validates_custom_cover_keys_and_cleans_up_orphan_files() {
        let dir = std::env::temp_dir().join(format!("rx_cover_test_{}", uuid::Uuid::new_v4().simple()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let active_key = "custom_0123456789abcdef";
        let orphan_key = "custom_fedcba9876543210";
        tokio::fs::write(dir.join(format!("{}.jpg", active_key)), b"jpg1")
            .await
            .unwrap();
        tokio::fs::write(dir.join(format!("{}.jpg", orphan_key)), b"jpg2")
            .await
            .unwrap();

        assert!(validate_cover_image_value(None, &dir).is_ok());
        assert!(validate_cover_image_value(Some("vlib_top_rated"), &dir).is_ok());
        assert!(validate_cover_image_value(Some("collage_cards"), &dir).is_ok());
        assert!(validate_cover_image_value(Some(active_key), &dir).is_ok());
        assert!(validate_cover_image_value(Some("custom_ffffffffffffffff"), &dir).is_err());
        assert!(validate_cover_image_value(Some("unknown_cover"), &dir).is_err());

        let mut referenced = HashSet::new();
        referenced.insert(active_key.to_string());
        cleanup_unreferenced_custom_cover_files(&dir, &referenced).await;

        assert!(dir.join(format!("{}.jpg", active_key)).is_file());
        assert!(!dir.join(format!("{}.jpg", orphan_key)).exists());
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
