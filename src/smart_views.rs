//! 智能视图的规则契约。计数、内容与封面共用参数化 SQL 条件。
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VirtualLibraryRule {
    pub media_type: Option<String>,
    pub sort_by: Option<String>,
    pub categories: Vec<String>,
    pub countries: Vec<String>,
    pub networks: Vec<String>,
    pub genres: Vec<String>,
    pub year_min: Option<i32>,
    pub year_max: Option<i32>,
    pub min_rating: Option<f32>,
    pub max_rating: Option<f32>,
    pub keyword: Option<String>,
    pub search_overview: bool,
    pub languages: Vec<String>,
    pub min_votes: Option<i32>,
    pub runtime_min: Option<i32>,
    pub runtime_max: Option<i32>,
    pub statuses: Vec<String>,
    pub tv_types: Vec<String>,
    pub genre_ids: Vec<i32>,
    pub network_ids: Vec<i32>,
    pub company_ids: Vec<i32>,
    pub collection_ids: Vec<i32>,
    pub has_collection: Option<bool>,
    pub seasons_min: Option<i32>,
    pub seasons_max: Option<i32>,
    pub episodes_min: Option<i32>,
    pub episodes_max: Option<i32>,
    pub min_popularity: Option<f64>,
    pub premiered_after: Option<String>,
    pub premiered_before: Option<String>,
    pub premiered_within_days: Option<i32>,
    pub added_within_days: Option<i32>,
    pub genres_all: bool,
    pub exclude_countries: Vec<String>,
    pub exclude_genres: Vec<String>,
    pub exclude_networks: Vec<String>,
    /// 多个标签须由同一物理源同时满足。
    pub version_tags: Vec<String>,
    /// 第二阶段 TMDB 扩展字段与衍生条件
    pub spoken_languages: Vec<String>,
    pub keyword_ids: Vec<i32>,
    pub director_ids: Vec<i32>,
    pub person_ids: Vec<i32>,
    pub watch_region: Option<String>,
    pub watch_provider_ids: Vec<i32>,
    pub watch_types: Vec<String>,
    pub cert_region: Option<String>,
    pub certifications: Vec<String>,
    pub digital_within_days: Option<i32>,
    pub airing_within_days: Option<i32>,
    pub aired_within_days: Option<i32>,
    pub local_complete: Option<String>,
    /// 当前组仍需全部满足；any 中至少一个子组满足。
    pub any: Vec<VirtualLibraryRule>,
}

