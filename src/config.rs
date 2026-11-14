use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::{collections::HashMap, fs, path::PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct BootstrapFileConfig {
    pub server: ServerSection,
    pub sync: SyncSection,
    pub alist: AlistSection,
    pub tmdb: TmdbSection,
    #[serde(default)]
    pub scraper: ScraperSection,
    pub image_cache: ImageCacheSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerSection {
    pub bind_addr: String,
    pub server_id: String,
    pub server_name: String,
    pub database_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SyncSection {
    pub strm_txt_source: String,
    pub sync_interval_sec: u64,
    pub parse_concurrency: usize,
    pub db_batch_size: usize,
    #[serde(default)]
    pub include_prefixes: String,
    #[serde(default)]
    pub exclude_keywords: String,
    #[serde(default)]
    pub max_lines_limit: usize,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ScraperSection {
    #[serde(default)]
    pub auto_background: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlistSection {
    pub base_url: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub otp_secret: String,
    #[serde(default)]
    pub path_prefix: String,
    pub direct_url_ttl_sec: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TmdbSection {
    pub api_key: String,
    #[serde(default)]
    pub proxy_url: String,
    pub api_base_url: String,
    pub image_base_url: String,
    pub rate_limit_per_sec: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImageCacheSection {
    pub cache_dir: String,
    pub max_size_mb: u64,
    pub evict_high_watermark: f64,
    pub evict_low_watermark: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicConfig {
    pub server_id: String,
    pub server_name: String,

    // 同步与过滤配置
    pub strm_txt_source: String,
    pub sync_interval_sec: u64,
    pub parse_concurrency: usize,
    pub db_batch_size: usize,
    pub sync_include_prefixes: String,
    pub sync_exclude_keywords: String,
    pub sync_max_lines_limit: usize,

    // 刮削控制
    pub scraper_auto_background: bool,

    // AList 配置
    pub alist_base_url: String,
    pub alist_username: String,
    pub alist_password: String,
    pub alist_otp_secret: Option<String>,
    pub alist_path_prefix: String,
    pub direct_url_ttl_sec: u64,

    // TMDB 配置
    pub tmdb_api_key: String,
    pub tmdb_proxy_url: Option<String>,
    pub tmdb_api_base: String,
    pub tmdb_image_base: String,
    pub tmdb_rate_limit_per_sec: u32,

    // 图片缓存 LRU 配置
    pub image_cache_dir: PathBuf,
    pub image_max_size_mb: u64,
    pub image_evict_high_watermark: f64,
    pub image_evict_low_watermark: f64,
}

impl BootstrapFileConfig {
    pub fn load_from_file(path: &str) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("读取启动配置文件失败: {}", path))?;
        let cfg: Self = toml::from_str(&content).context("解析 config.toml 格式失败")?;
        Ok(cfg)
    }

    pub fn into_dynamic(self) -> DynamicConfig {
        DynamicConfig {
            server_id: self.server.server_id,
            server_name: self.server.server_name,
            strm_txt_source: self.sync.strm_txt_source,
            sync_interval_sec: self.sync.sync_interval_sec,
            parse_concurrency: self.sync.parse_concurrency,
            db_batch_size: self.sync.db_batch_size,
            sync_include_prefixes: self.sync.include_prefixes,
            sync_exclude_keywords: self.sync.exclude_keywords,
            sync_max_lines_limit: self.sync.max_lines_limit,
            scraper_auto_background: self.scraper.auto_background,
            alist_base_url: self.alist.base_url.trim_end_matches('/').to_string(),
            alist_username: self.alist.username,
            alist_password: self.alist.password,
            alist_otp_secret: non_empty_opt(&self.alist.otp_secret),
            alist_path_prefix: normalize_prefix(&self.alist.path_prefix),
            direct_url_ttl_sec: self.alist.direct_url_ttl_sec,
            tmdb_api_key: self.tmdb.api_key,
            tmdb_proxy_url: non_empty_opt(&self.tmdb.proxy_url),
            tmdb_api_base: self.tmdb.api_base_url.trim_end_matches('/').to_string(),
            tmdb_image_base: self.tmdb.image_base_url.trim_end_matches('/').to_string(),
            tmdb_rate_limit_per_sec: self.tmdb.rate_limit_per_sec.max(1),
            image_cache_dir: PathBuf::from(self.image_cache.cache_dir),
            image_max_size_mb: self.image_cache.max_size_mb,
            image_evict_high_watermark: self.image_cache.evict_high_watermark,
            image_evict_low_watermark: self.image_cache.evict_low_watermark,
        }
    }
}

fn non_empty_opt(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_prefix(p: &str) -> String {
    let trimmed = p.trim().trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        String::new()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{}", trimmed)
    }
}

impl DynamicConfig {
    pub async fn sync_with_db(mut self, db: &PgPool) -> Result<Self> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT config_key, config_value FROM system_configs")
                .fetch_all(db)
                .await?;

        let mut db_map: HashMap<String, String> = rows.into_iter().collect();

        macro_rules! merge_field {
            ($key:expr, $field:expr, $val_type:expr, $desc:expr, $parse:expr) => {
                if let Some(raw_val) = db_map.remove($key) {
                    if let Ok(parsed) = $parse(&raw_val) {
                        $field = parsed;
                    }
                } else {
                    sqlx::query(
                        "INSERT INTO system_configs (config_key, config_value, value_type, description)
                         VALUES ($1, $2, $3, $4) ON CONFLICT (config_key) DO NOTHING",
                    )
                    .bind($key)
                    .bind($field.to_string())
                    .bind($val_type)
                    .bind($desc)
                    .execute(db)
                    .await?;
                }
            };
        }

        merge_field!("sync.strm_txt_source", self.strm_txt_source, "string", "Xiaoya strm.txt 源地址或路径", |s: &str| Ok::<_, ()>(s.to_string()));
        merge_field!("sync.sync_interval_sec", self.sync_interval_sec, "int", "增量同步间隔(秒)", |s: &str| s.parse::<u64>());
        merge_field!("sync.parse_concurrency", self.parse_concurrency, "int", "流式解析并发数", |s: &str| s.parse::<usize>());
        merge_field!("sync.db_batch_size", self.db_batch_size, "int", "数据库批量写入事务大小", |s: &str| s.parse::<usize>());
        merge_field!("sync.include_prefixes", self.sync_include_prefixes, "string", "同步目录白名单前缀(逗号分隔,留空全收)", |s: &str| Ok::<_, ()>(s.to_string()));
        merge_field!("sync.exclude_keywords", self.sync_exclude_keywords, "string", "同步排除关键词(逗号分隔)", |s: &str| Ok::<_, ()>(s.to_string()));
        merge_field!("sync.max_lines_limit", self.sync_max_lines_limit, "int", "单次最大解析行数(0为不限,调试可设5000)", |s: &str| s.parse::<usize>());

        merge_field!("scraper.auto_background", self.scraper_auto_background, "bool", "是否开启后台全库自动刮削(false则仅点开时按需刮削)", |s: &str| s.parse::<bool>());

        merge_field!("alist.base_url", self.alist_base_url, "string", "AList 服务基础地址", |s: &str| Ok::<_, ()>(s.trim_end_matches('/').to_string()));
        merge_field!("alist.username", self.alist_username, "string", "AList 登录账号", |s: &str| Ok::<_, ()>(s.to_string()));
        merge_field!("alist.password", self.alist_password, "secret", "AList 登录密码", |s: &str| Ok::<_, ()>(s.to_string()));
        merge_field!("alist.path_prefix", self.alist_path_prefix, "string", "小雅挂载子目录前缀", |s: &str| Ok::<_, ()>(normalize_prefix(s)));
        merge_field!("alist.direct_url_ttl_sec", self.direct_url_ttl_sec, "int", "直链内存缓存有效期(秒)", |s: &str| s.parse::<u64>());

        merge_field!("tmdb.api_key", self.tmdb_api_key, "secret", "TMDB API Key", |s: &str| Ok::<_, ()>(s.to_string()));
        merge_field!("tmdb.api_base_url", self.tmdb_api_base, "string", "TMDB API 基础地址", |s: &str| Ok::<_, ()>(s.trim_end_matches('/').to_string()));
        merge_field!("tmdb.image_base_url", self.tmdb_image_base, "string", "TMDB 图片源基础地址", |s: &str| Ok::<_, ()>(s.trim_end_matches('/').to_string()));
        merge_field!("tmdb.rate_limit_per_sec", self.tmdb_rate_limit_per_sec, "int", "TMDB 每秒请求限流数", |s: &str| s.parse::<u32>().map(|v| v.max(1)));

        if let Some(raw_proxy) = db_map.remove("tmdb.proxy_url") {
            self.tmdb_proxy_url = non_empty_opt(&raw_proxy);
        } else {
            sqlx::query(
                "INSERT INTO system_configs (config_key, config_value, value_type, description)
                 VALUES ('tmdb.proxy_url', $1, 'string', 'TMDB 请求代理地址') ON CONFLICT DO NOTHING",
            )
            .bind(self.tmdb_proxy_url.clone().unwrap_or_default())
            .execute(db)
            .await?;
        }

        merge_field!("image_cache.max_size_mb", self.image_max_size_mb, "int", "图片磁盘缓存上限(MB)", |s: &str| s.parse::<u64>());
        merge_field!("image_cache.evict_high_watermark", self.image_evict_high_watermark, "float", "图片缓存清理高水位线", |s: &str| s.parse::<f64>());
        merge_field!("image_cache.evict_low_watermark", self.image_evict_low_watermark, "float", "图片缓存清理低水位线", |s: &str| s.parse::<f64>());

        Ok(self)
    }
}
