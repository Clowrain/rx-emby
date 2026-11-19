use super::auth::resolve_valid_user_id_with_query;
use super::items::ensure_media_accessible;
use super::query::EmbyQuery as Query;
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::Response,
};
use serde::Deserialize;
use std::sync::{Arc, OnceLock};

const MAX_SUBTITLE_BYTES: usize = 16 * 1024 * 1024;
static SUBTITLE_HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn subtitle_http_client() -> Result<&'static reqwest::Client, (StatusCode, String)> {
    if let Some(client) = SUBTITLE_HTTP_CLIENT.get() {
        return Ok(client);
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
    let _ = SUBTITLE_HTTP_CLIENT.set(client);
    SUBTITLE_HTTP_CLIENT
        .get()
        .ok_or_else(|| (StatusCode::BAD_GATEWAY, "字幕 HTTP 客户端初始化失败".into()))
}

#[derive(Deserialize, Default)]
pub(super) struct SubtitleQuery {
    #[serde(
        rename = "api_key",
        alias = "ApiKey",
        alias = "X-Emby-Token",
        alias = "x-emby-token"
    )]
    api_key: Option<String>,
}

struct SubtitleStreamRequest {
    item_id: String,
    media_source_id: String,
    index: usize,
    stream_file: String,
    query: SubtitleQuery,
}

/// `GET / HEAD /Videos/:item_id/:media_source_id/Subtitles/:index/:stream_file`（兼容 `Stream.srt`/`Stream.vtt` 等）
/// 及 `/Items/:item_id/:media_source_id/Subtitles/:index/:stream_file`
pub(super) async fn get_subtitle_stream(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    method: Method,
    Path((item_id, media_source_id, index, stream_file)): Path<(String, String, usize, String)>,
    Query(q): Query<SubtitleQuery>,
) -> Result<Response, (StatusCode, String)> {
    get_subtitle_stream_inner(
        state,
        headers,
        method,
        SubtitleStreamRequest {
            item_id,
            media_source_id,
            index,
            stream_file,
            query: q,
        },
    )
    .await
}

pub(super) async fn get_subtitle_stream_at(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    method: Method,
    Path((item_id, media_source_id, index, _start_ticks, stream_file)): Path<(
        String,
        String,
        usize,
        String,
        String,
    )>,
    Query(q): Query<SubtitleQuery>,
) -> Result<Response, (StatusCode, String)> {
    get_subtitle_stream_inner(
        state,
        headers,
        method,
        SubtitleStreamRequest {
            item_id,
            media_source_id,
            index,
            stream_file,
            query: q,
        },
    )
    .await
}

async fn get_subtitle_stream_inner(
    state: Arc<AppState>,
    headers: HeaderMap,
    method: Method,
    request: SubtitleStreamRequest,
) -> Result<Response, (StatusCode, String)> {
    let SubtitleStreamRequest {
        item_id,
        media_source_id,
        index,
        stream_file,
        query,
    } = request;
    let user_id =
        resolve_valid_user_id_with_query(&state, None, &headers, query.api_key.as_deref()).await?;

    let src_id: i64 = media_source_id
        .strip_prefix("src_")
        .unwrap_or(&media_source_id)
        .parse()
        .map_err(|_| (StatusCode::BAD_REQUEST, "无效的媒体源 ID".to_string()))?;

    let row: Option<(i64, i32, i32, Vec<String>)> = sqlx::query_as(
        r#"
        SELECT f.work_id, sf.season_number, sf.episode_number, sf.subtitles
        FROM sync_files sf
        JOIN sync_folders f ON f.id = sf.folder_id
        WHERE sf.id = $1 AND f.work_id IS NOT NULL AND f.is_ignored = FALSE
        "#,
    )
    .bind(src_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some((work_id, season, episode, subtitles)) = row else {
        return Err((StatusCode::NOT_FOUND, "未找到对应的媒体源".to_string()));
    };
    ensure_media_accessible(&state, &user_id, work_id).await?;

    let matches_item = item_id == format!("work_{work_id}")
        || item_id == format!("media_{work_id}")
        || item_id == format!("ep_{work_id}_{season}_{episode}")
        || item_id == format!("src_{src_id}")
        || item_id == src_id.to_string();
    if !matches_item {
        return Err((StatusCode::NOT_FOUND, "媒体源不属于当前条目".into()));
    }
    let sub_path = subtitle_path(&subtitles, index).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "未找到指定索引的外挂字幕".to_string(),
        )
    })?;
    let extension = sub_path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let requested_format = resolve_subtitle_format(&stream_file, &extension)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "字幕格式与原文件不符".into()))?;

    let client_ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let path_hash = xxhash_rust::xxh3::xxh3_64(sub_path.as_bytes()) as i64;
    let resolved = state
        .resolve_playback_url(path_hash, sub_path, client_ua)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("获取字幕直链失败: {:#}", e),
            )
        })?;

    let client = subtitle_http_client()?;
    proxy_subtitle(
        client,
        &resolved.redirect_url,
        client_ua,
        requested_format,
        &extension,
        method == Method::HEAD,
    )
    .await
}

