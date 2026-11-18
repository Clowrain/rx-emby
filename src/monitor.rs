use crate::state::AppState;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{interval, Duration};
use tracing::{debug, warn};

const MAX_RECENT_SAMPLES: usize = 60;
const SAMPLE_INTERVAL_SECS: u64 = 30;

#[derive(Clone, Debug, Serialize)]
pub struct ResourceSample {
    pub sampled_at: DateTime<Utc>,
    pub scenario: String,
    pub app_pid: u32,
    pub app_cpu_pct: f64,
    pub app_mem_mb: f64,
    pub db_host_available: bool,
    pub db_cpu_pct: f64,
    pub db_mem_mb: f64,
    pub db_size_mb: f64,
    pub db_active_conns: i64,
    pub db_total_conns: i64,
    pub db_max_conns: i64,
    pub db_cache_hit_pct: f64,
    pub disk_total_gb: f64,
    pub disk_used_gb: f64,
    pub disk_avail_gb: f64,
    pub disk_usage_pct: f64,
    pub image_cache_mb: f64,
}

#[derive(sqlx::FromRow, Clone, Debug, Serialize)]
pub struct ResourceDailyRow {
    pub stat_date: NaiveDate,
    pub sample_count: i32,
    pub app_cpu_avg: f64,
    pub app_cpu_max: f64,
    pub app_mem_avg_mb: f64,
    pub app_mem_max_mb: f64,
    pub db_cpu_avg: f64,
    pub db_cpu_max: f64,
    pub db_mem_avg_mb: f64,
    pub db_mem_max_mb: f64,
    pub db_size_max_mb: f64,
    pub db_conn_avg: f64,
    pub db_conn_max: i32,
    pub disk_used_max_gb: f64,
    pub disk_usage_max_pct: f64,
    pub peak_cpu_at: Option<DateTime<Utc>>,
    pub peak_cpu_scenario: Option<String>,
    pub peak_mem_at: Option<DateTime<Utc>>,
    pub peak_mem_scenario: Option<String>,
    pub updated_at: DateTime<Utc>,
}

pub struct MonitorState {
    pub started_at: DateTime<Utc>,
    pub recent_samples: RwLock<VecDeque<ResourceSample>>,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub prev_cpu_sample: Mutex<Option<(u64, Instant)>>,
}

impl Default for MonitorState {
    fn default() -> Self {
        Self {
            started_at: Utc::now(),
            recent_samples: RwLock::new(VecDeque::with_capacity(MAX_RECENT_SAMPLES)),
            prev_cpu_sample: Mutex::new(None),
        }
    }
}

pub fn spawn_resource_monitor(state: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // 启动后先采样一次基线数据；停机时丢弃未完成的采样与其子进程。
        tokio::select! {
            _ = state.shutdown_token.cancelled() => return,
            result = collect_and_record(&state) => {
                if let Err(error) = result {
                    warn!("⚠️ [Monitor] 初始资源采样失败: {}", error);
                }
            }
        }

        let mut ticker = interval(Duration::from_secs(SAMPLE_INTERVAL_SECS));
        ticker.tick().await; // 跳过立即触发的首次 tick

        loop {
            tokio::select! {
                _ = state.shutdown_token.cancelled() => {
                    break;
                }
                _ = ticker.tick() => {
                    tokio::select! {
                        _ = state.shutdown_token.cancelled() => break,
                        result = collect_and_record(&state) => {
                            if let Err(error) = result {
                                debug!("资源监控定时采样异常: {}", error);
                            }
                        }
                    }
                }
            }
        }
    })
}

pub fn detect_current_scenario(state: &AppState) -> String {
    if state.sync_ctrl.is_syncing.load(Ordering::Relaxed) {
        return "媒体库同步中".to_string();
    }
    let sessions = state
        .sessions
        .iter()
        .filter(|entry| entry.is_active())
        .count();
    if sessions > 0 {
        return format!("并发播放 ({}路)", sessions);
    }
    "日常待机".to_string()
}

