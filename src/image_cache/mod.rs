pub mod collage;

use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{fs, time::sleep};
use tracing::{debug, error, info, warn};
use xxhash_rust::xxh3::xxh3_64;

/// 计算基于文件名 xxh3 哈希前 2 位十六进制的分片相对路径（如 `w500/3a/xxx.jpg`）
fn image_shard_rel_path(size: &str, file_name: &str) -> String {
    let hash = xxh3_64(file_name.as_bytes());
    let shard = (hash & 0xff) as u8;
    format!("{}/{:02x}/{}", size, shard, file_name)
}

impl AppState {
    /// 获取图片字节流（开启磁盘缓存时自动落盘并防击穿；关闭磁盘缓存时直接回源返回）。
    ///
    /// `image_hot_bytes` 的 `try_get_with` 同时承担两层职责：
    /// 1. 内存热缓存：热门海报/背景图命中后无需重复读盘；
    /// 2. 真正的 singleflight：同一 `cache_key` 的并发请求只会执行一次回源，其余等待共享结果。
    pub async fn get_or_fetch_tmdb_image(&self, size: &str, file_name: &str) -> Result<Arc<Vec<u8>>> {
        // 路径安全过滤，防止目录穿越攻击
        if size.contains("..") || file_name.contains("..") || file_name.contains('/') {
            return Err(anyhow!("非法的图片路径参数"));
        }

        let cache_key = format!("{}/{}", size, file_name);
        let key = cache_key.clone();
        let bytes = self
            .caches
            .image_hot_bytes
            .try_get_with(cache_key, async { self.load_image_bytes(size, file_name, &key).await })
            .await
            .map_err(|e| anyhow!("{:#}", e))?;
        Ok(bytes)
    }

    /// 实际加载逻辑：优先命中本地磁盘缓存，未命中则限流回源 TMDB 并原子落盘
    async fn load_image_bytes(&self, size: &str, file_name: &str, cache_key: &str) -> Result<Arc<Vec<u8>>> {
        let cfg = self.config.load();

        // 1. 命中本地磁盘缓存（优先分片路径，兼容历史平铺路径）
        if cfg.image_disk_cache_enabled {
            let rel_path = image_shard_rel_path(size, file_name);
            let full_path = cfg.image_cache_dir.join(&rel_path);
            let legacy_path = cfg.image_cache_dir.join(size).join(file_name);

            if let Ok(bytes) = fs::read(&full_path).await {
                self.touch_image_lru(cache_key).await;
                return Ok(Arc::new(bytes));
            }
            if let Ok(bytes) = fs::read(&legacy_path).await {
                self.touch_image_lru(cache_key).await;
                return Ok(Arc::new(bytes));
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

        let bytes = Arc::new(resp.bytes().await?.to_vec());

        // 4. 原子写入本地分片磁盘并更新 LRU 索引：先写同目录唯一临时文件再 rename 到位，
        //    并发读端永远不会看到半截图片文件
        if cfg.image_disk_cache_enabled {
            let rel_path = image_shard_rel_path(size, file_name);
            let full_path = cfg.image_cache_dir.join(&rel_path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).await?;
            }
            let tmp_path = full_path.with_extension(format!(
                "tmp-{}",
                uuid::Uuid::new_v4().simple()
            ));
            fs::write(&tmp_path, bytes.as_ref()).await?;
            if let Err(e) = fs::rename(&tmp_path, &full_path).await {
                let _ = fs::remove_file(&tmp_path).await;
                return Err(e).with_context(|| format!("原子落盘图片失败: {}", full_path.display()));
            }

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
            .bind(bytes.len() as i64)
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
        if self.caches.image_lru_touch_debounce.get(cache_key).await.is_some() {
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
pub fn spawn_image_lru_worker(state: Arc<AppState>) {
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
    });
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
        let victims: Vec<(String, String, i64, chrono::DateTime<chrono::Utc>)> = if let Some((ts, key)) = &cursor {
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
    use super::image_shard_rel_path;

    #[test]
    fn test_image_shard_rel_path() {
        let p1 = image_shard_rel_path("w500", "18vQBzGNLrEYCUaU9SaWanpms8D.jpg");
        let p2 = image_shard_rel_path("w500", "18vQBzGNLrEYCUaU9SaWanpms8D.jpg");
        assert_eq!(p1, p2);

        let parts: Vec<&str> = p1.split('/').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "w500");
        assert_eq!(parts[1].len(), 2);
        assert!(parts[1].chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(parts[2], "18vQBzGNLrEYCUaU9SaWanpms8D.jpg");
    }
}
