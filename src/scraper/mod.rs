pub mod tmdb;

async fn invalidate_admin_stats(state: &AppState) {
    for key in ["overview", "smart_view_options"] {
        state
            .caches
            .admin_stats_cache
            .invalidate(&key.to_string())
            .await;
    }
}

fn broadcast_verified_work(state: &AppState, work_id: i64, category: &str) {
    let library = match category {
        "电影" | "movie" => "lib_movie",
        "电视剧" | "tv" => "lib_tv",
        "动漫" | "anime" => "lib_anime",
        "纪录片" | "documentary" => "lib_doc",
        "综艺" | "variety" => "lib_variety",
        _ => return,
    };
    state.session_hub.broadcast(
        None,
        "LibraryChanged",
        serde_json::json!({
            "ItemsAdded": [format!("work_{work_id}")], "ItemsUpdated": [format!("work_{work_id}")],
            "ItemsRemoved": [], "FoldersAddedTo": [library], "FoldersRemovedFrom": []
        }),
    );
}

use crate::state::{AppState, ScrapeTask};
use anyhow::Result;
use chrono::{Datelike, Duration as ChronoDuration, NaiveDate, Utc};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tmdb::ScrapedMetadata;
use tokio::{
    sync::{mpsc, Notify},
    time::sleep,
};
use tracing::{debug, error, info};

async fn invalidate_work_image_meta(
    cache: &moka::future::Cache<String, Option<(String, String)>>,
    work_id: i64,
) {
    for key in [
        format!("work_{work_id}:Primary"),
        format!("work_{work_id}:Backdrop"),
        format!("media_{work_id}:Primary"),
        format!("media_{work_id}:Backdrop"),
    ] {
        cache.invalidate(&key).await;
    }
}

#[derive(sqlx::FromRow, Debug)]
struct PendingFolderRow {
    id: i64,
    root_dir: String,
    inferred_media_type: String,
    category: String,
    parsed_tmdb_id: Option<i32>,
    clean_title: String,
    year: Option<i32>,
    retry_count: i16,
    scrape_state: String,
    work_id: Option<i64>,
    is_locked: bool,
}

#[derive(sqlx::FromRow, Debug)]
struct WorkSummaryRow {
    id: i64,
    media_type: String,
    tmdb_id: Option<i32>,
    title: String,
    original_title: Option<String>,
    year: Option<i32>,
    is_locked: bool,
    metadata_version: i16,
    metadata_refresh_requested: bool,
    metadata_retry_count: i16,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ScrapeTaskRecord {
    pub id: u64,
    pub task_name: String,
    pub status: String,
    pub started_at: chrono::DateTime<Utc>,
    pub finished_at: Option<chrono::DateTime<Utc>>,
    pub total: usize,
    pub done: usize,
    pub succeeded: usize,
    pub failed: usize,
}

/// 所有刮削入口共享活动状态与暂停/取消/批次进度控制
#[derive(Default)]
pub struct ScrapeRuntime {
    active: AtomicUsize,
    manual_sweep: AtomicBool,
    paused: AtomicBool,
    auto_suppressed: AtomicBool,
    last_finished_unix: AtomicI64,
    batch_total: AtomicUsize,
    batch_done: AtomicUsize,
    batch_succeeded: AtomicUsize,
    batch_failed: AtomicUsize,
    next_history_id: AtomicU64,
    task_name: std::sync::Mutex<Option<String>>,
    current_item: std::sync::Mutex<Option<String>>,
    history: std::sync::Mutex<VecDeque<ScrapeTaskRecord>>,
    wake: Notify,
}

pub struct ScrapeSnapshot {
    pub running: bool,
    pub current_progress_percentage: Option<f64>,
    pub last_execution_finished: Option<chrono::DateTime<Utc>>,
}

struct ScrapeActivity<'a>(&'a ScrapeRuntime);

impl Drop for ScrapeActivity<'_> {
    fn drop(&mut self) {
        if self.0.active.fetch_sub(1, Ordering::Relaxed) == 1 {
            self.0
                .last_finished_unix
                .store(Utc::now().timestamp(), Ordering::Relaxed);
        }
    }
}

impl ScrapeRuntime {
    pub fn snapshot(&self) -> ScrapeSnapshot {
        let running = self.active.load(Ordering::Relaxed) > 0;
        let finished = self.last_finished_unix.load(Ordering::Relaxed);
        let total = self.batch_total.load(Ordering::Relaxed);
        let done = self.batch_done.load(Ordering::Relaxed);
        let pct = if running && total > 0 {
            Some(((done as f64 / total as f64) * 100.0).clamp(0.0, 100.0))
        } else {
            running.then_some(0.0)
        };
        ScrapeSnapshot {
            running,
            current_progress_percentage: pct,
            last_execution_finished: (finished > 0)
                .then(|| chrono::DateTime::from_timestamp(finished, 0))
                .flatten(),
        }
    }