pub async fn collect_and_record(state: &AppState) -> anyhow::Result<ResourceSample> {
    let cfg = state.config.load();
    let scenario = detect_current_scenario(state);
    let pid = std::process::id();

    // 1. 当前服务进程 CPU 与 RSS 内存 (Linux 优先直读 /proc，不依赖 procps；macOS 回退 ps)
    let (app_cpu_pct, app_mem_mb) = sample_current_process(state, pid).await;

    // 2. 同机 PostgreSQL 进程采样（若为独立容器或同机进程，尽力汇总 postgres 进程）
    let (db_host_available, db_cpu_pct, db_mem_mb) = sample_postgres_processes().await;

    // 3. PostgreSQL 内部统计指标
    let db_size_bytes: i64 =
        sqlx::query_scalar("SELECT pg_database_size(current_database())::bigint")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    let db_size_mb = db_size_bytes as f64 / (1024.0 * 1024.0);

    let (db_active_conns, db_total_conns): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE state = 'active'), COUNT(*) FROM pg_stat_activity WHERE datname = current_database()",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or((0, 0));

    let db_max_conns: i64 = sqlx::query_scalar::<_, String>("SHOW max_connections")
        .fetch_one(&state.db)
        .await
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(100);

    let (blks_hit, blks_read): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(blks_hit, 0)::bigint, COALESCE(blks_read, 0)::bigint FROM pg_stat_database WHERE datname = current_database()",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or((0, 0));
    let total_blks = blks_hit + blks_read;
    let db_cache_hit_pct = if total_blks > 0 {
        (blks_hit as f64 / total_blks as f64) * 100.0
    } else {
        100.0
    };

    // 4. 磁盘占用与图片缓存占用
    let (disk_total_gb, disk_used_gb, disk_avail_gb, disk_usage_pct) =
        sample_disk_df(&cfg.image_cache_dir)
            .await
            .unwrap_or((0.0, 0.0, 0.0, 0.0));

    let cache_bytes: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0)::bigint FROM image_cache_index")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    let image_cache_mb = cache_bytes as f64 / (1024.0 * 1024.0);

    let sample = ResourceSample {
        sampled_at: Utc::now(),
        scenario: scenario.clone(),
        app_pid: pid,
        app_cpu_pct,
        app_mem_mb,
        db_host_available,
        db_cpu_pct,
        db_mem_mb,
        db_size_mb,
        db_active_conns,
        db_total_conns,
        db_max_conns,
        db_cache_hit_pct,
        disk_total_gb,
        disk_used_gb,
        disk_avail_gb,
        disk_usage_pct,
        image_cache_mb,
    };

    {
        let mut ring = state.monitor.recent_samples.write().await;
        if ring.len() >= MAX_RECENT_SAMPLES {
            ring.pop_front();
        }
        ring.push_back(sample.clone());
    }

    // 5. 累加更新今日审计汇总表 resource_daily_stats
    sqlx::query(
        r#"
        INSERT INTO resource_daily_stats (
            stat_date, sample_count,
            app_cpu_avg, app_cpu_max, app_mem_avg_mb, app_mem_max_mb,
            db_cpu_avg, db_cpu_max, db_mem_avg_mb, db_mem_max_mb,
            db_size_max_mb, db_conn_avg, db_conn_max,
            disk_used_max_gb, disk_usage_max_pct,
            peak_cpu_at, peak_cpu_scenario, peak_mem_at, peak_mem_scenario,
            updated_at
        )
        VALUES (
            CURRENT_DATE, 1,
            $1, $1, $2, $2,
            $3, $3, $4, $4,
            $5, $6::double precision, $7,
            $8, $9,
            NOW(), $10, NOW(), $10,
            NOW()
        )
        ON CONFLICT (stat_date) DO UPDATE SET
            app_cpu_avg = (resource_daily_stats.app_cpu_avg * resource_daily_stats.sample_count + EXCLUDED.app_cpu_avg) / (resource_daily_stats.sample_count + 1),
            app_cpu_max = GREATEST(resource_daily_stats.app_cpu_max, EXCLUDED.app_cpu_max),
            app_mem_avg_mb = (resource_daily_stats.app_mem_avg_mb * resource_daily_stats.sample_count + EXCLUDED.app_mem_avg_mb) / (resource_daily_stats.sample_count + 1),
            app_mem_max_mb = GREATEST(resource_daily_stats.app_mem_max_mb, EXCLUDED.app_mem_max_mb),
            db_cpu_avg = (resource_daily_stats.db_cpu_avg * resource_daily_stats.sample_count + EXCLUDED.db_cpu_avg) / (resource_daily_stats.sample_count + 1),
            db_cpu_max = GREATEST(resource_daily_stats.db_cpu_max, EXCLUDED.db_cpu_max),
            db_mem_avg_mb = (resource_daily_stats.db_mem_avg_mb * resource_daily_stats.sample_count + EXCLUDED.db_mem_avg_mb) / (resource_daily_stats.sample_count + 1),
            db_mem_max_mb = GREATEST(resource_daily_stats.db_mem_max_mb, EXCLUDED.db_mem_max_mb),
            db_size_max_mb = GREATEST(resource_daily_stats.db_size_max_mb, EXCLUDED.db_size_max_mb),
            db_conn_avg = (resource_daily_stats.db_conn_avg * resource_daily_stats.sample_count + EXCLUDED.db_conn_avg) / (resource_daily_stats.sample_count + 1),
            db_conn_max = GREATEST(resource_daily_stats.db_conn_max, EXCLUDED.db_conn_max),
            disk_used_max_gb = GREATEST(resource_daily_stats.disk_used_max_gb, EXCLUDED.disk_used_max_gb),
            disk_usage_max_pct = GREATEST(resource_daily_stats.disk_usage_max_pct, EXCLUDED.disk_usage_max_pct),
            peak_cpu_at = CASE WHEN EXCLUDED.app_cpu_max >= resource_daily_stats.app_cpu_max THEN NOW() ELSE resource_daily_stats.peak_cpu_at END,
            peak_cpu_scenario = CASE WHEN EXCLUDED.app_cpu_max >= resource_daily_stats.app_cpu_max THEN EXCLUDED.peak_cpu_scenario ELSE resource_daily_stats.peak_cpu_scenario END,
            peak_mem_at = CASE WHEN EXCLUDED.app_mem_max_mb >= resource_daily_stats.app_mem_max_mb THEN NOW() ELSE resource_daily_stats.peak_mem_at END,
            peak_mem_scenario = CASE WHEN EXCLUDED.app_mem_max_mb >= resource_daily_stats.app_mem_max_mb THEN EXCLUDED.peak_mem_scenario ELSE resource_daily_stats.peak_mem_scenario END,
            sample_count = resource_daily_stats.sample_count + 1,
            updated_at = NOW()
        "#,
    )
    .bind(sample.app_cpu_pct)
    .bind(sample.app_mem_mb)
    .bind(sample.db_cpu_pct)
    .bind(sample.db_mem_mb)
    .bind(sample.db_size_mb)
    .bind(sample.db_total_conns as f64)
    .bind(sample.db_total_conns as i32)
    .bind(sample.disk_used_gb)
    .bind(sample.disk_usage_pct)
    .bind(&sample.scenario)
    .execute(&state.db)
    .await?;

    Ok(sample)
}

