use tauri::State;
use std::path::Path;

use crate::db::DbConn;
use crate::models::*;
use crate::scanner;
use crate::thumbnail;

/// 扫描目录
/// #1: 修复 updated 计数器
/// #2: 修复空扫描 panic
/// #3: 修复 folder_path 逻辑
#[tauri::command]
pub fn scan_videos(
    db: State<'_, DbConn>,
    dir_path: String,
    incremental: bool,
    default_watched: bool,
) -> Result<ScanResult, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;

    // 清空目录缓存，确保每次扫描读到最新文件系统状态
    scanner::clear_dir_cache();

    let video_files = if incremental {
        let mut stmt = conn
            .prepare("SELECT file_path FROM videos")
            .map_err(|e| AppError::db(e.to_string()))?;
        let existing: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| AppError::db(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();
        scanner::scan_directory_incremental(&dir_path, &existing)
    } else {
        scanner::scan_directory(&dir_path)
    };

    let total_found = video_files.len() as i64;

    // #2: 空扫描安全处理
    if total_found == 0 {
        return Ok(ScanResult {
            total_found: 0,
            new_added: 0,
            updated: 0,
            errors: Vec::new(),
            series_detected: Vec::new(),
        });
    }

    let mut new_added: i64 = 0;
    let mut updated: i64 = 0;
    let mut errors = Vec::new();
    // #3: 用 HashMap 记录每个 series 的实际文件夹路径和集数
    let mut series_map: std::collections::HashMap<String, (String, i64)> =
        std::collections::HashMap::new();

    // #26: 使用事务批量插入
    conn.execute_batch("BEGIN").map_err(|e| AppError::db(e.to_string()))?;

    for file in &video_files {
        let info = scanner::extract_video_info(file);

        let vtype = if info.series_name.is_some() { "episode" } else { "movie" };
        match crate::db::upsert_video(
            &conn,
            &info.title,
            &info.file_path,
            &info.file_name,
            info.file_size,
            vtype,
            info.series_name.as_deref(),
            info.season,
            info.episode,
            default_watched,
        ) {
            Ok(result) => {
                // #1: 区分插入和更新
                if result.is_inserted() {
                    new_added += 1;
                } else {
                    updated += 1;
                }
                // #3: 记录 series 的实际文件夹路径
                if let Some(ref series) = info.series_name {
                    let entry = series_map
                        .entry(series.clone())
                        .or_insert_with(|| {
                            let folder = Path::new(&info.file_path)
                                .parent()
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_default();
                            (folder, 0)
                        });
                    entry.1 += 1;
                }
            }
            Err(e) => {
                errors.push(format!("{}: {}", info.file_path, e));
            }
        }
    }

    conn.execute_batch("COMMIT").map_err(|e| AppError::db(e.to_string()))?;

    let series_detected: Vec<SeriesInfo> = series_map
        .into_iter()
        .map(|(name, (folder, count))| SeriesInfo {
            name,
            episode_count: count,
            folder_path: folder,
        })
        .collect();

    Ok(ScanResult {
        total_found,
        new_added,
        updated,
        errors,
        series_detected,
    })
}

/// 获取所有视频（分页）
#[tauri::command]
pub fn get_videos(db: State<'_, DbConn>, page: Option<i64>, page_size: Option<i64>) -> Result<PaginatedVideos, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_all_videos(&conn, page.unwrap_or(0), page_size.unwrap_or(50)).map_err(|e| e.to_string())
}

/// 搜索视频（分页）
#[tauri::command]
pub fn search_videos(db: State<'_, DbConn>, query: String, page: Option<i64>, page_size: Option<i64>) -> Result<PaginatedVideos, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::search_videos(&conn, &query, page.unwrap_or(0), page_size.unwrap_or(50)).map_err(|e| e.to_string())
}

/// 按标签筛选（分页）
#[tauri::command]
pub fn get_videos_by_tag(db: State<'_, DbConn>, tag_id: i64, page: Option<i64>, page_size: Option<i64>) -> Result<PaginatedVideos, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_videos_by_tag(&conn, tag_id, page.unwrap_or(0), page_size.unwrap_or(50)).map_err(|e| e.to_string())
}

