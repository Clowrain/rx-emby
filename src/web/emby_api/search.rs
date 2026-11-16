//! `GET /Search/Hints`：官方 Emby 客户端、Infuse、VidHub、Yamby 等使用的搜索提示接口。
//! 覆盖 Series / Movie 主条目、Episode 分集，并支持按 TMDB ID、目录名等扩展匹配。

use super::auth::resolve_valid_user_id_with_query;
use super::ids::{build_search_pattern, numeric_tmdb_id};
use super::views::{user_access_scope, UserAccessScope};
use crate::smart_views::VirtualLibraryRule;
use crate::state::AppState;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Deserialize, Default)]
pub(super) struct SearchHintsQuery {
    #[serde(rename = "SearchTerm", alias = "searchTerm")]
    search_term: Option<String>,
    #[serde(rename = "Limit", alias = "limit")]
    limit: Option<i64>,
    #[serde(rename = "UserId", alias = "userId")]
    user_id: Option<String>,
    #[serde(rename = "api_key", alias = "ApiKey", alias = "X-Emby-Token")]
    api_key: Option<String>,
    #[serde(rename = "IncludeItemTypes", alias = "includeItemTypes")]
    include_item_types: Option<String>,
    #[serde(rename = "MediaTypes", alias = "mediaTypes")]
    media_types: Option<String>,
}

pub(super) async fn get_search_hints(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<SearchHintsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    let access_scope = user_access_scope(&state, &user_id).await?;
    if access_scope.has_no_access() {
        return Ok(Json(json!({ "SearchHints": [], "TotalRecordCount": 0 })));
    }

    let cfg = state.config.load();
    let term = q.search_term.as_deref().map(str::trim).unwrap_or("");
    let pattern = build_search_pattern(Some(term));
    let tmdb = numeric_tmdb_id(Some(term));
    let Some(ref p) = pattern else {
        return Ok(Json(json!({ "SearchHints": [], "TotalRecordCount": 0 })));
    };

    let item_types = parse_type_filter(q.include_item_types.as_deref(), q.media_types.as_deref());
    let include_items = item_types
        .as_ref()
        .is_none_or(|types| types.iter().any(|t| t == "Movie" || t == "Series"));
    let include_episodes = item_types
        .as_ref()
        .is_none_or(|types| types.contains(&"Episode".to_string()));
    let limit = q.limit.unwrap_or(20).clamp(1, 50);
    // 条目与分集各占一部分配额
    let item_limit = (limit / 2).max(8);
    let ep_limit = limit.saturating_sub(item_limit).max(4);

    // 主条目：标题 / 原名 / 排序名 / TMDB ID / 关联目录名
    let item_search_predicate = main_item_search_predicate("$1", "$2");
    let (item_access_predicate, item_access_rules) = access_scope.sql_predicate(5);
    let item_sql = format!(
        r#"
        SELECT 'work_' || m.id AS item_id,
               m.title,
               m.media_type,
               m.year,
               m.poster_path,
               m.tmdb_id,
               NULL::text AS series_title,
               NULL::int AS season_number,
               NULL::int AS episode_number
        FROM media_works m
        WHERE $4::boolean
          AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE)
          {item_search_predicate}
          AND ({item_access_predicate})
        ORDER BY m.rating DESC, m.year DESC NULLS LAST, m.id DESC
        LIMIT $3
        "#,
        item_search_predicate = item_search_predicate,
        item_access_predicate = item_access_predicate,
    );
    let mut item_query = sqlx::query_as(&item_sql)
        .bind(p)
        .bind(tmdb)
        .bind(item_limit)
        .bind(include_items)
        .bind(access_scope.allowed_categories.clone().unwrap_or_default());
    for rule in item_access_rules {
        item_query = item_query.bind(rule);
    }
    let items: Vec<HintRow> = item_query
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // 分集：TMDB 分集标题 / 本地文件名 / 剧名（1~2 字符短查询退化为顺序扫描，语义不受影响）
    let (episode_access_predicate, episode_access_rules) = access_scope.sql_predicate(4);
    let episode_sql = format!(
        r#"
        SELECT 'ep_' || we.work_id || '_' || we.season_number || '_' || we.episode_number AS item_id,
               COALESCE(we.title, '第 ' || we.episode_number || ' 集') AS title,
               'episode' AS media_type,
               NULL::int AS year,
               we.still_path AS poster_path,
               m.tmdb_id,
               m.title AS series_title,
               we.season_number,
               we.episode_number,
               'season_' || we.work_id || '_' || we.season_number AS season_id
        FROM work_episodes we
        JOIN media_works m ON m.id = we.work_id
        WHERE $3::boolean
          AND EXISTS (
            SELECT 1 FROM sync_files sf
            JOIN sync_folders f ON f.id = sf.folder_id
            WHERE f.work_id = m.id AND f.is_ignored = FALSE
              AND sf.season_number = we.season_number AND sf.episode_number = we.episode_number
              AND (we.title ILIKE $1 OR m.title ILIKE $1 OR m.original_title ILIKE $1
                   OR m.sort_title ILIKE $1 OR sf.file_name ILIKE $1)
          )
          AND ({episode_access_predicate})
        ORDER BY m.rating DESC, we.season_number ASC, we.episode_number ASC
        LIMIT $2
        "#,
        episode_access_predicate = episode_access_predicate,
    );
    let mut episode_query = sqlx::query_as(&episode_sql)
        .bind(p)
        .bind(ep_limit)
        .bind(include_episodes)
        .bind(access_scope.allowed_categories.clone().unwrap_or_default());
    for rule in episode_access_rules {
        episode_query = episode_query.bind(rule);
    }
    let episodes: Vec<HintRow> = episode_query
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let hints: Vec<Value> = items
        .into_iter()
        .map(|r| {
            let hint_type = if r.media_type == "movie" { "Movie" } else { "Series" };
            json!({
                "Id": r.item_id,
                "ItemId": r.item_id,
                "Name": r.title,
                "MatchedTerm": term,
                "Type": hint_type,
                "ProductionYear": r.year,
                "PrimaryImageTag": super::images::image_path_tag(r.poster_path.as_deref()),
                "ServerId": cfg.server_id,
                "ProviderIds": r.tmdb_id.map(|t| json!({ "Tmdb": t.to_string() })).unwrap_or(Value::Null)
            })
        })
        .chain(episodes.into_iter().map(|r| {
            json!({
                "Id": r.item_id,
                "ItemId": r.item_id,
                "Name": r.title,
                "MatchedTerm": term,
                "Type": "Episode",
                "SeriesName": r.series_title,
                "SeasonId": r.season_id,
                "IndexNumber": r.episode_number,
                "ParentIndexNumber": r.season_number,
                "PrimaryImageTag": super::images::image_path_tag(r.poster_path.as_deref()),
                "ServerId": cfg.server_id
            })
        }))
        .collect();

    let total = hints.len() as i64;
    Ok(Json(json!({
        "SearchHints": hints,
        "TotalRecordCount": total
    })))
}

