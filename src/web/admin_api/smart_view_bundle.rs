use crate::{smart_views::VirtualLibraryRule, web::emby_api::is_builtin_cover_key};
use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, GenericImageView};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Read, Write},
    path::Path,
};

const MAX_VIEWS_JSON_BYTES: usize = 1024 * 1024;
const MAX_CUSTOM_COVER_BYTES: usize = 4 * 1024 * 1024;
const MAX_ZIP_ENTRIES: usize = 128;
const MAX_IMPORT_VIEWS: usize = 100;
const MAX_IMAGE_DIM: u32 = 4096;
const MAX_COVER_WIDTH: u32 = 1280;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportableView {
    pub name: String,
    pub icon_type: String,
    pub sort_order: i32,
    pub is_enabled: bool,
    pub cover_image: Option<String>,
    pub filter_rules: VirtualLibraryRule,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidatedImportView {
    pub name: String,
    pub icon_type: String,
    pub sort_order: i32,
    pub is_enabled: bool,
    pub cover_image: Option<String>,
    pub cover_type: &'static str,
    pub cover_label: String,
    pub filter_rules: VirtualLibraryRule,
}

#[derive(Debug, Clone)]
pub struct ParsedImportBundle {
    pub views: Vec<ValidatedImportView>,
    pub custom_covers: HashMap<String, Vec<u8>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawImportView {
    name: String,
    #[serde(default)]
    icon_type: Option<String>,
    #[serde(default)]
    sort_order: Option<i32>,
    #[serde(default = "default_true")]
    is_enabled: bool,
    #[serde(default)]
    cover_image: Option<String>,
    #[serde(default)]
    filter_rules: VirtualLibraryRule,
}

#[derive(Debug, Deserialize)]
struct RawImportManifest {
    #[allow(dead_code)]
    #[serde(default)]
    version: Option<u32>,
    #[allow(dead_code)]
    #[serde(default)]
    exported_at: Option<String>,
    views: Vec<RawImportView>,
}

fn default_true() -> bool {
    true
}

pub fn is_custom_cover_key(key: &str) -> bool {
    key.strip_prefix("custom_").is_some_and(|hex| {
        hex.len() == 16
            && hex
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

pub fn builtin_cover_label(key: &str) -> Option<&'static str> {
    match key {
        "vlib_top_rated" => Some("高分佳作"),
        "vlib_streaming" => Some("流媒体剧场"),
        "vlib_chinese" => Some("华语热播"),
        "vlib_kr_jp" => Some("日韩精选"),
        "vlib_latest" => Some("最新院线"),
        "vlib_default_movies" => Some("电影视图"),
        "vlib_default_tv" => Some("剧集视图"),
        "lib_movie" => Some("影院胶片"),
        "lib_tv" => Some("电视光影"),
        "lib_anime" => Some("二次元星空"),
        "lib_doc" => Some("自然山海"),
        "lib_variety" => Some("舞台灯光"),
        _ => None,
    }
}

pub fn dynamic_cover_label(key: &str) -> Option<&'static str> {
    match key {
        "auto" => Some("动态首图"),
        "collage_cards" => Some("立体叠放卡片"),
        "collage_diagonal" => Some("斜切电影条带"),
        "collage_hero" => Some("1主3辅非对称"),
        "collage_blended" => Some("羽化融合4连"),
        _ => None,
    }
}

fn read_bounded<R: Read>(reader: R, max_bytes: usize, err_msg: &str) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    reader
        .take((max_bytes + 1) as u64)
        .read_to_end(&mut buf)
        .map_err(|e| format!("读取数据失败：{}", e))?;
    if buf.len() > max_bytes {
        return Err(err_msg.to_string());
    }
    Ok(buf)
}

fn normalize_zip_cover_name(raw_path: &str) -> Option<String> {
    let clean = raw_path.trim().trim_start_matches("./");
    if clean.is_empty()
        || clean.contains("..")
        || clean.contains('\\')
        || clean.starts_with('/')
        || clean.contains("//")
    {
        return None;
    }
    let fname = clean.strip_prefix("covers/").unwrap_or(clean);
    if fname.is_empty() || fname.contains('/') || fname.starts_with('.') {
        return None;
    }
    Some(fname.to_string())
}

fn decode_and_normalize_cover_image(
    raw_bytes: &[u8],
    view_name: &str,
    file_label: &str,
) -> Result<(String, Vec<u8>), String> {
    let img = image::load_from_memory(raw_bytes).map_err(|_| {
        format!(
            "视图「{}」的封面图片「{}」无法解析或尺寸过大（需为 4MB 内、4096px 内的 JPG/PNG/WebP）",
            view_name, file_label
        )
    })?;
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 || w > MAX_IMAGE_DIM || h > MAX_IMAGE_DIM {
        return Err(format!(
            "视图「{}」的封面图片「{}」无法解析或尺寸过大（需为 4MB 内、4096px 内的 JPG/PNG/WebP）",
            view_name, file_label
        ));
    }
    let resized = if w > MAX_COVER_WIDTH {
        let target_h = ((h as u64 * MAX_COVER_WIDTH as u64) / w as u64).max(1) as u32;
        img.resize_exact(MAX_COVER_WIDTH, target_h, FilterType::Lanczos3)
    } else {
        img
    };
    let rgb = resized.to_rgb8();
    let mut jpeg_bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut jpeg_bytes, 88);
    encoder
        .encode_image(&rgb)
        .map_err(|e| format!("视图「{}」的封面图片编码失败：{}", view_name, e))?;
    let hash = xxhash_rust::xxh3::xxh3_64(&jpeg_bytes);
    let key = format!("custom_{:016x}", hash);
    Ok((key, jpeg_bytes))
}

fn parse_views_json(json_bytes: &[u8]) -> Result<Vec<RawImportView>, String> {
    if json_bytes.len() > MAX_VIEWS_JSON_BYTES {
        return Err("配置文件 views.json 超过 1 MB 上限".into());
    }
    let text = std::str::from_utf8(json_bytes)
        .map_err(|_| "配置文件不是有效的 UTF-8 文本，请上传 .zip 压缩包或 .json 配置文件".to_string())?;
    let trimmed = text.trim_start_matches('\u{feff}').trim();
    if trimmed.starts_with('[') {
        serde_json::from_str::<Vec<RawImportView>>(trimmed)
            .map_err(|e| format!("解析视图配置 JSON 数组失败：{}", e))
    } else if trimmed.starts_with('{') {
        serde_json::from_str::<RawImportManifest>(trimmed)
            .map(|m| m.views)
            .map_err(|e| format!("解析视图配置 JSON 失败：{}", e))
    } else {
        Err("无法识别的文件格式，请上传 .zip 压缩包或 .json 配置文件".into())
    }
}

pub fn parse_and_validate_import_bundle(
    bytes: &[u8],
    custom_covers_dir: &Path,
) -> Result<ParsedImportBundle, String> {
    if bytes.is_empty() {
        return Err("上传的文件为空，请选择有效的 .zip 或 .json 配置文件".into());
    }
    if bytes.starts_with(b"Rar!\x1A\x07") {
        return Err("暂不支持 .rar 格式，请上传 .zip 压缩包或 .json 配置文件".into());
    }

    let mut zip_cover_entries: HashMap<String, Vec<u8>> = HashMap::new();
    let raw_views = if bytes.starts_with(b"PK\x03\x04") {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| format!("解析 .zip 压缩包失败：{}", e))?;
        if archive.len() > MAX_ZIP_ENTRIES {
            return Err(format!("压缩包内文件数量超过 {} 个上限", MAX_ZIP_ENTRIES));
        }
        let mut views_json_bytes: Option<Vec<u8>> = None;
        for i in 0..archive.len() {
            let file = archive
                .by_index(i)
                .map_err(|e| format!("读取压缩包条目失败：{}", e))?;
            if file.is_dir() {
                continue;
            }
            let name = file.name().trim_start_matches("./").to_string();
            if name.contains("..") || name.contains('\\') || name.starts_with('/') {
                return Err(format!("压缩包包含非法路径：{}", name));
            }
            if name == "views.json" || (views_json_bytes.is_none() && !name.contains('/') && name.ends_with(".json")) {
                let data = read_bounded(
                    file,
                    MAX_VIEWS_JSON_BYTES,
                    "配置文件 views.json 超过 1 MB 上限",
                )?;
                views_json_bytes = Some(data);
            } else if let Some(stripped) = name.strip_prefix("covers/") {
                if let Some(clean_fname) = normalize_zip_cover_name(stripped) {
                    let data = read_bounded(
                        file,
                        MAX_CUSTOM_COVER_BYTES,
                        &format!("封面图片「{}」超过 4 MB 上限", clean_fname),
                    )?;
                    zip_cover_entries.insert(clean_fname, data);
                }
            }
        }
        let json_bytes = views_json_bytes
            .ok_or_else(|| "压缩包内未找到 views.json 配置文件".to_string())?;
        parse_views_json(&json_bytes)?
    } else {
        parse_views_json(bytes)?
    };

    if raw_views.is_empty() {
        return Err("导入包中未包含任何智能视图".into());
    }
    if raw_views.len() > MAX_IMPORT_VIEWS {
        return Err(format!("单次最多导入 {} 个智能视图", MAX_IMPORT_VIEWS));
    }

    let mut seen_names = HashSet::new();
    let mut validated_views = Vec::with_capacity(raw_views.len());
    let mut custom_covers = HashMap::new();

    for (idx, item) in raw_views.into_iter().enumerate() {
        let display_idx = idx + 1;
        let name = item.name.trim().to_string();
        if name.is_empty() || name.chars().count() > 64 {
            return Err(format!(
                "第 {} 个视图名称须为 1～64 个字符",
                display_idx
            ));
        }
        if !seen_names.insert(name.clone()) {
            return Err(format!(
                "导入包内存在重复的视图名称：「{}」，请保持名称唯一",
                name
            ));
        }

        let icon_type = match item
            .icon_type
            .as_deref()
            .map(str::trim)
            .unwrap_or("tvshows")
            .to_lowercase()
            .as_str()
        {
            "movies" | "movie" => "movies".to_string(),
            "tvshows" | "tv" | "" => "tvshows".to_string(),
            other => {
                return Err(format!(
                    "第 {} 个视图「{}」配置有误：不支持的展示样式「{}」（可选 movies 或 tvshows）",
                    display_idx, name, other
                ))
            }
        };

        let sort_order = item.sort_order.unwrap_or((display_idx as i32) * 10);

        if let Err(rule_err) = item.filter_rules.validate() {
            return Err(format!(
                "第 {} 个视图「{}」配置有误：{}",
                display_idx, name, rule_err
            ));
        }

        let raw_cover = item
            .cover_image
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let (cover_image, cover_type, cover_label) = match raw_cover {
            None => (None, "default", "默认封面".to_string()),
            Some(c) if is_builtin_cover_key(c) => (
                Some(c.to_string()),
                "builtin",
                builtin_cover_label(c).unwrap_or("内置封面").to_string(),
            ),
            Some(c) if dynamic_cover_label(c).is_some() => (
                Some(c.to_string()),
                "dynamic",
                dynamic_cover_label(c).unwrap_or("动态封面").to_string(),
            ),
            Some(c) if is_custom_cover_key(c) => {
                let disk_file = custom_covers_dir.join(format!("{}.jpg", c));
                let zip_fname = format!("{}.jpg", c);
                if let Some(raw_img) = zip_cover_entries.get(&zip_fname) {
                    let (key, jpg) = decode_and_normalize_cover_image(raw_img, &name, &zip_fname)
                        .map_err(|e| format!("第 {} 个视图「{}」配置有误：{}", display_idx, name, e))?;
                    custom_covers.insert(key.clone(), jpg);
                    (Some(key), "custom", "自定义封面".to_string())
                } else if disk_file.is_file() {
                    (Some(c.to_string()), "custom", "自定义封面".to_string())
                } else {
                    return Err(format!(
                        "第 {} 个视图「{}」配置有误：未找到自定义封面「{}」，请上传包含 covers/ 目录的 .zip 压缩包",
                        display_idx, name, c
                    ));
                }
            }
            Some(c) => {
                let Some(fname) = normalize_zip_cover_name(c) else {
                    return Err(format!(
                        "第 {} 个视图「{}」配置有误：非法封面路径或未知封面标识「{}」",
                        display_idx, name, c
                    ));
                };
                if let Some(raw_img) = zip_cover_entries.get(&fname) {
                    let (key, jpg) = decode_and_normalize_cover_image(raw_img, &name, &fname)
                        .map_err(|e| format!("第 {} 个视图「{}」配置有误：{}", display_idx, name, e))?;
                    custom_covers.insert(key.clone(), jpg);
                    (Some(key), "custom", format!("自定义封面 ({})", fname))
                } else {
                    let maybe_stem = fname.strip_suffix(".jpg").unwrap_or(&fname);
                    if is_custom_cover_key(maybe_stem)
                        && custom_covers_dir.join(format!("{}.jpg", maybe_stem)).is_file()
                    {
                        (
                            Some(maybe_stem.to_string()),
                            "custom",
                            "自定义封面".to_string(),
                        )
                    } else {
                        return Err(format!(
                            "第 {} 个视图「{}」配置有误：未在压缩包 covers/ 目录中找到封面图片「{}」",
                            display_idx, name, c
                        ));
                    }
                }
            }
        };

        validated_views.push(ValidatedImportView {
            name,
            icon_type,
            sort_order,
            is_enabled: item.is_enabled,
            cover_image,
            cover_type,
            cover_label,
            filter_rules: item.filter_rules,
        });
    }

    Ok(ParsedImportBundle {
        views: validated_views,
        custom_covers,
    })
}

