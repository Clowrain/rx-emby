# 智能视图导入导出与 TMDB 规则参考设计文档

日期：2026-11-19

## 1. 背景与目标（Intent & Why）

当前智能视图（Virtual Libraries）管理页提供了一个写死在后端代码中的「补充 9 个预设视图」按钮（`POST /api/admin/virtual_libraries/presets`）。随着 TMDB 深度刮削字段（`metadata_version >= 2`，涵盖语言、投票人数、热度、片长、剧集状态、本地收齐、出品公司、系列合集、关键词、演职员、播放平台、内容分级与数字发行日）全面就绪，固定预设已无法满足不同片库的个性化编排需求。

本设计移除原有的「补充 9 个预设视图」功能，改为**标准化的智能视图导入与导出机制**：
1. 用户可将当前所有智能视图（含视图配置、`VirtualLibraryRule` 筛选规则、默认/内置封面、动态拼贴封面及自定义静态封面图）一键导出为 `.zip` 配置包。
2. 导出的 `.zip` 包内自动附带一份 `AI-GUIDE.md`（包含当前已采集的 TMDB 字段字典、18 个场景化基础视图参考模板、合法封面枚举及当前库内已刮削的出品公司/网络/关键词 ID 速查表）。
3. 用户通过与 AI 对话生成或修改 `.zip` / `.json` 文件后，在管理后台通过**「文件解析预览 → 确认合并/清理策略 → 事务性导入」**流程直接应用。

---

## 2. 目标、非目标与反目标

### Goals（目标）
- **移除旧预设**：删除前端「补充 9 个预设视图」按钮与相关状态，移除后端 `POST /api/admin/virtual_libraries/presets` 路由与实现。
- **支持三种封面形态的导入与导出（扩展方案）**：
  1. **默认与内置封面**：留空 `""`（按名称/类型自动推导）或指定 12 款内置封面键（`vlib_top_rated`、`vlib_streaming`、`vlib_chinese`、`vlib_kr_jp`、`vlib_latest`、`vlib_default_movies`、`vlib_default_tv`、`lib_movie`、`lib_tv`、`lib_anime`、`lib_doc`、`lib_variety`）。
  2. **动态视图封面**：`"auto"`（动态首图）及 4 款动态海报拼贴（`"collage_cards"`、`"collage_diagonal"`、`"collage_hero"`、`"collage_blended"`）。
  3. **自定义静态封面图**：允许在 `.zip` 包的 `covers/` 目录下放置自定义图片（`.jpg` / `.jpeg` / `.png` / `.webp`），导入时经图像解码校验后持久化至 `{image_cache_dir}/custom_covers/`，并在管理端、Emby 客户端和后续导出 `.zip` 时完整保留。
- **两步式预览确认导入（方式 A）**：
  - 选择 `.zip` 或 `.json` 文件后，先调用预览接口完成全量规则、封面引用与图像解码校验，弹窗展示识别到的视图清单（标注「新增」或「同名更新」、封面类型与规则摘要）。
  - 弹窗提供开关**「同时清理导入包中未包含的旧视图」**（默认关闭），确认后在单事务内原子生效。
- **AI 对话开箱即用**：导出包内嵌 `AI-GUIDE.md`，汇总 TMDB 已采集字段、规则 JSON Schema、6 大类 18 个基础智能视图参考配置与本地动态 ID 列表。

### Non-Goals（非目标）
- 不支持 `.rar` / `.7z` 等私有或需 C/C++ FFI 动态库的压缩格式（保持 NAS 静态交叉编译能力）。上传 `.rar` 时在前端和后端给出明确提示，引导转换为 `.zip` 或直接上传 `.json`。
- 不涉及用户账号列表、播放历史或媒体刮削主表数据的导入导出。

### Anti-Goals（反目标）
- **禁止部分成功（Partial Commit）**：导入包中若有任何一个视图规则或封面不合法，整包拒绝并定位到具体视图序号与名称报错，绝不写入半截数据。
- **禁止破坏已有用户权限**：同名视图更新时必须保留原有的 `virtual_libraries.id`，从而使 `users.allowed_categories` 中的 `lib_vlib_{id}` 授权继续有效。
- **禁止自定义封面被 LRU 缓存清理误删**：自定义静态封面独立存放在 `{image_cache_dir}/custom_covers/`，不写入 `image_cache_index` LRU 淘汰表。

