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
use tokio::sync::{mpsc, watch, Mutex, Notify, RwLock, Semaphore};
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
    pub session_hub: SessionHub,
    pub client_capabilities: moka::sync::Cache<String, serde_json::Value>,
    pub monitor: crate::monitor::MonitorState,
    pub shutdown_token: CancellationToken,
}

/// 每个连接使用有界队列；慢客户端被断开，避免后台广播积累内存。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubEventKind {
    Sessions,
    LibraryChanged,
    UserDataChanged,
    Other,
}

#[derive(Clone, Debug)]
pub struct HubEvent {
    pub kind: HubEventKind,
    pub message: String,
}

impl HubEventKind {
    fn from_name(name: &str) -> Self {
        match name {
            "Sessions" => Self::Sessions,
            "LibraryChanged" => Self::LibraryChanged,
            "UserDataChanged" => Self::UserDataChanged,
            _ => Self::Other,
        }
    }
}

#[derive(Default)]
pub struct SessionHub {
    connections: DashMap<String, HubConnection>,
}

struct HubConnection {
    token: String,
    user_id: String,
    is_admin: bool,
    tx: mpsc::Sender<HubEvent>,
    cancelled: CancellationToken,
}

impl SessionHub {
    #[cfg(test)]
    pub fn register(
        &self,
        token: String,
        user_id: String,
    ) -> (String, mpsc::Receiver<HubEvent>, CancellationToken) {
        self.register_with_role(token, user_id, false)
    }

    pub fn register_with_role(
        &self,
        token: String,
        user_id: String,
        is_admin: bool,
    ) -> (String, mpsc::Receiver<HubEvent>, CancellationToken) {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::channel(64);
        let cancelled = CancellationToken::new();
        self.connections.insert(
            id.clone(),
            HubConnection {
                token,
                user_id,
                is_admin,
                tx,
                cancelled: cancelled.clone(),
            },
        );
        (id, rx, cancelled)
    }

    pub fn unregister(&self, id: &str) {
        if let Some((_, connection)) = self.connections.remove(id) {
            connection.cancelled.cancel();
        }
    }

    pub fn broadcast(&self, user_id: Option<&str>, message_type: &str, data: serde_json::Value) {
        let message = HubEvent {
            kind: HubEventKind::from_name(message_type),
            message: serde_json::json!({"MessageType": message_type, "Data": data}).to_string(),
        };
        let mut evicted = Vec::new();
        for entry in &self.connections {
            if user_id.is_none_or(|id| id == entry.user_id)
                && entry.tx.try_send(message.clone()).is_err()
            {
                entry.cancelled.cancel();
                evicted.push(entry.key().clone());
            }
        }
        for id in evicted {
            self.unregister(&id);
        }
    }

    pub fn broadcast_admins(&self, message_type: &str, data: serde_json::Value) {
        let message = HubEvent {
            kind: HubEventKind::from_name(message_type),
            message: serde_json::json!({"MessageType": message_type, "Data": data}).to_string(),
        };
        let mut evicted = Vec::new();
        for entry in &self.connections {
            if entry.is_admin && entry.tx.try_send(message.clone()).is_err() {
                entry.cancelled.cancel();
                evicted.push(entry.key().clone());
            }
        }
        for id in evicted {
            self.unregister(&id);
        }
    }

    pub fn disconnect_tokens(&self, tokens: &[String]) {
        self.connections.retain(|_, connection| {
            if tokens.contains(&connection.token) {
                connection.cancelled.cancel();
                false
            } else {
                true
            }
        });
    }

    pub fn has_user_connections(&self, user_id: &str) -> bool {
        self.connections
            .iter()
            .any(|entry| entry.user_id == user_id)
    }
}

pub struct AlistAuthManager {
    pub current_token: RwLock<Option<String>>,
    pub refresh_lock: Mutex<()>,
    /// 最近一次成功登录时间，用于 401 风暴时判断是否已有并发协程完成刷新
    pub last_refresh_at: RwLock<Option<Instant>>,
}

