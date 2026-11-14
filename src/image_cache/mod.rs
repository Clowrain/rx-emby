use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{fs, time::sleep};
use tracing::{debug, error, info};
use xxhash_rust::xxh3::xxh3_64;

/// 计算基于文件名 xxh3 哈希前 2 位十六进制的分片相对路径（如 `w500/3a/xxx.jpg`）
fn image_shard_rel_path(size: &str, file_name: &str) -> String {
    let hash = xxh3_64(file_name.as_bytes());
    let shard = (hash & 0xff) as u8;
    format!("{}/{:02x}/{}", size, shard, file_name)
}

impl AppState {
    /// 获取本地缓存的图片字节流（首次访问自动回源 TMDB 并落盘，防并发击穿）
    pub async fn get_or_fetch_tmdb_image(&self, size: &str, file_name: &str) -> Result<Vec<u8>> {
        // 路径安全过滤，防止目录穿越攻击
        if size.contains("..") || file_name.contains("..") || file_name.contains('/') {
            return Err(anyhow!("非法的图片路径参数"));
        }

        let cache_key = format!("{}/{}", size, file_name);
        let rel_path = image_shard_rel_path(size, file_name);
        let cfg = self.config.load();
        let full_path = cfg.image_cache_dir.join(&rel_path);
        let legacy_path = cfg.image_cache_dir.join(size).join(file_name);

        // 1. 命中本地磁盘缓存（优先分片路径，兼容历史平铺路径）
        if let Ok(bytes) = fs::read(&full_path).await {
            self.touch_image_lru(&cache_key).await;
            return Ok(bytes);
        }
        if let Ok(bytes) = fs::read(&legacy_path).await {
            self.touch_image_lru(&cache_key).await;
            return Ok(bytes);
        }

        // 2. Singleflight 防缓存击穿：同一张图片并发请求只放行一次回源
        let _inflight_guard = self
            .caches
            .image_fetch_inflight
            .get_with(cache_key.clone(), async { () })
            .await;

        // 二次检查是否已被前序并发请求落盘
        if let Ok(bytes) = fs::read(&full_path).await {
            self.touch_image_lru(&cache_key).await;
            return Ok(bytes);
        }
        if let Ok(bytes) = fs::read(&legacy_path).await {
            self.touch_image_lru(&cache_key).await;
            return Ok(bytes);
        }

        // 3. 回源 TMDB 图片源（自动走配置的代理与反代域名）
        let remote_url = format!("{}/{}/{}", cfg.tmdb_image_base, size, file_name);
        let client = self.tmdb_client.load();
        let resp = client
            .get(&remote_url)
            .send()
            .await
            .with_context(|| format!("回源拉取 TMDB 图片失败: {}", remote_url))?
            .error_for_status()?;

        let bytes = resp.bytes().await?.to_vec();
        let size_bytes = bytes.len() as i64;

        // 4. 异步写入本地分片磁盘并更新 LRU 索引
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&full_path, &bytes).await?;

        let _ = sqlx::query(
            r#"
            INSERT INTO image_cache_index (cache_key, local_rel_path, size_bytes, last_accessed_at)
            VALUES ($1, $2, $3, NOW())
            ON CONFLICT (cache_key) DO UPDATE SET
                local_rel_path = EXCLUDED.local_rel_path,
                size_bytes = EXCLUDED.size_bytes,
                last_accessed_at = NOW()
            "#,
        )
        .bind(&cache_key)
        .bind(&rel_path)
        .bind(size_bytes)
        .execute(&self.db)
        .await;

        Ok(bytes)
    }

    async fn touch_image_lru(&self, cache_key: &str) {
        let db = self.db.clone();
        let key = cache_key.to_string();
        tokio::spawn(async move {
            let _ = sqlx::query(
                "UPDATE image_cache_index SET last_accessed_at = NOW() WHERE cache_key = $1",
            )
            .bind(&key)
            .execute(&db)
            .await;
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
    let target_to_free = current_total - low_limit;

    let victims: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT cache_key, local_rel_path, size_bytes FROM image_cache_index ORDER BY last_accessed_at ASC LIMIT 2000",
    )
    .fetch_all(&state.db)
    .await?;

    let mut deleted_keys = Vec::new();
    for (key, rel_path, sz) in victims {
        if freed_bytes >= target_to_free {
            break;
        }
        let file_path: PathBuf = cfg.image_cache_dir.join(&rel_path);
        let _ = fs::remove_file(&file_path).await;
        freed_bytes += sz;
        deleted_keys.push(key);
    }

    if !deleted_keys.is_empty() {
        sqlx::query("DELETE FROM image_cache_index WHERE cache_key = ANY($1)")
            .bind(&deleted_keys)
            .execute(&state.db)
            .await?;
        debug!(
            "图片缓存 LRU 淘汰完成，释放空间: {} MB (清理 {} 个文件)",
            freed_bytes / 1024 / 1024,
            deleted_keys.len()
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