---

## 3. 方案对比与选型（Approaches）

| 维度 | 方案 1：仅配置 JSON 导入导出（轻量方案） | 方案 2：`.zip` 扩展包 + `.json` 双支持 + 预览确认（已选定） | 方案 3：在线 AI 接口直连生成 |
| :--- | :--- | :--- | :--- |
| **封面能力** | 仅支持内置封面与动态拼贴键 | 同时支持内置封面、动态拼贴及 `covers/` 自定义静态封面图打包 | 仅支持内置与动态封面 |
| **AI 协作体验** | 需额外复制提示词文档 | 导出 `.zip` 自带 `views.json` + `AI-GUIDE.md` + `covers/`，可直接丢给任意 AI 对话；AI 回复的 `.json` 或 `.zip` 均可直接导入 | 需在 NAS 配置大模型 API Key，增加网络与费用复杂度 |
| **安全性与可控性** | 无预览，直接覆盖 | 先预览新增/更新列表，支持按需勾选清理旧视图 | AI 直接改库风险较高 |

**选定方案**：方案 2（扩展封面方案 + 方式 A 预览确认弹窗）。

---

## 4. 详细设计

### 4.1 导出包与导入文件格式规范

#### 1. `.zip` 压缩包目录结构
```text
rx-emby-smart-views-YYYYMMDD.zip
├── views.json          # 智能视图配置清单（UTF-8 JSON）
├── AI-GUIDE.md         # TMDB 字段字典、规则契约、18 个基础参考视图与本地实体 ID 对照表
└── covers/             # 可选：自定义静态封面目录
    └── custom_<16hex>.jpg  # 或用户放入的任意名称图片（如 my_scifi.png）
```

#### 2. `views.json` 格式契约
兼容两种顶层 JSON 结构（方便 AI 直接输出）：
- **标准对象格式**（导出默认格式）：
  ```json
  {
    "version": 1,
    "exported_at": "2026-11-19T07:00:00Z",
    "views": [
      {
        "name": "万人高分电影榜",
        "icon_type": "movies",
        "sort_order": 10,
        "is_enabled": true,
        "cover_image": "collage_cards",
        "filter_rules": {
          "media_type": "movie",
          "min_rating": 8.0,
          "min_votes": 1000,
          "sort_by": "CommunityRating"
        }
      }
    ]
  }
  ```
- **简写数组格式**（AI 直接生成 `[ { "name": ... } ]` 时同样支持解析）：顶层直接为 ` Vec<ImportViewEntry> `。

#### 3. 单个视图条目字段校验规则（`ImportViewEntry`）
- `name`（必填）：去除首尾空白后长度 1～64 字符。**同一导入包内禁止出现重复的 `name`**（按 trim 后精确匹配）。
- `icon_type`（选填，默认 `"tvshows"`）：支持 `"movies"`、`"tvshows"`，并兼容 AI 常见的 `"movie"` → `"movies"`、`"tv"` → `"tvshows"` 别名自动归一化（Builder Call：提升 AI 生成容错率）。
- `sort_order`（选填）：若未提供，按其在数组中的顺序自动赋值为 `(index + 1) * 10`。
- `is_enabled`（选填，默认 `true`）：布尔值。
- `cover_image`（选填，默认 `None`）：
  - 空字符串 `""` 或 `null`：默认封面（按名称与 `icon_type` 自动推导内置图）。
  - 内置封面键：`vlib_top_rated`、`vlib_streaming`、`vlib_chinese`、`vlib_kr_jp`、`vlib_latest`、`vlib_default_movies`、`vlib_default_tv`、`lib_movie`、`lib_tv`、`lib_anime`、`lib_doc`、`lib_variety`。
  - 动态封面键：`auto`、`collage_cards`、`collage_diagonal`、`collage_hero`、`collage_blended`。
  - 自定义静态封面：
    - 若在 `.zip` 包内引用 `"covers/xxx.png"` 或 `"xxx.png"`（对应 zip 内 `covers/` 下的文件），后端读取该图片字节，使用 `image` 库解码校验（仅允许 JPEG/PNG/WebP，宽高不超过 4096×4096，最大比例缩放至宽 1280 以内并统一编码为 JPEG），计算内容哈希 `hash = xxh3_64(&jpeg_bytes)`，生成规范键名 `custom_{hash:016x}`（对应磁盘文件 `{image_cache_dir}/custom_covers/custom_{hash:016x}.jpg`）。
    - 若引用已存在于服务器磁盘 `{image_cache_dir}/custom_covers/custom_{16hex}.jpg` 的 `custom_{16hex}` 键（例如导出 `.json` 后再次导入 `.json`），直接校验磁盘文件存在性并复用。
    - 其他未知 `cover_image` 值一律报错拒绝。
