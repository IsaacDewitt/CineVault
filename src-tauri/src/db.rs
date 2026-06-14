use rusqlite::{Connection, params, Row};
use std::sync::Mutex;

use crate::models::*;

pub struct DbConn(pub Mutex<Connection>);

impl DbConn {
    pub fn init(conn: Connection) -> Self {
        Self(Mutex::new(conn))
    }
}

// ============================================
// #23: 公共行映射函数
// ============================================
fn row_to_video(row: &Row) -> rusqlite::Result<Video> {
    Ok(Video {
        id: row.get(0)?,
        title: row.get(1)?,
        file_path: row.get(2)?,
        file_name: row.get(3)?,
        file_size: row.get(4)?,
        duration: row.get(5)?,
        rating: row.get(6)?,
        is_favorite: row.get::<_, i32>(7)? != 0,
        video_type: row.get(8)?,
        series_name: row.get(9)?,
        season: row.get(10)?,
        episode: row.get(11)?,
        thumbnail_path: row.get(12)?,
        last_watched_at: row.get(13)?,
        watch_progress: row.get(14)?,
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
    })
}

/// 分隔符常量：ASCII 控制字符，不会出现在标签名/颜色值中
const TAG_SEP: &str = "\x1E";  // Record Separator — 分隔不同标签
const FIELD_SEP: &str = "\x1F"; // Unit Separator — 分隔 id/name/color

/// 解析聚合的标签字符串
fn parse_tags_string(tags_str: Option<String>) -> Vec<Tag> {
    match tags_str {
        None => Vec::new(),
        Some(s) if s.is_empty() => Vec::new(),
        Some(s) => s.split(TAG_SEP).filter_map(|part| {
            let mut parts = part.splitn(3, FIELD_SEP);
            let id: i64 = parts.next()?.parse().ok()?;
            let name = parts.next()?.to_string();
            let color = parts.next()?.to_string();
            Some(Tag { id, name, color, video_count: None })
        }).collect(),
    }
}

