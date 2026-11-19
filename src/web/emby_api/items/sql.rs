pub(super) use crate::smart_views::{can_access_view, VirtualLibraryRule};
pub(super) use crate::state::{AppState, ScrapeTask};
pub(super) use crate::web::emby_api::auth::{extract_emby_token, resolve_valid_user_id_with_query};
pub(super) use crate::web::emby_api::ids::{
    build_search_pattern, numeric_tmdb_id, parse_episode_id, parse_season_id, parse_work_id,
};
pub(super) use crate::web::emby_api::playback::{
    build_media_sources_for_episode, build_media_sources_for_movie, build_media_sources_for_movies,
    PlaybackQuery,
};
pub(super) use crate::web::emby_api::query::EmbyQuery as Query;
pub(super) use crate::web::emby_api::sessions::{is_media_favorited, load_user_favorite_keys};
pub(super) use crate::web::emby_api::views::{
    get_user_allowed_categories, user_access_scope, UserAccessScope,
};
pub(super) use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
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
    pub(super) sort_title: Option<String>,
    pub(super) original_title: Option<String>,
    pub(super) year: Option<i32>,
    pub(super) premiered_at: Option<String>,
    pub(super) overview: Option<String>,
    pub(super) genres: Vec<String>,
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

    /// 追加分页参数，沿用查询当前的占位符编号。
    pub(in crate::web::emby_api) fn push_pagination(
        &mut self,
        limit: i64,
        offset: i64,
    ) -> Result<(), (StatusCode, String)> {
        self.push_bind_push(" LIMIT ", limit, "")?;
        self.push_bind_push(" OFFSET ", offset, "")?;
        Ok(())
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
        self.push(&p);
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
    pub(super) genres: Option<&'a str>,
    pub(super) studios: Option<&'a str>,
    pub(super) tags: Option<&'a str>,
    pub(super) years: Option<&'a str>,
    pub(super) official_ratings: Option<&'a str>,
    pub(super) persons: Option<&'a str>,
    pub(super) artists: Option<&'a str>,

    pub(super) genre_ids: Option<&'a [String]>,
    pub(super) studio_ids: Option<&'a [String]>,
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
        let Some(categories) = &self.allowed_categories else {
            return Ok(());
        };
        let mut rules = Vec::with_capacity(self.virtual_libraries.len());
        for library in &self.virtual_libraries {
            let rule: VirtualLibraryRule = serde_json::from_value(library.filter_rules.clone())
                .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
            rule.validate()
                .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
            rules.push((library.filter_rules.clone(), rule));
        }
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

pub(in crate::web::emby_api) fn split_metadata_values(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or("")
        .split(['|', ','])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
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
            " OR EXISTS (SELECT 1 FROM sync_folders sf2 WHERE sf2.work_id = m.id AND sf2.is_ignored = FALSE AND (sf2.clean_title ILIKE {pat} OR sf2.raw_folder_name ILIKE {pat}))"
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

    push_metadata_filters(sql, f)?;

    // 仅在需要用户状态过滤时绑定 user_id，并在这些 EXISTS 中复用。
    let uid = if f.only_favorite || f.only_played || f.only_unplayed {
        Some(sql.bind(f.user_id.to_string())?)
    } else {
        None
    };
    if f.only_favorite {
        let uid = uid.as_deref().ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "缺少用户筛选参数".to_string(),
            )
        })?;
        sql.push(&format!(
            " AND EXISTS (SELECT 1 FROM user_favorites uf WHERE uf.user_id = {uid} AND ((uf.tmdb_id IS NOT NULL AND uf.tmdb_id = m.tmdb_id AND uf.media_type = m.media_type) OR uf.work_id = m.id))"
        ));
    }
    if f.only_played {
        let uid = uid.as_deref().ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "缺少用户筛选参数".to_string(),
            )
        })?;
        sql.push(&format!(" AND CASE WHEN m.media_type = 'tv' THEN (EXISTS (SELECT 1 FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id WHERE f.work_id=m.id AND f.is_ignored=FALSE) AND NOT EXISTS (SELECT 1 FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id WHERE f.work_id=m.id AND f.is_ignored=FALSE AND NOT EXISTS (SELECT 1 FROM play_histories ph WHERE ph.user_id={uid} AND ph.is_played=TRUE AND ph.season_number=sf.season_number AND ph.episode_number=sf.episode_number AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id=m.tmdb_id AND ph.media_type=m.media_type) OR ph.work_id=m.id)))) ELSE EXISTS (SELECT 1 FROM play_histories ph WHERE ph.user_id={uid} AND ph.is_played=TRUE AND ph.season_number=0 AND ph.episode_number=1 AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id=m.tmdb_id AND ph.media_type=m.media_type) OR ph.work_id=m.id)) END"));
    }
    if f.only_unplayed {
        let uid = uid.as_deref().ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "缺少用户筛选参数".to_string(),
            )
        })?;
        sql.push(&format!(" AND CASE WHEN m.media_type = 'tv' THEN NOT (EXISTS (SELECT 1 FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id WHERE f.work_id=m.id AND f.is_ignored=FALSE) AND NOT EXISTS (SELECT 1 FROM sync_files sf JOIN sync_folders f ON f.id=sf.folder_id WHERE f.work_id=m.id AND f.is_ignored=FALSE AND NOT EXISTS (SELECT 1 FROM play_histories ph WHERE ph.user_id={uid} AND ph.is_played=TRUE AND ph.season_number=sf.season_number AND ph.episode_number=sf.episode_number AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id=m.tmdb_id AND ph.media_type=m.media_type) OR ph.work_id=m.id)))) ELSE NOT EXISTS (SELECT 1 FROM play_histories ph WHERE ph.user_id={uid} AND ph.is_played=TRUE AND ph.season_number=0 AND ph.episode_number=1 AND ((ph.tmdb_id IS NOT NULL AND ph.tmdb_id=m.tmdb_id AND ph.media_type=m.media_type) OR ph.work_id=m.id)) END"));
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