async fn sample_current_process(state: &AppState, pid: u32) -> (f64, f64) {
    #[cfg(target_os = "linux")]
    {
        let mem_mb = read_proc_rss_mb().await.unwrap_or(0.0);
        let cpu_pct = read_proc_cpu_pct(state).await.unwrap_or(0.0);
        return (round2(cpu_pct), round2(mem_mb));
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = state;
        sample_process_ps(pid).await.unwrap_or((0.0, 0.0))
    }
}

#[cfg(target_os = "linux")]
fn parse_vm_rss_kb(status: &str) -> Option<f64> {
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest.split_whitespace().next()?.parse::<f64>().ok();
        }
    }
    None
}

#[cfg(target_os = "linux")]
async fn read_proc_rss_mb() -> Option<f64> {
    let status = tokio::fs::read_to_string("/proc/self/status").await.ok()?;
    Some(parse_vm_rss_kb(&status)? / 1024.0)
}

#[cfg(target_os = "linux")]
fn parse_proc_stat_ticks(stat: &str) -> Option<u64> {
    let rparen = stat.rfind(')')?;
    let after = stat.get(rparen + 2..)?;
    let fields: Vec<&str> = after.split_whitespace().collect();
    // 跳过 state(0)..cmajflt(10)，utime=11, stime=12
    let utime = fields.get(11)?.parse::<u64>().ok()?;
    let stime = fields.get(12)?.parse::<u64>().ok()?;
    Some(utime + stime)
}

