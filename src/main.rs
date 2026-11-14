mod alist;
mod config;
mod image_cache;
mod scraper;
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
    let (priority_tx, priority_rx) = mpsc::channel::<i64>(1024);

    // 4. 初始化全局 AppState
    let app_state = AppState::new(dynamic_cfg, db, priority_tx)?;

    // 5. 预热 AList 自动登录
    match app_state.get_alist_token(false).await {
        Ok(_) => info!("AList 账号鉴权预热成功"),
        Err(e) => warn!("AList 鉴权预热跳过（请在 config.toml 或后台配置真实账号密码）: {}", e),
    }

    // 6. 拉起三大后台异步工作协程
    sync::spawn_sync_worker(app_state.clone());
    scraper::spawn_scrape_worker(app_state.clone(), priority_rx);
    image_cache::spawn_image_lru_worker(app_state.clone());

    // 7. 启动 Axum Web & Emby 协议服务
    let app = web::build_router(app_state.clone());
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("绑定监听地址失败: {}", bind_addr))?;

    info!("🚀 RX-Emby 服务已全面启动，监听地址: http://{}", bind_addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            info!("收到退出信号，正在优雅停止后台任务...");
            app_state.shutdown_token.cancel();
        })
        .await?;

    Ok(())
}
