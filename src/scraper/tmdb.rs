use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

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

#[derive(Deserialize, Debug)]
struct NetworkObj {
    name: String,
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
    networks: Vec<NetworkObj>,
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
        let url = format!(
            "{}/{}/{}?api_key={}&language=zh-CN",
            cfg.tmdb_api_base, media_type, tmdb_id, cfg.tmdb_api_key
        );

        let resp = client.get(&url).send().await.context("请求 TMDB 详情失败")?;
        if !resp.status().is_success() {
            return Err(anyhow!("TMDB 返回非成功状态码: {}", resp.status()));
        }

        if media_type == "movie" {
            let m: MovieDetailResp = resp.json().await.context("解析 TMDB Movie JSON 失败")?;
            let year = parse_year_from_date(m.release_date.as_deref());
            let genre_ids: Vec<i32> = m.genres.iter().map(|g| g.id).collect();
            let genres: Vec<String> = m.genres.into_iter().map(|g| g.name).collect();
            let countries: Vec<String> = m
                .production_countries
                .into_iter()
                .map(|c| iso_country_to_cn(&c.iso_3166_1))
                .collect();
            let category = deduce_core_category("movie", &genre_ids);

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
                genres,
                category,
            })
        } else {
            let t: TvDetailResp = resp.json().await.context("解析 TMDB TV JSON 失败")?;
            let year = parse_year_from_date(t.first_air_date.as_deref());
            let genre_ids: Vec<i32> = t.genres.iter().map(|g| g.id).collect();
            let genres: Vec<String> = t.genres.into_iter().map(|g| g.name).collect();
            let countries: Vec<String> = t
                .origin_country
                .into_iter()
                .map(|iso| iso_country_to_cn(&iso))
                .collect();
            let networks: Vec<String> = t.networks.into_iter().map(|n| n.name).collect();
            let category = deduce_core_category("tv", &genre_ids);

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

fn iso_country_to_cn(iso: &str) -> String {
    match iso.to_uppercase().as_str() {
        "CN" => "中国大陆",
        "HK" => "中国香港",
        "TW" => "中国台湾",
        "US" => "美国",
        "JP" => "日本",
        "KR" => "韩国",
        "GB" | "UK" => "英国",
        "FR" => "法国",
        "DE" => "德国",
        "TH" => "泰国",
        "IN" => "印度",
        "CA" => "加拿大",
        "AU" => "澳大利亚",
        "ES" => "西班牙",
        "IT" => "意大利",
        "RU" => "俄罗斯",
        other => other,
    }
    .to_string()
}
