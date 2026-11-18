pub mod collage;

const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const TMDB_IMAGE_SIZES: &[&str] = &[
    "w45", "w92", "w154", "w185", "w300", "w342", "w500", "w780", "w1280", "h632", "original",
];

use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{fs, time::sleep};
use tracing::{debug, error, info, warn};
use xxhash_rust::xxh3::xxh3_64;

/// 计算基于文件名 xxh3 哈希前 2 位十六进制的分片相对路径（如 `w500/3a/xxx.jpg`）
fn image_shard_rel_path(size: &str, file_name: &str) -> String {
    let hash = xxh3_64(file_name.as_bytes());
    let shard = (hash & 0xff) as u8;
    format!("{}/{:02x}/{}", size, shard, file_name)
}

async fn atomic_write_image_file(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let tmp_path = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
    if let Err(error) = fs::write(&tmp_path, data).await {
        let _ = fs::remove_file(&tmp_path).await;
        return Err(error).with_context(|| format!("原子落盘图片失败: {}", path.display()));
    }
    if let Err(error) = fs::rename(&tmp_path, path).await {
        let _ = fs::remove_file(&tmp_path).await;
        return Err(error).with_context(|| format!("原子落盘图片失败: {}", path.display()));
    }
    Ok(())
}

impl AppState {
    /// 获取图片字节流（开启磁盘缓存时自动落盘并防击穿；关闭磁盘缓存时直接回源返回）。
    ///
    /// `image_hot_bytes` 的 `try_get_with` 同时承担两层职责：
    /// 1. 内存热缓存：热门海报/背景图命中后无需重复读盘；
    /// 2. 真正的 singleflight：同一 `cache_key` 的并发请求只会执行一次回源，其余等待共享结果。
    pub async fn get_or_fetch_tmdb_image(&self, size: &str, file_name: &str) -> Result<Bytes> {
        // 路径安全过滤，防止目录穿越攻击
        if !TMDB_IMAGE_SIZES.contains(&size)
            || file_name.contains("..")
            || file_name.contains('/')
            || file_name.contains('\\')
            || file_name.contains('\0')
        {
            return Err(anyhow!("非法的图片路径参数"));
        }

        let cache_key = format!("{}/{}", size, file_name);
        let key = cache_key.clone();
        let bytes = self
            .caches
            .image_hot_bytes
            .try_get_with(cache_key, async {
                self.load_image_bytes(size, file_name, &key).await
            })
            .await
            .map_err(|e| anyhow!("{:#}", e))?;
        Ok(bytes)
    }

    /// 实际加载逻辑：优先命中本地磁盘缓存，未命中则限流回源 TMDB 并原子落盘
    async fn load_image_bytes(
        &self,
        size: &str,
        file_name: &str,
        cache_key: &str,
    ) -> Result<Bytes> {
        let cfg = self.config.load();

        // 1. 命中本地磁盘缓存（优先分片路径，兼容历史平铺路径）
        if cfg.image_disk_cache_enabled {
            let rel_path = image_shard_rel_path(size, file_name);
            let full_path = cfg.image_cache_dir.join(&rel_path);
            let legacy_path = cfg.image_cache_dir.join(size).join(file_name);

            if let Ok(bytes) = fs::read(&full_path).await {
                self.touch_image_lru(cache_key).await;
                return Ok(Bytes::from(bytes));
            }
            if let Ok(bytes) = fs::read(&legacy_path).await {
                self.touch_image_lru(cache_key).await;
                return Ok(Bytes::from(bytes));
            }
        }

        // 2. 出站并发限流：避免海报墙同时打开过多 TMDB 下载连接
        let _permit = self
            .caches
            .image_download_permits
            .clone()
            .acquire_owned()
            .await;

        // 3. 回源 TMDB 图片源（自动走配置的代理与反代域名）
        let remote_url = format!("{}/{}/{}", cfg.tmdb_image_base, size, file_name);
        let client = self.tmdb_client.load();
        let resp = client
            .get(&remote_url)
            .send()
            .await
            .with_context(|| format!("回源拉取 TMDB 图片失败: {}", remote_url))?
            .error_for_status()?;

        let mut resp = resp;
        let mut data = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            if data.len().saturating_add(chunk.len()) > MAX_IMAGE_BYTES {
                return Err(anyhow!("TMDB 图片超过 {} 字节上限", MAX_IMAGE_BYTES));
            }
            data.extend_from_slice(&chunk);
        }
        let bytes = Bytes::from(data);

