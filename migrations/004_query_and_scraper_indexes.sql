-- 1. 清理历史重复的客户端访问令牌（同一用户 + 同一客户端标识仅保留最近活跃的一条）
DELETE FROM access_tokens a
USING access_tokens b
WHERE a.user_id = b.user_id
  AND a.client_name = b.client_name
  AND a.device_name = b.device_name
  AND (
      a.last_used_at < b.last_used_at
      OR (a.last_used_at = b.last_used_at AND a.token < b.token)
  );

CREATE INDEX IF NOT EXISTS idx_access_tokens_user_device
    ON access_tokens(user_id, device_id);

CREATE INDEX IF NOT EXISTS idx_access_tokens_user_client
    ON access_tokens(user_id, client_name);

-- 2. 加速后台刮削调度器对待刮削剧集季与残缺分集的轮询，消除全表顺序扫描
CREATE INDEX IF NOT EXISTS idx_seasons_media_attempts
    ON seasons(media_item_id, scrape_attempts);

CREATE INDEX IF NOT EXISTS idx_seasons_pending_attempts
    ON seasons(media_item_id)
    WHERE scrape_attempts = 0;

CREATE INDEX IF NOT EXISTS idx_episodes_incomplete_updated
    ON episodes(media_item_id, season_number, updated_at)
    WHERE meta_incomplete = TRUE;

-- 3. 加速待重试刮削队列与后台海报墙去重排序
CREATE INDEX IF NOT EXISTS idx_media_retry_queue
    ON media_items(scrape_priority ASC, id DESC)
    WHERE is_ignored = FALSE AND is_locked = FALSE AND scrape_status = 2 AND retry_count < 3;

CREATE INDEX IF NOT EXISTS idx_media_dedup_active
    ON media_items (
        (COALESCE(tmdb_id::text || ':' || media_type, 'raw:' || id::text)),
        (scrape_status IN (1, 3)) DESC,
        (poster_path IS NOT NULL) DESC,
        id ASC
    )
    WHERE is_ignored = FALSE;
