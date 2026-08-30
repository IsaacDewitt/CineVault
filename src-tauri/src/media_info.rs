//! 视频时长提取。
//!
//! 分两条路（**顺序很关键，别调换**）：
//!   1. 自研容器解析（优先，零子进程）—— 只支持 mp4 / mkv，覆盖库内 95.6% 的文件
//!   2. ffprobe（兜底，全格式准确）—— 需要本机有 ffmpeg，路径可配置
//!
//! **单位约定（改动前必读）**：
//!   - 本模块一律返回 **秒**（f64），与 videos.duration 一致（前端 `Math.round(duration/60)` 按分钟显示）。
//!   - PotPlayer 注册表播放位置是 **毫秒**，dpl 的 duration2 也是 **毫秒**。
//!     算百分比时必须统一：`pos_ms / 1000.0 / duration_sec`，混用会差 1000 倍。

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Duration;

/// 探测视频时长（秒）。
/// `allow_external = false` 时只走自研解析（不启子进程），用于扫描等批量热路径。
pub fn probe_duration_sec(path: &Path, allow_external: bool) -> Option<f64> {
    // **自研解析优先**（2026-08-30 调整，原顺序是反的）：
    // 自研解析 = 纯文件 IO，读几十 KB 头（实测 0.03 ms）；ffprobe = 子进程
    // （探测 78 ms + 路径解析 30 ms）。库内 95.6%（4853/5078）是 mp4/mkv，自研即可搞定，
    // 真正需要 ffprobe 的只有 rmvb/flv/avi/ts 等 225 个。
    // 顺序反过来时，批量补全 5071 个文件要 9.1 分钟；调整后约 18 秒。
    if let Some(sec) = parse_container_duration_sec(path) {
        if sec > 0.0 {
            return Some(sec);
        }
    }
    // 自研拿不到（格式不支持 / 头部损坏）才付子进程代价
    if allow_external {
        if let Some(sec) = ffprobe_duration_sec(path) {
            if sec > 0.0 {
                return Some(sec);
            }
        }
    }
    None
}

// ============================================================
// 1. ffprobe
// ============================================================

