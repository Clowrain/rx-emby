use once_cell::sync::Lazy;
use percent_encoding::percent_decode;
use regex::Regex;
use std::collections::HashSet;
use xxhash_rust::xxh3::xxh3_64;

static TMDB_ID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\{tmdb-(\d+)\}").unwrap());
static YEAR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[\(\[（](19\d{2}|20\d{2})[\)\]）]|\b(19\d{2}|20\d{2})\b").unwrap());
static SEASON_EP_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:S(\d{1,3})E(\d{1,4})|(?:^|[^A-Za-z0-9])E(\d{1,4})(?:$|[^0-9])|第\s*(\d{1,4})\s*[集话期])").unwrap()
});
static SEASON_DIR_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:(?:^|[\s._\-\[\(])Season\s*(\d{1,3})(?:$|[\s._\-\]\)])|(?:^|[\s._\-\[\(])S0*(\d{1,3})(?:$|[\s._\-\]\)])|第\s*(\d{1,3})\s*季)").unwrap()
});
static NOISE_PREFIX_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(?:刮削失败|每日更新|临时目录|未分类|115|阿里云盘|夸克网盘)/+").unwrap());

static VIDEO_EXTS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "mp4", "mkv", "rmvb", "iso", "ts", "flv", "avi", "rm", "mov", "wmv", "webm", "m4v",
    ]
    .into_iter()
    .collect()
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedStrmRecord {
    /// 影视根目录唯一键（用于聚合同一部电影或电视剧）
    pub root_dir: String,
    /// 原始文件夹名称
    pub raw_folder_name: String,
    /// 清洗后的标准片名
    pub clean_title: String,
    /// 识别出的年份
    pub year: Option<i32>,
    /// 初步推断类型 ('movie' 或 'tv'，后续由 TMDB 元数据二次覆写)
    pub inferred_media_type: String,
    /// 初步分类 ('电影' / '电视剧' / '动漫' / '纪录片' / '综艺')
    pub inferred_category: String,
    /// 双端提取出的 TMDB ID
    pub tmdb_id: Option<i32>,

    /// 季号 (电影为 0)
    pub season_number: i32,
    /// 集号 (电影为 1)
    pub episode_number: i32,

    /// 剥离域名与 sign 后的纯净 AList 逻辑路径
    pub alist_path: String,
    /// alist_path 的 64 位 xxh3 哈希（数据库 BIGINT 映射为 i64）
    pub path_hash: i64,
    /// 真实视频文件名
    pub file_name: String,
    /// 封装格式 (mkv / mp4 等)
    pub container: String,
    /// 清洗或兜底生成的版本标签
    pub version_tag: String,
    /// 多版本排序权重 (4K/DV > 1080p > 未知)
    pub priority_score: i32,
}