        // 4. 原子写入本地分片磁盘并更新 LRU 索引：先写同目录唯一临时文件再 rename 到位，
        //    并发读端永远不会看到半截图片文件
        if cfg.image_disk_cache_enabled {
            let rel_path = image_shard_rel_path(size, file_name);
            let full_path = cfg.image_cache_dir.join(&rel_path);
            atomic_write_image_file(&full_path, bytes.as_ref()).await?;

            if let Err(e) = sqlx::query(
                r#"
                INSERT INTO image_cache_index (cache_key, local_rel_path, size_bytes, last_accessed_at)
                VALUES ($1, $2, $3, NOW())
                ON CONFLICT (cache_key) DO UPDATE SET
                    local_rel_path = EXCLUDED.local_rel_path,
                    size_bytes = EXCLUDED.size_bytes,
                    last_accessed_at = NOW()
                "#,
            )
            .bind(cache_key)
            .bind(&rel_path)
            .bind(i64::try_from(bytes.len()).unwrap_or(i64::MAX))
            .execute(&self.db)
            .await
            {
                warn!("写入图片缓存 LRU 索引失败 ({}): {}", cache_key, e);
            }

            // 刚落盘的内容 30 分钟去抖窗口内无需再 touch
            self.caches
                .image_lru_touch_debounce
                .insert(cache_key.to_string(), ())
                .await;
        }

        Ok(bytes)
    }

    /// 去抖更新 LRU 访问时间：同一 key 在 30 分钟窗口内至多触发一次落库 UPDATE，
    /// 避免海报墙浏览期间的每请求 PG 写放大
    async fn touch_image_lru(&self, cache_key: &str) {
        if self
            .caches
            .image_lru_touch_debounce
            .get(cache_key)
            .await
            .is_some()
        {
            return;
        }
        self.caches
            .image_lru_touch_debounce
            .insert(cache_key.to_string(), ())
            .await;

        let db = self.db.clone();
        let key = cache_key.to_string();
        tokio::spawn(async move {
            // 仅当距上次更新时间超过 1 小时才执行写入
            if let Err(e) = sqlx::query(
                "UPDATE image_cache_index SET last_accessed_at = NOW() WHERE cache_key = $1 AND last_accessed_at < NOW() - INTERVAL '1 hour'",
            )
            .bind(&key)
            .execute(&db)
            .await
            {
                warn!("更新图片 LRU 访问时间失败 ({}): {}", key, e);
            }
        });
    }
}

/// 启动后台图片缓存 LRU 水位线巡检清理任务
pub fn spawn_image_lru_worker(state: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!("图片磁盘缓存 LRU 水位线监控协程已启动");
        loop {
            tokio::select! {
                _ = state.shutdown_token.cancelled() => break,
                _ = sleep(Duration::from_secs(600)) => {
                    if !state.config.load().image_disk_cache_enabled {
                        continue;
                    }
                    if let Err(e) = enforce_image_cache_quota(&state).await {
                        error!("执行图片缓存 LRU 淘汰异常: {:#}", e);
                    }
                }
            }
        }
    })
}

