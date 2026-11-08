//! 本地视频静帧缩略图。
//!
//! 不联网、不依赖第三方 Rust crate：优先调用本机已有 ffmpeg 抽一帧，
//! 成功就缓存到 app_data/thumbnails/{video_id}.jpg。没有 ffmpeg、文件不可读
//! 或解码失败时返回 None，前端会显示确定性的文字片头卡，不出现灰框或破图。
//!
//! 缓存失效规则：源文件 mtime 晚于缩略图 mtime 时重新生成。文件被替换、
//! 重新压制都能自然刷新；仅移动路径且内容未变时继续复用，省一次解码。
//!
//! 另外提供「刷新封面图」：删掉旧 jpg，改在随机位置重新抽一帧。这是纯本地
//! 操作，不联网、不下载任何海报，也不引入第三方随机数 crate。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tauri::Manager;

use crate::db::DbConn;

const THUMB_WIDTH: u32 = 480;
const FFMPEG_TIMEOUT: Duration = Duration::from_secs(20);

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

static FFMPEG_PATH: OnceLock<Option<String>> = OnceLock::new();

/// 正在被「刷新封面」写同一张 jpg 的 id。卡片懒加载是全局并发 2 的队列，
/// 刷新时若恰好有同 id 的抽帧任务在飞，两个 ffmpeg 会互相覆盖 target。
/// 标记后让懒加载直接返回 None（前端刷新广播会补上最终图片），刷新结束自动摘除。
static REFRESHING_IDS: OnceLock<Mutex<HashSet<i64>>> = OnceLock::new();

fn refreshing_ids() -> &'static Mutex<HashSet<i64>> {
    REFRESHING_IDS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn mark_refreshing(video_id: i64) {
    if let Ok(mut set) = refreshing_ids().lock() {
        set.insert(video_id);
    }
}

fn unmark_refreshing(video_id: i64) {
    if let Ok(mut set) = refreshing_ids().lock() {
        set.remove(&video_id);
    }
}

fn is_refreshing(video_id: i64) -> bool {
    refreshing_ids()
        .lock()
        .map(|set| set.contains(&video_id))
        .unwrap_or(false)
}

fn quiet_command(program: &str) -> Command {
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// 解析 ffmpeg：优先找与 ffprobe 同目录的 ffmpeg，再查 PATH。
/// OnceLock 保证整个进程只探测一次，不会每张卡片都起一个 `-version` 子进程。
pub fn resolve_ffmpeg_cached() -> Option<&'static str> {
    FFMPEG_PATH
        .get_or_init(|| {
            if let Some(probe) = crate::media_info::resolve_ffprobe_cached() {
                let probe = Path::new(probe);
                let candidates = if cfg!(windows) {
                    vec![probe.with_file_name("ffmpeg.exe")]
                } else {
                    vec![probe.with_file_name("ffmpeg")]
                };
                for c in candidates {
                    if c.is_file() {
                        return Some(c.to_string_lossy().to_string());
                    }
                }
            }
            for name in ["ffmpeg", "ffmpeg.exe"] {
                let ok = quiet_command(name)
                    .arg("-version")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if ok {
                    return Some((*name).to_string());
                }
            }
            None
        })
        .as_deref()
}

/// 生成本地缩略图并返回 `data:image/jpeg;base64,...`。
/// 返回 Ok(None) 属于正常降级，前端不应弹错误 toast。
#[tauri::command]
pub async fn get_video_thumbnail(
    app: tauri::AppHandle,
    video_id: i64,
) -> Result<Option<String>, String> {
    load_thumbnail(app, video_id, false).await
}

/// 右键「刷新封面图」：先删掉旧缓存图，再在随机位置重新抽一帧。
///
/// 与常规加载共用同一个 jpg 文件；生成成功后它的 mtime 新于源文件，
/// 后续 `get_video_thumbnail` 会直接复用这张新图，不会被固定 20% 的逻辑覆盖。
/// 返回 Ok(None) 表示「旧图已删但没能生成新图」（无 ffmpeg / 源文件不在 /
/// 解码失败），前端把卡片降级为文字片头卡并给出提示。
#[tauri::command]
pub async fn refresh_video_thumbnail(
    app: tauri::AppHandle,
    video_id: i64,
) -> Result<Option<String>, String> {
    mark_refreshing(video_id);
    // 不把 MutexGuard 带过 await（否则 Future 失去 Send），用普通标记 + 收尾清理，
    // 即便 load_thumbnail 提前返回 Err 也一定会摘掉标记。
    let result = load_thumbnail(app, video_id, true).await;
    unmark_refreshing(video_id);
    result
}