/// 解析单行 strm.txt 文本，返回 None 表示非视频杂项/样片/非法行需拦截过滤
pub fn parse_strm_line(raw_line: &str) -> Option<ParsedStrmRecord> {
    let line = raw_line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    // 1. 按第一个 '#' 切分为逻辑路径与真实直链（若无 '#' 则左右共用同一路径）
    let (left_raw, right_raw) = match line.split_once('#') {
        Some((l, r)) => (l.trim(), r.trim()),
        None => (line, line),
    };

    // 2. 清洗右侧真实 AList 路径（剥离任意占位域名、/d 前缀及 ?sign=...）
    let alist_path = extract_clean_alist_path(right_raw)?;
    let lower_path = alist_path.to_lowercase();

    // 拦截样片与花絮
    if lower_path.contains("-sample")
        || lower_path.contains(".sample.")
        || lower_path.contains("/sample/")
        || lower_path.contains("/trailer")
        || lower_path.contains("/preview")
    {
        return None;
    }

    // 提取真实文件名与扩展名校验
    let file_name = alist_path.rsplit('/').next()?.trim().to_string();
    if file_name.is_empty() {
        return None;
    }

    let container = match file_name.rsplit_once('.') {
        Some((_, ext)) => {
            let ext_lower = ext.to_lowercase();
            // 若存在扩展名且不在视频白名单内（如 .mp3, .jpg, .nfo, .ass），直接过滤
            if ext_lower.len() <= 5 && !VIDEO_EXTS.contains(ext_lower.as_str()) {
                return None;
            }
            if VIDEO_EXTS.contains(ext_lower.as_str()) {
                ext_lower
            } else {
                "mp4".to_string()
            }
        }
        None => "mp4".to_string(), // 无扩展名网盘视频流放行
    };

    // 3. 清洗左侧逻辑路径，剥离前导噪声分类前缀
    let mut left_clean = lossy_url_decode(left_raw)
        .trim_start_matches('/')
        .to_string();
    while NOISE_PREFIX_RE.is_match(&left_clean) {
        left_clean = NOISE_PREFIX_RE.replace(&left_clean, "").to_string();
    }

    // 4. 双端 {tmdb-ID} 互补提取
    let tmdb_id = extract_tmdb_id(&left_clean).or_else(|| extract_tmdb_id(&alist_path));

    // 5. 解析结构目录、季集号、片名与年份
    let segments: Vec<&str> = left_clean.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return None;
    }

    // 定位季集号（优先看左侧文件名，其次看右侧真实文件名）
    let left_file = segments.last().copied().unwrap_or(&file_name);
    let (mut season_opt, ep_num_opt) = extract_season_episode(left_file)
        .unwrap_or_else(|| extract_season_episode(&file_name).unwrap_or((None, None)));

    // 若文件名未写明季号（如仅写了 "01.mkv" 或 "E01.mkv"），从父级目录（如 "Season 1" / "Season 0" / "第一季"）回溯季号
    if season_opt.is_none() && segments.len() >= 2 {
        for seg in segments.iter().rev().skip(1).take(2) {
            if let Some(s) = extract_season_from_dir(seg) {
                season_opt = Some(s);
                break;
            }
        }
    }

    let is_tv = ep_num_opt.is_some() || season_opt.is_some();
    let inferred_category = infer_category_from_segments(&segments, is_tv);
    let (inferred_media_type, final_season, final_episode) = if is_tv {
        (
            "tv".to_string(),
            season_opt.unwrap_or(1),
            ep_num_opt.unwrap_or(1),
        )
    } else {
        ("movie".to_string(), 0, 1)
    };

    // 定位影视根目录（跳过末级文件名以及可能存在的 Season X 子目录）
    let (root_dir, raw_folder_name) = locate_media_root(&segments, is_tv);
    let (clean_title, year) = extract_title_and_year(&raw_folder_name);

    // 6. 提取版本标签与排序权重（白名单正则 -> 父目录差集 -> 网盘来源/容器兜底）
    let (version_tag, priority_score) =
        extract_version_tag_and_score(&alist_path, &file_name, &clean_title, &container);

    // 7. 计算全局物理路径哈希 (xxh3_64)
    let path_hash = xxh3_64(alist_path.as_bytes()) as i64;

    Some(ParsedStrmRecord {
        root_dir,
        raw_folder_name,
        clean_title,
        year,
        inferred_media_type,
        inferred_category,
        tmdb_id,
        season_number: final_season,
        episode_number: final_episode,
        alist_path,
        path_hash,
        file_name,
        container,
        version_tag,
        priority_score,
    })
}

/// 剥离域名与 ?sign= 参数，返回归一化且容错解码后的 AList 逻辑路径
fn extract_clean_alist_path(raw_url: &str) -> Option<String> {
    let without_query = raw_url.split('?').next().unwrap_or(raw_url).trim();
    // 直接剥离 scheme://host，避免 Url::parse 把文件名里的 `#` 当作 URL fragment 截断
    let raw_path = if let Some((_, after_scheme)) = without_query.split_once("://") {
        let idx = after_scheme.find('/')?;
        after_scheme[idx..].to_string()
    } else {
        without_query.to_string()
    };

    let decoded = lossy_url_decode(&raw_path);
    // 剥离 AList 常见的 /d/ 或 /p/ 下载路由前缀
    let stripped = decoded
        .strip_prefix("/d/")
        .or_else(|| decoded.strip_prefix("/p/"))
        .map(|s| format!("/{}", s))
        .unwrap_or(decoded);

    // 归一化多余斜杠，如 //电视剧///庆余年 -> /电视剧/庆余年
    let parts: Vec<&str> = stripped.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    Some(format!("/{}", parts.join("/")))
}

/// 容错 URL 解码：合法 %XX 序列按 UTF-8 解码，非法孤立 %（如 100%.mkv）原样保留
pub fn lossy_url_decode(input: &str) -> String {
    percent_decode(input.as_bytes()).decode_utf8_lossy().into_owned()
}

