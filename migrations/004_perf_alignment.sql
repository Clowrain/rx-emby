-- 004: Align indexes with hot image, media, and virtual-library access paths.
CREATE INDEX IF NOT EXISTS idx_media_works_premiered_id
    ON media_works(premiered_at DESC NULLS LAST, id DESC);
CREATE INDEX IF NOT EXISTS idx_image_cache_lru_key
    ON image_cache_index(last_accessed_at ASC, cache_key ASC);
CREATE INDEX IF NOT EXISTS idx_virtual_libraries_sort_id
    ON virtual_libraries(sort_order ASC, id ASC);
CREATE INDEX IF NOT EXISTS idx_work_episodes_work_season_episode
    ON work_episodes(work_id, season_number, episode_number);
CREATE INDEX IF NOT EXISTS idx_media_works_rating_id
    ON media_works(rating DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_media_works_created_id
    ON media_works(created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_media_works_title_id
    ON media_works(title ASC, id ASC);
CREATE INDEX IF NOT EXISTS idx_user_favorites_user_work
    ON user_favorites(user_id, work_id);
CREATE INDEX IF NOT EXISTS idx_sync_files_file_name_trgm
    ON sync_files USING GIN (file_name gin_trgm_ops);