impl Default for AlistAuthManager {
    fn default() -> Self {
        Self {
            current_token: RwLock::new(None),
            refresh_lock: Mutex::new(()),
            last_refresh_at: RwLock::new(None),
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct SyncProgress {
    pub log_id: Option<i64>,
    pub stage: String,
    pub step: u8,
    pub total_steps: u8,
    pub percent: u8,
    pub message: String,
    pub scanned_lines: i32,
    pub valid_records: i32,
    pub started_at: chrono::DateTime<chrono::Utc>,
}

pub struct SyncControl {
    pub is_syncing: AtomicBool,
    pub last_file_hash: RwLock<Option<u64>>,
    pub last_sync_time: RwLock<Option<chrono::DateTime<chrono::Utc>>>,
    pub progress: RwLock<Option<SyncProgress>>,
    pub manual_trigger: Notify,
}

impl Default for SyncControl {
    fn default() -> Self {
        Self {
            is_syncing: AtomicBool::new(false),
            last_file_hash: RwLock::new(None),
            last_sync_time: RwLock::new(None),
            progress: RwLock::new(None),
            manual_trigger: Notify::new(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ScrapeTask {
    Folder(i64),
    WorkSeasons(i64),
}

pub struct ScrapeScheduler {
    pub runtime: crate::scraper::ScrapeRuntime,
    pub priority_tx: mpsc::Sender<ScrapeTask>,
    pub limiter: ArcSwap<SharedRateLimiter>,
}

pub struct CacheLayer {
    /// Key = "{path_hash}:{client_ua}", Value = 网盘直链 raw_url
    pub alist_direct_urls: Cache<String, String>,
    /// Key = path_hash，存储不绑定 UA 的通用网盘直链（如阿里云盘/天翼/夸克等，排除 115/百度等强校验 UA 的 CDN），
    /// 实现 PlaybackInfo (UI UA) 预热后，/stream (播放器内核 UA) 0ms 直接跨 UA 命中！
    pub alist_ua_agnostic_urls: Cache<i64, String>,
    /// Key = path_hash，同一媒体文件的全局换链互斥锁：防止 PlaybackInfo 预热与播放器 /stream 因 UA 不同同时向小雅发起两次转存请求导致锁冲突
    pub alist_path_locks: Cache<i64, Arc<Mutex<()>>>,
    /// Key = AList 父目录路径，记录已通过 fs/list 预热过寻址的目录（避免短时间重复 list）
    pub alist_dir_prewarmed: Cache<String, ()>,
    /// Key = "{size}/{file_name}", Value = 图片字节流。
    /// `try_get_with` 兼做真正的 singleflight：并发同图请求只放行一次回源/渲染并共享结果。
    pub image_hot_bytes: Cache<String, bytes::Bytes>,
    /// Key = "collage:{view_id}:{style}:{hash}", Value = 渲染完成的拼接封面字节流
    pub collage_cache: Cache<String, bytes::Bytes>,
    /// TMDB 图片出站下载并发上限（防止海报墙风暴打满反代带宽与连接池）
    pub image_download_permits: Arc<Semaphore>,
    /// 图片条目元数据查询缓存：Key = "{item_id}:{image_type}" -> (size, file_name)
    pub image_meta_cache: Cache<String, Option<(String, String)>>,
    /// LRU touch 写库去抖：Key = cache_key，TTL 窗口内同一 key 至多触发一次 UPDATE
    pub image_lru_touch_debounce: Cache<String, ()>,
    /// 访问令牌校验缓存：token -> (user_id, is_admin)，注销/吊销时需显式失效
    pub token_user_cache: Cache<String, Arc<(String, bool)>>,
    /// 管理 API 的完整令牌身份缓存，避免用户名校验回退到每请求 PG。
    pub admin_token_user_cache: Cache<String, Arc<(String, String, bool)>>,
    /// 失败登录窗口计数；按用户名/IP隔离，60 秒自动过期。
    pub failed_login_attempts: Cache<String, Arc<std::sync::atomic::AtomicU32>>,
    /// access_tokens.last_used_at 写库去抖：同一 token 在窗口内至多更新一次
    pub token_touch_debounce: Cache<String, ()>,
    /// 后台概览与统计短时缓存（避免频繁切换 Tab 反复触发全表聚合扫描）
    pub admin_stats_cache: Cache<String, serde_json::Value>,
    /// 用户可访问分类缓存；None 表示管理员/通配 unrestricted，Some(empty) 表示没有分类。
    pub user_categories_cache: Cache<String, Option<Vec<String>>>,
    /// 启用的虚拟媒体库缓存（全局短 TTL）。
    pub enabled_vlibs_cache: Cache<String, Vec<(i32, String, serde_json::Value)>>,
    /// 元数据标签反查映射缓存（prefix:col_expr -> (tag_id -> 原始名称)）。
    pub metadata_tag_map_cache: Cache<String, Arc<std::collections::HashMap<String, String>>>,
}

impl CacheLayer {
    /// Moka 0.12 不启用后台淘汰线程；定时执行 pending tasks 才能在无请求时释放过期/失效条目。
    pub async fn run_pending_tasks(&self) {
        self.alist_direct_urls.run_pending_tasks().await;
        self.alist_ua_agnostic_urls.run_pending_tasks().await;
        self.alist_path_locks.run_pending_tasks().await;
        self.alist_dir_prewarmed.run_pending_tasks().await;
        self.image_hot_bytes.run_pending_tasks().await;
        self.collage_cache.run_pending_tasks().await;
        self.image_meta_cache.run_pending_tasks().await;
        self.image_lru_touch_debounce.run_pending_tasks().await;
        self.token_user_cache.run_pending_tasks().await;
        self.admin_token_user_cache.run_pending_tasks().await;
        self.failed_login_attempts.run_pending_tasks().await;
        self.token_touch_debounce.run_pending_tasks().await;
        self.admin_stats_cache.run_pending_tasks().await;
        self.user_categories_cache.run_pending_tasks().await;
        self.enabled_vlibs_cache.run_pending_tasks().await;
        self.metadata_tag_map_cache.run_pending_tasks().await;
    }
}

/// 提示 Linux glibc 分配器立即归还空闲堆页给操作系统，缓解同步/缓存峰值后的 RSS 驻留。
pub fn trim_process_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::malloc_trim(0);
    }
}

/// 锁定 Linux glibc 动态 MMAP/TRIM 阈值（128 KB），避免大块缓存或同步缓冲区把阈值抬升至 32 MB 后长期碎片化驻留堆内。
pub fn configure_process_allocator() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, 128 * 1024);
        libc::mallopt(libc::M_TRIM_THRESHOLD, 128 * 1024);
    }
}

#[derive(Clone, Debug)]
pub struct PlaybackSession {
    pub session_id: String,
    pub user_id: String,
    pub device_id: String,
    pub device_name: String,
    pub client_name: String,
    pub current_episode_id: Option<i64>,
    pub position_ticks: i64,
    pub duration_ticks: i64,
    pub next_ep_prewarmed: bool,
    pub completed_in_session: bool,
    pub last_activity: Instant,
}

impl PlaybackSession {
    pub fn is_active(&self) -> bool {
        self.last_activity.elapsed()
            <= if self.position_ticks <= 0 {
                SESSION_ZERO_PROGRESS_TIMEOUT
            } else {
                SESSION_IDLE_TIMEOUT
            }
    }
}

/// 播放会话最大无心跳存活时间（Emby 客户端正常播放时每 10 秒上报一次 Progress）
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(180);
/// 刚发起 Playing 但进度仍为 0（如流失败或秒退）的会话最大存活时间
const SESSION_ZERO_PROGRESS_TIMEOUT: Duration = Duration::from_secs(60);

impl AppState {
    pub fn new(
        initial_cfg: DynamicConfig,
        db: PgPool,
        priority_tx: mpsc::Sender<ScrapeTask>,
    ) -> Result<Arc<Self>> {
        let tmdb_http = build_tmdb_client(initial_cfg.tmdb_proxy_url.as_deref())?;
        let alist_http = build_alist_client()?;
        let limiter = build_rate_limiter(initial_cfg.tmdb_rate_limit_per_sec);

        let direct_urls = Cache::builder()
            .max_capacity(20_000)
            .time_to_live(Duration::from_secs(initial_cfg.direct_url_ttl_sec))
            .build();

        let ua_agnostic_urls = Cache::builder()
            .max_capacity(20_000)
            .time_to_live(Duration::from_secs(initial_cfg.direct_url_ttl_sec))
            .build();

        let path_locks = Cache::builder()
            .max_capacity(10_000)
            .time_to_idle(Duration::from_secs(120))
            .build();

        let dir_prewarmed = Cache::builder()
            .max_capacity(5_000)
            .time_to_live(Duration::from_secs(600))
            .build();

        let inflight = Cache::builder()
            .max_capacity(24 * 1024 * 1024)
            .weigher(|_key: &String, value: &bytes::Bytes| {
                value.len().min(u32::MAX as usize) as u32
            })
            .time_to_live(Duration::from_secs(600))
            .time_to_idle(Duration::from_secs(180))
            .build();

        let collage_cache = Cache::builder()
            .max_capacity(8 * 1024 * 1024)
            .weigher(|_key: &String, value: &bytes::Bytes| {
                value.len().min(u32::MAX as usize) as u32
            })
            .time_to_live(Duration::from_secs(86400))
            .time_to_idle(Duration::from_secs(1800))
            .build();

        let image_meta_cache = Cache::builder()
            .max_capacity(20_000)
            .time_to_live(Duration::from_secs(300))
            .build();

        let lru_touch_debounce = Cache::builder()
            .max_capacity(50_000)
            .time_to_live(Duration::from_secs(1800))
            .build();

        let token_user_cache = Cache::builder()
            .max_capacity(2_000)
            .time_to_live(Duration::from_secs(60))
            .build();

        let admin_token_user_cache = Cache::builder()
            .max_capacity(2_000)
            .time_to_live(Duration::from_secs(60))
            .build();

        let token_touch_debounce = Cache::builder()
            .max_capacity(5_000)
            .time_to_live(Duration::from_secs(300))
            .build();

        let failed_login_attempts = Cache::builder()
            .max_capacity(50_000)
            .time_to_live(Duration::from_secs(60))
            .build();

        let admin_stats_cache = Cache::builder()
            .max_capacity(32)
            .time_to_live(Duration::from_secs(10))
            .build();

        let user_categories_cache = Cache::builder()
            .max_capacity(2_000)
            .time_to_live(Duration::from_secs(60))
            .build();

        let enabled_vlibs_cache = Cache::builder()
            .max_capacity(1)
            .time_to_live(Duration::from_secs(30))
            .build();

        let metadata_tag_map_cache = Cache::builder()
            .max_capacity(8)
            .time_to_live(Duration::from_secs(60))
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
                runtime: Default::default(),
                priority_tx,
                limiter: ArcSwap::from_pointee(limiter),
            },
            caches: CacheLayer {
                alist_direct_urls: direct_urls,
                alist_ua_agnostic_urls: ua_agnostic_urls,
                alist_path_locks: path_locks,
                alist_dir_prewarmed: dir_prewarmed,
                image_hot_bytes: inflight,
                collage_cache,
                image_download_permits: Arc::new(Semaphore::new(8)),
                image_meta_cache,
                image_lru_touch_debounce: lru_touch_debounce,
                token_user_cache,
                admin_token_user_cache,
                failed_login_attempts,
                token_touch_debounce,
                admin_stats_cache,
                user_categories_cache,
                enabled_vlibs_cache,
                metadata_tag_map_cache,
            },
            sessions: DashMap::new(),
            session_hub: SessionHub::default(),
            client_capabilities: moka::sync::Cache::builder()
                .max_capacity(5_000)
                .time_to_idle(Duration::from_secs(7 * 24 * 3600))
                .build(),
            monitor: crate::monitor::MonitorState::default(),
            shutdown_token: CancellationToken::new(),
        }))
    }

    pub async fn login_rate_limited(&self, username: &str, ip: &str) -> bool {
        let username_limited = self
            .caches
            .failed_login_attempts
            .get(&format!("user:{}", username.to_lowercase()))
            .await
            .is_some_and(|counter| counter.load(std::sync::atomic::Ordering::Relaxed) >= 15);
        let ip_limited = self
            .caches
            .failed_login_attempts
            .get(&format!("ip:{ip}"))
            .await
            .is_some_and(|counter| counter.load(std::sync::atomic::Ordering::Relaxed) >= 30);
        username_limited || ip_limited
    }

    pub async fn record_failed_login(&self, username: &str, ip: &str) {
        for key in [
            format!("user:{}", username.to_lowercase()),
            format!("ip:{ip}"),
        ] {
            let counter = self
                .caches
                .failed_login_attempts
                .get_with(key, async {
                    Arc::new(std::sync::atomic::AtomicU32::new(0))
                })
                .await;
            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// 清理已断开但未收到 Stopped 信号的过期僵尸会话
    pub fn prune_stale_sessions(&self) {
        self.sessions.retain(|_, s| s.is_active());
    }

    /// 主动触发全部 Moka 缓存的延迟淘汰与失效回收，并归还 glibc 空闲堆页，
    /// 避免夜间低峰/待机期间已过期的图片与元数据缓存长期驻留 RSS。
    pub async fn run_cache_maintenance(&self) {
        self.caches.run_pending_tasks().await;
        self.client_capabilities.run_pending_tasks();
        trim_process_memory();
    }

    pub async fn run_session_cleanup(self: Arc<Self>, interval: Duration) {
        let mut timer = tokio::time::interval(interval);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_cache_maint = Instant::now();
        let mut last_token_prune = Instant::now();
        loop {
            tokio::select! {
                biased;
                _ = self.shutdown_token.cancelled() => break,
                _ = timer.tick() => {
                    self.prune_stale_sessions();
                    if last_cache_maint.elapsed() >= Duration::from_secs(60) {
                        last_cache_maint = Instant::now();
                        self.run_cache_maintenance().await;
                    }
                    if last_token_prune.elapsed() >= Duration::from_secs(3600) {
                        last_token_prune = Instant::now();
                        let _ = sqlx::query(
                            "DELETE FROM access_tokens WHERE last_used_at < NOW() - INTERVAL '90 days'",
                        )
                        .execute(&self.db)
                        .await;
                    }
                }
            }
        }
    }

    /// 运行时热更新配置（Web 后台保存设置后调用，无需重启服务）
    pub async fn reload_config(&self, new_cfg: DynamicConfig) -> Result<()> {
        validate_runtime_config(&new_cfg)?;
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
            info!(
                "TMDB 限流速率已热更新: {} req/s",
                new_cfg.tmdb_rate_limit_per_sec
            );
        }

        // 3. 若 AList 鉴权或路径前缀变更，清空旧 Token 与直链/目录预热缓存
        if old_cfg.alist_base_url != new_cfg.alist_base_url
            || old_cfg.alist_username != new_cfg.alist_username
            || old_cfg.alist_password != new_cfg.alist_password
            || old_cfg.alist_otp_secret != new_cfg.alist_otp_secret
            || old_cfg.alist_path_prefix != new_cfg.alist_path_prefix
            || old_cfg.direct_url_ttl_sec != new_cfg.direct_url_ttl_sec
        {
            let _refresh_guard = self.alist_auth.refresh_lock.lock().await;
            let mut token_guard = self.alist_auth.current_token.write().await;
            *token_guard = None;
            *self.alist_auth.last_refresh_at.write().await = None;
            self.caches.alist_direct_urls.invalidate_all();
            self.caches.alist_ua_agnostic_urls.invalidate_all();
            self.caches.alist_dir_prewarmed.invalidate_all();
            info!("AList 配置已变更，旧 Token、直链及目录预热缓存已清空");
        }

        let new_arc = Arc::new(new_cfg);
        self.config.store(new_arc.clone());
        let _ = self.config_tx.send(new_arc);
        Ok(())
    }
}