impl UserAccessScope {
    fn sql_predicate(&self, category_param: usize) -> (String, Vec<Value>) {
        if self.allowed_categories.is_none() {
            return ("TRUE".to_string(), Vec::new());
        }
        let mut parts = vec![format!("m.category = ANY(${}::text[])", category_param)];
        let mut rules = Vec::with_capacity(self.virtual_libraries.len());
        for library in &self.virtual_libraries {
            let Ok(rule) =
                serde_json::from_value::<VirtualLibraryRule>(library.filter_rules.clone())
            else {
                continue;
            };
            let rule_param = category_param + 1 + rules.len();
            parts.push(format!(
                "({})",
                rule.sql_predicate(&format!("${}::jsonb", rule_param))
            ));
            rules.push(library.filter_rules.clone());
        }
        (parts.join(" OR "), rules)
    }
}

fn parse_type_filter(
    include_item_types: Option<&str>,
    media_types: Option<&str>,
) -> Option<Vec<String>> {
    let values = include_item_types
        .into_iter()
        .chain(media_types)
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        return None;
    }
    let mut types = Vec::new();
    for value in values {
        let normalized = match value.to_ascii_lowercase().as_str() {
            "movie" => "Movie",
            "series" | "tv" => "Series",
            "episode" => "Episode",
            _ => continue,
        };
        if !types.contains(&normalized.to_string()) {
            types.push(normalized.to_string());
        }
    }
    Some(types)
}

fn main_item_search_predicate(pattern_param: &str, tmdb_param: &str) -> String {
    format!(
        "AND (m.title ILIKE {pattern_param} OR m.original_title ILIKE {pattern_param} OR m.sort_title ILIKE {pattern_param}\n         OR ({tmdb_param}::int4 IS NOT NULL AND m.tmdb_id = {tmdb_param})\n         OR EXISTS (SELECT 1 FROM sync_folders sf2 WHERE sf2.work_id = m.id\n                    AND (sf2.clean_title ILIKE {pattern_param} OR sf2.raw_folder_name ILIKE {pattern_param})))"
    )
}

#[cfg(test)]
mod tests {
    use super::{main_item_search_predicate, parse_type_filter, UserAccessScope};

    #[test]
    fn search_type_filters_normalize_supported_media_types() {
        assert_eq!(
            parse_type_filter(Some("Movie,Episode"), None),
            Some(vec!["Movie".into(), "Episode".into()])
        );
        assert_eq!(
            parse_type_filter(None, Some("Series")),
            Some(vec!["Series".into()])
        );
        assert_eq!(parse_type_filter(None, None), None);
    }

    #[test]
    fn access_scope_filters_categories_and_permitted_virtual_libraries_in_sql() {
        let scope = UserAccessScope {
            allowed_categories: Some(vec!["电影".into()]),
            virtual_libraries: vec![super::super::views::AccessibleVirtualLibrary {
                filter_rules: serde_json::json!({"min_rating": 8.0}),
            }],
        };
        let (predicate, rules) = scope.sql_predicate(4);
        assert!(predicate.contains("m.category = ANY($4::text[])"));
        assert!(predicate.contains("$5::jsonb"));
        assert!(predicate.contains("min_rating"));
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn episode_search_uses_one_exists_and_matches_series_sort_title() {
        let source = include_str!("search.rs");
        let start = source.find("FROM work_episodes we").unwrap();
        let rest = &source[start..];
        let end = rest.find("ORDER BY m.rating").unwrap();
        let query = &rest[..end];
        assert!(query.contains("m.sort_title ILIKE $1"));
        assert_eq!(query.matches("AND EXISTS (").count(), 1);
    }

    #[test]
    fn numeric_search_matches_tmdb_id_as_an_alternative_to_title() {
        let predicate = main_item_search_predicate("$1", "$2");
        assert!(predicate.contains("OR ($2::int4 IS NOT NULL AND m.tmdb_id = $2)"));
        assert!(!predicate.contains("AND ($2::int4 IS NULL OR m.tmdb_id = $2)"));
    }
}

#[derive(sqlx::FromRow)]
struct HintRow {
    item_id: String,
    title: String,
    media_type: String,
    year: Option<i32>,
    poster_path: Option<String>,
    tmdb_id: Option<i32>,
    series_title: Option<String>,
    season_number: Option<i32>,
    episode_number: Option<i32>,
    #[sqlx(default)]
    season_id: Option<String>,
}