- `filter_rules`（选填，默认空规则 `{}`）：复用 `VirtualLibraryRule`（`#[serde(default, deny_unknown_fields)]`）及 `rule.validate()`。

---

### 4.2 自定义静态封面存储、服务与清理机制

1. **存储路径**：`{image_cache_dir}/custom_covers/custom_{16hex}.jpg`
   - 数据库 `virtual_libraries.cover_image` 存储值为 `"custom_{16hex}"`（不带路径前缀与扩展名，彻底杜绝路径穿越）。
2. **图片读取服务**：
   - **管理后台预览（`GET /covers/*path`）**：在 `src/web/mod.rs` 的 `serve_covers` 中，若请求路径匹配 `^custom_[0-9a-f]{16}\.jpg$`，从 `{image_cache_dir}/custom_covers/` 读取并返回 `image/jpeg`；否则继续走 `WebAssets` 内嵌资源。
   - **Emby 客户端封面（`GET /Items/lib_vlib_{id}/Images/Primary`）**：在 `src/web/emby_api/images.rs` 的 `serve_library_cover` 中，若 `custom_cover` 匹配 `^custom_[0-9a-f]{16}$`，优先从 `{image_cache_dir}/custom_covers/{custom_cover}.jpg` 读取字节返回（若文件缺失则优雅回退至 `fallback_cover_key` 内置封面）。
   - **前端封面辅助函数（`frontend/src/utils/smartViewLabels.js`）**：
     - 新增 `isCustomCover(val)`：判断 `typeof val === 'string' && /^custom_[0-9a-f]{16}$/.test(val)`。
     - `coverImageUrl` 对 `custom_*` 仍走 `/covers/${key}.jpg?_t=${bust}`，无需改动调用方；在卡片左下角及编辑弹窗中显示「自定义封面」徽章。
3. **孤儿封面清理**：
   - 当删除智能视图、修改封面或执行带清理的导入后，后台可按需扫描 `{image_cache_dir}/custom_covers/`，删除不再被任何 `virtual_libraries.cover_image` 引用的 `custom_*.jpg` 文件。

---

### 4.3 后端 API 设计

在 `src/web/admin_api/mod.rs` 与 `src/web/admin_api/virtual_libraries.rs` 中：

1. **移除路由**：
   - 删除 `.route("/api/admin/virtual_libraries/presets", post(virtual_libraries::seed_preset_virtual_libraries))`
2. **新增导出路由**：
   - `GET /api/admin/virtual_libraries/export`
   - **行为**：
     - 查询 `virtual_libraries` 全表（按 `sort_order ASC, id ASC`）及当前库内已刮削的 facet 选项（networks / companies / collections / keywords / directors / cast / watch_providers）。
     - 在内存中构建 `.zip`：
       1. 写入 `views.json`（格式化缩进 JSON）。
       2. 写入 `AI-GUIDE.md`（包含已采集的 TMDB 字段总表、全部 `VirtualLibraryRule` 字段说明、封面枚举说明、6 大类 18 个基础参考视图 JSON 示例、以及当前媒体库已刮削的实体 ID 列表）。
       3. 遍历所有 `cover_image` 为 `custom_{16hex}` 的视图，若 `{image_cache_dir}/custom_covers/custom_{16hex}.jpg` 存在，将其写入 zip 的 `covers/custom_{16hex}.jpg`，同时在导出的 `views.json` 中将该视图的 `cover_image` 写为 `"covers/custom_{16hex}.jpg"`，使压缩包自包含、直观易懂。
     - 返回 `Content-Type: application/zip`、`Content-Disposition: attachment; filename="rx-emby-smart-views.zip"`。
