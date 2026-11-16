pub(super) use crate::smart_views::{can_access_view, VirtualLibraryRule};
pub(super) use crate::state::{AppState, ScrapeTask};
pub(super) use crate::web::emby_api::auth::{extract_emby_token, resolve_valid_user_id_with_query};
pub(super) use crate::web::emby_api::ids::{
    build_search_pattern, numeric_tmdb_id, parse_episode_id, parse_season_id, parse_work_id,
};
pub(super) use crate::web::emby_api::playback::{
    build_media_sources_for_episode, build_media_sources_for_movie, PlaybackQuery,
};
pub(super) use crate::web::emby_api::sessions::{
    is_media_favorited, load_media_episode_histories, load_user_favorite_keys,
};
pub(super) use crate::web::emby_api::views::{
    get_user_allowed_categories, user_access_scope, UserAccessScope,
};
pub(super) use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    Json,
};
pub(super) use serde::Deserialize;
pub(super) use serde_json::Value;
pub(super) use sqlx::{postgres::PgArguments, Arguments, Encode, Postgres};
pub(super) use std::{collections::HashSet, sync::Arc};
pub(super) use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub(super) struct MediaListRow {
    pub(super) id: i64,
    pub(super) media_type: String,
    pub(super) tmdb_id: Option<i32>,
    pub(super) title: String,
    pub(super) original_title: Option<String>,
    pub(super) year: Option<i32>,
    pub(super) premiered_at: Option<String>,
    pub(super) overview: Option<String>,
    pub(super) rating: f32,
    pub(super) poster_path: Option<String>,
    pub(super) backdrop_path: Option<String>,
    pub(super) created_at: chrono::DateTime<chrono::Utc>,
    pub(super) num_seasons: Option<i32>,
    pub(super) num_episodes: Option<i32>,
}

/// 动态 SQL 构建器：只拼接实际需要的谓词，绑定值与 `$N` 编号严格一一对应，
/// 避免 `($1 IS NULL OR ...)` 泛化谓词阻断 PostgreSQL 最优查询计划
pub(in crate::web::emby_api) struct DynSql {
    pub(in crate::web::emby_api) sql: String,
    pub(in crate::web::emby_api) args: PgArguments,
    pub(in crate::web::emby_api) n: usize,
}

impl DynSql {
    pub(in crate::web::emby_api) fn new(initial: &str) -> Self {
        Self {
            sql: initial.to_string(),
            args: PgArguments::default(),
            n: 0,
        }
    }

    pub(in crate::web::emby_api) fn push(&mut self, sql: &str) {
        self.sql.push_str(sql);
    }

    /// 下一个绑定值将获得的占位符编号（供 SQL 文本提前引用）
    pub(in crate::web::emby_api) fn next_param(&self) -> String {
        format!("${}", self.n + 1)
    }

    /// 绑定一个值并返回其 `$N` 占位符；可在谓词文本中重复引用同一占位符
    pub(in crate::web::emby_api) fn bind<T>(
        &mut self,
        value: T,
    ) -> Result<String, (StatusCode, String)>
    where
        T: Encode<'static, Postgres> + sqlx::Type<Postgres> + 'static,
    {
        self.n += 1;
        self.args
            .add(value)
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        Ok(format!("${}", self.n))
    }

    /// 常用包装：推入前缀 SQL 文本、绑定值、后缀 SQL 文本
    pub(in crate::web::emby_api) fn push_bind_push<T>(
        &mut self,
        prefix: &str,
        value: T,
        suffix: &str,
    ) -> Result<String, (StatusCode, String)>
    where
        T: Encode<'static, Postgres> + sqlx::Type<Postgres> + 'static,
    {
        self.push(prefix);
        let p = self.bind(value)?;
        self.push(suffix);
        Ok(p)
    }
}

/// get_user_items 主列表的过滤条件集合（主查询与计数兜底查询共用）
pub(super) struct MediaListFilters<'a> {
    pub(super) user_id: &'a str,
    pub(super) access_scope: &'a UserAccessScope,
    pub(super) category_filter: Option<&'a str>,
    pub(super) media_type_filter: Option<&'a str>,
    pub(super) search_pattern: Option<&'a str>,
    pub(super) search_tmdb: Option<i32>,
    pub(super) explicit_ids: Option<&'a [i64]>,
    pub(super) only_favorite: bool,
    pub(super) only_played: bool,
    pub(super) only_unplayed: bool,
    pub(super) rule_json: Option<&'a Value>,
    pub(super) rule: Option<&'a VirtualLibraryRule>,
}