/// 初始化数据库，创建所有表
pub fn initialize_database(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS videos (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL,
            file_path TEXT NOT NULL UNIQUE,
            file_name TEXT NOT NULL,
            file_size INTEGER NOT NULL DEFAULT 0,
            duration REAL,
            rating REAL NOT NULL DEFAULT 0,
            is_favorite INTEGER NOT NULL DEFAULT 0,
            video_type TEXT NOT NULL DEFAULT 'movie',
            series_name TEXT,
            season INTEGER,
            episode INTEGER,
            thumbnail_path TEXT,
            last_watched_at TEXT,
            watch_progress REAL NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime'))
        );

        CREATE TABLE IF NOT EXISTS tags (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            color TEXT NOT NULL DEFAULT '#6366f1',
            sort_order INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS video_tags (
            video_id INTEGER NOT NULL,
            tag_id INTEGER NOT NULL,
            PRIMARY KEY (video_id, tag_id),
            FOREIGN KEY (video_id) REFERENCES videos(id) ON DELETE CASCADE,
            FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS watch_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            video_id INTEGER NOT NULL,
            watched_at TEXT NOT NULL DEFAULT (datetime('now','localtime')),
            progress REAL NOT NULL DEFAULT 0,
            FOREIGN KEY (video_id) REFERENCES videos(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_videos_file_path ON videos(file_path);
        CREATE INDEX IF NOT EXISTS idx_videos_series ON videos(series_name);
        CREATE INDEX IF NOT EXISTS idx_videos_rating ON videos(rating);
        CREATE INDEX IF NOT EXISTS idx_videos_favorite ON videos(is_favorite);
        CREATE INDEX IF NOT EXISTS idx_watch_history_video ON watch_history(video_id);

        CREATE TABLE IF NOT EXISTS scan_folders (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            folder_path TEXT NOT NULL UNIQUE,
            video_count INTEGER NOT NULL DEFAULT 0,
            last_scanned_at TEXT NOT NULL DEFAULT (datetime('now','localtime'))
        );

        -- 默认标签
        INSERT OR IGNORE INTO tags (name, color) VALUES ('已看过', '#10b981');
        INSERT OR IGNORE INTO tags (name, color) VALUES ('未看过', '#6b6b80');
        "
    )?;

    // 数据库迁移：为 tags 表添加 sort_order 字段（如果不存在）
    let has_sort_order: bool = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('tags') WHERE name = 'sort_order'",
        [],
        |row| row.get::<_, i64>(0)
    ).unwrap_or(0) > 0;

    if !has_sort_order {
        conn.execute_batch("ALTER TABLE tags ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0")?;
    }

    Ok(())
}

// ============================================
// #14: 用一条 SQL JOIN 查出视频+标签
// NEW-2: 修复双重 JOIN，用子查询替代
// ============================================

/// 核心分页查询函数（公开供多标签筛选使用）
pub fn query_videos_paginated(
    conn: &Connection,
    where_extra: &str,
    params: &[&dyn rusqlite::types::ToSql],
    page: i64,
    page_size: i64,
) -> Result<PaginatedVideos, AppError> {
    // 先查总数
    let count_sql = format!(
        "SELECT COUNT(DISTINCT v.id) FROM videos v {}",
        where_extra
    );
    let total: i64 = conn.query_row(&count_sql, params, |row| row.get(0))?;

    // 再查分页数据
    let offset = page * page_size;
    let sql = format!(
        "SELECT
            v.id, v.title, v.file_path, v.file_name, v.file_size, v.duration, v.rating,
            v.is_favorite, v.video_type, v.series_name, v.season, v.episode, v.thumbnail_path,
            v.last_watched_at, v.watch_progress, v.created_at, v.updated_at,
            GROUP_CONCAT(t.id || '\x1F' || t.name || '\x1F' || t.color, '\x1E')
         FROM videos v
         LEFT JOIN video_tags vt ON v.id = vt.video_id
         LEFT JOIN tags t ON vt.tag_id = t.id
         {}
         GROUP BY v.id
         ORDER BY v.updated_at DESC
         LIMIT ? OFFSET ?",
        where_extra
    );

    // 把 limit/offset 追加到参数列表
    let mut all_params: Vec<&dyn rusqlite::types::ToSql> = params.to_vec();
    all_params.push(&page_size);
    all_params.push(&offset);

    let mut stmt = conn.prepare(&sql)?;
    let items = stmt.query_map(all_params.as_slice(), |row| {
        let video = row_to_video(row)?;
        let tags_str: Option<String> = row.get(17)?;
        let tags = parse_tags_string(tags_str);
        Ok(VideoWithTags { video, tags })
    })?.collect::<rusqlite::Result<Vec<_>>>()?;

    let has_more = offset + (items.len() as i64) < total;

    Ok(PaginatedVideos { items, total, page, page_size, has_more })
}

/// 获取所有视频（分页）
pub fn get_all_videos(conn: &Connection, page: i64, page_size: i64) -> Result<PaginatedVideos, AppError> {
    query_videos_paginated(conn, "", &[], page, page_size)
}

/// 搜索视频（分页）
pub fn search_videos(conn: &Connection, query: &str, page: i64, page_size: i64) -> Result<PaginatedVideos, AppError> {
    let pattern = format!("%{}%", query);
    query_videos_paginated(
        conn,
        "WHERE v.title LIKE ?1 OR v.file_name LIKE ?2 OR v.series_name LIKE ?3",
        params![pattern, pattern, pattern],
        page, page_size,
    )
}

/// 按标签筛选（分页）
pub fn get_videos_by_tag(conn: &Connection, tag_id: i64, page: i64, page_size: i64) -> Result<PaginatedVideos, AppError> {
    query_videos_paginated(
        conn,
        "WHERE EXISTS (SELECT 1 FROM video_tags vt2 WHERE vt2.video_id = v.id AND vt2.tag_id = ?1)",
        params![tag_id],
        page, page_size,
    )
}

/// 获取收藏视频（分页）
pub fn get_favorite_videos(conn: &Connection, page: i64, page_size: i64) -> Result<PaginatedVideos, AppError> {
    query_videos_paginated(conn, "WHERE v.is_favorite = 1", &[], page, page_size)
}

/// 获取剧集列表（分页）
pub fn get_series_list(conn: &Connection, page: i64, page_size: i64) -> Result<PaginatedVideos, AppError> {
    query_videos_paginated(
        conn,
        "WHERE v.series_name IS NOT NULL",
        &[],
        page, page_size,
    )
}

/// 更新评分 (#21: 范围验证)
pub fn update_rating(conn: &Connection, video_id: i64, rating: f64) -> Result<(), AppError> {
    if !rating.is_finite() || rating < 0.0 || rating > 10.0 {
        return Err(AppError::validation("评分必须在 0-10 之间"));
    }
    conn.execute(
        "UPDATE videos SET rating = ?1, updated_at = datetime('now','localtime') WHERE id = ?2",
        params![rating, video_id],
    )?;
    Ok(())
}

/// 切换收藏
pub fn toggle_favorite(conn: &Connection, video_id: i64) -> Result<bool, AppError> {
    conn.execute(
        "UPDATE videos SET is_favorite = CASE WHEN is_favorite = 1 THEN 0 ELSE 1 END,
         updated_at = datetime('now','localtime') WHERE id = ?1",
        params![video_id],
    )?;
    let fav: bool = conn.query_row(
        "SELECT is_favorite FROM videos WHERE id = ?1",
        params![video_id],
        |row| Ok(row.get::<_, i32>(0)? != 0),
    )?;
    Ok(fav)
}

/// 更新播放进度 (#22: 范围验证, #26: 事务)
pub fn update_watch_progress(conn: &Connection, video_id: i64, progress: f64) -> Result<(), AppError> {
    if !progress.is_finite() || progress < 0.0 || progress > 1.0 {
        return Err(AppError::validation("播放进度必须在 0.0-1.0 之间"));
    }
    conn.execute_batch("BEGIN")?;
    let result = (|| -> Result<(), AppError> {
        conn.execute(
            "UPDATE videos SET watch_progress = ?1, last_watched_at = datetime('now','localtime'),
             updated_at = datetime('now','localtime') WHERE id = ?2",
            params![progress, video_id],
        )?;
        conn.execute(
            "INSERT INTO watch_history (video_id, progress) VALUES (?1, ?2)",
            params![video_id, progress],
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => { conn.execute_batch("COMMIT")?; Ok(()) }
        Err(e) => { let _ = conn.execute_batch("ROLLBACK"); Err(e) }
    }
}

/// 获取播放历史
pub fn get_watch_history(conn: &Connection, limit: i64) -> Result<Vec<WatchHistory>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT h.id, h.video_id, h.watched_at, h.progress, v.title, v.file_path
         FROM watch_history h
         LEFT JOIN videos v ON h.video_id = v.id
         ORDER BY h.watched_at DESC LIMIT ?1"
    )?;

    let history = stmt.query_map(params![limit], |row| {
        Ok(WatchHistory {
            id: row.get(0)?,
            video_id: row.get(1)?,
            watched_at: row.get(2)?,
            progress: row.get(3)?,
            video_title: row.get(4)?,
            video_path: row.get(5)?,
        })
    })?.collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(history)
}

/// 获取所有标签（含视频数量）
pub fn get_all_tags(conn: &Connection) -> Result<Vec<Tag>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.name, t.color, COUNT(vt.video_id) as video_count
         FROM tags t LEFT JOIN video_tags vt ON t.id = vt.tag_id
         GROUP BY t.id ORDER BY t.sort_order, t.name"
    )?;

    let tags = stmt.query_map([], |row| {
        Ok(Tag {
            id: row.get(0)?,
            name: row.get(1)?,
            color: row.get(2)?,
            video_count: Some(row.get(3)?),
        })
    })?.collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(tags)
}

