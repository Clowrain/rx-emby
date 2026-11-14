CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- 0. 系统动态配置表（支持 Web 后台热更新）
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

-- 3. 影视主表
CREATE TABLE IF NOT EXISTS media_items (
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

    root_dir TEXT UNIQUE NOT NULL,
    raw_folder_name VARCHAR(255) NOT NULL,
    category VARCHAR(32) NOT NULL,
    scrape_status SMALLINT NOT NULL DEFAULT 0,
    scrape_priority SMALLINT NOT NULL DEFAULT 3,
    retry_count SMALLINT NOT NULL DEFAULT 0,
    next_retry_at TIMESTAMPTZ,
    is_locked BOOLEAN NOT NULL DEFAULT FALSE,
    is_ignored BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_media_category_year ON media_items(category, year DESC) WHERE is_ignored = FALSE;
CREATE INDEX IF NOT EXISTS idx_media_scrape_queue ON media_items(scrape_status, scrape_priority, id DESC) WHERE is_ignored = FALSE;
CREATE INDEX IF NOT EXISTS idx_media_tmdb ON media_items(tmdb_id, media_type) WHERE tmdb_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_media_created ON media_items(created_at DESC) WHERE is_ignored = FALSE;
CREATE INDEX IF NOT EXISTS idx_media_countries_gin ON media_items USING GIN (countries);
CREATE INDEX IF NOT EXISTS idx_media_networks_gin ON media_items USING GIN (networks);
CREATE INDEX IF NOT EXISTS idx_media_genres_gin ON media_items USING GIN (genres);
CREATE INDEX IF NOT EXISTS idx_media_title_trgm ON media_items USING GIN (title gin_trgm_ops);
CREATE INDEX IF NOT EXISTS idx_media_orig_title_trgm ON media_items USING GIN (original_title gin_trgm_ops);

-- 4. 电视剧季表
CREATE TABLE IF NOT EXISTS seasons (
    id BIGSERIAL PRIMARY KEY,
    media_item_id BIGINT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    season_number INTEGER NOT NULL,
    name VARCHAR(255),
    overview TEXT,
    poster_path VARCHAR(255),
    air_date VARCHAR(32),
    episode_count INTEGER NOT NULL DEFAULT 0,
    UNIQUE(media_item_id, season_number)
);

-- 5. 单集 / 视频物理源实体表（支持同剧集多版本挂载）
CREATE TABLE IF NOT EXISTS episodes (
    id BIGSERIAL PRIMARY KEY,
    media_item_id BIGINT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    season_id BIGINT REFERENCES seasons(id) ON DELETE CASCADE,
    season_number INTEGER NOT NULL DEFAULT 0,
    episode_number INTEGER NOT NULL DEFAULT 1,
    title VARCHAR(255),
    version_tag VARCHAR(128) NOT NULL DEFAULT '默认版本',
    priority_score INTEGER NOT NULL DEFAULT 0,
    overview TEXT,
    still_path VARCHAR(255),
    air_date VARCHAR(32),
    runtime INTEGER NOT NULL DEFAULT 0,

    alist_path TEXT NOT NULL,
    path_hash BIGINT UNIQUE NOT NULL,
    file_name VARCHAR(512) NOT NULL,
    container VARCHAR(32) NOT NULL,
    file_size_bytes BIGINT,
    subtitles TEXT[] NOT NULL DEFAULT '{}',
    meta_incomplete BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_episodes_media_season ON episodes(media_item_id, season_number, episode_number, priority_score DESC);

-- 6. 本地图片缓存 LRU 索引表
CREATE TABLE IF NOT EXISTS image_cache_index (
    cache_key VARCHAR(255) PRIMARY KEY,
    local_rel_path VARCHAR(255) NOT NULL,
    size_bytes BIGINT NOT NULL,
    last_accessed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_image_cache_lru ON image_cache_index(last_accessed_at ASC);

-- 7. 自定义虚拟媒体库配置表
CREATE TABLE IF NOT EXISTS virtual_libraries (
    id SERIAL PRIMARY KEY,
    name VARCHAR(64) NOT NULL,
    icon_type VARCHAR(32) NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    filter_rules JSONB NOT NULL,
    is_enabled BOOLEAN NOT NULL DEFAULT TRUE
);

-- 8. 多用户播放进度表
CREATE TABLE IF NOT EXISTS play_histories (
    id BIGSERIAL PRIMARY KEY,
    user_id VARCHAR(32) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    media_item_id BIGINT,
    tmdb_id INTEGER,
    media_type VARCHAR(16) NOT NULL,
    season_number INTEGER NOT NULL DEFAULT 0,
    episode_number INTEGER NOT NULL DEFAULT 1,
    position_ticks BIGINT NOT NULL DEFAULT 0,
    duration_ticks BIGINT NOT NULL DEFAULT 0,
    is_played BOOLEAN NOT NULL DEFAULT FALSE,
    last_played_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_history_user_tmdb ON play_histories(user_id, tmdb_id, media_type, season_number, episode_number) WHERE tmdb_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_history_user_continue ON play_histories(user_id, is_played, last_played_at DESC);

-- 9. 多用户收藏表
CREATE TABLE IF NOT EXISTS user_favorites (
    id BIGSERIAL PRIMARY KEY,
    user_id VARCHAR(32) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    media_item_id BIGINT,
    tmdb_id INTEGER,
    media_type VARCHAR(16) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_fav_user_tmdb ON user_favorites(user_id, tmdb_id, media_type) WHERE tmdb_id IS NOT NULL;

-- 10. 增量同步流水日志表
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
