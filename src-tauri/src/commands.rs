use std::collections::HashMap;
use std::path::Path;
use tauri::State;

use crate::db::DbConn;
use crate::models::*;
use crate::scanner;
use crate::thumbnail;

fn now_str() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 分类结果 → 系列归属处理
fn apply_series(
    tx: &rusqlite::Transaction,
    info: &scanner::VideoInfo,
) -> Result<Option<i64>, AppError> {
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
    let mut conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;

    // 先全量遍历构建同级信息（增量也需遍历目录才能分析目录结构）
    let (all_files, scan_errors) = scanner::scan_directory_reporting(&dir_path);
    let sibling_map = scanner::build_sibling_map_public(&all_files);

    let existing = crate::db::get_all_file_paths(&conn).map_err(|e| e.to_string())?;

    let total_found = all_files.len() as i64;
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
            let exists = if incremental { existing.contains(&path_str.to_string()) } else { false };
            if incremental && exists {
                continue;
            }
            let info = scanner::extract_video_info(file, &sibling_map);
            let series_id = apply_series(&tx, &info)?;
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

    let mut conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
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
                    }
                    // 强制归集兜底：移动匹配分支可能保留旧 series_id / 旧 user_kind，一律改挂本剧集
                    tx.execute(
                        "UPDATE videos SET series_id = ?1, kind = 'series', user_kind = 'series', updated_at = datetime('now','localtime') WHERE id = ?2",
                        rusqlite::params![sid, id],
                    )
                    .map_err(|e| AppError::db(e.to_string()))?;
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
    let mut conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    let _ = crate::db::set_setting(&conn, "last_refresh_at", &now_str());
    let scan_folders = crate::db::get_scan_folders(&conn).map_err(|e| e.to_string())?;

    let existing_paths = crate::db::get_all_file_paths(&conn).map_err(|e| e.to_string())?;

    let mut new_added: i64 = 0;
    let mut moved: i64 = 0;
    let mut errors = Vec::new();
    let mut pending: Vec<PendingItem> = Vec::new();
    let folders_scanned = scan_folders.len() as i64;

    // 第一阶段：扫描所有已订阅文件夹
    for folder in &scan_folders {
        let (all_files, scan_errors) = scanner::scan_directory_reporting(&folder.folder_path);
        for e in scan_errors {
            errors.push(format!("[{}] {}", folder.folder_path, e));
        }
        let sibling_map = scanner::build_sibling_map_public(&all_files);
        {
            let tx = conn
                .transaction()
                .map_err(|e| AppError::db(e.to_string()))?;
            for file in &all_files {
                let path_str = file.to_string_lossy().to_string();
                if existing_paths.contains(&path_str.to_string()) {
                    continue;
                }
                let info = scanner::extract_video_info(file, &sibling_map);
                let series_id = apply_series(&tx, &info)?;
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
        if let Err(e) = crate::db::upsert_scan_folder(&conn, &folder.folder_path) {
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

#[tauri::command]
pub fn get_history(db: State<'_, DbConn>, limit: Option<i64>) -> Result<Vec<WatchHistory>, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_watch_history(&conn, limit.unwrap_or(60)).map_err(|e| e.to_string())
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

/// 解析 PotPlayer 播放列表 dpl，把最近播放位置回填到 watch_progress
/// 只升不降（避免覆盖已看 / 手动进度）；不碰 last_watched_at
#[tauri::command]
pub fn sync_progress_from_player(db: State<'_, DbConn>) -> Result<i64, String> {
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    let dpl = Path::new(&appdata)
        .join("PotPlayerMini64")
        .join("Playlist")
        .join("PotPlayerMini64.dpl");
    if !dpl.exists() {
        return Ok(0);
    }
    let content = std::fs::read_to_string(&dpl).map_err(|e| e.to_string())?;

    // 聚合列表项：N*file*path / N*start*ms / N*duration2*ms
    #[derive(Default)]
    struct Entry {
        file: Option<String>,
        start_ms: Option<u64>,
        duration_ms: Option<u64>,
    }
    let mut entries: HashMap<u32, Entry> = HashMap::new();
    for line in content.lines() {
        let Some(star) = line.find('*') else { continue };
        let Ok(idx) = line[..star].parse::<u32>() else { continue };
        let rest = &line[star + 1..];
        let e = entries.entry(idx).or_default();
        if let Some(p) = rest.strip_prefix("file*") {
            e.file = Some(p.to_string());
        } else if let Some(p) = rest.strip_prefix("start*") {
            e.start_ms = p.trim().parse().ok();
        } else if let Some(p) = rest.strip_prefix("duration2*") {
            e.duration_ms = p.trim().parse().ok();
        }
    }

    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    let mut updated: i64 = 0;
    for e in entries.values() {
        let (Some(path), Some(start), Some(dur)) = (&e.file, e.start_ms, e.duration_ms) else {
            continue;
        };
        if start == 0 || dur == 0 {
            continue;
        }
        let progress = ((start as f64) / (dur as f64) * 100.0).clamp(0.0, 100.0);
        let n = conn
            .execute(
                "UPDATE videos SET watch_progress = ?1, updated_at = datetime('now','localtime')
                 WHERE file_path = ?2 COLLATE NOCASE AND watch_progress < ?1",
                rusqlite::params![progress, path],
            )
            .map_err(|e| e.to_string())?;
        updated += n as i64;
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
    crate::db::delete_video(&conn, video_id).map_err(|e| e.to_string())?;
    match std::fs::remove_file(&file_path) {
        Ok(()) => Ok(format!("已删除文件: {}", file_path)),
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                Ok(format!("记录已删除，文件不存在: {}", file_path))
            } else {
                Err(format!("记录已删除，但文件删除失败: {}", e))
            }
        }
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
pub fn check_migration(db: State<'_, DbConn>) -> Result<Option<String>, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    let _ = crate::migrator::already_migrated(&conn).map_err(|e| e.to_string())?;
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

/// 读取文本文件（配合导入：前端选文件后读取内容）
#[tauri::command]
pub fn read_text_file(file_path: String) -> Result<String, String> {
    let p = Path::new(&file_path);
    if !p.exists() || !p.is_file() {
        return Err(format!("文件不存在: {}", file_path));
    }
    std::fs::read_to_string(&file_path).map_err(|e| format!("读取文件失败: {}", e))
}

/// 写文本文件（配合导出：前端选保存位置后写入 JSON）
#[tauri::command]
pub fn write_text_file(file_path: String, content: String) -> Result<(), String> {
    let p = Path::new(&file_path);
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
        }
    }
    std::fs::write(&file_path, content).map_err(|e| format!("写入文件失败: {}", e))
}

// ============================================
// 缩略图（截帧兜底，暂不接入 UI）
// ============================================
#[tauri::command]
pub fn make_thumbnail(db: State<'_, DbConn>, video_id: i64, video_path: String, thumb_dir: String) -> Result<Option<String>, String> {
    let result = thumbnail::generate_thumbnail(&video_path, &thumb_dir);
    if let Some(ref path) = result {
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        conn.execute(
            "UPDATE videos SET thumbnail_path = ?1 WHERE id = ?2",
            rusqlite::params![path, video_id],
        )
        .map_err(|e| AppError::db(e.to_string()))?;
    }
    Ok(result)
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