/// 多标签筛选（AND 逻辑：视频必须同时拥有所有指定标签）
/// video_type: 可选，限定视频类型（"movie" / "episode"）
#[tauri::command]
pub fn get_videos_by_tags(db: State<'_, DbConn>, tag_ids: Vec<i64>, video_type: Option<String>, page: Option<i64>, page_size: Option<i64>) -> Result<PaginatedVideos, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    if tag_ids.is_empty() && video_type.is_none() {
        return crate::db::get_all_videos(&conn, page.unwrap_or(0), page_size.unwrap_or(50)).map_err(|e| e.to_string());
    }
    // 构建 WHERE EXISTS 子查询：视频必须拥有所有指定标签
    let mut conditions = Vec::new();
    for (i, _tag_id) in tag_ids.iter().enumerate() {
        conditions.push(format!("EXISTS (SELECT 1 FROM video_tags vt_{0} WHERE vt_{0}.video_id = v.id AND vt_{0}.tag_id = ?{0})", i + 1));
    }
    // 可选：限定视频类型
    if let Some(ref vtype) = video_type {
        conditions.push(format!("v.video_type = '{}'", vtype));
    }
    let where_clause = if conditions.is_empty() { String::new() } else { format!("WHERE {}", conditions.join(" AND ")) };
    let params: Vec<Box<dyn rusqlite::types::ToSql>> = tag_ids.iter().map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>).collect();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    crate::db::query_videos_paginated(&conn, &where_clause, &param_refs, page.unwrap_or(0), page_size.unwrap_or(50)).map_err(|e| e.to_string())
}

/// 获取收藏视频（分页）
#[tauri::command]
pub fn get_favorites(db: State<'_, DbConn>, page: Option<i64>, page_size: Option<i64>) -> Result<PaginatedVideos, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_favorite_videos(&conn, page.unwrap_or(0), page_size.unwrap_or(50)).map_err(|e| e.to_string())
}

/// 获取剧集列表（分页）
#[tauri::command]
pub fn get_series(db: State<'_, DbConn>, page: Option<i64>, page_size: Option<i64>) -> Result<PaginatedVideos, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_series_list(&conn, page.unwrap_or(0), page_size.unwrap_or(50)).map_err(|e| e.to_string())
}

/// 更新评分 (#21: 验证已在 db.rs 中)
#[tauri::command]
pub fn set_rating(db: State<'_, DbConn>, video_id: i64, rating: f64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::update_rating(&conn, video_id, rating).map_err(|e| e.to_string())
}

/// 切换收藏
#[tauri::command]
pub fn toggle_favorite(db: State<'_, DbConn>, video_id: i64) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::toggle_favorite(&conn, video_id).map_err(|e| e.to_string())
}

/// 更新播放进度 (#22: 验证已在 db.rs 中)
#[tauri::command]
pub fn update_progress(db: State<'_, DbConn>, video_id: i64, progress: f64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::update_watch_progress(&conn, video_id, progress).map_err(|e| e.to_string())
}

/// 获取播放历史
#[tauri::command]
pub fn get_history(db: State<'_, DbConn>, limit: Option<i64>) -> Result<Vec<WatchHistory>, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_watch_history(&conn, limit.unwrap_or(50)).map_err(|e| e.to_string())
}

/// 获取所有标签
#[tauri::command]
pub fn get_tags(db: State<'_, DbConn>) -> Result<Vec<Tag>, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_all_tags(&conn).map_err(|e| e.to_string())
}

/// 创建标签
#[tauri::command]
pub fn create_tag(db: State<'_, DbConn>, name: String, color: String) -> Result<Tag, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::create_tag(&conn, &name, &color).map_err(|e| e.to_string())
}

/// 删除标签
#[tauri::command]
pub fn delete_tag(db: State<'_, DbConn>, tag_id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::delete_tag(&conn, tag_id).map_err(|e| e.to_string())
}

