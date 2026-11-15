CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- 0. 系统动态配置与持久化同步游标表
CREATE TABLE IF NOT EXISTS system_configs (
    config_key VARCHAR(128) PRIMARY KEY,
    config_value TEXT NOT NULL,
    value_type VARCHAR(16) NOT NULL DEFAULT 'string',
    description VARCHAR(255),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 1. 用户表
CREATE TABLE IF NOT EXISTS users (
    id VARCHAR(32) PRIMARY KEY,
    username VARCHAR(64) UNIQUE NOT NULL,
    password_hash TEXT NOT NULL,
    is_admin BOOLEAN NOT NULL DEFAULT FALSE,
    allowed_categories TEXT[] NOT NULL DEFAULT ARRAY['*']::TEXT[],
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 2. 客户端访问令牌表
CREATE TABLE IF NOT EXISTS access_tokens (
    token VARCHAR(64) PRIMARY KEY,
    user_id VARCHAR(32) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id VARCHAR(128) NOT NULL,
    device_name VARCHAR(128) NOT NULL,
    client_name VARCHAR(64) NOT NULL,
    last_used_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_access_tokens_user ON access_tokens(user_id);
CREATE INDEX IF NOT EXISTS idx_access_tokens_user_device ON access_tokens(user_id, device_id);
CREATE INDEX IF NOT EXISTS idx_access_tokens_user_client ON access_tokens(user_id, client_name);

-- 3. 正式逻辑影视作品表（Canonical：仅收录刮削完整且通过校验的作品，海报墙唯一数据源）
CREATE TABLE IF NOT EXISTS media_works (
    id BIGSERIAL PRIMARY KEY,
    media_type VARCHAR(16) NOT NULL,
    tmdb_id INTEGER,
    title VARCHAR(255) NOT NULL,
    original_title VARCHAR(255),
    sort_title VARCHAR(255),
    year INTEGER,
    premiered_at VARCHAR(32),
    overview TEXT,
    rating REAL NOT NULL DEFAULT 0,
    poster_path VARCHAR(255),
    backdrop_path VARCHAR(255),

    countries TEXT[] NOT NULL DEFAULT '{}',
    networks TEXT[] NOT NULL DEFAULT '{}',
    genres TEXT[] NOT NULL DEFAULT '{}',
    tags TEXT[] NOT NULL DEFAULT '{}',
    category VARCHAR(32) NOT NULL DEFAULT '电影',

    tmdb_metadata JSONB NOT NULL DEFAULT '{}',
    metadata_version SMALLINT NOT NULL DEFAULT 0,
    metadata_fetched_at TIMESTAMPTZ,
    metadata_refresh_requested BOOLEAN NOT NULL DEFAULT FALSE,
    metadata_retry_count SMALLINT NOT NULL DEFAULT 0,
    metadata_retry_at TIMESTAMPTZ,

    is_locked BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_media_works_tmdb_unique
    ON media_works(tmdb_id, media_type)
    WHERE tmdb_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_media_works_category_year ON media_works(category, year DESC);
CREATE INDEX IF NOT EXISTS idx_media_works_created ON media_works(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_media_works_countries_gin ON media_works USING GIN (countries);
CREATE INDEX IF NOT EXISTS idx_media_works_networks_gin ON media_works USING GIN (networks);
CREATE INDEX IF NOT EXISTS idx_media_works_genres_gin ON media_works USING GIN (genres);
CREATE INDEX IF NOT EXISTS idx_media_works_title_trgm ON media_works USING GIN (title gin_trgm_ops);
CREATE INDEX IF NOT EXISTS idx_media_works_orig_title_trgm ON media_works USING GIN (original_title gin_trgm_ops);
CREATE INDEX IF NOT EXISTS idx_media_works_metadata_refresh
    ON media_works(metadata_refresh_requested, metadata_version, id)
    WHERE is_locked = FALSE;

-- 4. 正式作品季元数据表（每部剧全库仅存一份标准季信息）
CREATE TABLE IF NOT EXISTS work_seasons (
    id BIGSERIAL PRIMARY KEY,
    work_id BIGINT NOT NULL REFERENCES media_works(id) ON DELETE CASCADE,
    season_number INTEGER NOT NULL,
    name VARCHAR(255),
    overview TEXT,
    poster_path VARCHAR(255),
    air_date VARCHAR(32),
    episode_count INTEGER NOT NULL DEFAULT 0,
    scrape_attempts INTEGER NOT NULL DEFAULT 0,
    UNIQUE(work_id, season_number)
);
CREATE INDEX IF NOT EXISTS idx_work_seasons_attempts ON work_seasons(work_id, scrape_attempts);
CREATE INDEX IF NOT EXISTS idx_work_seasons_pending ON work_seasons(work_id) WHERE scrape_attempts = 0;

-- 5. 正式作品分集元数据表（每集全库仅存一份标准元数据）
CREATE TABLE IF NOT EXISTS work_episodes (
    id BIGSERIAL PRIMARY KEY,
    work_id BIGINT NOT NULL REFERENCES media_works(id) ON DELETE CASCADE,
    season_number INTEGER NOT NULL DEFAULT 0,
    episode_number INTEGER NOT NULL DEFAULT 1,
    title VARCHAR(255),
    overview TEXT,
    still_path VARCHAR(255),
    air_date VARCHAR(32),
    runtime INTEGER NOT NULL DEFAULT 0,
    meta_incomplete BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(work_id, season_number, episode_number)
);
CREATE INDEX IF NOT EXISTS idx_work_episodes_incomplete
    ON work_episodes(work_id, season_number, updated_at)
    WHERE meta_incomplete = TRUE;

-- 6. 中间暂存层：物理目录分组与刮削校验状态表（Staging）
CREATE TABLE IF NOT EXISTS sync_folders (
    id BIGSERIAL PRIMARY KEY,
    root_dir TEXT UNIQUE NOT NULL,
    raw_folder_name VARCHAR(255) NOT NULL,
    clean_title VARCHAR(255) NOT NULL,
    year INTEGER,
    inferred_media_type VARCHAR(16) NOT NULL,
    category VARCHAR(32) NOT NULL,
    parsed_tmdb_id INTEGER,

    work_id BIGINT REFERENCES media_works(id) ON DELETE SET NULL,
    scrape_state VARCHAR(16) NOT NULL DEFAULT 'pending', -- pending | verified | conflict | failed
    conflict_reason TEXT,
    scrape_priority SMALLINT NOT NULL DEFAULT 3,
    retry_count SMALLINT NOT NULL DEFAULT 0,
    next_retry_at TIMESTAMPTZ,

    priority_boost INTEGER NOT NULL DEFAULT 0,
    is_locked BOOLEAN NOT NULL DEFAULT FALSE,
    is_ignored BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_sync_folders_work ON sync_folders(work_id) WHERE work_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_sync_folders_queue
    ON sync_folders(scrape_state, scrape_priority, id DESC)
    WHERE is_ignored = FALSE;
CREATE INDEX IF NOT EXISTS idx_sync_folders_parsed_tmdb
    ON sync_folders(parsed_tmdb_id)
    WHERE parsed_tmdb_id IS NOT NULL;

-- 7. 中间暂存层：物理视频文件源表（Staging，增量同步唯一写入目标）
CREATE TABLE IF NOT EXISTS sync_files (
    id BIGSERIAL PRIMARY KEY,
    folder_id BIGINT NOT NULL REFERENCES sync_folders(id) ON DELETE CASCADE,
    season_number INTEGER NOT NULL DEFAULT 0,
    episode_number INTEGER NOT NULL DEFAULT 1,
    alist_path TEXT NOT NULL,
    path_hash BIGINT UNIQUE NOT NULL,
    file_name VARCHAR(512) NOT NULL,
    container VARCHAR(32) NOT NULL,
    version_tag VARCHAR(128) NOT NULL DEFAULT '默认版本',
    priority_score INTEGER NOT NULL DEFAULT 0,
    priority_override INTEGER,
    file_size_bytes BIGINT,
    subtitles TEXT[] NOT NULL DEFAULT '{}',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_sync_files_folder_ep
    ON sync_files(folder_id, season_number, episode_number, priority_score DESC);

-- 8. 本地图片缓存 LRU 索引表
CREATE TABLE IF NOT EXISTS image_cache_index (
    cache_key VARCHAR(255) PRIMARY KEY,
    local_rel_path VARCHAR(255) NOT NULL,
    size_bytes BIGINT NOT NULL,
    last_accessed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_image_cache_lru ON image_cache_index(last_accessed_at ASC);

-- 9. 自定义虚拟媒体库（智能视图）配置表
CREATE TABLE IF NOT EXISTS virtual_libraries (
    id SERIAL PRIMARY KEY,
    name VARCHAR(64) NOT NULL,
    icon_type VARCHAR(32) NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    filter_rules JSONB NOT NULL,
    is_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    cover_image VARCHAR(255),
    cover_revision BIGINT NOT NULL DEFAULT 1
);

-- 10. 多用户播放进度表
CREATE TABLE IF NOT EXISTS play_histories (
    id BIGSERIAL PRIMARY KEY,
    user_id VARCHAR(32) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    work_id BIGINT REFERENCES media_works(id) ON DELETE SET NULL,
    tmdb_id INTEGER,
    media_type VARCHAR(16) NOT NULL,
    season_number INTEGER NOT NULL DEFAULT 0,
    episode_number INTEGER NOT NULL DEFAULT 1,
    position_ticks BIGINT NOT NULL DEFAULT 0,
    duration_ticks BIGINT NOT NULL DEFAULT 0,
    is_played BOOLEAN NOT NULL DEFAULT FALSE,
    last_played_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_history_user_tmdb
    ON play_histories(user_id, tmdb_id, media_type, season_number, episode_number)
    WHERE tmdb_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_history_user_work_notmdb
    ON play_histories(user_id, work_id, season_number, episode_number)
    WHERE tmdb_id IS NULL AND work_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_history_user_continue ON play_histories(user_id, is_played, last_played_at DESC);

-- 11. 多用户收藏表
CREATE TABLE IF NOT EXISTS user_favorites (
    id BIGSERIAL PRIMARY KEY,
    user_id VARCHAR(32) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    work_id BIGINT REFERENCES media_works(id) ON DELETE SET NULL,
    tmdb_id INTEGER,
    media_type VARCHAR(16) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_fav_user_tmdb
    ON user_favorites(user_id, tmdb_id, media_type)
    WHERE tmdb_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_fav_user_work_notmdb
    ON user_favorites(user_id, work_id)
    WHERE tmdb_id IS NULL AND work_id IS NOT NULL;

-- 12. 增量同步流水日志表
CREATE TABLE IF NOT EXISTS sync_logs (
    id BIGSERIAL PRIMARY KEY,
    started_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ,
    duration_ms BIGINT,
    total_lines INTEGER NOT NULL DEFAULT 0,
    added_episodes INTEGER NOT NULL DEFAULT 0,
    deleted_episodes INTEGER NOT NULL DEFAULT 0,
    new_media_items INTEGER NOT NULL DEFAULT 0,
    status VARCHAR(32) NOT NULL,
    error_message TEXT
);

-- 13. 智能视图 SQL 辅助函数
CREATE OR REPLACE FUNCTION smart_texts(value JSONB) RETURNS TEXT[] LANGUAGE SQL IMMUTABLE AS $$
    SELECT COALESCE(array_agg(v), '{}'::text[])
    FROM jsonb_array_elements_text(COALESCE(NULLIF(value, 'null'::jsonb), '[]'::jsonb)) v
$$;

CREATE OR REPLACE FUNCTION smart_country_code(value TEXT) RETURNS TEXT LANGUAGE SQL IMMUTABLE AS $$
    SELECT CASE trim(value)
      WHEN '中国大陆' THEN 'CN' WHEN '中国' THEN 'CN' WHEN '中国香港' THEN 'HK' WHEN '香港' THEN 'HK'
      WHEN '中国台湾' THEN 'TW' WHEN '台湾' THEN 'TW' WHEN '美国' THEN 'US' WHEN '日本' THEN 'JP'
      WHEN '韩国' THEN 'KR' WHEN '英国' THEN 'GB' WHEN '法国' THEN 'FR' WHEN '德国' THEN 'DE'
      WHEN '泰国' THEN 'TH' WHEN '印度' THEN 'IN' WHEN '加拿大' THEN 'CA' WHEN '澳大利亚' THEN 'AU'
      WHEN '西班牙' THEN 'ES' WHEN '意大利' THEN 'IT' WHEN '俄罗斯' THEN 'RU'
      ELSE CASE upper(trim(value)) WHEN 'UK' THEN 'GB' ELSE upper(trim(value)) END END
$$;

CREATE OR REPLACE FUNCTION smart_network_name(value TEXT) RETURNS TEXT LANGUAGE SQL IMMUTABLE AS $$
    SELECT CASE lower(trim(value)) WHEN 'apple tv+' THEN 'apple tv' ELSE lower(trim(value)) END
$$;

CREATE OR REPLACE FUNCTION smart_country_values(value JSONB) RETURNS TEXT[] LANGUAGE SQL IMMUTABLE AS $$
    WITH codes AS (SELECT smart_country_code(v) AS code FROM unnest(smart_texts(value)) v),
    labels(code,name) AS (VALUES ('CN','中国大陆'),('CN','中国'),('HK','中国香港'),('HK','香港'),
      ('TW','中国台湾'),('TW','台湾'),('US','美国'),('JP','日本'),('KR','韩国'),('GB','英国'),
      ('FR','法国'),('DE','德国'),('TH','泰国'),('IN','印度'),('CA','加拿大'),('AU','澳大利亚'),
      ('ES','西班牙'),('IT','意大利'),('RU','俄罗斯'))
    SELECT COALESCE(array_agg(DISTINCT v),'{}'::text[]) FROM (
      SELECT code AS v FROM codes UNION SELECT name FROM labels JOIN codes USING(code)
    ) all_values
$$;

CREATE OR REPLACE FUNCTION smart_network_values(value JSONB) RETURNS TEXT[] LANGUAGE SQL IMMUTABLE AS $$
    SELECT COALESCE(array_agg(DISTINCT v),'{}'::text[]) FROM (
      SELECT unnest(smart_texts(value)) AS v
      UNION SELECT unnest(ARRAY['Apple TV','Apple TV+']) WHERE EXISTS (
        SELECT 1 FROM unnest(smart_texts(value)) n WHERE smart_network_name(n)='apple tv'
      )
    ) all_values
$$;
