// ============================================
// 旧库迁移：从 MovieUI（com.movieui.app）数据库迁移到 CineVault
// 直接读旧 .db 文件，全量迁移，比 JSON 导出可靠
// ============================================
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use std::path::Path;

use crate::models::{AppError, MigrationResult};

const OLD_APP_ID: &str = "com.movieui.app";
const OLD_DB_NAME: &str = "movieui.db";

/// 定位旧库路径：%APPDATA%/com.movieui.app/movieui.db
pub fn locate_old_db() -> Option<String> {
    let appdata = std::env::var("APPDATA").ok()?;
    let p = Path::new(&appdata).join(OLD_APP_ID).join(OLD_DB_NAME);
    if p.exists() {
        Some(p.to_string_lossy().to_string())
    } else {
        None
    }
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        params![name],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

pub fn migrate(conn: &Connection, old_path: &str) -> Result<MigrationResult, AppError> {
    let old = Connection::open_with_flags(
        old_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| AppError::io(format!("无法打开旧数据库 {}: {}", old_path, e)))?;

    let tx = conn.unchecked_transaction()?;

    // 历史去重索引：保证迁移可重复执行（幂等），避免重复堆积
    tx.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_watch_history_dedup
         ON watch_history(video_id, watched_at, progress);",
    )?;

    // ===== 1. 扫描目录 =====
    let mut scan_folders_migrated = 0i64;
    if table_exists(&old, "scan_folders") {
        let mut stmt = old
            .prepare("SELECT folder_path, last_scanned_at FROM scan_folders")
            .map_err(|e| AppError::db(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))
            .map_err(|e| AppError::db(e.to_string()))?;
        for r in rows {
            if let Ok((path, ts)) = r {
                tx.execute(
                    "INSERT OR IGNORE INTO scan_folders (folder_path, last_scanned_at) VALUES (?1, ?2)",
                    params![path, ts],
                )?;
                scan_folders_migrated += 1;
            }
        }
    }

    // ===== 2. series（旧 id -> 新 id 映射）=====
    let mut series_map: HashMap<i64, i64> = HashMap::new();
    let mut series_count = 0i64;
    if table_exists(&old, "series") {
        let mut stmt = old
            .prepare("SELECT id, name, folder_path, rating, last_watched_at, watch_progress FROM series")
            .map_err(|e| AppError::db(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, f64>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, f64>(5)?,
                ))
            })
            .map_err(|e| AppError::db(e.to_string()))?;
        for r in rows {
            if let Ok((old_id, name, folder, rating, last_watched, progress)) = r {
                tx.execute(
                    "INSERT INTO series (name, folder_path, rating, last_watched_at, watch_progress)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(name) DO NOTHING",
                    params![name, folder, rating, last_watched, progress],
                )?;
                let new_id: i64 = tx.query_row(
                    "SELECT id FROM series WHERE name = ?1",
                    params![name],
                    |r| r.get(0),
                )?;
                series_map.insert(old_id, new_id);
                series_count += 1;
            }
        }
    }

    // ===== 3. videos =====
    let mut video_map: HashMap<i64, i64> = HashMap::new();
    let mut videos_count = 0i64;
    if table_exists(&old, "videos") {
        // 探测列：老版本可能有 series_name（字符串）或 series_id
        let has_series_id = table_has_column(&old, "videos", "series_id");
        let has_series_name = table_has_column(&old, "videos", "series_name");
        let has_kind = table_has_column(&old, "videos", "kind");
        let has_user_kind = table_has_column(&old, "videos", "user_kind");

        let mut cols = "id, title, file_path, file_name, file_size, duration, rating, video_type, season, episode, last_watched_at, watch_progress, updated_at".to_string();
        if has_series_id {
            cols.push_str(", series_id");
        }
        if has_series_name {
            cols.push_str(", series_name");
        }
        if has_kind {
            cols.push_str(", kind");
        }
        if has_user_kind {
            cols.push_str(", user_kind");
        }
        let sql = format!("SELECT {} FROM videos", cols);
        let mut stmt = old.prepare(&sql).map_err(|e| AppError::db(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| {
                let mut idx = 0;
                let id: i64 = r.get(idx)?; idx += 1;
                let title: String = r.get(idx)?; idx += 1;
                let file_path: String = r.get(idx)?; idx += 1;
                let file_name: String = r.get(idx)?; idx += 1;
                let file_size: i64 = r.get(idx)?; idx += 1;
                let duration: Option<f64> = r.get(idx)?; idx += 1;
                let rating: f64 = r.get(idx)?; idx += 1;
                let video_type: String = r.get(idx)?; idx += 1;
                let season: Option<i32> = r.get(idx)?; idx += 1;
                let episode: Option<i32> = r.get(idx)?; idx += 1;
                let last_watched_at: Option<String> = r.get(idx)?; idx += 1;
                let watch_progress: f64 = r.get(idx)?; idx += 1;
                let updated_at: Option<String> = r.get(idx)?; idx += 1;
                let old_series_id: Option<i64> = if has_series_id { let v = r.get(idx)?; idx += 1; v } else { None };
                let old_series_name: Option<String> = if has_series_name { let v = r.get(idx)?; idx += 1; v } else { None };
                let old_kind: Option<String> = if has_kind { let v = r.get(idx)?; idx += 1; v } else { None };
                let old_user_kind: Option<String> = if has_user_kind { r.get(idx)? } else { None };
                Ok((id, title, file_path, file_name, file_size, duration, rating, video_type, season, episode, last_watched_at, watch_progress, updated_at, old_series_id, old_series_name, old_kind, old_user_kind))
            })
            .map_err(|e| AppError::db(e.to_string()))?;

        for r in rows {
            if let Ok((old_id, title, file_path, file_name, file_size, duration, rating,
                       video_type, season, episode, last_watched_at, watch_progress, updated_at,
                       old_series_id, old_series_name, old_kind, old_user_kind)) = r {
                // kind 映射
                let kind = old_user_kind.clone().or(old_kind.clone()).unwrap_or_else(|| {
                    match video_type.as_str() {
                        "episode" => "series".to_string(),
                        "movie" => "movie".to_string(),
                        _ => "unknown".to_string(),
                    }
                });
                let confidence = if old_user_kind.is_some() { 100 } else { 80 };
                // series_id 解析：优先旧外键，其次旧字符串名
                let new_series_id: Option<i64> = if let Some(sid) = old_series_id {
                    series_map.get(&sid).copied()
                } else if let Some(ref sname) = old_series_name {
                    if !sname.trim().is_empty() {
                        Some(crate::db::upsert_series(&tx, sname.trim(), None)?)
                    } else {
                        None
                    }
                } else {
                    None
                };
                tx.execute(
                    "INSERT OR IGNORE INTO videos (title, file_path, file_name, file_size, duration, rating, kind, confidence, user_kind, series_id, season, episode, last_watched_at, watch_progress, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, COALESCE(?15, datetime('now','localtime')), COALESCE(?16, datetime('now','localtime')))",
                    params![title, file_path, file_name, file_size, duration, rating,
                            kind, confidence, old_user_kind, new_series_id, season, episode, last_watched_at, watch_progress,
                            updated_at.as_deref().map(|s| if s.is_empty() { None } else { Some(s) }).flatten(),
                            updated_at.as_deref().map(|s| if s.is_empty() { None } else { Some(s) }).flatten()],
                )?;
                // INSERT OR IGNORE 跳过时 last_insert_rowid 是残留值——必须 SELECT 取真实 id
                let new_id: i64 = tx
                    .query_row(
                        "SELECT id FROM videos WHERE file_path = ?1",
                        params![file_path],
                        |r| r.get(0),
                    )
                    .map_err(|e| AppError::db(format!("新插入视频查不到真实 id ({:?}): {}", file_path, e)))?;
                video_map.insert(old_id, new_id);
                videos_count += 1;
            }
        }
    }

    // ===== 4. tags（排除已看过/未看过）+ 关联 =====
    let mut tag_map: HashMap<i64, i64> = HashMap::new();
    let mut tags_count = 0i64;
    if table_exists(&old, "tags") {
        let mut stmt = old
            .prepare("SELECT id, name, color FROM tags")
            .map_err(|e| AppError::db(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
            .map_err(|e| AppError::db(e.to_string()))?;
        for r in rows {
            if let Ok((old_id, name, color)) = r {
                if name == "已看过" || name == "未看过" {
                    continue;
                }
                tx.execute(
                    "INSERT OR IGNORE INTO tags (name, color) VALUES (?1, ?2)",
                    params![name, color],
                )?;
                let new_id: i64 = tx.query_row("SELECT id FROM tags WHERE name = ?1", params![name], |r| r.get(0))?;
                tag_map.insert(old_id, new_id);
                tags_count += 1;
            }
        }
    }

    // ===== 5. video_tags（含"已看过"→ watch_history 映射）=====
    let mut watched_tag_id: Option<i64> = None;
    if table_exists(&old, "tags") {
        watched_tag_id = old
            .query_row("SELECT id FROM tags WHERE name = '已看过'", [], |r| r.get(0))
            .optional()
            .ok()
            .flatten();
    }
    let mut history_count = 0i64;
    if table_exists(&old, "video_tags") {
        let mut stmt = old
            .prepare("SELECT video_id, tag_id FROM video_tags")
            .map_err(|e| AppError::db(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| AppError::db(e.to_string()))?;
        for r in rows {
            if let Ok((old_video_id, old_tag_id)) = r {
                if Some(old_tag_id) == watched_tag_id {
                    // 已看过 → 新库进度置 100（绕过 INSERT OR IGNORE 跳过，已存在行也生效）
                    if let Some(new_vid) = video_map.get(&old_video_id) {
                        tx.execute(
                            "UPDATE videos SET watch_progress = 100 WHERE id = ?1 AND watch_progress < 100",
                            params![new_vid],
                        )?;
                        let watched_at: Option<String> = tx
                            .query_row("SELECT updated_at FROM videos WHERE id = ?1", params![new_vid], |r| r.get(0))
                            .optional()
                            .ok()
                            .flatten();
                        let fallback = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                        tx.execute(
                            "INSERT OR IGNORE INTO watch_history (video_id, watched_at, progress) VALUES (?1, ?2, 100)",
                            params![new_vid, watched_at.unwrap_or(fallback)],
                        )?;
                        history_count += 1;
                    }
                    continue;
                }
                if let (Some(new_vid), Some(new_tag)) = (video_map.get(&old_video_id), tag_map.get(&old_tag_id)) {
                    tx.execute(
                        "INSERT OR IGNORE INTO video_tags (video_id, tag_id) VALUES (?1, ?2)",
                        params![new_vid, new_tag],
                    )?;
                }
            }
        }
    }

    // ===== 6. series_tags =====
    if table_exists(&old, "series_tags") {
        let mut stmt = old
            .prepare("SELECT series_id, tag_id FROM series_tags")
            .map_err(|e| AppError::db(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| AppError::db(e.to_string()))?;
        for r in rows {
            if let Ok((old_sid, old_tag_id)) = r {
                if let (Some(new_sid), Some(new_tag)) = (series_map.get(&old_sid), tag_map.get(&old_tag_id)) {
                    tx.execute(
                        "INSERT OR IGNORE INTO series_tags (series_id, tag_id) VALUES (?1, ?2)",
                        params![new_sid, new_tag],
                    )?;
                }
            }
        }
    }

    // ===== 7. watch_history（旧的独立历史表）=====
    if table_exists(&old, "watch_history") {
        let mut stmt = old
            .prepare("SELECT video_id, watched_at, progress FROM watch_history")
            .map_err(|e| AppError::db(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, f64>(2)?)))
            .map_err(|e| AppError::db(e.to_string()))?;
        for r in rows {
            if let Ok((old_video_id, watched_at, progress)) = r {
                if let Some(new_vid) = video_map.get(&old_video_id) {
                    tx.execute(
                        "INSERT OR IGNORE INTO watch_history (video_id, watched_at, progress) VALUES (?1, ?2, ?3)",
                        params![new_vid, watched_at, progress],
                    )?;
                    history_count += 1;
                }
            }
        }
    }

    // ===== 8. 记录迁移标记 =====
    crate::db::set_setting(&tx, "migrated_from", old_path)?;

    tx.commit()?;

    Ok(MigrationResult {
        videos: videos_count,
        series: series_count,
        tags: tags_count,
        history: history_count,
        scan_folders: scan_folders_migrated,
        from_path: old_path.to_string(),
    })
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> bool {
    conn.prepare(&format!("SELECT {} FROM {} LIMIT 1", column, table))
        .map(|_| true)
        .unwrap_or(false)
}