/// 更新标签（名称和/或颜色），保留关联
#[tauri::command]
pub fn update_tag(db: State<'_, DbConn>, tag_id: i64, name: Option<String>, color: Option<String>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::update_tag(&conn, tag_id, name.as_deref(), color.as_deref()).map_err(|e| e.to_string())
}

/// 给视频添加标签
#[tauri::command]
pub fn add_video_tag(db: State<'_, DbConn>, video_id: i64, tag_id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::add_tag_to_video(&conn, video_id, tag_id).map_err(|e| e.to_string())
}

/// 移除视频标签
#[tauri::command]
pub fn remove_video_tag(db: State<'_, DbConn>, video_id: i64, tag_id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::remove_tag_from_video(&conn, video_id, tag_id).map_err(|e| e.to_string())
}

/// 批量更新标签排序
#[tauri::command]
pub fn update_tag_orders(db: State<'_, DbConn>, orders: Vec<(i64, i64)>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::update_tag_orders(&conn, &orders).map_err(|e| e.to_string())
}

/// 生成缩略图
#[tauri::command]
pub fn make_thumbnail(
    db: State<'_, DbConn>,
    video_id: i64,
    video_path: String,
    thumb_dir: String,
) -> Result<Option<String>, String> {
    let result = thumbnail::generate_thumbnail(&video_path, &thumb_dir);
    if let Some(ref path) = result {
        let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
        crate::db::update_thumbnail(&conn, video_id, path).map_err(|e| e.to_string())?;
    }
    Ok(result)
}

/// 打开文件（用系统默认播放器）
#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    open::that(&path).map_err(|e| e.to_string())
}

