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

/// 分类结果 → 系列归属处理
/// 0. 路径锚点认领：文件落在某已注册剧集（series.folder_path）目录树内 → 直接归该剧集，
///    不再走分类器（避免「添加剧集」建的剧集被刷新扫描拆散 / 新集被分类成别名剧集）
/// 1. 分类器产出的剧集名 upsert（原名逻辑）
fn apply_series(
    tx: &rusqlite::Transaction,
    info: &scanner::VideoInfo,
    folder_anchors: &[(i64, String)],
) -> Result<Option<i64>, AppError> {
    if let Some(sid) = match_folder_anchor(&info.file_path, folder_anchors) {
        return Ok(Some(sid));
    }
    if info.kind == "series" {
        if let Some(ref name) = info.series_name {
            let name = name.trim();
            if !name.is_empty() {
                let sid = crate::db::upsert_series(tx, name, None)?;
                return Ok(Some(sid));
            }
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
    // 先扫描（不持 DB 锁，避免阻塞其他查询）
    let (all_files, scan_errors) = scanner::scan_directory_reporting(&dir_path);
    let sibling_map = scanner::build_sibling_map_public(&all_files);
    let total_found = all_files.len() as i64;

    let mut conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    // 已注册剧集目录锚点（路径认领用）
    let folder_anchors = load_folder_anchors(&conn)?;
    if total_found == 0 {
        let _ = crate::db::upsert_scan_folder(&conn, &dir_path);
        return Ok(ScanResult {
            total_found: 0,
            new_added: 0,
            updated: 0,
            errors: scan_errors,
            pending: Vec::new(),
        });
    }

    // HashSet 加速已存在路径判定（避免全库线性查找）
    let existing: std::collections::HashSet<String> =
        crate::db::get_all_file_paths(&conn).map_err(|e| e.to_string())?.into_iter().collect();

    let mut new_added: i64 = 0;
    let mut updated: i64 = 0;
    let mut errors = scan_errors;
    let mut pending: Vec<PendingItem> = Vec::new();

    {
        let tx = conn
            .transaction()
            .map_err(|e| AppError::db(e.to_string()))?;

        for file in &all_files {
            // 增量模式：已存在则跳过（但移动匹配需要检查路径变化 → 仍走 upsert 的移动匹配分支）
            let path_str = file.to_string_lossy().to_string();
            if incremental && existing.contains(&path_str) {
                continue;
            }
            let info = scanner::extract_video_info(file, &sibling_map);
            let series_id = apply_series(&tx, &info, &folder_anchors)?;
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
                    default_watched,
                },
                true,
            ) {
                Ok(r) => {
                    if r.is_inserted() {
                        new_added += 1;
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

    let _ = crate::db::upsert_scan_folder(&conn, &dir_path);

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

    // 先扫描（不持 DB 锁）
    let (all_files, scan_errors) = scanner::scan_directory_reporting(&dir_path);
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
    let mut new_added: i64 = 0;
    let mut updated: i64 = 0;
    let mut errors = scan_errors;

    let mut conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    {
        let tx = conn.transaction().map_err(|e| AppError::db(e.to_string()))?;
        // 文件夹名 = 剧集名（同名并入），folder_path 记录完整路径
        let sid = crate::db::upsert_series(&tx, series_name, Some(&dir_path)).map_err(|e| e.to_string())?;

        for file in &all_files {
            let info = scanner::extract_video_info(file, &sibling_map);
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
pub async fn refresh_all_scans(db: State<'_, DbConn>, default_watched: bool) -> Result<RefreshResult, String> {
    // 短暂持锁读取订阅列表与现有路径，随即释放（扫描期间不占锁）
    let (scan_folders, existing) = {
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        let _ = crate::db::set_setting(&conn, "last_refresh_at", &now_str());
        let folders = crate::db::get_scan_folders(&conn).map_err(|e| e.to_string())?;
        let paths: std::collections::HashSet<String> =
            crate::db::get_all_file_paths(&conn).map_err(|e| e.to_string())?.into_iter().collect();
        (folders, paths)
    };

    // 第一阶段：扫描所有订阅文件夹（不持锁）
    let mut folder_jobs = Vec::new();
    let mut errors = Vec::new();
    for folder in &scan_folders {
        let (all_files, scan_errors) = scanner::scan_directory_reporting(&folder.folder_path);
        for e in scan_errors {
            errors.push(format!("[{}] {}", folder.folder_path, e));
        }
        let sibling_map = scanner::build_sibling_map_public(&all_files);
        folder_jobs.push((folder.folder_path.clone(), all_files, sibling_map));
    }
    let folders_scanned = scan_folders.len() as i64;

    // 写库（重新持锁，锁范围只覆盖 DB 操作）
    let mut conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    // 已注册剧集目录锚点（路径认领用）
    let folder_anchors = load_folder_anchors(&conn)?;
    let mut new_added: i64 = 0;
    let mut moved: i64 = 0;
    let mut pending: Vec<PendingItem> = Vec::new();
    for (folder_path, all_files, sibling_map) in &folder_jobs {
        {
            let tx = conn
                .transaction()
                .map_err(|e| AppError::db(e.to_string()))?;
            for file in all_files {
                let path_str = file.to_string_lossy().to_string();
                if existing.contains(&path_str) {
                    continue;
                }
                let info = scanner::extract_video_info(file, sibling_map);
                let series_id = apply_series(&tx, &info, &folder_anchors)?;
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
                        default_watched,
                    },
                    true,
                ) {
                    Ok(r) => {
                        if r.is_inserted() {
                            new_added += 1;
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

    // 第二阶段：清理磁盘上已不存在的记录
    let mut removed: i64 = 0;
    let videos: Vec<(i64, String)> = {
        let mut stmt = conn
            .prepare("SELECT id, file_path FROM videos")
            .map_err(|e| AppError::db(e.to_string()))?;
        let mut out = Vec::new();
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| AppError::db(e.to_string()))?;
        for r in rows {
            if let Ok(v) = r {
                out.push(v);
            }
        }
        out
    };
    {
        let tx = conn
            .transaction()
            .map_err(|e| AppError::db(e.to_string()))?;
        for (id, file_path) in &videos {
            if !Path::new(file_path).exists() {
                match tx.execute("DELETE FROM videos WHERE id = ?1", rusqlite::params![id]) {
                    Ok(_) => removed += 1,
                    Err(e) => errors.push(format!("删除记录失败 ({}): {}", file_path, e)),
                }
            }
        }
        tx.commit().map_err(|e| AppError::db(e.to_string()))?;
    }

    Ok(RefreshResult {
        removed,
        new_added,
        moved,
        folders_scanned,
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
    favorites_only: Option<bool>,
    min_rating: Option<f64>,
    sort: Option<String>,
    watch_status: Option<String>,
) -> Result<PaginatedVideos, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    let page = page.unwrap_or(0);
    let page_size = page_size.unwrap_or(60);
    let min_rating = min_rating.unwrap_or(0.0);
    let sort = sort.as_deref().unwrap_or("last_watched");
    let kind = kind.filter(|k| !k.is_empty() && k != "all");
    let favorites = favorites_only == Some(true);
    let status = watch_status.as_deref().unwrap_or("all");

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
    if favorites {
        conditions.push("v.is_favorite = 1".to_string());
    }
    for tag_id in tag_ids.unwrap_or_default() {
        let idx = params.len() + 1;
        conditions.push(format!(
            "EXISTS (SELECT 1 FROM video_tags vt_{0} WHERE vt_{0}.video_id = v.id AND vt_{0}.tag_id = ?{0})",
            idx
        ));
        params.push(Box::new(tag_id));
    }
    match status {
        "unwatched" => conditions.push("v.watch_progress = 0".to_string()),
        "partial" => conditions.push("v.watch_progress > 0 AND v.watch_progress < 100".to_string()),
        "watched" => conditions.push("v.watch_progress >= 100".to_string()),
        _ => {}
    }

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    crate::db::query_videos_paginated(&conn, &conditions, &param_refs, page, page_size, min_rating, sort)
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
        page.unwrap_or(0),
        page_size.unwrap_or(60),
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
    crate::db::get_history_entries(&conn, limit.unwrap_or(60)).map_err(|e| e.to_string())
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

#[tauri::command]
pub fn toggle_favorite(db: State<'_, DbConn>, video_id: i64) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::toggle_favorite(&conn, video_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn toggle_series_favorite(db: State<'_, DbConn>, series_id: i64) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    conn.execute(
        "UPDATE series SET is_favorite = 1 - is_favorite, updated_at = datetime('now','localtime') WHERE id = ?1",
        rusqlite::params![series_id],
    )
    .map_err(|e| AppError::db(e.to_string()))?;
    let fav: i32 = conn
        .query_row(
            "SELECT is_favorite FROM series WHERE id = ?1",
            rusqlite::params![series_id],
            |r| r.get(0),
        )
        .map_err(|e| AppError::db(e.to_string()))?;
    Ok(fav != 0)
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
/// **历史**（2026-08-30 起改走 `cmd /c start`）：
/// - v1：直接 `open::that()`，Windows 上经 PowerShell 派发。冷启动 ~130ms，
///   且 PS 的 `$PWD` 是 `C:\WINDOWS\system32`，新进程继承它会让 PotPlayer
///   启动卡一会（要枚举工作目录生成「打开文件」对话框，4749 个文件拖慢初始化）。
/// - v2：手写 ShellExecuteExW + 独立线程 + STA CoInit。零中间进程、显式 `lpDirectory`。
///   但 2026-08-30 实测在新版 exe 上偶发让 PotPlayer 在 `nvwgf2umx.dll`
///   0xC0000005（NVIDIA D3D 用户模式驱动）崩溃；同一文件直接双击 OK，
///   即差异来自 CineVault 的启动方式。
/// - **v3（当前）**：走 `cmd /c start "" "<path>"`，再以 `current_dir(<dir>)`
///   控制新进程工作目录。cmd 冷启动 ~19ms 比 PS 快一个量级；启动语义跟
///   资源管理器双击一致（新进程 CWD = 文件所在目录）；不依赖 COM 状态、
///   不开新 OS 线程、不写任何 FFI，启动最简——把已知崩溃路径完全绕开。
#[cfg(target_os = "windows")]
fn shell_open(file: &Path, directory: Option<&Path>) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    // GUI 程序是 windows_subsystem="windows"，必须给子进程挂这个标志，
    // 否则 cmd 自己起一扇控制台黑窗（哪怕转瞬即逝也闪眼）。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // 工作目录：调用方传则用，否则取文件父目录（双击时 Shell 给的也是这个）。
    let cwd = directory
        .or_else(|| file.parent())
        .unwrap_or_else(|| Path::new("."));
    let display = file.to_string_lossy().to_string();

    // `start "" "<path>"`：第一个引号串是窗口标题（空串占位，否则 cmd 会把路径当标题）；
    // 第二个是要启动的文件 / 文件夹，由 `start` 内部 ShellExecute 派发。
    // **路径经环境变量传递**（`%CINEVAULT_OPEN_TARGET%`），不直接拼命令行：
    // cmd 解析命令行时会展开 %VAR%、把 & 当分隔符、吞 ^ 转义——路径里若出现
    // 这类字符（如 `100%全集.mkv`）直接拼进去会错乱。环境变量展开只发生在
    // 这一次 %CINEVAULT_OPEN_TARGET% 上，展开结果不会被二次解析，路径原样到达 start。
    // 与 open crate 用 $env:OPEN_RS_TARGET 的思路一致。单次 .arg() 而非 .args([...]) 数组：
    // 数组里 &str 和 &OsStr 混着塞会触发类型不匹配。
    // 必须用 .raw_arg() 加空格 + 引号：.arg() 遇到无空格的 "%CINEVAULT_OPEN_TARGET%"
    // 不会自动加引号，env 展开后路径里的空格会把 start 当多参数解析，导致
    // "H:\...\Arcane S02E05..." 被截断成 "H:\...\Arcane"（实测 2026-08-30）。
    let status = Command::new("cmd")
        .env("CINEVAULT_OPEN_TARGET", file.as_os_str())
        .arg("/c")
        .arg("start")
        .arg("")
        .raw_arg(" \"%CINEVAULT_OPEN_TARGET%\"")
        .current_dir(cwd)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("启动失败: {} ({})", display, e))?;

    if !status.success() {
        return Err(format!(
            "启动失败: {} (cmd exit {})",
            display,
            status.code().unwrap_or(-1)
        ));
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
        // 工作目录设为文件所在目录 —— 与资源管理器双击的行为一致
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
    match crate::media_info::resolve_ffprobe_cached() {
        Some(p) => FfprobeStatus { available: true, path: Some(p.to_string()) },
        None => FfprobeStatus { available: false, path: None },
    }
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
pub fn delete_video(db: State<'_, DbConn>, video_id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::delete_video(&conn, video_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_series(db: State<'_, DbConn>, series_id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::delete_series(&conn, series_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_video_with_file(db: State<'_, DbConn>, video_id: i64) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    let file_path: String = conn
        .query_row(
            "SELECT file_path FROM videos WHERE id = ?1",
            rusqlite::params![video_id],
            |r| r.get(0),
        )
        .map_err(|_| AppError::db("视频不存在"))?;
    // 先删文件再删记录：文件删除失败时保留记录，评分/标签/进度不丢
    match std::fs::remove_file(&file_path) {
        Ok(()) => {
            crate::db::delete_video(&conn, video_id).map_err(|e| e.to_string())?;
            Ok(format!("已删除文件: {}", file_path))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            crate::db::delete_video(&conn, video_id).map_err(|e| e.to_string())?;
            Ok(format!("记录已删除，文件不存在: {}", file_path))
        }
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

/// B-1：async 化，理由同 export_json——导入是批量写 + 全量校验，放 blocking 池。
#[tauri::command]
pub async fn import_json(app: tauri::AppHandle, json_str: String) -> Result<ImportResult, String> {
    const MAX_IMPORT_SIZE: usize = 50_000_000;
    if json_str.len() > MAX_IMPORT_SIZE {
        return Err(format!("导入数据过大（{} 字节），最大支持 50MB", json_str.len()));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let data: ExportData = serde_json::from_str(&json_str)
            .map_err(|e| AppError::validation(format!("JSON 解析失败: {}", e)))?;
        let db = app.state::<DbConn>();
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        crate::db::import_data(&conn, &data).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("导入失败: {e}"))?
}

/// 读取文本文件（配合导入：前端选文件后读取内容；仅限 .json 备份）
#[tauri::command]
pub fn read_text_file(file_path: String) -> Result<String, String> {
    const MAX_JSON_SIZE: u64 = 50_000_000;
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
}

/// 写文本文件（配合导出：前端选保存位置后写入 JSON；仅限 .json）
#[tauri::command]
pub fn write_text_file(file_path: String, content: String) -> Result<(), String> {
    const MAX_JSON_SIZE: usize = 50_000_000;
    if !file_path.to_lowercase().ends_with(".json") {
        return Err("仅支持写入 .json 文件".to_string());
    }
    if content.len() > MAX_JSON_SIZE {
        return Err("内容过大（超过 50MB）".to_string());
    }
    let p = Path::new(&file_path);
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
        }
    }
    std::fs::write(&file_path, content).map_err(|e| format!("写入文件失败: {}", e))
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
    use super::match_folder_anchor;

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
}
