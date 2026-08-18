// ============================================
// 三层分类引擎：电影 vs 剧集
// 第一层 文件名规则 → 第二层 目录上下文 → 第三层 概率融合
// 输出 kind(movie/series/unknown) + confidence(0-100)
// ============================================
use regex::Regex;
use std::sync::LazyLock;

const SEP: &str = r"[\s._\-\[\]()（）【】]";

/// 强信号：季+集（S01E01 / Season 1 Episode 2 / 第1季第2集）
static SEASON_EPISODE_RE: LazyLock<Regex> = LazyLock::new(|| {
    let p = format!(
        r"(?ix)(?:^|{sep})
        (?:S(?:eason)?\s*(\d{{1,2}})\s*(?:EP?|Episode)\s*(\d{{1,3}})
        |第\s*(\d{{1,2}})\s*季\s*第?\s*(\d{{1,3}})\s*集)
        (?:{sep}|$)",
        sep = SEP
    );
    Regex::new(&p).unwrap()
});

/// 单集信号：EP01 / Episode 2 / 第3集
static EPISODE_RE: LazyLock<Regex> = LazyLock::new(|| {
    let p = format!(
        r"(?ix)(?:^|{sep})(?:EP|Episode|第)\s*(\d{{1,3}})(?:集|{sep}|$)",
        sep = SEP
    );
    Regex::new(&p).unwrap()
});

/// 数字序号信号：xxx.01.mkv / 武林外传40.rmvb（1-3 位纯数字）
/// 注意：regex crate 不支持 lookbehind，前字符校验在代码里做（is_numeric_ep 排除 x264/1080p 等）
static NUMERIC_EP_RE: LazyLock<Regex> = LazyLock::new(|| {
    let p = format!(
        r"(\d{{1,3}})(?:{sep}|$)",
        sep = SEP
    );
    Regex::new(&p).unwrap()
});

/// 提取数字序号：校验前一个字符不是字母/数字（排除 x264、h265、多位数字尾部等）
fn numeric_episode(title: &str) -> Option<i32> {
    let caps = NUMERIC_EP_RE.captures(title)?;
    let m = caps.get(1)?;
    // 前一个字符不能是 ASCII 字母或数字（用字节向前定位，避免切片切到 UTF-8 字符中间）
    let bytes = title.as_bytes();
    let prev_ok = if m.start() > 0 {
        let mut i = m.start() - 1;
        while i > 0 && (bytes[i] & 0xC0) == 0x80 {
            i -= 1;
        }
        match std::str::from_utf8(&bytes[i..m.start()]) {
            Ok(s) => s.chars().next().map(|c| !c.is_ascii_alphanumeric()).unwrap_or(true),
            Err(_) => true,
        }
    } else {
        true
    };
    if !prev_ok {
        return None;
    }
    let n: i32 = m.as_str().parse().ok()?;
    if !(1..=999).contains(&n) {
        return None;
    }
    if is_resolution_or_year(title, n) {
        return None;
    }
    Some(n)
}

/// 年份信号：(2004) / [2023] / 2024
static YEAR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\(\[]\d{4}[\)\]]|\b(?:19|20)\d{2}\b").unwrap());

/// 季目录信号：Season 1 / S02 / 第3季
static SEASON_DIR_RE: LazyLock<Regex> = LazyLock::new(|| {
    let p = format!(
        r"(?ix)(?:^|{sep})(?:S(?:eason)?|第)\s*(\d{{1,2}})(?:季|{sep}|$)",
        sep = SEP
    );
    Regex::new(&p).unwrap()
});

pub struct Classification {
    pub kind: String,
    pub confidence: i32,
    pub series_name: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
}

impl Classification {
    fn movie(confidence: i32) -> Self {
        Self { kind: "movie".into(), confidence, series_name: None, season: None, episode: None }
    }
    fn unknown(confidence: i32, series_name: Option<String>, season: Option<i32>, episode: Option<i32>) -> Self {
        Self { kind: "unknown".into(), confidence, series_name, season, episode }
    }
    fn series(confidence: i32, series_name: Option<String>, season: Option<i32>, episode: Option<i32>) -> Self {
        Self { kind: "series".into(), confidence, series_name, season, episode }
    }
}

