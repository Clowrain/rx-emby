use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct ScrapedMetadata {
    pub media_type: String, // "movie" | "tv"
    pub tmdb_id: i32,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub premiered_at: Option<String>,
    pub overview: Option<String>,
    pub rating: f32,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub countries: Vec<String>,
    pub networks: Vec<String>,
    pub genres: Vec<String>,
    pub extra: TmdbExtraMetadata,
    pub category: String, // 电影 / 电视剧 / 动漫 / 纪录片 / 综艺
}

#[derive(Deserialize, Debug)]
struct GenreObj {
    id: i32,
    name: String,
}

#[derive(Deserialize, Debug)]
struct CountryObj {
    iso_3166_1: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TmdbNamedId { pub id: i32, pub name: String }

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TmdbWatchProviderEntry {
    pub region: String,
    #[serde(rename = "type")]
    pub mon_type: String,
    pub id: i32,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TmdbCertificationEntry {
    pub region: String,
    pub rating: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TmdbReleaseDateEntry {
    pub region: String,
    pub date: String,
    #[serde(rename = "type")]
    pub release_type: i32,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct TmdbExtraMetadata {
    pub original_language: Option<String>,
    pub vote_count: Option<i32>,
    pub popularity: Option<f64>,
    pub runtime: Option<i32>,
    pub episode_run_time: Vec<i32>,
    pub status: Option<String>,
    pub in_production: Option<bool>,
    #[serde(alias = "type")]
    pub tv_type: Option<String>,
    pub number_of_seasons: Option<i32>,
    pub number_of_episodes: Option<i32>,
    pub last_air_date: Option<String>,
    pub last_episode_to_air: Option<serde_json::Value>,
    pub next_episode_to_air: Option<serde_json::Value>,
    pub production_companies: Vec<TmdbNamedId>,
    #[serde(alias = "belongs_to_collection")]
    pub collection: Option<TmdbNamedId>,
    pub networks: Vec<TmdbNamedId>,
    pub genre_ids: Vec<i32>,
    pub spoken_language_codes: Vec<String>,
    pub keyword_items: Vec<TmdbNamedId>,
    pub directors: Vec<TmdbNamedId>,
    pub cast: Vec<TmdbNamedId>,
    pub watch_providers: Vec<TmdbWatchProviderEntry>,
    pub certifications: Vec<TmdbCertificationEntry>,
    pub release_stages: Vec<TmdbReleaseDateEntry>,
}

#[derive(Deserialize, Debug, Default)]
struct RawSpokenLanguage {
    iso_639_1: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct RawKeywordsWrap {
    #[serde(default)]
    keywords: Vec<TmdbNamedId>,
    #[serde(default)]
    results: Vec<TmdbNamedId>,
}

#[derive(Deserialize, Debug, Default)]
struct RawCreditPerson {
    id: i32,
    #[serde(default)]
    name: String,
    job: Option<String>,
    #[serde(default)]
    jobs: Vec<RawCreditJob>,
}

#[derive(Deserialize, Debug, Default)]
struct RawCreditJob {
    job: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct RawCreditsWrap {
    #[serde(default)]
    cast: Vec<RawCreditPerson>,
    #[serde(default)]
    crew: Vec<RawCreditPerson>,
}

#[derive(Deserialize, Debug, Default)]
struct RawProviderObj {
    provider_id: i32,
    provider_name: String,
}

#[derive(Deserialize, Debug, Default)]
struct RawProviderRegion {
    #[serde(default)]
    flatrate: Vec<RawProviderObj>,
    #[serde(default)]
    rent: Vec<RawProviderObj>,
    #[serde(default)]
    buy: Vec<RawProviderObj>,
}

#[derive(Deserialize, Debug, Default)]
struct RawWatchProvidersWrap {
    #[serde(default)]
    results: HashMap<String, RawProviderRegion>,
}

#[derive(Deserialize, Debug, Default)]
struct RawMovieReleaseItem {
    certification: Option<String>,
    release_date: Option<String>,
    #[serde(rename = "type")]
    release_type: Option<i32>,
}

#[derive(Deserialize, Debug, Default)]
struct RawMovieReleaseCountry {
    iso_3166_1: String,
    #[serde(default)]
    release_dates: Vec<RawMovieReleaseItem>,
}

#[derive(Deserialize, Debug, Default)]
struct RawMovieReleaseDatesWrap {
    #[serde(default)]
    results: Vec<RawMovieReleaseCountry>,
}

#[derive(Deserialize, Debug, Default)]
struct RawTvRatingItem {
    iso_3166_1: String,
    rating: String,
}

#[derive(Deserialize, Debug, Default)]
struct RawTvContentRatingsWrap {
    #[serde(default)]
    results: Vec<RawTvRatingItem>,
}

#[derive(Deserialize, Debug)]
struct MovieDetailResp {
    id: i32,
    title: String,
    original_title: Option<String>,
    release_date: Option<String>,
    overview: Option<String>,
    vote_average: Option<f32>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    #[serde(default)]
    genres: Vec<GenreObj>,
    #[serde(default)]
    production_countries: Vec<CountryObj>,
    #[serde(default)]
    spoken_languages: Vec<RawSpokenLanguage>,
    #[serde(default)]
    keywords: Option<RawKeywordsWrap>,
    #[serde(default)]
    credits: Option<RawCreditsWrap>,
    #[serde(rename = "watch/providers", default)]
    watch_providers: Option<RawWatchProvidersWrap>,
    #[serde(default)]
    release_dates: Option<RawMovieReleaseDatesWrap>,
    #[serde(flatten)]
    extra: TmdbExtraMetadata,
}

#[derive(Deserialize, Debug)]
struct TvDetailResp {
    id: i32,
    name: String,
    original_name: Option<String>,
    first_air_date: Option<String>,
    overview: Option<String>,
    vote_average: Option<f32>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    #[serde(default)]
    genres: Vec<GenreObj>,
    #[serde(default)]
    origin_country: Vec<String>,
    #[serde(default)]
    spoken_languages: Vec<RawSpokenLanguage>,
    #[serde(default)]
    created_by: Vec<TmdbNamedId>,
    #[serde(default)]
    keywords: Option<RawKeywordsWrap>,
    #[serde(default)]
    aggregate_credits: Option<RawCreditsWrap>,
    #[serde(rename = "watch/providers", default)]
    watch_providers: Option<RawWatchProvidersWrap>,
    #[serde(default)]
    content_ratings: Option<RawTvContentRatingsWrap>,
    #[serde(flatten)]
    extra: TmdbExtraMetadata,
}

#[derive(Deserialize, Debug)]
struct MultiSearchResp {
    #[serde(default)]
    results: Vec<MultiSearchItem>,
}

#[derive(Deserialize, Debug)]
struct MultiSearchItem {
    id: i32,
    media_type: String,
    title: Option<String>,
    name: Option<String>,
    release_date: Option<String>,
    first_air_date: Option<String>,
    popularity: Option<f64>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TmdbSeasonDetailResp {
    pub name: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub air_date: Option<String>,
    #[serde(default)]
    pub episodes: Vec<TmdbEpisodeDetailResp>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TmdbEpisodeDetailResp {
    pub episode_number: i32,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub still_path: Option<String>,
    pub air_date: Option<String>,
    pub runtime: Option<i32>,
}

impl AppState {
    /// 智能刮削入口：支持已知 TMDB ID 双向探测，以及无 ID 混合搜索匹配
    pub async fn scrape_tmdb_metadata(
        &self,
        known_tmdb_id: Option<i32>,
        inferred_type: &str,
        clean_title: &str,
        year: Option<i32>,
    ) -> Result<ScrapedMetadata> {
        if let Some(tid) = known_tmdb_id {
            let primary = if inferred_type == "movie" { "movie" } else { "tv" };
            let secondary = if primary == "movie" { "tv" } else { "movie" };

            if let Ok(meta) = self.fetch_tmdb_by_id(tid, primary).await {
                // 若年份偏差过大（> 2年），尝试反转类型探测
                if let (Some(y1), Some(y2)) = (year, meta.year) {
                    if (y1 - y2).abs() > 2 {
                        if let Ok(alt_meta) = self.fetch_tmdb_by_id(tid, secondary).await {
                            if alt_meta.year.map(|ay| (y1 - ay).abs() <= 2).unwrap_or(false) {
                                return Ok(alt_meta);
                            }
                        }
                    }
                }
                return Ok(meta);
            }

            // 第一顺位返回 404 或失败，自动反转类型探测另一端接口
            return self.fetch_tmdb_by_id(tid, secondary).await;
        }

        // 无 TMDB ID：调用 /search/multi 搜索最佳匹配
        self.search_and_fetch_tmdb(clean_title, year, inferred_type)
            .await
    }

    async fn fetch_tmdb_by_id(&self, tmdb_id: i32, media_type: &str) -> Result<ScrapedMetadata> {
        self.scraper.limiter.load().until_ready().await;

        let cfg = self.config.load();
        let client = self.tmdb_client.load();
        let append = if media_type == "movie" {
            "keywords,credits,watch/providers,release_dates"
        } else {
            "keywords,aggregate_credits,watch/providers,content_ratings"
        };
        let url = format!(
            "{}/{}/{}?api_key={}&language=zh-CN&append_to_response={}",
            cfg.tmdb_api_base, media_type, tmdb_id, cfg.tmdb_api_key, append
        );

        let resp = client.get(&url).send().await.context("请求 TMDB 详情失败")?;
        if !resp.status().is_success() {
            return Err(anyhow!("TMDB 返回非成功状态码: {}", resp.status()));
        }

        if media_type == "movie" {
            let mut m: MovieDetailResp = resp.json().await.context("解析 TMDB Movie JSON 失败")?;
            let year = parse_year_from_date(m.release_date.as_deref());
            let genre_ids: Vec<i32> = m.genres.iter().map(|g| g.id).collect();
            let genres: Vec<String> = m.genres.into_iter().map(|g| g.name).collect();
            let countries: Vec<String> = m
                .production_countries
                .into_iter()
                .map(|c| c.iso_3166_1.to_uppercase())
                .collect();
            let category = deduce_core_category("movie", &genre_ids);
            m.extra.genre_ids = genre_ids;
            m.extra.spoken_language_codes = extract_spoken_languages(&m.spoken_languages);
            if let Some(kw) = m.keywords {
                m.extra.keyword_items = kw.keywords;
            }
            if let Some(cr) = m.credits {
                m.extra.directors = extract_directors(&cr.crew, &[]);
                m.extra.cast = extract_top_cast(&cr.cast);
            }
            if let Some(wp) = m.watch_providers {
                m.extra.watch_providers = extract_watch_providers(wp);
            }
            if let Some(rd) = m.release_dates {
                let (certs, stages) = extract_movie_release_dates(rd);
                m.extra.certifications = certs;
                m.extra.release_stages = stages;
            }

            Ok(ScrapedMetadata {
                media_type: "movie".to_string(),
                tmdb_id: m.id,
                title: m.title,
                original_title: m.original_title,
                year,
                premiered_at: m.release_date,
                overview: m.overview,
                rating: m.vote_average.unwrap_or(0.0),
                poster_path: m.poster_path,
                backdrop_path: m.backdrop_path,
                countries,
                networks: vec![],
                extra: m.extra,
                genres,
                category,
            })
        } else {
            let mut t: TvDetailResp = resp.json().await.context("解析 TMDB TV JSON 失败")?;
            let year = parse_year_from_date(t.first_air_date.as_deref());
            let genre_ids: Vec<i32> = t.genres.iter().map(|g| g.id).collect();
            let genres: Vec<String> = t.genres.into_iter().map(|g| g.name).collect();
            let countries: Vec<String> = t
                .origin_country
                .into_iter()
                .map(|iso| iso.to_uppercase())
                .collect();
            let networks: Vec<String> = t.extra.networks.iter().map(|n| n.name.clone()).collect();
            let category = deduce_core_category("tv", &genre_ids);
            t.extra.genre_ids = genre_ids;
            t.extra.spoken_language_codes = extract_spoken_languages(&t.spoken_languages);
            if let Some(kw) = t.keywords {
                t.extra.keyword_items = kw.results;
            }
            let crew = t.aggregate_credits.as_ref().map(|c| c.crew.as_slice()).unwrap_or(&[]);
            t.extra.directors = extract_directors(crew, &t.created_by);
            if let Some(cr) = t.aggregate_credits {
                t.extra.cast = extract_top_cast(&cr.cast);
            }
            if let Some(wp) = t.watch_providers {
                t.extra.watch_providers = extract_watch_providers(wp);
            }
            if let Some(cr) = t.content_ratings {
                t.extra.certifications = extract_tv_certifications(cr);
            }

            Ok(ScrapedMetadata {
                media_type: "tv".to_string(),
                tmdb_id: t.id,
                title: t.name,
                original_title: t.original_name,
                year,
                premiered_at: t.first_air_date,
                overview: t.overview,
                rating: t.vote_average.unwrap_or(0.0),
                poster_path: t.poster_path,
                backdrop_path: t.backdrop_path,
                countries,
                networks,
                extra: t.extra,
                genres,
                category,
            })
        }
    }

    async fn search_and_fetch_tmdb(
        &self,
        query: &str,
        target_year: Option<i32>,
        preferred_type: &str,
    ) -> Result<ScrapedMetadata> {
        self.scraper.limiter.load().until_ready().await;

        let cfg = self.config.load();
        let client = self.tmdb_client.load();
        let url = format!(
            "{}/search/multi?api_key={}&language=zh-CN&query={}",
            cfg.tmdb_api_base,
            cfg.tmdb_api_key,
            percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC)
        );

        let resp: MultiSearchResp = client
            .get(&url)
            .send()
            .await
            .context("请求 TMDB /search/multi 失败")?
            .error_for_status()?
            .json()
            .await?;

        // 筛选 movie 或 tv 结果并按年份接近度 + 类型匹配度 + 热度打分
        let best = resp
            .results
            .into_iter()
            .filter(|item| item.media_type == "movie" || item.media_type == "tv")
            .max_by_key(|item| {
                let mut score: i64 = 0;
                if item.media_type == preferred_type {
                    score += 500;
                }
                let item_date = item.release_date.as_deref().or(item.first_air_date.as_deref());
                if let (Some(ty), Some(iy)) = (target_year, parse_year_from_date(item_date)) {
                    let diff = (ty - iy).abs();
                    if diff == 0 {
                        score += 1000;
                    } else if diff == 1 {
                        score += 400;
                    } else {
                        score -= (diff as i64) * 200;
                    }
                }
                let title_str = item.title.as_deref().or(item.name.as_deref()).unwrap_or("");
                if title_str == query {
                    score += 800;
                }
                score += item.popularity.unwrap_or(0.0) as i64;
                score
            })
            .ok_or_else(|| anyhow!("TMDB 未搜索到匹配条目: {}", query))?;

        self.fetch_tmdb_by_id(best.id, &best.media_type).await
    }

    /// 拉取电视剧某季及其下全部单集的元数据（`GET /tv/{tmdb_id}/season/{season_number}`）
    /// 若 TMDB 返回 404（无此季），返回 `Ok(None)` 以便标记跳过避免死循环重试
    pub async fn fetch_tmdb_season(
        &self,
        tmdb_id: i32,
        season_number: i32,
    ) -> Result<Option<TmdbSeasonDetailResp>> {
        self.scraper.limiter.load().until_ready().await;

        let cfg = self.config.load();
        let client = self.tmdb_client.load();
        let url = format!(
            "{}/tv/{}/season/{}?api_key={}&language=zh-CN",
            cfg.tmdb_api_base, tmdb_id, season_number, cfg.tmdb_api_key
        );

        let resp = client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("请求 TMDB 季详情失败: tv={} S{:02}", tmdb_id, season_number))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(anyhow!(
                "TMDB 季详情返回非成功状态码 ({}): tv={} S{:02}",
                resp.status(),
                tmdb_id,
                season_number
            ));
        }

        let mut season_data: TmdbSeasonDetailResp = resp
            .json()
            .await
            .with_context(|| format!("解析 TMDB Season JSON 失败: tv={} S{:02}", tmdb_id, season_number))?;

        // 清洗空字符串字段为 None
        season_data.name = season_data.name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        season_data.overview = season_data.overview.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        season_data.poster_path = season_data.poster_path.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        season_data.air_date = season_data.air_date.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

        for ep in &mut season_data.episodes {
            ep.name = ep.name.take().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            ep.overview = ep.overview.take().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            ep.still_path = ep.still_path.take().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            ep.air_date = ep.air_date.take().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        }

        Ok(Some(season_data))
    }
}

fn extract_spoken_languages(langs: &[RawSpokenLanguage]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for l in langs {
        if let Some(code) = l.iso_639_1.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let low = code.to_lowercase();
            if seen.insert(low.clone()) {
                out.push(low);
            }
        }
    }
    out
}

fn extract_directors(crew: &[RawCreditPerson], creators: &[TmdbNamedId]) -> Vec<TmdbNamedId> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for c in creators {
        if seen.insert(c.id) {
            out.push(c.clone());
        }
    }
    for p in crew {
        let is_dir = p.job.as_deref() == Some("Director")
            || p.jobs.iter().any(|j| j.job.as_deref() == Some("Director"));
        if is_dir && !p.name.trim().is_empty() && seen.insert(p.id) {
            out.push(TmdbNamedId {
                id: p.id,
                name: p.name.clone(),
            });
            if out.len() >= 10 {
                break;
            }
        }
    }
    out
}

fn extract_top_cast(cast: &[RawCreditPerson]) -> Vec<TmdbNamedId> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for p in cast {
        if !p.name.trim().is_empty() && seen.insert(p.id) {
            out.push(TmdbNamedId {
                id: p.id,
                name: p.name.clone(),
            });
            if out.len() >= 20 {
                break;
            }
        }
    }
    out
}

fn extract_watch_providers(wp: RawWatchProvidersWrap) -> Vec<TmdbWatchProviderEntry> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (region, reg_data) in wp.results {
        let reg = region.trim().to_uppercase();
        if reg.is_empty() {
            continue;
        }
        for (mon_type, list) in [
            ("flatrate", reg_data.flatrate),
            ("rent", reg_data.rent),
            ("buy", reg_data.buy),
        ] {
            for p in list {
                if seen.insert((reg.clone(), mon_type, p.provider_id)) {
                    out.push(TmdbWatchProviderEntry {
                        region: reg.clone(),
                        mon_type: mon_type.to_string(),
                        id: p.provider_id,
                        name: p.provider_name,
                    });
                }
            }
        }
    }
    out
}

fn extract_movie_release_dates(
    rd: RawMovieReleaseDatesWrap,
) -> (Vec<TmdbCertificationEntry>, Vec<TmdbReleaseDateEntry>) {
    let mut cert_seen = HashSet::new();
    let mut certs = Vec::new();
    let mut stage_seen = HashSet::new();
    let mut stages = Vec::new();

    for c in rd.results {
        let reg = c.iso_3166_1.trim().to_uppercase();
        if reg.is_empty() {
            continue;
        }
        for item in c.release_dates {
            if let Some(cert) = item.certification.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                if cert_seen.insert((reg.clone(), cert.to_string())) {
                    certs.push(TmdbCertificationEntry {
                        region: reg.clone(),
                        rating: cert.to_string(),
                    });
                }
            }
            if let (Some(rtype), Some(date_str)) = (item.release_type, item.release_date.as_deref()) {
                if (rtype == 3 || rtype == 4) && date_str.len() >= 10 {
                    let ymd = &date_str[..10];
                    if stage_seen.insert((reg.clone(), rtype, ymd.to_string())) {
                        stages.push(TmdbReleaseDateEntry {
                            region: reg.clone(),
                            date: ymd.to_string(),
                            release_type: rtype,
                        });
                    }
                }
            }
        }
    }
    (certs, stages)
}