    fn begin_task(&self) -> ScrapeActivity<'_> {
        self.active.fetch_add(1, Ordering::Relaxed);
        ScrapeActivity(self)
    }

    pub fn request_sweep(&self) {
        self.auto_suppressed.store(false, Ordering::SeqCst);
        self.manual_sweep.store(true, Ordering::Relaxed);
        self.wake.notify_one();
    }

    fn push_history_record(&self, name: &str, total: usize) {
        let id = self.next_history_id.fetch_add(1, Ordering::Relaxed) + 1;
        if let Ok(mut hist) = self.history.lock() {
            if let Some(front) = hist.front_mut() {
                if matches!(front.status.as_str(), "running" | "paused") {
                    front.status = "completed".to_string();
                    front.finished_at = Some(Utc::now());
                }
            }
            hist.push_front(ScrapeTaskRecord {
                id,
                task_name: name.to_string(),
                status: "running".to_string(),
                started_at: Utc::now(),
                finished_at: None,
                total,
                done: 0,
                succeeded: 0,
                failed: 0,
            });
            while hist.len() > 20 {
                hist.pop_back();
            }
        }
    }

    fn sync_front_history(&self, new_status: Option<&str>) {
        if let Ok(mut hist) = self.history.lock() {
            let done = self.batch_done.load(Ordering::Relaxed);
            let succeeded = self.batch_succeeded.load(Ordering::Relaxed);
            let failed = self.batch_failed.load(Ordering::Relaxed);
            let total = self.batch_total.load(Ordering::Relaxed);
            let front_is_active = hist
                .front()
                .is_some_and(|f| matches!(f.status.as_str(), "running" | "paused"));

            if front_is_active {
                if let Some(front) = hist.front_mut() {
                    front.done = done;
                    front.succeeded = succeeded;
                    front.failed = failed;
                    if total > 0 {
                        front.total = total;
                    } else if done > front.total {
                        front.total = done;
                    }
                    if let Some(st) = new_status {
                        front.status = st.to_string();
                        if matches!(st, "completed" | "cancelled") {
                            front.finished_at = Some(Utc::now());
                        } else {
                            front.finished_at = None;
                        }
                    }
                }
            } else if new_status.is_none() && done > 0 {
                let id = self.next_history_id.fetch_add(1, Ordering::Relaxed) + 1;
                hist.push_front(ScrapeTaskRecord {
                    id,
                    task_name: "后台全库自动刮削".to_string(),
                    status: "running".to_string(),
                    started_at: Utc::now(),
                    finished_at: None,
                    total: total.max(done),
                    done,
                    succeeded,
                    failed,
                });
                while hist.len() > 20 {
                    hist.pop_back();
                }
            }
        }
    }

    pub fn start_batch(&self, name: impl Into<String>, total: usize) {
        let name_str = name.into();
        self.paused.store(false, Ordering::SeqCst);
        self.auto_suppressed.store(false, Ordering::SeqCst);
        self.batch_total.store(total, Ordering::SeqCst);
        self.batch_done.store(0, Ordering::SeqCst);
        self.batch_succeeded.store(0, Ordering::SeqCst);
        self.batch_failed.store(0, Ordering::SeqCst);
        if let Ok(mut guard) = self.task_name.lock() {
            *guard = Some(name_str.clone());
        }
        if let Ok(mut guard) = self.current_item.lock() {
            *guard = None;
        }
        self.push_history_record(&name_str, total);
        self.manual_sweep.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }

    pub fn add_batch_items(&self, name: impl Into<String>, count: usize) {
        if count == 0 {
            return;
        }
        let name_str = name.into();
        self.paused.store(false, Ordering::SeqCst);
        self.auto_suppressed.store(false, Ordering::SeqCst);
        let prev_total = self.batch_total.load(Ordering::SeqCst);
        let prev_done = self.batch_done.load(Ordering::SeqCst);
        if prev_total == 0 || prev_done >= prev_total {
            self.batch_total.store(count, Ordering::SeqCst);
            self.batch_done.store(0, Ordering::SeqCst);
            self.batch_succeeded.store(0, Ordering::SeqCst);
            self.batch_failed.store(0, Ordering::SeqCst);
            if let Ok(mut guard) = self.task_name.lock() {
                *guard = Some(name_str.clone());
            }
            self.push_history_record(&name_str, count);
        } else {
            self.batch_total.fetch_add(count, Ordering::SeqCst);
            self.sync_front_history(Some("running"));
        }
        self.manual_sweep.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }

    pub fn set_current_item(&self, label: Option<String>) {
        if let Ok(mut guard) = self.current_item.lock() {
            *guard = label;
        }
    }

    pub fn record_item_result(&self, succeeded: bool) {
        self.batch_done.fetch_add(1, Ordering::Relaxed);
        if succeeded {
            self.batch_succeeded.fetch_add(1, Ordering::Relaxed);
        } else {
            self.batch_failed.fetch_add(1, Ordering::Relaxed);
        }
        self.sync_front_history(None);
        self.set_current_item(None);
    }

    pub fn clear_idle_counters_if_done(&self) {
        if self.active.load(Ordering::Relaxed) == 0 && !self.paused.load(Ordering::Relaxed) {
            self.sync_front_history(Some("completed"));
            self.batch_total.store(0, Ordering::SeqCst);
            self.batch_done.store(0, Ordering::SeqCst);
            self.batch_succeeded.store(0, Ordering::SeqCst);
            self.batch_failed.store(0, Ordering::SeqCst);
            if let Ok(mut guard) = self.task_name.lock() {
                *guard = None;
            }
        }
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        self.sync_front_history(Some("paused"));
        self.wake.notify_one();
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.auto_suppressed.store(false, Ordering::SeqCst);
        self.sync_front_history(Some("running"));
        self.manual_sweep.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }

    pub fn cancel(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.manual_sweep.store(false, Ordering::SeqCst);
        self.auto_suppressed.store(true, Ordering::SeqCst);
        self.sync_front_history(Some("cancelled"));
        self.batch_total.store(0, Ordering::SeqCst);
        self.batch_done.store(0, Ordering::SeqCst);
        self.batch_succeeded.store(0, Ordering::SeqCst);
        self.batch_failed.store(0, Ordering::SeqCst);
        if let Ok(mut guard) = self.task_name.lock() {
            *guard = None;
        }
        if let Ok(mut guard) = self.current_item.lock() {
            *guard = None;
        }
        self.wake.notify_one();
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn is_auto_suppressed(&self) -> bool {
        self.auto_suppressed.load(Ordering::Relaxed)
    }

    pub fn status_json(
        &self,
        pending_db_count: i64,
        refresh_db_count: i64,
        auto_bg: bool,
    ) -> serde_json::Value {
        let active = self.active.load(Ordering::Relaxed) > 0;
        let paused = self.paused.load(Ordering::Relaxed);
        let manual_sweep = self.manual_sweep.load(Ordering::Relaxed);
        let auto_suppressed = self.auto_suppressed.load(Ordering::Relaxed);
        let batch_total = self.batch_total.load(Ordering::Relaxed);
        let batch_done = self.batch_done.load(Ordering::Relaxed);
        let batch_succeeded = self.batch_succeeded.load(Ordering::Relaxed);
        let batch_failed = self.batch_failed.load(Ordering::Relaxed);
        let task_name = self.task_name.lock().ok().and_then(|g| g.clone());
        let current_item = self.current_item.lock().ok().and_then(|g| g.clone());
        let pending_remaining = (pending_db_count + refresh_db_count).max(0);

        let batch_active = batch_total > 0 && batch_done < batch_total;
        let auto_active = auto_bg && !auto_suppressed && pending_remaining > 0;
        let refresh_active = !auto_suppressed && refresh_db_count > 0;
        let running = !paused
            && (active
                || batch_active
                || (manual_sweep && pending_remaining > 0)
                || auto_active
                || refresh_active);
        let status = if paused {
            "paused"
        } else if running {
            "running"
        } else {
            "idle"
        };

        let effective_total = if batch_total > 0 {
            batch_total.max(batch_done + pending_remaining as usize)
        } else if auto_active
            || refresh_active
            || (paused && pending_remaining > 0)
            || (active && pending_remaining > 0)
        {
            batch_done + pending_remaining as usize
        } else {
            0
        };

        let effective_task_name = task_name.or_else(|| {
            if refresh_active {
                Some("补齐 TMDB 扩展信息".to_string())
            } else if auto_active || (paused && auto_bg && pending_remaining > 0) {
                Some("后台全库自动刮削".to_string())
            } else {
                None
            }
        });

        let percent = if effective_total > 0 {
            Some(
                ((batch_done.min(effective_total) as f64 / effective_total as f64) * 100.0 * 10.0)
                    .round()
                    / 10.0,
            )
        } else {
            None
        };

        let can_resume_auto = status == "idle" && auto_suppressed && pending_remaining > 0;
        let history: Vec<ScrapeTaskRecord> = self
            .history
            .lock()
            .map(|h| h.iter().cloned().collect())
            .unwrap_or_default();

        serde_json::json!({
            "status": status,
            "running": status == "running",
            "paused": paused,
            "auto_suppressed": auto_suppressed,
            "can_resume_auto": can_resume_auto,
            "task_name": effective_task_name,
            "current_item": current_item,
            "batch_total": effective_total,
            "batch_done": batch_done.min(effective_total),
            "batch_succeeded": batch_succeeded,
            "batch_failed": batch_failed,
            "pending_remaining": pending_remaining,
            "percent": percent,
            "history": history,
        })
    }
}

/// 将缺少 TMDB 扩展元数据的正式作品加入既有持久化刷新队列，并唤醒目录扫描。
pub async fn request_scheduled_scrape(state: &AppState) -> Result<()> {
    let cfg = state.config.load();
    anyhow::ensure!(
        !cfg.tmdb_api_key.is_empty() && cfg.tmdb_api_key != "your_tmdb_api_key",
        "尚未配置 TMDB API Key"
    );
    let res = sqlx::query(
        r#"
        UPDATE media_works
        SET metadata_refresh_requested = TRUE,
            metadata_retry_count = 0,
            metadata_retry_at = NULL
        WHERE NOT is_locked AND tmdb_id IS NOT NULL
          AND (metadata_version < 2 OR metadata_fetched_at IS NULL)
        "#,
    )
    .execute(&state.db)
    .await?;
    if res.rows_affected() > 0 {
        state
            .scraper
            .runtime
            .add_batch_items("补齐正式作品 TMDB 元数据", res.rows_affected() as usize);
    } else {
        state.scraper.runtime.request_sweep();
    }
    Ok(())
}

/// 启动后台四级优先级智能刮削协程
pub fn spawn_scrape_worker(
    state: Arc<AppState>,
    mut priority_rx: mpsc::Receiver<ScrapeTask>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!("TMDB 智能刮削调度器已启动（支持 Staging 校验、P0 实时插队与分集追更补齐）");
        let mut poll_delay = Duration::from_millis(200);
        loop {
            if state.scraper.runtime.is_auto_suppressed() {
                while priority_rx.try_recv().is_ok() {}
            }
            if state.scraper.runtime.is_paused() {
                tokio::select! {
                    _ = state.shutdown_token.cancelled() => {
                        info!("收到停机信号，刮削调度器退出");
                        break;
                    }
                    _ = state.scraper.runtime.wake.notified() => continue,
                    _ = sleep(Duration::from_secs(2)) => continue,
                }
            }
            let task = tokio::select! {
                _ = state.shutdown_token.cancelled() => {
                    info!("收到停机信号，刮削调度器退出");
                    break;
                }
                Some(task) = priority_rx.recv() => Some((task, true)),
                _ = state.scraper.runtime.wake.notified() => None,
                _ = sleep(poll_delay) => None,
            };
            if state.scraper.runtime.is_auto_suppressed() {
                while priority_rx.try_recv().is_ok() {}
                poll_delay = Duration::from_secs(3);
                continue;
            }
            if state.scraper.runtime.is_paused() {
                if let Some((t, _)) = task {
                    let _ = state.scraper.priority_tx.try_send(t);
                }
                continue;
            }
            let next_task = match task {
                Some(task) => Some(task),
                None => match fetch_next_pending_task(&state).await {
                    Ok(task) => task.map(|task| (task, false)),
                    Err(e) => {
                        error!("查询待刮削队列异常: {:#}", e);
                        poll_delay = Duration::from_secs(5);
                        continue;
                    }
                },
            };
            let Some((task, priority)) = next_task else {
                state.scraper.runtime.set_current_item(None);
                state.scraper.runtime.clear_idle_counters_if_done();
                poll_delay = Duration::from_secs(3);
                continue;
            };
            let result = match task {
                ScrapeTask::Folder(fid) => {
                    scrape_single_folder_with_mode(&state, fid, priority).await
                }
                ScrapeTask::WorkSeasons(wid) => {
                    scrape_work_seasons_by_id(&state, wid, priority).await
                }
            };
            state
                .scraper
                .runtime
                .record_item_result(result.is_ok());
            poll_delay = if let Err(e) = result {
                debug!("刮削任务 {:?} 失败: {:#}", task, e);
                Duration::from_secs(1)
            } else {
                Duration::from_millis(200)
            };
        }
    })
}

async fn fetch_next_pending_task(state: &AppState) -> Result<Option<ScrapeTask>> {
    let manual_sweep = state
        .scraper
        .runtime
        .manual_sweep
        .swap(false, Ordering::Relaxed);
    let result = fetch_next_pending_task_with_mode(state, manual_sweep).await;
    if manual_sweep && result.is_err() {
        state
            .scraper
            .runtime
            .manual_sweep
            .store(true, Ordering::SeqCst);
    } else if manual_sweep && !matches!(&result, Ok(None)) {
        state
            .scraper
            .runtime
            .manual_sweep
            .store(true, Ordering::Relaxed);
    }
    result
}