#[cfg(target_os = "linux")]
async fn read_proc_cpu_pct(state: &AppState) -> Option<f64> {
    let stat = tokio::fs::read_to_string("/proc/self/stat").await.ok()?;
    let total_ticks = parse_proc_stat_ticks(&stat)?;
    let now = Instant::now();

    let mut guard = state.monitor.prev_cpu_sample.lock().await;
    let pct = if let Some((prev_ticks, prev_at)) = *guard {
        let elapsed = now.duration_since(prev_at).as_secs_f64();
        if elapsed > 0.05 && total_ticks >= prev_ticks {
            // Linux USER_HZ 固定为 100
            let cpu_secs = (total_ticks - prev_ticks) as f64 / 100.0;
            (cpu_secs / elapsed) * 100.0
        } else {
            0.0
        }
    } else {
        0.0
    };
    *guard = Some((total_ticks, now));
    Some(pct)
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
async fn sample_process_ps(pid: u32) -> Option<(f64, f64)> {
    let out = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "%cpu=,rss="])
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| {
            warn!("执行 ps 采集当前进程指标失败: {}", error);
            error
        })
        .ok()?;
    if !out.status.success() {
        warn!("ps 采集当前进程指标退出异常: {}", out.status);
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.split_whitespace();
    let cpu = parts.next()?.parse::<f64>().ok()?;
    let rss_kb = parts.next()?.parse::<f64>().ok()?;
    Some((round2(cpu), round2(rss_kb / 1024.0)))
}

fn is_postgres_comm(comm: &str) -> bool {
    let lower = comm.trim().to_ascii_lowercase();
    lower.ends_with("postgres") || lower.contains("postgres:")
}

