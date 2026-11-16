pub mod batch_db;
pub mod parser;

use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use batch_db::{apply_sync_diff, deduplicate_records};
use chrono::Utc;
use parser::parse_strm_line;
use reqwest::{Client, Proxy, StatusCode};
use std::{
    io::{Cursor, Read},
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};
use tokio::{fs, io::AsyncBufReadExt, time::sleep};
use tracing::{error, info, warn};
use xxhash_rust::xxh3::xxh3_64;

const MIN_SYNC_INTERVAL_SEC: u64 = 60;
const STARTUP_OVERDUE_GRACE: Duration = Duration::from_secs(15);
const CONFIG_RELOAD_GRACE: Duration = Duration::from_secs(1);
const SYNC_ERROR_RETRY_SEC: u64 = 300;
const SYNC_LAST_CHECK_KEY: &str = "sync.last_check_at";

enum FetchedSource {
    NotModified,
    Bytes { data: Vec<u8>, etag: Option<String> },
}

fn compute_next_run_at(
    last_sync_utc: Option<chrono::DateTime<Utc>>,
    interval: Duration,
    overdue_grace: Duration,
) -> tokio::time::Instant {
    let now = tokio::time::Instant::now();
    if let Some(last_utc) = last_sync_utc {
        let elapsed_sec = Utc::now()
            .signed_duration_since(last_utc)
            .num_seconds()
            .max(0) as u64;
        let interval_sec = interval.as_secs();
        if elapsed_sec >= interval_sec {
            now + overdue_grace
        } else {
            now + Duration::from_secs(interval_sec - elapsed_sec)
        }
    } else {
        now + interval
    }
}

fn remaining_wait_secs(next_run_at: tokio::time::Instant) -> u64 {
    next_run_at
        .saturating_duration_since(tokio::time::Instant::now())
        .as_secs()
}

fn schedule_after_sync(
    succeeded: bool,
    interval_sec: u64,
    cycle_anchor_utc: &mut chrono::DateTime<Utc>,
) -> tokio::time::Instant {
    let now = tokio::time::Instant::now();
    if succeeded {
        // 仅在同步/检查成功时推进周期锚点；失败时保持原锚点并按短退避重试
        *cycle_anchor_utc = Utc::now();
        now + Duration::from_secs(interval_sec)
    } else {
        now + Duration::from_secs(interval_sec.min(SYNC_ERROR_RETRY_SEC))
    }
}