async fn fetch_next_pending_task_with_mode(
    state: &AppState,
    manual_sweep: bool,
) -> Result<Option<ScrapeTask>> {
    let cfg = state.config.load();
    if cfg.tmdb_api_key.is_empty() || cfg.tmdb_api_key == "your_tmdb_api_key" {
        return Ok(None);
    }
    if state.scraper.runtime.is_auto_suppressed() && !manual_sweep {
        return Ok(None);
    }

    // 1. 显式触发的正式作品元数据补齐任务
    let explicit_refresh: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM media_works
        WHERE NOT is_locked
          AND metadata_refresh_requested = TRUE
          AND (metadata_retry_at IS NULL OR metadata_retry_at <= NOW())
          AND metadata_retry_count < 3
        ORDER BY metadata_version ASC, id ASC
        LIMIT 1
        "#,
    )
    .fetch_optional(&state.db)
    .await?;
    if let Some(wid) = explicit_refresh {
        return Ok(Some(ScrapeTask::WorkSeasons(wid)));
    }

    if !cfg.scraper_auto_background && !manual_sweep {
        return Ok(None);
    }

    // 2a. 待刮削中间表目录（pending）
    let pending_fid: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM sync_folders
        WHERE scrape_state = 'pending' AND is_ignored = FALSE AND is_locked = FALSE
        ORDER BY scrape_priority ASC, id DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(&state.db)
    .await?;
    if let Some(fid) = pending_fid {
        return Ok(Some(ScrapeTask::Folder(fid)));
    }

    // 2b. 到期重试的失败目录（failed）
    let retry_fid: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM sync_folders
        WHERE scrape_state = 'failed'
          AND is_ignored = FALSE
          AND is_locked = FALSE
          AND retry_count < 3
          AND (next_retry_at IS NULL OR next_retry_at <= NOW())
        ORDER BY scrape_priority ASC, id DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(&state.db)
    .await?;
    if let Some(fid) = retry_fid {
        return Ok(Some(ScrapeTask::Folder(fid)));
    }

    // 3. 正式剧集作品存在未刮削季 / 增量新集 / 连载残缺集
    let cooldown_cutoff = Utc::now() - ChronoDuration::hours(24);
    let tv_wid: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT w.id
        FROM media_works w
        WHERE w.media_type = 'tv'
          AND w.tmdb_id IS NOT NULL
          AND EXISTS (
              SELECT 1 FROM sync_folders f
              WHERE f.work_id = w.id AND f.is_ignored = FALSE
          )
          AND (
              EXISTS (
                  SELECT 1 FROM sync_files sf
                  JOIN sync_folders f ON f.id = sf.folder_id
                  WHERE f.work_id = w.id AND f.is_ignored = FALSE
                    AND NOT EXISTS (
                        SELECT 1 FROM work_seasons ws
                        WHERE ws.work_id = w.id AND ws.season_number = sf.season_number
                    )
              )
              OR EXISTS (
                  SELECT 1 FROM work_seasons ws
                  WHERE ws.work_id = w.id AND ws.scrape_attempts = 0
              )
              OR EXISTS (
                  SELECT 1 FROM work_episodes we
                  JOIN work_seasons ws ON ws.work_id = we.work_id AND ws.season_number = we.season_number
                  WHERE we.work_id = w.id
                    AND we.meta_incomplete = TRUE
                    AND we.updated_at <= $1
                    AND ws.scrape_attempts BETWEEN 1 AND 4
              )
          )
        ORDER BY w.id DESC
        LIMIT 1
        "#,
    )
    .bind(cooldown_cutoff)
    .fetch_optional(&state.db)
    .await?;
    if let Some(wid) = tv_wid {
        return Ok(Some(ScrapeTask::WorkSeasons(wid)));
    }

    // 4. 被动刷新过期作品元数据（仅限仍有有效未忽略物理目录的在库作品）
    let stale_wid: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT w.id FROM media_works w
        WHERE NOT w.is_locked
          AND w.tmdb_id IS NOT NULL
          AND EXISTS (
              SELECT 1 FROM sync_folders f
              WHERE f.work_id = w.id AND f.is_ignored = FALSE
          )
          AND (w.metadata_retry_at IS NULL OR w.metadata_retry_at <= NOW())
          AND w.metadata_retry_count < 3
          AND (
              w.metadata_version < 2
              OR w.metadata_fetched_at < NOW() - CASE
                  WHEN w.tmdb_metadata->>'status' = 'Returning Series' THEN INTERVAL '1 day'
                  ELSE INTERVAL '7 days'
              END
          )
        ORDER BY w.id ASC
        LIMIT 1
        "#,
    )
    .fetch_optional(&state.db)
    .await?;

    Ok(stale_wid.map(ScrapeTask::WorkSeasons))
}

fn normalize_for_match(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(c))
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn titles_similar(local_title: &str, tmdb_title: &str, tmdb_orig: Option<&str>) -> bool {
    let a = normalize_for_match(local_title);
    if a.chars().count() < 2 {
        return true;
    }

    for cand_raw in std::iter::once(tmdb_title).chain(tmdb_orig) {
        let b = normalize_for_match(cand_raw);
        if b.is_empty() {
            continue;
        }
        if a.contains(&b) || b.contains(&a) {
            return true;
        }
        let set_a: HashSet<char> = a.chars().collect();
        let set_b: HashSet<char> = b.chars().collect();
        let common = set_a.intersection(&set_b).count();
        let min_len = set_a.len().min(set_b.len()).max(1);
        if (common as f64 / min_len as f64) >= 0.45 {
            return true;
        }
    }
    false
}

struct TmdbMatch<'a> {
    clean_title: &'a str,
    local_year: Option<i32>,
    ep_count: i64,
    max_season: i32,
    strict_title_check: bool,
    media_type: &'a str,
    title: &'a str,
    original_title: Option<&'a str>,
    year: Option<i32>,
}

/// 4 维度交叉校验：基本类型、片名相似度（含原名）、发行年份容差、本地多集结构
fn verify_tmdb_match(candidate: TmdbMatch<'_>) -> std::result::Result<(), String> {
    let TmdbMatch {
        clean_title,
        local_year,
        ep_count,
        max_season,
        strict_title_check,
        media_type: tmdb_media_type,
        title: tmdb_title,
        original_title: tmdb_orig,
        year: tmdb_year,
    } = candidate;
    if (ep_count > 1 || max_season > 0) && tmdb_media_type != "tv" {
        return Err(format!(
            "类型冲突：本地包含多集/季({}集, S{})，TMDB 返回电影《{}》",
            ep_count, max_season, tmdb_title
        ));
    }

    let title_ok = titles_similar(clean_title, tmdb_title, tmdb_orig);
    let year_diff = match (local_year, tmdb_year) {
        (Some(ly), Some(ty)) => Some((ly - ty).abs()),
        _ => None,
    };

    if strict_title_check && !title_ok {
        return Err(format!(
            "片名冲突：本地「{}」与 TMDB「{} / {}」差异过大",
            clean_title,
            tmdb_title,
            tmdb_orig.unwrap_or("-")
        ));
    }

    if !title_ok && year_diff.unwrap_or(0) > 2 {
        return Err(format!(
            "片名与年份双重冲突：本地「{}({})」vs TMDB「{}({})」",
            clean_title,
            local_year.unwrap_or(0),
            tmdb_title,
            tmdb_year.unwrap_or(0)
        ));
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkUpsertMode {
    /// 新目录入库、多源归并或人工绑定：推进 `media_works.updated_at`
    ContentChange,
    /// 后台周期保鲜 / 按需补齐 TMDB 元数据：仅更新 `metadata_fetched_at`，保持 `updated_at` 不变
    MetadataRefreshOnly,
}

impl WorkUpsertMode {
    const fn touches_updated_at(self) -> bool {
        matches!(self, Self::ContentChange)
    }
}

async fn upsert_media_work(
    state: &AppState,
    meta: &ScrapedMetadata,
    mode: WorkUpsertMode,
) -> Result<i64> {
    let previous_images: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT poster_path, backdrop_path FROM media_works WHERE tmdb_id = $1 AND media_type = $2",
    )
    .bind(meta.tmdb_id)
    .bind(&meta.media_type)
    .fetch_optional(&state.db)
    .await?;
    let image_metadata_changed = previous_images
        .as_ref()
        .map(|(poster, backdrop)| poster != &meta.poster_path || backdrop != &meta.backdrop_path)
        .unwrap_or(meta.poster_path.is_some() || meta.backdrop_path.is_some());
    let extra_json = serde_json::to_value(&meta.extra)?;
    let wid: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO media_works (
            media_type, tmdb_id, title, original_title, year, premiered_at, overview,
            rating, poster_path, backdrop_path, countries, networks, genres, category,
            tmdb_metadata, metadata_version, metadata_fetched_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, 2, NOW())
        ON CONFLICT (tmdb_id, media_type) WHERE tmdb_id IS NOT NULL DO UPDATE SET
            title = CASE WHEN media_works.is_locked THEN media_works.title ELSE EXCLUDED.title END,
            original_title = CASE WHEN media_works.is_locked THEN media_works.original_title ELSE EXCLUDED.original_title END,
            year = CASE WHEN media_works.is_locked THEN media_works.year ELSE COALESCE(EXCLUDED.year, media_works.year) END,
            premiered_at = CASE WHEN media_works.is_locked THEN media_works.premiered_at ELSE EXCLUDED.premiered_at END,
            overview = CASE WHEN media_works.is_locked THEN media_works.overview ELSE EXCLUDED.overview END,
            rating = CASE WHEN media_works.is_locked THEN media_works.rating ELSE EXCLUDED.rating END,
            poster_path = CASE WHEN media_works.is_locked THEN media_works.poster_path ELSE EXCLUDED.poster_path END,
            backdrop_path = CASE WHEN media_works.is_locked THEN media_works.backdrop_path ELSE EXCLUDED.backdrop_path END,
            countries = CASE WHEN media_works.is_locked THEN media_works.countries ELSE EXCLUDED.countries END,
            networks = CASE WHEN media_works.is_locked THEN media_works.networks ELSE EXCLUDED.networks END,
            genres = CASE WHEN media_works.is_locked THEN media_works.genres ELSE EXCLUDED.genres END,
            category = CASE WHEN media_works.is_locked THEN media_works.category ELSE EXCLUDED.category END,
            tmdb_metadata = EXCLUDED.tmdb_metadata,
            metadata_version = 2,
            metadata_fetched_at = NOW(),
            metadata_refresh_requested = FALSE,
            metadata_retry_count = 0,
            metadata_retry_at = NULL,
            updated_at = CASE WHEN $16 THEN NOW() ELSE media_works.updated_at END
        RETURNING id
        "#,
    )
    .bind(&meta.media_type)
    .bind(meta.tmdb_id)
    .bind(&meta.title)
    .bind(&meta.original_title)
    .bind(meta.year)
    .bind(&meta.premiered_at)
    .bind(&meta.overview)
    .bind(meta.rating)
    .bind(&meta.poster_path)
    .bind(&meta.backdrop_path)
    .bind(&meta.countries)
    .bind(&meta.networks)
    .bind(&meta.genres)
    .bind(&meta.category)
    .bind(extra_json)
    .bind(mode.touches_updated_at())
    .fetch_one(&state.db)
    .await?;

    if image_metadata_changed {
        invalidate_work_image_meta(&state.caches.image_meta_cache, wid).await;
    }
    state.caches.metadata_tag_map_cache.invalidate_all();
    Ok(wid)
}