/// `force = true` 走刷新分支（删旧图 + 随机落点），否则走常规加载（片长 20%）。
async fn load_thumbnail(
    app: tauri::AppHandle,
    video_id: i64,
    force: bool,
) -> Result<Option<String>, String> {
    // 只查一次路径与时长，锁立即释放；ffmpeg 解码在 blocking 线程上跑。
    let (source, duration) = {
        let db = app.state::<DbConn>();
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT file_path, duration FROM videos WHERE id = ?1",
            rusqlite::params![video_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<f64>>(1)?)),
        )
        .map_err(|e| format!("视频记录不存在: {e}"))?
    };

    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("读取应用数据目录失败: {e}"))?;
    let cache_dir = data_dir.join("thumbnails");
    std::fs::create_dir_all(&cache_dir).map_err(|e| format!("创建缩略图目录失败: {e}"))?;
    let target = cache_dir.join(format!("{video_id}.jpg"));

    // 刷新：无条件删掉老图。即使后面缺 ffmpeg / 源文件不在 / 解码失败，
    // 也不让旧封面继续挂着——用户要的就是「换一张」。
    if force {
        remove_one(&target);
    }

    let source_path = PathBuf::from(&source);
    if !source_path.is_file() {
        return Ok(None);
    }

    // 缓存有效：缩略图存在且不早于源文件修改时间。刷新时旧图已删，必然不命中。
    if let (Ok(tm), Ok(sm)) = (
        std::fs::metadata(&target).and_then(|m| m.modified()),
        std::fs::metadata(&source_path).and_then(|m| m.modified()),
    ) {
        if tm >= sm && target.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            return Ok(Some(read_data_url(&target)?));
        }
    }

    // 同 id 正在刷新时让路，别和刷新任务抢同一个 jpg 文件。
    if !force && is_refreshing(video_id) {
        return Ok(None);
    }

    let Some(ffmpeg) = resolve_ffmpeg_cached() else {
        return Ok(None);
    };
    let ffmpeg = ffmpeg.to_string();
    let source_for_task = source_path.clone();
    let target_for_task = target.clone();
    // 库里时长为空/异常时现场探测真实片长，两条路径都受益——默认封面
    // 也要按真实的 20% 落点，而不是盲抽第 5 秒。常规懒加载只走自研解析
    // （allow_external=false，零子进程），刷新属低频操作才允许 ffprobe 兜底；
    // 探测与抽帧一起放进 blocking 线程，不占 async runtime。
    let need_probe = !duration.is_some_and(|d| d.is_finite() && d > 2.0);

    let generated = tauri::async_runtime::spawn_blocking(move || {
        let effective = if need_probe {
            crate::media_info::probe_duration_sec(&source_for_task, force).or(duration)
        } else {
            duration
        };
        let seek = if force {
            random_seek(effective, video_id)
        } else {
            default_seek(effective)
        };
        generate_thumbnail(&ffmpeg, &source_for_task, &target_for_task, seek, force)
    })
    .await
    .map_err(|e| format!("缩略图任务异常: {e}"))?;

    if generated && target.is_file() {
        Ok(Some(read_data_url(&target)?))
    } else {
        Ok(None)
    }
}

/// 常规封面的抽帧位置：片长的 20%，无时长信息时退到第 5 秒。
///
/// **同样不能有 600 秒上限**（2026-11-08 修复）：电影普遍 100+ 分钟，
/// `clamp(0.5, 600)` 会把 20% 硬砍成第 10 分钟，等于所有长片封面全长一个样。
/// 0.2 × 片长恒小于片长，无需上限。
fn default_seek(duration: Option<f64>) -> f64 {
    duration
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(|d| (d * 0.2).max(0.5))
        .unwrap_or(5.0)
}