/// 主入口：对单个视频文件分类
/// - title: 文件名（不含扩展名）
/// - parent_name: 父目录名
/// - grandparent_name: 祖父目录名（可选）
/// - sibling_titles: 同目录下所有视频文件的标题（不含扩展名）
pub fn classify(title: &str, parent_name: &str, grandparent_name: Option<&str>, sibling_titles: &[String]) -> Classification {
    // ===== 第一层：文件名规则 =====
    if let Some(caps) = SEASON_EPISODE_RE.captures(title) {
        let season = caps.get(1).or_else(|| caps.get(3)).and_then(|m| m.as_str().parse().ok());
        let episode = caps.get(2).or_else(|| caps.get(4)).and_then(|m| m.as_str().parse().ok());
        let name = clean_series_name(title);
        return Classification::series(92, Some(name), season, episode);
    }

    // 单集信号
    let episode = EPISODE_RE.captures(title).and_then(|c| c.get(1)).and_then(|m| m.as_str().parse::<i32>().ok());
    // 数字序号信号（排除 1080/720 等分辨率、4k、年份、x264 前缀）
    let numeric_ep = numeric_episode(title);

    let has_year = YEAR_RE.is_match(title);
    let season_dir_match = SEASON_DIR_RE.is_match(parent_name);

    // ===== 第二层：目录上下文 =====
    let sibling_count = sibling_titles.len();
    let common_ratio = common_prefix_ratio(sibling_titles);
    let has_ep_signal = episode.is_some() || numeric_ep.is_some();
    // 目录内有大量文件且共享同一前缀 → 剧集
    let dir_signal = if sibling_count >= 5 && common_ratio >= 0.6 {
        if has_ep_signal { 30 } else { 20 }
    } else if sibling_count >= 3 && common_ratio >= 0.6 {
        if has_ep_signal { 25 } else { 12 }
    } else if sibling_count >= 5 {
        if has_ep_signal { 10 } else { 5 }
    } else {
        0
    };
    // 文件名以目录名为前缀（武林外传40 ⊂ 武林外传）且带集数 → 强剧集信号
    let name_prefix_signal = if !parent_name.is_empty()
        && has_ep_signal
        && title.starts_with(parent_name)
    {
        20
    } else {
        0
    };

    // ===== 第三层：概率融合 =====
    let mut score = 0i32;
    if episode.is_some() {
        score += 30;
    } else if numeric_ep.is_some() {
        score += 20;
    }
    if season_dir_match {
        score += 35;
    }
    if has_year {
        score -= 10;
    }
    score += dir_signal;
    score += name_prefix_signal;
    // 单文件独处目录 → 电影倾向；但若目录本身是季目录（强剧集信号），不惩罚
    if sibling_count <= 1 && !season_dir_match {
        score -= 20;
    }

    // 剧集名推断
    let mut series_name: Option<String> = None;
    let mut season: Option<i32> = None;
    if episode.is_some() || numeric_ep.is_some() {
        let mut name_candidate: Option<String> = None;
        if season_dir_match {
            // 季目录 → 祖父目录是剧集名
            if let Some(gp) = grandparent_name {
                if !gp.trim().is_empty() {
                    name_candidate = Some(gp.trim().to_string());
                }
            }
            season = SEASON_DIR_RE.captures(parent_name).and_then(|c| c.get(1)).and_then(|m| m.as_str().parse().ok());
        } else {
            name_candidate = Some(clean_series_name(title));
        }
        if let Some(name) = name_candidate {
            if !name.trim().is_empty() {
                series_name = Some(name.trim().to_string());
            }
        }
    } else if season_dir_match {
        // 无集数但目录是季目录：目录内文件归属祖父目录
        if let Some(gp) = grandparent_name {
            if !gp.trim().is_empty() {
                series_name = Some(gp.trim().to_string());
            }
        }
        season = SEASON_DIR_RE.captures(parent_name).and_then(|c| c.get(1)).and_then(|m| m.as_str().parse().ok());
    }

    // ===== 输出 =====
    if score >= 55 {
        Classification::series(score.min(90), series_name, season, episode.or(numeric_ep))
    } else if score >= 25 {
        Classification::unknown(score, series_name, season, episode.or(numeric_ep))
    } else {
        Classification::movie((100 - score.min(100)).max(50))
    }
}

fn is_resolution_or_year(title: &str, n: i32) -> bool {
    // 720/1080/2160/480 等分辨率
    if matches!(n, 480 | 720 | 1080 | 2160) {
        return true;
    }
    // 4 位数年份（1800-2099）
    if (1800..=2099).contains(&n) {
        return true;
    }
    // 标题中显式包含年份词（如 1998、2024）
    let lower = title.to_lowercase();
    lower.contains(&format!("{}", n)) && n >= 1900 && n <= 2099 && !lower.contains(&format!("season"))
}

