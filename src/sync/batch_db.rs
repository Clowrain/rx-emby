use crate::sync::parser::ParsedStrmRecord;
use anyhow::{anyhow, Context, Result};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use tracing::info;

#[derive(Debug, Default, Clone)]
pub struct SyncDiffStats {
    pub total_lines: i32,
    pub added_episodes: i32,
    pub deleted_episodes: i32,
    pub new_media_items: i32,
    pub updated_folder_ids: Vec<i64>,
}

fn record_rank(r: &ParsedStrmRecord) -> (bool, bool, i32) {
    (r.tmdb_id.is_some(), !r.has_noise_prefix, r.priority_score)
}

/// 内存级去重与同源重复目录收敛：
/// 1. 相同物理 path_hash 择优保留（优先带 tmdb_id、非 刮削失败/ 前缀、评分更高）
/// 2. 同逻辑目录同集数下末级文件名一致时保留高权重版本
pub fn deduplicate_records(records: Vec<ParsedStrmRecord>) -> Vec<ParsedStrmRecord> {
    let mut seen_hashes: HashMap<i64, usize> = HashMap::with_capacity(records.len());
    let mut seen_same_file: HashMap<(String, i32, i32, String), usize> =
        HashMap::with_capacity(records.len());
    let mut result: Vec<ParsedStrmRecord> = Vec::with_capacity(records.len());

    for rec in records {
        if let Some(&existing_idx) = seen_hashes.get(&rec.path_hash) {
            if record_rank(&rec) > record_rank(&result[existing_idx]) {
                result[existing_idx] = rec;
            }
            continue;
        }

        let ep_file_key = (
            rec.root_dir.clone(),
            rec.season_number,
            rec.episode_number,
            rec.file_name.clone(),
        );

        if let Some(&existing_idx) = seen_same_file.get(&ep_file_key) {
            if record_rank(&rec) > record_rank(&result[existing_idx]) {
                let old_hash = result[existing_idx].path_hash;
                seen_hashes.remove(&old_hash);
                seen_hashes.insert(rec.path_hash, existing_idx);
                result[existing_idx] = rec;
            }
            continue;
        }

        let idx = result.len();
        seen_hashes.insert(rec.path_hash, idx);
        seen_same_file.insert(ep_file_key, idx);
        result.push(rec);
    }

    result
}