fn extract_tmdb_id(text: &str) -> Option<i32> {
    TMDB_ID_RE
        .captures(text)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<i32>().ok())
}

fn infer_category_from_segments(segments: &[&str], is_tv: bool) -> String {
    for seg in segments.iter().take(3) {
        if seg.contains("动漫") || seg.contains("动画") || seg.contains("番剧") {
            return "动漫".to_string();
        }
        if seg.contains("纪录") {
            return "纪录片".to_string();
        }
        if seg.contains("综艺") || seg.contains("真人秀") {
            return "综艺".to_string();
        }
        if seg.contains("电影") {
            return "电影".to_string();
        }
        if seg.contains("剧") {
            return "电视剧".to_string();
        }
    }
    if is_tv {
        "电视剧".to_string()
    } else {
        "电影".to_string()
    }
}

fn extract_season_episode(name: &str) -> Option<(Option<i32>, Option<i32>)> {
    let caps = SEASON_EP_RE.captures(name)?;
    if let (Some(s_m), Some(ep_m)) = (caps.get(1), caps.get(2)) {
        let s = s_m.as_str().parse::<i32>().ok()?;
        let e = ep_m.as_str().parse::<i32>().ok()?;
        Some((Some(s), Some(e)))
    } else if let Some(ep_m) = caps.get(3) {
        let e = ep_m.as_str().parse::<i32>().ok()?;
        Some((None, Some(e)))
    } else if let Some(cn_ep) = caps.get(4) {
        let e = cn_ep.as_str().parse::<i32>().ok()?;
        Some((None, Some(e)))
    } else {
        None
    }
}

fn extract_season_from_dir(dir_name: &str) -> Option<i32> {
    let caps = SEASON_DIR_RE.captures(dir_name)?;
    caps.get(1)
        .or_else(|| caps.get(2))
        .or_else(|| caps.get(3))
        .and_then(|m| m.as_str().parse::<i32>().ok())
}

fn locate_media_root(segments: &[&str], is_tv: bool) -> (String, String) {
    if segments.len() == 1 {
        let name = segments[0].to_string();
        return (name.clone(), name);
    }

    // 去掉末级文件名
    let mut dir_end = segments.len() - 1;
    // 如果是剧集且倒数第二级是 Season X / 第一季，则再往上一级作为剧集根目录
    if is_tv && dir_end >= 2 && extract_season_from_dir(segments[dir_end - 1]).is_some() {
        dir_end -= 1;
    }

    let root_dir = segments[..dir_end].join("/");
    let mut raw_folder_name = segments[dir_end - 1].to_string();

    // 若当前目录名去掉 {tmdb-xxx} 后为空（例如目录就叫 "{tmdb-80009}"），向上取父目录作为真实片名
    let stripped = TMDB_ID_RE.replace_all(&raw_folder_name, "").trim().to_string();
    if stripped.is_empty() && dir_end >= 2 {
        raw_folder_name = segments[dir_end - 2].to_string();
    }
    (root_dir, raw_folder_name)
}

fn extract_title_and_year(folder_name: &str) -> (String, Option<i32>) {
    use chrono::Datelike;

    // 先移除 {tmdb-12345} 标签
    let without_tmdb = TMDB_ID_RE.replace_all(folder_name, "").trim().to_string();
    let max_valid_year = chrono::Utc::now().year() + 2;

    // 收集所有位于合理区间 [1900, 当前年+2] 且前面存在有效片名的年份候选，
    // 优先取括号包裹的年份（如 "2040 (2019)"），其次取最后一个候选年份（避免将片名中的数字如 "2040"/"1917" 当成发行年份）。
    let mut best_match: Option<(usize, i32, bool)> = None;
    for caps in YEAR_RE.captures_iter(&without_tmdb) {
        let full = match caps.get(0) {
            Some(m) => m,
            None => continue,
        };
        let is_bracketed = caps.get(1).is_some();
        let yr = match caps
            .get(1)
            .or_else(|| caps.get(2))
            .and_then(|m| m.as_str().parse::<i32>().ok())
        {
            Some(y) if (1900..=max_valid_year).contains(&y) => y,
            _ => continue,
        };

        let prefix_clean = without_tmdb[..full.start()]
            .trim_end_matches(|c: char| c == '(' || c == '[' || c == '（' || c == '.' || c == '-' || c.is_whitespace());
        if prefix_clean.is_empty() {
            // 年份处于最开头（前面没有任何片名，如目录就叫 "2040" 或 "1917"），这是片名本身，不能当成发行年份
            continue;
        }

        match best_match {
            None => best_match = Some((full.start(), yr, is_bracketed)),
            Some((_, _, prev_bracketed)) => {
                if is_bracketed || !prev_bracketed {
                    best_match = Some((full.start(), yr, is_bracketed));
                }
            }
        }
    }

    let (title_part, year) = if let Some((pos, yr, _)) = best_match {
        (&without_tmdb[..pos], Some(yr))
    } else {
        (without_tmdb.as_str(), None)
    };

    let clean = title_part
        .trim_end_matches(|c: char| c == '(' || c == '[' || c == '（' || c == '.' || c == '-' || c.is_whitespace())
        .replace('.', " ")
        .trim()
        .to_string();

    let final_title = if clean.is_empty() {
        without_tmdb
    } else {
        clean
    };
    (final_title, year)
}

