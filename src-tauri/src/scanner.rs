use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::classifier::{self, Classification};

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "rmvb", "wmv", "flv", "mov", "ts", "m4v", "mpg", "mpeg", "webm", "vob",
    "ogv", "3gp", "f4v", "mts", "m2ts", "divx", "asf", "rm", "tp",
];

/// 体积下限默认值（字节）：小于该值的视频文件不入库，刷新扫描时一并清除记录。
/// 50MB 的依据（2026-09-02 生产库 5216 条实测）：50~110MB 区间 662 条几乎全是正片
/// （海绵宝宝/Family Guy/灌篮高手 HALFCD 等），50MB 以下垃圾浓度高（广告水印/BD Menu/预告片/sample）。
/// 实际值读取设置 scan_min_size_mb；50MB 以下仍需保留的正片走可配置的豁免前缀。
pub const MIN_VIDEO_SIZE_BYTES: u64 = 50 * 1024 * 1024;

/// 体积豁免名单默认值（文件名前缀）：名单内文件不受体积下限限制，扫描照常入库、刷新不删。
/// 「欢乐正前方」27 集横跨 9.87~131.95MB，低段 14 集小于 50MB。
/// 「Pingu」企鹅家族 104 集单集 15~36MB。
/// 实际扫描读取设置里的 scan_size_exempt_prefixes，默认值仅用于首次启动/旧库。
pub const SIZE_EXEMPT_PREFIXES: &[&str] = &["欢乐正前方", "Pingu"];

/// 是否豁免体积过滤（按文件名前缀匹配）
pub fn is_size_exempt_with(file_name: &str, prefixes: &[String]) -> bool {
    prefixes.iter().any(|p| !p.is_empty() && file_name.starts_with(p))
}

/// 默认豁免名单的兼容入口（仅测试使用）；扫描命令走配置版。
#[cfg(test)]
pub fn is_size_exempt(file_name: &str) -> bool {
    SIZE_EXEMPT_PREFIXES.iter().any(|p| file_name.starts_with(p))
}

pub fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| VIDEO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// 扫描目录（带错误收集）：返回 (视频文件列表, 遍历错误)。
/// 体积下限与豁免前缀由设置传入；三个入库口共用，避免规则各处漂移。
pub fn scan_directory_reporting_with(
    path: &str,
    min_size_bytes: u64,
    exempt_prefixes: &[String],
) -> (Vec<PathBuf>, Vec<String>) {
    let mut files = Vec::new();
    let mut errors = Vec::new();
    for entry in WalkDir::new(path).into_iter() {
        match entry {
            Ok(e) => {
                if e.file_type().is_file() && is_video_file(e.path()) {
                    // 体积下限：小文件（广告水印/BD Menu/预告片/sample）直接不进扫描列表。
                    // walkdir 的 DirEntry::metadata() 在 Windows 上直接用目录枚举缓存，无额外系统调用。
                    if let Ok(meta) = e.metadata() {
                        if meta.len() < min_size_bytes {
                            let name = e.file_name().to_string_lossy();
                            if !is_size_exempt_with(&name, exempt_prefixes) {
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
    /// 合体文件（一个文件含多集）的末集号，单集文件为 None
    pub episode_end: Option<i32>,
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
        episode_end: c.episode_end,
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

    /// Pingu 单集 15~36MB，整部都在 50MB 门槛以下。扫描入口
    /// （scan_directory_reporting）与刷新清理（refresh_all_scans 第二阶段）
    /// 共用 is_size_exempt，这里同时钉住两条语义，避免日后改名单时漏掉一边。
    #[test]
    fn test_pingu_is_size_exempt() {
        assert!(is_size_exempt("Pingu - 01. Hello Pingu - (Original VHS version - HQ - Restored).ia.mp4"));
        assert!(is_size_exempt("Pingu - 104 - Pingu the King.mp4"));
        assert!(is_size_exempt("Pingu - A Very Special Wedding (1997).ia.mp4"));
        // 前缀是字符串前缀匹配（宽松），"Pinguino.mp4" 也会命中 —— 记录真实行为，不做假断言
        assert!(is_size_exempt("Pinguino.mp4"));
        // 大小写敏感：小写 pingu 不豁免（本机文件名首字母是大写 P）
        assert!(!is_size_exempt("pingu - 01.mp4"));
        // 刷新阶段的实际判定式：体积达标与否只看这条
        let small = 15 * 1024 * 1024u64;
        assert!(small < MIN_VIDEO_SIZE_BYTES);
        assert!(!is_size_exempt("海绵宝宝.S01E01.mp4"));
    }

    #[test]
    fn test_min_video_size_value() {
        assert_eq!(MIN_VIDEO_SIZE_BYTES, 52_428_800);
    }
}