pub async fn scrape_single_folder(state: &AppState, folder_id: i64) -> Result<()> {
    scrape_single_folder_with_mode(state, folder_id, true).await
}

pub async fn scrape_single_folder_with_mode(
    state: &AppState,
    folder_id: i64,
    is_p0: bool,
) -> Result<()> {
    let _activity = state.scraper.runtime.begin_task();
    let row: Option<PendingFolderRow> = sqlx::query_as(
        "SELECT id, root_dir, inferred_media_type, category, parsed_tmdb_id, clean_title, year, retry_count, scrape_state, work_id, is_locked FROM sync_folders WHERE id = $1",
    )
    .bind(folder_id)
    .fetch_optional(&state.db)
    .await?;

    let Some(mut folder) = row else {
        return Ok(());
    };
    state
        .scraper
        .runtime
        .set_current_item(Some(folder.clean_title.clone()));

    if folder.is_locked {
        if let Some(wid) = folder.work_id {
            scrape_work_seasons_by_id(state, wid, is_p0).await?;
            broadcast_verified_work(state, wid, &folder.category);
        }
        return Ok(());
    }

    if folder.scrape_state == "verified" {
        if let Some(wid) = folder.work_id {
            scrape_work_seasons_by_id(state, wid, is_p0).await?;
            broadcast_verified_work(state, wid, &folder.category);
            return Ok(());
        }
    }

    let (ep_count, max_season): (i64, i32) = sqlx::query_as(
        "SELECT COUNT(DISTINCT (season_number, episode_number)), COALESCE(MAX(season_number), 0) FROM sync_files WHERE folder_id = $1",
    )
    .bind(folder.id)
    .fetch_one(&state.db)
    .await?;

    let is_tv = folder.inferred_media_type == "tv" || ep_count > 1 || max_season > 0;

    // 若目录末级仅为 {tmdb-xxx}（如历史同步误取上级合集名或 Season X），按统一规则自愈纠正片名与年份
    if crate::sync::parser::is_pure_tmdb_root_dir(&folder.root_dir) {
        let fnames: Vec<String> = sqlx::query_scalar(
            "SELECT file_name FROM sync_files WHERE folder_id = $1 ORDER BY id ASC LIMIT 5",
        )
        .bind(folder.id)
        .fetch_all(&state.db)
        .await?;

        if let Some((new_title, new_year)) =
            crate::sync::parser::resolve_pure_tmdb_folder_title_year(
                &folder.root_dir,
                is_tv,
                &fnames,
            )
        {
            if new_title != folder.clean_title || (new_year.is_some() && folder.year.is_none()) {
                folder.clean_title = new_title;
                if folder.year.is_none() {
                    folder.year = new_year;
                }
                sqlx::query(
                    "UPDATE sync_folders SET clean_title = $1, year = $2, conflict_reason = NULL WHERE id = $3",
                )
                .bind(&folder.clean_title)
                .bind(folder.year)
                .bind(folder.id)
                .execute(&state.db)
                .await?;
            }
        }
    }

    // 0. 泛词/结构目录名安全拦截：禁止将 Specials / 国语版 / 第一季 / 剧场版 等非片名目录送往 TMDB 关键词搜索，
    // 防止跨作品“黑洞归并”。若无有效 parsed_tmdb_id 则直接标记 conflict。
    let is_generic_title = crate::sync::parser::is_generic_non_title(&folder.clean_title);
    if is_generic_title && folder.parsed_tmdb_id.is_none() {
        let conflict_reason = format!(
            "泛词/结构目录名「{}」无法直接刮削，请检查目录归属或重新同步",
            folder.clean_title
        );
        sqlx::query(
            r#"
            UPDATE sync_folders SET
                scrape_state = 'conflict',
                conflict_reason = $1,
                work_id = NULL,
                updated_at = NOW()
            WHERE id = $2
            "#,
        )
        .bind(&conflict_reason)
        .bind(folder.id)
        .execute(&state.db)
        .await?;
        invalidate_admin_stats(state).await;
        info!(
            "⚠️ [Scraper] 拦截泛词目录 folder_id={}: {}",
            folder.id, conflict_reason
        );
        return Ok(());
    }

    // 1. 若自带 parsed_tmdb_id 且库中已有新鲜且同类型的正式作品，先本地交叉校验后零开销复用
    if let Some(tid) = folder.parsed_tmdb_id {
        let cached_work: Option<WorkSummaryRow> = sqlx::query_as(
            r#"
            SELECT id, media_type, tmdb_id, title, original_title, year, is_locked,
                   metadata_version, metadata_refresh_requested, metadata_retry_count
            FROM media_works
            WHERE tmdb_id = $1 AND media_type = $2
              AND metadata_version >= 2
              AND metadata_fetched_at > NOW() - INTERVAL '1 day'
                LIMIT 1
            "#,
        )
        .bind(tid)
        .bind(&folder.inferred_media_type)
        .fetch_optional(&state.db)
        .await?;

        if let Some(work) = cached_work {
            if verify_tmdb_match(TmdbMatch {
                clean_title: &folder.clean_title,
                local_year: folder.year,
                ep_count,
                max_season,
                strict_title_check: true,
                media_type: &work.media_type,
                title: &work.title,
                original_title: work.original_title.as_deref(),
                year: work.year,
            })
            .is_ok()
            {
                sqlx::query(
                    "UPDATE sync_folders SET work_id = $1, scrape_state = 'verified', conflict_reason = NULL, retry_count = 0, next_retry_at = NULL, updated_at = NOW() WHERE id = $2",
                )
                .bind(work.id)
                .bind(folder.id)
                .execute(&state.db)
                .await?;
                sqlx::query("UPDATE media_works SET updated_at = NOW() WHERE id = $1")
                    .bind(work.id)
                    .execute(&state.db)
                    .await?;

                invalidate_admin_stats(state).await;
                broadcast_verified_work(state, work.id, &folder.category);
                if work.media_type == "tv" {
                    scrape_tv_seasons(state, work.id, tid, work.year.or(folder.year), is_p0)
                        .await?;
                }
                return Ok(());
            }
        }
    }

    // 2. 请求 TMDB 刮削并做多重交叉校验
    let effective_type = if ep_count > 1 || max_season > 0 {
        "tv"
    } else {
        folder.inferred_media_type.as_str()
    };

    let fetch_res = if let Some(tid) = folder.parsed_tmdb_id {
        match state
            .scrape_tmdb_metadata(Some(tid), effective_type, &folder.clean_title, folder.year)
            .await
        {
            Ok(meta) => {
                match verify_tmdb_match(TmdbMatch {
                    clean_title: &folder.clean_title,
                    local_year: folder.year,
                    ep_count,
                    max_season,
                    strict_title_check: true,
                    media_type: &meta.media_type,
                    title: &meta.title,
                    original_title: meta.original_title.as_deref(),
                    year: meta.year,
                }) {
                    Ok(()) => Ok(Ok(meta)),
                    Err(reason) => {
                        if is_generic_title {
                            debug!(
                                "⚠️ [Scraper] folder_id={} parsed_tmdb_id={} 校验未通过({}) 且目录名为泛词「{}」，禁止降级搜索",
                                folder.id, tid, reason, folder.clean_title
                            );
                            Ok(Err(format!(
                                "泛词/结构目录名「{}」与 TMDB({}) 校验冲突，禁止降级搜索：{}",
                                folder.clean_title, tid, reason
                            )))
                        } else {
                            debug!(
                                "⚠️ [Scraper] folder_id={} parsed_tmdb_id={} 校验未通过({}), 降级用片名搜索兜底",
                                folder.id, tid, reason
                            );
                            match state
                                .scrape_tmdb_metadata(
                                    None,
                                    effective_type,
                                    &folder.clean_title,
                                    folder.year,
                                )
                                .await
                            {
                                Ok(fb) => {
                                    match verify_tmdb_match(TmdbMatch {
                                        clean_title: &folder.clean_title,
                                        local_year: folder.year,
                                        ep_count,
                                        max_season,
                                        strict_title_check: false,
                                        media_type: &fb.media_type,
                                        title: &fb.title,
                                        original_title: fb.original_title.as_deref(),
                                        year: fb.year,
                                    }) {
                                        Ok(()) => Ok(Ok(fb)),
                                        Err(fb_reason) => Ok(Err(fb_reason)),
                                    }
                                }
                                Err(_) => Ok(Err(reason)),
                            }
                        }
                    }
                }
            }
            Err(e) => Err(e),
        }
    } else {
        match state
            .scrape_tmdb_metadata(None, effective_type, &folder.clean_title, folder.year)
            .await
        {
            Ok(meta) => {
                match verify_tmdb_match(TmdbMatch {
                    clean_title: &folder.clean_title,
                    local_year: folder.year,
                    ep_count,
                    max_season,
                    strict_title_check: false,
                    media_type: &meta.media_type,
                    title: &meta.title,
                    original_title: meta.original_title.as_deref(),
                    year: meta.year,
                }) {
                    Ok(()) => Ok(Ok(meta)),
                    Err(reason) => Ok(Err(reason)),
                }
            }
            Err(e) => Err(e),
        }
    };

    match fetch_res {
        Ok(Ok(meta)) => {
            let work_id = upsert_media_work(state, &meta, WorkUpsertMode::ContentChange).await?;
            sqlx::query(
                r#"
                UPDATE sync_folders SET
                    work_id = $1,
                    scrape_state = 'verified',
                    conflict_reason = NULL,
                    retry_count = 0,
                    next_retry_at = NULL,
                    updated_at = NOW()
                WHERE id = $2
                "#,
            )
            .bind(work_id)
            .bind(folder.id)
            .execute(&state.db)
            .await?;

            invalidate_admin_stats(state).await;
            broadcast_verified_work(state, work_id, &meta.category);
            info!(
                "✅ [Scraper] 目录刮削并校验通过 folder_id={} -> work_id={} tmdb_id={} type={} title={:?}",
                folder.id, work_id, meta.tmdb_id, meta.media_type, meta.title
            );

            if meta.media_type == "tv" {
                if let Err(e) = scrape_tv_seasons(
                    state,
                    work_id,
                    meta.tmdb_id,
                    meta.year.or(folder.year),
                    is_p0,
                )
                .await
                {
                    debug!("作品 work_id={} 分集刮削部分异常: {:#}", work_id, e);
                }
            }
        }
        Ok(Err(conflict_reason)) => {
            sqlx::query(
                r#"
                UPDATE sync_folders SET
                    scrape_state = 'conflict',
                    conflict_reason = $1,
                    work_id = NULL,
                    updated_at = NOW()
                WHERE id = $2
                "#,
            )
            .bind(&conflict_reason)
            .bind(folder.id)
            .execute(&state.db)
            .await?;
            invalidate_admin_stats(state).await;
            info!(
                "⚠️ [Scraper] 目录校验存疑 folder_id={}: {}",
                folder.id, conflict_reason
            );
        }
        Err(e) => {
            let next_retry = folder.retry_count + 1;
            let backoff_hours = match next_retry {
                1 => 24,
                2 => 72,
                _ => 168,
            };
            let next_at = Utc::now() + ChronoDuration::hours(backoff_hours);
            let fail_reason: String = format!("{:#}", e).chars().take(500).collect();

            sqlx::query(
                "UPDATE sync_folders SET scrape_state = 'failed', conflict_reason = $1, retry_count = $2, next_retry_at = $3, updated_at = NOW() WHERE id = $4",
            )
            .bind(&fail_reason)
            .bind(next_retry)
            .bind(next_at)
            .bind(folder.id)
            .execute(&state.db)
            .await?;
            invalidate_admin_stats(state).await;
            return Err(e);
        }
    }

    Ok(())
}