pub async fn enforce_image_cache_quota(state: &AppState) -> Result<u64> {
    let cfg = state.config.load();
    let max_bytes = (cfg.image_max_size_mb * 1024 * 1024) as i64;
    if max_bytes <= 0 {
        return Ok(0);
    }

    let high_limit = (max_bytes as f64 * cfg.image_evict_high_watermark) as i64;
    let low_limit = (max_bytes as f64 * cfg.image_evict_low_watermark) as i64;

    let current_total: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0)::bigint FROM image_cache_index")
            .fetch_one(&state.db)
            .await?;

    if current_total <= high_limit {
        return Ok(0);
    }

    info!(
        "图片缓存总大小 ({} MB) 超过高水位线 ({} MB)，开始执行 LRU 淘汰...",
        current_total / 1024 / 1024,
        high_limit / 1024 / 1024
    );

    let mut freed_bytes: i64 = 0;
    let mut deleted_count: usize = 0;
    let target_to_free = current_total - low_limit;
    // keyset 分页游标：必须与 ORDER BY (last_accessed_at, cache_key) 完全对齐使用元组比较，
    // 单独用 cache_key > $1 在 cache_key 与 last_accessed_at 不单调一致时会漏扫/重扫
    let mut cursor: Option<(chrono::DateTime<chrono::Utc>, String)> = None;

    while freed_bytes < target_to_free {
        let victims: Vec<(String, String, i64, chrono::DateTime<chrono::Utc>)> =
            if let Some((ts, key)) = &cursor {
                sqlx::query_as(
                    r#"
                SELECT cache_key, local_rel_path, size_bytes, last_accessed_at
                FROM image_cache_index
                WHERE (last_accessed_at, cache_key) > ($1, $2)
                ORDER BY last_accessed_at ASC, cache_key ASC
                LIMIT 2000
                "#,
                )
                .bind(ts)
                .bind(key)
                .fetch_all(&state.db)
                .await?
            } else {
                sqlx::query_as(
                    r#"
                SELECT cache_key, local_rel_path, size_bytes, last_accessed_at
                FROM image_cache_index
                ORDER BY last_accessed_at ASC, cache_key ASC
                LIMIT 2000
                "#,
                )
                .fetch_all(&state.db)
                .await?
            };

        if victims.is_empty() {
            break;
        }
        cursor = victims.last().map(|(key, _, _, ts)| (*ts, key.clone()));

        let mut deleted_keys = Vec::new();
        for (key, rel_path, sz, _) in victims {
            if freed_bytes >= target_to_free {
                break;
            }
            let file_path: PathBuf = cfg.image_cache_dir.join(&rel_path);
            let removed = match fs::remove_file(&file_path).await {
                Ok(()) => true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
                Err(e) => {
                    warn!("删除图片缓存文件失败 ({}): {}", file_path.display(), e);
                    false
                }
            };
            if removed {
                freed_bytes += sz;
                deleted_keys.push(key);
            }
        }

        if deleted_keys.is_empty() {
            break;
        }
        deleted_count += deleted_keys.len();
        sqlx::query("DELETE FROM image_cache_index WHERE cache_key = ANY($1)")
            .bind(&deleted_keys)
            .execute(&state.db)
            .await?;
    }

    if deleted_count > 0 {
        debug!(
            "图片缓存 LRU 淘汰完成，释放空间: {} MB (清理 {} 个文件)",
            freed_bytes / 1024 / 1024,
            deleted_count
        );
    }

    Ok(freed_bytes as u64)
}

#[cfg(test)]
mod tests {
    use super::{atomic_write_image_file, image_shard_rel_path};
    use bytes::Bytes;
    use moka::future::Cache;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn same_key_concurrent_loads_are_coalesced() {
        let cache: Cache<String, Bytes> = Cache::builder()
            .max_capacity(1024)
            .weigher(|_key: &String, value: &Bytes| value.len() as u32)
            .build();
        let loads = Arc::new(AtomicUsize::new(0));
        let mut requests = Vec::new();

        for _ in 0..8 {
            let cache = cache.clone();
            let loads = loads.clone();
            requests.push(tokio::spawn(async move {
                cache
                    .try_get_with("same-image".to_string(), async move {
                        loads.fetch_add(1, Ordering::SeqCst);
                        sleep(Duration::from_millis(30)).await;
                        Ok::<_, &'static str>(Bytes::from_static(b"image"))
                    })
                    .await
                    .unwrap()
            }));
        }

        for request in requests {
            assert_eq!(request.await.unwrap().as_ref(), b"image");
        }
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn hot_cache_capacity_is_weighted_by_image_bytes() {
        let cache: Cache<String, Bytes> = Cache::builder()
            .max_capacity(5)
            .weigher(|_key: &String, value: &Bytes| value.len() as u32)
            .build();

        cache
            .insert("large".to_string(), Bytes::from(vec![1; 4]))
            .await;
        cache.run_pending_tasks().await;
        cache
            .insert("small".to_string(), Bytes::from(vec![2; 3]))
            .await;
        cache.run_pending_tasks().await;

        let large = cache.get(&"large".to_string()).await;
        let small = cache.get(&"small".to_string()).await;
        assert!(
            large.is_some() || small.is_some(),
            "the cache should retain an entry"
        );
        assert!(
            large.is_none() || small.is_none(),
            "byte weights must force one eviction"
        );
    }

    #[tokio::test]
    async fn atomic_image_write_leaves_no_temporary_files() {
        let root =
            std::env::temp_dir().join(format!("rx-emby-image-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let destination = root.join("poster.jpg");
        atomic_write_image_file(&destination, b"image-bytes")
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"image-bytes");
        let mut entries = tokio::fs::read_dir(&root).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        assert_eq!(names, vec!["poster.jpg"]);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[test]
    fn test_image_shard_rel_path() {
        let p1 = image_shard_rel_path("w500", "18vQBzGNLrEYCUaU9SaWanpms8D.jpg");
        let p2 = image_shard_rel_path("w500", "18vQBzGNLrEYCUaU9SaWanpms8D.jpg");
        assert_eq!(p1, p2);

        let parts: Vec<&str> = p1.split('/').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "w500");
        assert_eq!(parts[1].len(), 2);
        assert!(parts[1]
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(parts[2], "18vQBzGNLrEYCUaU9SaWanpms8D.jpg");
    }
}
