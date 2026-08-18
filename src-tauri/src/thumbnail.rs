use std::fs;
use std::path::Path;
use std::process::Command;

/// 生成视频截帧（作为无封面时的占位兜底；UI 当前不渲染，保留能力）
pub fn generate_thumbnail(video_path: &str, thumb_dir: &str) -> Option<String> {
    let video = Path::new(video_path);
    let file_stem = video.file_stem()?.to_str()?;
    let thumb_dir_path = Path::new(thumb_dir);
    let _ = fs::create_dir_all(thumb_dir_path);
    let thumb_path = thumb_dir_path.join(format!("{}.jpg", file_stem));
    let thumb_path_str = thumb_path.to_string_lossy().to_string();

    if thumb_path.exists() {
        return Some(thumb_path_str);
    }

    let seek_time = get_video_seek_time(video_path);

    let output = Command::new("ffmpeg")
        .args([
            "-i",
            video_path,
            "-ss",
            &seek_time,
            "-vframes",
            "1",
            "-vf",
            "scale=320:-1",
            "-q:v",
            "3",
            "-y",
            &thumb_path_str,
        ])
        .output();

    match output {
        Ok(result) => {
            if result.status.success() && thumb_path.exists() {
                Some(thumb_path_str)
            } else {
                generate_thumbnail_from_first_frame(video_path, &thumb_path_str)
            }
        }
        Err(_) => generate_thumbnail_from_first_frame(video_path, &thumb_path_str),
    }
}

fn get_video_seek_time(video_path: &str) -> String {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            video_path,
        ])
        .output();

    if let Ok(result) = output {
        if result.status.success() {
            let duration_str = String::from_utf8_lossy(&result.stdout);
            if let Ok(duration) = duration_str.trim().parse::<f64>() {
                let seek = (duration * 0.1).clamp(1.0, 120.0);
                return format!("{:.0}", seek);
            }
        }
    }
    "90".to_string()
}

fn generate_thumbnail_from_first_frame(video_path: &str, thumb_path: &str) -> Option<String> {
    let output = Command::new("ffmpeg")
        .args([
            "-i",
            video_path,
            "-vframes",
            "1",
            "-vf",
            "scale=320:-1",
            "-q:v",
            "3",
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
