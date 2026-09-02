use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::classifier::{self, Classification};

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "rmvb", "wmv", "flv", "mov", "ts", "m4v", "mpg", "mpeg", "webm", "vob",
    "ogv", "3gp", "f4v", "mts", "m2ts", "divx", "asf", "rm", "tp",
];

/// 体积下限（字节）：小于该值的视频文件不入库，刷新扫描时一并清除记录。
/// 50MB 的依据（2026-09-02 生产库 5216 条实测）：50~110MB 区间 662 条几乎全是正片
/// （海绵宝宝/Family Guy/灌篮高手 HALFCD 等），50MB 以下垃圾浓度高（广告水印/BD Menu/预告片/sample）。
/// 50MB 以下仍需保留的正片走 SIZE_EXEMPT_PREFIXES 豁免。
pub const MIN_VIDEO_SIZE_BYTES: u64 = 50 * 1024 * 1024;

/// 体积豁免名单（文件名前缀）：名单内文件不受体积下限限制，扫描照常入库、刷新不删。
/// 「欢乐正前方」27 集横跨 9.87~131.95MB，低段 14 集小于 50MB，属用户点名保留的正片。
/// 以后若有同类"小体积正片"，加进这个数组即可。
pub const SIZE_EXEMPT_PREFIXES: &[&str] = &["欢乐正前方"];

/// 是否豁免体积过滤（按文件名前缀匹配）
pub fn is_size_exempt(file_name: &str) -> bool {
    SIZE_EXEMPT_PREFIXES.iter().any(|p| file_name.starts_with(p))
}

pub fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| VIDEO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// 扫描目录（带错误收集）：返回 (视频文件列表, 遍历错误)
pub fn scan_directory_reporting(path: &str) -> (Vec<PathBuf>, Vec<String>) {
    let mut files = Vec::new();
    let mut errors = Vec::new();
    for entry in WalkDir::new(path).into_iter() {
        match entry {
            Ok(e) => {
                if e.file_type().is_file() && is_video_file(e.path()) {
                    // 体积下限：小文件（广告水印/BD Menu/预告片/sample）直接不进扫描列表，
                    // 三个入库口（scan_folder/add_series_from_folder/refresh_all_scans）共用此处，一处过滤全覆盖。
                    // walkdir 的 DirEntry::metadata() 在 Windows 上直接用目录枚举缓存，无额外系统调用。
                    if let Ok(meta) = e.metadata() {
                        if meta.len() < MIN_VIDEO_SIZE_BYTES {
                            let name = e.file_name().to_string_lossy();
                            if !is_size_exempt(&name) {
                                continue;
                            }
                        }
                    }
                    files.push(e.path().to_path_buf());
                }
            }
            Err(e) => errors.push(e.to_string()),
        }
    }
    (files, errors)
}

pub struct VideoInfo {
    pub title: String,
    pub file_path: String,
    pub file_name: String,
    pub file_size: i64,
    pub duration: Option<f64>,
    pub kind: String,
    pub confidence: i32,
    pub series_name: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
}

/// 按目录分组统计同级视频标题，供分类引擎使用
fn build_sibling_map(files: &[PathBuf]) -> HashMap<PathBuf, Vec<String>> {
    let mut map: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for f in files {
        if let Some(parent) = f.parent() {
            let title = f
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            map.entry(parent.to_path_buf()).or_default().push(title);
        }
    }
    map
}

/// 公开版：按目录分组统计同级视频标题（供命令层使用）
pub fn build_sibling_map_public(files: &[PathBuf]) -> HashMap<PathBuf, Vec<String>> {
    build_sibling_map(files)
}

/// 提取视频信息（含三层分类）
pub fn extract_video_info(path: &Path, sibling_map: &HashMap<PathBuf, Vec<String>>) -> VideoInfo {
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
    let file_size = std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0);
    let file_path = path.to_string_lossy().to_string();

    let parent_name = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let grandparent_name = path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string());
    let siblings: Vec<String> = sibling_map
        .get(path.parent().map(|p| p.to_path_buf()).as_ref().unwrap_or(&PathBuf::new()))
        .cloned()
        .unwrap_or_default();

    let c: Classification = classifier::classify(&title, &parent_name, grandparent_name.as_deref(), &siblings);

    // 时长：只走自研容器解析（mp4/mkv，读文件头，零子进程）。
    // 扫描动辄几千个文件，这里若启 ffprobe（每文件一个进程，50~100ms）会把扫描从几十秒拖到近十分钟。
    // 非主流格式（rmvb/flv/avi/ts）留到"补全片长"或同步时按需用 ffprobe 补，扫描速度优先。
    let duration = crate::media_info::probe_duration_sec(path, false);

    VideoInfo {
        title,
        file_path,
        file_name,
        file_size,
        duration,
        kind: c.kind,
        confidence: c.confidence,
        series_name: c.series_name,
        season: c.season,
        episode: c.episode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_size_exempt_prefix_match() {
        assert!(is_size_exempt("欢乐正前方.大灰狼和三只小猪的故事.mkv"));
        assert!(is_size_exempt("欢乐正前方.mkv"));
        assert!(!is_size_exempt("欢乐正前.mkv"));
        assert!(!is_size_exempt("海绵宝宝.S01E01.mp4"));
        assert!(!is_size_exempt(""));
    }

    #[test]
    fn test_min_video_size_value() {
        assert_eq!(MIN_VIDEO_SIZE_BYTES, 52_428_800);
    }
}