/// 刷新用的随机抽帧位置。不引第三方 rand crate：刷新是低频人工操作，
/// 用「纳秒时间 + video_id」经 xorshift 打散即可，完全离线且可审计。
/// 比例落在 [0.10, 0.90)，刻意避开片头字幕与片尾名单这种千篇一律的画面。
///
/// **绝不能给这里加 600 秒上限**（2026-11-08 修复）：当时照抄了默认封面
/// 的 `clamp(0.5, 600)`，片长一旦超过 50 分钟，随机的 d×ratio 就大面积
/// 撞上 600 秒上限，全部坍缩成同一帧——表现即「刷新封面怎么点都是同一张图」。
/// 按片长比例取值，天然不会越界。
fn random_seek(duration: Option<f64>, video_id: i64) -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = nanos ^ (video_id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    let ratio = 0.10 + (x % 1000) as f64 / 1000.0 * 0.80;
    match duration.filter(|d| d.is_finite() && *d > 2.0) {
        // ratio < 0.90 ⇒ 结果恒小于片长，无需 clamp 上限。
        Some(d) => (d * ratio).max(0.5),
        // 时长探测也拿不到时，在 10~50 秒里随机落点；越过片长会由
        // generate_thumbnail 分级退让，不会每次都跌回 0 秒那张恒定画面。
        None => 5.0 + ratio * 50.0,
    }
}

/// 删除单个视频的缩略图缓存。**幂等**：文件本就不存在视为已完成。
///
/// 失败不向上传播：记录已经从库里消失，残留一个几十 KB 的 jpg 只是占地方，
/// 不该让「删电影」这个动作报错。`videos.id` 是 `AUTOINCREMENT`（id 不复用），
/// 所以万一漏删，孤儿文件也不会被将来的新记录误认成自己的封面。
pub fn remove_cached_thumbnail(app: &tauri::AppHandle, video_id: i64) {
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    remove_one(&dir.join("thumbnails").join(format!("{video_id}.jpg")));
}

/// 批量清理（删剧集时一次清掉几十集）。空列表直接返回，不做任何 IO。
pub fn remove_cached_thumbnails(app: &tauri::AppHandle, video_ids: &[i64]) {
    if video_ids.is_empty() {
        return;
    }
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let thumbs = dir.join("thumbnails");
    for id in video_ids {
        remove_one(&thumbs.join(format!("{id}.jpg")));
    }
}

fn remove_one(target: &Path) {
    match std::fs::remove_file(target) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        // 被别的进程占着（正在读图 / 杀毒扫描）也只是一次清理失败，
        // 记一行供排查，绝不把异常抛回去打断删除主流程。
        Err(e) => eprintln!("[cinevault] 缩略图清理失败 {}: {e}", target.display()),
    }
}

fn read_data_url(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取缩略图失败: {e}"))?;
    if bytes.is_empty() {
        return Err("缩略图为空".to_string());
    }
    Ok(format!("data:image/jpeg;base64,{}", base64_encode(&bytes)))
}

fn generate_thumbnail(
    ffmpeg: &str,
    source: &Path,
    target: &Path,
    seek: f64,
    force: bool,
) -> bool {
    if extract_once(ffmpeg, source, target, seek) {
        return true;
    }
    // 刷新专属兜底：DB 时长偏大或随机落点越过片长时，先退到一半的位置
    // （每次仍不同），避免直接跌到 0 秒那张恒定片头帧。常规加载路径不进这里，
    // 保持原有的「一次失败 → 回片头」行为不变。
    if force && seek > 1.0 && extract_once(ffmpeg, source, target, seek * 0.5) {
        return true;
    }
    // 有些文件关键帧/时长元数据异常，指定位置抓不到帧；回到开头再试一次。
    seek > 0.6 && extract_once(ffmpeg, source, target, 0.0)
}

fn extract_once(ffmpeg: &str, source: &Path, target: &Path, seek: f64) -> bool {
    let _ = std::fs::remove_file(target);
    let scale_filter = format!("scale={THUMB_WIDTH}:-2");
    let mut child = match quiet_command(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-ss"])
        .arg(format!("{seek:.3}"))
        .arg("-i")
        .arg(source)
        .args([
            "-frames:v",
            "1",
            "-vf",
            scale_filter.as_str(),
            "-q:v",
            "4",
            "-y",
        ])
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    let deadline = Instant::now() + FFMPEG_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return status.success()
                    && target
                        .metadata()
                        .map(|m| m.is_file() && m.len() > 0)
                        .unwrap_or(false);
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(40));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(target);
                return false;
            }
        }
    }
}

