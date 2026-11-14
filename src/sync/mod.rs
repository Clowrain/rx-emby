pub mod batch_db;
pub mod parser;

use crate::state::AppState;
use anyhow::{Context, Result};
use batch_db::{apply_sync_diff, deduplicate_records};
use chrono::Utc;
use parser::parse_strm_line;
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};
use tokio::{fs, io::AsyncBufReadExt, time::sleep};
use tracing::{error, info, warn};
use xxhash_rust::xxh3::xxh3_64;

pub fn spawn_sync_worker(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut config_rx = state.config_tx.subscribe();
        let mut current_interval_sec = state.config.load().sync_interval_sec.max(60);

        info!("增量同步调度器已启动，当前同步周期: {} 秒", current_interval_sec);

        loop {
            let timer = sleep(Duration::from_secs(current_interval_sec));
            tokio::pin!(timer);

            tokio::select! {
                _ = state.shutdown_token.cancelled() => {
                    info!("收到停机信号，增量同步调度器退出");
                    break;
                }
                _ = &mut timer => {
                    info!("定时同步周期到达，开始执行 strm.txt 增量同步");
                    if let Err(e) = run_sync_once(&state, false).await {
                        error!("定时增量同步执行失败: {:#}", e);
                    }
                }
                _ = state.sync_ctrl.manual_trigger.notified() => {
                    info!("收到手动触发指令，立即执行 strm.txt 全量对账同步");
                    if let Err(e) = run_sync_once(&state, true).await {
                        error!("手动触发同步执行失败: {:#}", e);
                    }
                }
                Ok(_) = config_rx.changed() => {
                    let new_interval = config_rx.borrow().sync_interval_sec.max(60);
                    if new_interval != current_interval_sec {
                        current_interval_sec = new_interval;
                        info!("检测到同步周期热更新，已重置定时器为: {} 秒", current_interval_sec);
                    }
                }
            }
        }
    });
}

pub async fn run_sync_once(state: &AppState, force: bool) -> Result<()> {
    if state
        .sync_ctrl
        .is_syncing
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        warn!("已有同步任务正在运行中，跳过本次重复触发");
        return Ok(());
    }

    let res = execute_sync_pipeline(state, force).await;
    state.sync_ctrl.is_syncing.store(false, Ordering::SeqCst);
    res
}

async fn execute_sync_pipeline(state: &AppState, force: bool) -> Result<()> {
    let start_instant = Instant::now();
    let started_at = Utc::now();
    let cfg = state.config.load();
    let source = cfg.strm_txt_source.clone();
    let batch_size = cfg.db_batch_size;
    let max_lines = cfg.sync_max_lines_limit;

    let includes: Vec<String> = cfg
        .sync_include_prefixes
        .split(',')
        .map(|s| s.trim().trim_start_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let excludes: Vec<String> = cfg
        .sync_exclude_keywords
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let log_id: i64 = sqlx::query_scalar(
        "INSERT INTO sync_logs (started_at, status) VALUES ($1, 'running') RETURNING id",
    )
    .bind(started_at)
    .fetch_one(&state.db)
    .await?;

    let pipeline_res: Result<(i32, i32, i32, i32)> = async {
        let raw_bytes = if source.starts_with("http://") || source.starts_with("https://") {
            let client = state.alist_client.load();
            client
                .get(&source)
                .send()
                .await
                .with_context(|| format!("下载 strm.txt 失败: {}", source))?
                .error_for_status()?
                .bytes()
                .await?
                .to_vec()
        } else {
            fs::read(&source)
                .await
                .with_context(|| format!("读取本地 strm.txt 文件失败: {}", source))?
        };

        let file_hash = xxh3_64(&raw_bytes);
        if !force {
            let last_hash = *state.sync_ctrl.last_file_hash.read().await;
            if last_hash == Some(file_hash) {
                info!("strm.txt 文件哈希未变更 ({:#x})，跳过本次入库", file_hash);
                return Ok((0, 0, 0, 0));
            }
        }

        let mut lines = tokio::io::BufReader::new(raw_bytes.as_slice()).lines();
        let mut total_lines = 0_i32;
        let mut parsed_records = Vec::with_capacity(100_000);

        while let Some(line) = lines.next_line().await? {
            if max_lines > 0 && (total_lines as usize) >= max_lines {
                break;
            }
            total_lines += 1;

            if let Some(rec) = parse_strm_line(&line) {
                // 白名单过滤
                if !includes.is_empty()
                    && !includes.iter().any(|inc| rec.root_dir.starts_with(inc))
                {
                    continue;
                }
                // 黑名单关键词过滤
                if excludes
                    .iter()
                    .any(|exc| rec.root_dir.contains(exc) || rec.alist_path.contains(exc))
                {
                    continue;
                }
                parsed_records.push(rec);
            }
        }

        info!(
            "strm.txt 解析完成：读取行数 {}，命中有效视频行 {}，开始去重与多版本收敛...",
            total_lines,
            parsed_records.len()
        );

        let deduped = deduplicate_records(parsed_records);
        let stats = apply_sync_diff(&state.db, deduped, total_lines, batch_size).await?;

        *state.sync_ctrl.last_file_hash.write().await = Some(file_hash);
        *state.sync_ctrl.last_sync_time.write().await = Some(Utc::now());

        Ok((
            stats.total_lines,
            stats.added_episodes,
            stats.deleted_episodes,
            stats.new_media_items,
        ))
    }
    .await;

    let duration_ms = start_instant.elapsed().as_millis() as i64;
    let finished_at = Utc::now();

    match pipeline_res {
        Ok((total_lines, added, deleted, new_media)) => {
            sqlx::query(
                r#"
                UPDATE sync_logs SET
                    finished_at = $1, duration_ms = $2, total_lines = $3,
                    added_episodes = $4, deleted_episodes = $5, new_media_items = $6,
                    status = 'success'
                WHERE id = $7
                "#,
            )
            .bind(finished_at)
            .bind(duration_ms)
            .bind(total_lines)
            .bind(added)
            .bind(deleted)
            .bind(new_media)
            .bind(log_id)
            .execute(&state.db)
            .await?;

            info!(
                "同步任务完成 (耗时 {}ms): 新增剧集 {}, 清理失效 {}, 新增影视条目 {}",
                duration_ms, added, deleted, new_media
            );
            Ok(())
        }
        Err(e) => {
            let err_msg = format!("{:#}", e);
            let _ = sqlx::query(
                "UPDATE sync_logs SET finished_at = $1, duration_ms = $2, status = 'failed', error_message = $3 WHERE id = $4",
            )
            .bind(finished_at)
            .bind(duration_ms)
            .bind(&err_msg)
            .bind(log_id)
            .execute(&state.db)
            .await;
            Err(e)
        }
    }
}