/// 刷新正式作品（扩展元数据 + 剧集季/分集）
pub async fn scrape_work_seasons_by_id(state: &AppState, work_id: i64, is_p0: bool) -> Result<()> {
    let _activity = state.scraper.runtime.begin_task();
    let row: Option<WorkSummaryRow> = sqlx::query_as(
        "SELECT id, media_type, tmdb_id, title, original_title, year, is_locked, metadata_version, metadata_refresh_requested, metadata_retry_count FROM media_works WHERE id = $1",
    )
    .bind(work_id)
    .fetch_optional(&state.db)
    .await?;

    let Some(work) = row else {
        return Ok(());
    };
    state
        .scraper
        .runtime
        .set_current_item(Some(work.title.clone()));

    let Some(tid) = work.tmdb_id else {
        return Ok(());
    };

    let stale: bool = sqlx::query_scalar(
        "SELECT metadata_fetched_at IS NULL OR metadata_fetched_at < NOW() - CASE WHEN tmdb_metadata->>'status'='Returning Series' THEN INTERVAL '1 day' ELSE INTERVAL '7 days' END FROM media_works WHERE id = $1",
    )
    .bind(work.id)
    .fetch_one(&state.db)
    .await?;

    if !work.is_locked && (work.metadata_version < 2 || work.metadata_refresh_requested || stale) {
        match state
            .scrape_tmdb_metadata(Some(tid), &work.media_type, &work.title, work.year)
            .await
        {
            Ok(meta) if meta.media_type == work.media_type => {
                let _ =
                    upsert_media_work(state, &meta, WorkUpsertMode::MetadataRefreshOnly).await?;
                invalidate_admin_stats(state).await;
            }
            Ok(meta) => {
                let err = anyhow::anyhow!(
                    "刷新作品元数据返回类型不匹配 work_id={} expected={} actual={} tmdb_id={}",
                    work.id,
                    work.media_type,
                    meta.media_type,
                    meta.tmdb_id
                );
                let retries = work.metadata_retry_count.saturating_add(1);
                if let Err(update_err) = sqlx::query(
                    "UPDATE media_works SET metadata_retry_count = $2, metadata_retry_at = NOW() + INTERVAL '1 day' WHERE id = $1",
                )
                .bind(work.id)
                .bind(retries)
                .execute(&state.db)
                .await
                {
                    error!("更新作品元数据重试状态失败 work_id={}: {:#}", work.id, update_err);
                }
                return Err(err);
            }
            Err(e) => {
                let retries = work.metadata_retry_count.saturating_add(1);
                if let Err(update_err) = sqlx::query(
                    "UPDATE media_works SET metadata_retry_count = $2, metadata_retry_at = NOW() + INTERVAL '1 day' WHERE id = $1",
                )
                .bind(work.id)
                .bind(retries)
                .execute(&state.db)
                .await
                {
                    error!("更新作品元数据重试状态失败 work_id={}: {:#}", work.id, update_err);
                }
                return Err(e);
            }
        }
    }

    if work.media_type == "tv" {
        scrape_tv_seasons(state, work.id, tid, work.year, is_p0).await?;
    }
    Ok(())
}

