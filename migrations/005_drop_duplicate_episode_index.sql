DROP INDEX IF EXISTS idx_work_episodes_work_season_episode;

-- Keep the premiere-date year fallback order while supporting the hot sort path.
CREATE INDEX IF NOT EXISTS idx_media_works_premiere_fallback_id
    ON media_works (
        (COALESCE(
            NULLIF(premiered_at, ''),
            CASE WHEN year IS NOT NULL THEN year::text || '-01-01' ELSE NULL END
        )) DESC NULLS LAST,
        id DESC
    );