fn validate_runtime_config(cfg: &DynamicConfig) -> Result<()> {
    fn validate_url(value: &str, allowed_schemes: &[&str], name: &str) -> Result<()> {
        let url = reqwest::Url::parse(value).with_context(|| format!("invalid {name} URL"))?;
        anyhow::ensure!(
            allowed_schemes.contains(&url.scheme()) && url.host_str().is_some(),
            "invalid {name} URL scheme or host"
        );
        Ok(())
    }

    validate_url(&cfg.tmdb_api_base, &["http", "https"], "TMDB API base")?;
    validate_url(&cfg.tmdb_image_base, &["http", "https"], "TMDB image base")?;
    if let Some(proxy) = cfg
        .tmdb_proxy_url
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        validate_url(proxy, &["http", "https", "socks5", "socks5h"], "TMDB proxy")?;
    }
    if !cfg.alist_base_url.trim().is_empty() {
        validate_url(&cfg.alist_base_url, &["http", "https"], "AList base")?;
    }
    let source = cfg.strm_txt_source.trim();
    if source.starts_with("http://") || source.starts_with("https://") {
        validate_url(source, &["http", "https"], "sync source")?;
    } else {
        anyhow::ensure!(!source.is_empty(), "sync source cannot be empty");
        anyhow::ensure!(
            !["/dev/", "/proc/", "/sys/"]
                .iter()
                .any(|p| source.starts_with(p)),
            "sync source cannot be a system pseudo-file"
        );
        let metadata =
            std::fs::metadata(source).with_context(|| "sync source must be a regular file")?;
        anyhow::ensure!(metadata.is_file(), "sync source must be a regular file");
    }
    anyhow::ensure!(
        !cfg.image_cache_dir.as_os_str().is_empty()
            && !cfg
                .image_cache_dir
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir)),
        "image cache directory cannot be empty or contain parent traversal"
    );
    Ok(())
}

