use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use walkdir::WalkDir;
use regex::Regex;

/// 视频文件扩展名
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "rmvb", "wmv", "flv", "mov", "ts", "m4v",
    "mpg", "mpeg", "webm", "vob", "ogv", "3gp", "f4v", "mts", "m2ts",
    "divx", "asf", "rm", "tp",
];

// ============================================
// 正则定义
// NEW-5: 加回中文格式支持
// NEW-6: 字符类写法明确化
// NEW-7: 去掉单字母 E，只匹配 EP/Episode
// ============================================

/// 分隔符字符类，统一定义避免歧义
const SEP: &str = r"[\s._\-\[\]()（）]";

/// 匹配完整季+集：S01E01, Season 1 Episode 2, 第1季第2集
static SEASON_EPISODE_RE: LazyLock<Regex> = LazyLock::new(|| {
    let pattern = format!(
        r"(?ix)(?:^|{sep})
        (?:S(?:eason)?\s*(\d{{1,2}})\s*(?:EP?|Episode)\s*(\d{{1,3}})
        |第\s*(\d{{1,2}})\s*季\s*第?\s*(\d{{1,3}})\s*集)
        (?:{sep}|$)",
        sep = SEP
    );
    Regex::new(&pattern).unwrap()
});

/// 匹配季号：S01, Season 1, 第1季
static SEASON_RE: LazyLock<Regex> = LazyLock::new(|| {
    let pattern = format!(
        r"(?ix)(?:^|{sep})
        (?:S(?:eason)?|第)\s*(\d{{1,2}})
        (?:季|{sep}|$)",
        sep = SEP
    );
    Regex::new(&pattern).unwrap()
});

/// 匹配集数：EP01, Episode 1, 第2集
static EPISODE_RE: LazyLock<Regex> = LazyLock::new(|| {
    let pattern = format!(
        r"(?ix)(?:^|{sep})
        (?:EP|Episode|第)\s*(\d{{1,3}})
        (?:集|{sep}|$)",
        sep = SEP
    );
    Regex::new(&pattern).unwrap()
});

/// 匹配年份
static YEAR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\((\d{4})\)").unwrap()
});

/// 判断文件是否是视频
fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| VIDEO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// 扫描目录，返回所有视频文件路径
pub fn scan_directory(path: &str) -> Vec<PathBuf> {
    WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && is_video_file(e.path()))
        .map(|e| e.path().to_path_buf())
        .collect()
}

/// 增量扫描：只返回数据库中不存在的文件
pub fn scan_directory_incremental(path: &str, existing_paths: &[String]) -> Vec<PathBuf> {
    let existing: std::collections::HashSet<&str> = existing_paths
        .iter()
        .map(|s| s.as_str())
        .collect();

    WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && is_video_file(e.path()))
        .filter(|e| {
            let path_str = e.path().to_string_lossy();
            !existing.contains(path_str.as_ref())
        })
        .map(|e| e.path().to_path_buf())
        .collect()
}

/// 从文件路径提取视频信息
pub fn extract_video_info(path: &Path) -> VideoInfo {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let title = path
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let file_size = std::fs::metadata(path)
        .map(|m| m.len() as i64)
        .unwrap_or(0);

    let file_path = path.to_string_lossy().to_string();

    let (series_name, season, episode) = detect_series_info(path, &title);

    VideoInfo {
        title,
        file_path,
        file_name,
        file_size,
        series_name,
        season,
        episode,
    }
}

/// #16: 目录视频数量缓存，避免重复 read_dir
static DIR_VIDEO_COUNTS: LazyLock<Mutex<HashMap<PathBuf, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 清空目录缓存，每次扫描前调用确保数据新鲜
pub fn clear_dir_cache() {
    DIR_VIDEO_COUNTS.lock().unwrap().clear();
}

/// #16: 统计同级目录下的视频文件数量（带缓存）
fn count_sibling_videos(path: &Path) -> usize {
    let parent = match path.parent() {
        Some(p) => p.to_path_buf(),
        None => return 0,
    };

    // 先查缓存
    {
        let cache = DIR_VIDEO_COUNTS.lock().unwrap();
        if let Some(&count) = cache.get(&parent) {
            return count;
        }
    }

    // 缓存未命中，读文件系统
    let count = std::fs::read_dir(&parent)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| is_video_file(&e.path()))
                .count()
        })
        .unwrap_or(0);

    // 写入缓存
    DIR_VIDEO_COUNTS.lock().unwrap().insert(parent, count);
    count
}