pub fn build_smart_view_export_zip(
    views: &[ExportableView],
    facets: &Value,
    custom_covers_dir: &Path,
) -> Result<Vec<u8>, String> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        let mut exported_views = Vec::with_capacity(views.len());
        let mut covers_to_pack: Vec<(String, Vec<u8>)> = Vec::new();
        let mut packed_keys = HashSet::new();

        for v in views {
            let mut copy = v.clone();
            if let Some(key) = v.cover_image.as_deref() {
                if is_custom_cover_key(key) {
                    let disk_path = custom_covers_dir.join(format!("{}.jpg", key));
                    if let Ok(bytes) = std::fs::read(&disk_path) {
                        copy.cover_image = Some(format!("covers/{}.jpg", key));
                        if packed_keys.insert(key.to_string()) {
                            covers_to_pack.push((format!("covers/{}.jpg", key), bytes));
                        }
                    }
                }
            }
            exported_views.push(copy);
        }

        let manifest = json!({
            "version": 1,
            "exported_at": chrono::Utc::now().to_rfc3339(),
            "views": exported_views
        });
        let views_json = serde_json::to_string_pretty(&manifest)
            .map_err(|e| format!("序列化 views.json 失败：{}", e))?;
        zw.start_file("views.json", opts)
            .map_err(|e| format!("写入 views.json 失败：{}", e))?;
        zw.write_all(views_json.as_bytes())
            .map_err(|e| format!("写入 views.json 失败：{}", e))?;

        let guide = render_ai_guide_markdown(facets);
        zw.start_file("AI-GUIDE.md", opts)
            .map_err(|e| format!("写入 AI-GUIDE.md 失败：{}", e))?;
        zw.write_all(guide.as_bytes())
            .map_err(|e| format!("写入 AI-GUIDE.md 失败：{}", e))?;

        for (entry_path, data) in covers_to_pack {
            zw.start_file(&entry_path, opts)
                .map_err(|e| format!("写入封面 {} 失败：{}", entry_path, e))?;
            zw.write_all(&data)
                .map_err(|e| format!("写入封面 {} 失败：{}", entry_path, e))?;
        }

        zw.finish()
            .map_err(|e| format!("封包 .zip 失败：{}", e))?;
    }
    Ok(buf.into_inner())
}