/// Windows 专属：禁止子进程创建控制台窗口（CREATE_NO_WINDOW = 0x0800_0000）。
/// 本程序是 GUI 程序（windows_subsystem="windows"），起 ffprobe 这类控制台程序时
/// 系统会为其分配一个新控制台 → 每次探测都闪一个黑窗。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Windows 下给 Command 挂 CREATE_NO_WINDOW；其他平台原样返回。
fn quiet_command(program: &str) -> Command {
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// 解析 ffprobe 可执行文件路径：设置项 → PATH → 常见安装位。
/// `configured` 为用户在设置里填的路径（可为 None）。
pub fn resolve_ffprobe(configured: Option<&str>) -> Option<String> {
    if let Some(p) = configured.map(str::trim).filter(|s| !s.is_empty()) {
        if Path::new(p).is_file() {
            return Some(p.to_string());
        }
    }
    // PATH 查找（Windows 上 ffprobe.exe / ffprobe 都试）
    for name in ["ffprobe", "ffprobe.exe"] {
        if probe_runs(name) {
            return Some(name.to_string());
        }
    }
    // 常见安装位兜底
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(root) = std::env::var("ProgramFiles") {
        candidates.push(format!(r"{}\ffmpeg\bin\ffprobe.exe", root));
    }
    if let Ok(root) = std::env::var("ProgramFiles(x86)") {
        candidates.push(format!(r"{}\ffmpeg\bin\ffprobe.exe", root));
    }
    candidates.push(r"C:\ffmpeg\bin\ffprobe.exe".to_string());
    candidates.push(r"C:\ProgramData\chocolatey\bin\ffprobe.exe".to_string());
    for c in candidates {
        if Path::new(&c).is_file() {
            return Some(c);
        }
    }
    None
}

/// 该命令能否真的跑起来（`-version` 成功）。用来排除 PATH 同名垃圾文件。
fn probe_runs(cmd: &str) -> bool {
    quiet_command(cmd)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// ffprobe 可执行文件路径的**进程级缓存**。
///
/// `resolve_ffprobe` 的 PATH 查找会真的起一次 `ffprobe -version` 子进程并同步等它退出
/// （`probe_runs`，实测 30 ms/次）。不缓存的话，批量补全 5000 个文件 = 白烧 150 秒，
/// 8 个探测线程并发时更是进程创建风暴。进程生命周期内 ffprobe 的安装位置不会变，解析一次即可。
///
/// 注意：这里缓存的是 `configured = None`（自动查找）的结果。若将来设置页支持自定义路径，
/// 那条路径要单独调 `resolve_ffprobe`，不要复用本缓存。
static FFPROBE_PATH: OnceLock<Option<String>> = OnceLock::new();

/// 缓存版 ffprobe 路径解析。时长探测与设置页展示统一走这里。
pub fn resolve_ffprobe_cached() -> Option<&'static str> {
    FFPROBE_PATH.get_or_init(|| resolve_ffprobe(None)).as_deref()
}

/// 调 ffprobe 取时长（秒）。带 15 秒超时，超时则杀进程返回 None。
pub fn ffprobe_duration_sec(path: &Path) -> Option<f64> {
    // 走缓存：每次都重新解析的话，光是 `ffprobe -version` 就要多付 30 ms/文件
    let exe = resolve_ffprobe_cached()?;
    let mut child = quiet_command(exe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let mut stdout = child.stdout.take()?;
    // 读 stdout 放到独立线程：管道缓冲满时子进程会阻塞，主线程等超时后 kill 掉即可
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });

    let output = match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(s) => s,
        Err(_) => {
            let _ = child.kill();
            return None;
        }
    };
    match child.wait() {
        Ok(st) if st.success() => {}
        _ => return None,
    }
    parse_seconds(&output)
}

fn parse_seconds(raw: &str) -> Option<f64> {
    raw.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter_map(|l| l.parse::<f64>().ok())
        .find(|v| *v > 0.0)
}

// ============================================================
// 2. 自研容器解析
// ============================================================

fn parse_container_duration_sec(path: &Path) -> Option<f64> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "mp4" | "m4v" | "mov" => parse_mp4_duration_sec(path),
        "mkv" | "webm" => parse_mkv_duration_sec(path),
        _ => None,
    }
}

// ---------------- MP4 / MOV（ISO BMFF） ----------------

const MP4_HEAD_READ_LIMIT: usize = 64 * 1024 * 1024; // moov 超过 64MB 就放弃，交给 ffprobe

/// 遍历顶层 box 找 moov（mdat 等大 box 直接按 size seek 跳过，不读进内存）。
pub fn parse_mp4_duration_sec(path: &Path) -> Option<f64> {
    let mut f = std::fs::File::open(path).ok()?;
    let file_len = f.metadata().ok()?.len();
    let mut pos: u64 = 0;
    while pos + 8 <= file_len {
        if f.seek(SeekFrom::Start(pos)).is_err() {
            return None;
        }
        let mut hdr = [0u8; 8];
        if f.read_exact(&mut hdr).is_err() {
            return None;
        }
        let mut size = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
        let mut header_len = 8u64;
        if size == 1 {
            // 64 位 largesize
            let mut ext = [0u8; 8];
            if f.read_exact(&mut ext).is_err() {
                return None;
            }
            size = u64::from_be_bytes(ext);
            header_len = 16;
        } else if size == 0 {
            size = file_len.saturating_sub(pos); // 到文件末尾
        }
        // size 小于头部 = 结构损坏 / 无法前进，止损
        if size < header_len {
            return None;
        }
        if &hdr[4..8] == b"moov" {
            let moov_len = size - header_len;
            if moov_len > MP4_HEAD_READ_LIMIT as u64 {
                return None;
            }
            let mut buf = vec![0u8; moov_len as usize];
            if f.read_exact(&mut buf).is_err() {
                return None;
            }
            return find_mvhd(&buf);
        }
        pos += size;
    }
    None
}