/// 极简 Base64（RFC 4648 标准表，带 `=` 填充）。只处理 JPEG 字节，
/// 为此引一个 crate 不值得；实现和测试都保持可审计。
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        base64_encode, default_seek, generate_thumbnail, random_seek, resolve_ffmpeg_cached,
    };
    use std::process::{Command, Stdio};

    #[test]
    fn seek_positions_stay_in_range() {
        // 常规加载仍是原来的固定 20%，刷新不能改变既有行为。
        assert_eq!(default_seek(Some(100.0)), 20.0);
        assert_eq!(default_seek(None), 5.0);
        for id in 0..64 {
            let with_duration = random_seek(Some(100.0), id);
            assert!(
                (10.0..=90.0).contains(&with_duration),
                "id={id} seek={with_duration}"
            );
            let no_duration = random_seek(None, id);
            assert!(
                (5.0..=55.0).contains(&no_duration),
                "id={id} seek={no_duration}"
            );
        }
    }

    /// 回归：2026-11-08 报障「刷新封面怎么点都是同一张图」。
    /// 当时 random_seek 把 d×ratio 也 clamp 到 600 秒，2 小时以上的片子
    /// 所有落点全变成 600.0，连默认封面也是 600 秒，刷新前后自然一模一样。
    #[test]
    fn random_seek_varies_for_long_videos() {
        let seeks: Vec<f64> = (0..32)
            .map(|id| random_seek(Some(8700.7), id))
            .collect();
        for (id, s) in seeks.iter().enumerate() {
            assert!(
                *s > 600.0 && *s < 8700.7,
                "长片落点不能撞 600 秒上限: id={id} seek={s}"
            );
        }
        let min = seeks.iter().copied().fold(f64::INFINITY, f64::min);
        let max = seeks.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!(
            max - min > 3000.0,
            "32 次刷新的落点应铺开在大部分片长上，实际 {min:.1}..{max:.1}"
        );
    }

    /// 回归：默认封面也被同一个 600 秒上限砍过，2 小时片一律抽第 10 分钟。
    #[test]
    fn default_seek_uses_full_runtime() {
        let d = 8700.7;
        let s = default_seek(Some(d));
        assert!((s - d * 0.2).abs() < 1e-6, "应落在片长 20%: {s}");
        assert!(s > 600.0, "2 小时片不能还抽第 10 分钟");
        assert_eq!(default_seek(Some(100.0)), 20.0);
        assert_eq!(default_seek(None), 5.0);
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    /// 有 ffmpeg 时做一次真实抽帧；没有就跳过（测试不把外部工具变成硬依赖）。
    #[test]
    fn real_ffmpeg_extracts_jpeg() {
        let Some(ffmpeg) = resolve_ffmpeg_cached() else {
            return;
        };
        let dir = std::env::temp_dir().join(format!(
            "cinevault-thumb-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("sample.mp4");
        let target = dir.join("thumb.jpg");
        let made = Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x180:rate=24",
                "-t",
                "12",
                "-pix_fmt",
                "yuv420p",
                "-y",
            ])
            .arg(&source)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !made {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        assert!(
            generate_thumbnail(ffmpeg, &source, &target, 0.4, false),
            "ffmpeg 应能为测试视频生成缩略图"
        );
        assert!(target.metadata().map(|m| m.len() > 0).unwrap_or(false));
        let bytes = std::fs::read(&target).unwrap();
        assert_eq!(&bytes[..2], [0xFF, 0xD8], "输出应是 JPEG");

        // 端到端印证「换落点 = 换画面」：合成片画面逐帧变化，
        // 两个相距很远的 -ss 必须抽出不同的 JPEG。
        let other = dir.join("thumb2.jpg");
        assert!(generate_thumbnail(ffmpeg, &source, &other, 9.0, false));
        assert_ne!(
            bytes,
            std::fs::read(&other).unwrap(),
            "不同抽帧位置不能得到同一张图"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
