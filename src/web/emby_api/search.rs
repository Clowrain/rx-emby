//! `GET /Search/Hints`：官方 Emby 客户端、Infuse、VidHub、Yamby 等使用的搜索提示接口。
//! 覆盖 Series / Movie 主条目、Episode 分集，并支持按 TMDB ID、目录名等扩展匹配。

use super::auth::resolve_valid_user_id_with_query;
use super::ids::{build_search_pattern, numeric_tmdb_id};
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
}

pub(super) async fn get_search_hints(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<SearchHintsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user_id =
        resolve_valid_user_id_with_query(&state, q.user_id.as_deref(), &headers, q.api_key.as_deref())
            .await?;
    let access: Option<(bool, Vec<String>)> = sqlx::query_as(
        "SELECT is_admin, allowed_categories FROM users WHERE id = $1",
    )
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        tracing::error!("查询搜索用户权限失败: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;
    let allowed_categories = access.and_then(|(admin, categories)| {
        (!admin && !categories.is_empty() && !categories.iter().any(|c| c == "*"))
            .then_some(categories)
    });

    let cfg = state.config.load();
    let term = q.search_term.as_deref().map(str::trim).unwrap_or("");
    let pattern = build_search_pattern(Some(term));
    let tmdb = numeric_tmdb_id(Some(term));
    let Some(ref p) = pattern else {
        return Ok(Json(json!({ "SearchHints": [], "TotalRecordCount": 0 })));
    };

    let limit = q.limit.unwrap_or(20).clamp(1, 50);
    // 条目与分集各占一部分配额
    let item_limit = (limit / 2).max(8);
    let ep_limit = limit.saturating_sub(item_limit).max(4);

    // 主条目：标题 / 原名 / 排序名 / TMDB ID / 关联目录名
    let item_search_predicate = main_item_search_predicate("$1", "$2");
    let items: Vec<HintRow> = sqlx::query_as(&format!(
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
        WHERE EXISTS (SELECT 1 FROM sync_folders f WHERE f.work_id = m.id AND f.is_ignored = FALSE)
          {item_search_predicate}
          AND ($4::text[] IS NULL OR m.category = ANY($4))
        ORDER BY m.rating DESC, m.year DESC NULLS LAST, m.id DESC
        LIMIT $3
        "#,
        item_search_predicate = item_search_predicate,
    ))
    .bind(p)
    .bind(tmdb)
    .bind(item_limit)
    .bind(&allowed_categories)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // 分集：TMDB 分集标题 / 本地文件名 / 剧名（1~2 字符短查询退化为顺序扫描，语义不受影响）
    let episodes: Vec<HintRow> = sqlx::query_as(
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
        WHERE EXISTS (
            SELECT 1 FROM sync_files sf
            JOIN sync_folders f ON f.id = sf.folder_id
            WHERE f.work_id = m.id AND f.is_ignored = FALSE
              AND sf.season_number = we.season_number AND sf.episode_number = we.episode_number
        )
          AND ($3::text[] IS NULL OR m.category = ANY($3))
          AND (we.title ILIKE $1
               OR m.title ILIKE $1 OR m.original_title ILIKE $1
               OR EXISTS (
                    SELECT 1 FROM sync_files sf
                    JOIN sync_folders f ON f.id = sf.folder_id
                    WHERE f.work_id = m.id AND f.is_ignored = FALSE
                      AND sf.season_number = we.season_number AND sf.episode_number = we.episode_number
                      AND sf.file_name ILIKE $1
               ))
        ORDER BY m.rating DESC, we.season_number ASC, we.episode_number ASC
        LIMIT $2
        "#,
    )
    .bind(p)
    .bind(ep_limit)
    .bind(&allowed_categories)
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
                "PrimaryImageTag": if r.poster_path.is_some() { "v1" } else { "" },
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
                "PrimaryImageTag": if r.poster_path.is_some() { "v1" } else { "" },
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

fn main_item_search_predicate(pattern_param: &str, tmdb_param: &str) -> String {
    format!(
        "AND (m.title ILIKE {pattern_param} OR m.original_title ILIKE {pattern_param} OR m.sort_title ILIKE {pattern_param}\n         OR ({tmdb_param}::int4 IS NOT NULL AND m.tmdb_id = {tmdb_param})\n         OR EXISTS (SELECT 1 FROM sync_folders sf2 WHERE sf2.work_id = m.id\n                    AND (sf2.clean_title ILIKE {pattern_param} OR sf2.raw_folder_name ILIKE {pattern_param})))"
    )
}

#[cfg(test)]
mod tests {
    use super::main_item_search_predicate;

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
