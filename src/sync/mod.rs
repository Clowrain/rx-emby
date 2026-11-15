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

enum FetchedSource {
    NotModified,
    Bytes { data: Vec<u8>, etag: Option<String> },
}

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
                    info!("定时同步周期到达，开始执行索引增量同步");
                    if let Err(e) = run_sync_once(&state, false).await {
                        error!("定时增量同步执行失败: {:#}", e);
                    }
                }
                _ = state.sync_ctrl.manual_trigger.notified() => {
                    info!("收到手动触发指令，立即执行全量对账同步");
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