/// 智能检测剧集信息
fn detect_series_info(path: &Path, title: &str) -> (Option<String>, Option<i32>, Option<i32>) {
    // 检测 S01E01 / 第1季第2集 格式
    if let Some(caps) = SEASON_EPISODE_RE.captures(title) {
        // 英文格式：caps[1]=season, caps[2]=episode
        // 中文格式：caps[3]=season, caps[4]=episode
        let season = caps.get(1).or_else(|| caps.get(3))
            .and_then(|m| m.as_str().parse().ok());
        let episode = caps.get(2).or_else(|| caps.get(4))
            .and_then(|m| m.as_str().parse().ok());
        let series_name = clean_series_name(title);
        return (Some(series_name), season, episode);
    }

    // 检测单独的集数：EP01 / Episode 5 / 第2集
    let episode = EPISODE_RE.captures(title)
        .and_then(|caps| caps.get(1))
        .and_then(|m| m.as_str().parse().ok());

    // 检测单独的季号：S02 / Season 3 / 第1季
    let season = SEASON_RE.captures(title)
        .and_then(|caps| caps.get(1))
        .and_then(|m| m.as_str().parse().ok());

    // 检查父文件夹
    if let Some(parent) = path.parent() {
        let parent_name = parent
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        // 如果父文件夹名包含 Season/季
        if SEASON_RE.is_match(parent_name) {
            if let Some(grandparent) = parent.parent() {
                let series_name = grandparent
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                if !series_name.is_empty() {
                    return (Some(series_name), season, episode);
                }
            }
        }

        // #9: 阈值 5
        let sibling_videos = count_sibling_videos(path);
        if sibling_videos >= 5 && episode.is_some() {
            let series_name = parent_name.to_string();
            if !series_name.is_empty() {
                return (Some(series_name), season.or(Some(1)), episode);
            }
        }
    }

    // 如果只检测到集数，用父文件夹名作为剧集名
    if episode.is_some() {
        if let Some(parent) = path.parent() {
            let parent_name = parent
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            if !parent_name.is_empty() {
                return (Some(parent_name), season.or(Some(1)), episode);
            }
        }
    }

    (None, None, None)
}

/// #13: 清理剧集名称（大小写不敏感）
/// NEW-3: 循环移除所有出现
/// NEW-4: is_char_boundary 安全检查
fn clean_series_name(title: &str) -> String {
    let suffixes = [
        "1080p", "720p", "4k", "2160p", "480p",
        "bluray", "bdrip", "web-dl", "hdrip", "dvdrip",
        "x264", "x265", "h264", "h265", "hevc", "aac",
        "flac", "dts", "ac3", "remux", "proper", "repack",
    ];

    let mut name = title.to_string();

    // 移除季/集信息
    name = SEASON_EPISODE_RE.replace_all(&name, "").to_string();
    name = SEASON_RE.replace_all(&name, "").to_string();
    name = EPISODE_RE.replace_all(&name, "").to_string();

    // 移除年份
    name = YEAR_RE.replace_all(&name, "").to_string();

    // NEW-3: 循环移除所有 suffix，每轮重新计算 name_lower
    let mut changed = true;
    while changed {
        changed = false;
        let name_lower = name.to_lowercase();
        for suffix in &suffixes {
            if let Some(pos) = name_lower.find(suffix) {
                if name.is_char_boundary(pos) && name.is_char_boundary(pos + suffix.len()) {
                    name = format!("{}{}", &name[..pos], &name[pos + suffix.len()..]);
                    changed = true;
                    break;
                }
            }
        }
    }

    // 清理分隔符
    name = name.replace(['.', '_', '-'], " ");
    name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    name.trim().to_string()
}

/// 视频文件信息
pub struct VideoInfo {
    pub title: String,
    pub file_path: String,
    pub file_name: String,
    pub file_size: i64,
    pub series_name: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
}