async fn load_last_sync_utc(state: &AppState) -> Option<chrono::DateTime<Utc>> {
    let log_ts = match sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(
        "SELECT MAX(started_at) FROM sync_logs WHERE status = 'success'",
    )
    .fetch_one(&state.db)
    .await
    {
        Ok(ts) => ts,
        Err(e) => {
            warn!("读取最近成功同步时间失败，将回退到检查游标: {}", e);
            None
        }
    };

    let check_ts = load_persisted_cursor(state, SYNC_LAST_CHECK_KEY)
        .await
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    match (log_ts, check_ts) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

pub fn spawn_sync_worker(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut config_rx = state.config_tx.subscribe();
        let (mut current_interval_sec, mut current_source) = {
            let cfg = state.config.load();
            (
                cfg.sync_interval_sec.max(MIN_SYNC_INTERVAL_SEC),
                cfg.strm_txt_source.clone(),
            )
        };

        let last_sync_utc = load_last_sync_utc(&state).await;
        // 即使历史从未同步，也以启动时刻作为当前周期锚点，保证调整周期时保留已等待时长
        let mut cycle_anchor_utc = last_sync_utc.unwrap_or_else(Utc::now);
        let mut next_run_at = compute_next_run_at(
            last_sync_utc,
            Duration::from_secs(current_interval_sec),
            STARTUP_OVERDUE_GRACE,
        );

        info!(
            "增量同步调度器已启动，当前同步周期: {} 秒，距下次定时检查: {} 秒",
            current_interval_sec,
            remaining_wait_secs(next_run_at)
        );

        loop {
            let timer = tokio::time::sleep_until(next_run_at);
            tokio::pin!(timer);

            tokio::select! {
                _ = state.shutdown_token.cancelled() => {
                    info!("收到停机信号，增量同步调度器退出");
                    break;
                }
                _ = &mut timer => {
                    info!("定时同步周期到达，开始执行索引增量同步");
                    let ok = match run_sync_once(&state, false).await {
                        Ok(()) => true,
                        Err(e) => {
                            error!("定时增量同步执行失败: {:#}", e);
                            false
                        }
                    };
                    next_run_at = schedule_after_sync(ok, current_interval_sec, &mut cycle_anchor_utc);
                }
                _ = state.sync_ctrl.manual_trigger.notified() => {
                    info!("收到手动触发指令，立即执行全量对账同步");
                    let ok = match run_sync_once(&state, true).await {
                        Ok(()) => true,
                        Err(e) => {
                            error!("手动触发同步执行失败: {:#}", e);
                            false
                        }
                    };
                    next_run_at = schedule_after_sync(ok, current_interval_sec, &mut cycle_anchor_utc);
                }
                changed = config_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let (new_interval, new_source) = {
                        let cfg = config_rx.borrow_and_update();
                        (
                            cfg.sync_interval_sec.max(MIN_SYNC_INTERVAL_SEC),
                            cfg.strm_txt_source.clone(),
                        )
                    };

                    if new_source != current_source {
                        current_source = new_source;
                        current_interval_sec = new_interval;
                        // 索引源地址变更时清理旧 ETag/哈希游标，避免新源命中旧缓存并立即触发同步
                        *state.sync_ctrl.last_file_hash.write().await = None;
                        let _ = sqlx::query(
                            "DELETE FROM system_configs WHERE config_key IN ('sync.last_etag', 'sync.last_file_hash')",
                        )
                        .execute(&state.db)
                        .await;
                        next_run_at = tokio::time::Instant::now() + CONFIG_RELOAD_GRACE;
                        info!(
                            "检测到索引源变更为 {}，已清理旧游标并将在 {} 秒后触发同步",
                            current_source,
                            remaining_wait_secs(next_run_at)
                        );
                    } else if new_interval != current_interval_sec {
                        current_interval_sec = new_interval;
                        next_run_at = compute_next_run_at(
                            Some(cycle_anchor_utc),
                            Duration::from_secs(current_interval_sec),
                            CONFIG_RELOAD_GRACE,
                        );
                        info!(
                            "检测到同步周期热更新为 {} 秒，距下次定时检查: {} 秒",
                            current_interval_sec,
                            remaining_wait_secs(next_run_at)
                        );
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
    *state.sync_ctrl.progress.write().await = None;
    state.sync_ctrl.is_syncing.store(false, Ordering::SeqCst);
    res
}

async fn set_sync_progress(
    state: &AppState,
    log_id: i64,
    started_at: chrono::DateTime<chrono::Utc>,
    stage: &str,
    step: u8,
    percent: u8,
    message: impl Into<String>,
    scanned_lines: i32,
    valid_records: i32,
) {
    *state.sync_ctrl.progress.write().await = Some(crate::state::SyncProgress {
        log_id: Some(log_id),
        stage: stage.to_string(),
        step,
        total_steps: 5,
        percent: percent.min(100),
        message: message.into(),
        scanned_lines,
        valid_records,
        started_at,
    });
}

async fn load_persisted_cursor(state: &AppState, key: &str) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT config_value FROM system_configs WHERE config_key = $1",
    )
    .bind(key)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
}

async fn save_persisted_cursor(state: &AppState, key: &str, val: &str, desc: &str) {
    let _ = sqlx::query(
        r#"
        INSERT INTO system_configs (config_key, config_value, value_type, description, updated_at)
        VALUES ($1, $2, 'string', $3, NOW())
        ON CONFLICT (config_key) DO UPDATE SET
            config_value = EXCLUDED.config_value,
            updated_at = NOW()
        "#,
    )
    .bind(key)
    .bind(val)
    .bind(desc)
    .execute(&state.db)
    .await;
}

fn build_sync_http_client(proxy_url: Option<&str>) -> Result<Client> {
    let mut builder = Client::builder().timeout(Duration::from_secs(120));
    if let Some(p) = proxy_url.map(str::trim).filter(|s| !s.is_empty()) {
        let proxy = Proxy::all(p).with_context(|| format!("无效的同步代理地址: {}", p))?;
        builder = builder.proxy(proxy);
    }
    builder.build().context("构建同步 HTTP Client 失败")
}

async fn fetch_source_bytes(
    state: &AppState,
    source: &str,
    last_etag: Option<&str>,
    force: bool,
) -> Result<FetchedSource> {
    if source.starts_with("http://") || source.starts_with("https://") {
        let proxy = state.config.load().tmdb_proxy_url.clone();
        let client = build_sync_http_client(proxy.as_deref())?;
        let mut last_err = None;
        let mut resp_opt = None;
        for attempt in 1..=3 {
            let mut req = client.get(source);
            if !force {
                if let Some(etag) = last_etag {
                    req = req.header("If-None-Match", etag);
                }
            }
            match req.send().await {
                Ok(r) => {
                    resp_opt = Some(r);
                    break;
                }
                Err(e) => {
                    let is_last = attempt == 3;
                    if !is_last {
                        warn!(
                            "⚠️ [Sync] 下载索引源第 {} 次失败，1.5s 后重试 ({}): {}",
                            attempt, source, e
                        );
                        tokio::time::sleep(Duration::from_millis(1500)).await;
                    }
                    last_err = Some(e);
                }
            }
        }
        let resp = match resp_opt {
            Some(r) => r,
            None => {
                return Err(last_err.unwrap())
                    .with_context(|| format!("下载索引源失败: {}", source));
            }
        };
        if resp.status() == StatusCode::NOT_MODIFIED {
            return Ok(FetchedSource::NotModified);
        }
        let resp = resp.error_for_status()?;
        let new_etag = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let data = resp.bytes().await?.to_vec();
        Ok(FetchedSource::Bytes {
            data,
            etag: new_etag,
        })
    } else {
        let meta1 = fs::metadata(source)
            .await
            .with_context(|| format!("读取本地索引文件失败: {}", source))?;
        sleep(Duration::from_millis(300)).await;
        let meta2 = fs::metadata(source)
            .await
            .with_context(|| format!("读取本地索引文件失败: {}", source))?;
        if meta1.len() != meta2.len() {
            return Err(anyhow!(
                "本地索引文件 {} 正在写入中（大小变化 {} -> {}），跳过本次同步",
                source,
                meta1.len(),
                meta2.len()
            ));
        }
        let data = fs::read(source)
            .await
            .with_context(|| format!("读取本地索引文件失败: {}", source))?;
        Ok(FetchedSource::Bytes { data, etag: None })
    }
}

fn is_zip_payload(source: &str, raw_bytes: &[u8]) -> bool {
    let clean_source = source.split('?').next().unwrap_or(source).to_lowercase();
    clean_source.ends_with(".zip") || raw_bytes.starts_with(&[0x50, 0x4B, 0x03, 0x04])
}

async fn extract_txt_from_zip(raw_bytes: Vec<u8>) -> Result<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        let cursor = Cursor::new(raw_bytes);
        let mut archive = zip::ZipArchive::new(cursor).context("解析 .zip 压缩包失败")?;
        if archive.is_empty() {
            return Err(anyhow!(".zip 压缩包内无任何文件"));
        }

        let mut target_idx: Option<usize> = None;
        for i in 0..archive.len() {
            let file = archive.by_index(i)?;
            if file.is_dir() {
                continue;
            }
            let name = file.name().to_lowercase();
            if name == "strm.txt" || name.ends_with("/strm.txt") {
                target_idx = Some(i);
                break;
            }
            if target_idx.is_none() && name.ends_with(".txt") {
                target_idx = Some(i);
            }
        }

        let idx = target_idx.ok_or_else(|| anyhow!(".zip 压缩包内未找到 .txt 索引文件"))?;
        let mut entry = archive.by_index(idx)?;
        let mut out = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut out).context("解压 .txt 内容失败")?;
        Ok(out)
    })
    .await?
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

    let persisted_etag = load_persisted_cursor(state, "sync.last_etag").await;
    let mut mem_hash = *state.sync_ctrl.last_file_hash.read().await;
    if mem_hash.is_none() {
        if let Some(raw_h) = load_persisted_cursor(state, "sync.last_file_hash").await {
            if let Ok(parsed_h) = u64::from_str_radix(raw_h.trim_start_matches("0x"), 16) {
                mem_hash = Some(parsed_h);
                *state.sync_ctrl.last_file_hash.write().await = mem_hash;
            }
        }
    }

    let log_id: i64 = sqlx::query_scalar(
        "INSERT INTO sync_logs (started_at, status) VALUES ($1, 'running') RETURNING id",
    )
    .bind(started_at)
    .fetch_one(&state.db)
    .await?;

    set_sync_progress(
        state,
        log_id,
        started_at,
        "downloading",
        1,
        10,
        "正在下载/读取索引源…",
        0,
        0,
    )
    .await;

    let fetched = match fetch_source_bytes(state, &source, persisted_etag.as_deref(), force).await {
        Ok(FetchedSource::NotModified) => {
            info!("远程索引源 ETag 未变更 (304 Not Modified)，跳过本次同步");
            let now = Utc::now();
            *state.sync_ctrl.last_sync_time.write().await = Some(now);
            save_persisted_cursor(state, SYNC_LAST_CHECK_KEY, &now.to_rfc3339(), "最近一次索引检查时间").await;
            let _ = sqlx::query("DELETE FROM sync_logs WHERE id = $1")
                .bind(log_id)
                .execute(&state.db)
                .await;
            return Ok(());
        }
        Ok(FetchedSource::Bytes { data, etag }) => (data, etag),
        Err(e) => {
            let err_msg = format!("{:#}", e);
            let _ = sqlx::query(
                "UPDATE sync_logs SET finished_at = $1, duration_ms = $2, status = 'failed', error_message = $3 WHERE id = $4",
            )
            .bind(Utc::now())
            .bind(start_instant.elapsed().as_millis() as i64)
            .bind(&err_msg)
            .bind(log_id)
            .execute(&state.db)
            .await;
            return Err(e);
        }
    };

    let (raw_bytes, new_etag) = fetched;
    let file_hash = xxh3_64(&raw_bytes);
    if !force && mem_hash == Some(file_hash) {
        info!("索引文件哈希未变更 ({:#x})，跳过本次入库", file_hash);
        let now = Utc::now();
        *state.sync_ctrl.last_sync_time.write().await = Some(now);
        save_persisted_cursor(state, SYNC_LAST_CHECK_KEY, &now.to_rfc3339(), "最近一次索引检查时间").await;
        if let Some(ref etag) = new_etag {
            save_persisted_cursor(state, "sync.last_etag", etag, "最近一次同步远程索引 ETag").await;
        }
        let _ = sqlx::query("DELETE FROM sync_logs WHERE id = $1")
            .bind(log_id)
            .execute(&state.db)
            .await;
        return Ok(());
    }

    let pipeline_res: Result<(i32, i32, i32, i32, Vec<i64>)> = async {
        let txt_bytes = if is_zip_payload(&source, &raw_bytes) {
            set_sync_progress(
                state,
                log_id,
                started_at,
                "extracting",
                2,
                25,
                "正在解压 .zip 压缩索引…",
                0,
                0,
            )
            .await;
            info!("检测到 .zip 压缩索引源，正在内存中解压...");
            extract_txt_from_zip(raw_bytes).await?
        } else {
            raw_bytes
        };

        set_sync_progress(
            state,
            log_id,
            started_at,
            "parsing",
            3,
            35,
            "正在逐行解析索引内容…",
            0,
            0,
        )
        .await;

        let mut lines = tokio::io::BufReader::new(txt_bytes.as_slice()).lines();
        let mut total_lines = 0_i32;
        let mut parsed_records = Vec::with_capacity(100_000);

        while let Some(line) = lines.next_line().await? {
            if max_lines > 0 && (total_lines as usize) >= max_lines {
                break;
            }
            total_lines += 1;

            if let Some(rec) = parse_strm_line(&line) {
                if !includes.is_empty()
                    && !includes.iter().any(|inc| rec.root_dir.starts_with(inc))
                {
                    continue;
                }
                if excludes
                    .iter()
                    .any(|exc| rec.root_dir.contains(exc) || rec.alist_path.contains(exc))
                {
                    continue;
                }
                parsed_records.push(rec);
            }

            if total_lines % 20_000 == 0 {
                let pct = (35 + ((total_lines / 20_000).min(25) as u8)).min(60);
                set_sync_progress(
                    state,
                    log_id,
                    started_at,
                    "parsing",
                    3,
                    pct,
                    format!("正在解析索引行（已扫描 {} 行，命中 {} 条视频）…", total_lines, parsed_records.len()),
                    total_lines,
                    parsed_records.len() as i32,
                )
                .await;
            }
        }

        info!(
            "索引解析完成：读取行数 {}，命中有效视频行 {}，开始择优去重...",
            total_lines,
            parsed_records.len()
        );

        set_sync_progress(
            state,
            log_id,
            started_at,
            "deduplicating",
            4,
            68,
            format!("正在对 {} 条视频源择优去重…", parsed_records.len()),
            total_lines,
            parsed_records.len() as i32,
        )
        .await;

        let deduped = deduplicate_records(parsed_records);

        set_sync_progress(
            state,
            log_id,
            started_at,
            "writing_db",
            5,
            82,
            format!("正在增量比对并写入数据库（去重后 {} 条）…", deduped.len()),
            total_lines,
            deduped.len() as i32,
        )
        .await;

        let stats = apply_sync_diff(&state.db, deduped, total_lines, batch_size, force).await?;

        *state.sync_ctrl.last_file_hash.write().await = Some(file_hash);
        *state.sync_ctrl.last_sync_time.write().await = Some(Utc::now());
        save_persisted_cursor(
            state,
            "sync.last_file_hash",
            &format!("{:#x}", file_hash),
            "最近一次成功入库的索引文件哈希",
        )
        .await;
        if let Some(ref etag) = new_etag {
            save_persisted_cursor(state, "sync.last_etag", etag, "最近一次同步远程索引 ETag").await;
        }

        Ok((
            stats.total_lines,
            stats.added_episodes,
            stats.deleted_episodes,
            stats.new_media_items,
            stats.updated_folder_ids,
        ))
    }
    .await;

    let duration_ms = start_instant.elapsed().as_millis() as i64;
    let finished_at = Utc::now();

    match pipeline_res {
        Ok((total_lines, added, deleted, new_media, updated_folder_ids)) => {
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

            for fid in updated_folder_ids {
                let _ = state.scraper.priority_tx.try_send(crate::state::ScrapeTask::Folder(fid));
            }

            state.caches.admin_stats_cache.invalidate_all();

            info!(
                "同步任务完成 (耗时 {}ms): 新增视频源 {}, 清理失效 {}, 新增目录分组 {}",
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

#[cfg(test)]
mod tests {
    use super::{compute_next_run_at, remaining_wait_secs, schedule_after_sync, SYNC_ERROR_RETRY_SEC};
    use chrono::{Duration as ChronoDuration, Utc};
    use std::time::Duration;

    #[test]
    fn compute_next_run_triggers_after_grace_when_overdue() {
        let last = Utc::now() - ChronoDuration::seconds(7200);
        let next = compute_next_run_at(Some(last), Duration::from_secs(3600), Duration::from_secs(15));
        let wait = remaining_wait_secs(next);
        assert!((13..=15).contains(&wait));
    }

    #[test]
    fn compute_next_run_waits_remaining_interval_when_recent() {
        let last = Utc::now() - ChronoDuration::seconds(1000);
        let next = compute_next_run_at(Some(last), Duration::from_secs(3600), Duration::from_secs(15));
        let wait = remaining_wait_secs(next);
        assert!((2595..=2600).contains(&wait));
    }

    #[test]
    fn compute_next_run_waits_full_interval_when_never_synced() {
        let next = compute_next_run_at(None, Duration::from_secs(3600), Duration::from_secs(15));
        let wait = remaining_wait_secs(next);
        assert!((3595..=3600).contains(&wait));
    }

    #[test]
    fn schedule_after_sync_uses_short_backoff_on_failure_and_keeps_anchor() {
        let original_anchor = Utc::now() - ChronoDuration::seconds(5000);
        let mut anchor = original_anchor;
        let next = schedule_after_sync(false, 21600, &mut anchor);
        let wait = remaining_wait_secs(next);
        assert_eq!(anchor, original_anchor);
        assert!((SYNC_ERROR_RETRY_SEC - 2..=SYNC_ERROR_RETRY_SEC).contains(&wait));
    }

    #[test]
    fn schedule_after_sync_advances_anchor_on_success() {
        let original_anchor = Utc::now() - ChronoDuration::seconds(5000);
        let mut anchor = original_anchor;
        let next = schedule_after_sync(true, 3600, &mut anchor);
        let wait = remaining_wait_secs(next);
        assert!(anchor > original_anchor);
        assert!((3598..=3600).contains(&wait));
    }
}