/// 刮削正式剧集作品的季与单集元数据（全库单份存储在 work_seasons / work_episodes）
pub async fn scrape_tv_seasons(
    state: &AppState,
    work_id: i64,
    tmdb_id: i32,
    series_year: Option<i32>,
    is_p0: bool,
) -> Result<()> {
    let _activity = state.scraper.runtime.begin_task();
    // 1. 确保所有关联物理源包含的季都在 work_seasons 中存在
    sqlx::query(
        r#"
        INSERT INTO work_seasons (work_id, season_number, name, scrape_attempts)
        SELECT DISTINCT
            $1,
            sf.season_number,
            CASE WHEN sf.season_number = 0 THEN '特别篇' ELSE '第 ' || sf.season_number || ' 季' END,
            0
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        WHERE f.work_id = $1 AND f.is_ignored = FALSE
        ON CONFLICT (work_id, season_number) DO NOTHING
        "#,
    )
    .bind(work_id)
    .execute(&state.db)
    .await?;

    // 确保新同步进来的单集占位在 work_episodes 中存在（便于追踪 meta_incomplete）
    sqlx::query(
        r#"
        INSERT INTO work_episodes (work_id, season_number, episode_number, meta_incomplete)
        SELECT DISTINCT $1, sf.season_number, sf.episode_number, TRUE
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        WHERE f.work_id = $1 AND f.is_ignored = FALSE
        ON CONFLICT (work_id, season_number, episode_number) DO NOTHING
        "#,
    )
    .bind(work_id)
    .execute(&state.db)
    .await?;

    let cooldown_hours = if is_p0 { 12 } else { 24 };
    let cutoff = Utc::now() - ChronoDuration::hours(cooldown_hours);

    let pending_seasons: Vec<(i32, i32)> = sqlx::query_as(
        r#"
        SELECT s.season_number, s.scrape_attempts
        FROM work_seasons s
        WHERE s.work_id = $1
          AND s.scrape_attempts >= 0
          AND (
              s.scrape_attempts = 0
              OR (
                  s.scrape_attempts BETWEEN 1 AND 4
                  AND EXISTS (
                      SELECT 1 FROM work_episodes e
                      WHERE e.work_id = $1
                        AND e.season_number = s.season_number
                        AND e.meta_incomplete = TRUE
                        AND e.updated_at <= $2
                  )
              )
              OR EXISTS (
                  SELECT 1 FROM work_episodes e
                  WHERE e.work_id = $1
                    AND e.season_number = s.season_number
                    AND e.still_path IS NULL
                    AND e.overview IS NULL
                    AND e.meta_incomplete = TRUE
              )
          )
        ORDER BY s.season_number ASC
        "#,
    )
    .bind(work_id)
    .bind(cutoff)
    .fetch_all(&state.db)
    .await?;

    if pending_seasons.is_empty() {
        return Ok(());
    }

    let current_year = Utc::now().year();
    let mut season_image_changed = false;
    let mut updated_season_posters = HashSet::new();
    let mut updated_still_episodes = HashSet::new();

    for (season_num, attempts) in pending_seasons {
        let next_attempt = (attempts.max(0) + 1).min(5);
        match state.fetch_tmdb_season(tmdb_id, season_num).await {
            Ok(Some(season_meta)) => {
                if let Some(new_poster) = season_meta.poster_path.as_ref() {
                    let old_poster: Option<Option<String>> = sqlx::query_scalar(
                        "SELECT poster_path FROM work_seasons WHERE work_id = $1 AND season_number = $2",
                    )
                    .bind(work_id)
                    .bind(season_num)
                    .fetch_optional(&state.db)
                    .await?;
                    if old_poster.as_ref().and_then(Option::as_ref) != Some(new_poster) {
                        season_image_changed = true;
                        updated_season_posters.insert(season_num);
                    }
                }
                sqlx::query(
                    r#"
                    UPDATE work_seasons SET
                        name = COALESCE($1, name),
                        overview = COALESCE($2, overview),
                        poster_path = COALESCE($3, poster_path),
                        air_date = COALESCE($4, air_date),
                        scrape_attempts = $5,
                        episode_count = $8
                    WHERE work_id = $6 AND season_number = $7
                    "#,
                )
                .bind(&season_meta.name)
                .bind(&season_meta.overview)
                .bind(&season_meta.poster_path)
                .bind(&season_meta.air_date)
                .bind(next_attempt)
                .bind(work_id)
                .bind(season_num)
                .bind(season_meta.episodes.len() as i32)
                .execute(&state.db)
                .await?;

                let has_any_air_date = season_meta.episodes.iter().any(|e| e.air_date.is_some());
                let season_is_ongoing = season_meta
                    .episodes
                    .iter()
                    .any(|e| is_air_date_recent(e.air_date.as_deref(), 35))
                    || (!has_any_air_date
                        && series_year.map(|y| y >= current_year - 1).unwrap_or(false));

                let old_stills: HashMap<i32, Option<String>> = sqlx::query_as(
                    "SELECT episode_number, still_path FROM work_episodes WHERE work_id = $1 AND season_number = $2",
                )
                .bind(work_id)
                .bind(season_num)
                .fetch_all(&state.db)
                .await?
                .into_iter()
                .collect();
                let mut ep_nums = Vec::with_capacity(season_meta.episodes.len());
                let mut names = Vec::with_capacity(season_meta.episodes.len());
                let mut overviews = Vec::with_capacity(season_meta.episodes.len());
                let mut still_paths = Vec::with_capacity(season_meta.episodes.len());
                let mut air_dates = Vec::with_capacity(season_meta.episodes.len());
                let mut runtimes = Vec::with_capacity(season_meta.episodes.len());
                let mut incompletes = Vec::with_capacity(season_meta.episodes.len());

                for ep in season_meta.episodes {
                    if ep.still_path.is_some()
                        && old_stills.get(&ep.episode_number) != Some(&ep.still_path)
                    {
                        updated_still_episodes.insert((season_num, ep.episode_number));
                    }
                    let has_still = ep.still_path.is_some();
                    let has_overview = ep.overview.is_some();
                    let has_real_name = ep
                        .name
                        .as_deref()
                        .map(|n| !is_placeholder_episode_title(n, ep.episode_number))
                        .unwrap_or(false);

                    let is_complete = has_still && has_overview && has_real_name;
                    let ep_is_recent = is_air_date_recent(ep.air_date.as_deref(), 30)
                        || (ep.air_date.is_none() && season_is_ongoing);
                    let keep_incomplete = !is_complete && ep_is_recent && next_attempt < 5;

                    ep_nums.push(ep.episode_number);
                    names.push(ep.name);
                    overviews.push(ep.overview);
                    still_paths.push(ep.still_path);
                    air_dates.push(ep.air_date);
                    runtimes.push(ep.runtime.unwrap_or(0));
                    incompletes.push(keep_incomplete);
                }

                if !ep_nums.is_empty() {
                    sqlx::query(
                        r#"
                        INSERT INTO work_episodes (
                            work_id, season_number, episode_number, title, overview,
                            still_path, air_date, runtime, meta_incomplete, updated_at
                        )
                        SELECT
                            $8, $9, src.episode_number, src.name, src.overview,
                            src.still_path, src.air_date, src.runtime, src.incomplete, NOW()
                        FROM UNNEST(
                            $1::int4[], $2::varchar[], $3::text[], $4::varchar[], $5::varchar[], $6::int4[], $7::bool[]
                        ) AS src(episode_number, name, overview, still_path, air_date, runtime, incomplete)
                        ON CONFLICT (work_id, season_number, episode_number) DO UPDATE SET
                            title = COALESCE(EXCLUDED.title, work_episodes.title),
                            overview = COALESCE(EXCLUDED.overview, work_episodes.overview),
                            still_path = COALESCE(EXCLUDED.still_path, work_episodes.still_path),
                            air_date = COALESCE(EXCLUDED.air_date, work_episodes.air_date),
                            runtime = CASE WHEN EXCLUDED.runtime > 0 THEN EXCLUDED.runtime ELSE work_episodes.runtime END,
                            meta_incomplete = EXCLUDED.meta_incomplete,
                            updated_at = NOW()
                        "#,
                    )
                    .bind(&ep_nums)
                    .bind(&names)
                    .bind(&overviews)
                    .bind(&still_paths)
                    .bind(&air_dates)
                    .bind(&runtimes)
                    .bind(&incompletes)
                    .bind(work_id)
                    .bind(season_num)
                    .execute(&state.db)
                    .await?;
                }

                let unindexed_incomplete = season_is_ongoing && next_attempt < 5;
                sqlx::query(
                    r#"
                    UPDATE work_episodes SET
                        meta_incomplete = $1,
                        updated_at = NOW()
                    WHERE work_id = $2
                      AND season_number = $3
                      AND NOT (episode_number = ANY($4))
                    "#,
                )
                .bind(unindexed_incomplete)
                .bind(work_id)
                .bind(season_num)
                .bind(&ep_nums)
                .execute(&state.db)
                .await?;

                info!(
                    "📺 [SeasonScraper] 剧集 work_id={} (tmdb_id={}) S{:02} 分集刮削完成 (TMDB集数={}, 尝试={}/5)",
                    work_id,
                    tmdb_id,
                    season_num,
                    ep_nums.len(),
                    next_attempt
                );
            }
            Ok(None) => {
                sqlx::query(
                    "UPDATE work_seasons SET scrape_attempts = -1 WHERE work_id = $1 AND season_number = $2",
                )
                .bind(work_id)
                .bind(season_num)
                .execute(&state.db)
                .await?;

                sqlx::query(
                    "UPDATE work_episodes SET meta_incomplete = FALSE, updated_at = NOW() WHERE work_id = $1 AND season_number = $2",
                )
                .bind(work_id)
                .bind(season_num)
                .execute(&state.db)
                .await?;
            }
            Err(e) => {
                debug!(
                    "拉取 TMDB 剧集季失败 (tmdb_id={}, S{:02}): {:#}",
                    tmdb_id, season_num, e
                );
                sqlx::query(
                    "UPDATE work_seasons SET scrape_attempts = LEAST(GREATEST(scrape_attempts, 0) + 1, 5) WHERE work_id = $1 AND season_number = $2",
                )
                .bind(work_id)
                .bind(season_num)
                .execute(&state.db)
                .await?;

                sqlx::query(
                    "UPDATE work_episodes SET meta_incomplete = TRUE, updated_at = NOW() WHERE work_id = $1 AND season_number = $2 AND still_path IS NULL AND overview IS NULL",
                )
                .bind(work_id)
                .bind(season_num)
                .execute(&state.db)
                .await?;
            }
        }
    }

    for season_num in updated_season_posters {
        state
            .caches
            .image_meta_cache
            .invalidate(&format!("season_{work_id}_{season_num}:Primary"))
            .await;
    }
    for (season_num, episode_num) in &updated_still_episodes {
        for key in [
            format!("ep_{work_id}_{season_num}_{episode_num}:Primary"),
            format!("ep_{work_id}_{season_num}_{episode_num}:Backdrop"),
        ] {
            state.caches.image_meta_cache.invalidate(&key).await;
        }
    }
    if season_image_changed || !updated_still_episodes.is_empty() {
        invalidate_work_image_meta(&state.caches.image_meta_cache, work_id).await;
    }
    Ok(())
}