impl VirtualLibraryRule {
    /// 仅把启用的条件编译为 SQL；所有用户值仍通过一个 JSON 参数绑定。
    /// param 由调用方固定为 $1 / $10 / $12，子组仅追加数组索引。
    pub fn sql_predicate(&self, param: &str) -> String {
        let r = format!("({})",param);
        let mut clauses = Vec::new();
        let mut add = |sql: String| clauses.push(sql);
        if self.media_type.as_deref().is_some_and(|v| !v.is_empty()) { add(format!("m.media_type = {r}->>'media_type'")); }
        for (field,enabled,column) in [
            ("categories",!self.categories.is_empty(),"m.category"),
            ("languages",!self.languages.is_empty(),"m.tmdb_metadata->>'original_language'"),
            ("statuses",!self.statuses.is_empty(),"m.tmdb_metadata->>'status'"),
            ("tv_types",!self.tv_types.is_empty(),"m.tmdb_metadata->>'tv_type'"),
        ] { if enabled { add(format!("{column} = ANY(smart_texts({r}->'{field}'))")); } }
        for (field,enabled,column,helper,negated) in [
            ("countries",!self.countries.is_empty(),"m.countries","smart_country_values",false),
            ("exclude_countries",!self.exclude_countries.is_empty(),"m.countries","smart_country_values",true),
            ("networks",!self.networks.is_empty(),"m.networks","smart_network_values",false),
            ("exclude_networks",!self.exclude_networks.is_empty(),"m.networks","smart_network_values",true),
            ("exclude_genres",!self.exclude_genres.is_empty(),"m.genres","smart_texts",true),
        ] { if enabled { add(format!("{}({column} && {helper}({r}->'{field}'))",if negated { "NOT " } else { "" })); } }
        let genre_op = if self.genres_all { "@>" } else { "&&" };
        if !self.genres.is_empty() { add(format!("m.genres {genre_op} smart_texts({r}->'genres')")); }
        if !self.genre_ids.is_empty() { add(format!("smart_texts(m.tmdb_metadata->'genre_ids') {genre_op} smart_texts({r}->'genre_ids')")); }
        for (field,enabled,column,op,cast) in [
            ("year_min",self.year_min.is_some(),"m.year",">=","int"),
            ("year_max",self.year_max.is_some(),"m.year","<=","int"),
            ("min_rating",self.min_rating.is_some(),"m.rating",">=","real"),
            ("max_rating",self.max_rating.is_some(),"m.rating","<=","real"),
            ("min_votes",self.min_votes.is_some(),"(m.tmdb_metadata->>'vote_count')::int",">=","int"),
            ("min_popularity",self.min_popularity.is_some(),"(m.tmdb_metadata->>'popularity')::float8",">=","float8"),
            ("seasons_min",self.seasons_min.is_some(),"(m.tmdb_metadata->>'number_of_seasons')::int",">=","int"),
            ("seasons_max",self.seasons_max.is_some(),"(m.tmdb_metadata->>'number_of_seasons')::int","<=","int"),
            ("episodes_min",self.episodes_min.is_some(),"(m.tmdb_metadata->>'number_of_episodes')::int",">=","int"),
            ("episodes_max",self.episodes_max.is_some(),"(m.tmdb_metadata->>'number_of_episodes')::int","<=","int"),
            ("premiered_after",self.premiered_after.is_some(),"NULLIF(m.premiered_at,'')",">=","text"),
            ("premiered_before",self.premiered_before.is_some(),"NULLIF(m.premiered_at,'')","<=","text"),
        ] { if enabled { add(format!("{column} {op} ({r}->>'{field}')::{cast}")); } }
        if self.keyword.as_deref().is_some_and(|s|!s.trim().is_empty()) {
            let overview = if self.search_overview { ",m.overview" } else { "" };
            add(format!("position(lower(trim({r}->>'keyword')) in lower(concat_ws(' ',m.title,m.original_title,m.root_dir{overview}))) > 0"));
        }
        for (field,enabled,column) in [
            ("network_ids",!self.network_ids.is_empty(),"networks"),
            ("company_ids",!self.company_ids.is_empty(),"production_companies"),
        ] { if enabled { add(format!("EXISTS(SELECT 1 FROM jsonb_array_elements(COALESCE(m.tmdb_metadata->'{column}','[]')) v WHERE v->>'id'=ANY(smart_texts({r}->'{field}')))")); } }
        if !self.collection_ids.is_empty() { add(format!("m.tmdb_metadata->'collection'->>'id'=ANY(smart_texts({r}->'collection_ids'))")); }
        if self.has_collection.is_some() { add(format!("m.media_type='movie' AND m.metadata_version>=1 AND ((m.tmdb_metadata->'collection'->>'id' IS NOT NULL)=({r}->>'has_collection')::boolean)")); }
        if self.runtime_min.is_some() || self.runtime_max.is_some() {
            let mut ranges=vec!["t::int > 0".to_string()];
            if self.runtime_min.is_some() { ranges.push(format!("t::int>=({r}->>'runtime_min')::int")); }
            if self.runtime_max.is_some() { ranges.push(format!("t::int<=({r}->>'runtime_max')::int")); }
            add(format!("EXISTS(SELECT 1 FROM unnest(CASE WHEN m.media_type='movie' THEN ARRAY[m.tmdb_metadata->>'runtime'] ELSE smart_texts(m.tmdb_metadata->'episode_run_time') END) t WHERE {})",ranges.join(" AND ")));
        }
        let today="(CURRENT_TIMESTAMP AT TIME ZONE 'Asia/Shanghai')::date";
        if self.premiered_within_days.is_some() { add(format!("NULLIF(m.premiered_at,'') BETWEEN ({today}-(({r}->>'premiered_within_days')::int-1))::text AND {today}::text")); }
        if self.added_within_days.is_some() { add(format!("(m.created_at AT TIME ZONE 'Asia/Shanghai')::date BETWEEN {today}-(({r}->>'added_within_days')::int-1) AND {today}")); }
        if !self.version_tags.is_empty() {
            add(format!("EXISTS(SELECT 1 FROM episodes e WHERE e.media_item_id=m.id AND regexp_split_to_array(e.version_tag, '\\s*·\\s*') @> smart_texts({r}->'version_tags'))"));
        }
        if !self.spoken_languages.is_empty() {
            add(format!("smart_texts(m.tmdb_metadata->'spoken_language_codes') && smart_texts({r}->'spoken_languages')"));
        }
        for (field,column,enabled) in [
            ("keyword_ids","keyword_items",!self.keyword_ids.is_empty()),
            ("director_ids","directors",!self.director_ids.is_empty()),
        ] {
            if enabled {
                add(format!("EXISTS(SELECT 1 FROM jsonb_array_elements(COALESCE(m.tmdb_metadata->'{column}','[]')) v WHERE v->>'id'=ANY(smart_texts({r}->'{field}')))"));
            }
        }
        if !self.person_ids.is_empty() {
            add(format!("EXISTS(SELECT 1 FROM jsonb_array_elements(COALESCE(m.tmdb_metadata->'directors','[]') || COALESCE(m.tmdb_metadata->'cast','[]')) v WHERE v->>'id'=ANY(smart_texts({r}->'person_ids')))"));
        }
        if !self.watch_provider_ids.is_empty() {
            let mut conds = vec![format!("p->>'id'=ANY(smart_texts({r}->'watch_provider_ids'))")];
            if self.watch_region.as_deref().is_some_and(|s| !s.trim().is_empty()) {
                conds.push(format!("p->>'region'=upper(trim({r}->>'watch_region'))"));
            }
            if !self.watch_types.is_empty() {
                conds.push(format!("p->>'type'=ANY(smart_texts({r}->'watch_types'))"));
            }
            add(format!("EXISTS(SELECT 1 FROM jsonb_array_elements(COALESCE(m.tmdb_metadata->'watch_providers','[]')) p WHERE {})", conds.join(" AND ")));
        }
        if !self.certifications.is_empty() {
            let mut conds = vec![format!("c->>'rating'=ANY(smart_texts({r}->'certifications'))")];
            if self.cert_region.as_deref().is_some_and(|s| !s.trim().is_empty()) {
                conds.push(format!("c->>'region'=upper(trim({r}->>'cert_region'))"));
            }
            add(format!("EXISTS(SELECT 1 FROM jsonb_array_elements(COALESCE(m.tmdb_metadata->'certifications','[]')) c WHERE {})", conds.join(" AND ")));
        }
        if self.digital_within_days.is_some() {
            let region_cond = if self.cert_region.as_deref().is_some_and(|s| !s.trim().is_empty()) {
                format!(" AND s->>'region'=upper(trim({r}->>'cert_region'))")
            } else {
                String::new()
            };
            add(format!("m.media_type='movie' AND EXISTS(SELECT 1 FROM jsonb_array_elements(COALESCE(m.tmdb_metadata->'release_stages','[]')) s WHERE (s->>'type')::int=4{region_cond} AND NULLIF(s->>'date','') BETWEEN ({today}-(({r}->>'digital_within_days')::int-1))::text AND {today}::text)"));
        }
        if self.airing_within_days.is_some() {
            add(format!("m.media_type='tv' AND NULLIF(m.tmdb_metadata->'next_episode_to_air'->>'air_date','') BETWEEN {today}::text AND ({today}+(({r}->>'airing_within_days')::int))::text"));
        }
        if self.aired_within_days.is_some() {
            add(format!("m.media_type='tv' AND NULLIF(m.tmdb_metadata->>'last_air_date','') BETWEEN ({today}-(({r}->>'aired_within_days')::int-1))::text AND {today}::text"));
        }
        if let Some(lc) = self.local_complete.as_deref().filter(|s| !s.is_empty()) {
            if lc == "all" {
                add("m.media_type='tv' AND m.tmdb_metadata->>'status'='Ended' AND COALESCE((m.tmdb_metadata->>'number_of_episodes')::int,0)>0 AND (SELECT COUNT(DISTINCT (e.season_number, e.episode_number)) FROM episodes e WHERE e.media_item_id=m.id AND e.season_number>0)>=(m.tmdb_metadata->>'number_of_episodes')::int".into());
            } else if lc == "aired" {
                add("m.media_type='tv' AND COALESCE((SELECT SUM(s.episode_count) FROM seasons s WHERE s.media_item_id=m.id AND s.season_number>0 AND s.episode_count>0),0)>0 AND (SELECT COUNT(DISTINCT (e.season_number, e.episode_number)) FROM episodes e WHERE e.media_item_id=m.id AND e.season_number>0)>=(SELECT COALESCE(SUM(s.episode_count),0) FROM seasons s WHERE s.media_item_id=m.id AND s.season_number>0 AND s.episode_count>0)".into());
            }
        }
        if !self.any.is_empty() {
            let children=self.any.iter().enumerate().map(|(i,child)|child.sql_predicate(&format!("{r}->'any'->{i}"))).collect::<Vec<_>>();
            add(format!("({})",children.join(" OR ")));
        }
        if clauses.is_empty() { "TRUE".into() } else { format!("({})",clauses.join(" AND ")) }
    }