impl UserAccessScope {
    pub(in crate::web::emby_api) fn push_sql_predicate(
        &self,
        sql: &mut DynSql,
    ) -> Result<(), (StatusCode, String)> {
        let mut rules = Vec::with_capacity(self.virtual_libraries.len());
        for library in &self.virtual_libraries {
            let rule: VirtualLibraryRule = serde_json::from_value(library.filter_rules.clone())
                .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
            rule.validate()
                .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
            rules.push((library.filter_rules.clone(), rule));
        }
        let Some(categories) = &self.allowed_categories else {
            return Ok(());
        };
        let category_param = sql.bind(categories.clone())?;
        let mut predicates = vec![format!("m.category = ANY({category_param})")];
        for (rules_json, rule) in rules {
            let rule_param = sql.bind(rules_json)?;
            predicates.push(rule.sql_predicate(&format!("{}::jsonb", rule_param)));
        }
        sql.push(&format!(" AND ({})", predicates.join(" OR ")));
        Ok(())
    }
}

pub(super) fn push_user_access_scope(
    sql: &mut DynSql,
    scope: &UserAccessScope,
) -> Result<(), (StatusCode, String)> {
    scope.push_sql_predicate(sql)
}

/// 把过滤条件以编号绑定参数形式写入 SQL 构建器（作用于 `media_works m` 别名）
pub(super) fn push_media_filters(
    sql: &mut DynSql,
    f: &MediaListFilters<'_>,
) -> Result<(), (StatusCode, String)> {
    sql.push(
        "EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE)",
    );

    if let Some(cat) = &f.category_filter {
        sql.push_bind_push(" AND m.category = ", cat.to_string(), "")?;
    }

    if let Some(p) = &f.search_pattern {
        // 模式只绑定一次，多处占位符复用同一 $N
        let pat = sql.bind(p.to_string())?;
        let mut pred = format!(
            " AND (m.title ILIKE {pat} OR m.original_title ILIKE {pat} OR m.sort_title ILIKE {pat}"
        );
        pred.push_str(&format!(
            " OR EXISTS (SELECT 1 FROM sync_folders sf2 WHERE sf2.work_id = m.id AND (sf2.clean_title ILIKE {pat} OR sf2.raw_folder_name ILIKE {pat}))"
        ));
        if let Some(tid) = f.search_tmdb {
            let tid_param = sql.bind(tid)?;
            pred.push_str(&format!(" OR m.tmdb_id = {}", tid_param));
        }
        pred.push(')');
        sql.push(&pred);
    }

    if let Some(mt) = &f.media_type_filter {
        sql.push_bind_push(" AND m.media_type = ", mt.to_string(), "")?;
    }

    if let Some(ids) = &f.explicit_ids {
        sql.push_bind_push(" AND m.id = ANY(", ids.to_vec(), ")")?;
    }

    // user_id 只绑定一次，收藏/已看/未看三个 EXISTS 复用同一占位符
    let uid = sql.bind(f.user_id.to_string())?;
    if f.only_favorite {
        sql.push(&format!(
            " AND EXISTS (SELECT 1 FROM user_favorites uf WHERE uf.user_id = {uid} AND ((uf.tmdb_id IS NOT NULL AND uf.tmdb_id = m.tmdb_id AND uf.media_type = m.media_type) OR uf.work_id = m.id))"
        ));
    }
    if f.only_played {
        sql.push(&format!(
            " AND EXISTS (SELECT 1 FROM play_histories ph WHERE ph.user_id = {uid} AND ph.is_played = TRUE AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = m.tmdb_id AND ph.media_type = m.media_type) OR ph.work_id = m.id))"
        ));
    }
    if f.only_unplayed {
        sql.push(&format!(
            " AND NOT EXISTS (SELECT 1 FROM play_histories ph WHERE ph.user_id = {uid} AND ph.is_played = TRUE AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id = m.tmdb_id AND ph.media_type = m.media_type) OR ph.work_id = m.id))"
        ));
    }

    push_user_access_scope(sql, f.access_scope)?;

    if let Some(rule) = &f.rule {
        let rj = sql.bind(f.rule_json.cloned().unwrap_or(Value::Null))?;
        sql.push(&format!(
            " AND {}",
            rule.sql_predicate(&format!("{}::jsonb", rj))
        ));
    }
    Ok(())
}

pub(in crate::web::emby_api) async fn ensure_media_accessible(
    state: &AppState,
    user_id: &str,
    work_id: i64,
) -> Result<(String, Option<i32>, String), (StatusCode, String)> {
    let row: Option<(String, Option<i32>, String)> = sqlx::query_as(
        r#"
        SELECT w.media_type, w.tmdb_id, w.category
        FROM media_works w
        WHERE w.id = $1
          AND EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = w.id AND f.is_ignored = FALSE)
        "#,
    )
    .bind(work_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (media_type, tmdb_id, category) =
        row.ok_or((StatusCode::NOT_FOUND, "媒体不存在或已忽略".to_string()))?;

    let access_scope = user_access_scope(state, user_id).await?;
    if !work_access_allowed(state, work_id, &category, &access_scope).await? {
        return Err((StatusCode::FORBIDDEN, "无权访问该分类下的媒体".to_string()));
    }

    Ok((media_type, tmdb_id, category))
}