/// 执行内存 Diff 并使用 PostgreSQL UNNEST 批量写入 Staging 中间表 (sync_folders + sync_files)
pub async fn apply_sync_diff(
    db: &PgPool,
    records: Vec<ParsedStrmRecord>,
    total_lines: i32,
    batch_size: usize,
    force: bool,
) -> Result<SyncDiffStats> {
    let existing_hashes: Vec<i64> = sqlx::query_scalar("SELECT path_hash FROM sync_files")
        .fetch_all(db)
        .await
        .context("查询现有 sync_files path_hash 失败")?;

    let existing_set: HashSet<i64> = existing_hashes.into_iter().collect();

    if !existing_set.is_empty() && records.is_empty() {
        return Err(anyhow!(
            "安全熔断：本次解析出 0 条有效视频，拒绝清空现有 {} 条记录",
            existing_set.len()
        ));
    }

    let current_set: HashSet<i64> = records.iter().map(|r| r.path_hash).collect();

    let to_insert: Vec<ParsedStrmRecord> = records
        .into_iter()
        .filter(|r| !existing_set.contains(&r.path_hash))
        .collect();

    let to_delete: Vec<i64> = existing_set.difference(&current_set).copied().collect();

    if !force && !existing_set.is_empty() && to_delete.len() > 100 {
        let ratio = to_delete.len() as f64 / existing_set.len() as f64;
        if ratio > 0.15 {
            return Err(anyhow!(
                "安全熔断：自动同步检测到待移除 {} 条记录（占比 {:.1}% > 15%），已中止以防上游残缺误删；若确认上游有意精简，请在后台点击「手动触发同步」强制对账",
                to_delete.len(),
                ratio * 100.0
            ));
        }
    }

    let added_episodes = to_insert.len() as i32;
    let deleted_episodes = to_delete.len() as i32;

    if to_insert.is_empty() && to_delete.is_empty() {
        info!("增量 Diff 完成：无新增或失效资源");
        return Ok(SyncDiffStats {
            total_lines,
            added_episodes: 0,
            deleted_episodes: 0,
            new_media_items: 0,
            updated_folder_ids: Vec::new(),
        });
    }

    let mut tx = db.begin().await.context("开启同步事务失败")?;
    let mut new_folders_count = 0_i32;
    let mut touched_folder_ids: HashSet<i64> = HashSet::new();

    if !to_insert.is_empty() {
        // 按 root_dir 聚合出唯一的新增目录分组
        let mut folder_map: HashMap<&str, &ParsedStrmRecord> = HashMap::new();
        for rec in &to_insert {
            folder_map
                .entry(&rec.root_dir)
                .and_modify(|prev| {
                    if rec.tmdb_id.is_some() && prev.tmdb_id.is_none() {
                        *prev = rec;
                    }
                })
                .or_insert(rec);
        }

        let unique_folders: Vec<&ParsedStrmRecord> = folder_map.into_values().collect();
        let mut root_to_folder_id: HashMap<String, i64> =
            HashMap::with_capacity(unique_folders.len());

        for chunk in unique_folders.chunks(batch_size.max(500)) {
            let mut root_dirs = Vec::with_capacity(chunk.len());
            let mut raw_folders = Vec::with_capacity(chunk.len());
            let mut clean_titles = Vec::with_capacity(chunk.len());
            let mut years = Vec::with_capacity(chunk.len());
            let mut media_types = Vec::with_capacity(chunk.len());
            let mut categories = Vec::with_capacity(chunk.len());
            let mut tmdb_ids = Vec::with_capacity(chunk.len());

            for r in chunk {
                root_dirs.push(r.root_dir.as_str());
                raw_folders.push(r.raw_folder_name.as_str());
                clean_titles.push(r.clean_title.as_str());
                years.push(r.year);
                media_types.push(r.inferred_media_type.as_str());
                categories.push(r.inferred_category.as_str());
                tmdb_ids.push(r.tmdb_id);
            }

            let rows: Vec<(i64, String, bool)> = sqlx::query_as(
                r#"
                INSERT INTO sync_folders (
                    root_dir, raw_folder_name, clean_title, year,
                    inferred_media_type, category, parsed_tmdb_id
                )
                SELECT * FROM UNNEST(
                    $1::text[], $2::text[], $3::text[], $4::int4[],
                    $5::text[], $6::text[], $7::int4[]
                )
                ON CONFLICT (root_dir) DO UPDATE SET
                    raw_folder_name = CASE WHEN sync_folders.is_locked THEN sync_folders.raw_folder_name ELSE EXCLUDED.raw_folder_name END,
                    clean_title = CASE WHEN sync_folders.is_locked THEN sync_folders.clean_title ELSE EXCLUDED.clean_title END,
                    year = CASE WHEN sync_folders.is_locked THEN sync_folders.year ELSE EXCLUDED.year END,
                    scrape_state = CASE
                        WHEN NOT sync_folders.is_locked
                             AND sync_folders.scrape_state IN ('conflict', 'failed')
                             AND (
                                 sync_folders.clean_title IS DISTINCT FROM EXCLUDED.clean_title
                                 OR sync_folders.year IS DISTINCT FROM EXCLUDED.year
                             )
                        THEN 'pending'
                        ELSE sync_folders.scrape_state
                    END,
                    conflict_reason = CASE
                        WHEN NOT sync_folders.is_locked
                             AND sync_folders.scrape_state IN ('conflict', 'failed')
                             AND (
                                 sync_folders.clean_title IS DISTINCT FROM EXCLUDED.clean_title
                                 OR sync_folders.year IS DISTINCT FROM EXCLUDED.year
                             )
                        THEN NULL
                        ELSE sync_folders.conflict_reason
                    END,
                    retry_count = CASE
                        WHEN NOT sync_folders.is_locked
                             AND sync_folders.scrape_state IN ('conflict', 'failed')
                             AND (
                                 sync_folders.clean_title IS DISTINCT FROM EXCLUDED.clean_title
                                 OR sync_folders.year IS DISTINCT FROM EXCLUDED.year
                             )
                        THEN 0
                        ELSE sync_folders.retry_count
                    END,
                    next_retry_at = CASE
                        WHEN NOT sync_folders.is_locked
                             AND sync_folders.scrape_state IN ('conflict', 'failed')
                             AND (
                                 sync_folders.clean_title IS DISTINCT FROM EXCLUDED.clean_title
                                 OR sync_folders.year IS DISTINCT FROM EXCLUDED.year
                             )
                        THEN NULL
                        ELSE sync_folders.next_retry_at
                    END,
                    parsed_tmdb_id = COALESCE(sync_folders.parsed_tmdb_id, EXCLUDED.parsed_tmdb_id),
                    updated_at = NOW()
                RETURNING id, root_dir, (xmax = 0) AS is_inserted
                "#,
            )
            .bind(&root_dirs)
            .bind(&raw_folders)
            .bind(&clean_titles)
            .bind(&years)
            .bind(&media_types)
            .bind(&categories)
            .bind(&tmdb_ids)
            .fetch_all(&mut *tx)
            .await
            .context("批量 UPSERT sync_folders 失败")?;

            for (id, root, is_inserted) in rows {
                if is_inserted {
                    new_folders_count += 1;
                } else {
                    touched_folder_ids.insert(id);
                }
                root_to_folder_id.insert(root, id);
            }
        }

        // 批量插入物理视频文件 sync_files
        for chunk in to_insert.chunks(batch_size.max(500)) {
            let mut folder_ids = Vec::with_capacity(chunk.len());
            let mut seasons = Vec::with_capacity(chunk.len());
            let mut ep_nums = Vec::with_capacity(chunk.len());
            let mut alist_paths = Vec::with_capacity(chunk.len());
            let mut path_hashes = Vec::with_capacity(chunk.len());
            let mut file_names = Vec::with_capacity(chunk.len());
            let mut containers = Vec::with_capacity(chunk.len());
            let mut version_tags = Vec::with_capacity(chunk.len());
            let mut priority_scores = Vec::with_capacity(chunk.len());

            for r in chunk {
                let fid = *root_to_folder_id
                    .get(&r.root_dir)
                    .context("缺失 sync_folders ID 映射")?;
                folder_ids.push(fid);
                seasons.push(r.season_number);
                ep_nums.push(r.episode_number);
                alist_paths.push(r.alist_path.as_str());
                path_hashes.push(r.path_hash);
                file_names.push(r.file_name.as_str());
                containers.push(r.container.as_str());
                version_tags.push(r.version_tag.as_str());
                priority_scores.push(r.priority_score);
            }

            sqlx::query(
                r#"
                INSERT INTO sync_files (
                    folder_id, season_number, episode_number, alist_path,
                    path_hash, file_name, container, version_tag, priority_score
                )
                SELECT * FROM UNNEST(
                    $1::int8[], $2::int4[], $3::int4[], $4::text[],
                    $5::int8[], $6::text[], $7::text[], $8::text[], $9::int4[]
                )
                ON CONFLICT (path_hash) DO NOTHING
                "#,
            )
            .bind(&folder_ids)
            .bind(&seasons)
            .bind(&ep_nums)
            .bind(&alist_paths)
            .bind(&path_hashes)
            .bind(&file_names)
            .bind(&containers)
            .bind(&version_tags)
            .bind(&priority_scores)
            .execute(&mut *tx)
            .await
            .context("批量 INSERT sync_files 失败")?;
        }
    }

    if !to_delete.is_empty() {
        for chunk in to_delete.chunks(10_000) {
            sqlx::query("DELETE FROM sync_files WHERE path_hash = ANY($1)")
                .bind(chunk)
                .execute(&mut *tx)
                .await
                .context("批量清理失效 sync_files 失败")?;
        }

        // 清理已没有任何物理文件的空 sync_folders（media_works 及其元数据缓存保留不删）
        sqlx::query(
            r#"
            DELETE FROM sync_folders f
            WHERE NOT EXISTS (
                SELECT 1 FROM sync_files sf WHERE sf.folder_id = f.id
            )
            "#,
        )
        .execute(&mut *tx)
        .await
        .context("清理孤儿 sync_folders 失败")?;
    }

    // 查出已绑定 work_id 但新增了分集的老目录，通知刮削器增量补齐新集元数据
    let mut updated_folder_ids = Vec::new();
    if !touched_folder_ids.is_empty() {
        let ids: Vec<i64> = touched_folder_ids.into_iter().collect();
        updated_folder_ids = sqlx::query_scalar(
            r#"
            SELECT f.id
            FROM sync_folders f
            WHERE f.id = ANY($1)
              AND f.work_id IS NOT NULL
              AND f.is_ignored = FALSE
            "#,
        )
        .bind(&ids)
        .fetch_all(&mut *tx)
        .await
        .unwrap_or_default();
    }

    tx.commit().await.context("提交同步事务失败")?;

    Ok(SyncDiffStats {
        total_lines,
        added_episodes,
        deleted_episodes,
        new_media_items: new_folders_count,
        updated_folder_ids,
    })
}
