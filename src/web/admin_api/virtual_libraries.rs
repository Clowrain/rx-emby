use crate::{
    state::AppState,
    web::emby_api::{
        count_virtual_library_matches, resolve_virtual_library_cover_key, VirtualLibraryRule,
    },
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

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

    invalidate_enabled_vlibs_cache(&state.caches.enabled_vlibs_cache);
    state.caches.admin_stats_cache.invalidate_all();
    invalidate_vlib_image_meta(&state, vlib_id).await;

    Ok(Json(
        json!({ "status": "ok", "message": "智能视图已保存并立即生效" }),
    ))
}

pub(super) async fn seed_preset_virtual_libraries(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let presets: Vec<(&str, &str, &str, i32, VirtualLibraryRule)> = vec![
        (
            "🔥 TMDB 8.0+ 高分佳作",
            "tvshows",
            "vlib_top_rated",
            10,
            VirtualLibraryRule {
                min_rating: Some(8.0),
                ..Default::default()
            },
        ),
        (
            "🎬 Netflix / HBO 精品剧场",
            "tvshows",
            "vlib_streaming",
            20,
            VirtualLibraryRule {
                media_type: Some("tv".to_string()),
                networks: vec![
                    "Netflix".to_string(),
                    "HBO".to_string(),
                    "HBO Max".to_string(),
                    "Apple TV+".to_string(),
                    "Disney+".to_string(),
                ],
                ..Default::default()
            },
        ),
        (
            "🇨🇳 华语热播剧场",
            "tvshows",
            "vlib_chinese",
            30,
            VirtualLibraryRule {
                media_type: Some("tv".to_string()),
                categories: vec!["电视剧".to_string()],
                countries: vec!["CN".to_string(), "HK".to_string(), "TW".to_string()],
                ..Default::default()
            },
        ),
        (
            "🇰🇷 日韩剧集精选",
            "tvshows",
            "vlib_kr_jp",
            40,
            VirtualLibraryRule {
                media_type: Some("tv".to_string()),
                categories: vec!["电视剧".to_string()],
                countries: vec!["KR".to_string(), "JP".to_string()],
                ..Default::default()
            },
        ),
        (
            "🎞️ 2025-2026 最新院线与新番",
            "movies",
            "vlib_latest",
            50,
            VirtualLibraryRule {
                year_min: Some(2025),
                ..Default::default()
            },
        ),
        (
            "高分口碑电影",
            "movies",
            "vlib_top_rated",
            60,
            VirtualLibraryRule {
                media_type: Some("movie".into()),
                min_rating: Some(8.0),
                min_votes: Some(1000),
                ..Default::default()
            },
        ),
        (
            "已完结短剧",
            "tvshows",
            "vlib_default_tv",
            70,
            VirtualLibraryRule {
                media_type: Some("tv".into()),
                statuses: vec!["Ended".into()],
                episodes_max: Some(12),
                ..Default::default()
            },
        ),
        (
            "90 分钟内电影",
            "movies",
            "vlib_default_movies",
            80,
            VirtualLibraryRule {
                media_type: Some("movie".into()),
                runtime_max: Some(90),
                ..Default::default()
            },
        ),
        (
            "4K 杜比视界",
            "movies",
            "vlib_default_movies",
            90,
            VirtualLibraryRule {
                version_tags: vec!["4K".into(), "杜比视界".into()],
                ..Default::default()
            },
        ),
    ];

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    sqlx::query("SELECT pg_advisory_xact_lock(728314)")
        .execute(&mut *tx)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut inserted = 0;
    for (name, icon_type, cover_image, sort_order, rule) in presets {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM virtual_libraries WHERE name=$1)")
                .bind(name)
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if exists {
            continue;
        }
        let filter_rules = serde_json::to_value(rule)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        sqlx::query("INSERT INTO virtual_libraries (name,icon_type,sort_order,filter_rules,is_enabled,cover_image) VALUES ($1,$2,$3,$4,TRUE,$5)")
            .bind(name).bind(icon_type).bind(sort_order).bind(filter_rules).bind(cover_image)
            .execute(&mut *tx).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?;
        inserted += 1;
    }
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if inserted > 0 {
        invalidate_enabled_vlibs_cache(&state.caches.enabled_vlibs_cache);
        state.caches.admin_stats_cache.invalidate_all();
        let ids: Vec<i32> = sqlx::query_scalar("SELECT id FROM virtual_libraries")
            .fetch_all(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        for id in ids {
            for key in [
                format!("lib_vlib_{id}:Primary"),
                format!("lib_vlib_{id}:Backdrop"),
            ] {
                state.caches.image_meta_cache.invalidate(&key).await;
            }
        }
        state.caches.collage_cache.invalidate_all();
    }

    Ok(Json(json!({
        "status": "ok",
        "inserted": inserted,
        "message": format!("已创建 {} 个预设智能视图", inserted)
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

    let covers_dir = state.config.load().image_cache_dir.join("covers");
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
    invalidate_enabled_vlibs_cache(&state.caches.enabled_vlibs_cache);
    state.caches.admin_stats_cache.invalidate_all();
    state.caches.collage_cache.invalidate_all();
    Ok(Json(
        json!({"queued":result.rows_affected(),"message":format!("已将 {} 部作品加入深度元数据补齐队列",result.rows_affected())}),
    ))
}

#[cfg(test)]
mod cache_tests {
    use super::invalidate_enabled_vlibs_cache;
    use moka::future::Cache;

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
}