/// 创建标签
pub fn create_tag(conn: &Connection, name: &str, color: &str) -> Result<Tag, AppError> {
    conn.execute(
        "INSERT INTO tags (name, color) VALUES (?1, ?2)",
        params![name, color],
    )?;
    let id = conn.last_insert_rowid();
    Ok(Tag {
        id,
        name: name.to_string(),
        color: color.to_string(),
        video_count: Some(0),
    })
}

/// 删除标签
pub fn delete_tag(conn: &Connection, tag_id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM tags WHERE id = ?1", params![tag_id])?;
    Ok(())
}

/// 更新标签（名称和/或颜色），保留 video_tags 关联
pub fn update_tag(conn: &Connection, tag_id: i64, name: Option<&str>, color: Option<&str>) -> Result<(), AppError> {
    match (name, color) {
        (Some(n), Some(c)) => {
            conn.execute(
                "UPDATE tags SET name = ?1, color = ?2 WHERE id = ?3",
                params![n, c, tag_id],
            )?;
        }
        (Some(n), None) => {
            conn.execute(
                "UPDATE tags SET name = ?1 WHERE id = ?2",
                params![n, tag_id],
            )?;
        }
        (None, Some(c)) => {
            conn.execute(
                "UPDATE tags SET color = ?1 WHERE id = ?2",
                params![c, tag_id],
            )?;
        }
        (None, None) => {}
    }
    Ok(())
}

