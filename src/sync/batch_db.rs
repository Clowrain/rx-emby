use crate::sync::parser::ParsedStrmRecord;
use anyhow::{Context, Result};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use tracing::info;

#[derive(Debug, Default, Clone)]
pub struct SyncDiffStats {
    pub total_lines: i32,
    pub added_episodes: i32,
    pub deleted_episodes: i32,
    pub new_media_items: i32,
}

/// 内存级去重与同源重复目录收敛（拦截完全重复 path_hash 以及同剧集下末级文件名相同的重复挂载目录）
pub fn deduplicate_records(records: Vec<ParsedStrmRecord>) -> Vec<ParsedStrmRecord> {
    let mut seen_hashes: HashSet<i64> = HashSet::with_capacity(records.len());
    // Key = (root_dir, season_number, episode_number, file_name) -> 保留优先级最高的那一条
    let mut seen_same_file: HashMap<(String, i32, i32, String), usize> =
        HashMap::with_capacity(records.len());
    let mut result: Vec<ParsedStrmRecord> = Vec::with_capacity(records.len());

    for rec in records {
        // 1. 物理路径绝对唯一去重
        if !seen_hashes.insert(rec.path_hash) {
            continue;
        }

        // 2. 同剧集同集数下末级文件名完全一致（多目录重复映射），保留目录更规范或评分更高的条目
        let ep_file_key = (
            rec.root_dir.clone(),
            rec.season_number,
            rec.episode_number,
            rec.file_name.clone(),
        );

        if let Some(&existing_idx) = seen_same_file.get(&ep_file_key) {
            if rec.priority_score > result[existing_idx].priority_score {
                result[existing_idx] = rec;
            }
            continue;
        }

        seen_same_file.insert(ep_file_key, result.len());
        result.push(rec);
    }

    result
}