pub(super) async fn work_access_allowed(
    state: &AppState,
    work_id: i64,
    category: &str,
    scope: &UserAccessScope,
) -> Result<bool, (StatusCode, String)> {
    let Some(categories) = &scope.allowed_categories else {
        return Ok(true);
    };
    if categories.iter().any(|c| c == "*" || c == category) {
        return Ok(true);
    }
    for library in &scope.virtual_libraries {
        let rule: VirtualLibraryRule = serde_json::from_value(library.filter_rules.clone())
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        rule.validate()
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
        let predicate = rule.sql_predicate("$2::jsonb");
        let query =
            format!("SELECT EXISTS(SELECT 1 FROM media_works m WHERE m.id = $1 AND ({predicate}))");
        let matches: bool = sqlx::query_scalar(&query)
            .bind(work_id)
            .bind(&library.filter_rules)
            .fetch_one(&state.db)
            .await
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        if matches {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::emby_api::views::{AccessibleVirtualLibrary, UserAccessScope};

    #[test]
    fn dyn_sql_bind_returns_a_parameter_or_an_http_error() {
        let mut sql = DynSql::new("");
        assert_eq!(sql.bind("value".to_string()).unwrap(), "$1");
        assert_eq!(sql.n, 1);
    }

    #[test]
    fn user_access_scope_shared_sql_helper_appends_validated_rules() {
        let scope = UserAccessScope {
            allowed_categories: Some(vec!["电影".to_string()]),
            virtual_libraries: vec![AccessibleVirtualLibrary {
                filter_rules: serde_json::json!({"min_rating": 8.0}),
            }],
        };
        let mut sql = DynSql::new("SELECT m.id FROM media_works m WHERE TRUE");
        scope.push_sql_predicate(&mut sql).unwrap();
        assert!(sql.sql.contains("m.category = ANY($1)"));
        assert!(sql.sql.contains("m.rating >="));
        assert!(sql.sql.contains("$2::jsonb"));
        assert_eq!(sql.n, 2);
    }

    #[test]
    fn list_access_sql_combines_categories_and_accessible_virtual_libraries() {
        let scope = Arc::new(UserAccessScope {
            allowed_categories: Some(vec!["电影".to_string()]),
            virtual_libraries: vec![AccessibleVirtualLibrary {
                filter_rules: serde_json::json!({"min_rating": 8.0}),
            }],
        });
        let filters = MediaListFilters {
            user_id: "user",
            access_scope: &scope,
            category_filter: None,
            media_type_filter: None,
            search_pattern: None,
            search_tmdb: None,
            explicit_ids: None,
            only_favorite: false,
            only_played: false,
            only_unplayed: false,
            rule_json: None,
            rule: None,
        };
        let mut sql = DynSql::new("SELECT m.id FROM media_works m WHERE ");
        push_media_filters(&mut sql, &filters).unwrap();
        let query = sql.sql;
        assert!(query.contains("m.category = ANY($2)"), "{query}");
        assert!(query.contains(" OR (m.rating >="), "{query}");
        assert!(query.contains("f.work_id = m.id"), "{query}");
    }
}

#[derive(Debug, Default, Deserialize)]
pub(in crate::web::emby_api) struct ItemsQuery {
    #[serde(rename = "ParentId", alias = "parentId")]
    pub(in crate::web) parent_id: Option<String>,
    #[serde(rename = "SearchTerm", alias = "searchTerm")]
    pub(in crate::web) search_term: Option<String>,
    #[serde(rename = "Filters", alias = "filters")]
    pub(in crate::web) filters: Option<String>,
    #[serde(rename = "IncludeItemTypes", alias = "includeItemTypes")]
    pub(in crate::web) include_item_types: Option<String>,
    #[serde(rename = "SortBy", alias = "sortBy")]
    pub(in crate::web) sort_by: Option<String>,
    #[serde(rename = "SortOrder", alias = "sortOrder")]
    pub(in crate::web) sort_order: Option<String>,
    #[serde(rename = "Ids", alias = "ids")]
    pub(in crate::web) ids: Option<String>,
    #[serde(rename = "UserId", alias = "userId")]
    pub(in crate::web) user_id: Option<String>,
    #[serde(rename = "api_key", alias = "ApiKey", alias = "X-Emby-Token")]
    pub(in crate::web) api_key: Option<String>,
    #[serde(rename = "StartIndex", alias = "startIndex", default)]
    pub(in crate::web) start_index: i64,
    #[serde(rename = "Limit", alias = "limit")]
    pub(in crate::web) limit: Option<i64>,
}