pub fn build_tmdb_client(proxy_url: Option<&str>) -> Result<Client> {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(16);

    if let Some(p_url) = proxy_url {
        let proxy =
            Proxy::all(p_url).with_context(|| format!("无效的 TMDB 代理地址: {}", p_url))?;
        builder = builder.proxy(proxy);
    }

    builder.build().context("构建 TMDB HTTP Client 失败")
}

pub fn build_alist_client() -> Result<Client> {
    Client::builder()
        .no_proxy() // 内网 AList 强制直连，绕过系统环境变量代理
        .redirect(reqwest::redirect::Policy::none()) // 禁用自动重定向以便捕获 Location
        .timeout(Duration::from_secs(35))
        .tcp_keepalive(Duration::from_secs(60))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(32)
        .build()
        .context("构建 AList HTTP Client 失败")
}

pub fn build_rate_limiter(per_sec: u32) -> SharedRateLimiter {
    let quota = Quota::per_second(NonZeroU32::new(per_sec.max(1)).unwrap_or(NonZeroU32::MAX));
    RateLimiter::direct(quota)
}

#[cfg(test)]
mod session_hub_tests {
    use super::*;

    #[tokio::test]
    async fn background_cleanup_removes_stale_sessions_and_stops_on_shutdown() {
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/test")
            .unwrap();
        let (tx, _) = mpsc::channel(1);
        let state = AppState::new(
            crate::config::BootstrapFileConfig::default().into_dynamic(),
            db,
            tx,
        )
        .unwrap();
        let session = PlaybackSession {
            session_id: "stale".into(),
            user_id: "u".into(),
            device_id: "d".into(),
            device_name: "d".into(),
            client_name: "test".into(),
            current_episode_id: None,
            position_ticks: 0,
            duration_ticks: 0,
            next_ep_prewarmed: false,
            completed_in_session: false,
            last_activity: Instant::now() - Duration::from_secs(61),
        };
        state.sessions.insert("stale".into(), session.clone());
        state.sessions.insert(
            "active".into(),
            PlaybackSession {
                last_activity: Instant::now(),
                ..session
            },
        );
        let task = tokio::spawn(state.clone().run_session_cleanup(Duration::from_millis(1)));
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.sessions.contains_key("stale") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(state.sessions.contains_key("active"));
        state.shutdown_token.cancel();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn failed_login_window_limits_after_fifteen_attempts_case_insensitively() {
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/test")
            .unwrap();
        let (tx, _) = mpsc::channel(1);
        let state = AppState::new(
            crate::config::BootstrapFileConfig::default().into_dynamic(),
            db,
            tx,
        )
        .unwrap();
        for _ in 0..15 {
            state.record_failed_login("Alice", "192.0.2.1").await;
        }
        assert!(state.login_rate_limited("alice", "192.0.2.1").await);
        assert!(!state.login_rate_limited("bob", "192.0.2.2").await);
        for _ in 0..15 {
            state.record_failed_login("other", "192.0.2.3").await;
        }
        assert!(state.login_rate_limited("other", "192.0.2.3").await);
        for index in 0..30 {
            state
                .record_failed_login(&format!("distinct-user-{index}"), "192.0.2.4")
                .await;
        }
        assert!(
            state
                .login_rate_limited("different-user", "192.0.2.4")
                .await
        );
    }

    #[test]
    fn runtime_config_rejects_invalid_urls_and_cache_paths() {
        let mut cfg = crate::config::BootstrapFileConfig::default().into_dynamic();
        cfg.tmdb_api_base = "javascript:alert(1)".into();
        assert!(validate_runtime_config(&cfg).is_err());
        cfg.tmdb_api_base = "https://api.example.test".into();
        cfg.image_cache_dir = "../outside".into();
        assert!(validate_runtime_config(&cfg).is_err());
    }

    #[tokio::test]
    async fn hub_broadcasts_global_session_events_only_to_admin_connections() {
        let hub = SessionHub::default();
        let (_, mut admin, _) = hub.register_with_role("admin-token".into(), "root".into(), true);
        let (_, mut user, _) = hub.register("user-token".into(), "alice".into());
        hub.broadcast_admins("Sessions", serde_json::json!([{"UserId":"alice"}]));
        let message = admin.recv().await.unwrap();
        assert!(message.message.contains("alice"));
        assert!(user.try_recv().is_err());
    }

    #[test]
    fn hub_evicts_slow_connections_when_queue_is_full() {
        let hub = SessionHub::default();
        let (_, _rx, cancelled) = hub.register("t".into(), "u".into());
        for _ in 0..65 {
            hub.broadcast(None, "Sessions", serde_json::json!([]));
        }
        assert!(cancelled.is_cancelled());
        assert!(hub.connections.is_empty());
    }

    #[tokio::test]
    async fn hub_scopes_push_and_revokes_all_connections_for_token() {
        let hub = SessionHub::default();
        let (id, mut alice, cancelled) = hub.register("a".into(), "alice".into());
        let (_, mut bob, bob_cancelled) = hub.register("b".into(), "bob".into());
        let (_, _, second) = hub.register("a".into(), "alice".into());
        hub.broadcast(
            Some("alice"),
            "UserDataChanged",
            serde_json::json!({"Id":1}),
        );
        assert!(alice.recv().await.unwrap().kind == HubEventKind::UserDataChanged);
        assert!(bob.try_recv().is_err());
        hub.broadcast(None, "LibraryChanged", serde_json::json!({}));
        assert!(bob.recv().await.unwrap().kind == HubEventKind::LibraryChanged);
        hub.disconnect_tokens(&["a".into()]);
        assert!(cancelled.is_cancelled());
        assert!(second.is_cancelled());
        assert!(!bob_cancelled.is_cancelled());
        hub.unregister(&id);
    }

    #[tokio::test]
    async fn cache_maintenance_reclaims_invalidated_entries_and_weights_collages() {
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/test")
            .unwrap();
        let (tx, _) = mpsc::channel(1);
        let state = AppState::new(
            crate::config::BootstrapFileConfig::default().into_dynamic(),
            db,
            tx,
        )
        .unwrap();

        state
            .caches
            .image_hot_bytes
            .insert("w500/a.jpg".into(), bytes::Bytes::from(vec![1u8; 4096]))
            .await;
        state
            .caches
            .collage_cache
            .insert("collage:lib_tv:cards".into(), bytes::Bytes::from(vec![2u8; 8192]))
            .await;
        state
            .client_capabilities
            .insert("tok".into(), serde_json::json!({"SupportsMediaControl": true}));

        state.run_cache_maintenance().await;
        assert_eq!(state.caches.image_hot_bytes.weighted_size(), 4096);
        assert_eq!(state.caches.collage_cache.weighted_size(), 8192);
        assert_eq!(state.client_capabilities.entry_count(), 1);

        state.caches.image_hot_bytes.invalidate_all();
        state.caches.collage_cache.invalidate_all();
        state.client_capabilities.invalidate_all();
        state.run_cache_maintenance().await;

        assert_eq!(state.caches.image_hot_bytes.weighted_size(), 0);
        assert_eq!(state.caches.collage_cache.weighted_size(), 0);
        assert_eq!(state.client_capabilities.entry_count(), 0);
    }
}
