-- 003: 性能与查询计划优化索引（仅 CREATE INDEX IF NOT EXISTS，幂等可重复执行）
-- 注意：work_episodes(work_id, season_number, episode_number) 与
--       sync_files(folder_id, season_number, episode_number, ...) 已由 001 的
--       UNIQUE / 复合索引覆盖，此处不重复创建。

-- 1. 海报墙 / 列表高频过滤：仅统计未忽略且有归属作品的目录
CREATE INDEX IF NOT EXISTS idx_sync_folders_work_active
    ON sync_folders(work_id)
    WHERE work_id IS NOT NULL AND is_ignored = FALSE;

-- 2. DirPrewarm / 播放寻址：按目录 + 文件名定位唯一版本文件
CREATE INDEX IF NOT EXISTS idx_sync_files_folder_name
    ON sync_files(folder_id, file_name);

-- 3. media_works 常见排序/筛选模式（降序索引可直接满足 ORDER BY，避免额外 Sort 节点）
CREATE INDEX IF NOT EXISTS idx_media_works_year_id
    ON media_works(year DESC NULLS LAST, id DESC);
CREATE INDEX IF NOT EXISTS idx_media_works_rating_year_id
    ON media_works(rating DESC, year DESC NULLS LAST, id DESC);
CREATE INDEX IF NOT EXISTS idx_media_works_updated_id
    ON media_works(updated_at DESC, id DESC);

-- 4. 智能视图 / 封面聚合会按 tmdb_metadata JSON 字段过滤，GIN 路径操作符索引体积更小
CREATE INDEX IF NOT EXISTS idx_media_works_tmdb_metadata_gin
    ON media_works USING GIN (tmdb_metadata jsonb_path_ops);

-- 5. 继续观看 (Resume)：按用户 + 媒体类型过滤后按最近播放倒序取前 N 条
CREATE INDEX IF NOT EXISTS idx_history_user_type_played
    ON play_histories(user_id, media_type, last_played_at DESC);
-- NextUp / 单作品集数进度聚合：按用户 + 作品定位全部播放记录
CREATE INDEX IF NOT EXISTS idx_history_user_work
    ON play_histories(user_id, work_id);

-- 6. 搜索列补齐：sort_title、sync_folders 目录名（clean/raw）、分集标题的三字组索引
--    （1~2 字符短查询无法用 trgm 索引，但 ILIKE 语义不受影响，退化为顺序扫描）
CREATE INDEX IF NOT EXISTS idx_media_works_sort_title_trgm
    ON media_works USING GIN (sort_title gin_trgm_ops);
CREATE INDEX IF NOT EXISTS idx_sync_folders_clean_title_trgm
    ON sync_folders USING GIN (clean_title gin_trgm_ops);
CREATE INDEX IF NOT EXISTS idx_sync_folders_raw_name_trgm
    ON sync_folders USING GIN (raw_folder_name gin_trgm_ops);
CREATE INDEX IF NOT EXISTS idx_work_episodes_title_trgm
    ON work_episodes USING GIN (title gin_trgm_ops);
