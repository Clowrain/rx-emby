pub mod tmdb;

use crate::state::AppState;
use anyhow::Result;
use chrono::{Datelike, Duration as ChronoDuration, NaiveDate, Utc};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::{sync::mpsc, time::sleep};
use tracing::{debug, error, info};

#[derive(sqlx::FromRow, Debug)]
struct PendingMediaRow {
    id: i64,
    media_type: String,
    tmdb_id: Option<i32>,
    title: String,
    year: Option<i32>,
    retry_count: i16,
    scrape_status: i16,
    is_locked: bool,
}

/// 启动后台四级优先级智能刮削协程
pub fn spawn_scrape_worker(state: Arc<AppState>, mut priority_rx: mpsc::Receiver<i64>) {
    tokio::spawn(async move {
        info!("TMDB 智能刮削调度器已启动（支持 P0 实时插队、分集刮削与追更自动补齐）");

        loop {
            tokio::select! {
                _ = state.shutdown_token.cancelled() => {
                    info!("收到停机信号，刮削调度器退出");
                    break;
                }
                // P0 实时插队通道（前端浏览或点开详情/季列表触发）
                Some(urgent_media_id) = priority_rx.recv() => {
                    if let Err(e) = scrape_single_media_with_mode(&state, urgent_media_id, true).await {
                        debug!("P0 实时刮削条目 {} 失败: {:#}", urgent_media_id, e);
                    }
                }
                // 常规队列轮询（按 P1 -> P2 -> P3 顺序消化主条目与待补齐分集）
                _ = sleep(Duration::from_millis(200)) => {
                    match fetch_next_pending_id(&state).await {
                        Ok(Some(next_id)) => {
                            if let Err(e) = scrape_single_media_with_mode(&state, next_id, false).await {
                                debug!("常规队列刮削条目 {} 失败: {:#}", next_id, e);
                            }
                        }
                        Ok(None) => {
                            // 队列空闲时休眠 3 秒避免空转查询 PG
                            sleep(Duration::from_secs(3)).await;
                        }
                        Err(e) => {
                            error!("查询待刮削队列异常: {:#}", e);
                            sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
            }
        }
    });
}

async fn fetch_next_pending_id(state: &AppState) -> Result<Option<i64>> {
    let cfg = state.config.load();
    // 若未开启后台全库自动刮削，或未配置 API Key，则静默等待 P0 实时插队请求
    if !cfg.scraper_auto_background || cfg.tmdb_api_key == "your_tmdb_api_key" {
        return Ok(None);
    }

    // 1. 优先消化尚未完成主条目刮削的影视
    let id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM media_items
        WHERE is_ignored = FALSE
          AND is_locked = FALSE
          AND (
              scrape_status = 0
              OR (scrape_status = 2 AND retry_count < 3 AND (next_retry_at IS NULL OR next_retry_at <= NOW()))
          )
        ORDER BY scrape_priority ASC, id DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(&state.db)
    .await?;

    if id.is_some() {
        return Ok(id);
    }

    // 2. 消化已刮削剧集中存在“未刮削新季/新集 (episode_count = 0)”或“连载残缺集已过 24h 冷却期 (meta_incomplete = TRUE)”的条目
    let season_pending_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT m.id
        FROM media_items m
        WHERE m.is_ignored = FALSE
          AND m.media_type = 'tv'
          AND m.tmdb_id IS NOT NULL
          AND m.scrape_status IN (1, 3)
          AND (
              EXISTS (
                  SELECT 1 FROM seasons s
                  WHERE s.media_item_id = m.id AND s.episode_count = 0
              )
              OR EXISTS (
                  SELECT 1 FROM seasons s
                  JOIN episodes e ON e.media_item_id = s.media_item_id AND e.season_number = s.season_number
                  WHERE s.media_item_id = m.id
                    AND s.episode_count BETWEEN 1 AND 4
                    AND e.meta_incomplete = TRUE
                    AND e.updated_at <= NOW() - INTERVAL '24 hours'
              )
              OR EXISTS (
                  SELECT 1 FROM episodes e
                  WHERE e.media_item_id = m.id
                    AND NOT EXISTS (
                        SELECT 1 FROM seasons s
                        WHERE s.media_item_id = m.id AND s.season_number = e.season_number
                    )
              )
          )
        ORDER BY m.scrape_priority ASC, m.id DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(&state.db)
    .await?;

    Ok(season_pending_id)
}

pub async fn scrape_single_media(state: &AppState, media_id: i64) -> Result<()> {
    scrape_single_media_with_mode(state, media_id, true).await
}

pub async fn scrape_single_media_with_mode(
    state: &AppState,
    media_id: i64,
    is_p0: bool,
) -> Result<()> {
    let row: Option<PendingMediaRow> = sqlx::query_as(
        "SELECT id, media_type, tmdb_id, title, year, retry_count, scrape_status, is_locked FROM media_items WHERE id = $1",
    )
    .bind(media_id)
    .fetch_optional(&state.db)
    .await?;

    let Some(item) = row else {
        return Ok(());
    };

    // 若主条目已刮削完成（scrape_status 为 1 或 3），直接检查并刮削其下待更新的季与分集
    if item.scrape_status == 1 || item.scrape_status == 3 {
        if item.media_type == "tv" {
            if let Some(tid) = item.tmdb_id {
                scrape_tv_seasons(state, item.id, tid, item.year, is_p0).await?;
            }
        }
        return Ok(());
    }

    if item.is_locked {
        return Ok(());
    }

    // 优化：若当前条目已带 tmdb_id，且库中已有其它同 (tmdb_id, media_type) 的已刮削兄弟目录，
    // 直接零网络开销复用本地元数据，省去重复请求 TMDB API
    if let Some(tid) = item.tmdb_id {
        let reused = sqlx::query(
            r#"
            UPDATE media_items AS target SET
                title = src.title,
                original_title = src.original_title,
                year = COALESCE(src.year, target.year),
                premiered_at = src.premiered_at,
                overview = src.overview,
                rating = src.rating,
                poster_path = src.poster_path,
                backdrop_path = src.backdrop_path,
                countries = src.countries,
                networks = src.networks,
                genres = src.genres,
                category = src.category,
                scrape_status = 1,
                updated_at = NOW()
            FROM (
                SELECT title, original_title, year, premiered_at, overview, rating,
                       poster_path, backdrop_path, countries, networks, genres, category
                FROM media_items
                WHERE tmdb_id = $1 AND media_type = $2 AND scrape_status IN (1, 3) AND id != $3
                LIMIT 1
            ) AS src
            WHERE target.id = $3
            "#,
        )
        .bind(tid)
        .bind(&item.media_type)
        .bind(item.id)
        .execute(&state.db)
        .await?
        .rows_affected();

        if reused > 0 {
            debug!(
                "♻️ [Scraper] media_id={} (tmdb_id={}) 命中本地兄弟目录元数据缓存，跳过主条目远端请求",
                item.id, tid
            );
            if item.media_type == "tv" {
                scrape_tv_seasons(state, item.id, tid, item.year, is_p0).await?;
            }
            return Ok(());
        }
    }

    match state
        .scrape_tmdb_metadata(item.tmdb_id, &item.media_type, &item.title, item.year)
        .await
    {
        Ok(meta) => {
            // 同时更新当前条目以及库中具有相同 (tmdb_id, media_type) 且尚未刮削的兄弟目录
            sqlx::query(
                r#"
                UPDATE media_items SET
                    media_type = $1, tmdb_id = $2, title = $3, original_title = $4,
                    year = COALESCE($5, year), premiered_at = $6, overview = $7,
                    rating = $8, poster_path = $9, backdrop_path = $10,
                    countries = $11, networks = $12, genres = $13, category = $14,
                    scrape_status = 1, updated_at = NOW()
                WHERE id = $15
                   OR (tmdb_id = $2 AND media_type = $1 AND scrape_status = 0 AND is_locked = FALSE)
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
            .bind(item.id)
            .execute(&state.db)
            .await?;

            // 主条目刮削成功后，若为剧集则继续刮削其下所有季与单集元数据
            if meta.media_type == "tv" {
                if let Err(e) =
                    scrape_tv_seasons(state, item.id, meta.tmdb_id, meta.year.or(item.year), is_p0)
                        .await
                {
                    debug!(
                        "剧集 media_id={} (tmdb_id={}) 分集元数据刮削部分异常: {:#}",
                        item.id, meta.tmdb_id, e
                    );
                }
            }
        }
        Err(e) => {
            // 指数退避重试：第1次 24小时，第2次 3天，第3次 7天
            let next_retry = item.retry_count + 1;
            let backoff_hours = match next_retry {
                1 => 24,
                2 => 72,
                _ => 168,
            };
            let next_at = Utc::now() + ChronoDuration::hours(backoff_hours);

            sqlx::query(
                r#"
                UPDATE media_items SET
                    scrape_status = 2, retry_count = $1, next_retry_at = $2, updated_at = NOW()
                WHERE id = $3
                "#,
            )
            .bind(next_retry)
            .bind(next_at)
            .bind(item.id)
            .execute(&state.db)
            .await?;

            return Err(e);
        }
    }

    Ok(())
}

/// 刮削电视剧的季与单集元数据（支持多目录共享复用、增量新集立即刮削、未完结残缺新集冷却重刷）
pub async fn scrape_tv_seasons(
    state: &AppState,
    media_id: i64,
    tmdb_id: i32,
    series_year: Option<i32>,
    is_p0: bool,
) -> Result<()> {
    let sibling_ids = state.resolve_sibling_media_ids(media_id).await;

    // 1. 确保所有包含单集的季都在 seasons 表中有对应记录（兼容原本被推断为 movie 后修正为 tv 的条目）
    sqlx::query(
        r#"
        INSERT INTO seasons (media_item_id, season_number, name, episode_count)
        SELECT DISTINCT
            e.media_item_id,
            e.season_number,
            CASE WHEN e.season_number = 0 THEN '特别篇' ELSE '第 ' || e.season_number || ' 季' END,
            0
        FROM episodes e
        WHERE e.media_item_id = ANY($1)
        ON CONFLICT (media_item_id, season_number) DO NOTHING
        "#,
    )
    .bind(&sibling_ids)
    .execute(&state.db)
    .await?;

    // 2. 先在同 tmdb_id 兄弟目录之间横向复用已刮削的季与单集元数据（如 1080P 已刮削，新增 4K 目录同集直接零网络开销复用）
    sqlx::query(
        r#"
        UPDATE episodes AS target SET
            title = src.title,
            overview = src.overview,
            still_path = src.still_path,
            air_date = src.air_date,
            runtime = CASE WHEN src.runtime > 0 THEN src.runtime ELSE target.runtime END,
            meta_incomplete = src.meta_incomplete,
            updated_at = NOW()
        FROM (
            SELECT DISTINCT ON (season_number, episode_number)
                season_number, episode_number, title, overview, still_path, air_date, runtime, meta_incomplete
            FROM episodes
            WHERE media_item_id = ANY($1)
              AND (still_path IS NOT NULL OR overview IS NOT NULL)
            ORDER BY season_number, episode_number, (still_path IS NOT NULL AND overview IS NOT NULL) DESC, updated_at DESC
        ) AS src
        WHERE target.media_item_id = ANY($1)
          AND target.season_number = src.season_number
          AND target.episode_number = src.episode_number
          AND target.still_path IS NULL
          AND target.overview IS NULL
        "#,
    )
    .bind(&sibling_ids)
    .execute(&state.db)
    .await?;

    // 若兄弟目录某季所有分集均已通过横向复用补齐（不存在未刮削的新集），同步复用 seasons 表元数据与状态
    sqlx::query(
        r#"
        UPDATE seasons AS target SET
            name = COALESCE(src.name, target.name),
            overview = COALESCE(src.overview, target.overview),
            poster_path = COALESCE(src.poster_path, target.poster_path),
            air_date = COALESCE(src.air_date, target.air_date),
            episode_count = src.episode_count
        FROM (
            SELECT DISTINCT ON (season_number)
                season_number, name, overview, poster_path, air_date, episode_count
            FROM seasons
            WHERE media_item_id = ANY($1) AND episode_count > 0
            ORDER BY season_number, episode_count DESC
        ) AS src
        WHERE target.media_item_id = ANY($1)
          AND target.season_number = src.season_number
          AND target.episode_count = 0
          AND NOT EXISTS (
              SELECT 1 FROM episodes e
              WHERE e.media_item_id = target.media_item_id
                AND e.season_number = target.season_number
                AND e.still_path IS NULL
                AND e.overview IS NULL
          )
        "#,
    )
    .bind(&sibling_ids)
    .execute(&state.db)
    .await?;

    // 3. 查询仍需向 TMDB 请求的季列表：
    //    - episode_count = 0：首次刮削或增量同步刚入库了新集，立即刮削（无冷却等待）
    //    - episode_count IN 1..4 且存在 meta_incomplete = TRUE 的分集已过冷却期（P0 点开 12h，常规后台 24h）
    let cooldown_hours = if is_p0 { 12 } else { 24 };
    let cutoff = Utc::now() - ChronoDuration::hours(cooldown_hours);

    let pending_seasons: Vec<(i32, i32)> = sqlx::query_as(
        r#"
        SELECT
            s.season_number,
            CASE WHEN MIN(s.episode_count) = 0 THEN 0 ELSE MAX(s.episode_count) END AS base_attempts
        FROM seasons s
        WHERE s.media_item_id = ANY($1)
          AND s.episode_count >= 0
          AND (
              s.episode_count = 0
              OR (
                  s.episode_count BETWEEN 1 AND 4
                  AND EXISTS (
                      SELECT 1 FROM episodes e
                      WHERE e.media_item_id = ANY($1)
                        AND e.season_number = s.season_number
                        AND e.meta_incomplete = TRUE
                        AND e.updated_at <= $2
                  )
              )
          )
        GROUP BY s.season_number
        ORDER BY s.season_number ASC
        "#,
    )
    .bind(&sibling_ids)
    .bind(cutoff)
    .fetch_all(&state.db)
    .await?;

    if pending_seasons.is_empty() {
        return Ok(());
    }

    let current_year = Utc::now().year();

    for (season_num, max_attempts) in pending_seasons {
        let next_attempt = (max_attempts.max(0) + 1).min(5);
        match state.fetch_tmdb_season(tmdb_id, season_num).await {
            Ok(Some(season_meta)) => {
                // 更新 seasons 表（名称、简介、季海报、开播日期、刮削次数状态）
                sqlx::query(
                    r#"
                    UPDATE seasons SET
                        name = COALESCE($1, name),
                        overview = COALESCE($2, overview),
                        poster_path = COALESCE($3, poster_path),
                        air_date = COALESCE($4, air_date),
                        episode_count = $5
                    WHERE media_item_id = ANY($6) AND season_number = $7
                    "#,
                )
                .bind(&season_meta.name)
                .bind(&season_meta.overview)
                .bind(&season_meta.poster_path)
                .bind(&season_meta.air_date)
                .bind(next_attempt)
                .bind(&sibling_ids)
                .bind(season_num)
                .execute(&state.db)
                .await?;

                // 判定该季是否属于正在连载/近期热播季（任意一集播出时间在最近 35 天内，或尚无播出日期但剧集年份为近两年）
                let has_any_air_date = season_meta.episodes.iter().any(|e| e.air_date.is_some());
                let season_is_ongoing = season_meta
                    .episodes
                    .iter()
                    .any(|e| is_air_date_recent(e.air_date.as_deref(), 35))
                    || (!has_any_air_date
                        && series_year.map(|y| y >= current_year - 1).unwrap_or(false));

                let mut ep_nums = Vec::with_capacity(season_meta.episodes.len());
                let mut names = Vec::with_capacity(season_meta.episodes.len());
                let mut overviews = Vec::with_capacity(season_meta.episodes.len());
                let mut still_paths = Vec::with_capacity(season_meta.episodes.len());
                let mut air_dates = Vec::with_capacity(season_meta.episodes.len());
                let mut runtimes = Vec::with_capacity(season_meta.episodes.len());
                let mut incompletes = Vec::with_capacity(season_meta.episodes.len());
                let mut matched_ep_set = HashSet::with_capacity(season_meta.episodes.len());

                for ep in season_meta.episodes {
                    matched_ep_set.insert(ep.episode_number);
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

                    // 若单集缺少剧照/简介/正式标题，且属于近期连载剧集且重试未满 5 次，则保留 meta_incomplete = TRUE 以便后续自动补齐
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
                        UPDATE episodes AS e SET
                            title = COALESCE(src.name, e.title),
                            overview = COALESCE(src.overview, e.overview),
                            still_path = COALESCE(src.still_path, e.still_path),
                            air_date = COALESCE(src.air_date, e.air_date),
                            runtime = CASE WHEN src.runtime > 0 THEN src.runtime ELSE e.runtime END,
                            meta_incomplete = src.incomplete,
                            updated_at = NOW()
                        FROM UNNEST(
                            $1::int4[], $2::varchar[], $3::text[], $4::varchar[], $5::varchar[], $6::int4[], $7::bool[]
                        ) AS src(episode_number, name, overview, still_path, air_date, runtime, incomplete)
                        WHERE e.media_item_id = ANY($8)
                          AND e.season_number = $9
                          AND e.episode_number = src.episode_number
                        "#,
                    )
                    .bind(&ep_nums)
                    .bind(&names)
                    .bind(&overviews)
                    .bind(&still_paths)
                    .bind(&air_dates)
                    .bind(&runtimes)
                    .bind(&incompletes)
                    .bind(&sibling_ids)
                    .bind(season_num)
                    .execute(&state.db)
                    .await?;
                }

                // 对于本地已入库但 TMDB 季列表尚未收录的超前分集（如刚更新的第 N 集）：
                // 若该季仍在连载且未满 5 次重试则保持 meta_incomplete = TRUE 并刷新 updated_at 进入冷却；否则置为 FALSE
                let unindexed_incomplete = season_is_ongoing && next_attempt < 5;
                sqlx::query(
                    r#"
                    UPDATE episodes SET
                        meta_incomplete = $1,
                        updated_at = NOW()
                    WHERE media_item_id = ANY($2)
                      AND season_number = $3
                      AND NOT (episode_number = ANY($4))
                    "#,
                )
                .bind(unindexed_incomplete)
                .bind(&sibling_ids)
                .bind(season_num)
                .bind(&ep_nums)
                .execute(&state.db)
                .await?;

                info!(
                    "📺 [SeasonScraper] 剧集 tmdb_id={} S{:02} 分集刮削完成 (TMDB集数={}, 尝试次数={}/5, 连载跟进={})",
                    tmdb_id,
                    season_num,
                    ep_nums.len(),
                    next_attempt,
                    season_is_ongoing
                );
            }
            Ok(None) => {
                // TMDB 返回 404，标记 episode_count = -1 避免后台重复请求不存在的季
                sqlx::query(
                    "UPDATE seasons SET episode_count = -1 WHERE media_item_id = ANY($1) AND season_number = $2",
                )
                .bind(&sibling_ids)
                .bind(season_num)
                .execute(&state.db)
                .await?;

                sqlx::query(
                    "UPDATE episodes SET meta_incomplete = FALSE, updated_at = NOW() WHERE media_item_id = ANY($1) AND season_number = $2",
                )
                .bind(&sibling_ids)
                .bind(season_num)
                .execute(&state.db)
                .await?;
            }
            Err(e) => {
                debug!(
                    "拉取 TMDB 剧集季失败 (tmdb_id={}, S{:02}): {:#}",
                    tmdb_id, season_num, e
                );
                // 将该季分集 updated_at 推后，避免网络波动时高频死循环重试
                sqlx::query(
                    "UPDATE episodes SET updated_at = NOW() WHERE media_item_id = ANY($1) AND season_number = $2",
                )
                .bind(&sibling_ids)
                .bind(season_num)
                .execute(&state.db)
                .await?;
            }
        }
    }

    Ok(())
}

/// 判断单集播出日期是否在最近 `within_days` 天内（含未来待播日期）
fn is_air_date_recent(air_date: Option<&str>, within_days: i64) -> bool {
    let Some(raw) = air_date else {
        return false;
    };
    let Ok(date) = NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d") else {
        return false;
    };
    let today = Utc::now().date_naive();
    let days_since = (today - date).num_days();
    days_since <= within_days
}

/// 判断单集标题是否仍为 TMDB 默认占位标题（如 "第 8 集"、"Episode 8"）
fn is_placeholder_episode_title(name: &str, ep_num: i32) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return true;
    }
    let cn_default_no_space = format!("第{}集", ep_num);
    let cn_default_with_space = format!("第 {} 集", ep_num);
    let en_default = format!("episode {}", ep_num);
    trimmed == cn_default_no_space
        || trimmed == cn_default_with_space
        || trimmed.eq_ignore_ascii_case(&en_default)
}
