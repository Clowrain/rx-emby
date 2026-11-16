use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tracing::{debug, info, warn};

type HmacSha256 = Hmac<Sha256>;

fn refresh_completed_since(request_started: Instant, last_refresh: Option<Instant>) -> bool {
    last_refresh.is_some_and(|refreshed_at| refreshed_at > request_started)
}

#[cfg(test)]
mod tests {
    use super::refresh_completed_since;
    use std::time::Instant;

    #[test]
    fn recognizes_refresh_completed_after_request_started() {
        let started = Instant::now();
        let completed = started + std::time::Duration::from_millis(1);

        assert!(refresh_completed_since(started, Some(completed)));
        assert!(!refresh_completed_since(started, Some(started)));
        assert!(!refresh_completed_since(started, None));
    }
}

/// URL 路径段百分号编码字符集（保留 `/` 分隔符，安全编码中文、空格与 Emoji）
const PATH_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'[')
    .add(b']')
    .add(b'&')
    .add(b'+');

#[derive(Serialize)]
struct LoginReq<'a> {
    username: &'a str,
    password: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    otp_code: Option<&'a str>,
}

#[derive(Deserialize)]
struct AlistResp<T> {
    code: i32,
    message: String,
    data: Option<T>,
}

#[derive(Deserialize)]
struct LoginData {
    token: String,
}

#[derive(Serialize)]
struct FsGetReq<'a> {
    path: &'a str,
    password: &'a str,
}