/// 多维版本标签提取与权重评分（分辨率 + 介质 + 动态范围 + 帧率 + 音轨规格 + 压制组/语言）
pub fn extract_version_tag_and_score(
    alist_path: &str,
    file_name: &str,
    clean_title: &str,
    container: &str,
) -> (String, i32) {
    let upper_file = file_name.to_uppercase();
    // 同时参考父目录名称（许多小雅资源将 [60帧] [高码] [杜比] 写在父文件夹名上）
    let parent_dir = alist_path
        .rsplit('/')
        .nth(1)
        .unwrap_or("")
        .to_uppercase();
    let combined_upper = format!("{} {}", parent_dir, upper_file);

    let mut tags: Vec<String> = Vec::new();
    let mut score = 0;

    // 1. 分辨率识别
    if combined_upper.contains("2160P") || combined_upper.contains("4K") {
        tags.push("4K".to_string());
        score += 400;
    } else if combined_upper.contains("1080P") {
        tags.push("1080p".to_string());
        score += 200;
    } else if combined_upper.contains("720P") {
        tags.push("720p".to_string());
        score += 100;
    }

    // 2. 介质来源
    if combined_upper.contains("REMUX") {
        tags.push("REMUX".to_string());
        score += 80;
    } else if combined_upper.contains("BLURAY") || combined_upper.contains("BLU-RAY") {
        tags.push("BluRay".to_string());
        score += 50;
    } else if combined_upper.contains("WEB-DL") || combined_upper.contains("WEBDL") {
        tags.push("WEB-DL".to_string());
        score += 30;
    }

    // 3. 高帧率特征 (60FPS / 120FPS)
    if combined_upper.contains("120FPS") || combined_upper.contains("120帧") {
        tags.push("120FPS".to_string());
        score += 35;
    } else if combined_upper.contains("60FPS") || combined_upper.contains("60帧") {
        tags.push("60FPS".to_string());
        score += 25;
    }

    // 4. 动态范围特效 (DV / HDR / SDR / IMAX)
    if upper_file.contains(".DV.")
        || upper_file.contains("DOVI")
        || combined_upper.contains("杜比视界")
        || upper_file.contains("DOLBY.VISION")
    {
        tags.push("杜比视界".to_string());
        score += 40;
    } else if combined_upper.contains("HDR10+") {
        tags.push("HDR10+".to_string());
        score += 25;
    } else if combined_upper.contains("HDR") {
        tags.push("HDR".to_string());
        score += 20;
    } else if combined_upper.contains("SDR") {
        tags.push("SDR".to_string());
        score += 5;
    }

    if combined_upper.contains("IMAX") {
        tags.push("IMAX".to_string());
        score += 25;
    }

    // 5. 音轨规格特征 (Atmos / TrueHD / DTS-HD / FLAC / DDP)
    if combined_upper.contains("ATMOS") || combined_upper.contains("全景声") {
        tags.push("Atmos".to_string());
        score += 20;
    } else if combined_upper.contains("TRUEHD") {
        tags.push("TrueHD".to_string());
        score += 18;
    } else if combined_upper.contains("DTS-HD") || combined_upper.contains("DTSHD") {
        tags.push("DTS-HD".to_string());
        score += 16;
    } else if combined_upper.contains("FLAC") {
        tags.push("FLAC".to_string());
        score += 15;
    } else if upper_file.contains("DDP5.1") || upper_file.contains("DDP.5.1") {
        tags.push("DDP5.1".to_string());
        score += 10;
    } else if upper_file.contains("DDP") || upper_file.contains("EAC3") {
        tags.push("DDP".to_string());
        score += 8;
    }

    // 6. 语言与字幕特征
    if file_name.contains("国粤") || alist_path.contains("国粤") {
        tags.push("国粤双语".to_string());
        score += 15;
    } else if file_name.contains("国语") || file_name.contains("台配") {
        tags.push("国语".to_string());
        score += 10;
    } else if file_name.contains("粤语") {
        tags.push("粤语".to_string());
        score += 10;
    }
    if file_name.contains("泰语中字") {
        tags.push("泰语中字".to_string());
    } else if file_name.contains("特效字幕") {
        tags.push("特效字幕".to_string());
    } else if file_name.contains("中字") {
        tags.push("中字".to_string());
    }

    // 7. 提取文件名末尾压制组标识（如 "-HiveWeb", "-FRDS"）辅助区分同规格不同片源
    if let Some((stem, _)) = file_name.rsplit_once('.') {
        if let Some((_, group)) = stem.rsplit_once('-') {
            let g = group.trim();
            if g.len() >= 3
                && g.len() <= 14
                && g.chars().all(|c| c.is_ascii_alphanumeric())
                && !g.eq_ignore_ascii_case("HIFI")
                && !g.eq_ignore_ascii_case("DL")
                && !g.eq_ignore_ascii_case("HD")
            {
                tags.push(g.to_string());
            }
        }
    }

    // 若在文件名/父目录中命中了有效版本标签，直接组合返回
    if !tags.is_empty() {
        return (tags.join(" · "), score);
    }

    // 8. 兜底阶梯一：向上回溯 1~2 级父目录剥离剧名后的差集文本
    let dirs: Vec<&str> = alist_path.split('/').filter(|s| !s.is_empty()).collect();
    if dirs.len() >= 2 {
        for parent in dirs.iter().rev().skip(1).take(2) {
            if extract_season_from_dir(parent).is_some() {
                continue;
            }
            let diff = parent.replace(clean_title, "").trim().to_string();
            if !diff.is_empty() && diff != *parent && diff.chars().count() <= 24 {
                let cleaned_diff = diff
                    .trim_matches(|c: char| c == '(' || c == ')' || c == '[' || c == ']' || c == '-' || c.is_whitespace())
                    .to_string();
                if !cleaned_diff.is_empty() && cleaned_diff.parse::<i32>().is_err() {
                    return (cleaned_diff, 10);
                }
            }
        }
    }

    // 9. 兜底阶梯二：提取顶级网盘来源特征 + 容器格式（如 "115 · MP4"）
    let origin = extract_origin_label(alist_path);
    (format!("{} · {}", origin, container.to_uppercase()), 0)
}