fn extract_tv_certifications(cr: RawTvContentRatingsWrap) -> Vec<TmdbCertificationEntry> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in cr.results {
        let reg = item.iso_3166_1.trim().to_uppercase();
        let rating = item.rating.trim();
        if !reg.is_empty() && !rating.is_empty() && seen.insert((reg.clone(), rating.to_string())) {
            out.push(TmdbCertificationEntry {
                region: reg,
                rating: rating.to_string(),
            });
        }
    }
    out
}

fn parse_year_from_date(date_str: Option<&str>) -> Option<i32> {
    date_str
        .and_then(|d| d.get(0..4))
        .and_then(|y| y.parse::<i32>().ok())
}

/// 权威元数据反推 5 大核心业务类：电影 / 电视剧 / 动漫 / 纪录片 / 综艺
fn deduce_core_category(media_type: &str, genre_ids: &[i32]) -> String {
    if genre_ids.contains(&16) {
        return "动漫".to_string();
    }
    if genre_ids.contains(&99) {
        return "纪录片".to_string();
    }
    if genre_ids.contains(&10764) || genre_ids.contains(&10767) {
        return "综艺".to_string();
    }
    if media_type == "movie" {
        "电影".to_string()
    } else {
        "电视剧".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detail_fields_survive_deserialization() {
        let movie: MovieDetailResp = serde_json::from_value(serde_json::json!({
            "id":1,"title":"测试电影","original_language":"ja","vote_count":1200,"runtime":88,
            "genres":[{"id":16,"name":"动画"}],"belongs_to_collection":{"id":2,"name":"系列"},
            "production_companies":[{"id":3,"name":"制作公司"}]
        })).unwrap();
        assert_eq!(movie.extra.vote_count,Some(1200));
        assert_eq!(movie.extra.collection.unwrap().id,2);
        let tv: TvDetailResp = serde_json::from_value(serde_json::json!({
            "id":2,"name":"测试剧","type":"Miniseries","status":"Ended",
            "networks":[{"id":2552,"name":"Apple TV"}],"number_of_episodes":8
        })).unwrap();
        assert_eq!(tv.extra.tv_type.as_deref(),Some("Miniseries"));
        assert_eq!(tv.extra.networks[0].id,2552);
        assert_eq!(tv.extra.number_of_episodes,Some(8));

        let rich_movie: MovieDetailResp = serde_json::from_value(serde_json::json!({
            "id": 10, "title": "时空之旅", "spoken_languages": [{"iso_639_1": "ja"}, {"iso_639_1": "en"}],
            "keywords": {"keywords": [{"id": 4379, "name": "time travel"}]},
            "credits": {
                "cast": [{"id": 101, "name": "演员A"}],
                "crew": [{"id": 201, "name": "导演B", "job": "Director"}]
            },
            "watch/providers": {"results": {"US": {"flatrate": [{"provider_id": 8, "provider_name": "Netflix"}]}}},
            "release_dates": {"results": [{"iso_3166_1": "US", "release_dates": [{"certification": "PG-13", "release_date": "2026-10-01T00:00:00.000Z", "type": 4}]}]}
        })).unwrap();
        assert_eq!(extract_spoken_languages(&rich_movie.spoken_languages), vec!["ja", "en"]);
        assert_eq!(rich_movie.keywords.unwrap().keywords[0].id, 4379);
        let cr = rich_movie.credits.unwrap();
        assert_eq!(extract_directors(&cr.crew, &[])[0].id, 201);
        assert_eq!(extract_top_cast(&cr.cast)[0].id, 101);
        let wp = extract_watch_providers(rich_movie.watch_providers.unwrap());
        assert_eq!(wp[0].region, "US");
        assert_eq!(wp[0].id, 8);
        let (certs, stages) = extract_movie_release_dates(rich_movie.release_dates.unwrap());
        assert_eq!(certs[0].rating, "PG-13");
        assert_eq!(stages[0].release_type, 4);
        assert_eq!(stages[0].date, "2026-10-01");
    }
}