fn format_facet_sample(facets: &Value, key: &str, max_items: usize) -> String {
    let Some(arr) = facets.get(key).and_then(|v| v.as_array()) else {
        return "（当前库内暂无已刮削数据）".to_string();
    };
    if arr.is_empty() {
        return "（当前库内暂无已刮削数据）".to_string();
    }
    let items: Vec<String> = arr
        .iter()
        .take(max_items)
        .filter_map(|item| {
            let id = item.get("id")?.as_i64()?;
            let name = item.get("name")?.as_str()?;
            Some(format!("`{}` ({})", id, name))
        })
        .collect();
    if items.is_empty() {
        "（当前库内暂无已刮削数据）".to_string()
    } else if arr.len() > max_items {
        format!("{} …（共 {} 项）", items.join("、"), arr.len())
    } else {
        items.join("、")
    }
}

pub fn render_ai_guide_markdown(facets: &Value) -> String {
    let networks = format_facet_sample(facets, "networks", 60);
    let companies = format_facet_sample(facets, "companies", 60);
    let collections = format_facet_sample(facets, "collections", 60);
    let keywords = format_facet_sample(facets, "keywords", 80);
    let directors = format_facet_sample(facets, "directors", 60);
    let cast = format_facet_sample(facets, "cast", 60);
    let providers = format_facet_sample(facets, "watch_providers", 40);

    format!(
        r#"# rx-emby 智能视图配置与 AI 编写指南 (AI-GUIDE)

你可以将本文件（或整个 `.zip` 压缩包）发送给 AI 助手，让 AI 根据你的观影偏好生成 `views.json` 配置文件或包含 `covers/` 自定义封面的 `.zip` 压缩包，随后在 rx-emby 管理后台点击 **「导入视图配置」** 一键应用。

---

## 一、输出文件格式要求（供 AI 严格遵循）

1. 输出合法的 UTF-8 JSON（保存为 `views.json` 或打包在 `.zip` 根目录的 `views.json` 中）。
2. 顶层结构支持标准对象 `{{"version": 1, "views": [...]}}` 或直接输出视图数组 `[...]`。
3. **禁止编造任何未在下文列出的字段名**（后端开启了 `deny_unknown_fields` 强校验）。
4. 每个视图对象的顶层字段：
   - `name` (string, 必填): 视图名称（1～64 字符，同一包内不可重名）。
   - `icon_type` (string, 必填): `"movies"`（电影横向展示样式）或 `"tvshows"`（剧集展示样式）。
   - `sort_order` (int, 选填): 首页排序权重，越小越靠前（如 `10, 20, 30`）。
   - `is_enabled` (bool, 选填): 是否默认挂载至客户端首页（默认 `true`）。
   - `cover_image` (string, 选填): 封面配置，仅限以下 4 种取值方式：
     1. **默认自动匹配**：留空 `""` 或 `null`。
     2. **动态生成封面（推荐）**：
        - `"auto"`：动态首图（自动取匹配最高分作品剧照 + 标题腰封）
        - `"collage_cards"`：立体叠放卡片
        - `"collage_diagonal"`：斜切电影条带
        - `"collage_hero"`：1主3辅非对称拼贴
        - `"collage_blended"`：羽化融合4连海报
     3. **内置静态封面**：`"vlib_top_rated"`、`"vlib_streaming"`、`"vlib_chinese"`、`"vlib_kr_jp"`、`"vlib_latest"`、`"vlib_default_movies"`、`"vlib_default_tv"`、`"lib_movie"`、`"lib_tv"`、`"lib_anime"`、`"lib_doc"`、`"lib_variety"`。
     4. **自定义静态封面（需打包为 `.zip`）**：在 `.zip` 的 `covers/` 目录下放入 JPG/PNG/WebP 图片（≤ 4MB），并将 `cover_image` 设为 `"covers/文件名.jpg"`。
   - `filter_rules` (object, 必填): 筛选规则对象，仅可使用第二节列出的字段。

---

## 二、目前已采集的 TMDB 信息与 `filter_rules` 完整字段字典

| 分类 | JSON 字段名 | 类型 | 取值范围与说明 |
| :--- | :--- | :--- | :--- |
| **1. 基础与文本** | `media_type` | string | `"movie"`（仅电影）或 `"tv"`（仅剧集） |
| | `categories` | string[] | 5 大推导分类：`["电影", "电视剧", "动漫", "纪录片", "综艺"]` |
| | `keyword` | string | 匹配片名、原名或目录名关键词 |
| | `search_overview` | bool | 配合 `keyword` 使用，`true` 时同时检索剧情简介 |
| | `sort_by` | string | 默认降序排序：`"PremiereDate"`（发行年份）、`"CommunityRating"`（评分）、`"Popularity"`（热度）、`"DateCreated"`（入库时间）、`"SortName"`（片名） |
| **2. 地区与流派** | `countries` | string[] | 制片国家/地区 ISO 代码，如 `["CN", "HK", "TW", "US", "JP", "KR", "GB", "FR"]` |
| | `genres` | string[] | 流派中文名，如 `["剧情", "喜剧", "动作", "科幻", "悬疑", "犯罪", "动画", "奇幻", "爱情", "惊悚", "冒险", "家庭", "战争", "历史", "恐怖", "纪录"]` |
| | `genre_ids` | int[] | TMDB 流派 ID 数组 |
| | `genres_all` | bool | `true` 表示所选流派必须全部同时满足（AND）；默认 `false`（任一满足 OR） |
| | `exclude_countries` | string[] | 排除的制片国家/地区 ISO 代码 |
| | `exclude_genres` | string[] | 排除的流派名称 |
| | `exclude_networks` | string[] | 排除的播出网络名称 |
| **3. 口碑、热度与语言** | `min_rating` / `max_rating` | float | TMDB 评分上下限（`0.0` ～ `10.0`） |
| | `min_votes` | int | 最低投票人数（如 `500` 或 `1000`，过滤冷门虚高片） |
| | `min_popularity` | float | 最低 TMDB 热度值 |
| | `languages` | string[] | 作品原作语言 ISO 639-1 代码，如 `["zh", "cn", "en", "ja", "ko", "fr", "de", "es", "th", "hi"]` |
| | `spoken_languages` | string[] | 对白语言 ISO 639-1 代码 |
| **4. 时长与发行日期** | `runtime_min` / `runtime_max` | int | 最短 / 最长时长（分钟，非负整数） |
| | `year_min` / `year_max` | int | 最早 / 最晚发行年份（如 `2024`） |
| | `premiered_after` / `premiered_before` | string | 首播日期区间，格式 `"YYYY-MM-DD"` |
| | `premiered_within_days` | int | 最近 N 天内首播（`1` ～ `36500`） |
| | `digital_within_days` | int | 最近 N 天内数字版上线（仅电影，对应 TMDB release_type=4） |
| **5. 剧集状态与收齐** | `statuses` | string[] | 剧集状态：`["Ended", "Returning Series", "Canceled", "In Production", "Planned"]` |
| | `tv_types` | string[] | 节目类型：`["Miniseries", "Scripted", "Reality", "Documentary", "Talk Show", "Animation"]` |
| | `seasons_min` / `seasons_max` | int | 最少 / 最多季数 |
| | `episodes_min` / `episodes_max` | int | 最少 / 最多总集数 |
| | `local_complete` | string | 本地收齐状态：`"aired"`（已播出集已收齐）或 `"all"`（全剧已完结且全集收齐） |
| | `aired_within_days` | int | 最近 N 天内有新集播出（`1` ～ `36500`） |
| | `airing_within_days` | int | 未来 N 天内有播出计划（`0` 表示今日播出，`7` 表示未来 7 天） |
| **6. 出品方与合集** | `networks` | string[] | 播出网络名称，如 `["Netflix", "HBO", "HBO Max", "Apple TV+", "Disney+", "Prime Video", "BBC", "tvN", "腾讯视频", "爱奇艺", "优酷", "哔哩哔哩"]` |
| | `network_ids` | int[] | 播出网络 TMDB ID（见第四节本地字典） |
| | `company_ids` | int[] | 制作公司 TMDB ID（见第四节本地字典） |
| | `collection_ids` | int[] | 电影系列合集 TMDB ID（见第四节本地字典） |
| | `has_collection` | bool | `true` 仅属于系列电影合集的作品；`false` 仅单部非合集电影 |
| **7. 题材、主创与分级** | `keyword_ids` | int[] | 题材关键词 TMDB ID（见第四节本地字典） |
| | `director_ids` | int[] | 导演 / 创作者 TMDB ID（见第四节本地字典） |
| | `person_ids` | int[] | 演员 / 导演 / 创作者 TMDB ID（见第四节本地字典） |
| | `watch_region` | string | 播放平台地区代码（如 `"US"`, `"HK"`, `"JP"`） |
| | `watch_types` | string[] | 观看方式：`["flatrate", "rent", "buy"]`（订阅 / 租赁 / 购买） |
| | `watch_provider_ids` | int[] | 播放平台 ID（见第四节本地字典） |
| | `cert_region` | string | 内容分级地区代码（如 `"US"`, `"HK"`） |
| | `certifications` | string[] | 内容分级，如 `["G", "PG", "PG-13", "R", "TV-G", "TV-14", "TV-MA", "IIA", "IIB", "III"]` |
| **8. 本地版本与嵌套分组** | `version_tags` | string[] | 同一物理文件同时满足的版本标签：`["4K", "1080p", "REMUX", "HDR", "杜比视界", "Atmos", "国语", "粤语", "中字"]` |
| | `added_within_days` | int | 最近 N 天内入库（`1` ～ `36500`） |
| | `any` | object[] | OR 条件子组数组（最多 3 层，每层最多 8 组），满足主条件后再满足 `any` 中任意一组 |

---

## 三、6 大场景 18 个基础智能视图参考模板

以下配置可直接复制到 `views.json` 的 `views` 数组中使用或按需组合：

```json
[
  {{
    "name": "万人高分电影榜",
    "icon_type": "movies",
    "sort_order": 10,
    "cover_image": "collage_cards",
    "filter_rules": {{ "media_type": "movie", "min_rating": 8.0, "min_votes": 1000, "sort_by": "CommunityRating" }}
  }},
  {{
    "name": "近期在库热门",
    "icon_type": "movies",
    "sort_order": 20,
    "cover_image": "auto",
    "filter_rules": {{ "min_popularity": 50.0, "year_min": 2024, "sort_by": "Popularity" }}
  }},
  {{
    "name": "高分冷门遗珠",
    "icon_type": "movies",
    "sort_order": 30,
    "cover_image": "collage_hero",
    "filter_rules": {{ "min_rating": 7.8, "min_votes": 100, "sort_by": "CommunityRating" }}
  }},
  {{
    "name": "已完结全集爽看",
    "icon_type": "tvshows",
    "sort_order": 40,
    "cover_image": "collage_cards",
    "filter_rules": {{ "media_type": "tv", "local_complete": "all", "sort_by": "CommunityRating" }}
  }},
  {{
    "name": "本周连载更新",
    "icon_type": "tvshows",
    "sort_order": 50,
    "cover_image": "auto",
    "filter_rules": {{ "media_type": "tv", "aired_within_days": 7, "sort_by": "PremiereDate" }}
  }},
  {{
    "name": "周末刷完 · 精品短剧",
    "icon_type": "tvshows",
    "sort_order": 60,
    "cover_image": "collage_diagonal",
    "filter_rules": {{ "media_type": "tv", "statuses": ["Ended"], "episodes_max": 12, "min_rating": 7.5, "sort_by": "CommunityRating" }}
  }},
  {{
    "name": "90 分钟轻松看",
    "icon_type": "movies",
    "sort_order": 70,
    "cover_image": "collage_blended",
    "filter_rules": {{ "media_type": "movie", "runtime_max": 90, "min_rating": 7.0, "sort_by": "CommunityRating" }}
  }},
  {{
    "name": "下饭短剧与喜剧",
    "icon_type": "tvshows",
    "sort_order": 80,
    "cover_image": "auto",
    "filter_rules": {{ "media_type": "tv", "runtime_max": 30, "genres": ["喜剧", "动画"] }}
  }},
  {{
    "name": "合家欢 · 少儿放心看",
    "icon_type": "movies",
    "sort_order": 90,
    "cover_image": "collage_cards",
    "filter_rules": {{ "genres": ["家庭", "动画"], "min_rating": 7.0 }}
  }},
  {{
    "name": "Netflix / HBO 精品剧场",
    "icon_type": "tvshows",
    "sort_order": 100,
    "cover_image": "vlib_streaming",
    "filter_rules": {{ "media_type": "tv", "networks": ["Netflix", "HBO", "HBO Max", "Apple TV+", "Disney+"] }}
  }},
  {{
    "name": "本月新上流媒体电影",
    "icon_type": "movies",
    "sort_order": 110,
    "cover_image": "vlib_latest",
    "filter_rules": {{ "media_type": "movie", "digital_within_days": 30, "sort_by": "PremiereDate" }}
  }},
  {{
    "name": "系列电影合集",
    "icon_type": "movies",
    "sort_order": 120,
    "cover_image": "collage_diagonal",
    "filter_rules": {{ "media_type": "movie", "has_collection": true, "sort_by": "PremiereDate" }}
  }},
  {{
    "name": "华语热播剧场",
    "icon_type": "tvshows",
    "sort_order": 130,
    "cover_image": "vlib_chinese",
    "filter_rules": {{ "media_type": "tv", "categories": ["电视剧"], "countries": ["CN", "HK", "TW"] }}
  }},
  {{
    "name": "日韩剧集精选",
    "icon_type": "tvshows",
    "sort_order": 140,
    "cover_image": "vlib_kr_jp",
    "filter_rules": {{ "media_type": "tv", "categories": ["电视剧"], "countries": ["KR", "JP"], "min_rating": 7.5 }}
  }},
  {{
    "name": "原声日语动漫",
    "icon_type": "tvshows",
    "sort_order": 150,
    "cover_image": "lib_anime",
    "filter_rules": {{ "categories": ["动漫"], "languages": ["ja"], "min_rating": 7.5, "sort_by": "CommunityRating" }}
  }},
  {{
    "name": "粤语怀旧与配音佳作",
    "icon_type": "movies",
    "sort_order": 160,
    "cover_image": "collage_hero",
    "filter_rules": {{ "any": [{{ "languages": ["cn"] }}, {{ "version_tags": ["粤语"] }}] }}
  }},
  {{
    "name": "4K 杜比视界 · 视听大片",
    "icon_type": "movies",
    "sort_order": 170,
    "cover_image": "collage_cards",
    "filter_rules": {{ "version_tags": ["4K", "杜比视界"], "min_rating": 7.5 }}
  }},
  {{
    "name": "原盘 REMUX 珍藏",
    "icon_type": "movies",
    "sort_order": 180,
    "cover_image": "lib_movie",
    "filter_rules": {{ "version_tags": ["REMUX"], "sort_by": "DateCreated" }}
  }}
]
```

---

## 四、当前媒体库已刮削的 TMDB 实体 ID 速查表

填写 `network_ids`、`company_ids`、`collection_ids`、`keyword_ids`、`director_ids`、`person_ids`、`watch_provider_ids` 时，可直接选用以下当前库内已存在的正整数 ID：

- **播出网络 (`network_ids`)**：{networks}
- **制作公司 (`company_ids`)**：{companies}
- **电影合集 (`collection_ids`)**：{collections}
- **题材关键词 (`keyword_ids`)**：{keywords}
- **导演 / 创作者 (`director_ids`)**：{directors}
- **主要演员 (`person_ids`)**：{cast}
- **地区播放平台 (`watch_provider_ids`)**：{providers}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_rar_magic_bytes_with_clear_copy() {
        let dir = std::env::temp_dir().join(format!("rx_test_{}", uuid::Uuid::new_v4().simple()));
        let rar_bytes = b"Rar!\x1A\x07\x00dummy";
        let err = parse_and_validate_import_bundle(rar_bytes, &dir).unwrap_err();
        assert_eq!(err, "暂不支持 .rar 格式，请上传 .zip 压缩包或 .json 配置文件");
    }

    #[test]
    fn normalizes_ai_shorthand_array_and_rejects_duplicate_names() {
        let dir = std::env::temp_dir().join(format!("rx_test_{}", uuid::Uuid::new_v4().simple()));
        let raw = r#"[
            {"name": " 万人高分电影榜 ", "icon_type": "movie", "cover_image": "collage_cards", "filter_rules": {"media_type": "movie", "min_rating": 8.0, "min_votes": 1000}},
            {"name": "已完结短剧", "icon_type": "tv", "cover_image": "auto", "filter_rules": {"media_type": "tv", "statuses": ["Ended"], "episodes_max": 12}}
        ]"#.as_bytes();
        let parsed = parse_and_validate_import_bundle(raw, &dir).unwrap();
        assert_eq!(parsed.views.len(), 2);
        assert_eq!(parsed.views[0].name, "万人高分电影榜");
        assert_eq!(parsed.views[0].icon_type, "movies");
        assert_eq!(parsed.views[0].sort_order, 10);
        assert_eq!(parsed.views[0].cover_type, "dynamic");
        assert_eq!(parsed.views[1].icon_type, "tvshows");
        assert_eq!(parsed.views[1].sort_order, 20);

        let dup = r#"[{"name": "高分"}, {"name": " 高分 "}]"#.as_bytes();
        let err = parse_and_validate_import_bundle(dup, &dir).unwrap_err();
        assert_eq!(err, "导入包内存在重复的视图名称：「高分」，请保持名称唯一");
    }

    #[test]
    fn rejects_unknown_cover_or_missing_zip_image_and_path_traversal() {
        let dir = std::env::temp_dir().join(format!("rx_test_{}", uuid::Uuid::new_v4().simple()));
        let bad_cover = r#"[{"name": "测试", "cover_image": "covers/missing.png"}]"#.as_bytes();
        let err = parse_and_validate_import_bundle(bad_cover, &dir).unwrap_err();
        assert!(err.contains("第 1 个视图「测试」配置有误"));

        let traversal = r#"[{"name": "越权", "cover_image": "covers/../../etc/passwd"}]"#.as_bytes();
        let err2 = parse_and_validate_import_bundle(traversal, &dir).unwrap_err();
        assert!(err2.contains("第 1 个视图「越权」配置有误"));
    }

    #[test]
    fn zip_roundtrip_bundles_custom_cover_and_ai_guide() {
        let dir = std::env::temp_dir().join(format!("rx_test_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut img = image::RgbImage::new(32, 18);
        for p in img.pixels_mut() {
            *p = image::Rgb([20, 180, 120]);
        }
        let mut png_bytes = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png)
            .unwrap();

        let mut zip_buf = std::io::Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut zip_buf);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("views.json", opts).unwrap();
            std::io::Write::write_all(
                &mut zw,
                r#"{"version":1,"views":[{"name":"自定义封面视图","icon_type":"movies","cover_image":"covers/my_banner.png","filter_rules":{"media_type":"movie"}}]}"#.as_bytes(),
            ).unwrap();
            zw.start_file("covers/my_banner.png", opts).unwrap();
            std::io::Write::write_all(&mut zw, &png_bytes).unwrap();
            zw.finish().unwrap();
        }
        let parsed = parse_and_validate_import_bundle(&zip_buf.into_inner(), &dir).unwrap();
        assert_eq!(parsed.views.len(), 1);
        let assigned_key = parsed.views[0].cover_image.clone().unwrap();
        assert!(is_custom_cover_key(&assigned_key));
        assert_eq!(parsed.views[0].cover_type, "custom");
        let jpg_bytes = parsed.custom_covers.get(&assigned_key).unwrap();
        std::fs::write(dir.join(format!("{assigned_key}.jpg")), jpg_bytes).unwrap();

        let exported_zip = build_smart_view_export_zip(
            &[ExportableView {
                name: "自定义封面视图".into(),
                icon_type: "movies".into(),
                sort_order: 10,
                is_enabled: true,
                cover_image: Some(assigned_key.clone()),
                filter_rules: VirtualLibraryRule::default(),
            }],
            &json!({"companies": [{"id": 42, "name": "吉卜力"}]}),
            &dir,
        )
        .unwrap();

        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&exported_zip)).unwrap();
        assert!(archive.by_name("AI-GUIDE.md").is_ok());
        assert!(archive.by_name(&format!("covers/{assigned_key}.jpg")).is_ok());
        let re_imported = parse_and_validate_import_bundle(&exported_zip, &dir).unwrap();
        assert_eq!(re_imported.views[0].cover_image.as_deref(), Some(assigned_key.as_str()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
