use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use rusqlite::{params, OptionalExtension};
use tauri::{Emitter, Manager, State};

use crate::db::DbConn;
use crate::models::*;
use crate::scanner;

/// 批量补全时长的中断开关（在 main.rs 里 manage 成全局状态）
pub struct BackfillCancel(pub Arc<AtomicBool>);

impl Default for BackfillCancel {
    fn default() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }
}

/// 播放进度同步的「进行中」标志（main.rs manage 成全局状态）。
/// async 化后同步跑在运行时线程上，60s 轮询 / focus / visibilitychange 三路触发可能撞车，
/// 拆锁（探测 ffprobe 在锁外）后并发跑会重复探测、重复弹 toast——用原子标志串行化。
pub struct SyncInFlight(pub AtomicBool);

impl Default for SyncInFlight {
    fn default() -> Self {
        Self(AtomicBool::new(false))
    }
}

/// Drop 兜底：同步中途 panic 也不能把标志卡死在 true，否则同步永远罢工。
struct SyncInFlightGuard<'a>(&'a AtomicBool);

impl Drop for SyncInFlightGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn now_str() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 路径比较键：Windows 下 `/` 与 `\`、大小写都等价。
/// 数据库里仍保留用户原始路径，只在内存判定时归一化。
fn path_key(path: &str) -> String {
    path.replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// 扫描体积规则（可由用户在设置抽屉里改）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScanConfig {
    pub min_size_mb: i64,
    pub exempt_prefixes: Vec<String>,
}

fn normalize_prefixes(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in raw.split([',', '，', '\n', '\r']) {
        let p = part.trim();
        if !p.is_empty() && !out.iter().any(|x| x == p) {
            out.push(p.to_string());
        }
    }
    out
}

fn load_scan_rules(conn: &rusqlite::Connection) -> ScanConfig {
    let min_size_mb = crate::db::get_setting(conn, "scan_min_size_mb")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or((scanner::MIN_VIDEO_SIZE_BYTES / (1024 * 1024)) as i64)
        .clamp(0, 10240);
    let exempt_prefixes = match crate::db::get_setting(conn, "scan_size_exempt_prefixes")
        .ok()
        .flatten()
    {
        Some(raw) => normalize_prefixes(&raw),
        None => scanner::SIZE_EXEMPT_PREFIXES
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
    };
    ScanConfig { min_size_mb, exempt_prefixes }
}

fn scan_rules_bytes(rules: &ScanConfig) -> u64 {
    (rules.min_size_mb.max(0) as u64) * 1024 * 1024
}

/// 文件是否落在任一可访问扫描根目录内（按目录边界，防 H:\剧误配 H:\剧集）。
fn path_within_roots(file_path: &str, roots: &[String]) -> bool {
    let fp = path_key(file_path);
    let fp_bytes = fp.as_bytes();
    roots.iter().any(|root| {
        if root.is_empty() || fp.len() < root.len() || !fp_bytes.starts_with(root.as_bytes()) {
            return false;
        }
        fp.len() == root.len()
            || fp[root.len()..]
                .chars()
                .next()
                .map_or(false, |c| c == '\\' || c == '/')
    })
}

/// 分类结果 → 系列归属处理
/// 0. 路径锚点认领：文件落在某已注册剧集（series.folder_path）目录树内 → 直接归该剧集，
///    不再走分类器（避免「添加剧集」建的剧集被刷新扫描拆散 / 新集被分类成别名剧集）
/// 1. 分类器产出的剧集名 upsert（原名逻辑）
///
/// consensus_name：目录共识名（方案 A）。非 None 且分类器判定为 series 时优先采用，
/// 覆盖分类器逐文件推断出的发布名。仅在「同目录多文件共享足够多词」时才有值，
/// 是补上「分类器能判这堆文件是剧集、却判不出它们是同一个剧集」这一环。
/// 方案 C 的收紧门槛：某剧集至少这么多集、且全部落在同一目录，才自动补 folder_path 锚点。
/// 门槛不宜低：太低会把「碰巧同名」的零散记录锁成剧集；太高则新剧首次扫描收不到锚点。
pub const MIN_ANCHOR_EPISODES: usize = 5;

fn apply_series(
    tx: &rusqlite::Transaction,
    info: &scanner::VideoInfo,
    folder_anchors: &[(i64, String)],
    consensus_name: Option<&str>,
) -> Result<Option<i64>, AppError> {
    if let Some(sid) = match_folder_anchor(&info.file_path, folder_anchors) {
        return Ok(Some(sid));
    }
    if info.kind == "series" {
        // 方案 A：目录共识名优先于分类器的逐文件推断名
        let name_owned;
        let name = match consensus_name {
            Some(n) if !n.trim().is_empty() => {
                name_owned = n.trim().to_string();
                &name_owned
            }
            _ => match info.series_name.as_ref() {
                Some(n) => n,
                None => return Ok(None),
            },
        };
        let name = name.trim();
        if !name.is_empty() {
            let sid = crate::db::upsert_series(tx, name, None)?;
            return Ok(Some(sid));
        }
    }
    Ok(None)
}

/// 最长前缀匹配：file_path 是否落在某 series.folder_path 目录树内（大小写不敏感，按目录边界）
/// 前置条件：anchors 由 load_folder_anchors 加载，**已归一化**（lowercase + 去尾部斜杠）。
/// 闭包只在 file_path 上跑一次；锚点若在循环内重算归一化，就是 5000 文件 × 97 锚点次重复
/// 分配（实测 153ms → 31ms，4.9 倍差距全在这一行）。
fn match_folder_anchor(file_path: &str, anchors: &[(i64, String)]) -> Option<i64> {
    let norm = |p: &str| p.trim_end_matches(|c| c == '/' || c == '\\').to_lowercase();
    let fp = norm(file_path);
    // 用字节前缀比较，不要写 fp[..f.len()] == f：锚点路径含 2 字节 UTF-8 字符时，
    // 切片位置可能落在多字节字符中间，直接 panic。
    let fp_bytes = fp.as_bytes();
    let mut best: Option<(usize, i64)> = None;
    for (sid, folder) in anchors {
        let f = folder.as_str(); // 已归一化，直接用
        if f.is_empty() || fp.len() < f.len() || !fp_bytes.starts_with(f.as_bytes()) {
            continue;
        }
        // 走到这里说明前 f.len() 字节正好是 f（合法 UTF-8），f.len() 一定是 fp 的字符边界
        // 必须落在目录边界（folder 是文件本身所在目录的祖先），避免 H:\剧 误配 H:\剧集
        let boundary_ok = fp.len() == f.len()
            || fp[f.len()..]
                .chars()
                .next()
                .map_or(false, |c| c == '\\' || c == '/');
        if boundary_ok && best.as_ref().map_or(true, |(blen, _)| f.len() > *blen) {
            best = Some((f.len(), *sid));
        }
    }
    best.map(|(_, sid)| sid)
}

/// 读取所有已注册剧集目录锚点（folder_path 非空）。
/// 返回的 String 已归一化（lowercase + 去尾部斜杠），专供 match_folder_anchor 使用，
/// 归一化只做这一次，别在匹配循环里重算。
fn load_folder_anchors(conn: &rusqlite::Connection) -> Result<Vec<(i64, String)>, AppError> {
    let norm = |p: &str| p.trim_end_matches(|c| c == '/' || c == '\\').to_lowercase();
    let mut stmt = conn.prepare(
        "SELECT id, folder_path FROM series WHERE folder_path IS NOT NULL AND folder_path != ''",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    let mut v = Vec::new();
    for r in rows {
        if let Ok((sid, folder)) = r {
            let f = norm(&folder);
            if !f.is_empty() {
                v.push((sid, f));
            }
        }
    }
    Ok(v)
}

/// 某目录下的视频归属几部剧集：1 = 独占（可安全写锚点），>1 = 共享（写锚点必串记录）。
///
/// 目录归一化与 `match_folder_anchor` 同源：只取到最后一个反斜杠为止，并保留尾部反斜杠。
/// SQLite 的 `rtrim(X, Y)` 会删掉 X 尾部所有「出现在 Y 里的字符」，把 Y 取成
/// 路径去掉全部反斜杠后的串，等价于「截到最后一个反斜杠」——纯表达式，无需应用侧再扫一遍全表。
fn dir_series_owners(conn: &rusqlite::Connection, folder: &str) -> Result<i64, AppError> {
    let dir_key = format!("{}\\", folder.trim_end_matches('\\'));
    let owners: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT v.series_id) FROM videos v
         WHERE v.series_id IS NOT NULL
           AND rtrim(v.file_path, replace(v.file_path, '\\', '')) = ?1 COLLATE NOCASE",
        rusqlite::params![dir_key],
        |r| r.get(0),
    )?;
    Ok(owners)
}

// ============================================
// 扫描
// ============================================
// Tauri 2 中非 async 命令跑在主线程——扫描类命令是文件系统遍历（可达秒/分钟级），
// 必须声明为 async 才会丢到运行时线程，否则用户点一次"刷新"整个界面冻死。
#[tauri::command]
pub async fn scan_folder(
    db: State<'_, DbConn>,
    dir_path: String,
    incremental: bool,
    default_watched: bool,
) -> Result<ScanResult, String> {
    // 先取扫描规则（短锁），再扫描（不持 DB 锁，避免阻塞其他查询）
    let rules = {
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        load_scan_rules(&conn)
    };
    let (all_files, scan_errors) = scanner::scan_directory_reporting_with(
        &dir_path,
        scan_rules_bytes(&rules),
        &rules.exempt_prefixes,
    );
    let sibling_map = scanner::build_sibling_map_public(&all_files);
    let total_found = all_files.len() as i64;
    // 方案 A：目录共识命名。扫完就把「同目录该叫什么」算好，
    // 全程只跑一次（内部按目录分组 + 词频统计），不随文件数二次方增长。
    let consensus_names = crate::consensus::directory_series_names(&all_files);

    if total_found == 0 {
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        let _ = crate::db::upsert_scan_folder(&conn, &dir_path);
        return Ok(ScanResult {
            total_found: 0,
            new_added: 0,
            updated: 0,
            errors: scan_errors,
            pending: Vec::new(),
        });
    }

    // 数据库锁只覆盖两次短读写：读已有路径/锚点，和最后的批量写入。
    // 文件元数据与容器头解析全部在锁外完成——网络盘上读文件头比 SQL 慢几个数量级。
    let (existing, folder_anchors) = {
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        // HashSet 加速已存在路径判定（避免全库线性查找）。
        // 统一小写：Windows 路径大小写不敏感，否则同文件换个盘符大小写就会重复入库。
        let existing: std::collections::HashSet<String> = crate::db::get_all_file_paths(&conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|p| path_key(&p))
            .collect();
        let anchors = load_folder_anchors(&conn)?;
        (existing, anchors)
    };

    let mut prepared: Vec<(scanner::VideoInfo, Option<String>)> = Vec::new();
    for file in &all_files {
        let path_str = file.to_string_lossy().to_string();
        if incremental && existing.contains(&path_key(&path_str)) {
            continue;
        }
        let info = scanner::extract_video_info(file, &sibling_map);
        let cname = file
            .parent()
            .and_then(|d| consensus_names.get(d))
            .map(|s| s.to_string());
        prepared.push((info, cname));
    }

    if prepared.is_empty() {
        if let Ok(conn) = db.0.lock() {
            let _ = crate::db::upsert_scan_folder(&conn, &dir_path);
        }
        return Ok(ScanResult {
            total_found,
            new_added: 0,
            updated: 0,
            errors: scan_errors,
            pending: Vec::new(),
        });
    }

    let mut new_added: i64 = 0;
    let mut updated: i64 = 0;
    let mut errors = scan_errors;
    let mut pending: Vec<PendingItem> = Vec::new();

    {
        let mut conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        let tx = conn
            .transaction()
            .map_err(|e| AppError::db(e.to_string()))?;

        for (info, cname) in &prepared {
            let series_id = apply_series(&tx, info, &folder_anchors, cname.as_deref())?;
            match crate::db::upsert_video(
                &tx,
                &VideoUpsertParams {
                    title: &info.title,
                    file_path: &info.file_path,
                    file_name: &info.file_name,
                    file_size: info.file_size,
                    duration: info.duration,
                    kind: &info.kind,
                    confidence: info.confidence,
                    user_kind: None,
                    series_id,
                    season: info.season,
                    episode: info.episode,
                    episode_end: info.episode_end,
                    default_watched,
                },
                true,
            ) {
                Ok(r) => {
                    if r.is_inserted() {
                        new_added += 1;
                        if let Some(sid) = series_id {
                            crate::db::touch_series(&tx, sid)?;
                        }
                    } else {
                        updated += 1;
                    }
                    if info.kind == "unknown" {
                        pending.push(PendingItem {
                            video_id: r.id(),
                            title: info.title.clone(),
                            file_path: info.file_path.clone(),
                            confidence: info.confidence,
                            suggested: "剧集?".into(),
                        });
                    }
                }
                Err(e) => errors.push(format!("{}: {}", info.file_path, e)),
            }
        }
        tx.commit().map_err(|e| AppError::db(e.to_string()))?;
    }

    if let Ok(conn) = db.0.lock() {
        let _ = crate::db::upsert_scan_folder(&conn, &dir_path);
    }

    Ok(ScanResult {
        total_found,
        new_added,
        updated,
        errors,
        pending,
    })
}

/// 添加剧集：选文件夹 → 文件夹名即剧集名，文件夹内**所有**视频强制归集到该剧集
/// 与 scan_folder 的区别：
/// - 不走自动分类 / 待确认队列（kind 直接锁定 series）
/// - 同名剧集并入，folder_path 记录完整路径
/// - 不注册进 scan_folders（避免日后刷新被分类器重新判定拆散）
#[tauri::command]
pub async fn add_series_from_folder(
    db: State<'_, DbConn>,
    dir_path: String,
    default_watched: bool,
) -> Result<ScanResult, String> {
    let dir = Path::new(&dir_path);
    if !dir.is_dir() {
        return Err(format!("不是有效的文件夹: {}", dir_path));
    }
    let Some(series_name) = dir.file_name().and_then(|n| n.to_str()) else {
        return Err("无法从文件夹路径获取剧集名".to_string());
    };
    if series_name.trim().is_empty() {
        return Err("文件夹名不能为空".to_string());
    }

    // 先取扫描规则（短锁），再扫描（不持 DB 锁）
    let rules = {
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        load_scan_rules(&conn)
    };
    let (all_files, scan_errors) = scanner::scan_directory_reporting_with(
        &dir_path,
        scan_rules_bytes(&rules),
        &rules.exempt_prefixes,
    );
    let total_found = all_files.len() as i64;
    if total_found == 0 {
        return Ok(ScanResult {
            total_found: 0,
            new_added: 0,
            updated: 0,
            errors: scan_errors,
            pending: Vec::new(),
        });
    }

    let sibling_map = scanner::build_sibling_map_public(&all_files);
    // 文件解析放在 DB 锁外，避免网络盘/机械盘读头阻塞其他查询。
    let prepared: Vec<scanner::VideoInfo> = all_files
        .iter()
        .map(|file| scanner::extract_video_info(file, &sibling_map))
        .collect();
    let mut new_added: i64 = 0;
    let mut updated: i64 = 0;
    let mut errors = scan_errors;

    {
        let mut conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        let tx = conn.transaction().map_err(|e| AppError::db(e.to_string()))?;
        // 文件夹名 = 剧集名（同名并入），folder_path 记录完整路径
        let sid = crate::db::upsert_series(&tx, series_name, Some(&dir_path)).map_err(|e| e.to_string())?;

        for info in &prepared {
            // 强制归集：kind=series + user_kind=series（锁定防重判）+ series_id=本剧集
            match crate::db::upsert_video(
                &tx,
                &VideoUpsertParams {
                    title: &info.title,
                    file_path: &info.file_path,
                    file_name: &info.file_name,
                    file_size: info.file_size,
                    duration: info.duration,
                    kind: "series",
                    confidence: 100,
                    user_kind: Some("series"),
                    series_id: Some(sid),
                    season: info.season,
                    episode: info.episode,
                    episode_end: info.episode_end,
                    default_watched,
                },
                true,
            ) {
                Ok(r) => {
                    let id = r.id();
                    if r.is_inserted() {
                        new_added += 1;
                    } else {
                        updated += 1;
                        // 移动匹配分支可能保留旧 series_id / 旧 user_kind，改挂本剧集（新插入无需兜底）
                        tx.execute(
                            "UPDATE videos SET series_id = ?1, kind = 'series', user_kind = 'series', updated_at = datetime('now','localtime') WHERE id = ?2",
                            rusqlite::params![sid, id],
                        )
                        .map_err(|e| AppError::db(e.to_string()))?;
                    }
                }
                Err(e) => errors.push(format!("{}: {}", info.file_path, e)),
            }
        }
        if new_added > 0 {
            crate::db::touch_series(&tx, sid)?;
        }
        tx.commit().map_err(|e| AppError::db(e.to_string()))?;
    }

    Ok(ScanResult {
        total_found,
        new_added,
        updated,
        errors,
        pending: Vec::new(),
    })
}

#[tauri::command]
pub async fn refresh_all_scans(
    app: tauri::AppHandle,
    db: State<'_, DbConn>,
    default_watched: bool,
) -> Result<RefreshResult, String> {
    // 短暂持锁读取订阅列表与现有路径，随即释放（扫描期间不占锁）
    let (scan_folders, existing, rules) = {
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        let _ = crate::db::set_setting(&conn, "last_refresh_at", &now_str());
        let folders = crate::db::get_scan_folders(&conn).map_err(|e| e.to_string())?;
        let paths: std::collections::HashSet<String> = crate::db::get_all_file_paths(&conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|p| path_key(&p))
            .collect();
        let rules = load_scan_rules(&conn);
        (folders, paths, rules)
    };

    // 第一阶段：扫描所有订阅文件夹（不持锁）。
    // accessible_roots 是本次真正能访问的根目录；清理阶段只允许在这些根内删记录。
    // 外接硬盘没插 / 网络盘掉线时，整个根不会被列入，已有评分与历史原样保留。
    let mut folder_jobs = Vec::new();
    let mut accessible_roots: Vec<String> = Vec::new();
    let mut queued_paths = existing.clone();
    let mut errors = Vec::new();
    for folder in &scan_folders {
        if !Path::new(&folder.folder_path).is_dir() {
            errors.push(format!(
                "[{}] 目录当前不可访问，已跳过扫描与清理",
                folder.folder_path
            ));
            continue;
        }
        accessible_roots.push(path_key(&folder.folder_path));
        let (all_files, scan_errors) = scanner::scan_directory_reporting_with(
            &folder.folder_path,
            scan_rules_bytes(&rules),
            &rules.exempt_prefixes,
        );
        for e in scan_errors {
            errors.push(format!("[{}] {}", folder.folder_path, e));
        }
        let sibling_map = scanner::build_sibling_map_public(&all_files);
        // 方案 A：目录共识命名（每个订阅文件夹各算一次，扫完即算，不随文件数二次方增长）
        let consensus_names = crate::consensus::directory_series_names(&all_files);
        let mut prepared: Vec<(scanner::VideoInfo, Option<String>)> = Vec::new();
        for file in &all_files {
            let path_str = file.to_string_lossy().to_string();
            // queued_paths 同时挡住“已入库”和“订阅根目录互相重叠”两种情况，
            // 避免同一文件被两个根重复 upsert、moved 计数虚高。
            if !queued_paths.insert(path_key(&path_str)) {
                continue;
            }
            let info = scanner::extract_video_info(file, &sibling_map);
            let cname = file
                .parent()
                .and_then(|d| consensus_names.get(d))
                .map(|s| s.to_string());
            prepared.push((info, cname));
        }
        folder_jobs.push((folder.folder_path.clone(), prepared));
    }
    let folders_scanned = folder_jobs.len() as i64;

    // 写库（重新持锁，锁范围只覆盖 DB 操作）
    let mut conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    // 已注册剧集目录锚点（路径认领用）
    let folder_anchors = load_folder_anchors(&conn)?;
    let mut new_added: i64 = 0;
    let mut moved: i64 = 0;
    let mut pending: Vec<PendingItem> = Vec::new();
    for (folder_path, prepared) in &folder_jobs {
        {
            let tx = conn
                .transaction()
                .map_err(|e| AppError::db(e.to_string()))?;
            for (info, cname) in prepared {
                let series_id = apply_series(&tx, info, &folder_anchors, cname.as_deref())?;
                match crate::db::upsert_video(
                    &tx,
                    &VideoUpsertParams {
                        title: &info.title,
                        file_path: &info.file_path,
                        file_name: &info.file_name,
                        file_size: info.file_size,
                        duration: info.duration,
                        kind: &info.kind,
                        confidence: info.confidence,
                        user_kind: None,
                        series_id,
                        season: info.season,
                        episode: info.episode,
                        episode_end: info.episode_end,
                        default_watched,
                    },
                    true,
                ) {
                    Ok(r) => {
                        if r.is_inserted() {
                            new_added += 1;
                            if let Some(sid) = series_id {
                                crate::db::touch_series(&tx, sid)?;
                            }
                        } else {
                            moved += 1;
                        }
                        if info.kind == "unknown" {
                            pending.push(PendingItem {
                                video_id: r.id(),
                                title: info.title.clone(),
                                file_path: info.file_path.clone(),
                                confidence: info.confidence,
                                suggested: "剧集?".into(),
                            });
                        }
                    }
                    Err(e) => errors.push(format!("{}: {}", info.file_path, e)),
                }
            }
            tx.commit().map_err(|e| AppError::db(e.to_string()))?;
        }
        if let Err(e) = crate::db::upsert_scan_folder(&conn, folder_path) {
            eprintln!("[refresh_all_scans] {}", e);
        }
    }

    // 第二阶段半（方案 C）：自动收紧目录锚点。
    // 解决「先下载、后刷新扫描」的死结——「添加剧集」建的锚点能保后续，但保不了第一次：
    // 用户先下载、第一次扫描时锚点还不存在，于是那批文件只能靠分类器起名。
    // 这里在扫描写库后做一次收口：某剧集下 ≥ MIN_ANCHOR_EPISODES 集且全部落在
    // 同一个目录树内 → 把该目录写进 series.folder_path，之后新集自带锚点、不再被拆散。
    //
    // 保守起见（守住「宁可拆散，不可串记录」）：
    //   - 只处理 folder_path 为空的剧集（已有锚点的不动，避免覆盖用户手动设置）
    //   - 所有集必须在同一目录（跨目录一律放弃，可能是多碟/多来源）
    //   - 集数达标才收紧（防止把「碰巧同名」的零散记录锁成剧集）
    let mut anchors_tightened: i64 = 0;
    {
        let candidates: Vec<(i64, String, i64)> = {
            let mut out = Vec::new();
            let mut stmt = conn
                .prepare(
                    "SELECT se.id, se.name, COUNT(*) AS n,
                            COUNT(DISTINCT rtrim(v.file_path, replace(v.file_path, '\\', ''))) AS dirs
                     FROM series se
                     JOIN videos v ON v.series_id = se.id
                     WHERE se.folder_path IS NULL OR se.folder_path = ''
                     GROUP BY se.id, se.name
                     HAVING n >= ?1 AND dirs = 1",
                )
                .map_err(|e| AppError::db(e.to_string()))?;
            let rows = stmt
                .query_map(rusqlite::params![MIN_ANCHOR_EPISODES as i64], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
                })
                .map_err(|e| AppError::db(e.to_string()))?;
            for r in rows {
                if let Ok(v) = r {
                    out.push(v);
                }
            }
            out
        };

        if !candidates.is_empty() {
            let tx = conn.transaction().map_err(|e| AppError::db(e.to_string()))?;
            for (sid, _name, _n) in &candidates {
                // 取该剧集所有集文件所在目录（唯一），写进 folder_path
                let dir: Option<String> = tx
                    .query_row(
                        "SELECT v.file_path FROM videos v WHERE v.series_id = ?1 LIMIT 1",
                        rusqlite::params![sid],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(|e| AppError::db(e.to_string()))?;
                let Some(fp) = dir else { continue };
                let folder = match fp.rfind('\\') {
                    Some(i) if i > 0 => fp[..i].to_string(),
                    _ => continue,
                };
                if folder.is_empty() {
                    continue;
                }
                // 目录独占闸（2026-11-08 补）：同一目录若还挂着别的剧集，把目录写成锚点
                // 会让两部剧共用同一个前缀，下次刷新时 match_folder_anchor 只能命中其中
                // 一部（同长度先到先得，见该函数 `f.len() > *blen` 的严格比较），另一部的
                // 集就被整体改判过去 —— 正是「宁可拆散，不可串记录」要防的事。
                // 宁可放弃这次收紧（保持无锚点、继续靠分类器命名），也不能合并两剧。
                let owners = dir_series_owners(&tx, &folder)?;
                if owners > 1 {
                    errors.push(format!(
                        "已跳过锚点收紧（剧集 {}，{} 集）：目录 {} 同时归属 {} 部剧集",
                        sid, _n, folder, owners
                    ));
                    continue;
                }
                if let Err(e) = tx.execute(
                    "UPDATE series SET folder_path = ?1, updated_at = datetime('now','localtime') WHERE id = ?2",
                    rusqlite::params![folder, sid],
                ) {
                    errors.push(format!("收紧锚点失败 (series {}): {}", sid, e));
                } else {
                    anchors_tightened += 1;
                }
            }
            tx.commit().map_err(|e| AppError::db(e.to_string()))?;
        }
    }

    // 第二阶段：只清理**本次可访问扫描根目录内**的失效记录。
    // 关键安全边界：不在任何已订阅目录里的记录（手工添加的剧集、临时导入的备份）
    // 永不在这里被删除；扫描根离线时整根跳过，绝不因为盘没插就把评分/历史清空。
    // 体积判断用当前磁盘上的真实 metadata，而不是数据库里的旧 file_size，
    // 这样“文件被替换成小体积版本”才真的能被发现。
    //
    // 先释放上一阶段持有的 DB 锁，再对全库做 metadata 探测；网络盘上几千次 stat
    // 可能耗时数秒，绝不能把这串 IO 塞在数据库锁里。
    drop(conn);
    let videos: Vec<(i64, String, String)> = if accessible_roots.is_empty() {
        Vec::new()
    } else {
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        let mut stmt = conn
            .prepare("SELECT id, file_path, file_name FROM videos")
            .map_err(|e| AppError::db(e.to_string()))?;
        let mut out = Vec::new();
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|e| AppError::db(e.to_string()))?;
        for r in rows {
            if let Ok(v) = r {
                out.push(v);
            }
        }
        out
    };

    let mut to_remove: Vec<(i64, String)> = Vec::new();
    for (id, file_path, file_name) in &videos {
        if !path_within_roots(file_path, &accessible_roots) {
            continue;
        }
        let should_remove = match std::fs::metadata(file_path) {
            Ok(meta) => {
                !meta.is_file()
                    || (meta.len() < scan_rules_bytes(&rules)
                        && !scanner::is_size_exempt_with(file_name, &rules.exempt_prefixes))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            Err(_) => false, // 权限/IO 临时异常：宁可保留记录，也不误删
        };
        if should_remove {
            to_remove.push((*id, file_path.clone()));
        }
    }

    let mut removed: i64 = 0;
    let mut removed_ids: Vec<i64> = Vec::new();
    let mut backup_path: Option<String> = None;
    if !to_remove.is_empty() {
        let mut conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        // 删除前整库快照：刷新扫描的“失效”判断依赖磁盘可见性，
        // 万一判断错（外接盘短暂掉线、权限异常），用户还能从这份快照找回评分/历史。
        match app.path().app_data_dir() {
            Ok(dir) => {
                match crate::db::backup_database(&conn, &dir, "pre-refresh") {
                    Ok(path) => backup_path = Some(path),
                    Err(e) => {
                        errors.push(format!(
                            "清理前备份失败，已跳过本轮删除（共 {} 条待清理）: {}",
                            to_remove.len(),
                            e
                        ));
                        to_remove.clear();
                    }
                }
            }
            Err(e) => {
                errors.push(format!("读取备份目录失败，已跳过本轮删除: {e}"));
                to_remove.clear();
            }
        }
        let tx = conn
            .transaction()
            .map_err(|e| AppError::db(e.to_string()))?;
        for (id, file_path) in &to_remove {
            match tx.execute("DELETE FROM videos WHERE id = ?1", rusqlite::params![id]) {
                Ok(_) => {
                    removed += 1;
                    removed_ids.push(*id);
                }
                Err(e) => errors.push(format!("删除记录失败 ({}): {}", file_path, e)),
            }
        }
        let _ = crate::db::delete_orphan_series(&tx);
        tx.commit().map_err(|e| AppError::db(e.to_string()))?;
    }
    // 缩略图缓存按 video_id 命名，记录删了图就成孤儿。放在锁外做文件 IO，
    // 与 delete_video / delete_series 走同一条清理路径（否则刷新删档会一直漏）。
    if !removed_ids.is_empty() {
        crate::thumbnail::remove_cached_thumbnails(&app, &removed_ids);
    }
    // 兜底清理历史遗留空壳（即使本轮删除了 0 条，也顺手收口）
    if let Ok(conn) = db.0.lock() {
        let _ = crate::db::delete_orphan_series(&conn);
    }

    Ok(RefreshResult {
        removed,
        new_added,
        moved,
        folders_scanned,
        anchors_tightened,
        backup_path,
        errors,
        pending,
    })
}

// ============================================
// 查询
// ============================================
#[tauri::command]
pub fn get_videos(
    db: State<'_, DbConn>,
    page: Option<i64>,
    page_size: Option<i64>,
    query: Option<String>,
    kind: Option<String>,
    tag_ids: Option<Vec<i64>>,
    min_rating: Option<f64>,
    max_rating: Option<f64>,
    sort: Option<String>,
    order: Option<String>,
    watch_statuses: Option<Vec<String>>,
) -> Result<PaginatedVideos, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    // 本地 IPC 也可能被异常调用；负 LIMIT 在 SQLite 里等于“不限量”，必须兜住。
    let page = page.unwrap_or(0).max(0);
    let page_size = page_size.unwrap_or(60).clamp(1, 500);
    let min_rating = min_rating.unwrap_or(0.0);
    let max_rating = max_rating.unwrap_or(10.0);
    let sort = sort.as_deref().unwrap_or("last_watched");
    let order = order.as_deref().unwrap_or("desc");
    let kind = kind.filter(|k| !k.is_empty() && k != "all");

    // 统一拼装查询条件（含观看状态过滤）
    let mut conditions: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    let q = query.unwrap_or_default();
    if !q.trim().is_empty() {
        // 多关键词：按空白拆分，每个词都需命中（AND）
        // 搜索范围：标题 / 文件名 / 完整路径 / 剧集名 / 标签名
        let terms: Vec<String> = q
            .split_whitespace()
            .map(|t| format!("%{}%", crate::db::escape_like(t)))
            .collect();
        if !terms.is_empty() {
            let mut term_conds = Vec::new();
            for t in &terms {
                let idx = params.len() + 1;
                term_conds.push(format!(
                    "(v.title LIKE ?{0} ESCAPE '\\' OR v.file_name LIKE ?{0} ESCAPE '\\' \
                     OR v.file_path LIKE ?{0} ESCAPE '\\' OR s.name LIKE ?{0} ESCAPE '\\' \
                     OR EXISTS (SELECT 1 FROM video_tags vt_ JOIN tags t_ ON t_.id = vt_.tag_id \
                                WHERE vt_.video_id = v.id AND t_.name LIKE ?{0} ESCAPE '\\'))",
                    idx
                ));
                params.push(Box::new(t.clone()));
            }
            conditions.push(format!("({})", term_conds.join(" AND ")));
        }
    }
    if let Some(k) = kind {
        let idx = params.len() + 1;
        conditions.push(format!("v.kind = ?{}", idx));
        params.push(Box::new(k));
    }
    for tag_id in tag_ids.unwrap_or_default() {
        let idx = params.len() + 1;
        conditions.push(format!(
            "EXISTS (SELECT 1 FROM video_tags vt_{0} WHERE vt_{0}.video_id = v.id AND vt_{0}.tag_id = ?{0})",
            idx
        ));
        params.push(Box::new(tag_id));
    }
    // 观看状态多选：勾选集合即过滤集合（OR 连接）。
    // None / 未传 = 不过滤（全选时前端不发该字段）；空集合 = 恒假（前端"全灭"态给空结果）。
    // 状态值白名单化，字面量进 SQL，不引入绑定参数，不影响既有 ?N 编号。
    if let Some(list) = watch_statuses {
        let mut ors: Vec<&str> = Vec::new();
        for s in &list {
            match s.as_str() {
                "unwatched" => ors.push("v.watch_progress = 0"),
                // 与剧集统计/前端 watched() 同口径：99.9 起算“已看”，
                // 避免 PotPlayer 算出 99.98% 时卡片显示已看、筛选却把它归到“在看”。
                "partial" => ors.push("(v.watch_progress > 0 AND v.watch_progress < 99.9)"),
                "watched" => ors.push("v.watch_progress >= 99.9"),
                _ => {} // 未知值忽略
            }
        }
        if ors.is_empty() {
            conditions.push("1=0".to_string());
        } else {
            conditions.push(format!("({})", ors.join(" OR ")));
        }
    }

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    crate::db::query_videos_paginated(
        &conn, &conditions, &param_refs, page, page_size, min_rating, max_rating, sort, order,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_recently_added(
    db: State<'_, DbConn>,
    page: Option<i64>,
    page_size: Option<i64>,
    min_rating: Option<f64>,
) -> Result<PaginatedVideos, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_recently_added_videos(
        &conn,
        page.unwrap_or(0).max(0),
        page_size.unwrap_or(60).clamp(1, 500),
        min_rating.unwrap_or(0.0),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_series_overview(db: State<'_, DbConn>) -> Result<Vec<SeriesOverview>, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_series_overview(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_series_episodes(db: State<'_, DbConn>, series_id: i64) -> Result<Vec<VideoWithTags>, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_series_episodes(&conn, series_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_video_detail(db: State<'_, DbConn>, video_id: i64) -> Result<VideoWithTags, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_video_by_id(&conn, video_id).map_err(|e| e.to_string())
}

/// 播放历史（界面用）：按视频去重，每个视频只出现一次，带完整视频数据
#[tauri::command]
pub fn get_history(db: State<'_, DbConn>, limit: Option<i64>) -> Result<Vec<HistoryEntry>, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_history_entries(&conn, limit.unwrap_or(60).clamp(1, 500))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_stats(db: State<'_, DbConn>) -> Result<VideoStats, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_stats(&conn).map_err(|e| e.to_string())
}

// ============================================
// 标签
// ============================================
#[tauri::command]
pub fn get_tags(db: State<'_, DbConn>) -> Result<Vec<Tag>, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_all_tags(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_tag(db: State<'_, DbConn>, name: String, color: String) -> Result<Tag, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::create_tag(&conn, &name, &color).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_tag(db: State<'_, DbConn>, tag_id: i64, name: Option<String>, color: Option<String>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::update_tag(&conn, tag_id, name.as_deref(), color.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_tag(db: State<'_, DbConn>, tag_id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::delete_tag(&conn, tag_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_video_tag(db: State<'_, DbConn>, video_id: i64, tag_id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::add_tag_to_video(&conn, video_id, tag_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_video_tag(db: State<'_, DbConn>, video_id: i64, tag_id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::remove_tag_from_video(&conn, video_id, tag_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_series_tag(db: State<'_, DbConn>, series_id: i64, tag_id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::add_series_tag(&conn, series_id, tag_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_series_tag(db: State<'_, DbConn>, series_id: i64, tag_id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::remove_series_tag(&conn, series_id, tag_id).map_err(|e| e.to_string())
}

// ============================================
// 操作
// ============================================
#[tauri::command]
pub fn set_rating(db: State<'_, DbConn>, video_id: i64, rating: f64) -> Result<(), String> {
    if !rating.is_finite() || !(0.0..=10.0).contains(&rating) {
        return Err("评分必须在 0-10 之间".to_string());
    }
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::update_rating(&conn, video_id, rating).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_series_rating(db: State<'_, DbConn>, series_id: i64, rating: f64) -> Result<(), String> {
    if !rating.is_finite() || !(0.0..=10.0).contains(&rating) {
        return Err("评分必须在 0-10 之间".to_string());
    }
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    conn.execute(
        "UPDATE series SET rating = ?1, updated_at = datetime('now','localtime') WHERE id = ?2",
        rusqlite::params![rating, series_id],
    )
    .map_err(|e| AppError::db(e.to_string()))?;
    Ok(())
}

/// 全剧标记已看/未看
#[tauri::command]
pub fn mark_series_watched(db: State<'_, DbConn>, series_id: i64, watched: bool) -> Result<i64, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::mark_series_watched(&conn, series_id, watched).map_err(|e| e.to_string())
}

/// 一次业务写操作 = 一个事务。
/// 背景：update_watch_progress / mark_played 内部都是 4 次独立写入
/// （UPDATE videos + record_watch + trim_history + sync_series_aggregate），
/// 逐条自动提交时每次都要等一次 fsync。配合 synchronous=NORMAL 再把它们包进同一个事务，
/// 实测从 ~126 ms/次 降到 ~0.4 ms/次。
#[tauri::command]
pub fn update_progress(db: State<'_, DbConn>, video_id: i64, progress: f64) -> Result<(), String> {
    if !progress.is_finite() {
        return Err("观看进度必须是有限数值".to_string());
    }
    let progress = progress.clamp(0.0, 100.0);
    let mut conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    let tx = conn.transaction().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::update_watch_progress(&tx, video_id, progress).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| AppError::db(e.to_string()))?;
    Ok(())
}

/// 点击播放即标记"最近观看"：刷新 last_watched_at + 写一条观看历史（不动进度百分比）。
/// 前端 play() 打开播放器前调用，这是最可靠的"看过"记录时机。
#[tauri::command]
pub fn mark_played(db: State<'_, DbConn>, video_id: i64) -> Result<(), String> {
    let mut conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    let tx = conn.transaction().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::mark_played(&tx, video_id).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| AppError::db(e.to_string()))?;
    Ok(())
}

// ============================================
// 文件操作
// ============================================

/// 用系统 Shell 打开路径（等价于在资源管理器里双击）。
///
/// **历史**（四代演进，2026-09-02 起 v4）：
/// - v1：`open::that()`，Windows 上经 PowerShell 派发。冷启动 ~130ms，多一个进程，路径经环境变量传递。
/// - v2：手写 ShellExecuteExW + 专用线程 + STA CoInit + 显式 `lpDirectory`。
///   曾偶发 PotPlayer 在 `nvwgf2umx.dll` 0xC0000005（NVIDIA D3D 驱动）崩溃而弃用；
///   2026-09-02 A/B 实测：ShellExecuteExW 本身与双击耗时一致（见下），当年崩溃应属
///   NVIDIA 驱动偶发（4K HEVC + 驱动层的已知崩溃形态），并非本调用方式所致。
/// - v3：`cmd /c start "" "<path>"` + `current_dir` + 路径走环境变量。规避了解析坑，
///   但仍多一个 cmd 进程、给 PotPlayer 注入 CINEVAULT_OPEN_TARGET 环境变量、
///   覆写其 CWD——与双击始终存在三处结构差异，且 env/引号/CWD 三代里各踩过一次坑。
/// - **v4（当前）**：spawn_blocking 线程上直接 `ShellExecuteExW`（MTA + SEE_MASK_NOASYNC）。
///   与资源管理器双击**字面上同一个 API、同一条代码路径**：零中间进程、无命令行/环境
///   变量解析环节、`lpDirectory`/`lpVerb` 均与双击同参（默认动词 + NULL 目录）。
///   2026-09-02 实测（explorer 派发 vs v3 链路 vs ShellExecuteExW 三方对照，真实 2160p 文件，
///   含 PotPlayer 已运行换集场景）：三者"拉起到开播"耗时一致（1.3~1.8s，本机热态），
///   窗口零假死（IsHungAppWindow 采样）；并实测 PotPlayer 的 CWD 与其所在目录大小
///   （4~4000 文件）对启动耗时无影响——历代"CWD 枚举拖慢启动"的诊断不成立。
#[cfg(target_os = "windows")]
fn shell_open(file: &Path, _directory: Option<&Path>) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    // SEE_MASK_NOASYNC：不等 DDE 会话、不在调用线程上排队给 Shell 代理——
    // 从普通进程发起时语义与双击一致且不会吊住调用线程。
    const SEE_MASK_NOASYNC: u32 = 0x0000_0100;
    const SW_SHOWNORMAL: i32 = 1;
    const COINIT_MULTITHREADED: u32 = 0x0;

    #[repr(C)]
    struct ShellExecuteInfoW {
        cb_size: u32,
        f_mask: u32,
        hwnd: isize,
        lp_verb: *const u16,
        lp_file: *const u16,
        lp_parameters: *const u16,
        lp_directory: *const u16,
        n_show: i32,
        h_inst_app: isize,
        lp_id_list: *mut core::ffi::c_void,
        lp_class: *const u16,
        hkey_class: isize,
        dw_hot_key: u32,
        h_icon_or_monitor: isize,
        h_process: isize,
    }

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteExW(lp_exec_info: *mut ShellExecuteInfoW) -> i32;
    }
    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *const core::ffi::c_void, co_init: u32) -> i32;
        fn CoUninitialize();
    }

    let file_w: Vec<u16> = file.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut sei = ShellExecuteInfoW {
        cb_size: std::mem::size_of::<ShellExecuteInfoW>() as u32,
        f_mask: SEE_MASK_NOASYNC,
        hwnd: 0,
        lp_verb: std::ptr::null(),      // 默认动词 "open"，与双击相同
        lp_file: file_w.as_ptr(),
        lp_parameters: std::ptr::null(),
        lp_directory: std::ptr::null(), // 与双击同参：不指定工作目录（实测对启动无影响）
        n_show: SW_SHOWNORMAL,
        h_inst_app: 0,
        lp_id_list: std::ptr::null_mut(),
        lp_class: std::ptr::null(),
        hkey_class: 0,
        dw_hot_key: 0,
        h_icon_or_monitor: 0,
        h_process: 0,
    };

    // ShellExecuteExW 要求调用线程已初始化 COM。spawn_blocking 线程默认无 COM 状态：
    // MTA 初始化 → 调用 → 成对 CoUninitialize，不在复用线程上遗留状态。
    // RPC_E_CHANGED_MODE（线程已是 STA）时照常调用，仅跳过成对释放。
    let display = file.to_string_lossy().to_string();
    let hr = unsafe { CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED) };
    let com_inited = hr == 0 || hr == 1; // S_OK / S_FALSE（已初始化，引用计数 +1）
    let ok = unsafe { ShellExecuteExW(&mut sei) };
    if com_inited {
        unsafe { CoUninitialize() };
    }
    if ok == 0 {
        let err = std::io::Error::last_os_error();
        return Err(format!("打开失败: {} ({})", display, err));
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn shell_open(file: &Path, _directory: Option<&Path>) -> Result<(), String> {
    open::that(file).map_err(|e| e.to_string())
}

/// B-3：async + spawn_blocking——Shell 调用再慢（网络盘 / DDE）也只占用阻塞池线程，
/// 绝不堵主线程。存在性检查放 spawn_blocking 外（本地 stat，微秒级）。
#[tauri::command]
pub async fn open_file(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.exists() || !p.is_file() {
        return Err(format!("文件不存在: {}", path));
    }
    tauri::async_runtime::spawn_blocking(move || {
        // directory 参数保留给 open_folder 语义（v4 起不再用于覆写子进程 CWD，
        // 见 shell_open 文档：实测 CWD 对启动耗时无影响，传 NULL 才与双击同参）
        shell_open(Path::new(&path), Path::new(&path).parent())
    })
    .await
    .map_err(|e| format!("打开失败: {e}"))?
}

/// 从 PotPlayer 回填播放记录。
/// 数据源（按可靠性）：
///   1. 注册表 HKCU\Software\Daum\PotPlayerMini64\RememberFiles —— PotPlayer 位置记忆（权威）
///      格式：<序号> REG_SZ <位置毫秒>=<文件路径>，如 "295 REG_SZ 649035=E:\...\Trial...mkv"
///   2. dpl %APPDATA%\PotPlayerMini64\Playlist\PotPlayerMini64.dpl 的 playname=（最近打开的文件）
///      以及条目里的 start*/duration2*（PotPlayer 播放中会写位置时）
/// 增量语义：把上次见过的 {path: 位置} 快照存 settings，只有"新出现 / 位置变化 / playname 切换"
/// 的文件才刷新最近播放 + 写观看历史，避免每次同步把所有历史播放全刷成"现在"。
/// 单次同步最多补提时长的文件数。补提要启 ffprobe 子进程（或读文件头），
/// 而同步是前端 60 秒轮询触发的，一口气补太多会拖慢界面 —— 宁可分几轮摊开。
const PROBES_PER_SYNC: i64 = 3;

/// 进度只升不降；无时长数据时只记"看过"（最近播放/历史），百分比待补 duration 后自动生效。
/// 缺时长的视频会在同步时就地补提一次（见下方 PROBES_PER_SYNC 限额）。
/// async 化说明（2026-08-29「打开卡一下」修复）：
/// 此命令曾是同步命令、跑在主线程——启动时窗口获得焦点即触发 focus 事件，注册表枚举 +
/// dpl 解析 + ffprobe 子进程探测全部堵在主线程上，与初始数据加载串行排队，就是
/// 「打开时卡一下」的根因。async 命令跑在运行时线程，主线程只管渲染。
#[tauri::command]
pub async fn sync_progress_from_player(
    db: State<'_, DbConn>,
    in_flight: State<'_, SyncInFlight>,
) -> Result<i64, String> {
    // 并发防护：60s 轮询 / focus / visibilitychange 三路触发可能撞车；
    // 探测又在锁外跑，并发同步会重复探测、重复弹 toast——正在跑时直接跳过
    //（快照增量幂等，漏掉的下一轮会补上）。
    if in_flight.0.swap(true, Ordering::SeqCst) {
        return Ok(0);
    }
    let _guard = SyncInFlightGuard(&in_flight.0);
    sync_progress_impl(&db)
}

fn sync_progress_impl(db: &DbConn) -> Result<i64, String> {
    // ========== 1. 收集播放证据：path -> (位置毫秒, 时长毫秒) ==========
    let mut evidence: HashMap<String, (Option<u64>, Option<u64>)> = HashMap::new();

    #[cfg(windows)]
    {
        use winreg::enums::*;
        use winreg::RegKey;
        if let Ok(key) = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(r"Software\Daum\PotPlayerMini64\RememberFiles", KEY_READ)
        {
            for item in key.enum_values() {
                let Ok((_, value)) = item else { continue };
                if value.vtype != RegType::REG_SZ {
                    continue; // 跳过 19_a / 280_c1 等辅助 REG_DWORD 键
                }
                // REG_SZ 是 UTF-16LE，可能带尾部 \0
                let units: Vec<u16> = value
                    .bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let s = String::from_utf16_lossy(&units);
                let s = s.trim_end_matches('\0').trim();
                if s.is_empty() {
                    continue;
                }
                let Some(eq) = s.find('=') else { continue };
                let (ms, path) = s.split_at(eq);
                let path = path[1..].trim().to_string();
                if path.is_empty() {
                    continue;
                }
                let pos: Option<u64> = ms.trim().parse().ok();
                // 位置 0 = 打开即关 / 从未真正播放，不算播放证据。
                // 否则 PotPlayer 里"打开过又关掉"的文件会被当成"新看过"批量刷最近播放。
                if pos == Some(0) {
                    continue;
                }
                let entry = evidence.entry(path).or_insert((None, None));
                if pos.is_some() {
                    entry.0 = pos; // 注册表位置优先
                }
            }
        }
    }

    // dpl 兜底：playname 是最近打开/播放的文件；条目里顺带收 start/duration2（若 PotPlayer 写了）
    let mut playname: Option<String> = None;
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    let dpl = Path::new(&appdata)
        .join("PotPlayerMini64")
        .join("Playlist")
        .join("PotPlayerMini64.dpl");
    if dpl.exists() {
        if let Ok(content) = std::fs::read_to_string(&dpl) {
            for line in content.lines() {
                if let Some(p) = line.strip_prefix("playname=") {
                    let p = p.trim();
                    if !p.is_empty() {
                        playname = Some(p.to_string());
                        evidence.entry(p.to_string()).or_insert((None, None));
                    }
                    break;
                }
            }
            let mut entries: HashMap<u32, (Option<String>, Option<u64>, Option<u64>)> = HashMap::new();
            for line in content.lines() {
                let Some(star) = line.find('*') else { continue };
                let Ok(idx) = line[..star].parse::<u32>() else { continue };
                let rest = &line[star + 1..];
                let e = entries.entry(idx).or_default();
                if let Some(p) = rest.strip_prefix("file*") {
                    e.0 = Some(p.trim().to_string());
                } else if let Some(p) = rest.strip_prefix("start*") {
                    e.1 = p.trim().parse().ok();
                } else if let Some(p) = rest
                    .strip_prefix("duration2*")
                    .or_else(|| rest.strip_prefix("duration*"))
                {
                    e.2 = p.trim().parse().ok();
                }
            }
            for (_, (file, start, dur)) in entries {
                if let Some(f) = file {
                    // 纯列表条目（无播放位置、无时长）不算播放证据：
                    // 把整个文件夹拖进 PotPlayer 会让全部集都进 dpl，但没播放过的集
                    // 不该进"最近播放"（否则整部剧被批量误标）。
                    let has_evidence = start.map_or(false, |s| s > 0) || dur.map_or(false, |d| d > 0);
                    if !has_evidence {
                        continue;
                    }
                    let entry = evidence.entry(f).or_insert((None, None));
                    if entry.0.is_none() {
                        entry.0 = start; // dpl start 只在注册表没有时补充
                    }
                    if dur.is_some() {
                        entry.1 = dur;
                    }
                }
            }
        }
    }
    if evidence.is_empty() {
        return Ok(0);
    }

    // ========== 2. 锁①：读快照，算增量待处理清单（锁范围只覆盖 DB 读写）==========
    struct PendingWrite {
        vid: i64,
        cur_progress: f64,
        cur_duration: Option<f64>,
        pos_ms: Option<u64>,
        dpl_dur_ms: Option<u64>,
    }
    let mut new_snapshot: HashMap<String, u64> = HashMap::new();
    let mut pending: Vec<(String, PendingWrite)> = Vec::new();
    let is_first_sync;
    let playname_switched;
    {
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        let last_raw: Option<String> = crate::db::get_setting(&conn, "potplayer_evidence_snapshot")
            .map_err(|e| e.to_string())?;
        let parsed_snapshot = last_raw
            .as_deref()
            .and_then(|raw| serde_json::from_str::<HashMap<String, u64>>(raw).ok());
        // 快照从未建立（或损坏）→ 首轮同步：只建立基线，不写任何库。
        // 否则 RememberFiles 里全部库内文件会被当成"新播放"批量刷新，
        // 把历史最近播放全刷成同一时刻（2026-08-28 事故根因）。
        is_first_sync = parsed_snapshot.is_none();
        let last_map = parsed_snapshot.unwrap_or_default();
        let last_playname: Option<String> =
            crate::db::get_setting(&conn, "potplayer_dpl_playname").map_err(|e| e.to_string())?;
        playname_switched = last_playname.as_deref() != playname.as_deref();

        for (path, (pos, dur)) in &evidence {
            let pos_val = pos.unwrap_or(0);
            new_snapshot.insert(path.clone(), pos_val);
            let is_playname = playname.as_deref() == Some(path.as_str());
            // 首轮只建档（playname 除外——它代表正在播放的文件，值得立即记录）
            if is_first_sync && !is_playname {
                continue;
            }
            let pos_changed = last_map.get(path).copied() != Some(pos_val);
            // playname 切换 → 无条件视为"刚打开播放"；否则依赖位置变化
            if !pos_changed && !(is_playname && playname_switched) {
                continue;
            }
            // 写入侧防御：无真实播放位置（pos=0 / 无位置）且非正在播放的文件，
            // 只更新快照基线、不刷"最近播放"（否则整部剧批量误标，2026-08-30 匹诺曹案例）。
            if pos_val == 0 && !is_playname {
                continue;
            }
            // 匹配库内视频（NOCASE 索引 idx_videos_file_path_nocase 生效）
            let row = conn
                .query_row(
                    "SELECT id, watch_progress, duration FROM videos WHERE file_path = ?1 COLLATE NOCASE",
                    params![path],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, f64>(1)?,
                            r.get::<_, Option<f64>>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let Some((vid, cur_progress, cur_duration)) = row else {
                continue; // 库外文件（PotPlayer 里看了但不在库里），跳过
            };
            pending.push((
                path.clone(),
                PendingWrite { vid, cur_progress, cur_duration, pos_ms: *pos, dpl_dur_ms: *dur },
            ));
        }
    } // 锁①释放：ffprobe 探测要起子进程（秒级，可能等满超时），绝不能持锁做

    // ========== 3. 锁外探测缺时长 ==========
    // 单次同步最多补提几个文件的时长：探测要启 ffprobe 子进程（或读文件头），
    // 同步是前端 60s 轮询触发的，一口气补几十个会明显拖慢同步。宁可少补几次，分轮摊开。
    let mut probes_left: i64 = PROBES_PER_SYNC;
    let mut probed: HashMap<i64, f64> = HashMap::new();
    for (path, p) in &pending {
        if p.cur_duration.map_or(false, |d| d > 0.0) {
            continue; // 库里已有有效时长
        }
        if probes_left <= 0 {
            break;
        }
        probes_left -= 1;
        if let Some(d) = crate::media_info::probe_duration_sec(Path::new(path), true) {
            probed.insert(p.vid, d);
        }
    }

    // ========== 4. 锁②：写库 ==========
    let mut updated: i64 = 0;
    let now = now_str();
    {
        let mut conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        // 整批写入合成一个事务：每条 pending 有 4 次写入（补时长 + 更新进度 + 写历史 + 刷剧集聚合），
        // 逐条自动提交时代价是 4 次 fsync；合成一个事务后整批只落一次盘。
        // 中途出错时 `?` 提前返回，Transaction 的 Drop 会自动回滚，不会留下半截状态。
        let tx = conn.transaction().map_err(|e| AppError::db(e.to_string()))?;
        for (_, p) in &pending {
            // ---- 缺时长就地补提 ----
            // 这是进度百分比能算出来的前提：PotPlayer 只给"看到第几毫秒"，不给片长，
            // 而扫描流程从未写过 duration（全库曾长期为 NULL）→ 百分比分支永远进不去，功能形同虚设。
            let probed_sec = probed.get(&p.vid).copied();
            if let Some(d) = probed_sec {
                let _ = crate::db::set_video_duration(&tx, p.vid, d);
            }
            let duration_sec = p.cur_duration.filter(|d| *d > 0.0).or(probed_sec);

            // ---- 算百分比（量纲统一到毫秒）----
            // 注册表位置 pos 是毫秒，dpl 的 duration2 也是毫秒，而 videos.duration 是**秒**（前端 /60 显示分钟）。
            // 不换算会差 1000 倍，务必保持下面这一步的 /1000。
            let mut final_progress = p.cur_progress;
            if let Some(pos) = p.pos_ms {
                if pos > 0 {
                    let total_ms = duration_sec
                        .map(|d| d * 1000.0) // 库里是秒 → 毫秒
                        .or(p.dpl_dur_ms.map(|d| d as f64)); // dpl 的 duration2 本来就是毫秒
                    if let Some(total) = total_ms {
                        if total > 0.0 {
                            final_progress = ((pos as f64) / total * 100.0).clamp(0.0, 100.0);
                        }
                    }
                }
            }
            tx.execute(
                "UPDATE videos SET watch_progress = MAX(watch_progress, ?1), last_watched_at = ?2, updated_at = datetime('now','localtime') WHERE id = ?3",
                params![final_progress, now, p.vid],
            )
            .map_err(|e| e.to_string())?;
            // 同一次观影（5 分钟内）合并成一条，不再每次轮询新增一行
            crate::db::record_watch(&tx, p.vid, final_progress).map_err(|e| e.to_string())?;
            crate::db::sync_series_aggregate(&tx, p.vid).map_err(|e| e.to_string())?;
            updated += 1;
        }

        // 快照写回（含库外文件：位置变化是纯外部事实，与是否入库无关）
        if let Ok(snap_json) = serde_json::to_string(&new_snapshot) {
            let _ = crate::db::set_setting(&tx, "potplayer_evidence_snapshot", &snap_json);
        }
        if let Some(p) = &playname {
            let _ = crate::db::set_setting(&tx, "potplayer_dpl_playname", p);
        }
        tx.commit().map_err(|e| AppError::db(e.to_string()))?;
    }
    Ok(updated)
}

// ============================================================
// 时长批量补全
// ============================================================

/// 并发线程数。本地盘 IO + ffprobe 进程，8 条足够吃满又不会把磁盘打爆。
const BACKFILL_THREADS: usize = 8;
/// 攒够多少条结果写一次库（减少事务次数）
const BACKFILL_FLUSH: usize = 200;

/// 批量补全所有缺时长的视频。
/// 自研容器解析优先（mp4/mkv 零子进程），ffprobe 兜底（全格式）。
/// 通过 `backfill-progress` 事件回报进度，可用 cancel_duration_backfill 中断。
///
/// **2026-08-30 重写（旧版是 duration 填充率 7/5078 的根因）**：
/// 1. 旧版是同步 fn 命令 → 整个 `rx.recv()` 循环跑在**主线程**上，UI 冻结数分钟，
///    用户以为假死直接杀进程；且 `cancel_duration_backfill` 也是同步命令需要主线程派发，
///    形成"取消按钮永远点不动"的死锁式体验。
///    现在改 async + `spawn_blocking`：async worker 立即释放，循环跑在 blocking 池，
///    主线程空闲 → 取消按钮真正生效。
/// 2. 旧版在循环**外**就 `db.0.lock()` 并持有数分钟 → 期间一切查询（含 60s 轮询的同步）
///    排队到 busy_timeout 报 database is locked。现在只在攒批落库的一瞬取锁（flush_durations）。
#[tauri::command]
pub async fn backfill_durations(
    db: State<'_, DbConn>,
    app: tauri::AppHandle,
    cancel: State<'_, BackfillCancel>,
) -> Result<BackfillResult, String> {
    let jobs: Vec<(i64, String)> = {
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        crate::db::get_videos_missing_duration(&conn).map_err(|e| e.to_string())?
    };
    let total = jobs.len() as i64;
    if total == 0 {
        return Ok(BackfillResult { total: 0, filled: 0, failed: 0, cancelled: false });
    }
    cancel.0.store(false, Ordering::Relaxed);

    let jobs = Arc::new(jobs);
    let next = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel::<(i64, String, Option<f64>)>();

    for _ in 0..BACKFILL_THREADS {
        let jobs = Arc::clone(&jobs);
        let next = Arc::clone(&next);
        let tx = tx.clone();
        let flag = Arc::clone(&cancel.0);
        std::thread::spawn(move || {
            loop {
                if flag.load(Ordering::Relaxed) {
                    break;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= jobs.len() {
                    break;
                }
                let (id, path) = &jobs[i];
                let d = crate::media_info::probe_duration_sec(Path::new(path), true);
                if tx.send((*id, path.clone(), d)).is_err() {
                    break; // 接收端已放弃
                }
            }
        });
    }
    drop(tx); // 本线程持有的一份必须丢掉，否则 rx.recv() 永不返回 Err

    // 消费循环丢进 blocking 线程池：闭包需要 'static，所以 DB 句柄从 AppHandle 现取、
    // 取消标志克隆内部 Arc（State 本身带借用生命周期，进不了 'static 闭包）。
    let cancel_flag = Arc::clone(&cancel.0);
    let handle = tauri::async_runtime::spawn_blocking(move || {
        let db_state = app.state::<DbConn>();
        let mut done: i64 = 0;
        let mut filled: i64 = 0;
        let mut failed: i64 = 0;
        let mut pending: Vec<(i64, f64)> = Vec::new();
        let mut cancelled = false;

        while let Ok((id, path, d)) = rx.recv() {
            done += 1;
            match d {
                Some(sec) => {
                    filled += 1;
                    pending.push((id, sec));
                }
                None => failed += 1,
            }
            if pending.len() >= BACKFILL_FLUSH {
                flush_durations(&db_state, &mut pending);
            }
            // 进度回报：每 10 条或收尾时发一次，避免刷屏
            if done % 10 == 0 || done == total {
                let _ = app.emit(
                    "backfill-progress",
                    BackfillProgress { done, total, filled, failed, current: path.clone() },
                );
            }
            if cancel_flag.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
        }

        // 收尾：把余下结果落库（含取消前已收到的部分——用户点取消不该丢掉已完成的工作）
        flush_durations(&db_state, &mut pending);
        BackfillResult { total, filled, failed, cancelled }
    });

    match handle.await {
        Ok(result) => Ok(result),
        Err(e) => Err(format!("补全任务异常退出: {e}")),
    }
}

/// 补全的攒批落库：短暂取锁 + 单事务提交。
/// BACKFILL_FLUSH=200 条在 synchronous=NORMAL 下约 20 ms，期间其他查询最多等这一瞬。
/// 绝不在探测等待期间持锁。落库失败只记日志不打断补全（与旧版 `let _ =` 行为一致，
/// 但旧版的 `execute_batch("BEGIN")` 在失败时会退化成逐条自动提交；
/// 这里 `transaction()` 失败则整批跳过，不会退化）。
fn flush_durations(db: &DbConn, pending: &mut Vec<(i64, f64)>) {
    if pending.is_empty() {
        return;
    }
    let mut conn = match db.0.lock() {
        Ok(c) => c,
        Err(_) => {
            eprintln!("[cinevault] 补全落库失败：数据库锁不可用");
            return;
        }
    };
    let tx = match conn.transaction() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[cinevault] 补全落库失败：无法开启事务: {e}");
            return;
        }
    };
    let mut failed = 0;
    for (vid, sec) in pending.drain(..) {
        if crate::db::set_video_duration(&tx, vid, sec).is_err() {
            failed += 1;
        }
    }
    if let Err(e) = tx.commit() {
        eprintln!("[cinevault] 补全落库失败：事务提交: {e}");
    } else if failed > 0 {
        eprintln!("[cinevault] 补全落库：{failed} 条写失败");
    }
}

#[tauri::command]
pub fn cancel_duration_backfill(cancel: State<'_, BackfillCancel>) -> Result<(), String> {
    cancel.0.store(true, Ordering::Relaxed);
    Ok(())
}

/// 设置页显示用：ffprobe 是否可用、走的哪个路径。
/// async 化：resolve_ffprobe 可能真的起 ffprobe -version 子进程同步等返回（probe_runs），
/// Sidebar 一挂载就调用本命令——同步版会在启动时堵主线程。
/// 走缓存版：首次调用后路径就固定在进程级 OnceLock 里，之后零开销，且与探测共用同一份结果。
#[tauri::command]
pub async fn get_ffprobe_status() -> FfprobeStatus {
    // 首次解析会真的起一次 `ffprobe -version`，不能在 async 运行时线程上同步等子进程。
    tauri::async_runtime::spawn_blocking(|| match crate::media_info::resolve_ffprobe_cached() {
        Some(p) => FfprobeStatus { available: true, path: Some(p.to_string()) },
        None => FfprobeStatus { available: false, path: None },
    })
    .await
    .unwrap_or(FfprobeStatus { available: false, path: None })
}

/// B-3：async + spawn_blocking，理由同 open_file——Shell 慢不堵主线程。
#[tauri::command]
pub async fn open_folder(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    let target = if p.is_dir() {
        path.clone()
    } else {
        p.parent().map(|par| par.to_string_lossy().to_string()).unwrap_or_else(|| path.clone())
    };
    if !Path::new(&target).exists() {
        return Err(format!("文件夹不存在: {}", target));
    }
    tauri::async_runtime::spawn_blocking(move || {
        shell_open(Path::new(&target), Some(Path::new(&target)))
    })
    .await
    .map_err(|e| format!("打开失败: {e}"))?
}

#[tauri::command]
pub fn rename_video(db: State<'_, DbConn>, video_id: i64, new_title: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::rename_video(&conn, video_id, new_title).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rename_series(db: State<'_, DbConn>, series_id: i64, new_name: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::rename_series(&conn, series_id, new_name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_video(
    app: tauri::AppHandle,
    db: State<'_, DbConn>,
    video_id: i64,
) -> Result<(), String> {
    {
        // 锁只在删记录期间持有。缩略图清理是文件 IO，放到锁外做，
        // 别让其他查询陪着等一次可能很慢的 unlink（杀毒软件扫描时尤其慢）。
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        crate::db::delete_video(&conn, video_id).map_err(|e| e.to_string())?;
    }
    crate::thumbnail::remove_cached_thumbnail(&app, video_id);
    Ok(())
}

#[tauri::command]
pub fn delete_series(
    app: tauri::AppHandle,
    db: State<'_, DbConn>,
    series_id: i64,
) -> Result<(), String> {
    let video_ids = {
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        // 先取 id 再删：DELETE 之后这批 id 就无从查起，而缓存的 jpg 正是按 id 命名。
        let ids = crate::db::list_series_video_ids(&conn, series_id).map_err(|e| e.to_string())?;
        crate::db::delete_series(&conn, series_id).map_err(|e| e.to_string())?;
        ids
    };
    crate::thumbnail::remove_cached_thumbnails(&app, &video_ids);
    Ok(())
}

#[tauri::command]
pub async fn delete_video_with_file(
    app: tauri::AppHandle,
    db: State<'_, DbConn>,
    video_id: i64,
) -> Result<String, String> {
    let file_path: String = {
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        conn.query_row(
            "SELECT file_path FROM videos WHERE id = ?1",
            rusqlite::params![video_id],
            |r| r.get(0),
        )
        .map_err(|_| AppError::db("视频不存在"))?
    };
    // 文件删除在 DB 锁外、blocking 线程上做：网络盘/大文件删除失败或变慢
    // 都不该把整个界面卡住，也不能让其他查询陪着等。
    let delete_path = file_path.clone();
    let remove = tauri::async_runtime::spawn_blocking(move || std::fs::remove_file(delete_path))
        .await
        .map_err(|e| format!("文件删除任务异常: {e}"))?;
    match remove {
        Ok(()) => {
            {
                let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
                crate::db::delete_video(&conn, video_id).map_err(|e| e.to_string())?;
            }
            crate::thumbnail::remove_cached_thumbnail(&app, video_id);
            Ok(format!("已删除文件: {}", file_path))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            {
                let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
                crate::db::delete_video(&conn, video_id).map_err(|e| e.to_string())?;
            }
            crate::thumbnail::remove_cached_thumbnail(&app, video_id);
            Ok(format!("记录已删除，文件不存在: {}", file_path))
        }
        // 文件没删掉 = 记录保留 = 缓存也留着，下次打开还要用。
        Err(e) => Err(format!("文件删除失败，记录已保留: {}", e)),
    }
}

// ============================================
// 待确认队列
// ============================================
#[tauri::command]
pub fn get_pending(db: State<'_, DbConn>) -> Result<Vec<PendingItem>, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_pending_videos(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn resolve_pending(
    db: State<'_, DbConn>,
    video_id: i64,
    kind: String,
    series_name: Option<String>,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::resolve_pending(&conn, video_id, &kind, series_name.as_deref()).map_err(|e| e.to_string())
}

// ============================================
// 扫描目录管理
// ============================================
#[tauri::command]
pub fn get_scan_folders(db: State<'_, DbConn>) -> Result<Vec<ScanFolder>, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_scan_folders(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_scan_config(db: State<'_, DbConn>) -> Result<ScanConfig, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    Ok(load_scan_rules(&conn))
}

#[tauri::command]
pub fn set_scan_config(
    db: State<'_, DbConn>,
    min_size_mb: i64,
    exempt_prefixes: Vec<String>,
) -> Result<ScanConfig, String> {
    if !(0..=10240).contains(&min_size_mb) {
        return Err("最小入库体积必须在 0–10240 MB 之间".to_string());
    }
    if exempt_prefixes.len() > 50 {
        return Err("体积豁免前缀最多 50 条".to_string());
    }
    let normalized = normalize_prefixes(&exempt_prefixes.join(","));
    if normalized.iter().any(|p| p.chars().count() > 64) {
        return Err("单个豁免前缀不能超过 64 个字符".to_string());
    }
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::set_setting(&conn, "scan_min_size_mb", &min_size_mb.to_string())
        .map_err(|e| e.to_string())?;
    crate::db::set_setting(
        &conn,
        "scan_size_exempt_prefixes",
        &normalized.join(","),
    )
    .map_err(|e| e.to_string())?;
    Ok(ScanConfig { min_size_mb, exempt_prefixes: normalized })
}

#[tauri::command]
pub fn delete_scan_folder(db: State<'_, DbConn>, folder_id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::delete_scan_folder(&conn, folder_id).map_err(|e| e.to_string())
}

// ============================================
// 迁移
// ============================================
/// 探测旧库：返回旧库路径（存在即返回——已迁移过也可重复同步，迁移幂等）
#[tauri::command]
pub fn check_migration() -> Result<Option<String>, String> {
    Ok(crate::migrator::locate_old_db())
}

#[tauri::command]
pub async fn migrate_from_old(db: State<'_, DbConn>, old_path: Option<String>) -> Result<MigrationResult, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    let path = match old_path {
        Some(p) => p,
        None => crate::migrator::locate_old_db().ok_or_else(|| "未找到旧数据库".to_string())?,
    };
    crate::migrator::migrate(&conn, &path).map_err(|e| e.to_string())
}

// ============================================
// 导出 / 导入
// ============================================
#[derive(serde::Serialize)]
pub struct ExportFileResult {
    pub path: String,
    pub videos: i64,
    pub series: i64,
}

#[derive(serde::Serialize)]
pub struct ImportFileResult {
    #[serde(flatten)]
    pub result: ImportResult,
    /// 导入前自动生成的整库快照路径
    pub backup_path: String,
}

/// B-1：async 化——导出要全库读出（5,078 视频 + 2,433 历史），同步版在主线程上
/// 持锁做全部查询，大库时界面冻住。读库放 blocking 池，DB 锁只在读库期间持有。
#[tauri::command]
pub async fn export_json(app: tauri::AppHandle) -> Result<ExportData, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<DbConn>();
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        crate::db::export_data(&conn).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("导出失败: {e}"))?
}

/// 主路径：后端直接写文件。大库 JSON 序列化/写盘全在 blocking 线程，
/// 不再把几十 MB 数据经 IPC 搬到 WebView 主线程上 stringify。
#[tauri::command]
pub async fn export_json_to_file(
    app: tauri::AppHandle,
    file_path: String,
) -> Result<ExportFileResult, String> {
    if !file_path.to_lowercase().ends_with(".json") {
        return Err("备份文件必须使用 .json 扩展名".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<DbConn>();
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        let data = crate::db::export_data(&conn).map_err(|e| e.to_string())?;
        let videos = data.videos.len() as i64;
        let series = data.series.len() as i64;
        let json = serde_json::to_string_pretty(&data)
            .map_err(|e| format!("序列化备份失败: {e}"))?;
        let p = Path::new(&file_path);
        if let Some(parent) = p.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("创建备份目录失败: {e}"))?;
            }
        }
        std::fs::write(&file_path, json).map_err(|e| format!("写入备份失败: {e}"))?;
        Ok(ExportFileResult { path: file_path, videos, series })
    })
    .await
    .map_err(|e| format!("导出任务异常: {e}"))?
}

/// B-1：async 化，理由同 export_json——导入是批量写 + 全量校验，放 blocking 池。
/// 该入口保留给旧前端/调试调用；正式 UI 走 import_json_from_file（自动备份 + 不搬大字符串）。
#[tauri::command]
pub async fn import_json(app: tauri::AppHandle, json_str: String) -> Result<ImportResult, String> {
    const MAX_IMPORT_SIZE: usize = 50_000_000;
    if json_str.len() > MAX_IMPORT_SIZE {
        return Err(format!("导入数据过大（{} 字节），最大支持 50MB", json_str.len()));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let data: ExportData = serde_json::from_str(&json_str)
            .map_err(|e| AppError::validation(format!("JSON 解析失败: {}", e)))?;
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("读取应用数据目录失败: {e}"))?;
        let db = app.state::<DbConn>();
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        // 旧入口同样先整库快照，避免任何导入路径绕过安全闸。
        crate::db::backup_database(&conn, &data_dir, "pre-import")
            .map_err(|e| format!("导入前备份失败，已中止: {e}"))?;
        crate::db::import_data(&conn, &data).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("导入失败: {e}"))?
}

/// 正式导入路径：后端读文件/解析/整库备份/导入一气呵成。
/// 返回自动备份路径，前端可以明确告诉用户“后悔药在哪”。
#[tauri::command]
pub async fn import_json_from_file(
    app: tauri::AppHandle,
    file_path: String,
) -> Result<ImportFileResult, String> {
    // 与导出保持一致：导出端不再限制大小，导入端也不能在 50MB 处把自家备份拒之门外。
    // 512MB 已远超几万条视频 + 历史的 JSON 体积，读取仍在 blocking 线程完成。
    const MAX_IMPORT_SIZE: u64 = 512 * 1024 * 1024;
    let p = Path::new(&file_path);
    if !p.is_file() {
        return Err(format!("备份文件不存在: {}", file_path));
    }
    if !file_path.to_lowercase().ends_with(".json") {
        return Err("仅支持导入 .json 备份".to_string());
    }
    if p.metadata().map(|m| m.len()).unwrap_or(0) > MAX_IMPORT_SIZE {
        return Err("备份文件超过 512MB，已拒绝导入".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let content = std::fs::read_to_string(&file_path)
            .map_err(|e| format!("读取备份失败: {e}"))?;
        let data: ExportData = serde_json::from_str(&content)
            .map_err(|e| format!("JSON 解析失败: {e}"))?;
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("读取应用数据目录失败: {e}"))?;
        let db = app.state::<DbConn>();
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        let backup_path = crate::db::backup_database(&conn, &data_dir, "pre-import")
            .map_err(|e| format!("导入前备份失败，已中止: {e}"))?;
        let result = crate::db::import_data(&conn, &data).map_err(|e| e.to_string())?;
        Ok(ImportFileResult { result, backup_path })
    })
    .await
    .map_err(|e| format!("导入任务异常: {e}"))?
}

/// 读取文本文件（保留给调试/旧调用；仅限 .json 备份）。
/// 50MB 文件读盘放到 blocking 池，绝不占用 WebView 主线程。
#[tauri::command]
pub async fn read_text_file(file_path: String) -> Result<String, String> {
    const MAX_JSON_SIZE: u64 = 50_000_000;
    tauri::async_runtime::spawn_blocking(move || {
        let p = Path::new(&file_path);
        if !p.exists() || !p.is_file() {
            return Err(format!("文件不存在: {}", file_path));
        }
        if !file_path.to_lowercase().ends_with(".json") {
            return Err("仅支持读取 .json 文件".to_string());
        }
        if p.metadata().map(|m| m.len()).unwrap_or(0) > MAX_JSON_SIZE {
            return Err("文件过大（超过 50MB）".to_string());
        }
        std::fs::read_to_string(&file_path).map_err(|e| format!("读取文件失败: {}", e))
    })
    .await
    .map_err(|e| format!("读取任务异常: {e}"))?
}

/// 写文本文件（保留给调试/旧调用；仅限 .json）
#[tauri::command]
pub async fn write_text_file(file_path: String, content: String) -> Result<(), String> {
    const MAX_JSON_SIZE: usize = 50_000_000;
    if !file_path.to_lowercase().ends_with(".json") {
        return Err("仅支持写入 .json 文件".to_string());
    }
    if content.len() > MAX_JSON_SIZE {
        return Err("内容过大（超过 50MB）".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let p = Path::new(&file_path);
        if let Some(parent) = p.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
            }
        }
        std::fs::write(&file_path, content).map_err(|e| format!("写入文件失败: {}", e))
    })
    .await
    .map_err(|e| format!("写入任务异常: {e}"))?
}

// ============================================
// 驱动器（供扫描对话框使用）
// ============================================
#[derive(serde::Serialize)]
pub struct DriveInfo {
    pub letter: String,
    pub path: String,
    pub label: String,
}

/// 盘符列表（扫描弹窗用）。
/// B-2：Windows 用 GetLogicalDrives() 位掩码枚举，绝不逐个 Path::exists() 探测——
/// 探测 A: 空软驱 / 未就绪读卡器会触发系统错误弹窗或数秒长超时（经典 Win32 坑）。
/// 位掩码是系统已挂载盘的权威列表，一次 FFI 调用微秒级返回，零探测开销。
#[tauri::command]
pub fn get_drives() -> Result<Vec<DriveInfo>, String> {
    #[cfg(target_os = "windows")]
    {
        extern "system" {
            fn GetLogicalDrives() -> u32;
        }
        let mask = unsafe { GetLogicalDrives() };
        let mut drives = Vec::new();
        for i in 0..26u32 {
            if mask & (1 << i) != 0 {
                let letter = (b'A' + i as u8) as char;
                let path = format!("{}:\\", letter);
                drives.push(DriveInfo {
                    letter: letter.to_string(),
                    path,
                    label: format!("本地磁盘 ({})", letter),
                });
            }
        }
        Ok(drives)
    }
    #[cfg(not(target_os = "windows"))]
    {
        // 非 Windows：Path::exists 不会触发系统弹窗，保持简单实现
        let mut drives = Vec::new();
        for letter in 'A'..='Z' {
            let path = format!("{}:\\", letter);
            if Path::new(&path).exists() {
                drives.push(DriveInfo {
                    letter: letter.to_string(),
                    path,
                    label: format!("本地磁盘 ({})", letter),
                });
            }
        }
        Ok(drives)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_series, dir_series_owners, load_scan_rules, match_folder_anchor, path_key,
        path_within_roots, scan_rules_bytes, MIN_ANCHOR_EPISODES,
    };
    use crate::scanner;
    use std::collections::HashMap;

    fn anchors() -> Vec<(i64, String)> {
        // P-6 之后 match_folder_anchor 约定入参已归一化（load_folder_anchors 保证），
        // 测试夹具同步预归一化，模拟真实调用路径。
        vec![
            (1, "h:\\待看的剧".to_string()),
            (2, "h:\\待看的剧\\绿灯侠".to_string()),
            (3, "e:\\动漫风车".to_string()),
        ]
    }

    #[test]
    fn anchor_hits_subfolder() {
        // 绿灯侠 folder_path 最长前缀优先，命中 2 而非 1
        let a = anchors();
        assert_eq!(
            match_folder_anchor(
                "H:\\待看的剧\\绿灯侠\\Lanterns.2026.S01E02.1080p.官方中英.mp4",
                &a
            ),
            Some(2)
        );
    }

    #[test]
    fn anchor_hits_loose_prefix() {
        let a = anchors();
        // 直接在父目录下的文件 → 归父目录剧集
        assert_eq!(
            match_folder_anchor("H:\\待看的剧\\SomeShow.S01E01.mkv", &a),
            Some(1)
        );
    }

    #[test]
    fn anchor_respects_dir_boundary() {
        // H:\待看的剧集 不是 H:\待看的剧 的子目录，不得误配
        let a = anchors();
        assert_eq!(
            match_folder_anchor("H:\\待看的剧集\\X.S01E01.mkv", &a),
            None
        );
    }

    #[test]
    fn anchor_is_case_insensitive() {
        let a = anchors();
        assert_eq!(
            match_folder_anchor("h:\\待看的剧\\绿灯侠\\ep02.mp4", &a),
            Some(2)
        );
    }

    #[test]
    fn anchor_no_match() {
        let a = anchors();
        assert_eq!(
            match_folder_anchor("Z:\\电影\\Batman.2022.mkv", &a),
            None
        );
    }

    #[test]
    fn cleanup_roots_are_case_insensitive_and_boundary_safe() {
        let roots = vec![path_key("H:\\Movies")];
        assert!(path_within_roots("h:\\movies\\A.mkv", &roots));
        assert!(path_within_roots("H:/Movies/A.mkv", &roots));
        assert!(!path_within_roots("H:\\Movies2\\A.mkv", &roots));
        assert!(!path_within_roots("Z:\\Movies\\A.mkv", &roots));
        // 盘根 H:\ 归一化成 h:，仍应按目录边界命中
        let drive_root = vec![path_key("H:\\")];
        assert!(path_within_roots("H:\\Movies\\A.mkv", &drive_root));
    }

    #[test]
    fn cleanup_roots_empty_never_matches() {
        assert!(!path_within_roots("H:\\Movies\\A.mkv", &[]));
    }

    #[test]
    fn scan_rules_are_configurable_and_default_safe() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::initialize_database(&conn).unwrap();
        let defaults = load_scan_rules(&conn);
        assert_eq!(defaults.min_size_mb, 50);
        assert!(defaults.exempt_prefixes.iter().any(|p| p == "Pingu"));
        assert_eq!(scan_rules_bytes(&defaults), 50 * 1024 * 1024);

        crate::db::set_setting(&conn, "scan_min_size_mb", "0").unwrap();
        crate::db::set_setting(&conn, "scan_size_exempt_prefixes", "短片，Demo, 短片").unwrap();
        let custom = load_scan_rules(&conn);
        assert_eq!(custom.min_size_mb, 0);
        assert_eq!(custom.exempt_prefixes, vec!["短片", "Demo"]);
        assert_eq!(scan_rules_bytes(&custom), 0);
    }

    // ============================================
    // 方案 A 端到端：目录共识命名必须把「一个文件夹」收敛成「一个剧集」
    // 复现 2026-09-19「我与超人的冒险」事故的真实形态：
    // 27 集分四种发布组标记（EDITH / NTb / HMAX / xingfan），
    // 旧逻辑会按 clean_series_name 逐文件起名 → 拆成 7 个剧集。
    // ============================================
    fn superman_episode_paths() -> Vec<String> {
        let d = "H:\\待看的剧\\我与超人冒险";
        let mut v = Vec::new();
        // S01E01~E10：NTb / EDITH 混排
        v.push(format!("{}\\My.Adventures.with.Superman.S01E01.Adventures.of.a.Normal.Man.Part.1.1080p.MAX.WEB-DL.DDP5.1.x264-NTb.chs.eng.mp4", d));
        v.push(format!("{}\\My.Adventures.with.Superman.S01E02.Adventures.of.a.Normal.Man.Part.2.1080p.MAX.WEB-DL.DDP5.1.x264-NTb.chs.eng.mp4", d));
        v.push(format!("{}\\My.Adventures.with.Superman.S01E03.1080p.WEB.h264-EDITH.chs.eng.mp4", d));
        v.push(format!("{}\\My.Adventures.with.Superman.S01E04.1080p.WEB.h264-EDITH.chs.eng.mp4", d));
        v.push(format!("{}\\My.Adventures.with.Superman.S01E05.You.Will.Believe.a.Man.Can.Lie.1080p.HMAX.WEB-DL.DD5.1.x264-NTb.chs.eng.mp4", d));
        v.push(format!("{}\\My.Adventures.with.Superman.S01E06.1080p.WEB.h264-EDITH.chs.eng.mp4", d));
        v.push(format!("{}\\My.Adventures.with.Superman.S01E07.1080p.WEB.h264-EDITH.chs.eng.mp4", d));
        v.push(format!("{}\\My.Adventures.with.Superman.S01E08.Zero.Day.Part.1.1080p.HMAX.WEB-DL.DD5.1.x264-NTb.chs.eng.mp4", d));
        v.push(format!("{}\\My.Adventures.with.Superman.S01E09.1080p.WEB.h264-EDITH.chs.eng.mp4", d));
        v.push(format!("{}\\My.Adventures.with.Superman.S01E10.1080p.WEB.h264-EDITH[eztv.re].chs.eng.mp4", d));
        // S02E01~E10：清一色 EDITH
        for e in 1..=10 {
            v.push(format!("{}\\My.Adventures.with.Superman.S02E{:02}.1080p.WEB.h264-EDITH.chs.eng.mp4", d, e));
        }
        // S03E01~E07：xingfan 站，带中文前缀
        for e in 1..=7 {
            v.push(format!("{}\\我与超人的冒险.My.Adventures.with.Superman.S03E{:02}.1080p.CHS&ENG.www.xingfan.cc.mp4", d, e));
        }
        v
    }

    /// 把路径列表转成 scanner::VideoInfo 需要的 sibling_map
    fn sibling_map_of(paths: &[String]) -> HashMap<std::path::PathBuf, Vec<String>> {
        let files: Vec<std::path::PathBuf> =
            paths.iter().map(|s| std::path::PathBuf::from(s)).collect();
        scanner::build_sibling_map_public(&files)
    }

    /// 方案 A 的算法正确性（绕过生产开关直测核心）。
    /// 生产入口 directory_series_names 因 ENABLE_DIRECTORY_CONSENSUS=false 返回空表，
    /// 这里验证「开关一旦打开，算法本身是对的」——避免开关开了才发现算法有洞。
    #[test]
    fn consensus_algorithm_is_correct_when_enabled() {
        let paths = superman_episode_paths();
        assert_eq!(paths.len(), 27, "样本应为 27 集");

        let files: Vec<std::path::PathBuf> =
            paths.iter().map(|s| std::path::PathBuf::from(s)).collect();
        let names = crate::consensus::directory_series_names_ungated_for_test(&files);

        let dir = std::path::PathBuf::from("H:\\待看的剧\\我与超人冒险");
        let name = names.get(&dir).expect("该目录应产出共识名");

        println!("共识剧集名 = {:?}", name);
        // 发布组标记一个都不能进名字
        for bad in ["edith", "ntb", "hmax", "xingfan", "www", "cc", "eztv", "1080p", "web", "ddp5"] {
            assert!(!name.to_lowercase().contains(bad), "污染词 {} 进了名字: {}", bad, name);
        }
        let lower = name.to_lowercase();
        assert!(lower.contains("superman"), "缺本体名: {}", name);
        assert!(lower.contains("adventures"), "缺本体名: {}", name);
    }

    /// 生产入口在开关关闭时必须返回空表（= 行为与改动前完全一致，零风险）。
    #[test]
    fn consensus_entry_is_noop_when_disabled() {
        let paths = superman_episode_paths();
        let files: Vec<std::path::PathBuf> =
            paths.iter().map(|s| std::path::PathBuf::from(s)).collect();
        assert!(
            crate::consensus::directory_series_names(&files).is_empty(),
            "开关关闭时生产入口必须返回空表"
        );
    }

    /// 事故复现 + 两条修复路线的效果对比（核心测试）。
    ///
    /// 断言分两段，把这个坑的真实形态钉死在测试里：
    ///   ① 方案 A 关闭时（= 当前线上行为）：27 集被拆成 **7 个剧集**
    ///      —— 正是 2026-09-19「我与超人的冒险」事故的数字，
    ///      证明诊断准确、测试忠实建模，不是纸上推演
    ///   ② 方案 A 临时开启：收敛为 1 个剧集（算法能力上限）
    ///
    /// 当前发布**只启用方案 C**（扫描后自动补 folder_path 锚点，不改名、不动已有剧集），
    /// A 因广告水印污染风险暂缓（详见 consensus.rs 的 ENABLE_DIRECTORY_CONSENSUS 注释）。
    #[test]
    fn incident_reproduction_and_fix_paths() {
        let paths = superman_episode_paths();
        let anchors: Vec<(i64, String)> = Vec::new(); // 首次扫描：无任何锚点

        // 跑一遍全目录，返回 (剧集数, 首个剧集名, 未归属数)
        let run = |use_consensus: bool| -> (i64, String, i64) {
            let conn = rusqlite::Connection::open_in_memory().unwrap();
            crate::db::initialize_database(&conn).unwrap();
            let sib = sibling_map_of(&paths);
            let files: Vec<std::path::PathBuf> =
                paths.iter().map(|s| std::path::PathBuf::from(s)).collect();
            let cnames = if use_consensus {
                crate::consensus::directory_series_names_ungated_for_test(&files)
            } else {
                crate::consensus::directory_series_names(&files)
            };

            let tx = conn.unchecked_transaction().unwrap();
            for p in &paths {
                let path = std::path::Path::new(p);
                let info = scanner::extract_video_info(path, &sib);
                let cname = path.parent().and_then(|d| cnames.get(d)).map(|s| s.as_str());
                let sid = apply_series(&tx, &info, &anchors, cname).unwrap();
                tx.execute(
                    "INSERT INTO videos (title, file_path, file_name, kind, series_id, season, episode)
                     VALUES (?1, ?2, ?2, 'series', ?3, ?4, ?5)",
                    rusqlite::params![info.title, p, sid, info.season, info.episode],
                )
                .unwrap();
            }
            tx.commit().unwrap();
            let n: i64 = conn.query_row("SELECT COUNT(*) FROM series", [], |r| r.get(0)).unwrap();
            let name: String = conn
                .query_row("SELECT name FROM series ORDER BY id LIMIT 1", [], |r| r.get(0))
                .unwrap();
            let orphan: i64 = conn
                .query_row("SELECT COUNT(*) FROM videos WHERE series_id IS NULL", [], |r| r.get(0))
                .unwrap();
            (n, name, orphan)
        };

        // ① 当前线上行为（A 关闭）：复现事故
        let (n_off, name_off, orph_off) = run(false);
        println!("[A 关闭/线上] 剧集数={} 未归属={} 首名={:?}", n_off, orph_off, name_off);
        assert_eq!(n_off, 7, "应复现事故的 7 个剧集（实得 {}）", n_off);
        assert_eq!(orph_off, 0, "即使拆散也不该有未归属");

        // ② 开启 A：收敛（证明算法能力）
        let (n_on, name_on, orph_on) = run(true);
        println!("[A 开启] 剧集数={} 未归属={} 首名={:?}", n_on, orph_on, name_on);
        assert_eq!(n_on, 1, "开启共识后应收敛为 1 个剧集（实得 {}）", n_on);
        assert_eq!(orph_on, 0);
        assert!(
            !name_on.to_lowercase().contains("edith"),
            "剧集名被发布组污染: {}",
            name_on
        );
    }

    /// 方案 C 的生产价值：把锚点收紧后再扫一遍，新集不再被拆散。
    /// 这是当前发布真正生效的防线（A 关闭状态下依然成立）。
    #[test]
    fn anchor_tightening_prevents_future_splitting() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::initialize_database(&conn).unwrap();

        // 模拟 A 关闭时首次扫描的结果：一个无锚点剧集，6 集同目录
        conn.execute("INSERT INTO series (id, name, folder_path) VALUES (1, '待收紧剧集', NULL)", [])
            .unwrap();
        for e in 1..=6 {
            conn.execute(
                "INSERT INTO videos (title, file_path, file_name, kind, series_id, season, episode)
                 VALUES (?1, ?2, ?2, 'series', 1, 1, ?3)",
                rusqlite::params![
                    format!("ep{}", e),
                    format!("H:\\测试\\待收紧剧集\\ep{}.mp4", e),
                    e
                ],
            )
            .unwrap();
        }

        // 执行收紧（与 refresh_all_scans 内的逻辑同源）
        let fp: String = conn
            .query_row("SELECT file_path FROM videos WHERE series_id = 1 LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let folder = fp[..fp.rfind('\\').unwrap()].to_string();
        conn.execute("UPDATE series SET folder_path = ?1 WHERE id = 1", rusqlite::params![folder])
            .unwrap();

        // 收紧后：新集命中锚点 → 直接归该剧集，不再走分类器起名
        let anchors = vec![(1i64, "h:\\测试\\待收紧剧集".to_string())];
        assert_eq!(
            match_folder_anchor("H:\\测试\\待收紧剧集\\ep99.mkv", &anchors),
            Some(1),
            "收紧后新集应命中锚点，从而免疫拆散"
        );
    }

    /// 方案 C：无锚点剧集在扫描后应被自动补上 folder_path。
    /// 复现「先下载、后刷新」第一次扫描的收口动作。
    #[test]
    fn anchor_tightening_fills_missing_folder_path() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::initialize_database(&conn).unwrap();

        // 造一个无锚点剧集 + 6 集（达标 MIN_ANCHOR_EPISODES），全部同目录
        conn.execute(
            "INSERT INTO series (id, name, folder_path) VALUES (1, '测试剧集', NULL)",
            [],
        )
        .unwrap();
        for e in 1..=6 {
            conn.execute(
                "INSERT INTO videos (title, file_path, file_name, kind, series_id, season, episode)
                 VALUES (?1, ?2, ?2, 'series', 1, 1, ?3)",
                rusqlite::params![
                    format!("ep{}", e),
                    format!("H:\\测试库\\测试剧集\\ep{}.mp4", e),
                    e
                ],
            )
            .unwrap();
        }

        // 复现收紧查询（与 refresh_all_scans 内的逻辑同源）
        let cands: Vec<(i64, String, i64)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT se.id, se.name, COUNT(*) AS n,
                            COUNT(DISTINCT rtrim(v.file_path, replace(v.file_path, '\\', ''))) AS dirs
                     FROM series se
                     JOIN videos v ON v.series_id = se.id
                     WHERE se.folder_path IS NULL OR se.folder_path = ''
                     GROUP BY se.id, se.name
                     HAVING n >= ?1 AND dirs = 1",
                )
                .unwrap();
            let rows = stmt
                .query_map(rusqlite::params![MIN_ANCHOR_EPISODES as i64], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
                })
                .unwrap();
            rows.filter_map(|r| r.ok()).collect()
        };
        assert_eq!(cands.len(), 1, "应识别出 1 个待收紧剧集，实得 {:?}", cands);

        let fp: String = conn
            .query_row("SELECT file_path FROM videos WHERE series_id = 1 LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let folder = fp[..fp.rfind('\\').unwrap()].to_string();
        conn.execute(
            "UPDATE series SET folder_path = ?1 WHERE id = 1",
            rusqlite::params![folder],
        )
        .unwrap();

        let got: Option<String> = conn
            .query_row("SELECT folder_path FROM series WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(got.as_deref(), Some("H:\\测试库\\测试剧集"));
    }

    /// 方案 C 的安全边界：跨目录的剧集不该被收紧（可能是多碟/多来源，硬锁会串记录）
    #[test]
    fn anchor_tightening_skips_multi_directory() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::initialize_database(&conn).unwrap();
        conn.execute(
            "INSERT INTO series (id, name, folder_path) VALUES (1, '跨目录剧集', NULL)",
            [],
        )
        .unwrap();
        for (i, d) in ["H:\\盘A\\剧", "H:\\盘B\\剧", "H:\\盘C\\剧"].iter().enumerate() {
            for e in 1..=2 {
                conn.execute(
                    "INSERT INTO videos (title, file_path, file_name, kind, series_id, season, episode)
                     VALUES (?1, ?2, ?2, 'series', 1, 1, ?3)",
                    rusqlite::params![
                        format!("{}-{}", i, e),
                        format!("{}\\ep{}.mp4", d, e),
                        e
                    ],
                )
                .unwrap();
            }
        }
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM (
                    SELECT se.id FROM series se JOIN videos v ON v.series_id = se.id
                    WHERE se.folder_path IS NULL OR se.folder_path = ''
                    GROUP BY se.id HAVING COUNT(*) >= ?1
                       AND COUNT(DISTINCT rtrim(v.file_path, replace(v.file_path, '\\', ''))) = 1
                 )",
                rusqlite::params![MIN_ANCHOR_EPISODES as i64],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "跨目录剧集不该被收紧");
    }

    /// 同目录两剧：锚点收紧必须拒绝。
    ///
    /// 2026-11-08 实证事故形态：平铺目录下 A/B 各 5 集，两部都满足「≥5 集、单目录」，
    /// 于是拿到同一个 folder_path；下次刷新时 match_folder_anchor 同长度先到先得，
    /// B 的文件被整体改判给 A（实测 剧B 文件 → Some(剧A)），剧B 变空壳后被 delete_orphan_series 删掉。
    /// 生产逻辑读这个计数 >1 时跳过收紧，此处钉住判据本身。
    #[test]
    fn anchor_tightening_refuses_shared_directory() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::initialize_database(&conn).unwrap();
        for (id, name) in [(1, "剧A"), (2, "剧B"), (3, "剧C")] {
            conn.execute(
                "INSERT INTO series (id, name, folder_path) VALUES (?1, ?2, NULL)",
                rusqlite::params![id, name],
            )
            .unwrap();
        }
        // 剧A / 剧B 各 5 集，同处一个平铺目录
        for sid in [1, 2] {
            for e in 1..=5 {
                conn.execute(
                    "INSERT INTO videos (title, file_path, file_name, kind, series_id, season, episode)
                     VALUES (?1, ?2, ?2, 'series', ?3, 1, ?4)",
                    rusqlite::params![
                        format!("剧{}-{}", sid, e),
                        format!("E:\\平铺目录\\S{}-E{}.mp4", sid, e),
                        sid,
                        e
                    ],
                )
                .unwrap();
            }
        }
        // 剧C 独占另一个目录
        for e in 1..=5 {
            conn.execute(
                "INSERT INTO videos (title, file_path, file_name, kind, series_id, season, episode)
                 VALUES (?1, ?2, ?2, 'series', 3, 1, ?3)",
                rusqlite::params![
                    format!("剧C-{}", e),
                    format!("E:\\独立目录\\C-E{}.mp4", e),
                    e
                ],
            )
            .unwrap();
        }

        assert_eq!(
            dir_series_owners(&conn, "E:\\平铺目录").unwrap(),
            2,
            "平铺目录必须被识别为两剧共享"
        );
        assert_eq!(
            dir_series_owners(&conn, "E:\\独立目录").unwrap(),
            1,
            "独占目录应放行收紧"
        );
        // 与 match_folder_anchor 同源：大小写与尾斜杠都不影响判定（Windows 路径语义）
        assert_eq!(dir_series_owners(&conn, "e:\\平铺目录\\").unwrap(), 2);
    }
}