/// 计算共享前缀比例：同目录文件名的公共前缀长度 / 文件名平均长度
fn common_prefix_ratio(titles: &[String]) -> f32 {
    if titles.len() < 2 {
        return 0.0;
    }
    let cleaned: Vec<String> = titles.iter().map(|t| clean_series_name(t)).collect();
    let first = match cleaned.first() {
        Some(f) => f,
        None => return 0.0,
    };
    if first.is_empty() {
        return 0.0;
    }
    let mut prefix_len = first.len();
    for other in cleaned.iter().skip(1) {
        let mut l = 0;
        for (a, b) in first.chars().zip(other.chars()) {
            if a == b {
                l += 1;
            } else {
                break;
            }
        }
        prefix_len = prefix_len.min(l);
    }
    if prefix_len < 2 {
        return 0.0;
    }
    let avg_len = cleaned
        .iter()
        .map(|s| s.chars().count())
        .sum::<usize>() as f32
        / cleaned.len() as f32;
    (prefix_len as f32 / avg_len).min(1.0)
}

/// 清理剧集名：移除季/集、年份、分辨率、编码格式后缀
pub fn clean_series_name(title: &str) -> String {
    let suffixes = [
        "1080p", "720p", "4k", "2160p", "480p", "bluray", "bdrip", "web-dl", "hdrip", "dvdrip",
        "x264", "x265", "h264", "h265", "hevc", "aac", "flac", "dts", "ac3", "remux", "proper",
        "repack", "chinese", "中文字幕", "国语", "粤语",
    ];
    let mut name = title.to_string();
    name = SEASON_EPISODE_RE.replace_all(&name, "").to_string();
    name = EPISODE_RE.replace_all(&name, "").to_string();
    name = YEAR_RE.replace_all(&name, "").to_string();
    let mut changed = true;
    while changed {
        changed = false;
        let lower = name.to_lowercase();
        for s in &suffixes {
            if let Some(pos) = lower.find(s) {
                if name.is_char_boundary(pos) && name.is_char_boundary(pos + s.len()) {
                    name = format!("{}{}", &name[..pos], &name[pos + s.len()..]);
                    changed = true;
                    break;
                }
            }
        }
    }
    name = name.replace(['.', '_', '-'], " ");
    name.split_whitespace().collect::<Vec<_>>().join(" ").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(title: &str, parent: &str, siblings: &[&str]) -> Classification {
        let sib: Vec<String> = siblings.iter().map(|s| s.to_string()).collect();
        classify(title, parent, None, &sib)
    }

    #[test]
    fn season_episode_is_series() {
        let c = t("Breaking.Bad.S01E01.1080p", "Breaking Bad", &[]);
        assert_eq!(c.kind, "series");
        assert_eq!(c.season, Some(1));
        assert_eq!(c.episode, Some(1));
    }

    #[test]
    fn chinese_episode_is_series() {
        // file_stem 语义：目录名"武林外传"，文件以目录名为前缀且带数字序号
        let c = t("武林外传40", "武林外传", &["武林外传40", "武林外传41", "武林外传42"]);
        assert_eq!(c.kind, "series", "got kind={} conf={} season={:?} ep={:?} name={:?}", c.kind, c.confidence, c.season, c.episode, c.series_name);
    }

    #[test]
    fn plain_movie() {
        let c = t("Avengers.Endgame.2019.1080p", "Avengers Endgame (2019)", &[]);
        assert_eq!(c.kind, "movie");
    }

    #[test]
    fn season_dir_with_grandparent() {
        let c = classify("01.720p.mkv", "Season 1", Some("Better Call Saul"), &[]);
        assert_eq!(c.kind, "series");
        assert_eq!(c.series_name.as_deref(), Some("Better Call Saul"));
    }

    #[test]
    fn low_confidence_is_unknown() {
        // 3 个共享前缀的目录文件，含数字序号 → 可能剧集，待确认（siblings 含自身）
        let c = t("random.ep.01", "misc", &["random.ep.01", "random.ep.02", "random.ep.03"]);
        assert!(c.kind == "series" || c.kind == "unknown", "got kind={}", c.kind);
    }
}