/// 从 AList 路径提取简短来源特征标签（用于兜底或同规格重名消歧）
pub fn extract_origin_label(alist_path: &str) -> String {
    let dirs: Vec<&str> = alist_path.split('/').filter(|s| !s.is_empty()).collect();
    let first = dirs.first().copied().unwrap_or("默认源");
    let second = dirs.get(1).copied().unwrap_or("");
    if first.contains("每日更新") {
        if second.contains("115") || alist_path.contains("115") {
            "每日更新·115".to_string()
        } else if second.contains("夸克") || alist_path.contains("夸克") {
            "每日更新·夸克".to_string()
        } else {
            "每日更新".to_string()
        }
    } else if first.contains("115") {
        "115分享".to_string()
    } else if first.contains("阿里") {
        "阿里云盘".to_string()
    } else if first.contains("夸克") {
        "夸克网盘".to_string()
    } else {
        first.trim_start_matches(|c: char| !c.is_alphanumeric() && !('\u{4e00}'..='\u{9fa5}').contains(&c)).to_string()
    }
}

/// 同集多版本重名自动消歧：
/// 1. 实时用最新规则重算一次标签与权重（使历史已入库记录无需重新同步也立即生效）；
/// 2. 若同一集下仍有 >=2 个版本标签完全相同，自动追加 `[来源目录]` 或 `#序号` 确保 100% 不重名。
pub fn disambiguate_version_tags(items: &mut [(String, i32, String, String)]) {
    // items 元素元组: (version_tag, priority_score, alist_path, container)
    for (tag, score, alist_path, container) in items.iter_mut() {
        let file_name = alist_path.rsplit('/').next().unwrap_or("");
        let (new_tag, new_score) = extract_version_tag_and_score(alist_path, file_name, "", container);
        if !new_tag.is_empty() {
            *tag = new_tag;
            *score = new_score;
        }
    }

    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (tag, _, _, _) in items.iter() {
        *counts.entry(tag.clone()).or_insert(0) += 1;
    }

    // 对出现重名的版本追加来源目录特征
    let mut final_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (tag, _, alist_path, _) in items.iter_mut() {
        if counts.get(tag).copied().unwrap_or(0) > 1 {
            let origin = extract_origin_label(alist_path);
            *tag = format!("{} [{}]", tag, origin);
        }
        *final_counts.entry(tag.clone()).or_insert(0) += 1;
    }

    // 若同一来源目录下仍有重名，追加 #1, #2 兜底区分
    let mut seq_tracker: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (tag, _, _, _) in items.iter_mut() {
        if final_counts.get(tag).copied().unwrap_or(0) > 1 {
            let idx = seq_tracker.entry(tag.clone()).or_insert(0);
            *idx += 1;
            *tag = format!("{} #{}", tag, idx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dirty_xiaoya_line_with_emoji_and_prefix() {
        let line = "刮削失败/115/电视剧/东南亚/泰国爱侣 (2017)/Season 1/泰国爱侣.2017.S01E01.strm#http://xiaoya.host:5678/d/🏷️我的115分享/电视剧/泰剧/泰国爱侣num中字/【百度num吧】泰剧《泰国爱侣》第01集【泰语中字】.mp4?sign=SIGN_STR";
        let rec = parse_strm_line(line).expect("应当成功解析");

        assert_eq!(rec.clean_title, "泰国爱侣");
        assert_eq!(rec.year, Some(2017));
        assert_eq!(rec.inferred_media_type, "tv");
        assert_eq!(rec.season_number, 1);
        assert_eq!(rec.episode_number, 1);
        assert_eq!(rec.container, "mp4");
        assert_eq!(
            rec.alist_path,
            "/🏷️我的115分享/电视剧/泰剧/泰国爱侣num中字/【百度num吧】泰剧《泰国爱侣》第01集【泰语中字】.mp4"
        );
        assert_eq!(rec.version_tag, "泰语中字");
    }

    #[test]
    fn test_parse_untagged_episode_fallback() {
        // 文件名完全无版本标签，但父目录有 [4K高码率版]
        let line1 = "电视剧/绝命毒师 (2008)/Season 1/01.strm#http://127.0.0.1:5678/d/我的115分享/绝命毒师 [4K高码率版]/S01/01.mkv";
        let rec1 = parse_strm_line(line1).unwrap();
        assert_eq!(rec1.version_tag, "4K高码率版");

        // 父目录也完全无差异，触发来源 + 容器兜底
        let line2 = "电视剧/绝命毒师 (2008)/Season 1/01.strm#http://127.0.0.1:5678/d/我的115分享/绝命毒师/S01/01.mp4";
        let rec2 = parse_strm_line(line2).unwrap();
        assert_eq!(rec2.version_tag, "115分享 · MP4");
    }

    #[test]
    fn test_multi_version_disambiguation() {
        let mut versions = vec![
            (
                String::new(),
                0,
                "/🏷️我的115分享/电视剧/更新合集13/D 小城大事 (2026)[4K MAX+ 60帧 SDR][4K 杜比视界][FLAC-HIFI声] [全40集]/小城大事 (2026) [4K 杜比视界][FLAC-HIFI声]/小城大事.S01E04.2026.2160p.WEB-DL.H265.DV.10bit.25fps.FLAC-HIFI 2.0.mkv".to_string(),
                "mkv".to_string(),
            ),
            (
                String::new(),
                0,
                "/每日更新/电视剧/国产剧（115）/小城大事/The.Dream.Maker.2026.S01E04.2160p.WEB-DL.DV.H.265.DDP5.1-HiveWeb.mkv".to_string(),
                "mkv".to_string(),
            ),
            (
                String::new(),
                0,
                "/🏷️我的115分享/电视剧/更新合集13/D 小城大事 (2026)[4K MAX+ 60帧 SDR][4K 杜比视界][FLAC-HIFI声] [全40集]/小城大事 (2026) [4K MAX+ 60帧 SDR][FLAC-HIFI声]/小城大事.S01E04.2026.2160p.WEB-DL.H265.SDR.10bit.60fps.FLAC-HIFI 2.0.mkv".to_string(),
                "mkv".to_string(),
            ),
        ];

        disambiguate_version_tags(&mut versions);
        assert_eq!(versions[0].0, "4K · WEB-DL · 杜比视界 · FLAC");
        assert_eq!(versions[1].0, "4K · WEB-DL · 杜比视界 · DDP5.1 · HiveWeb");
        assert_eq!(versions[2].0, "4K · WEB-DL · 60FPS · SDR · FLAC");
    }

    #[test]
    fn test_lossy_percent_and_hash_in_filename() {
        // 文件名包含非法百分号 % 和第二个 # 号
        let line = "电影/测试电影 (2024)/测试.strm#http://xiaoya.host:5678/d/电影/100%_完整版 #1.mkv?sign=abc";
        let rec = parse_strm_line(line).unwrap();
        assert_eq!(rec.alist_path, "/电影/100%_完整版 #1.mkv");
        assert_eq!(rec.container, "mkv");
    }

    #[test]
    fn test_filter_sample_and_audio() {
        let sample = "电影/沙丘2 (2024)/Dune.2024.sample.mkv#http://127.0.0.1:5678/d/电影/Dune.2024.sample.mkv";
        let audio = "电影/沙丘2 (2024)/theme.mp3#http://127.0.0.1:5678/d/电影/theme.mp3";
        assert!(parse_strm_line(sample).is_none());
        assert!(parse_strm_line(audio).is_none());
    }

    #[test]
    fn test_numeric_movie_title_not_mistaken_for_year() {
        let line1 = "纪录片/科学技术/2040/2040.mp4#http://127.0.0.1:5678/d/纪录片/科学技术/2040/2040.mp4";
        let rec1 = parse_strm_line(line1).unwrap();
        assert_eq!(rec1.clean_title, "2040");
        assert_eq!(rec1.year, None);

        let line2 = "纪录片/科学技术/2040 (2019)/2040.2019.mp4#http://127.0.0.1:5678/d/纪录片/科学技术/2040 (2019)/2040.2019.mp4";
        let rec2 = parse_strm_line(line2).unwrap();
        assert_eq!(rec2.clean_title, "2040");
        assert_eq!(rec2.year, Some(2019));

        let line3 = "电影/1917 (2019)/1917.2019.mkv#http://127.0.0.1:5678/d/电影/1917 (2019)/1917.2019.mkv";
        let rec3 = parse_strm_line(line3).unwrap();
        assert_eq!(rec3.clean_title, "1917");
        assert_eq!(rec3.year, Some(2019));
    }

    #[test]
    fn test_specials_season_zero_and_boundary_safety() {
        // S00 特别篇必须保留为 season_number = 0
        let sp = "电视剧/某剧 (2024)/Season 0/某剧.S00E01.mkv#http://127.0.0.1:5678/d/电视剧/某剧 (2024)/Season 0/某剧.S00E01.mkv";
        let rec_sp = parse_strm_line(sp).unwrap();
        assert_eq!(rec_sp.inferred_media_type, "tv");
        assert_eq!(rec_sp.season_number, 0);
        assert_eq!(rec_sp.episode_number, 1);

        // Case39 / PS5 不能误识别为集号或季目录，无“电影”路径的电影默认归类“电影”
        let case39 = "115/欧美/Case39 (2009)/Case39.2009.mkv#http://127.0.0.1:5678/d/115/欧美/Case39 (2009)/Case39.2009.mkv";
        let rec_case = parse_strm_line(case39).unwrap();
        assert_eq!(rec_case.inferred_media_type, "movie");
        assert_eq!(rec_case.inferred_category, "电影");

        let ps5 = "电视剧/PS5宣传片/01.mp4#http://127.0.0.1:5678/d/电视剧/PS5宣传片/01.mp4";
        let rec_ps5 = parse_strm_line(ps5).unwrap();
        assert_eq!(rec_ps5.root_dir, "电视剧/PS5宣传片");
    }
}