/// 人工绑定 TMDB ID 或无 TMDB 强行发布到正式作品表
pub async fn promote_folder_manually(
    state: &AppState,
    folder_id: i64,
    tmdb_id: Option<i32>,
    media_type: Option<&str>,
) -> Result<i64> {
    if let Some(tid) = tmdb_id {
        let folder_row: (String, String, Option<i32>) = sqlx::query_as(
            "SELECT inferred_media_type, clean_title, year FROM sync_folders WHERE id = $1",
        )
        .bind(folder_id)
        .fetch_one(&state.db)
        .await?;

        let mtype = media_type.unwrap_or(&folder_row.0);
        let meta = state
            .scrape_tmdb_metadata(Some(tid), mtype, &folder_row.1, folder_row.2)
            .await?;
        let work_id = upsert_media_work(state, &meta, WorkUpsertMode::ContentChange).await?;

        sqlx::query(
            r#"
            UPDATE sync_folders SET
                work_id = $1,
                parsed_tmdb_id = $2,
                scrape_state = 'verified',
                conflict_reason = NULL,
                is_locked = TRUE,
                retry_count = 0,
                next_retry_at = NULL,
                updated_at = NOW()
            WHERE id = $3
            "#,
        )
        .bind(work_id)
        .bind(tid)
        .bind(folder_id)
        .execute(&state.db)
        .await?;

        invalidate_admin_stats(state).await;
        if meta.media_type == "tv" {
            let _ = scrape_tv_seasons(state, work_id, tid, meta.year.or(folder_row.2), true).await;
        }
        Ok(work_id)
    } else {
        // 无 TMDB 强行发布：创建自定义 media_works 记录
        let folder_row: (String, String, Option<i32>, String) = sqlx::query_as(
            "SELECT inferred_media_type, clean_title, year, category FROM sync_folders WHERE id = $1",
        )
        .bind(folder_id)
        .fetch_one(&state.db)
        .await?;

        let work_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO media_works (media_type, tmdb_id, title, year, category, is_locked, metadata_version)
            VALUES ($1, NULL, $2, $3, $4, TRUE, 2)
            RETURNING id
            "#,
        )
        .bind(&folder_row.0)
        .bind(&folder_row.1)
        .bind(folder_row.2)
        .bind(&folder_row.3)
        .fetch_one(&state.db)
        .await?;

        sqlx::query(
            "UPDATE sync_folders SET work_id = $1, scrape_state = 'verified', conflict_reason = NULL, is_locked = TRUE, updated_at = NOW() WHERE id = $2",
        )
        .bind(work_id)
        .bind(folder_id)
        .execute(&state.db)
        .await?;

        invalidate_admin_stats(state).await;
        Ok(work_id)
    }
}

fn is_air_date_recent(air_date: Option<&str>, within_days: i64) -> bool {
    let Some(raw) = air_date else {
        return false;
    };
    let Ok(date) = NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d") else {
        return false;
    };
    let today = Utc::now().date_naive();
    (today - date).num_days() <= within_days
}

fn is_placeholder_episode_title(name: &str, ep_num: i32) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return true;
    }
    trimmed == format!("第{}集", ep_num)
        || trimmed == format!("第 {} 集", ep_num)
        || trimmed.eq_ignore_ascii_case(&format!("episode {}", ep_num))
}

#[cfg(test)]
mod scheduled_task_tests {
    use super::*;

    #[test]
    fn activity_guard_reports_running_until_last_concurrent_task_finishes() {
        let runtime = ScrapeRuntime::default();
        assert!(!runtime.snapshot().running);
        let first = runtime.begin_task();
        let second = runtime.begin_task();
        assert!(runtime.snapshot().running);
        drop(first);
        assert!(runtime.snapshot().running);
        drop(second);
        assert!(!runtime.snapshot().running);
    }

    #[test]
    fn work_upsert_mode_separates_content_change_from_metadata_refresh() {
        assert!(WorkUpsertMode::ContentChange.touches_updated_at());
        assert!(!WorkUpsertMode::MetadataRefreshOnly.touches_updated_at());
    }

    #[test]
    fn history_creates_separate_records_across_completed_batches_and_auto_scrapes() {
        let runtime = ScrapeRuntime::default();

        // 1. 批次 #1：提交 5 项但仅执行 3 项后队列变空 -> 仍应结算为 completed 并清零计数
        runtime.add_batch_items("批量刮削指定目录", 5);
        runtime.record_item_result(true);
        runtime.record_item_result(true);
        runtime.record_item_result(false);
        runtime.clear_idle_counters_if_done();

        // 2. 批次 #2：再次调用 add_batch_items -> 应新建 #2 而非覆盖 #1
        runtime.add_batch_items("补齐正式作品 TMDB 元数据", 2);
        runtime.record_item_result(true);
        runtime.record_item_result(true);
        runtime.clear_idle_counters_if_done();

        // 3. 批次 #3：后台自动/P0 刮削（不走 start_batch）-> 应新建 #3 而非丢弃或覆盖 #2
        runtime.record_item_result(true);
        runtime.clear_idle_counters_if_done();

        let hist = runtime.history.lock().unwrap();
        assert_eq!(hist.len(), 3);
        assert_eq!(hist[0].id, 3);
        assert_eq!(hist[0].task_name, "后台全库自动刮削");
        assert_eq!(hist[0].done, 1);
        assert_eq!(hist[0].status, "completed");

        assert_eq!(hist[1].id, 2);
        assert_eq!(hist[1].task_name, "补齐正式作品 TMDB 元数据");
        assert_eq!(hist[1].done, 2);
        assert_eq!(hist[1].succeeded, 2);
        assert_eq!(hist[1].status, "completed");

        assert_eq!(hist[2].id, 1);
        assert_eq!(hist[2].task_name, "批量刮削指定目录");
        assert_eq!(hist[2].done, 3);
        assert_eq!(hist[2].succeeded, 2);
        assert_eq!(hist[2].failed, 1);
        assert_eq!(hist[2].status, "completed");
    }

    #[tokio::test]
    async fn manual_request_wakes_worker_and_enables_sweep() {
        let runtime = ScrapeRuntime::default();
        runtime.request_sweep();
        assert!(runtime.manual_sweep.load(Ordering::Relaxed));
        tokio::time::timeout(Duration::from_millis(50), runtime.wake.notified())
            .await
            .expect("manual request should retain a wake permit");
    }