/// 在 moov 内递归找 mvhd，换算 duration / timescale。
fn find_mvhd(buf: &[u8]) -> Option<f64> {
    let mut pos = 0usize;
    while pos + 8 <= buf.len() {
        let mut size = u32::from_be_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as usize;
        let typ = &buf[pos + 4..pos + 8];
        let mut header_len = 8usize;
        if size == 1 {
            if pos + 16 > buf.len() {
                return None;
            }
            size = u64::from_be_bytes([
                buf[pos + 8],
                buf[pos + 9],
                buf[pos + 10],
                buf[pos + 11],
                buf[pos + 12],
                buf[pos + 13],
                buf[pos + 14],
                buf[pos + 15],
            ]) as usize;
            header_len = 16;
        } else if size == 0 {
            size = buf.len() - pos;
        }
        if size < header_len || pos + size > buf.len() {
            return None;
        }
        if typ == b"mvhd" {
            let p = pos + header_len;
            let version = *buf.get(p)?;
            if version == 1 {
                // version(1) flags(3) creation(8) modification(8) timescale(4) duration(8) → 共 32 字节
                if p + 32 > buf.len() {
                    return None;
                }
                let timescale = u32::from_be_bytes([buf[p + 20], buf[p + 21], buf[p + 22], buf[p + 23]]) as f64;
                let duration = u64::from_be_bytes([
                    buf[p + 24],
                    buf[p + 25],
                    buf[p + 26],
                    buf[p + 27],
                    buf[p + 28],
                    buf[p + 29],
                    buf[p + 30],
                    buf[p + 31],
                ]) as f64;
                if timescale > 0.0 && duration > 0.0 {
                    return Some(duration / timescale);
                }
            } else {
                // creation(4) modification(4) timescale(4) duration(4)
                if p + 20 > buf.len() {
                    return None;
                }
                let timescale = u32::from_be_bytes([buf[p + 12], buf[p + 13], buf[p + 14], buf[p + 15]]) as f64;
                let duration = u32::from_be_bytes([buf[p + 16], buf[p + 17], buf[p + 18], buf[p + 19]]) as f64;
                if timescale > 0.0 && duration > 0.0 {
                    return Some(duration / timescale);
                }
            }
        }
        if typ == b"moov" || typ == b"trak" {
            if let Some(d) = find_mvhd(&buf[pos + header_len..pos + size]) {
                return Some(d);
            }
        }
        pos += size;
    }
    None
}

// ---------------- Matroska（mkv / webm） ----------------

/// Matroska 头部扫描：先读 64KB，找不到再扩到 1MB 重试一次。
///
/// **不要改回一次性分配 4MB**（2026-08-30 性能修复）：`scanner.rs` 对扫描到的**每一个**
/// 文件都调用本函数，4MB 缓冲意味着每个 mkv 都要分配 + 零填充 + 实读 4MB。
/// 实测 8.51 ms/文件 vs 64KB 的 0.03 ms——库内 2747 个 mkv 就是 23 秒纯浪费（冷盘可达数分钟）。
/// Segment/Info 元素实际就在文件开头几百字节，64KB 绰绰有余；扩读分支只是给
/// "头部塞了大段无关元素"的罕见文件兜底。
const MKV_HEAD_FIRST: usize = 64 * 1024;
const MKV_HEAD_MAX: usize = 1024 * 1024;

// 注意：下面是**解码后**的值，不是规范里的 ID 写法。
// EBML 的元素 ID 存成 VINT，首字节最高位的 marker 位属长度标记、不属于数据位，
// read_vint 会把它剥掉 —— 例如规范写 Segment = 0x18538067，文件里是 18 53 80 67，
// 解码后为 0x08538067。拿规范值直接比会永远匹配不上（曾导致全部 mkv 解析失败）。
const ID_SEGMENT: u64 = 0x0853_8067;
const ID_INFO: u64 = 0x0549_A966;
const ID_TIMESTAMP_SCALE: u64 = 0x0AD_7B1;
const ID_DURATION: u64 = 0x0489;