#[cfg(target_os = "linux")]
async fn sample_postgres_processes() -> (bool, f64, f64) {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    static PREV_PG_CPU: OnceLock<Mutex<Option<(HashMap<u32, u64>, Instant)>>> = OnceLock::new();

    let Ok(mut dir) = tokio::fs::read_dir("/proc").await else {
        return (false, 0.0, 0.0);
    };

    let mut found = false;
    let mut rss_kb_sum = 0.0;
    let mut cur_ticks: HashMap<u32, u64> = HashMap::new();

    while let Ok(Some(entry)) = dir.next_entry().await {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let proc_dir = entry.path();
        let Ok(comm) = tokio::fs::read_to_string(proc_dir.join("comm")).await else {
            continue;
        };
        if !is_postgres_comm(&comm) {
            continue;
        }
        found = true;
        if let Ok(status) = tokio::fs::read_to_string(proc_dir.join("status")).await {
            if let Some(kb) = parse_vm_rss_kb(&status) {
                rss_kb_sum += kb;
            }
        }
        if let Ok(stat) = tokio::fs::read_to_string(proc_dir.join("stat")).await {
            if let Some(ticks) = parse_proc_stat_ticks(&stat) {
                cur_ticks.insert(pid, ticks);
            }
        }
    }

    if !found {
        if let Ok(mut guard) = PREV_PG_CPU.get_or_init(|| Mutex::new(None)).lock() {
            *guard = None;
        }
        return (false, 0.0, 0.0);
    }

    let now = Instant::now();
    let cpu_pct = if let Ok(mut guard) = PREV_PG_CPU.get_or_init(|| Mutex::new(None)).lock() {
        let pct = if let Some((ref prev_map, prev_at)) = *guard {
            let elapsed = now.duration_since(prev_at).as_secs_f64();
            if elapsed > 0.05 {
                let delta_ticks: u64 = cur_ticks
                    .iter()
                    .map(|(pid, ticks)| {
                        prev_map
                            .get(pid)
                            .map(|prev| ticks.saturating_sub(*prev))
                            .unwrap_or(0)
                    })
                    .sum();
                (delta_ticks as f64 / 100.0 / elapsed) * 100.0
            } else {
                0.0
            }
        } else {
            0.0
        };
        *guard = Some((cur_ticks, now));
        pct
    } else {
        0.0
    };

    (true, round2(cpu_pct), round2(rss_kb_sum / 1024.0))
}

#[cfg(not(target_os = "linux"))]
async fn sample_postgres_processes() -> (bool, f64, f64) {
    let out = match Command::new("ps")
        .args(["-Ao", "%cpu=,rss=,comm="])
        .kill_on_drop(true)
        .output()
        .await
    {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            warn!("ps 采集 PostgreSQL 进程指标退出异常: {}", o.status);
            return (false, 0.0, 0.0);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!("未找到 ps 命令，跳过 PostgreSQL 进程指标采集");
            return (false, 0.0, 0.0);
        }
        Err(error) => {
            warn!("执行 ps 采集 PostgreSQL 进程指标失败: {}", error);
            return (false, 0.0, 0.0);
        }
    };

    let text = String::from_utf8_lossy(&out.stdout);
    let mut found = false;
    let mut cpu_sum = 0.0;
    let mut rss_kb_sum = 0.0;

    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(cpu_s), Some(rss_s), Some(comm)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if is_postgres_comm(comm) {
            if let (Ok(c), Ok(r)) = (cpu_s.parse::<f64>(), rss_s.parse::<f64>()) {
                found = true;
                cpu_sum += c;
                rss_kb_sum += r;
            }
        }
    }

    (found, round2(cpu_sum), round2(rss_kb_sum / 1024.0))
}

async fn sample_disk_df(path: &std::path::Path) -> Option<(f64, f64, f64, f64)> {
    let target = if tokio::fs::try_exists(path).await.unwrap_or(false) {
        path
    } else {
        std::path::Path::new(".")
    };
    let out = Command::new("df")
        .arg("-k")
        .arg(target)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| {
            warn!("执行 df 采集磁盘指标失败: {}", error);
            error
        })
        .ok()?;
    if !out.status.success() {
        warn!("df 采集磁盘指标退出异常: {}", out.status);
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let data_line = text.lines().nth(1)?;
    let cols: Vec<&str> = data_line.split_whitespace().collect();
    if cols.len() < 5 {
        return None;
    }
    let total_kb = cols[1].parse::<f64>().ok()?;
    let used_kb = cols[2].parse::<f64>().ok()?;
    let avail_kb = cols[3].parse::<f64>().ok()?;
    let kb_to_gb = 1024.0 * 1024.0;
    let total_gb = total_kb / kb_to_gb;
    let used_gb = used_kb / kb_to_gb;
    let avail_gb = avail_kb / kb_to_gb;
    let pct = if total_kb > 0.0 {
        (used_kb / total_kb) * 100.0
    } else {
        0.0
    };
    Some((
        round2(total_gb),
        round2(used_gb),
        round2(avail_gb),
        round2(pct),
    ))
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