#[derive(Serialize)]
struct FsListReq<'a> {
    path: &'a str,
    password: &'a str,
    page: i32,
    per_page: i32,
    refresh: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct FsGetData {
    pub size: i64,
    pub raw_url: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct FsListData {
    content: Option<Vec<FsListItem>>,
}

#[derive(Deserialize, Debug, Clone)]
struct FsListItem {
    name: String,
    size: i64,
    is_dir: bool,
}

#[derive(Debug, Clone)]
pub struct ResolvedStream {
    pub redirect_url: String,
}

impl AppState {
    /// 获取有效 AList Token（带并发防雪崩锁）
    pub async fn get_alist_token(&self, force_refresh: bool) -> Result<String> {
        let request_started = Instant::now();
        if !force_refresh {
            let guard = self.alist_auth.current_token.read().await;
            if let Some(ref token) = *guard {
                return Ok(token.clone());
            }
        }

        // 加互斥锁防止并发请求同时触发多次登录
        let _lock = self.alist_auth.refresh_lock.lock().await;

        // 双重检查；强制刷新请求若等待期间已有请求完成刷新，则复用新 Token。
        if force_refresh {
            let last_refresh = *self.alist_auth.last_refresh_at.read().await;
            if refresh_completed_since(request_started, last_refresh) {
                if let Some(token) = self.alist_auth.current_token.read().await.as_ref() {
                    return Ok(token.clone());
                }
            }
        } else {
            let guard = self.alist_auth.current_token.read().await;
            if let Some(ref token) = *guard {
                return Ok(token.clone());
            }
        }

        let cfg = self.config.load();
        let login_url = format!("{}/api/auth/login", cfg.alist_base_url);
        let client = self.alist_client.load();

        let resp = client
            .post(&login_url)
            .json(&LoginReq {
                username: &cfg.alist_username,
                password: &cfg.alist_password,
                otp_code: None,
            })
            .send()
            .await
            .context("请求 AList 登录接口失败")?;

        let body: AlistResp<LoginData> = resp.json().await.context("解析 AList 登录响应失败")?;
        if body.code != 200 {
            return Err(anyhow!(
                "AList 登录失败 (code={}): {}",
                body.code,
                body.message
            ));
        }

        let token = body
            .data
            .map(|d| d.token)
            .ok_or_else(|| anyhow!("AList 登录成功但未返回 token"))?;

        let mut guard = self.alist_auth.current_token.write().await;
        *guard = Some(token.clone());
        *self.alist_auth.last_refresh_at.write().await = Some(Instant::now());
        info!("AList 自动登录成功并已刷新内存 Token");
        Ok(token)
    }

    /// 结合路径前缀拼装最终在 AList 中的完整路径
    pub fn build_full_alist_path(&self, alist_path: &str) -> String {
        let cfg = self.config.load();
        if cfg.alist_path_prefix.is_empty() {
            alist_path.to_string()
        } else {
            format!("{}{}", cfg.alist_path_prefix, alist_path)
        }
    }

    /// 获取播放器 302 跳转直链（Singleflight 并发合并、严格透传客户端 User-Agent、按 UA 隔离缓存，支持 401 重登与 /d/ 签名降级）
    pub async fn resolve_playback_url(
        &self,
        path_hash: i64,
        alist_path: &str,
        client_ua: &str,
    ) -> Result<ResolvedStream> {
        let effective_ua = if client_ua.trim().is_empty() {
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36"
        } else {
            client_ua.trim()
        };

        let cache_key = format!("{}:{}", path_hash, effective_ua);
        if let Some(cached_url) = self.caches.alist_direct_urls.get(&cache_key).await {
            debug!(
                "⚡ [DirectURL HIT] path_hash={} 命中直链内存缓存",
                path_hash
            );
            return Ok(ResolvedStream {
                redirect_url: cached_url,
            });
        }

        let full_path = self.build_full_alist_path(alist_path);
        let db = self.db.clone();
        // Singleflight：当 PlaybackInfo (IsPlayback=true) 预热与播放器 stream.mp4 并发到达时，共享同一个换链请求
        let final_url = self
            .caches
            .alist_direct_urls
            .try_get_with(cache_key, async {
                let total_start = Instant::now();
                let fs_data = match self.call_alist_fs_get(&full_path, effective_ua, false).await {
                    Ok(data) => data,
                    Err(e) if e.to_string().contains("401") => {
                        warn!("AList Token 失效，触发自动重登并重试 /api/fs/get");
                        self.call_alist_fs_get(&full_path, effective_ua, true).await?
                    }
                    Err(e) => return Err(e),
                };

                let file_size = if fs_data.size > 0 { Some(fs_data.size) } else { None };
                if let Some(sz) = file_size {
                    let db_clone = db.clone();
                    tokio::spawn(async move {
                        let _ = sqlx::query(
                            "UPDATE sync_files SET file_size_bytes = $1 WHERE path_hash = $2 AND file_size_bytes IS NULL",
                        )
                        .bind(sz)
                        .bind(path_hash)
                        .execute(&db_clone)
                        .await;
                    });
                }

                // 优先使用单跳网盘 CDN raw_url；若为空（强制中转网盘）则降级走 AList /d/ 签名直链
                let url = match fs_data.raw_url.filter(|u| !u.trim().is_empty()) {
                    Some(raw_url) => raw_url,
                    None => {
                        let token = self.get_alist_token(false).await?;
                        self.build_signed_d_url(&full_path, &token, 14400)?
                    }
                };

                info!(
                    "🔗 [DirectURL FETCH] path={} | size={:?} | 换链总耗时={}ms",
                    full_path,
                    file_size,
                    total_start.elapsed().as_millis()
                );
                Ok::<String, anyhow::Error>(url)
            })
            .await
            .map_err(|e| anyhow!("{}", e))?;

        Ok(ResolvedStream {
            redirect_url: final_url,
        })
    }

    /// 零转存浪费的目录寻址预热：用户点进详情页/分集列表时，在后台对父目录执行只读 `/api/fs/list`，
    /// 让小雅提前把挂载目录树和分享文件列表加载进内存，并顺手把整季单集的真实文件体积批量回填到数据库。
    pub fn spawn_prewarm_media_dirs(self: &Arc<Self>, work_id: i64, season_number: Option<i32>) {
        let state = self.clone();
        tokio::spawn(async move {
            let sample_paths: Vec<String> = sqlx::query_scalar(
                r#"
                SELECT DISTINCT ON (sf.folder_id, sf.season_number) sf.alist_path
                FROM sync_files sf
                JOIN sync_folders f ON f.id = sf.folder_id
                WHERE f.work_id = $1
                  AND f.is_ignored = FALSE
                  AND ($2::int4 IS NULL OR sf.season_number = $2)
                ORDER BY sf.folder_id ASC, sf.season_number ASC, COALESCE(sf.priority_override, sf.priority_score + f.priority_boost) DESC
                LIMIT 4
                "#,
            )
            .bind(work_id)
            .bind(season_number)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

            for alist_path in sample_paths {
                let Some((parent_dir, _)) = alist_path.rsplit_once('/') else {
                    continue;
                };
                if parent_dir.is_empty() {
                    continue;
                }
                let full_dir = state.build_full_alist_path(parent_dir);
                if state
                    .caches
                    .alist_dir_prewarmed
                    .get(&full_dir)
                    .await
                    .is_some()
                {
                    continue;
                }

                let state_inner = state.clone();
                let dir_key = full_dir.clone();
                let _ = state
                    .caches
                    .alist_dir_prewarmed
                    .try_get_with(dir_key, async move {
                        let start = Instant::now();
                        let items = state_inner.call_alist_fs_list(&full_dir).await?;
                        let elapsed = start.elapsed().as_millis();

                        let mut sizes_by_name = std::collections::HashMap::new();
                        for item in &items {
                            if !item.is_dir && item.size > 0 {
                                sizes_by_name.entry(item.name.clone()).or_insert(item.size);
                            }
                        }
                        let (file_names, file_sizes): (Vec<_>, Vec<_>) =
                            sizes_by_name.into_iter().unzip();
                        let updated = if file_names.is_empty() {
                            0
                        } else {
                            sqlx::query(
                                r#"
                                UPDATE sync_files sf SET file_size_bytes = listing.file_size_bytes
                                FROM sync_folders f,
                                     unnest($2::text[], $3::bigint[]) AS listing(file_name, file_size_bytes)
                                WHERE sf.folder_id = f.id
                                  AND f.work_id = $1
                                  AND f.is_ignored = FALSE
                                  AND sf.file_name = listing.file_name
                                  AND sf.file_size_bytes IS NULL
                                "#,
                            )
                            .bind(work_id)
                            .bind(file_names)
                            .bind(file_sizes)
                            .execute(&state_inner.db)
                            .await
                            .map(|result| result.rows_affected() as usize)
                            .unwrap_or_default()
                        };

                        info!(
                            "📂 [DirPrewarm] 小雅目录寻址预热完成: dir={} | 条目数={} | 回填体积={}条 | 耗时={}ms",
                            full_dir,
                            items.len(),
                            updated,
                            elapsed
                        );
                        Ok::<(), anyhow::Error>(())
                    })
                    .await;
            }
        });
    }

    async fn call_alist_fs_list(&self, full_dir: &str) -> Result<Vec<FsListItem>> {
        match self.call_alist_fs_list_inner(full_dir, false).await {
            Ok(items) => Ok(items),
            Err(e) if e.to_string().contains("401") => {
                warn!("AList Token 失效，触发自动重登并重试 /api/fs/list");
                self.call_alist_fs_list_inner(full_dir, true).await
            }
            Err(e) => Err(e),
        }
    }

    async fn call_alist_fs_list_inner(
        &self,
        full_dir: &str,
        force_refresh_token: bool,
    ) -> Result<Vec<FsListItem>> {
        let token = self.get_alist_token(force_refresh_token).await?;
        let cfg = self.config.load();
        let url = format!("{}/api/fs/list", cfg.alist_base_url);
        let client = self.alist_client.load();

        let resp = client
            .post(&url)
            .header("Authorization", &token)
            .json(&FsListReq {
                path: full_dir,
                password: "",
                page: 1,
                per_page: 0,
                refresh: false,
            })
            .send()
            .await
            .context("调用 AList /api/fs/list 失败")?;

        let status = resp.status();
        if status.as_u16() == 401 {
            return Err(anyhow!("401 Unauthorized"));
        }
        if !status.is_success() {
            return Err(anyhow!("AList /api/fs/list HTTP 错误: {}", status));
        }

        let body: AlistResp<FsListData> =
            resp.json().await.context("解析 /api/fs/list 响应失败")?;
        if body.code == 401 {
            return Err(anyhow!("401 Unauthorized: {}", body.message));
        }
        if body.code != 200 {
            return Err(anyhow!(
                "AList fs/list 错误 (code={}): {}",
                body.code,
                body.message
            ));
        }

        Ok(body.data.and_then(|d| d.content).unwrap_or_default())
    }

    async fn call_alist_fs_get(
        &self,
        full_path: &str,
        client_ua: &str,
        force_refresh_token: bool,
    ) -> Result<FsGetData> {
        let token = self.get_alist_token(force_refresh_token).await?;
        let cfg = self.config.load();
        let url = format!("{}/api/fs/get", cfg.alist_base_url);
        let client = self.alist_client.load();

        let resp = client
            .post(&url)
            .header("Authorization", &token)
            .header("User-Agent", client_ua) // 关键：透传播放器真实 UA，防止网盘 CDN 403
            .json(&FsGetReq {
                path: full_path,
                password: "",
            })
            .send()
            .await
            .context("调用 AList /api/fs/get 失败")?;

        let status = resp.status();
        if status.as_u16() == 401 {
            return Err(anyhow!("401 Unauthorized"));
        }
        if !status.is_success() {
            return Err(anyhow!("AList /api/fs/get HTTP 错误: {}", status));
        }

        let body: AlistResp<FsGetData> = resp.json().await.context("解析 /api/fs/get 响应失败")?;
        if body.code == 401 {
            return Err(anyhow!("401 Unauthorized: {}", body.message));
        }
        if body.code != 200 {
            return Err(anyhow!(
                "AList fs/get 错误 (code={}): {}",
                body.code,
                body.message
            ));
        }

        body.data
            .ok_or_else(|| anyhow!("AList fs/get 未返回文件数据"))
    }

    /// 本地计算 AList /d/ 路径的 HMAC-SHA256 签名直链（用于强制中转网盘兜底）
    pub fn build_signed_d_url(
        &self,
        full_path: &str,
        token: &str,
        expire_sec: u64,
    ) -> Result<String> {
        let cfg = self.config.load();
        let expire_ts = if expire_sec == 0 {
            0
        } else {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                + expire_sec
        };

        let to_sign = format!("{}:{}", full_path, expire_ts);
        let mut mac = HmacSha256::new_from_slice(token.as_bytes())
            .map_err(|_| anyhow!("初始化 HMAC-SHA256 失败"))?;
        mac.update(to_sign.as_bytes());
        let signature = URL_SAFE.encode(mac.finalize().into_bytes());
        let sign_param = format!("{}:{}", signature, expire_ts);

        let encoded_path = utf8_percent_encode(full_path, PATH_ENCODE_SET).to_string();
        Ok(format!(
            "{}/d{}?sign={}",
            cfg.alist_base_url, encoded_path, sign_param
        ))
    }
}
