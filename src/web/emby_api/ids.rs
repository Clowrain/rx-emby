//! Emby 条目 ID 解析与搜索模式工具函数（纯函数，可单测）

/// 解析 `work_{wid}` / 兼容旧 `media_{wid}` / 纯数字 ID
pub(in crate::web) fn parse_work_id(raw: &str) -> Option<i64> {
    raw.strip_prefix("work_")
        .or_else(|| raw.strip_prefix("media_"))
        .unwrap_or(raw)
        .parse::<i64>()
        .ok()
}

/// 解析 `season_{wid}_{snum}`（允许季号 0 = 特别篇）
pub(in crate::web) fn parse_season_id(raw: &str) -> Option<(i64, i32)> {
    let rest = raw.strip_prefix("season_")?;
    let (wid, snum) = rest.rsplit_once('_')?;
    let wid = parse_work_id(wid)?;
    let snum = snum.parse::<i32>().ok()?;
    (snum >= 0).then_some((wid, snum))
}

/// 解析 `ep_{wid}_{snum}_{epnum}`（允许集号 0 = 特殊集，季号 0 = 特别篇）
pub(in crate::web) fn parse_episode_id(raw: &str) -> Option<(i64, i32, i32)> {
    let rest = raw.strip_prefix("ep_")?;
    let (head, epnum) = rest.rsplit_once('_')?;
    let (wid, snum) = parse_season_id(&format!("season_{head}"))?;
    let epnum = epnum.parse::<i32>().ok()?;
    (epnum >= 0).then_some((wid, snum, epnum))
}

/// 转义 LIKE/ILIKE 模式中的通配符与转义符，防止用户输入被解释为 `%` / `_` 通配
pub(in crate::web) fn escape_like(term: &str) -> String {
    let mut out = String::with_capacity(term.len());
    for ch in term.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '%' => out.push_str("\\%"),
            '_' => out.push_str("\\_"),
            _ => out.push(ch),
        }
    }
    out
}

/// 把客户端 `SearchTerm` 归一化为 ILIKE 模式（自动包裹 `%`）；空白输入返回 None
pub(in crate::web) fn build_search_pattern(term: Option<&str>) -> Option<String> {
    let trimmed = term.map(str::trim).filter(|s| !s.is_empty())?;
    Some(format!("%{}%", escape_like(trimmed)))
}

/// 当搜索词是纯数字时，额外精确匹配 `media_works.tmdb_id`
pub(in crate::web) fn numeric_tmdb_id(term: Option<&str>) -> Option<i32> {
    term.map(str::trim).filter(|s| !s.is_empty()).and_then(|s| s.parse::<i32>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_work_id_variants() {
        assert_eq!(parse_work_id("work_123"), Some(123));
        assert_eq!(parse_work_id("media_123"), Some(123));
        assert_eq!(parse_work_id("123"), Some(123));
        assert_eq!(parse_work_id("ep_1_2_3"), None);
        assert_eq!(parse_work_id("lib_movie"), None);
    }

    #[test]
    fn test_parse_season_id_includes_specials() {
        assert_eq!(parse_season_id("season_42_0"), Some((42, 0)));
        assert_eq!(parse_season_id("season_42_1"), Some((42, 1)));
        assert_eq!(parse_season_id("season_42_-1"), None);
        assert_eq!(parse_season_id("work_42"), None);
        assert_eq!(parse_season_id("season_x_1"), None);
    }

    #[test]
    fn test_parse_episode_id_allows_zero_episode_number() {
        assert_eq!(parse_episode_id("ep_42_0_0"), Some((42, 0, 0)));
        assert_eq!(parse_episode_id("ep_42_1_12"), Some((42, 1, 12)));
        assert_eq!(parse_episode_id("ep_42_1_-3"), None);
        assert_eq!(parse_episode_id("ep_42_1"), None);
    }

    #[test]
    fn test_escape_like_and_pattern() {
        assert_eq!(escape_like("50%_off\\x"), "50\\%\\_off\\\\x");
        assert_eq!(
            build_search_pattern(Some("  海贼王 ")),
            Some("%海贼王%".to_string())
        );
        assert_eq!(build_search_pattern(Some("   ")), None);
        assert_eq!(build_search_pattern(None), None);
        assert_eq!(numeric_tmdb_id(Some("12345")), Some(12345));
        assert_eq!(numeric_tmdb_id(Some("海贼王")), None);
    }
}
