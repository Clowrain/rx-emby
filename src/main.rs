mod alist;
mod config;
mod image_cache;
mod monitor;
mod scraper;
mod smart_views;
mod state;
mod sync;
mod web;

use anyhow::{Context, Result};
use config::BootstrapFileConfig;
use sqlx::postgres::PgPoolOptions;
use state::AppState;
use tokio::sync::mpsc;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    state::configure_process_allocator();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rx_emby=info".into()),
        )
        .init();

    // 1. 加载本地启动配置（环境变量优先覆盖 config.toml，缺失 config.toml 时使用默认值）
    let file_cfg = BootstrapFileConfig::load_with_env("config.toml")?;
    let db_url = file_cfg.server.database_url.clone();
    let bind_addr = file_cfg.server.bind_addr.clone();

    // 2. 连接 PostgreSQL 并执行初始化建表迁移
    let db = PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .idle_timeout(std::time::Duration::from_secs(10 * 60))
        .max_lifetime(std::time::Duration::from_secs(30 * 60))
        .connect(&db_url)
        .await
        .context("连接 PostgreSQL 数据库失败，请确认 docker compose up -d 已运行")?;

    sqlx::migrate!("./migrations")
        .run(&db)
        .await
        .context("执行数据库初始化迁移脚本失败")?;
    info!("PostgreSQL 数据库连接及迁移初始化成功");

    // 3. 合并数据库中的动态热配置
    let dynamic_cfg = file_cfg.into_dynamic().sync_with_db(&db).await?;
    let (priority_tx, priority_rx) = mpsc::channel::<state::ScrapeTask>(1024);

    // 4. 初始化全局 AppState
    let app_state = AppState::new(dynamic_cfg, db, priority_tx)?;

    // 5. 预热 AList 自动登录
    match app_state.get_alist_token(false).await {
        Ok(_) => info!("AList 账号鉴权预热成功"),
        Err(e) => warn!(
            "AList 鉴权预热跳过（请在 config.toml 或后台配置真实账号密码）: {}",
            e
        ),
    }

    // 6. 拉起后台异步工作协程
    let session_cleanup = tokio::spawn(
        app_state
            .clone()
            .run_session_cleanup(std::time::Duration::from_secs(30)),
    );
    let sync_worker = sync::spawn_sync_worker(app_state.clone());
    let scraper_worker = scraper::spawn_scrape_worker(app_state.clone(), priority_rx);
    let image_cache_worker = image_cache::spawn_image_lru_worker(app_state.clone());
    let resource_monitor = monitor::spawn_resource_monitor(app_state.clone());

    // 7. 启动 Axum Web & Emby 协议服务
    let app = web::build_router(app_state.clone());
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("绑定监听地址失败: {}", bind_addr))?;

    info!("🚀 RX-Emby 服务已全面启动，监听地址: http://{}", bind_addr);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        #[cfg(unix)]
        {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("注册 SIGTERM 处理器失败");
            tokio::select! {
                result = tokio::signal::ctrl_c() => {
                    if let Err(error) = result {
                        warn!("监听 Ctrl-C 失败: {}", error);
                    }
                }
                _ = terminate.recv() => info!("收到 SIGTERM 信号"),
            }
        }
        #[cfg(not(unix))]
        if let Err(error) = tokio::signal::ctrl_c().await {
            warn!("监听 Ctrl-C 失败: {}", error);
        }
        info!("收到退出信号，正在优雅停止后台任务...");
        app_state.shutdown_token.cancel();
    })
    .await?;

    let workers = async {
        let (session, sync, scraper, image_cache, monitor) = tokio::join!(
            session_cleanup,
            sync_worker,
            scraper_worker,
            image_cache_worker,
            resource_monitor,
        );
        session.context("等待会话清理任务退出失败")?;
        sync.context("等待同步调度器退出失败")?;
        scraper.context("等待刮削调度器退出失败")?;
        image_cache.context("等待图片缓存任务退出失败")?;
        monitor.context("等待资源监控任务退出失败")?;
        Ok::<(), anyhow::Error>(())
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), workers)
        .await
        .context("后台任务在 10 秒内未能退出")??;
    Ok(())
}