/// 批量更新标签排序
pub fn update_tag_orders(conn: &Connection, orders: &[(i64, i64)]) -> Result<(), AppError> {
    let mut stmt = conn.prepare("UPDATE tags SET sort_order = ?1 WHERE id = ?2")?;
    for &(tag_id, sort_order) in orders {
        stmt.execute(params![sort_order, tag_id])?;
    }
    Ok(())
}

/// 给视频添加标签（已看过/未看过互斥）
pub fn add_tag_to_video(conn: &Connection, video_id: i64, tag_id: i64) -> Result<(), AppError> {
    // 查看要添加的标签名
    let tag_name: String = conn.query_row(
        "SELECT name FROM tags WHERE id = ?1",
        params![tag_id],
        |r| r.get(0),
    ).unwrap_or_default();

    // 如果是"已看过"或"未看过"，先移除互斥的那个
    let opposite = match tag_name.as_str() {
        "已看过" => Some("未看过"),
        "未看过" => Some("已看过"),
        _ => None,
    };
    if let Some(opposite_name) = opposite {
        conn.execute(
            "DELETE FROM video_tags WHERE video_id = ?1 AND tag_id = (SELECT id FROM tags WHERE name = ?2)",
            params![video_id, opposite_name],
        )?;
    }

    conn.execute(
        "INSERT OR IGNORE INTO video_tags (video_id, tag_id) VALUES (?1, ?2)",
        params![video_id, tag_id],
    )?;
    Ok(())
}

/// 移除视频标签
pub fn remove_tag_from_video(conn: &Connection, video_id: i64, tag_id: i64) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM video_tags WHERE video_id = ?1 AND tag_id = ?2",
        params![video_id, tag_id],
    )?;
    Ok(())
}

/// #1/#7: 返回 UpsertResult 区分插入和更新
/// #8: 直接赋值替代 COALESCE，允许清除 series 信息
pub fn upsert_video(
    conn: &Connection,
    title: &str,
    file_path: &str,
    file_name: &str,
    file_size: i64,
    video_type: &str,
    series_name: Option<&str>,
    season: Option<i32>,
    episode: Option<i32>,
    default_watched: bool,
) -> Result<UpsertResult, AppError> {
    let existing: Result<i64, _> = conn.query_row(
        "SELECT id FROM videos WHERE file_path = ?1",
        params![file_path],
        |row| row.get(0),
    );

    match existing {
        Ok(id) => {
            conn.execute(
                "UPDATE videos SET title = ?1, file_name = ?2, file_size = ?3,
                 video_type = ?4, series_name = ?5, season = ?6, episode = ?7,
                 updated_at = datetime('now','localtime')
                 WHERE id = ?8",
                params![title, file_name, file_size, video_type, series_name, season, episode, id],
            )?;
            Ok(UpsertResult::Updated(id))
        }
        Err(_) => {
            conn.execute(
                "INSERT INTO videos (title, file_path, file_name, file_size, video_type, series_name, season, episode)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![title, file_path, file_name, file_size, video_type, series_name, season, episode],
            )?;
            let new_id = conn.last_insert_rowid();
        // 根据参数决定默认标签
        let default_tag = if default_watched { "已看过" } else { "未看过" };
        let _ = conn.execute(
            "INSERT OR IGNORE INTO video_tags (video_id, tag_id)
             SELECT ?1, id FROM tags WHERE name = ?2",
            rusqlite::params![new_id, default_tag],
        );
        Ok(UpsertResult::Inserted(new_id))
        }
    }
}

/// 更新缩略图路径
pub fn update_thumbnail(conn: &Connection, video_id: i64, thumb_path: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE videos SET thumbnail_path = ?1 WHERE id = ?2",
        params![thumb_path, video_id],
    )?;
    Ok(())
}

