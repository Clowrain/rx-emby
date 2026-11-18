-- 007: Cover incremental staging scans and hot episode/season lookups.
-- Keep every index idempotent so the migration can safely be reapplied.
CREATE INDEX IF NOT EXISTS idx_sync_folders_updated_id
    ON sync_folders (updated_at DESC, id DESC);

-- Pending and retry polling are separate predicates; partial indexes keep the
-- active queue small while retaining the queue's priority ordering.
CREATE INDEX IF NOT EXISTS idx_sync_folders_pending_poll
    ON sync_folders (scrape_priority ASC, id DESC)
    WHERE scrape_state = 'pending' AND is_ignored = FALSE AND is_locked = FALSE;
CREATE INDEX IF NOT EXISTS idx_sync_folders_failed_poll
    ON sync_folders (scrape_priority ASC, id DESC)
    WHERE scrape_state = 'failed' AND is_ignored = FALSE;
CREATE INDEX IF NOT EXISTS idx_sync_folders_work_not_ignored
    ON sync_folders (work_id, id)
    WHERE work_id IS NOT NULL AND is_ignored = FALSE;

-- Episode/season metadata lookups by work and season. sync_files coordinates
-- and work_episodes coordinates are already covered by indexes declared in 001.
CREATE INDEX IF NOT EXISTS idx_work_seasons_work_number
    ON work_seasons (work_id, season_number);