/// 打开文件所在文件夹
#[tauri::command]
pub fn open_folder(path: String) -> Result<(), String> {
    let parent = Path::new(&path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or(path);
    open::that(&parent).map_err(|e| e.to_string())
}

/// 获取统计信息
#[tauri::command]
pub fn get_stats(db: State<'_, DbConn>) -> Result<VideoStats, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_stats(&conn).map_err(|e| e.to_string())
}

/// 导入数据
#[tauri::command]
pub fn import_json(db: State<'_, DbConn>, json_str: String) -> Result<ImportResult, String> {
    let data: ExportData = serde_json::from_str(&json_str)
        .map_err(|e| AppError::validation(format!("JSON 解析失败: {}", e)))?;
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    conn.execute_batch("BEGIN").map_err(|e| AppError::db(e.to_string()))?;

    let mut imported = 0i64;
    let mut skipped = 0i64;

    for vwt in &data.videos {
        let v = &vwt.video;
        let result = crate::db::upsert_video(
            &conn,
            &v.title,
            &v.file_path,
            &v.file_name,
            v.file_size,
            &v.video_type,
            v.series_name.as_deref(),
            v.season,
            v.episode,
            false, // 导入时保持原样，标签单独处理
        );
        match result {
            Ok(r) => {
                let vid = r.id();
                // 恢复评分和收藏
                if v.rating > 0.0 {
                    let _ = crate::db::update_rating(&conn, vid, v.rating);
                }
                if v.is_favorite {
                    let _ = conn.execute(
                        "UPDATE videos SET is_favorite = 1 WHERE id = ?1",
                        rusqlite::params![vid],
                    );
                }
                // 恢复标签
                for tag in &vwt.tags {
                    // 确保标签存在
                    let existing_tag: Result<i64, _> = conn.query_row(
                        "SELECT id FROM tags WHERE name = ?1",
                        rusqlite::params![tag.name],
                        |r| r.get(0),
                    );
                    let tag_id = match existing_tag {
                        Ok(id) => id,
                        Err(_) => {
                            conn.execute(
                                "INSERT INTO tags (name, color) VALUES (?1, ?2)",
                                rusqlite::params![tag.name, tag.color],
                            ).map_err(|e| AppError::db(e.to_string()))?;
                            conn.last_insert_rowid()
                        }
                    };
                    let _ = crate::db::add_tag_to_video(&conn, vid, tag_id);
                }
                imported += 1;
            }
            Err(_) => skipped += 1,
        }
    }

    conn.execute_batch("COMMIT").map_err(|e| AppError::db(e.to_string()))?;
    Ok(ImportResult { imported, skipped })
}

/// 导出数据
#[tauri::command]
pub fn export_json(db: State<'_, DbConn>) -> Result<ExportData, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::export_data(&conn).map_err(|e| e.to_string())
}

/// 读取文本文件内容
#[tauri::command]
pub fn read_text_file(file_path: String) -> Result<String, String> {
    std::fs::read_to_string(&file_path).map_err(|e| format!("读取文件失败: {}", e))
}

/// 切换已看过/未看过状态
#[tauri::command]
pub fn toggle_watched(db: State<'_, DbConn>, video_id: i64) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;

    // 查找两个标签的 ID
    let watched_id: Option<i64> = conn.query_row(
        "SELECT id FROM tags WHERE name = '已看过'", [], |r| r.get(0)
    ).ok();
    let unwatched_id: Option<i64> = conn.query_row(
        "SELECT id FROM tags WHERE name = '未看过'", [], |r| r.get(0)
    ).ok();

    // 检查当前状态
    let has_watched = if let Some(wid) = watched_id {
        conn.query_row(
            "SELECT COUNT(*) FROM video_tags WHERE video_id = ?1 AND tag_id = ?2",
            rusqlite::params![video_id, wid],
            |r| r.get::<_, i64>(0),
        ).unwrap_or(0) > 0
    } else { false };

    if has_watched {
        // 当前是已看过 → 切换为未看过
        if let Some(wid) = watched_id {
            let _ = conn.execute("DELETE FROM video_tags WHERE video_id = ?1 AND tag_id = ?2", rusqlite::params![video_id, wid]);
        }
        if let Some(uwid) = unwatched_id {
            let _ = conn.execute("INSERT OR IGNORE INTO video_tags (video_id, tag_id) VALUES (?1, ?2)", rusqlite::params![video_id, uwid]);
        }
        Ok("未看过".to_string())
    } else {
        // 当前是未看过 → 切换为已看过
        if let Some(uwid) = unwatched_id {
            let _ = conn.execute("DELETE FROM video_tags WHERE video_id = ?1 AND tag_id = ?2", rusqlite::params![video_id, uwid]);
        }
        if let Some(wid) = watched_id {
            let _ = conn.execute("INSERT OR IGNORE INTO video_tags (video_id, tag_id) VALUES (?1, ?2)", rusqlite::params![video_id, wid]);
        }
        Ok("已看过".to_string())
    }
}

/// 扫描为剧集：选中的文件夹就是一部剧，里面的视频都是剧集
#[tauri::command]
pub fn scan_series(
    db: State<'_, DbConn>,
    dir_path: String,
    series_name: String,
    incremental: bool,
    default_watched: bool,
) -> Result<ScanResult, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    scanner::clear_dir_cache();

    let video_files = if incremental {
        let mut stmt = conn
            .prepare("SELECT file_path FROM videos")
            .map_err(|e| AppError::db(e.to_string()))?;
        let existing: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| AppError::db(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();
        scanner::scan_directory_incremental(&dir_path, &existing)
    } else {
        scanner::scan_directory(&dir_path)
    };

    let total_found = video_files.len() as i64;
    if total_found == 0 {
        return Ok(ScanResult { total_found: 0, new_added: 0, updated: 0, errors: Vec::new(), series_detected: Vec::new() });
    }

    let mut new_added: i64 = 0;
    let mut updated: i64 = 0;
    let mut errors = Vec::new();

    conn.execute_batch("BEGIN").map_err(|e| AppError::db(e.to_string()))?;

    for (idx, file) in video_files.iter().enumerate() {
        let mut info = scanner::extract_video_info(file);
        // 强制设为剧集
        info.series_name = Some(series_name.clone());
        // 如果没有解析到集数，用序号
        if info.episode.is_none() {
            info.episode = Some((idx + 1) as i32);
        }

        match crate::db::upsert_video(
            &conn,
            &info.title,
            &info.file_path,
            &info.file_name,
            info.file_size,
            "episode",
            info.series_name.as_deref(),
            info.season.or(Some(1)),
            info.episode,
            default_watched,
        ) {
            Ok(result) => {
                if result.is_inserted() { new_added += 1; } else { updated += 1; }
            }
            Err(e) => { errors.push(format!("{}: {}", info.file_path, e)); }
        }
    }

    conn.execute_batch("COMMIT").map_err(|e| AppError::db(e.to_string()))?;

    let mut series_detected = Vec::new();
    series_detected.push(SeriesInfo { name: series_name, episode_count: new_added + updated, folder_path: dir_path });

    Ok(ScanResult { total_found, new_added, updated, errors, series_detected })
}

/// 获取电影列表（不含剧集）
#[tauri::command]
pub fn get_movies(db: State<'_, DbConn>, page: Option<i64>, page_size: Option<i64>) -> Result<PaginatedVideos, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::query_videos_paginated(&conn, "WHERE v.video_type = 'movie'", &[], page.unwrap_or(0), page_size.unwrap_or(999999)).map_err(|e| e.to_string())
}

/// 获取剧集概览（按 series_name 分组，含进度）
#[tauri::command]
pub fn get_series_overview(db: State<'_, DbConn>) -> Result<Vec<SeriesOverview>, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_series_overview(&conn).map_err(|e| e.to_string())
}

/// 获取某个剧集的所有剧集（按 SxxExx 或集数排序）
#[tauri::command]
pub fn get_series_episodes(db: State<'_, DbConn>, series: String) -> Result<Vec<VideoWithTags>, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::get_series_episodes(&conn, &series).map_err(|e| e.to_string())
}

/// 批量标记剧集为已看过
#[tauri::command]
pub fn mark_series_watched(db: State<'_, DbConn>, series: String, watched: bool) -> Result<i64, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::mark_series_watched(&conn, &series, watched).map_err(|e| e.to_string())
}

/// 修改视频标题
#[tauri::command]
pub fn rename_video(db: State<'_, DbConn>, video_id: i64, new_title: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    conn.execute(
        "UPDATE videos SET title = ?1, updated_at = datetime('now','localtime') WHERE id = ?2",
        rusqlite::params![new_title, video_id],
    ).map_err(|e| AppError::db(e.to_string()))?;
    Ok(())
}

