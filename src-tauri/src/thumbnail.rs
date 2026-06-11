use std::path::Path;
use std::process::Command;
use std::fs;

/// 生成视频缩略图
/// #11: 动态计算截取时间点（视频时长的 10%，避免短视频黑屏）
/// #25: 用 Path::join 拼接路径
pub fn generate_thumbnail(video_path: &str, thumb_dir: &str) -> Option<String> {
    let video = Path::new(video_path);
    let file_stem = video.file_stem()?.to_str()?;

    // #25: 用 Path::join 正确拼接路径
    let thumb_dir_path = Path::new(thumb_dir);
    let _ = fs::create_dir_all(thumb_dir_path);
    let thumb_path = thumb_dir_path.join(format!("{}.jpg", file_stem));
    let thumb_path_str = thumb_path.to_string_lossy().to_string();

    // 如果缩略图已存在，直接返回
    if thumb_path.exists() {
        return Some(thumb_path_str);
    }

    // 获取视频时长（秒），用于计算截取位置
    let seek_time = get_video_seek_time(video_path);

    // 尝试用 ffmpeg 截取缩略图
    let output = Command::new("ffmpeg")
        .args([
            "-i", video_path,
            "-ss", &seek_time,
            "-vframes", "1",
            "-vf", "scale=320:-1",
            "-q:v", "3",
            "-y",
            &thumb_path_str,
        ])
        .output();

    match output {
        Ok(result) => {
            if result.status.success() && thumb_path.exists() {
                Some(thumb_path_str)
            } else {
                // ffmpeg 失败，尝试简单方式
                generate_thumbnail_from_first_frame(video_path, &thumb_path_str)
            }
        }
        Err(_) => {
            // ffmpeg 不存在，尝试简单方式
            generate_thumbnail_from_first_frame(video_path, &thumb_path_str)
        }
    }
}

/// #11: 获取视频截取时间点（时长的 10%，最少 1 秒，最多 120 秒）
fn get_video_seek_time(video_path: &str) -> String {
    // 尝试用 ffprobe 获取时长
    let output = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
            video_path,
        ])
        .output();

    if let Ok(result) = output {
        if result.status.success() {
            let duration_str = String::from_utf8_lossy(&result.stdout);
            if let Ok(duration) = duration_str.trim().parse::<f64>() {
                // 取时长的 10%，最少 1 秒，最多 120 秒
                let seek = (duration * 0.1).clamp(1.0, 120.0);
                return format!("{:.0}", seek);
            }
        }
    }

    // 无法获取时长，默认 90 秒
    "90".to_string()
}

/// #12: 从视频第一帧生成缩略图（不 seek，直接取第一帧）
fn generate_thumbnail_from_first_frame(video_path: &str, thumb_path: &str) -> Option<String> {
    let output = Command::new("ffmpeg")
        .args([
            "-i", video_path,
            "-vframes", "1",
            "-vf", "scale=320:-1",
            "-q:v", "3",
            "-y",
            thumb_path,
        ])
        .output();

    match output {
        Ok(result) => {
            if result.status.success() && Path::new(thumb_path).exists() {
                Some(thumb_path.to_string())
            } else {
                None
            }
        }
        Err(_) => None,
    }
}