fn push_metadata_filters(
    sql: &mut DynSql,
    f: &MediaListFilters<'_>,
) -> Result<(), (StatusCode, String)> {
    for (column, raw) in [("genres", f.genres), ("networks", f.studios)] {
        let names = split_metadata_values(raw);
        if !names.is_empty() {
            sql.push_bind_push(&format!(" AND m.{column} && "), names, "::text[]")?;
        }
    }
    let tags = split_metadata_values(f.tags);
    if !tags.is_empty() {
        let param = sql.bind(tags)?;
        sql.push(&format!(" AND (m.tags && {param}::text[] OR EXISTS (SELECT 1 FROM jsonb_array_elements(COALESCE(m.tmdb_metadata->'keyword_items', '[]'::jsonb)) k WHERE k->>'name' = ANY({param})))"));
    }
    for (column, names) in [("genres", f.genre_ids), ("networks", f.studio_ids)] {
        if let Some(names) = names {
            sql.push_bind_push(&format!(" AND m.{column} && "), names.to_vec(), "::text[]")?;
        }
    }
    let years = split_metadata_values(f.years)
        .into_iter()
        .map(|year| {
            year.strip_prefix("year_")
                .unwrap_or(&year)
                .parse::<i32>()
                .map_err(|_| (StatusCode::BAD_REQUEST, "Years 包含无效年份".to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !years.is_empty() {
        sql.push_bind_push(" AND m.year = ANY(", years, ")")?;
    }
    let ratings = split_metadata_values(f.official_ratings);
    if !ratings.is_empty() {
        let param = sql.bind(ratings)?;
        sql.push(&format!(" AND EXISTS (SELECT 1 FROM jsonb_array_elements(COALESCE(m.tmdb_metadata->'certifications','[]')) c WHERE c->>'rating' = ANY({param}))"));
    }
    let persons = split_metadata_values(f.persons);
    if !persons.is_empty() {
        let param = sql.bind(persons)?;
        sql.push(&format!(" AND EXISTS (SELECT 1 FROM jsonb_array_elements(COALESCE(m.tmdb_metadata->'cast', '[]'::jsonb) || COALESCE(m.tmdb_metadata->'directors', '[]'::jsonb)) p WHERE p->>'name' = ANY({param}))"));
    }
    if !split_metadata_values(f.artists).is_empty() {
        // This library currently has no music artist entities.
        sql.push(" AND FALSE");
    }

    Ok(())
}

/// 在分集和续播查询中复用主列表的元数据筛选，不改变已有访问范围。
pub(in crate::web::emby_api) async fn push_metadata_query_filters(
    state: &AppState,
    user_id: &str,
    q: &ItemsQuery,
    sql: &mut DynSql,
) -> Result<(), (StatusCode, String)> {
    let normalized =
        super::super::metadata_lists::normalize_metadata_query(state, user_id, q).await?;
    let q = &normalized;
    let genre_names = super::super::metadata_lists::resolve_metadata_ids(
        state,
        user_id,
        q.genre_ids.as_deref(),
        "m.genres",
        "genre",
    )
    .await?;
    let studio_names = super::super::metadata_lists::resolve_metadata_ids(
        state,
        user_id,
        q.studio_ids.as_deref(),
        "m.networks",
        "studio",
    )
    .await?;
    let scope = UserAccessScope {
        allowed_categories: None,
        virtual_libraries: vec![],
    };
    let person_names = super::super::metadata_lists::resolve_metadata_ids(
        state,
        user_id,
        q.person_ids.as_deref(),
        super::super::metadata_lists::PERSON_NAMES,
        "person",
    )
    .await?;
    let person_values = [
        split_metadata_values(q.persons.as_deref()),
        person_names.unwrap_or_default(),
    ]
    .concat();
    let person_filter = (!person_values.is_empty()).then(|| person_values.join("|"));
    let artist_values = [q.artists.as_deref(), q.artist_ids.as_deref()]
        .into_iter()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("|");
    let filters = MediaListFilters {
        user_id,
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
        genres: q.genres.as_deref(),
        studios: q.studios.as_deref(),
        tags: q.tags.as_deref(),
        years: q.years.as_deref(),
        official_ratings: q.official_ratings.as_deref(),
        persons: person_filter.as_deref(),
        artists: (!artist_values.is_empty()).then_some(artist_values.as_str()),
        genre_ids: genre_names.as_deref(),
        studio_ids: studio_names.as_deref(),
    };
    push_metadata_filters(sql, &filters)
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

pub(in crate::web::emby_api) async fn work_access_allowed(
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
    let mut sql = DynSql::new("SELECT EXISTS(SELECT 1 FROM media_works m WHERE m.id = ");
    let id = sql.bind(work_id)?;
    let mut predicates = Vec::with_capacity(scope.virtual_libraries.len());
    for library in &scope.virtual_libraries {
        let rule: VirtualLibraryRule = serde_json::from_value(library.filter_rules.clone())
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        rule.validate()
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
        let rules = sql.bind(library.filter_rules.clone())?;
        predicates.push(rule.sql_predicate(&format!("{rules}::jsonb")));
    }
    if predicates.is_empty() {
        return Ok(false);
    }
    sql.push(&format!("{id} AND ({})", predicates.join(" OR ")));
    sql.push(")");
    sqlx::query_scalar_with(&sql.sql, sql.args)
        .fetch_one(&state.db)
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
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
    fn dyn_sql_pagination_preserves_existing_parameter_numbers() {
        let mut sql = DynSql::new("SELECT m.id FROM media_works m WHERE ");
        sql.push_bind_push("m.category = ", "电影".to_string(), "")
            .unwrap();
        sql.push_pagination(20, 40).unwrap();
        assert_eq!(
            sql.sql,
            "SELECT m.id FROM media_works m WHERE m.category = $1 LIMIT $2 OFFSET $3"
        );
        assert_eq!(sql.n, 3);
    }

    #[test]
    fn dyn_sql_push_bind_push_inserts_numbered_parameter() {
        let mut sql = DynSql::new("SELECT m.id FROM media_works m WHERE ");
        let parameter = sql
            .push_bind_push("m.category = ", "电影".to_string(), " AND TRUE")
            .unwrap();
        assert_eq!(parameter, "$1");
        assert_eq!(
            sql.sql,
            "SELECT m.id FROM media_works m WHERE m.category = $1 AND TRUE"
        );
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
    fn folder_name_search_excludes_ignored_folders() {
        let mut sql = DynSql::new("SELECT m.id FROM media_works m WHERE TRUE");
        let filters = MediaListFilters {
            user_id: "user",
            access_scope: &UserAccessScope {
                allowed_categories: None,
                virtual_libraries: vec![],
            },
            category_filter: None,
            media_type_filter: None,
            search_pattern: Some("%term%"),
            search_tmdb: None,
            explicit_ids: None,
            genres: None,
            studios: None,
            tags: None,
            years: None,
            official_ratings: None,
            persons: None,
            artists: None,
            genre_ids: None,
            studio_ids: None,
            only_favorite: false,
            only_played: false,
            only_unplayed: false,
            rule_json: None,
            rule: None,
        };
        push_media_filters(&mut sql, &filters).unwrap();
        assert!(sql.sql.contains("sf2.is_ignored = FALSE"));
        assert_eq!(
            sql.n, 1,
            "user_id is not bound when user-state filters are off"
        );
    }

    #[test]
    fn no_category_scope_skips_virtual_library_rule_validation() {
        let scope = UserAccessScope {
            allowed_categories: None,
            virtual_libraries: vec![AccessibleVirtualLibrary {
                filter_rules: serde_json::json!({"unknown_rule": true}),
            }],
        };
        let mut sql = DynSql::new("SELECT m.id FROM media_works m WHERE TRUE");
        scope.push_sql_predicate(&mut sql).unwrap();
        assert_eq!(sql.n, 0);
    }

    #[test]
    fn list_access_sql_combines_categories_and_virtual_library_rules() {
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
            genres: None,
            studios: None,
            tags: None,
            years: None,
            official_ratings: None,
            persons: None,
            artists: None,
            genre_ids: None,
            studio_ids: None,
            only_favorite: false,
            only_played: false,
            only_unplayed: false,
            rule_json: None,
            rule: None,
        };
        let mut sql = DynSql::new("SELECT m.id FROM media_works m WHERE ");
        push_media_filters(&mut sql, &filters).unwrap();
        let query = sql.sql;
        assert!(query.contains("m.category = ANY($1)"), "{query}");
        assert!(query.contains(" OR (m.rating >="), "{query}");
        assert!(query.contains("f.work_id = m.id"), "{query}");
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
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
    #[serde(rename = "Fields", alias = "fields")]
    pub(in crate::web) fields: Option<String>,
    #[serde(rename = "Recursive", alias = "recursive", default)]
    pub(in crate::web) recursive: bool,
    #[serde(rename = "ExcludeItemTypes", alias = "excludeItemTypes")]
    pub(in crate::web) exclude_item_types: Option<String>,
    #[serde(rename = "Ids", alias = "ids")]
    pub(in crate::web) ids: Option<String>,
    #[serde(rename = "Genres", alias = "genres")]
    pub(in crate::web) genres: Option<String>,
    #[serde(rename = "GenreIds", alias = "genreIds")]
    pub(in crate::web) genre_ids: Option<String>,
    #[serde(rename = "Studios", alias = "studios")]
    pub(in crate::web) studios: Option<String>,
    #[serde(rename = "StudioIds", alias = "studioIds")]
    pub(in crate::web) studio_ids: Option<String>,
    #[serde(rename = "Tags", alias = "tags")]
    pub(in crate::web) tags: Option<String>,
    #[serde(rename = "Years", alias = "years")]
    pub(in crate::web) years: Option<String>,
    #[serde(rename = "OfficialRatings", alias = "officialRatings")]
    pub(in crate::web) official_ratings: Option<String>,
    #[serde(rename = "PersonIds", alias = "personIds")]
    pub(in crate::web) person_ids: Option<String>,
    #[serde(rename = "Persons", alias = "persons")]
    pub(in crate::web) persons: Option<String>,
    #[serde(rename = "ArtistIds", alias = "artistIds")]
    pub(in crate::web) artist_ids: Option<String>,
    #[serde(rename = "Artists", alias = "artists")]
    pub(in crate::web) artists: Option<String>,
    #[serde(rename = "UserId", alias = "userId")]
    pub(in crate::web) user_id: Option<String>,
    #[serde(
        rename = "api_key",
        alias = "ApiKey",
        alias = "X-Emby-Token",
        alias = "x-emby-token"
    )]
    pub(in crate::web) api_key: Option<String>,
    #[serde(rename = "StartIndex", alias = "startIndex", default)]
    pub(in crate::web) start_index: i64,
    #[serde(rename = "Limit", alias = "limit")]
    pub(in crate::web) limit: Option<i64>,
}

#[cfg(test)]
mod metadata_filter_tests {
    use super::*;

    #[test]
    fn items_query_accepts_fields_recursive_and_excluded_item_types() {
        let q: ItemsQuery = serde_json::from_value(serde_json::json!({
            "Fields": "Overview,Genres,MediaSources",
            "Recursive": true,
            "ExcludeItemTypes": "Episode,Season"
        }))
        .unwrap();
        assert_eq!(q.fields.as_deref(), Some("Overview,Genres,MediaSources"));
        assert!(q.recursive);
        assert_eq!(q.exclude_item_types.as_deref(), Some("Episode,Season"));
    }

    #[test]
    fn items_query_accepts_person_and_artist_filter_aliases() {
        let q: ItemsQuery = serde_json::from_value(serde_json::json!({
            "PersonIds": "person_1", "Persons": "Actor",
            "ArtistIds": "artist_1", "Artists": "Singer"
        }))
        .unwrap();
        assert_eq!(q.person_ids.as_deref(), Some("person_1"));
        assert_eq!(q.persons.as_deref(), Some("Actor"));
        assert_eq!(q.artist_ids.as_deref(), Some("artist_1"));
        assert_eq!(q.artists.as_deref(), Some("Singer"));
    }

    #[test]
    fn items_query_deserializes_metadata_filters_with_compatible_names() {
        let q: ItemsQuery = serde_json::from_value(serde_json::json!({
            "Genres": "A|genre_id", "GenreIds": "A,genre_id", "Studios": "B",
            "StudioIds": "studio_id", "Tags": "tag_id|HDR", "Years": "year_2024,2025",
            "OfficialRatings": "PG|R"
        }))
        .unwrap();
        assert_eq!(q.genres.as_deref(), Some("A|genre_id"));
        assert_eq!(q.genre_ids.as_deref(), Some("A,genre_id"));
        assert_eq!(q.studios.as_deref(), Some("B"));
        assert_eq!(q.studio_ids.as_deref(), Some("studio_id"));
        assert_eq!(q.tags.as_deref(), Some("tag_id|HDR"));
        assert_eq!(q.years.as_deref(), Some("year_2024,2025"));
        assert_eq!(q.official_ratings.as_deref(), Some("PG|R"));
    }

    #[test]
    fn metadata_filters_split_both_delimiters_and_bind_arrays() {
        let scope = UserAccessScope {
            allowed_categories: None,
            virtual_libraries: vec![],
        };
        let filters = MediaListFilters {
            user_id: "u",
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
            genre_ids: None,
            studio_ids: None,
            genres: Some("剧情|喜剧, 动作"),
            studios: Some("A|B"),
            tags: Some("4K,HDR"),
            years: Some("year_2024|2025"),
            official_ratings: None,
            persons: None,
            artists: None,
        };
        let mut sql = DynSql::new("SELECT m.id FROM media_works m WHERE ");
        push_media_filters(&mut sql, &filters).unwrap();
        assert!(sql.sql.contains("m.genres && $1"), "{}", sql.sql);
        assert!(sql.sql.contains("m.networks && $2"));
        assert!(sql.sql.contains("m.tags && $3"));
        assert!(sql.sql.contains("m.year = ANY($4)"));
        assert_eq!(
            split_metadata_values(Some("剧情|喜剧, 动作")),
            vec!["剧情", "喜剧", "动作"]
        );
    }

    #[test]
    fn invalid_years_are_errors_and_ratings_use_stored_certifications() {
        let scope = UserAccessScope {
            allowed_categories: None,
            virtual_libraries: vec![],
        };
        let mut filters = MediaListFilters {
            user_id: "u",
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
            genre_ids: None,
            studio_ids: None,
            genres: None,
            studios: None,
            tags: None,
            years: Some("wrong"),
            official_ratings: None,
            persons: None,
            artists: None,
        };
        assert_eq!(
            push_media_filters(&mut DynSql::new(""), &filters)
                .unwrap_err()
                .0,
            StatusCode::BAD_REQUEST
        );
        filters.years = None;
        filters.official_ratings = Some("PG-13");
        let mut sql = DynSql::new("");
        push_media_filters(&mut sql, &filters).unwrap();
        assert!(sql.sql.contains("tmdb_metadata->'certifications'"));
        assert!(sql.sql.contains("c->>'rating' = ANY($1)"));
    }
}