/// 删除视频记录
#[tauri::command]
pub fn delete_video(db: State<'_, DbConn>, video_id: i64) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    crate::db::delete_video(&conn, video_id).map_err(|e| e.to_string())
}

/// 删除视频文件 + 数据库记录
#[tauri::command]
pub fn delete_video_with_file(db: State<'_, DbConn>, video_id: i64) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    // 先查出文件路径
    let file_path: String = conn.query_row(
        "SELECT file_path FROM videos WHERE id = ?1",
        rusqlite::params![video_id],
        |r| r.get(0),
    ).map_err(|_| AppError::db("视频不存在"))?;
    // 删除数据库记录
    crate::db::delete_video(&conn, video_id).map_err(|e| e.to_string())?;
    // 删除文件
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

/// 删除指定文件夹下的所有视频记录
#[tauri::command]
pub fn delete_videos_by_folder(db: State<'_, DbConn>, folder_path: String) -> Result<i64, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    // 统一路径格式
    let normalized = folder_path.replace('/', "\\").trim_end_matches('\\').to_string();
    let pattern = format!("{}\\%", normalized);
    let count_before: i64 = conn.query_row("SELECT COUNT(*) FROM videos", [], |r| r.get(0))
        .map_err(|e| AppError::db(e.to_string()))?;
    conn.execute(
        "DELETE FROM videos WHERE file_path = ?1 OR file_path LIKE ?2",
        rusqlite::params![normalized, pattern],
    ).map_err(|e| AppError::db(e.to_string()))?;
    let count_after: i64 = conn.query_row("SELECT COUNT(*) FROM videos", [], |r| r.get(0))
        .map_err(|e| AppError::db(e.to_string()))?;
    Ok(count_before - count_after)
}

/// 清理非视频文件记录（扩展名不在视频列表中的）
#[tauri::command]
pub fn clean_non_videos(db: State<'_, DbConn>) -> Result<i64, String> {
    let conn = db.0.lock().map_err(|e| AppError::db(e.to_string()))?;
    let video_exts = [
        "mp4", "mkv", "avi", "rmvb", "wmv", "flv", "mov", "ts", "m4v",
        "mpg", "mpeg", "webm", "vob", "ogv", "3gp", "f4v", "mts", "m2ts",
        "divx", "asf", "rm", "tp",
    ];
    let sql = format!(
        "DELETE FROM videos WHERE file_name NOT LIKE '%.mp4' AND file_name NOT LIKE '%.mkv'
         AND file_name NOT LIKE '%.avi' AND file_name NOT LIKE '%.rmvb'
         AND file_name NOT LIKE '%.wmv' AND file_name NOT LIKE '%.flv'
         AND file_name NOT LIKE '%.mov' AND file_name NOT LIKE '%.ts'
         AND file_name NOT LIKE '%.m4v' AND file_name NOT LIKE '%.mpg'
         AND file_name NOT LIKE '%.mpeg' AND file_name NOT LIKE '%.webm'
         AND file_name NOT LIKE '%.vob' AND file_name NOT LIKE '%.ogv'
         AND file_name NOT LIKE '%.3gp' AND file_name NOT LIKE '%.f4v'
         AND file_name NOT LIKE '%.mts' AND file_name NOT LIKE '%.m2ts'
         AND file_name NOT LIKE '%.divx' AND file_name NOT LIKE '%.asf'
         AND file_name NOT LIKE '%.rm' AND file_name NOT LIKE '%.tp'"
    );
    conn.execute_batch("BEGIN").map_err(|e| AppError::db(e.to_string()))?;
    let count_before: i64 = conn.query_row("SELECT COUNT(*) FROM videos", [], |r| r.get(0))
        .map_err(|e| AppError::db(e.to_string()))?;
    conn.execute_batch(&sql).map_err(|e| AppError::db(e.to_string()))?;
    let count_after: i64 = conn.query_row("SELECT COUNT(*) FROM videos", [], |r| r.get(0))
        .map_err(|e| AppError::db(e.to_string()))?;
    conn.execute_batch("COMMIT").map_err(|e| AppError::db(e.to_string()))?;
    Ok(count_before - count_after)
}

/// #17: 一次 PowerShell 调用获取所有盘符的卷标
#[tauri::command]
pub fn get_drives() -> Result<Vec<DriveInfo>, String> {
    let labels = get_all_volume_labels();
    let mut drives = Vec::new();
    for letter in 'A'..='Z' {
        let path = format!("{}:\\", letter);
        if Path::new(&path).exists() {
            let label = labels
                .get(&letter)
                .cloned()
                .unwrap_or_else(|| format!("本地磁盘 ({})", letter));
            drives.push(DriveInfo {
                letter: letter.to_string(),
                path,
                label,
            });
        }
    }
    Ok(drives)
}

/// #17: 一条命令查完所有盘符卷标
fn get_all_volume_labels() -> std::collections::HashMap<char, String> {
    let mut result = std::collections::HashMap::new();
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile", "-Command",
            "[Console]::OutputEncoding = [Text.Encoding]::UTF8; Get-Volume | Where-Object { $_.DriveLetter } | ForEach-Object { \"$($_.DriveLetter)`t$($_.FileSystemLabel)\" }",
        ])
        .output();

    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let line = line.trim();
            if let Some((letter_str, label)) = line.split_once('\t') {
                if let Some(letter) = letter_str.trim().chars().next() {
                    let label = label.trim().to_string();
                    if !label.is_empty() {
                        result.insert(letter, label);
                    }
                }
            }
        }
    }
    result
}

#[derive(serde::Serialize)]
pub struct DriveInfo {
    pub letter: String,
    pub path: String,
    pub label: String,
}