/// #15: 统计信息 — 单条 SQL
pub fn get_stats(conn: &Connection) -> Result<VideoStats, AppError> {
    let stats = conn.query_row(
        "SELECT
            COUNT(*),
            COUNT(DISTINCT series_name),
            COALESCE(SUM(file_size), 0),
            SUM(CASE WHEN is_favorite = 1 THEN 1 ELSE 0 END),
            SUM(CASE WHEN last_watched_at IS NOT NULL THEN 1 ELSE 0 END),
            COALESCE(AVG(CASE WHEN rating > 0 THEN rating END), 0),
            SUM(CASE WHEN video_type = 'movie' THEN 1 ELSE 0 END)
         FROM videos",
        [],
        |row| {
            Ok(VideoStats {
                total_videos: row.get(0)?,
                total_series: row.get(1)?,
                total_size: row.get(2)?,
                favorites_count: row.get(3)?,
                watched_count: row.get(4)?,
                average_rating: row.get(5)?,
                movie_count: row.get(6)?,
            })
        },
    )?;
    Ok(stats)
}

/// #10: 导出全部数据，不截断
pub fn export_data(conn: &Connection) -> Result<ExportData, AppError> {
    // 导出用：一次性取全部（大 page_size）
    let result = get_all_videos(conn, 0, i64::MAX)?;
    let watch_history = get_watch_history(conn, i64::MAX)?;
    Ok(ExportData {
        version: "1.0.0".to_string(),
        exported_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        videos: result.items,
        watch_history,
    })
}

/// 删除视频
pub fn delete_video(conn: &Connection, video_id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM videos WHERE id = ?1", params![video_id])?;
    Ok(())
}

/// 获取剧集概览（按 series_name 分组，含观看进度）
pub fn get_series_overview(conn: &Connection) -> Result<Vec<SeriesOverview>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT
            COALESCE(v.series_name, '') as series_name,
            COUNT(*) as total,
            SUM(CASE WHEN EXISTS (
                SELECT 1 FROM video_tags vt
                INNER JOIN tags t ON vt.tag_id = t.id
                WHERE vt.video_id = v.id AND t.name = '已看过'
            ) THEN 1 ELSE 0 END) as watched,
            COALESCE(SUM(v.file_size), 0),
            COALESCE(AVG(CASE WHEN v.rating > 0 THEN v.rating END), 0)
         FROM videos v
         WHERE v.video_type = 'episode'
         GROUP BY COALESCE(v.series_name, '')
         ORDER BY COALESCE(v.series_name, '')"
    )?;

    let series = stmt.query_map([], |row| {
        let name: String = row.get(0)?;
        let total: i64 = row.get(1)?;
        let watched: i64 = row.get(2)?;
        let total_size: i64 = row.get(3)?;
        let rating: f64 = row.get(4)?;
        let progress = if total > 0 { watched as f64 / total as f64 } else { 0.0 };
        Ok(SeriesOverview { name, total_episodes: total, watched_episodes: watched, progress, total_size, rating })
    })?.collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(series)
}

/// 获取某个剧集的所有剧集（按 SxxExx 或集数排序）
pub fn get_series_episodes(conn: &Connection, series_name: &str) -> Result<Vec<VideoWithTags>, AppError> {
    let sql = if series_name.is_empty() {
        // 空字符串剧集：匹配空字符串或 NULL
        format!(
            "SELECT
                v.id, v.title, v.file_path, v.file_name, v.file_size, v.duration, v.rating,
                v.is_favorite, v.video_type, v.series_name, v.season, v.episode, v.thumbnail_path,
                v.last_watched_at, v.watch_progress, v.created_at, v.updated_at,
                GROUP_CONCAT(t.id || '\x1F' || t.name || '\x1F' || t.color, '\x1E')
             FROM videos v
             LEFT JOIN video_tags vt ON v.id = vt.video_id
             LEFT JOIN tags t ON vt.tag_id = t.id
             WHERE (v.series_name = '' OR v.series_name IS NULL) AND v.video_type = 'episode'
             GROUP BY v.id
             ORDER BY COALESCE(v.season, 1), COALESCE(v.episode, 999999)"
        )
    } else {
        format!(
            "SELECT
                v.id, v.title, v.file_path, v.file_name, v.file_size, v.duration, v.rating,
                v.is_favorite, v.video_type, v.series_name, v.season, v.episode, v.thumbnail_path,
                v.last_watched_at, v.watch_progress, v.created_at, v.updated_at,
                GROUP_CONCAT(t.id || '\x1F' || t.name || '\x1F' || t.color, '\x1E')
             FROM videos v
             LEFT JOIN video_tags vt ON v.id = vt.video_id
             LEFT JOIN tags t ON vt.tag_id = t.id
             WHERE v.series_name = ?1
             GROUP BY v.id
             ORDER BY COALESCE(v.season, 1), COALESCE(v.episode, 999999)"
        )
    };

    let mut stmt = conn.prepare(&sql)?;
    let videos = if series_name.is_empty() {
        stmt.query_map([], |row| {
            let video = row_to_video(row)?;
            let tags_str: Option<String> = row.get(17)?;
            let tags = parse_tags_string(tags_str);
            Ok(VideoWithTags { video, tags })
        })?.collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map(params![series_name], |row| {
            let video = row_to_video(row)?;
            let tags_str: Option<String> = row.get(17)?;
            let tags = parse_tags_string(tags_str);
            Ok(VideoWithTags { video, tags })
        })?.collect::<rusqlite::Result<Vec<_>>>()?
    };

    Ok(videos)
}