3. **新增导入预览路由**：
   - `POST /api/admin/virtual_libraries/import/preview`
   - **请求体**：原始二进制流（`Bytes`，支持 `.zip` 或 `.json` 文件内容，路由层设置 `DefaultBodyLimit::max(16 * 1024 * 1024)`）。自动通过文件魔数（`PK\x03\x04` 识别 `.zip`，`{` 或 `[` 识别 `.json`；若为 `Rar!` 魔数则直接返回 `400 Bad Request`："暂不支持 .rar 格式，请使用 .zip 压缩包或 .json 配置文件"）。
   - **行为**：
     - 解析并校验所有视图条目与 zip 内的自定义图片（不落盘、不改数据库）。
     - 与数据库现有 `virtual_libraries` 按 `name` 对照。
     - **响应 JSON**：
       ```json
       {
         "total_count": 5,
         "create_count": 3,
         "update_count": 2,
         "custom_cover_count": 1,
         "unmatched_existing_count": 4,
         "items": [
           {
             "name": "万人高分电影榜",
             "action": "update",
             "icon_type": "movies",
             "sort_order": 10,
             "is_enabled": true,
             "cover_type": "dynamic",
             "cover_label": "立体叠放卡片",
             "filter_rules": { "media_type": "movie", "min_rating": 8.0, "min_votes": 1000 }
           }
         ]
       }
       ```
4. **新增导入执行路由**：
   - `POST /api/admin/virtual_libraries/import/apply?prune_missing=false`
   - **请求体**：与 `preview` 相同的原始文件二进制流（`Bytes`），通过 Query 参数 `prune_missing: bool` 控制是否删除未包含在导入包中的既有视图。
   - **行为**：
     - 执行与 `preview` 完全一致的解析与校验。
     - 先将所有新解码的自定义封面原子写入 `{image_cache_dir}/custom_covers/custom_{16hex}.jpg`。
     - 开启 PostgreSQL 事务（加 `pg_advisory_xact_lock(728314)` 防止并发导入冲突）：
       - 对同名视图执行 `UPDATE virtual_libraries SET icon_type=$1, sort_order=$2, filter_rules=$3, is_enabled=$4, cover_image=$5, cover_revision=cover_revision+1 WHERE id=$6`。
       - 对新视图执行 `INSERT INTO virtual_libraries ...`。
       - 若 `prune_missing == true`，删除名字不在本次导入集合中的旧视图，并同步从 `users.allowed_categories` 中移除被删视图对应的 `lib_vlib_{id}`。
     - 提交事务，清理孤儿自定义封面文件，失效 `enabled_vlibs_cache`、`admin_stats_cache`、`image_meta_cache` 与 `collage_cache`。
     - 返回 `{"status": "ok", "created": X, "updated": Y, "deleted": Z, "message": "..."}`。

---

### 4.4 安全与防呆边界（Sharp-Edges 防护）

| 边界项 | 限制值 / 策略 | 错误文案（UX Writing 规范） |
| :--- | :--- | :--- |
| 上传包体上限 | `16 MiB` | `导入文件不能超过 16 MB，请精简封面图片大小后重试` |
| `.rar` 格式拦截 | 检测 `Rar!\x1A\x07` 魔数或 `.rar` 扩展名 | `暂不支持 .rar 格式，请上传 .zip 压缩包或 .json 配置文件` |
| `views.json` 解压上限 | `Read::take(1 MiB)` | `配置文件 views.json 超过 1 MB 上限` |
| 单张自定义封面上限 | `Read::take(4 MiB)`，像素 ≤ 4096×4096 | `视图「{name}」的封面图片「{file}」无法解析或尺寸过大（需为 4MB 内、4096px 内的 JPG/PNG/WebP）` |
| 视图数量上限 | 单次导入 `1..=100` 个视图 | `导入包中未包含任何智能视图` / `单次最多导入 100 个智能视图` |
| 包内重名拦截 | 导入包内 `name.trim()` 唯一 | `导入包内存在重复的视图名称：「{name}」，请保持名称唯一` |
| 规则或封面校验失败 | 定位到具体序号与名称 | `第 {idx} 个视图「{name}」配置有误：{reason}` |

