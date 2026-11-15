-- 每日系统与数据库资源使用审计表
CREATE TABLE IF NOT EXISTS resource_daily_stats (
    stat_date DATE PRIMARY KEY,
    sample_count INTEGER NOT NULL DEFAULT 0,

    app_cpu_avg DOUBLE PRECISION NOT NULL DEFAULT 0,
    app_cpu_max DOUBLE PRECISION NOT NULL DEFAULT 0,
    app_mem_avg_mb DOUBLE PRECISION NOT NULL DEFAULT 0,
    app_mem_max_mb DOUBLE PRECISION NOT NULL DEFAULT 0,

    db_cpu_avg DOUBLE PRECISION NOT NULL DEFAULT 0,
    db_cpu_max DOUBLE PRECISION NOT NULL DEFAULT 0,
    db_mem_avg_mb DOUBLE PRECISION NOT NULL DEFAULT 0,
    db_mem_max_mb DOUBLE PRECISION NOT NULL DEFAULT 0,
    db_size_max_mb DOUBLE PRECISION NOT NULL DEFAULT 0,
    db_conn_avg DOUBLE PRECISION NOT NULL DEFAULT 0,
    db_conn_max INTEGER NOT NULL DEFAULT 0,

    disk_used_max_gb DOUBLE PRECISION NOT NULL DEFAULT 0,
    disk_usage_max_pct DOUBLE PRECISION NOT NULL DEFAULT 0,

    peak_cpu_at TIMESTAMPTZ,
    peak_cpu_scenario VARCHAR(64),
    peak_mem_at TIMESTAMPTZ,
    peak_mem_scenario VARCHAR(64),

    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_resource_daily_stats_date
    ON resource_daily_stats (stat_date DESC);
