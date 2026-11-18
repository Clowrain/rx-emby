-- P2 阶段：Emby 客户端视图偏好、播放列表与合集兼容表

CREATE TABLE IF NOT EXISTS user_display_preferences (
    id BIGSERIAL PRIMARY KEY,
    user_id VARCHAR(32) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    client VARCHAR(64) NOT NULL DEFAULT 'emby',
    display_prefs_id VARCHAR(128) NOT NULL,
    prefs_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_id, client, display_prefs_id)
);

CREATE TABLE IF NOT EXISTS playlists (
    id BIGSERIAL PRIMARY KEY,
    user_id VARCHAR(32) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name VARCHAR(255) NOT NULL,
    media_type VARCHAR(32) NOT NULL DEFAULT 'Video',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_playlists_user ON playlists(user_id);

CREATE TABLE IF NOT EXISTS playlist_items (
    id BIGSERIAL PRIMARY KEY,
    playlist_id BIGINT NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    item_id VARCHAR(64) NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_playlist_items_pl ON playlist_items(playlist_id, sort_order, id);

CREATE TABLE IF NOT EXISTS collections (
    id BIGSERIAL PRIMARY KEY,
    user_id VARCHAR(32) REFERENCES users(id) ON DELETE CASCADE,
    name VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS collection_items (
    id BIGSERIAL PRIMARY KEY,
    collection_id BIGINT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    item_id VARCHAR(64) NOT NULL,
    UNIQUE(collection_id, item_id)
);
CREATE INDEX IF NOT EXISTS idx_collection_items_col ON collection_items(collection_id);

-- 兼容已创建表的升级路径；历史合集保留空所有者。
ALTER TABLE collections ADD COLUMN IF NOT EXISTS user_id VARCHAR(32) REFERENCES users(id) ON DELETE CASCADE;
ALTER TABLE collections ADD COLUMN IF NOT EXISTS parent_id VARCHAR(64);
ALTER TABLE collections ADD COLUMN IF NOT EXISTS is_locked BOOLEAN NOT NULL DEFAULT FALSE;
CREATE INDEX IF NOT EXISTS idx_collections_user ON collections(user_id);
CREATE INDEX IF NOT EXISTS idx_playlist_items_pl_item ON playlist_items(playlist_id, item_id);
CREATE INDEX IF NOT EXISTS idx_media_works_tags_gin ON media_works USING GIN(tags);
CREATE INDEX IF NOT EXISTS idx_media_works_year ON media_works(year) WHERE year IS NOT NULL;
ALTER TABLE play_histories ADD COLUMN IF NOT EXISTS play_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE play_histories ADD COLUMN IF NOT EXISTS hidden_from_resume BOOLEAN NOT NULL DEFAULT FALSE;
