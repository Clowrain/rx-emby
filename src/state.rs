use crate::config::DynamicConfig;
use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use dashmap::DashMap;
use governor::{
    clock::DefaultClock,
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter,
};
use moka::future::Cache;
use reqwest::{Client, Proxy};
use sqlx::PgPool;
use std::{
    num::NonZeroU32,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch, Mutex, Notify, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::info;

pub type SharedRateLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

pub struct AppState {
    pub config: ArcSwap<DynamicConfig>,
    pub config_tx: watch::Sender<Arc<DynamicConfig>>,

    pub db: PgPool,
    pub tmdb_client: ArcSwap<Client>,
    pub alist_client: ArcSwap<Client>,
    pub alist_auth: AlistAuthManager,

    pub sync_ctrl: SyncControl,
    pub scraper: ScrapeScheduler,
    pub caches: CacheLayer,
    pub sessions: DashMap<String, PlaybackSession>,
    pub shutdown_token: CancellationToken,
}

pub struct AlistAuthManager {
    pub current_token: RwLock<Option<String>>,
    pub refresh_lock: Mutex<()>,
}

impl Default for AlistAuthManager {
    fn default() -> Self {
        Self {
            current_token: RwLock::new(None),
            refresh_lock: Mutex::new(()),
        }
    }
}

pub struct SyncControl {
    pub is_syncing: AtomicBool,
    pub last_file_hash: RwLock<Option<u64>>,
    pub last_sync_time: RwLock<Option<chrono::DateTime<chrono::Utc>>>,
    pub manual_trigger: Notify,
}

impl Default for SyncControl {
    fn default() -> Self {
        Self {
            is_syncing: AtomicBool::new(false),
            last_file_hash: RwLock::new(None),
            last_sync_time: RwLock::new(None),
            manual_trigger: Notify::new(),
        }
    }
}

pub struct ScrapeScheduler {
    pub priority_tx: mpsc::Sender<i64>,
    pub limiter: ArcSwap<SharedRateLimiter>,
}

pub struct CacheLayer {
    /// Key = "{path_hash}:{client_ua}", Value = 网盘直链 raw_url
    pub alist_direct_urls: Cache<String, String>,
    /// Key = AList 父目录路径，记录已通过 fs/list 预热过寻址的目录（避免短时间重复 list）
    pub alist_dir_prewarmed: Cache<String, ()>,
    pub image_fetch_inflight: Cache<String, ()>,
    /// 后台概览与统计短时缓存（避免频繁切换 Tab 反复触发全表聚合扫描）
    pub admin_stats_cache: Cache<String, serde_json::Value>,
}

#[derive(Clone, Debug)]
pub struct PlaybackSession {
    pub session_id: String,
    pub user_id: String,
    pub device_name: String,
    pub client_name: String,
    pub current_episode_id: Option<i64>,
    pub position_ticks: i64,
    pub next_ep_prewarmed: bool,
    pub last_activity: Instant,
}

/// 播放会话最大无心跳存活时间（Emby 客户端正常播放时每 10 秒上报一次 Progress）
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(180);
/// 刚发起 Playing 但进度仍为 0（如流失败或秒退）的会话最大存活时间
const SESSION_ZERO_PROGRESS_TIMEOUT: Duration = Duration::from_secs(60);

impl AppState {
    pub fn new(
        initial_cfg: DynamicConfig,
        db: PgPool,
        priority_tx: mpsc::Sender<i64>,
    ) -> Result<Arc<Self>> {
        let tmdb_http = build_tmdb_client(initial_cfg.tmdb_proxy_url.as_deref())?;
        let alist_http = build_alist_client()?;
        let limiter = build_rate_limiter(initial_cfg.tmdb_rate_limit_per_sec);

        let direct_urls = Cache::builder()
            .max_capacity(20_000)
            .time_to_live(Duration::from_secs(initial_cfg.direct_url_ttl_sec))
            .build();

        let dir_prewarmed = Cache::builder()
            .max_capacity(5_000)
            .time_to_live(Duration::from_secs(600))
            .build();

        let inflight = Cache::builder()
            .max_capacity(5_000)
            .time_to_live(Duration::from_secs(30))
            .build();

        let admin_stats_cache = Cache::builder()
            .max_capacity(32)
            .time_to_live(Duration::from_secs(10))
            .build();

        let cfg_arc = Arc::new(initial_cfg);
        let (config_tx, _) = watch::channel(cfg_arc.clone());

        Ok(Arc::new(Self {
            config: ArcSwap::from(cfg_arc),
            config_tx,
            db,
            tmdb_client: ArcSwap::from_pointee(tmdb_http),
            alist_client: ArcSwap::from_pointee(alist_http),
            alist_auth: AlistAuthManager::default(),
            sync_ctrl: SyncControl::default(),
            scraper: ScrapeScheduler {
                priority_tx,
                limiter: ArcSwap::from_pointee(limiter),
            },
            caches: CacheLayer {
                alist_direct_urls: direct_urls,
                alist_dir_prewarmed: dir_prewarmed,
                image_fetch_inflight: inflight,
                admin_stats_cache,
            },
            sessions: DashMap::new(),
            shutdown_token: CancellationToken::new(),
        }))
    }

    /// 清理已断开但未收到 Stopped 信号的过期僵尸会话
    pub fn prune_stale_sessions(&self) {
        self.sessions.retain(|_, s| {
            let idle = s.last_activity.elapsed();
            if s.position_ticks <= 0 {
                idle <= SESSION_ZERO_PROGRESS_TIMEOUT
            } else {
                idle <= SESSION_IDLE_TIMEOUT
            }
        });
    }

    /// 运行时热更新配置（Web 后台保存设置后调用，无需重启服务）
    pub async fn reload_config(&self, new_cfg: DynamicConfig) -> Result<()> {
        let old_cfg = self.config.load();

        // 1. 若代理配置变更，原子重建 TMDB HTTP Client
        if old_cfg.tmdb_proxy_url != new_cfg.tmdb_proxy_url {
            let client = build_tmdb_client(new_cfg.tmdb_proxy_url.as_deref())?;
            self.tmdb_client.store(Arc::new(client));
            info!("TMDB HTTP 客户端代理已热更新: {:?}", new_cfg.tmdb_proxy_url);
        }

        // 2. 若限流速率变更，原子替换令牌桶
        if old_cfg.tmdb_rate_limit_per_sec != new_cfg.tmdb_rate_limit_per_sec {
            let limiter = build_rate_limiter(new_cfg.tmdb_rate_limit_per_sec);
            self.scraper.limiter.store(Arc::new(limiter));
            info!("TMDB 限流速率已热更新: {} req/s", new_cfg.tmdb_rate_limit_per_sec);
        }

        // 3. 若 AList 账号或地址变更，清空旧 Token 触发重新登录
        if old_cfg.alist_base_url != new_cfg.alist_base_url
            || old_cfg.alist_username != new_cfg.alist_username
            || old_cfg.alist_password != new_cfg.alist_password
        {
            let mut token_guard = self.alist_auth.current_token.write().await;
            *token_guard = None;
            info!("AList 鉴权配置已变更，旧 Token 已失效并等待刷新");
        }

        let new_arc = Arc::new(new_cfg);
        self.config.store(new_arc.clone());
        let _ = self.config_tx.send(new_arc);
        Ok(())
    }
}

pub fn build_tmdb_client(proxy_url: Option<&str>) -> Result<Client> {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(15))
        .pool_max_idle_per_host(16);

    if let Some(p_url) = proxy_url {
        let proxy = Proxy::all(p_url).with_context(|| format!("无效的 TMDB 代理地址: {}", p_url))?;
        builder = builder.proxy(proxy);
    }

    builder.build().context("构建 TMDB HTTP Client 失败")
}

pub fn build_alist_client() -> Result<Client> {
    Client::builder()
        .no_proxy() // 内网 AList 强制直连，绕过系统环境变量代理
        .redirect(reqwest::redirect::Policy::none()) // 禁用自动重定向以便捕获 Location
        .timeout(Duration::from_secs(15))
        .tcp_keepalive(Duration::from_secs(60))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(32)
        .build()
        .context("构建 AList HTTP Client 失败")
}

pub fn build_rate_limiter(per_sec: u32) -> SharedRateLimiter {
    let quota = Quota::per_second(NonZeroU32::new(per_sec.max(1)).unwrap());
    RateLimiter::direct(quota)
}
