use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BootstrapFileConfig {
    pub server: ServerSection,
    pub sync: SyncSection,
    pub alist: AlistSection,
    pub tmdb: TmdbSection,
    pub scraper: ScraperSection,
    pub image_cache: ImageCacheSection,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerSection {
    pub bind_addr: String,
    pub server_id: String,
    pub server_name: String,
    pub database_url: String,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:8096".to_string(),
            server_id: "rx-emby-nas-01".to_string(),
            server_name: "RX-Emby Media Server".to_string(),
            database_url: "postgres://media_admin:rx_emby_strong_password@127.0.0.1:5432/xiaoya_media".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SyncSection {
    pub strm_txt_source: String,
    pub sync_interval_sec: u64,
    pub parse_concurrency: usize,
    pub db_batch_size: usize,
    pub include_prefixes: String,
    pub exclude_keywords: String,
    pub max_lines_limit: usize,
}

impl Default for SyncSection {
    fn default() -> Self {
        Self {
            strm_txt_source: "http://192.168.1.10:5244/d/strm.txt".to_string(),
            sync_interval_sec: 21600,
            parse_concurrency: 16,
            db_batch_size: 5000,
            include_prefixes: String::new(),
            exclude_keywords: "演唱会,短剧,广播剧".to_string(),
            max_lines_limit: 0,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ScraperSection {
    pub auto_background: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AlistSection {
    pub base_url: String,
    pub username: String,
    pub password: String,
    pub otp_secret: String,
    pub path_prefix: String,
    pub direct_url_ttl_sec: u64,
}

impl Default for AlistSection {
    fn default() -> Self {
        Self {
            base_url: "http://192.168.1.10:5244".to_string(),
            username: "admin".to_string(),
            password: String::new(),
            otp_secret: String::new(),
            path_prefix: String::new(),
            direct_url_ttl_sec: 600,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TmdbSection {
    pub api_key: String,
    pub proxy_url: String,
    pub api_base_url: String,
    pub image_base_url: String,
    pub rate_limit_per_sec: u32,
}

impl Default for TmdbSection {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            proxy_url: String::new(),
            api_base_url: "https://api.themoviedb.org/3".to_string(),
            image_base_url: "https://image.tmdb.org/t/p".to_string(),
            rate_limit_per_sec: 8,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ImageCacheSection {
    pub disk_cache_enabled: bool,
    pub cache_dir: String,
    pub max_size_mb: u64,
    pub evict_high_watermark: f64,
    pub evict_low_watermark: f64,
}

impl Default for ImageCacheSection {
    fn default() -> Self {
        Self {
            disk_cache_enabled: true,
            cache_dir: "./cache/images".to_string(),
            max_size_mb: 20480,
            evict_high_watermark: 0.90,
            evict_low_watermark: 0.75,
        }
    }
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
    pub image_disk_cache_enabled: bool,
    pub image_cache_dir: PathBuf,
    pub image_max_size_mb: u64,
    pub image_evict_high_watermark: f64,
    pub image_evict_low_watermark: f64,
}

impl BootstrapFileConfig {
    pub fn load_with_env(path: &str) -> Result<Self> {
        let mut cfg = if Path::new(path).is_file() {
            let content = fs::read_to_string(path)
                .with_context(|| format!("读取启动配置文件失败: {}", path))?;
            toml::from_str(&content).context("解析 config.toml 格式失败")?
        } else {
            Self::default()
        };
        cfg.apply_env_overrides();
        Ok(cfg)
    }

    fn apply_env_overrides(&mut self) {
        // [server]
        env_str("BIND_ADDR", &mut self.server.bind_addr);
        env_str("SERVER_ID", &mut self.server.server_id);
        env_str("SERVER_NAME", &mut self.server.server_name);
        env_str("DATABASE_URL", &mut self.server.database_url);

        // [sync]
        env_str("STRM_TXT_SOURCE", &mut self.sync.strm_txt_source);
        env_parse("SYNC_INTERVAL_SEC", &mut self.sync.sync_interval_sec);
        env_parse("PARSE_CONCURRENCY", &mut self.sync.parse_concurrency);
        env_parse("DB_BATCH_SIZE", &mut self.sync.db_batch_size);
        env_str("SYNC_INCLUDE_PREFIXES", &mut self.sync.include_prefixes);
        env_str("SYNC_EXCLUDE_KEYWORDS", &mut self.sync.exclude_keywords);
        env_parse("SYNC_MAX_LINES_LIMIT", &mut self.sync.max_lines_limit);

        // [scraper]
        if let Ok(v) = env::var("SCRAPER_AUTO_BACKGROUND") {
            match v.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => self.scraper.auto_background = true,
                "false" | "0" | "no" => self.scraper.auto_background = false,
                _ => {}
            }
        }

        // [alist]
        env_str("ALIST_BASE_URL", &mut self.alist.base_url);
        env_str("ALIST_USERNAME", &mut self.alist.username);
        env_str("ALIST_PASSWORD", &mut self.alist.password);
        env_str("ALIST_OTP_SECRET", &mut self.alist.otp_secret);
        env_str("ALIST_PATH_PREFIX", &mut self.alist.path_prefix);
        env_parse("ALIST_DIRECT_URL_TTL_SEC", &mut self.alist.direct_url_ttl_sec);

        // [tmdb]
        env_str("TMDB_API_KEY", &mut self.tmdb.api_key);
        env_str("TMDB_PROXY", &mut self.tmdb.proxy_url);
        env_str("TMDB_PROXY_URL", &mut self.tmdb.proxy_url);
        env_str("TMDB_API_BASE_URL", &mut self.tmdb.api_base_url);
        env_str("TMDB_IMAGE_BASE_URL", &mut self.tmdb.image_base_url);
        env_parse("TMDB_RATE_LIMIT_PER_SEC", &mut self.tmdb.rate_limit_per_sec);

        // [image_cache]
        env_parse("IMAGE_CACHE_DISK_ENABLED", &mut self.image_cache.disk_cache_enabled);
        env_str("IMAGE_CACHE_DIR", &mut self.image_cache.cache_dir);
        env_parse("IMAGE_CACHE_MAX_SIZE_MB", &mut self.image_cache.max_size_mb);
        env_parse(
            "IMAGE_CACHE_EVICT_HIGH_WATERMARK",
            &mut self.image_cache.evict_high_watermark,
        );
        env_parse(
            "IMAGE_CACHE_EVICT_LOW_WATERMARK",
            &mut self.image_cache.evict_low_watermark,
        );
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
            image_disk_cache_enabled: self.image_cache.disk_cache_enabled,
            image_cache_dir: PathBuf::from(self.image_cache.cache_dir),
            image_max_size_mb: self.image_cache.max_size_mb,
            image_evict_high_watermark: self.image_cache.evict_high_watermark,
            image_evict_low_watermark: self.image_cache.evict_low_watermark,
        }
    }
}

fn env_str(key: &str, target: &mut String) {
    if let Ok(v) = env::var(key) {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            *target = trimmed.to_string();
        }
    }
}

fn env_parse<T: std::str::FromStr>(key: &str, target: &mut T) {
    if let Ok(v) = env::var(key) {
        if let Ok(parsed) = v.trim().parse::<T>() {
            *target = parsed;
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
        if let Some(raw_otp) = db_map.remove("alist.otp_secret") {
            self.alist_otp_secret = non_empty_opt(&raw_otp);
        } else {
            sqlx::query(
                "INSERT INTO system_configs (config_key, config_value, value_type, description)
                 VALUES ('alist.otp_secret', $1, 'secret', 'AList TOTP 2FA 密钥') ON CONFLICT DO NOTHING",
            )
            .bind(self.alist_otp_secret.clone().unwrap_or_default())
            .execute(db)
            .await?;
        }
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

        if let Some(raw_dir) = db_map.remove("image_cache.cache_dir") {
            let trimmed = raw_dir.trim();
            if !trimmed.is_empty() {
                self.image_cache_dir = PathBuf::from(trimmed);
            }
        } else {
            sqlx::query(
                "INSERT INTO system_configs (config_key, config_value, value_type, description)
                 VALUES ('image_cache.cache_dir', $1, 'string', '图片磁盘缓存目录') ON CONFLICT DO NOTHING",
            )
            .bind(self.image_cache_dir.to_string_lossy().to_string())
            .execute(db)
            .await?;
        }
        merge_field!("image_cache.disk_cache_enabled", self.image_disk_cache_enabled, "bool", "是否将海报与剧照缓存到本地磁盘", |s: &str| s.parse::<bool>());
        merge_field!("image_cache.max_size_mb", self.image_max_size_mb, "int", "图片磁盘缓存上限(MB)", |s: &str| s.parse::<u64>());
        merge_field!("image_cache.evict_high_watermark", self.image_evict_high_watermark, "float", "图片缓存清理高水位线", |s: &str| s.parse::<f64>());
        merge_field!("image_cache.evict_low_watermark", self.image_evict_low_watermark, "float", "图片缓存清理低水位线", |s: &str| s.parse::<f64>());

        Ok(self)
    }
}
