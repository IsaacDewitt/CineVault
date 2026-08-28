use std::collections::HashMap;
use std::path::Path;
use rusqlite::{params, OptionalExtension};
use tauri::State;

use crate::db::DbConn;
use crate::models::*;
use crate::scanner;

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
fn match_folder_anchor(file_path: &str, anchors: &[(i64, String)]) -> Option<i64> {
    let norm = |p: &str| p.trim_end_matches(|c| c == '/' || c == '\\').to_lowercase();
    let fp = norm(file_path);
    // 用字节前缀比较，不要写 fp[..f.len()] == f：锚点路径含 2 字节 UTF-8 字符时，
    // 切片位置可能落在多字节字符中间，直接 panic。
    let fp_bytes = fp.as_bytes();
    let mut best: Option<(usize, i64)> = None;
    for (sid, folder) in anchors {
        let f = norm(folder);
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

/// 读取所有已注册剧集目录锚点（folder_path 非空）
fn load_folder_anchors(conn: &rusqlite::Connection) -> Result<Vec<(i64, String)>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, folder_path FROM series WHERE folder_path IS NOT NULL AND folder_path != ''",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let mut v = Vec::new();
    for r in rows {
        if let Ok(x) = r {
            v.push(x);
        }
    }
    Ok(v)
}

// ============================================
// 扫描
// ============================================
#[tauri::command]
pub fn scan_folder(
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
                    duration: None,
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
pub fn add_series_from_folder(
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
                    duration: None,
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
pub fn refresh_all_scans(db: State<'_, DbConn>, default_watched: bool) -> Result<RefreshResult, String> {
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
                        duration: None,
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

#[tauri::command]
pub fn update_progress(db: State<'_, DbConn>, video_id: i64, progress: f64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::update_watch_progress(&conn, video_id, progress).map_err(|e| e.to_string())
}

/// 点击播放即标记"最近观看"：刷新 last_watched_at + 写一条观看历史（不动进度百分比）。
/// 前端 play() 打开播放器前调用，这是最可靠的"看过"记录时机。
#[tauri::command]
pub fn mark_played(db: State<'_, DbConn>, video_id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::mark_played(&conn, video_id).map_err(|e| e.to_string())
}

// ============================================
// 文件操作
// ============================================
#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.exists() || !p.is_file() {
        return Err(format!("文件不存在: {}", path));
    }
    open::that(&path).map_err(|e| e.to_string())
}

/// 从 PotPlayer 回填播放记录。
/// 数据源（按可靠性）：
///   1. 注册表 HKCU\Software\Daum\PotPlayerMini64\RememberFiles —— PotPlayer 位置记忆（权威）
///      格式：<序号> REG_SZ <位置毫秒>=<文件路径>，如 "295 REG_SZ 649035=E:\...\Trial...mkv"
///   2. dpl %APPDATA%\PotPlayerMini64\Playlist\PotPlayerMini64.dpl 的 playname=（最近打开的文件）
///      以及条目里的 start*/duration2*（PotPlayer 播放中会写位置时）
/// 增量语义：把上次见过的 {path: 位置} 快照存 settings，只有"新出现 / 位置变化 / playname 切换"
/// 的文件才刷新最近播放 + 写观看历史，避免每次同步把所有历史播放全刷成"现在"。
/// 进度只升不降；无时长数据时只记"看过"（最近播放/历史），百分比待扫描补 duration 后自动生效。
#[tauri::command]
pub fn sync_progress_from_player(db: State<'_, DbConn>) -> Result<i64, String> {
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

    // ========== 2. 增量检测：只处理新出现 / 位置变化 / playname 切换 ==========
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    let last_raw: Option<String> = crate::db::get_setting(&conn, "potplayer_evidence_snapshot")
        .map_err(|e| e.to_string())?;
    let parsed_snapshot = last_raw
        .as_deref()
        .and_then(|raw| serde_json::from_str::<HashMap<String, u64>>(raw).ok());
    // 快照从未建立（或损坏）→ 首轮同步：只建立基线，不写任何库。
    // 否则 RememberFiles 里全部库内文件会被当成"新播放"批量刷新，
    // 把历史最近播放全刷成同一时刻（2026-08-28 事故根因）。
    let is_first_sync = parsed_snapshot.is_none();
    let last_map = parsed_snapshot.unwrap_or_default();
    let last_playname: Option<String> =
        crate::db::get_setting(&conn, "potplayer_dpl_playname").map_err(|e| e.to_string())?;
    let playname_switched = last_playname.as_deref() != playname.as_deref();

    let now = now_str();
    let mut new_snapshot: HashMap<String, u64> = HashMap::new();
    let mut updated: i64 = 0;

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
        // 匹配库内视频（无 file_path 索引时全表扫描，5000 行量级可接受）
        let row = conn
            .query_row(
                "SELECT id, watch_progress FROM videos WHERE file_path = ?1 COLLATE NOCASE",
                params![path],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((vid, cur_progress)) = row else {
            continue; // 库外文件（PotPlayer 里看了但不在库里），跳过
        };
        // 进度只升不降；有位置+时长才算百分比，否则维持现值（至少记"看过"）
        let mut final_progress = cur_progress;
        if let (Some(p), Some(d)) = (*pos, *dur) {
            if p > 0 && d > 0 {
                final_progress = ((p as f64) / (d as f64) * 100.0).clamp(0.0, 100.0);
            }
        }
        conn.execute(
            "UPDATE videos SET watch_progress = MAX(watch_progress, ?1), last_watched_at = ?2, updated_at = datetime('now','localtime') WHERE id = ?3",
            params![final_progress, now, vid],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR IGNORE INTO watch_history (video_id, watched_at, progress) VALUES (?1, ?2, ?3)",
            params![vid, now, final_progress],
        )
        .map_err(|e| e.to_string())?;
        crate::db::trim_history(&conn, vid).map_err(|e| e.to_string())?;
        crate::db::sync_series_aggregate(&conn, vid).map_err(|e| e.to_string())?;
        updated += 1;
    }

    // 快照写回（含库外文件：位置变化是纯外部事实，与是否入库无关）
    if let Ok(snap_json) = serde_json::to_string(&new_snapshot) {
        let _ = crate::db::set_setting(&conn, "potplayer_evidence_snapshot", &snap_json);
    }
    if let Some(p) = playname {
        let _ = crate::db::set_setting(&conn, "potplayer_dpl_playname", &p);
    }
    Ok(updated)
}

#[tauri::command]
pub fn open_folder(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    let target = if p.is_dir() {
        path.clone()
    } else {
        p.parent().map(|par| par.to_string_lossy().to_string()).unwrap_or_else(|| path.clone())
    };
    if !Path::new(&target).exists() {
        return Err(format!("文件夹不存在: {}", target));
    }
    open::that(&target).map_err(|e| e.to_string())
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
pub fn migrate_from_old(db: State<'_, DbConn>, old_path: Option<String>) -> Result<MigrationResult, String> {
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
#[tauri::command]
pub fn export_json(db: State<'_, DbConn>) -> Result<ExportData, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::export_data(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn import_json(db: State<'_, DbConn>, json_str: String) -> Result<ImportResult, String> {
    const MAX_IMPORT_SIZE: usize = 50_000_000;
    if json_str.len() > MAX_IMPORT_SIZE {
        return Err(format!("导入数据过大（{} 字节），最大支持 50MB", json_str.len()));
    }
    let data: ExportData = serde_json::from_str(&json_str)
        .map_err(|e| AppError::validation(format!("JSON 解析失败: {}", e)))?;
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::import_data(&conn, &data).map_err(|e| e.to_string())
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

#[tauri::command]
pub fn get_drives() -> Result<Vec<DriveInfo>, String> {
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

#[cfg(test)]
mod tests {
    use super::match_folder_anchor;

    fn anchors() -> Vec<(i64, String)> {
        vec![
            (1, "H:\\待看的剧".to_string()),
            (2, "H:\\待看的剧\\绿灯侠".to_string()),
            (3, "E:\\动漫风车".to_string()),
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