pub fn parse_mkv_duration_sec(path: &Path) -> Option<f64> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; MKV_HEAD_FIRST];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    if n < 16 {
        return None;
    }
    if let Some(sec) = mkv_duration_from_buffer(&buf, n) {
        return Some(sec);
    }
    // 首段没找到 → 扩读到上限再试一次。文件本身没那么长时扩读无意义，直接放弃。
    if n >= MKV_HEAD_MAX {
        return None;
    }
    let mut big = vec![0u8; MKV_HEAD_MAX];
    f.rewind().ok()?;
    let m = f.read(&mut big).ok()?;
    if m <= n {
        return None; // 文件比首段还小，扩读拿不到新内容
    }
    big.truncate(m);
    mkv_duration_from_buffer(&big, m)
}

/// 在已读入内存的头部缓冲里解析 Duration。拆成纯内存函数，便于"小缓冲先试、不够再扩读"两段式复用。
fn mkv_duration_from_buffer(buf: &[u8], n: usize) -> Option<f64> {
    // 顶层找 Segment（EBML 头之后）
    let (seg_start, seg_end) = ebml_find(buf, 0, n, ID_SEGMENT)?;
    // Segment 内找 Info
    let (info_start, info_end) = ebml_find(buf, seg_start, seg_end, ID_INFO)?;

    // TimestampScale：uint，单位纳秒，缺省 1_000_000（= 1ms）
    let scale_ns = ebml_find(buf, info_start, info_end, ID_TIMESTAMP_SCALE)
        .and_then(|(s, e)| ebml_uint(buf, s, e))
        .unwrap_or(1_000_000) as f64;

    // Duration：float，单位 = TimestampScale（纳秒）
    let (ds, de) = ebml_find(buf, info_start, info_end, ID_DURATION)?;
    let d = ebml_float(buf, ds, de)?;
    if d <= 0.0 || scale_ns <= 0.0 {
        return None;
    }
    let sec = d * scale_ns / 1_000_000_000.0;
    Some(sec)
}

/// EBML VINT：首字节前导 0 的个数决定总长度（1..8），其余为数据位。
/// 返回 (值, 字节数)。数据位全 1 = unknown size。
fn read_vint(data: &[u8], pos: usize) -> Option<(u64, usize)> {
    let b = *data.get(pos)?;
    if b == 0 {
        return None;
    }
    let len = b.leading_zeros() as usize + 1;
    if len > 8 || pos + len > data.len() {
        return None;
    }
    let mut v = (b as u64) & (0xFFu64 >> len);
    for i in 1..len {
        v = (v << 8) | data[pos + i] as u64;
    }
    Some((v, len))
}

/// 该 VINT 是否表示 unknown size（所有数据位为 1）
fn vint_is_unknown(value: u64, len: usize) -> bool {
    let bits = len * 7;
    if bits >= 64 {
        value == u64::MAX
    } else {
        value == (1u64 << bits) - 1
    }
}

/// 在 [start, end) 范围内按 EBML 元素顺序查找目标 ID，返回 (内容起始, 内容结束)。
fn ebml_find(data: &[u8], start: usize, end: usize, target: u64) -> Option<(usize, usize)> {
    let mut p = start;
    while p < end && p + 2 <= data.len() {
        let (id, id_len) = read_vint(data, p)?;
        let (size, size_len) = read_vint(data, p + id_len)?;
        let content_start = p + id_len + size_len;
        if content_start > end {
            return None;
        }
        let content_end = if vint_is_unknown(size, size_len) {
            end
        } else {
            (content_start as u64 + size).min(end as u64) as usize
        };
        if id == target {
            return Some((content_start, content_end));
        }
        if content_end <= content_start {
            return None; // 零长元素且非目标 → 结构异常，止损
        }
        p = content_end;
    }
    None
}