---

### 4.5 前端交互与文案设计（UX Writing & Copy Editing）

1. **顶部操作栏（`VirtualLibrariesTab.vue`）**：
   - 移除「补充 9 个预设视图」按钮。
   - 新增两个语义恒定的按钮（不论列表是否为空，文案保持一致，消除“有视图后去哪下载 AI 模板”的认知断裂，同时明确操作对象是“视图配置”而非“影视文件”）：
     - **「下载配置与 AI 模板」**（图标：`Download`，title：`打包下载当前视图规则、自定义封面及 AI 编写模板 (.zip)，可直接发给 AI 生成新配置`）
     - **「导入视图配置」**（图标：`Upload`，title：`上传由 AI 生成或备份的视图配置（支持 .zip 或 .json）`，点击打开导入预览弹窗）
2. **列表空状态文案更新**：
   - 标题：`尚未配置智能视图`
   - 说明：`点击右上角「下载配置与 AI 模板」发给 AI 批量生成视图，再通过「导入视图配置」一键生效；也可以直接点击「新建智能视图」手动创建。`
3. **导入预览确认弹窗（`VirtualLibraryImportModal.vue`）**：
   - **标题**：`预览并导入视图配置`
   - **文件选择与 AI 模板快捷入口**：
     - 弹窗内支持选择或重新选择 `.zip` / `.json` 配置文件。
     - 底部常驻辅助提示与快捷下载链接：`还没有配置文件？` **[下载当前配置与 AI 编写模板 (.zip)]** `，发给 AI 对话即可生成。`
   - **解析摘要栏**：`已识别「{fileName}」中的 {total} 项视图配置（新增 {create_count} 项，同名更新 {update_count} 项{custom_cover_count ? `，含 ${custom_cover_count} 张自定义封面` : ''}）`
   - **视图预览列表**：逐行展示视图名称、`新增`（翠绿徽章）或 `同名更新`（琥珀/靛蓝徽章）、展示样式（电影/剧集）、封面类型标签（如 `动态拼贴` / `自定义封面` / `内置封面`）以及用 `ruleBadges()` 渲染的规则摘要。
   - **清理旧视图开关**（仅当 `unmatched_existing_count > 0` 时可勾选）：
     - 复选框文案：`同时移除本次配置中未包含的 {unmatched_existing_count} 个旧视图`
     - 辅助提示：`默认仅新增或更新同名视图；勾选后将彻底删除其余旧视图。`
   - **底部按钮**：`取消` / `确认应用配置（{total} 项）`（主 CTA 使用明确动词 + 结果数量）。

---

### 4.6 整理沉淀：TMDB 已采集信息与智能视图参考文档

同步在 `docs/TMDB-SMART-VIEW-FIELD-RESEARCH.md`（或内置于导出包的 `AI-GUIDE.md` 生成模板中）固化本次整理的：
1. **8 大类已刮削 TMDB 与本地联动字段字典**（含 JSON 键名、类型、取值范围与注意事项）。
2. **6 大场景 18 个开箱即用的基础智能视图参考配置**（口碑热度榜、追剧进度与囤剧、时长与观影场景、流媒体厂牌与系列、语种与地域风味、4K/杜比视界发烧友专题），既供用户查阅，也可在当前库内尚未创建任何视图时作为 `export` 导出包中的示例参考。

---

## 5. 留给实现者（Builder）的工程细节
- `AI-GUIDE.md` 的 Markdown 模板可以拆分为独立模块（如 `src/web/admin_api/smart_view_bundle.rs`），避免 `virtual_libraries.rs` 体积膨胀。
- 导入预览与导入执行共用同一个纯内存解析函数 `parse_and_validate_import_bundle(bytes: &[u8], custom_covers_dir: &Path) -> Result<ParsedImportBundle, String>`，确保预览与执行校验逻辑 100% 一致。