fn resolve_subtitle_format<'a>(stream_file: &str, source_extension: &'a str) -> Option<&'a str> {
    let requested = stream_file.strip_prefix("Stream.")?;
    let requested = requested.to_ascii_lowercase();
    if requested == source_extension || (requested == "vtt" && source_extension == "srt") {
        Some(if requested == "vtt" {
            "vtt"
        } else {
            source_extension
        })
    } else {
        None
    }
}

fn convert_srt_to_vtt(bytes: &[u8]) -> Result<Vec<u8>, (StatusCode, String)> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| (StatusCode::BAD_GATEWAY, "SRT 字幕不是有效 UTF-8".into()))?;
    let text = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n");
    let mut output = String::with_capacity(text.len() + 9);
    output.push_str("WEBVTT\n\n");
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.contains(" --> ") {
            output.push_str(&line.replace(',', "."));
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }
    Ok(output.into_bytes())
}

fn subtitle_path(paths: &[String], index: usize) -> Option<&str> {
    index
        .checked_sub(2)
        .and_then(|offset| paths.get(offset))
        .map(String::as_str)
}

fn validated_subtitle_redirect(
    base: &reqwest::Url,
    location: &str,
) -> Result<reqwest::Url, (StatusCode, String)> {
    let target = base
        .join(location)
        .map_err(|_| (StatusCode::BAD_GATEWAY, "字幕重定向地址无效".to_string()))?;
    if !matches!(target.scheme(), "http" | "https") || target.host_str().is_none() {
        return Err((StatusCode::BAD_GATEWAY, "字幕重定向地址无效".to_string()));
    }
    Ok(target)
}

async fn send_subtitle_request(
    client: &reqwest::Client,
    url: &str,
    user_agent: &str,
    head: bool,
    range_probe: bool,
) -> Result<reqwest::Response, (StatusCode, String)> {
    let mut target = reqwest::Url::parse(url)
        .map_err(|_| (StatusCode::BAD_GATEWAY, "字幕源地址无效".to_string()))?;
    for redirects in 0..=2 {
        if !matches!(target.scheme(), "http" | "https") || target.host_str().is_none() {
            return Err((StatusCode::BAD_GATEWAY, "字幕源地址无效".to_string()));
        }
        let mut request = if head {
            client.head(target.clone())
        } else {
            client.get(target.clone())
        }
        .header(header::USER_AGENT, user_agent);
        if range_probe {
            request = request.header(header::RANGE, "bytes=0-0");
        }
        let response = request
            .send()
            .await
            .map_err(|error| (StatusCode::BAD_GATEWAY, format!("读取字幕失败: {error}")))?;
        if matches!(
            response.status(),
            StatusCode::MOVED_PERMANENTLY
                | StatusCode::FOUND
                | StatusCode::SEE_OTHER
                | StatusCode::TEMPORARY_REDIRECT
                | StatusCode::PERMANENT_REDIRECT
        ) {
            if redirects == 2 {
                return Err((StatusCode::BAD_GATEWAY, "字幕重定向次数超限".into()));
            }
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or((StatusCode::BAD_GATEWAY, "字幕重定向缺少 Location".into()))?;
            target = validated_subtitle_redirect(&target, location)?;
            continue;
        }
        return Ok(response);
    }
    Err((StatusCode::BAD_GATEWAY, "字幕重定向次数超限".into()))
}