/// 执行内存 Diff 并使用 PostgreSQL UNNEST 批量入库
pub async fn apply_sync_diff(
    db: &PgPool,
    records: Vec<ParsedStrmRecord>,
    total_lines: i32,
    batch_size: usize,
) -> Result<SyncDiffStats> {
    // 1. 拉取数据库中现有的全部 path_hash 做内存 Diff
    let existing_hashes: Vec<i64> = sqlx::query_scalar("SELECT path_hash FROM episodes")
        .fetch_all(db)
        .await
        .context("查询现有 episodes path_hash 失败")?;

    let existing_set: HashSet<i64> = existing_hashes.into_iter().collect();
    let current_set: HashSet<i64> = records.iter().map(|r| r.path_hash).collect();

    // 筛选出新增条目与失效待清理条目
    let to_insert: Vec<ParsedStrmRecord> = records
        .into_iter()
        .filter(|r| !existing_set.contains(&r.path_hash))
        .collect();

    let to_delete: Vec<i64> = existing_set
        .difference(&current_set)
        .copied()
        .collect();

    let added_episodes = to_insert.len() as i32;
    let deleted_episodes = to_delete.len() as i32;

    if to_insert.is_empty() && to_delete.is_empty() {
        info!("增量 Diff 完成：无新增或失效资源");
        return Ok(SyncDiffStats {
            total_lines,
            added_episodes: 0,
            deleted_episodes: 0,
            new_media_items: 0,
        });
    }

    let mut tx = db.begin().await.context("开启增量同步事务失败")?;

    // 2. 批量清理已失效的剧集物理源及孤儿 seasons / media_items
    if !to_delete.is_empty() {
        for chunk in to_delete.chunks(batch_size.max(1000)) {
            sqlx::query("DELETE FROM episodes WHERE path_hash = ANY($1)")
                .bind(chunk)
                .execute(&mut *tx)
                .await
                .context("批量删除失效 episodes 失败")?;
        }

        // 清理已没有任何分集的空季与空影视主条目，避免残留幽灵卡片与无效刮削
        sqlx::query(
            r#"
            DELETE FROM seasons s
            WHERE NOT EXISTS (
                SELECT 1 FROM episodes e
                WHERE e.media_item_id = s.media_item_id AND e.season_number = s.season_number
            )
            "#,
        )
        .execute(&mut *tx)
        .await
        .context("清理孤儿 seasons 失败")?;

        sqlx::query(
            r#"
            DELETE FROM media_items m
            WHERE NOT EXISTS (
                SELECT 1 FROM episodes e WHERE e.media_item_id = m.id
            )
            "#,
        )
        .execute(&mut *tx)
        .await
        .context("清理孤儿 media_items 失败")?;
    }

    // 3. 批量 upsert media_items（按 root_dir 聚合去重）
    let mut media_map: HashMap<String, &ParsedStrmRecord> = HashMap::new();
    for rec in &to_insert {
        media_map.entry(rec.root_dir.clone()).or_insert(rec);
    }

    let unique_media: Vec<&ParsedStrmRecord> = media_map.into_values().collect();
    let mut new_media_items = 0_i32;

    for chunk in unique_media.chunks(batch_size.max(1000)) {
        let root_dirs: Vec<String> = chunk.iter().map(|r| r.root_dir.clone()).collect();
        let raw_folders: Vec<String> = chunk.iter().map(|r| r.raw_folder_name.clone()).collect();
        let titles: Vec<String> = chunk.iter().map(|r| r.clean_title.clone()).collect();
        let media_types: Vec<String> = chunk.iter().map(|r| r.inferred_media_type.clone()).collect();
        let categories: Vec<String> = chunk.iter().map(|r| r.inferred_category.clone()).collect();
        let years: Vec<Option<i32>> = chunk.iter().map(|r| r.year).collect();
        let tmdb_ids: Vec<Option<i32>> = chunk.iter().map(|r| r.tmdb_id).collect();
        // 近两年新片设为 P1 高优刮削，其余存量为 P3
        let priorities: Vec<i16> = chunk
            .iter()
            .map(|r| if r.year.unwrap_or(0) >= 2025 { 1 } else { 3 })
            .collect();

        let inserted_count: i64 = sqlx::query_scalar(
            r#"
            WITH upserted AS (
                INSERT INTO media_items (
                    root_dir, raw_folder_name, title, media_type, category, year, tmdb_id, scrape_priority
                )
                SELECT * FROM UNNEST(
                    $1::text[], $2::text[], $3::text[], $4::varchar[], $5::varchar[], $6::int4[], $7::int4[], $8::int2[]
                )
                ON CONFLICT (root_dir) DO UPDATE SET
                    tmdb_id = COALESCE(media_items.tmdb_id, EXCLUDED.tmdb_id),
                    updated_at = NOW()
                RETURNING (xmax = 0) AS is_new
            )
            SELECT COUNT(*) FROM upserted WHERE is_new = TRUE
            "#,
        )
        .bind(&root_dirs)
        .bind(&raw_folders)
        .bind(&titles)
        .bind(&media_types)
        .bind(&categories)
        .bind(&years)
        .bind(&tmdb_ids)
        .bind(&priorities)
        .fetch_one(&mut *tx)
        .await
        .context("UNNEST 批量写入 media_items 失败")?;

        new_media_items += inserted_count as i32;
    }

    // 4. 查询涉及到的 root_dir -> media_item_id 映射
    let target_roots: Vec<String> = to_insert
        .iter()
        .map(|r| r.root_dir.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let id_rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT root_dir, id FROM media_items WHERE root_dir = ANY($1)")
            .bind(&target_roots)
            .fetch_all(&mut *tx)
            .await
            .context("查询 media_items ID 映射失败")?;

    let root_to_id: HashMap<String, i64> = id_rows.into_iter().collect();

    // 5. 批量创建电视剧 seasons 记录
    let mut season_pairs: HashSet<(i64, i32)> = HashSet::new();
    for rec in &to_insert {
        if rec.inferred_media_type == "tv" {
            if let Some(&mid) = root_to_id.get(&rec.root_dir) {
                season_pairs.insert((mid, rec.season_number));
            }
        }
    }

    if !season_pairs.is_empty() {
        let pairs_vec: Vec<(i64, i32)> = season_pairs.into_iter().collect();
        for chunk in pairs_vec.chunks(batch_size.max(1000)) {
            let m_ids: Vec<i64> = chunk.iter().map(|p| p.0).collect();
            let s_nums: Vec<i32> = chunk.iter().map(|p| p.1).collect();
            let s_names: Vec<String> = chunk
                .iter()
                .map(|p| {
                    if p.1 == 0 {
                        "特别篇".to_string()
                    } else {
                        format!("第 {} 季", p.1)
                    }
                })
                .collect();

            sqlx::query(
                r#"
                INSERT INTO seasons (media_item_id, season_number, name, scrape_attempts)
                SELECT *, 0 FROM UNNEST($1::int8[], $2::int4[], $3::varchar[])
                ON CONFLICT (media_item_id, season_number) DO UPDATE SET
                    scrape_attempts = 0
                "#,
            )
            .bind(&m_ids)
            .bind(&s_nums)
            .bind(&s_names)
            .execute(&mut *tx)
            .await
            .context("UNNEST 批量写入 seasons 失败")?;
        }
    }

    // 拉取 (media_item_id, season_number) -> season_id 映射
    let target_mids: Vec<i64> = root_to_id.values().copied().collect();
    let season_rows: Vec<(i64, i32, i64)> = sqlx::query_as(
        "SELECT media_item_id, season_number, id FROM seasons WHERE media_item_id = ANY($1)",
    )
    .bind(&target_mids)
    .fetch_all(&mut *tx)
    .await
    .context("查询 seasons ID 映射失败")?;

    let season_to_id: HashMap<(i64, i32), i64> = season_rows
        .into_iter()
        .map(|(mid, snum, sid)| ((mid, snum), sid))
        .collect();

    // 6. UNNEST 批量写入 episodes 物理源（包含多版本标签与权重）
    for chunk in to_insert.chunks(batch_size.max(1000)) {
        let mut m_ids = Vec::with_capacity(chunk.len());
        let mut s_ids = Vec::with_capacity(chunk.len());
        let mut s_nums = Vec::with_capacity(chunk.len());
        let mut e_nums = Vec::with_capacity(chunk.len());
        let mut titles = Vec::with_capacity(chunk.len());
        let mut v_tags = Vec::with_capacity(chunk.len());
        let mut p_scores = Vec::with_capacity(chunk.len());
        let mut a_paths = Vec::with_capacity(chunk.len());
        let mut p_hashes = Vec::with_capacity(chunk.len());
        let mut f_names = Vec::with_capacity(chunk.len());
        let mut containers = Vec::with_capacity(chunk.len());
        let mut incompletes = Vec::with_capacity(chunk.len());

        for rec in chunk {
            let Some(&mid) = root_to_id.get(&rec.root_dir) else {
                continue;
            };
            let sid = season_to_id.get(&(mid, rec.season_number)).copied();
            let is_tv = rec.inferred_media_type != "movie";
            let ep_title = if !is_tv {
                rec.clean_title.clone()
            } else {
                format!("第 {} 集", rec.episode_number)
            };

            m_ids.push(mid);
            s_ids.push(sid);
            s_nums.push(rec.season_number);
            e_nums.push(rec.episode_number);
            titles.push(ep_title);
            v_tags.push(rec.version_tag.clone());
            p_scores.push(rec.priority_score);
            a_paths.push(rec.alist_path.clone());
            p_hashes.push(rec.path_hash);
            f_names.push(rec.file_name.clone());
            containers.push(rec.container.clone());
            incompletes.push(is_tv);
        }

        if m_ids.is_empty() {
            continue;
        }

        sqlx::query(
            r#"
            INSERT INTO episodes (
                media_item_id, season_id, season_number, episode_number,
                title, version_tag, priority_score,
                alist_path, path_hash, file_name, container, meta_incomplete
            )
            SELECT * FROM UNNEST(
                $1::int8[], $2::int8[], $3::int4[], $4::int4[],
                $5::varchar[], $6::varchar[], $7::int4[],
                $8::text[], $9::int8[], $10::varchar[], $11::varchar[], $12::bool[]
            )
            ON CONFLICT (path_hash) DO UPDATE SET
                version_tag = EXCLUDED.version_tag,
                priority_score = EXCLUDED.priority_score,
                updated_at = NOW()
            "#,
        )
        .bind(&m_ids)
        .bind(&s_ids)
        .bind(&s_nums)
        .bind(&e_nums)
        .bind(&titles)
        .bind(&v_tags)
        .bind(&p_scores)
        .bind(&a_paths)
        .bind(&p_hashes)
        .bind(&f_names)
        .bind(&containers)
        .bind(&incompletes)
        .execute(&mut *tx)
        .await
        .context("UNNEST 批量写入 episodes 失败")?;
    }

    tx.commit().await.context("提交增量同步事务失败")?;
    Ok(SyncDiffStats {
        total_lines,
        added_episodes,
        deleted_episodes,
        new_media_items,
    })
}