/// 批量标记剧集为已看过/未看过
pub fn mark_series_watched(conn: &Connection, series_name: &str, watched: bool) -> Result<i64, AppError> {
    let tag_name = if watched { "已看过" } else { "未看过" };
    let opposite = if watched { "未看过" } else { "已看过" };

    // 获取目标标签 ID
    let tag_id: i64 = conn.query_row(
        "SELECT id FROM tags WHERE name = ?1",
        params![tag_name],
        |r| r.get(0),
    )?;

    // 获取互斥标签 ID
    let opposite_id: Option<i64> = conn.query_row(
        "SELECT id FROM tags WHERE name = ?1",
        params![opposite],
        |r| r.get(0),
    ).ok();

    // 获取该剧集所有视频 ID
    let mut stmt = conn.prepare("SELECT id FROM videos WHERE series_name = ?1")?;
    let video_ids: Vec<i64> = stmt.query_map(params![series_name], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    let mut count = 0i64;
    for vid in &video_ids {
        // 移除互斥标签
        if let Some(oid) = opposite_id {
            let _ = conn.execute("DELETE FROM video_tags WHERE video_id = ?1 AND tag_id = ?2", params![vid, oid]);
        }
        // 添加目标标签
        let _ = conn.execute("INSERT OR IGNORE INTO video_tags (video_id, tag_id) VALUES (?1, ?2)", params![vid, tag_id]);
        count += 1;
    }

    Ok(count)
}

// ============================================
// 已扫描文件夹管理
// ============================================

/// 获取所有已扫描文件夹
pub fn get_scan_folders(conn: &Connection) -> Result<Vec<crate::models::ScanFolder>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, folder_path, video_count, last_scanned_at FROM scan_folders ORDER BY last_scanned_at DESC"
    )?;
    let folders = stmt.query_map([], |row| {
        Ok(crate::models::ScanFolder {
            id: row.get(0)?,
            folder_path: row.get(1)?,
            video_count: row.get(2)?,
            last_scanned_at: row.get(3)?,
        })
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(folders)
}

/// 添加或更新已扫描文件夹记录
pub fn upsert_scan_folder(conn: &Connection, folder_path: &str) -> Result<(), AppError> {
    // 统计该文件夹下的视频数量
    let pattern = format!("{}%", folder_path.replace('\\', "/").trim_end_matches('/'));
    let video_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM videos WHERE REPLACE(file_path, '\\', '/') LIKE ?1",
        params![pattern],
        |r| r.get(0),
    ).unwrap_or(0);

    conn.execute(
        "INSERT INTO scan_folders (folder_path, video_count, last_scanned_at)
         VALUES (?1, ?2, datetime('now','localtime'))
         ON CONFLICT(folder_path) DO UPDATE SET video_count = ?2, last_scanned_at = datetime('now','localtime')",
        params![folder_path, video_count],
    )?;
    Ok(())
}

/// 删除已扫描文件夹记录
pub fn delete_scan_folder(conn: &Connection, folder_id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM scan_folders WHERE id = ?1", params![folder_id])?;
    Ok(())
}