async fn proxy_subtitle(
    client: &reqwest::Client,
    url: &str,
    user_agent: &str,
    extension: &str,
    source_extension: &str,
    head: bool,
) -> Result<Response, (StatusCode, String)> {
    let mut upstream = send_subtitle_request(client, url, user_agent, head, false).await?;
    if head && upstream.status() == StatusCode::METHOD_NOT_ALLOWED {
        upstream = send_subtitle_request(client, url, user_agent, false, true).await?;
    }
    if !upstream.status().is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("读取字幕失败: upstream returned {}", upstream.status()),
        ));
    }
    let upstream_len = if head && upstream.status() == StatusCode::PARTIAL_CONTENT {
        upstream
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("bytes "))
            .and_then(|value| value.split_once('/')?.1.parse::<u64>().ok())
            .or_else(|| {
                upstream
                    .headers()
                    .get(header::CONTENT_LENGTH)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<u64>().ok())
                    .or_else(|| upstream.content_length())
            })
    } else {
        upstream
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .or_else(|| upstream.content_length())
    };
    if upstream_len.is_some_and(|len| len > MAX_SUBTITLE_BYTES as u64) {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            "字幕文件超过 16 MiB 限制".into(),
        ));
    }
    let mut body =
        Vec::with_capacity(upstream_len.unwrap_or(0).min(MAX_SUBTITLE_BYTES as u64) as usize);
    if !head {
        while let Some(chunk) = upstream
            .chunk()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?
        {
            if body.len().saturating_add(chunk.len()) > MAX_SUBTITLE_BYTES {
                return Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "字幕文件超过 16 MiB 限制".into(),
                ));
            }
            body.extend_from_slice(&chunk);
        }
    }
    let bytes = if !head && extension == "vtt" && source_extension == "srt" {
        convert_srt_to_vtt(&body)?
    } else {
        body
    };
    let content_type = match extension {
        "vtt" => "text/vtt; charset=utf-8",
        "ass" => "text/x-ass; charset=utf-8",
        "ssa" => "text/x-ssa; charset=utf-8",
        _ => "application/x-subrip; charset=utf-8",
    };
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type);
    let response = if head {
        if extension == source_extension {
            if let Some(len) = upstream_len.filter(|&len| len > 0) {
                response.header(header::CONTENT_LENGTH, len)
            } else {
                response
            }
        } else {
            response
        }
    } else {
        response.header(header::CONTENT_LENGTH, bytes.len())
    };
    response
        .body(if head {
            axum::body::Body::empty()
        } else {
            axum::body::Body::from(bytes)
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[test]
    fn subtitle_redirects_resolve_relative_locations_and_reject_non_http_schemes() {
        let base = reqwest::Url::parse("https://origin.example/path/subtitle.srt").unwrap();
        assert_eq!(
            validated_subtitle_redirect(&base, "../cdn/subtitle.srt")
                .unwrap()
                .as_str(),
            "https://origin.example/cdn/subtitle.srt"
        );
        assert!(validated_subtitle_redirect(&base, "file:///etc/passwd").is_err());
    }

    #[tokio::test]
    async fn subtitle_stream_routes_are_mounted_and_require_authentication() {
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://media_admin:pw@127.0.0.1:5432/rx_emby")
            .unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let state = AppState::new(
            crate::config::BootstrapFileConfig::default().into_dynamic(),
            db,
            tx,
        )
        .unwrap();

        for path in [
            "/emby/Videos/work_1/src_1/Subtitles/2/Stream.vtt",
            "/Videos/work_1/src_1/Subtitles/2/0/Stream.vtt",
            "/emby/Items/work_1/src_1/Subtitles/2/0/Stream.srt",
        ] {
            let request = Request::builder().uri(path).body(Body::empty()).unwrap();
            let response = crate::web::build_router(state.clone())
                .oneshot(request)
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        }
    }

    #[tokio::test]
    async fn subtitle_proxy_returns_bytes_and_head_metadata() {
        use axum::{
            body::Body,
            routing::{get, head},
            Router,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route(
                        "/subtitle",
                        get(|| async { "WEBVTT\n\n00:00.000 --> 00:01.000\n字幕\n" }).merge(head(
                            || async { ([(header::CONTENT_LENGTH, "39")], Body::empty()) },
                        )),
                    )
                    .route(
                        "/subtitle.srt",
                        get(|| async { "1\n00:00:01,200 --> 00:00:02,300\nHi\n" }),
                    ),
            )
            .await
            .unwrap();
        });
        let client = reqwest::Client::new();
        let url = format!("http://{addr}/subtitle");
        let response = proxy_subtitle(&client, &url, "client", "vtt", "vtt", false)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/vtt; charset=utf-8"
        );
        let length = response.headers()[header::CONTENT_LENGTH].clone();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            &bytes[..],
            "WEBVTT\n\n00:00.000 --> 00:01.000\n字幕\n".as_bytes()
        );
        let head = proxy_subtitle(&client, &url, "client", "vtt", "vtt", true)
            .await
            .unwrap();
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(head.headers()[header::CONTENT_LENGTH], length);
        assert!(axum::body::to_bytes(head.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty());
        let srt_url = format!("http://{addr}/subtitle.srt");
        let converted = proxy_subtitle(&client, &srt_url, "client", "vtt", "srt", false)
            .await
            .unwrap();
        let converted_bytes = axum::body::to_bytes(converted.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            &converted_bytes[..],
            b"WEBVTT\n\n1\n00:00:01.200 --> 00:00:02.300\nHi\n"
        );
        server.abort();
    }

    #[test]
    fn srt_conversion_and_format_resolution_support_vtt_delivery() {
        let converted =
            convert_srt_to_vtt(b"\xEF\xBB\xBF1\r\n00:00:01,200 --> 00:00:02,300\r\nHi\r\n")
                .unwrap();
        assert_eq!(
            String::from_utf8(converted).unwrap(),
            "WEBVTT\n\n1\n00:00:01.200 --> 00:00:02.300\nHi\n"
        );
        assert_eq!(resolve_subtitle_format("Stream.vtt", "srt"), Some("vtt"));
        assert_eq!(resolve_subtitle_format("Stream.srt", "srt"), Some("srt"));
        assert_eq!(resolve_subtitle_format("Stream.vtt", "ass"), None);
    }

    #[test]
    fn subtitle_stream_indices_reserve_video_and_audio() {
        let paths = vec![
            "a.srt".to_string(),
            "b.ass".to_string(),
            "c.vtt".to_string(),
        ];
        assert_eq!(subtitle_path(&paths, 2), Some("a.srt"));
        assert_eq!(subtitle_path(&paths, 4), Some("c.vtt"));
        assert_eq!(subtitle_path(&paths, 0), None);
        assert_eq!(subtitle_path(&paths, 5), None);
    }
}
