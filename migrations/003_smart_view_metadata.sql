ALTER TABLE virtual_libraries ADD COLUMN cover_revision BIGINT NOT NULL DEFAULT 1;
-- 扩展元数据独立保留版本和刷新状态，失败时保留已成功刮削的信息。
ALTER TABLE media_items
    ADD COLUMN tmdb_metadata JSONB NOT NULL DEFAULT '{}',
    ADD COLUMN metadata_version SMALLINT NOT NULL DEFAULT 0,
    ADD COLUMN metadata_fetched_at TIMESTAMPTZ,
    ADD COLUMN metadata_refresh_requested BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN metadata_retry_count SMALLINT NOT NULL DEFAULT 0,
    ADD COLUMN metadata_retry_at TIMESTAMPTZ;
CREATE INDEX idx_metadata_refresh_queue ON media_items(metadata_refresh_requested, metadata_version, id)
    WHERE is_ignored = FALSE AND is_locked = FALSE;

-- 原 episode_count 存放刮削尝试状态，迁至专用字段后恢复真实集数含义。
ALTER TABLE seasons ADD COLUMN scrape_attempts INTEGER NOT NULL DEFAULT 0;
UPDATE seasons SET scrape_attempts = episode_count, episode_count = 0;

CREATE FUNCTION smart_texts(value JSONB) RETURNS TEXT[] LANGUAGE SQL IMMUTABLE AS $$
    SELECT COALESCE(array_agg(v), '{}'::text[]) FROM jsonb_array_elements_text(COALESCE(NULLIF(value, 'null'::jsonb), '[]'::jsonb)) v
$$;
CREATE FUNCTION smart_country_code(value TEXT) RETURNS TEXT LANGUAGE SQL IMMUTABLE AS $$
    SELECT CASE trim(value)
      WHEN '中国大陆' THEN 'CN' WHEN '中国' THEN 'CN' WHEN '中国香港' THEN 'HK' WHEN '香港' THEN 'HK'
      WHEN '中国台湾' THEN 'TW' WHEN '台湾' THEN 'TW' WHEN '美国' THEN 'US' WHEN '日本' THEN 'JP'
      WHEN '韩国' THEN 'KR' WHEN '英国' THEN 'GB' WHEN '法国' THEN 'FR' WHEN '德国' THEN 'DE'
      WHEN '泰国' THEN 'TH' WHEN '印度' THEN 'IN' WHEN '加拿大' THEN 'CA' WHEN '澳大利亚' THEN 'AU'
      WHEN '西班牙' THEN 'ES' WHEN '意大利' THEN 'IT' WHEN '俄罗斯' THEN 'RU'
      ELSE CASE upper(trim(value)) WHEN 'UK' THEN 'GB' ELSE upper(trim(value)) END END
$$;
CREATE FUNCTION smart_network_name(value TEXT) RETURNS TEXT LANGUAGE SQL IMMUTABLE AS $$
    SELECT CASE lower(trim(value)) WHEN 'apple tv+' THEN 'apple tv' ELSE lower(trim(value)) END
$$;
UPDATE media_items SET countries = ARRAY(SELECT DISTINCT smart_country_code(c) FROM unnest(countries) c)
WHERE cardinality(countries) > 0;

-- 同时兼容历史中文规则和标准国家代码，展开参数而不对每一条媒体记录做转换。
CREATE FUNCTION smart_country_values(value JSONB) RETURNS TEXT[] LANGUAGE SQL IMMUTABLE AS $$
    WITH codes AS (SELECT smart_country_code(v) AS code FROM unnest(smart_texts(value)) v),
    labels(code,name) AS (VALUES ('CN','中国大陆'),('CN','中国'),('HK','中国香港'),('HK','香港'),
      ('TW','中国台湾'),('TW','台湾'),('US','美国'),('JP','日本'),('KR','韩国'),('GB','英国'),
      ('FR','法国'),('DE','德国'),('TH','泰国'),('IN','印度'),('CA','加拿大'),('AU','澳大利亚'),
      ('ES','西班牙'),('IT','意大利'),('RU','俄罗斯'))
    SELECT COALESCE(array_agg(DISTINCT v),'{}'::text[]) FROM (
      SELECT code AS v FROM codes UNION SELECT name FROM labels JOIN codes USING(code)
    ) all_values
$$;
CREATE FUNCTION smart_network_values(value JSONB) RETURNS TEXT[] LANGUAGE SQL IMMUTABLE AS $$
    SELECT COALESCE(array_agg(DISTINCT v),'{}'::text[]) FROM (
      SELECT unnest(smart_texts(value)) AS v
      UNION SELECT unnest(ARRAY['Apple TV','Apple TV+']) WHERE EXISTS (
        SELECT 1 FROM unnest(smart_texts(value)) n WHERE smart_network_name(n)='apple tv'
      )
    ) all_values
$$;