    pub fn validate(&self) -> Result<(), String> {
        self.validate_depth(0)
    }

    fn validate_depth(&self, depth: usize) -> Result<(), String> {
        if depth > 2 || self.any.len() > 8 {
            return Err("条件分组最多 3 层，每层最多 8 组".into());
        }
        if let Some(t) = &self.media_type {
            if !["", "movie", "tv"].contains(&t.as_str()) {
                return Err("媒体类型必须为电影或剧集".into());
            }
        }
        if self.sort_by.as_deref().is_some_and(|s| !["PremiereDate", "CommunityRating", "Popularity", "DateCreated", "SortName"].contains(&s)) {
            return Err("不支持的结果排序方式".into());
        }
        for value in [self.min_rating, self.max_rating].into_iter().flatten() {
            if !value.is_finite() || !(0.0..=10.0).contains(&value) {
                return Err("评分必须在 0～10 之间".into());
            }
        }
        if matches!((self.min_rating, self.max_rating), (Some(a), Some(b)) if a > b) {
            return Err("最低评分不能高于最高评分".into());
        }
        for (name, min, max) in [
            ("年份", self.year_min, self.year_max),
            ("时长", self.runtime_min, self.runtime_max),
            ("季数", self.seasons_min, self.seasons_max),
            ("集数", self.episodes_min, self.episodes_max),
        ] {
            if min.is_some_and(|v| v < 0) || max.is_some_and(|v| v < 0) || matches!((min,max),(Some(a),Some(b)) if a>b) {
                return Err(format!("{}范围无效：下限不能大于上限，且必须为非负整数", name));
            }
        }
        if self.min_votes.is_some_and(|v| v < 0)
            || self.min_popularity.is_some_and(|v| !v.is_finite() || v < 0.0)
            || self.premiered_within_days.is_some_and(|v| !(1..=36500).contains(&v))
            || self.added_within_days.is_some_and(|v| !(1..=36500).contains(&v))
            || self.digital_within_days.is_some_and(|v| !(1..=36500).contains(&v))
            || self.airing_within_days.is_some_and(|v| !(0..=365).contains(&v))
            || self.aired_within_days.is_some_and(|v| !(1..=36500).contains(&v)) {
            return Err("投票数与热度须为非负数；最近天数须在 1～36500 之间".into());
        }
        if self.local_complete.as_deref().is_some_and(|s| !["", "aired", "all"].contains(&s)) {
            return Err("本地收齐条件无效".into());
        }
        for ids in [&self.genre_ids, &self.network_ids, &self.company_ids, &self.collection_ids,
            &self.keyword_ids, &self.director_ids, &self.person_ids, &self.watch_provider_ids] {
            if ids.len() > 100 || ids.iter().any(|v| *v <= 0) {
                return Err("TMDB 标识必须为正整数，每项最多 100 个".into());
            }
        }
        for list in [&self.categories, &self.countries, &self.networks, &self.genres, &self.languages,
            &self.statuses, &self.tv_types, &self.exclude_countries, &self.exclude_genres, &self.exclude_networks,
            &self.version_tags, &self.spoken_languages, &self.watch_types, &self.certifications] {
            if list.len()>100 || list.iter().any(|v| v.trim().is_empty() || v.len()>255) {
                return Err("筛选列表每项不能为空或超过 255 字节，最多 100 项".into());
            }
        }
        for date in [&self.premiered_after, &self.premiered_before].into_iter().flatten() {
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| "日期格式须为 YYYY-MM-DD")?;
        }
        if matches!((&self.premiered_after,&self.premiered_before),(Some(a),Some(b)) if a>b) {
            return Err("开始日期不能晚于结束日期".into());
        }
        for child in &self.any { child.validate_depth(depth+1)?; }
        Ok(())
    }
}

pub fn can_access_view(cats: Option<&[String]>, id: &str, name: &str) -> bool {
    cats.is_none_or(|values| values.iter().any(|c| c == "*" || c == "vlib_*" || c == id || c == name))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_ranges_are_rejected() {
        for rule in [
            VirtualLibraryRule { min_rating: Some(11.0), ..Default::default() },
            VirtualLibraryRule { year_min: Some(2026), year_max: Some(2020), ..Default::default() },
            VirtualLibraryRule { runtime_min: Some(-1), ..Default::default() },
            VirtualLibraryRule { premiered_after: Some("invalid".into()), ..Default::default() },
        ] { assert!(rule.validate().is_err()); }
    }
    #[test]
    fn legacy_rules_and_explicit_view_grants() {
        let r: VirtualLibraryRule = serde_json::from_str(r#"{"countries":["CN"],"min_rating":8}"#).unwrap();
        assert!(r.validate().is_ok());
        assert!(can_access_view(Some(&["vlib_*".into()]),"lib_vlib_1","高分"));
        assert!(!can_access_view(Some(&["电视剧".into()]),"lib_vlib_1","高分"));
        assert!(can_access_view(Some(&["lib_vlib_1".into()]),"lib_vlib_1","高分"));
    }
}