/// EBML uint：1..8 字节大端
fn ebml_uint(data: &[u8], start: usize, end: usize) -> Option<u64> {
    let len = end.saturating_sub(start);
    if len == 0 || len > 8 || end > data.len() {
        return None;
    }
    let mut v = 0u64;
    for b in &data[start..end] {
        v = (v << 8) | *b as u64;
    }
    Some(v)
}

/// EBML float：4 字节 f32 或 8 字节 f64（大端）
fn ebml_float(data: &[u8], start: usize, end: usize) -> Option<f64> {
    let len = end.saturating_sub(start);
    if end > data.len() {
        return None;
    }
    match len {
        4 => {
            let arr = [data[start], data[start + 1], data[start + 2], data[start + 3]];
            Some(f32::from_be_bytes(arr) as f64)
        }
        8 => {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&data[start..start + 8]);
            Some(f64::from_be_bytes(arr))
        }
        0 => {
            // 零长 float = 0
            Some(0.0)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vint_lengths() {
        // 0x80 → 1 字节，值 0
        assert_eq!(read_vint(&[0x80, 0x00], 0), Some((0, 1)));
        // 0x40 0x01 → 2 字节，值 1
        assert_eq!(read_vint(&[0x40, 0x01], 0), Some((1, 2)));
        // 0x20 0x00 0x10 → 3 字节，值 0x10
        assert_eq!(read_vint(&[0x20, 0x00, 0x10], 0), Some((16, 3)));
        // 全 0 非法
        assert_eq!(read_vint(&[0x00], 0), None);
    }

    /// 把 ID 常量和文件里的真实字节绑死：改错一个十六进制位立刻变红。
    /// 这些常量必须等于 read_vint 对原始字节的解码结果（marker 位已剥离）。
    #[test]
    fn ebml_ids_match_actual_bytes() {
        assert_eq!(read_vint(&[0x18, 0x53, 0x80, 0x67], 0), Some((ID_SEGMENT, 4)));
        assert_eq!(read_vint(&[0x15, 0x49, 0xA9, 0x66], 0), Some((ID_INFO, 4)));
        assert_eq!(read_vint(&[0x2A, 0xD7, 0xB1], 0), Some((ID_TIMESTAMP_SCALE, 3)));
        assert_eq!(read_vint(&[0x44, 0x89], 0), Some((ID_DURATION, 2)));
    }

    #[test]
    fn unknown_size_detection() {
        // 1 字节 VINT 数据位 7 位全 1 → 0x7F
        assert!(vint_is_unknown(0x7F, 1));
        assert!(!vint_is_unknown(0x7E, 1));
        // 8 字节 VINT：56 位全 1
        assert!(vint_is_unknown((1u64 << 56) - 1, 8));
    }

    #[test]
    fn uint_and_float_decoding() {
        assert_eq!(ebml_uint(&[0x0F, 0x42, 0x40], 0, 3), Some(1_000_000));
        let f = f64::from_be_bytes(600_000.0f64.to_be_bytes());
        let bytes = f.to_be_bytes();
        assert_eq!(ebml_float(&bytes, 0, 8), Some(600_000.0));
        let f32b = (600_000.0f32).to_be_bytes();
        assert!((ebml_float(&f32b, 0, 4).unwrap() - 600_000.0).abs() < 1.0);
    }

    #[test]
    fn seconds_parsing() {
        assert_eq!(parse_seconds("5925.125000\r\n"), Some(5925.125));
        assert_eq!(parse_seconds(""), None);
        assert_eq!(parse_seconds("N/A\n0.000000\n"), None);
        assert_eq!(parse_seconds("0.000000\n120.5\n"), Some(120.5));
    }

    /// 构造最小 mvhd（version 0，timescale 1000，duration 60000 → 60 秒）
    #[test]
    fn mvhd_v0_decoding() {
        // 完整 box：size(4) type(4) version/flags(4) creation(4) modification(4) timescale(4) duration(4)
        let mut box_body = Vec::new();
        box_body.extend_from_slice(&[0u8; 4]); // version + flags
        box_body.extend_from_slice(&0u32.to_be_bytes()); // creation
        box_body.extend_from_slice(&0u32.to_be_bytes()); // modification
        box_body.extend_from_slice(&1000u32.to_be_bytes()); // timescale
        box_body.extend_from_slice(&60_000u32.to_be_bytes()); // duration
        let total = (8 + box_body.len()) as u32;
        let mut full = Vec::new();
        full.extend_from_slice(&total.to_be_bytes());
        full.extend_from_slice(b"mvhd");
        full.extend_from_slice(&box_body);
        assert_eq!(find_mvhd(&full), Some(60.0));
    }

    /// version 1：timescale 90000，duration 5_400_000 → 60 秒
    #[test]
    fn mvhd_v1_decoding() {
        let mut box_body = Vec::new();
        box_body.push(1); // version
        box_body.extend_from_slice(&[0u8; 3]); // flags
        box_body.extend_from_slice(&0u64.to_be_bytes()); // creation
        box_body.extend_from_slice(&0u64.to_be_bytes()); // modification
        box_body.extend_from_slice(&90_000u32.to_be_bytes()); // timescale
        box_body.extend_from_slice(&5_400_000u64.to_be_bytes()); // duration
        let total = (8 + box_body.len()) as u32;
        let mut full = Vec::new();
        full.extend_from_slice(&total.to_be_bytes());
        full.extend_from_slice(b"mvhd");
        full.extend_from_slice(&box_body);
        assert_eq!(find_mvhd(&full), Some(60.0));
    }

    /// 真机验证（依赖本机文件，默认不跑）：`cargo test real_world -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_world_compare() {
        let samples: &[&str] = &[
            r"D:\已阅影音\剧集的盒子\中\Love.Death.and.Robots.S03.1080p.NF.WEBRip.DDP5.1.Atmos.x264-SMURF[rartv]\Love.Death.and.Robots.S03E01.Three.Robots.Exit.Strategies.1080p.NF.WEB-DL.DDP5.1.Atmos.x264-SMURF.mkv",
            r"D:\已阅影音\剧集的盒子\中\The Boys\The Boys 2\The.Boys.S02E04.中英字幕.WEBrip.AAC.1080p.x264-VINEnc.mp4",
            r"D:\已阅影音\剧集的盒子\[超人正义联盟.Justice.League]\[SGER.net][SGT字幕组][超人正义联盟.Justice League][第一季26集全][DVDRip-RMVB]\[SGER.net][SGT字幕组][超人正义联盟.Justice League][第一季第10集-Paradise.Lost.(Part.One)][DVDRip-RMVB].rmvb",
            r"D:\已阅影音\电影房\神作\腾扬赛评 终极一战 RNGvsKZ 2018年.flv",
            r"D:\已阅影音\剧集的盒子\南方公园\South.Park.S01.1080p.BluRay.x264-SHORTBREHD\South.Park.Pilot.The.Spirit.Of.Christmas.Jesus.VS.Santa.1995.DVDRip.XviD-SAiNTS.avi",
            r"D:\已阅影音\纪录片的洞穴\高\Two.Hundred.Years.Of.Surgery.2019.Complete.1080p.HDTV.H264.AAC-TJUPT\Two.Hundred.Years.Of.Surgery.2019.E01.1080p.HDTV.H264.AAC-TJUPT.ts",
        ];
        for p in samples {
            let path = Path::new(p);
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("?");
            if !path.exists() {
                println!("{:<5} SKIP (文件不存在)", ext);
                continue;
            }
            let own = parse_container_duration_sec(path);
            let ff = ffprobe_duration_sec(path);
            println!("{:<5} 自研={:?}  ffprobe={:?}", ext, own, ff);
        }
    }

    #[test]
    fn nonexistent_file_returns_none() {
        assert_eq!(parse_mp4_duration_sec(Path::new("Z:/nope/nope.mp4")), None);
        assert_eq!(parse_mkv_duration_sec(Path::new("Z:/nope/nope.mkv")), None);
    }
}
