-- 为虚拟媒体库（智能视图）增加封面图配置字段
ALTER TABLE virtual_libraries
    ADD COLUMN IF NOT EXISTS cover_image VARCHAR(255);
