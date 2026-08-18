use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::classifier::{self, Classification};

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "rmvb", "wmv", "flv", "mov", "ts", "m4v", "mpg", "mpeg", "webm", "vob",
    "ogv", "3gp", "f4v", "mts", "m2ts", "divx", "asf", "rm", "tp",
];

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
                    files.push(e.path().to_path_buf());
                }
            }
            Err(e) => errors.push(e.to_string()),
        }
    }
    (files, errors)
}

/// 增量扫描（带错误收集）：只返回数据库中不存在的文件
pub fn scan_directory_incremental_reporting(
    path: &str,
    existing_paths: &[String],
) -> (Vec<PathBuf>, Vec<String>) {
    let existing: std::collections::HashSet<&str> =
        existing_paths.iter().map(|s| s.as_str()).collect();
    let mut files = Vec::new();
    let mut errors = Vec::new();
    for entry in WalkDir::new(path).into_iter() {
        match entry {
            Ok(e) => {
                if !e.file_type().is_file() || !is_video_file(e.path()) {
                    continue;
                }
                let path_str = e.path().to_string_lossy();
                if !existing.contains(path_str.as_ref()) {
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

    VideoInfo {
        title,
        file_path,
        file_name,
        file_size,
        kind: c.kind,
        confidence: c.confidence,
        series_name: c.series_name,
        season: c.season,
        episode: c.episode,
    }
}