    #[tokio::test]
    #[ignore = "需要 TEST_DATABASE_URL；使用单连接临时表隔离队列数据"]
    async fn scheduled_scrape_enqueues_missing_metadata_and_manually_drains_pending_folders() {
        let url = std::env::var("TEST_DATABASE_URL").expect("设置测试数据库连接");
        let db = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql(
            r#"
            CREATE TEMP TABLE media_works (
                id BIGINT PRIMARY KEY, tmdb_id INTEGER, is_locked BOOLEAN DEFAULT FALSE,
                metadata_version SMALLINT DEFAULT 0, metadata_fetched_at TIMESTAMPTZ,
                metadata_refresh_requested BOOLEAN DEFAULT FALSE,
                metadata_retry_count SMALLINT DEFAULT 0, metadata_retry_at TIMESTAMPTZ
            );
            CREATE TEMP TABLE sync_folders (
                id BIGINT PRIMARY KEY, scrape_state TEXT, is_ignored BOOLEAN DEFAULT FALSE,
                is_locked BOOLEAN DEFAULT FALSE, scrape_priority INTEGER DEFAULT 0
            );
            INSERT INTO media_works (id, tmdb_id, is_locked, metadata_version, metadata_fetched_at)
            VALUES (1, 101, TRUE, 0, NULL), (2, 102, FALSE, 0, NULL),
                   (3, 103, FALSE, 2, NOW()), (4, NULL, FALSE, 0, NULL);
            INSERT INTO sync_folders (id, scrape_state) VALUES (5, 'pending');
        "#,
        )
        .execute(&db)
        .await
        .unwrap();
        let mut cfg = crate::config::BootstrapFileConfig::default().into_dynamic();
        cfg.tmdb_api_key = "fixture".into();
        cfg.scraper_auto_background = false;
        let (tx, _rx) = mpsc::channel(4);
        let state = AppState::new(cfg, db.clone(), tx).unwrap();
        assert!(fetch_next_pending_task(&state).await.unwrap().is_none());
        request_scheduled_scrape(&state).await.unwrap();
        let queued: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM media_works WHERE metadata_refresh_requested ORDER BY id",
        )
        .fetch_all(&db)
        .await
        .unwrap();
        assert_eq!(queued, vec![2]);
        assert!(matches!(
            fetch_next_pending_task(&state).await.unwrap(),
            Some(ScrapeTask::WorkSeasons(2))
        ));
        sqlx::query("UPDATE media_works SET metadata_refresh_requested = FALSE WHERE id = 2")
            .execute(&db)
            .await
            .unwrap();
        assert!(matches!(
            fetch_next_pending_task(&state).await.unwrap(),
            Some(ScrapeTask::Folder(5))
        ));
        db.close().await;
    }

    #[tokio::test]
    #[ignore = "需要 TEST_DATABASE_URL；验证被动刷新不覆写 updated_at、过滤孤儿目录，且新增分集正确推进 updated_at"]
    async fn passive_metadata_refresh_preserves_updated_at_and_skips_inactive_works() {
        let url = std::env::var("TEST_DATABASE_URL").expect("设置测试数据库连接");
        let db = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql(
            r#"
            CREATE TEMP TABLE media_works (
                id BIGSERIAL PRIMARY KEY,
                media_type VARCHAR(16) NOT NULL,
                tmdb_id INTEGER,
                title VARCHAR(255) NOT NULL,
                original_title VARCHAR(255),
                sort_title VARCHAR(255),
                year INTEGER,
                premiered_at VARCHAR(32),
                overview TEXT,
                rating REAL NOT NULL DEFAULT 0,
                poster_path VARCHAR(255),
                backdrop_path VARCHAR(255),
                countries TEXT[] NOT NULL DEFAULT '{}',
                networks TEXT[] NOT NULL DEFAULT '{}',
                genres TEXT[] NOT NULL DEFAULT '{}',
                tags TEXT[] NOT NULL DEFAULT '{}',
                category VARCHAR(32) NOT NULL DEFAULT '电影',
                tmdb_metadata JSONB NOT NULL DEFAULT '{}',
                metadata_version SMALLINT NOT NULL DEFAULT 0,
                metadata_fetched_at TIMESTAMPTZ,
                metadata_refresh_requested BOOLEAN NOT NULL DEFAULT FALSE,
                metadata_retry_count SMALLINT NOT NULL DEFAULT 0,
                metadata_retry_at TIMESTAMPTZ,
                is_locked BOOLEAN NOT NULL DEFAULT FALSE,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            );
            CREATE UNIQUE INDEX idx_temp_media_works_tmdb
                ON media_works(tmdb_id, media_type) WHERE tmdb_id IS NOT NULL;
            CREATE TEMP TABLE sync_folders (
                id BIGSERIAL PRIMARY KEY,
                root_dir TEXT UNIQUE NOT NULL,
                raw_folder_name VARCHAR(255) NOT NULL,
                clean_title VARCHAR(255) NOT NULL,
                year INTEGER,
                inferred_media_type VARCHAR(16) NOT NULL,
                category VARCHAR(32) NOT NULL,
                parsed_tmdb_id INTEGER,
                work_id BIGINT,
                scrape_state VARCHAR(16) NOT NULL DEFAULT 'verified',
                conflict_reason TEXT,
                scrape_priority SMALLINT NOT NULL DEFAULT 3,
                retry_count SMALLINT NOT NULL DEFAULT 0,
                next_retry_at TIMESTAMPTZ,
                is_locked BOOLEAN NOT NULL DEFAULT FALSE,
                is_ignored BOOLEAN NOT NULL DEFAULT FALSE,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            );
            CREATE TEMP TABLE sync_files (
                id BIGSERIAL PRIMARY KEY,
                folder_id BIGINT NOT NULL,
                season_number INTEGER NOT NULL DEFAULT 0,
                episode_number INTEGER NOT NULL DEFAULT 1,
                alist_path TEXT NOT NULL,
                path_hash BIGINT UNIQUE NOT NULL,
                file_name VARCHAR(512) NOT NULL,
                container VARCHAR(32) NOT NULL,
                version_tag VARCHAR(128) NOT NULL DEFAULT '默认版本',
                priority_score INTEGER NOT NULL DEFAULT 0,
                subtitles TEXT[] NOT NULL DEFAULT '{}'
            );
            CREATE TEMP TABLE work_seasons (
                work_id BIGINT NOT NULL,
                season_number INTEGER NOT NULL,
                scrape_attempts INTEGER NOT NULL DEFAULT 0
            );
            CREATE TEMP TABLE work_episodes (
                work_id BIGINT NOT NULL,
                season_number INTEGER NOT NULL,
                meta_incomplete BOOLEAN NOT NULL DEFAULT FALSE,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            );
            INSERT INTO media_works (id, media_type, tmdb_id, title, category, metadata_version, metadata_fetched_at, updated_at)
            VALUES
                (1, 'movie', 1001, '孤儿作品', '电影', 2, NOW() - INTERVAL '8 days', NOW() - INTERVAL '8 days'),
                (2, 'movie', 1002, '已屏蔽作品', '电影', 2, NOW() - INTERVAL '8 days', NOW() - INTERVAL '8 days'),
                (3, 'movie', 1003, '在库老作品', '电影', 2, NOW() - INTERVAL '8 days', NOW() - INTERVAL '8 days');
            INSERT INTO sync_folders (id, root_dir, raw_folder_name, clean_title, inferred_media_type, category, work_id, scrape_state, is_ignored)
            VALUES
                (20, '/ignored/2', '2', '已屏蔽作品', 'movie', '电影', 2, 'verified', TRUE),
                (30, '/active/3', '3', '在库老作品', 'movie', '电影', 3, 'verified', FALSE);
            INSERT INTO sync_files (folder_id, season_number, episode_number, alist_path, path_hash, file_name, container)
            VALUES (30, 0, 1, '/active/3/old.mkv', 3001, 'old.mkv', 'mkv');
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        let mut cfg = crate::config::BootstrapFileConfig::default().into_dynamic();
        cfg.tmdb_api_key = "fixture".into();
        cfg.scraper_auto_background = true;
        let (tx, _rx) = mpsc::channel(4);
        let state = AppState::new(cfg, db.clone(), tx).unwrap();

        // 第 4 优先级应跳过孤儿作品 #1 与已屏蔽作品 #2，仅捞出在库过期作品 #3
        assert!(matches!(
            fetch_next_pending_task(&state).await.unwrap(),
            Some(ScrapeTask::WorkSeasons(3))
        ));

        let (before_updated, before_fetched): (chrono::DateTime<Utc>, Option<chrono::DateTime<Utc>>) =
            sqlx::query_as("SELECT updated_at, metadata_fetched_at FROM media_works WHERE id = 3")
                .fetch_one(&db)
                .await
                .unwrap();

        let meta = ScrapedMetadata {
            media_type: "movie".into(),
            tmdb_id: 1003,
            title: "在库老作品".into(),
            original_title: None,
            year: Some(2020),
            premiered_at: Some("2020-01-01".into()),
            overview: Some("简介".into()),
            rating: 8.1,
            poster_path: None,
            backdrop_path: None,
            countries: vec!["CN".into()],
            networks: vec![],
            genres: vec!["剧情".into()],
            extra: tmdb::TmdbExtraMetadata::default(),
            category: "电影".into(),
        };

        // 被动元数据刷新：应推进 metadata_fetched_at，但绝不修改 updated_at
        upsert_media_work(&state, &meta, WorkUpsertMode::MetadataRefreshOnly)
            .await
            .unwrap();
        let (after_refresh_updated, after_refresh_fetched): (
            chrono::DateTime<Utc>,
            Option<chrono::DateTime<Utc>>,
        ) = sqlx::query_as("SELECT updated_at, metadata_fetched_at FROM media_works WHERE id = 3")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(after_refresh_updated, before_updated);
        assert!(after_refresh_fetched > before_fetched);

        // 内容变更入库/新版本归并：应推进 updated_at
        upsert_media_work(&state, &meta, WorkUpsertMode::ContentChange)
            .await
            .unwrap();
        let after_content_updated: chrono::DateTime<Utc> =
            sqlx::query_scalar("SELECT updated_at FROM media_works WHERE id = 3")
                .fetch_one(&db)
                .await
                .unwrap();
        assert!(after_content_updated > before_updated);

        // 重置 updated_at 后验证 sync 新增物理文件会推进已绑定作品的 updated_at
        sqlx::query("UPDATE media_works SET updated_at = NOW() - INTERVAL '8 days' WHERE id = 3")
            .execute(&db)
            .await
            .unwrap();
        let before_sync_updated: chrono::DateTime<Utc> =
            sqlx::query_scalar("SELECT updated_at FROM media_works WHERE id = 3")
                .fetch_one(&db)
                .await
                .unwrap();
        let existing_rec = crate::sync::parser::ParsedStrmRecord {
            alist_path: "/active/3/old.mkv".into(),
            path_hash: 3001,
            root_dir: "/active/3".into(),
            raw_folder_name: "3".into(),
            clean_title: "在库老作品".into(),
            year: Some(2020),
            tmdb_id: Some(1003),
            inferred_media_type: "movie".into(),
            inferred_category: "电影".into(),
            season_number: 0,
            episode_number: 1,
            file_name: "old.mkv".into(),
            container: "mkv".into(),
            version_tag: "1080P".into(),
            priority_score: 10,
            has_noise_prefix: false,
            subtitles: vec![],
        };
        let new_version_rec = crate::sync::parser::ParsedStrmRecord {
            alist_path: "/active/3/4k.mkv".into(),
            path_hash: 3002,
            file_name: "4k.mkv".into(),
            version_tag: "4K".into(),
            priority_score: 50,
            ..existing_rec.clone()
        };
        let stats = crate::sync::batch_db::apply_sync_diff(
            &db,
            vec![existing_rec, new_version_rec],
            2,
            500,
            false,
        )
        .await
        .unwrap();
        assert_eq!(stats.updated_folder_ids, vec![30]);
        let after_sync_updated: chrono::DateTime<Utc> =
            sqlx::query_scalar("SELECT updated_at FROM media_works WHERE id = 3")
                .fetch_one(&db)
                .await
                .unwrap();
        assert!(after_sync_updated > before_sync_updated);

        db.close().await;
    }
}
