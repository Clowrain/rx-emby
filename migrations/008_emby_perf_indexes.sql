-- 008: Cover (user_id, work_id) lookups on play_histories and user_favorites when tmdb_id IS NOT NULL.
CREATE INDEX IF NOT EXISTS idx_history_user_work_id
    ON play_histories (user_id, work_id)
    WHERE work_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_fav_user_work_id
    ON user_favorites (user_id, work_id)
    WHERE work_id IS NOT NULL;
