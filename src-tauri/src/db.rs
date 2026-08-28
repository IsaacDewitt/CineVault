use rusqlite::{params, Connection, OptionalExtension, Row};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::models::*;

// ============================================
// 连接与初始化
// ============================================
pub struct DbConn(pub Mutex<Connection>);

impl DbConn {
    pub fn init(conn: Connection) -> Self {
        Self(Mutex::new(conn))
    }
}

const SCHEMA_VERSION: i64 = 1;

pub fn initialize_database(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA foreign_keys=ON;
         PRAGMA busy_timeout=5000;
         CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS series (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            folder_path TEXT,
            rating REAL NOT NULL DEFAULT 0,
            is_favorite INTEGER NOT NULL DEFAULT 0,
            last_watched_at TEXT,
            watch_progress REAL NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime'))
         );
         CREATE TABLE IF NOT EXISTS videos (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL,
            file_path TEXT NOT NULL UNIQUE,
            file_name TEXT NOT NULL,
            file_size INTEGER NOT NULL DEFAULT 0,
            duration REAL,
            rating REAL NOT NULL DEFAULT 0,
            is_favorite INTEGER NOT NULL DEFAULT 0,
            kind TEXT NOT NULL DEFAULT 'unknown',
            confidence INTEGER NOT NULL DEFAULT 0,
            user_kind TEXT,
            series_id INTEGER REFERENCES series(id) ON DELETE SET NULL,
            season INTEGER,
            episode INTEGER,
            last_watched_at TEXT,
            watch_progress REAL NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now','localtime')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now','localtime'))
         );
         CREATE INDEX IF NOT EXISTS idx_videos_kind ON videos(kind);
         CREATE INDEX IF NOT EXISTS idx_videos_series ON videos(series_id);
         CREATE INDEX IF NOT EXISTS idx_videos_file_name ON videos(file_name);
         CREATE INDEX IF NOT EXISTS idx_videos_file_path ON videos(file_path);
         -- PotPlayer 同步按路径匹配时用的是 COLLATE NOCASE，普通 BINARY 索引在那条查询上不生效
         CREATE INDEX IF NOT EXISTS idx_videos_file_path_nocase ON videos(file_path COLLATE NOCASE);
         CREATE TABLE IF NOT EXISTS tags (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            color TEXT NOT NULL DEFAULT '#e8b34b',
            sort_order INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE IF NOT EXISTS video_tags (
            video_id INTEGER NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
            tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
            PRIMARY KEY (video_id, tag_id)
         );
         CREATE TABLE IF NOT EXISTS series_tags (
            series_id INTEGER NOT NULL REFERENCES series(id) ON DELETE CASCADE,
            tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
            PRIMARY KEY (series_id, tag_id)
         );
         CREATE TABLE IF NOT EXISTS watch_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            video_id INTEGER NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
            watched_at TEXT NOT NULL DEFAULT (datetime('now','localtime')),
            progress REAL NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS idx_watch_history_video ON watch_history(video_id);
         -- 去重索引：让各处 INSERT OR IGNORE 真正生效（同一视频同一时刻同一进度只留一条）。
         -- 必须在这里建而不能只靠 migrator，否则全新库会退化成每条都插，历史被同一部片子刷屏。
         -- 建索引前先清存量重复（老库/导入场景可能已有），否则 CREATE UNIQUE INDEX 会直接失败。
         DELETE FROM watch_history WHERE id NOT IN (
            SELECT MIN(id) FROM watch_history GROUP BY video_id, watched_at, progress
         );
         CREATE UNIQUE INDEX IF NOT EXISTS idx_watch_history_dedup
            ON watch_history(video_id, watched_at, progress);
         CREATE TABLE IF NOT EXISTS scan_folders (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            folder_path TEXT NOT NULL UNIQUE,
            last_scanned_at TEXT
         );
         CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT
         );",
    )?;

    let v: Option<i64> = conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| r.get(0))
        .optional()?;
    if v.is_none() {
        conn.execute("INSERT INTO schema_version (version) VALUES (?1)", params![SCHEMA_VERSION])?;
    }
    Ok(())
}

// ============================================
// 设置
// ============================================
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>, AppError> {
    let v = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0))
        .optional()?;
    Ok(v)
}

// ============================================
// 行映射
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
        kind: row.get(8)?,
        confidence: row.get(9)?,
        user_kind: row.get(10)?,
        series_id: row.get(11)?,
        series_name: row.get(12)?,
        season: row.get(13)?,
        episode: row.get(14)?,
        last_watched_at: row.get(15)?,
        watch_progress: row.get(16)?,
        created_at: row.get(17)?,
        updated_at: row.get(18)?,
    })
}

const VIDEO_COLS: &str = "v.id, v.title, v.file_path, v.file_name, v.file_size, v.duration, v.rating, v.is_favorite, v.kind, v.confidence, v.user_kind, v.series_id, s.name, v.season, v.episode, v.last_watched_at, v.watch_progress, v.created_at, v.updated_at";

fn row_to_series_overview(row: &Row) -> rusqlite::Result<SeriesOverview> {
    Ok(SeriesOverview {
        id: row.get(0)?,
        name: row.get(1)?,
        folder_path: row.get(2)?,
        total_episodes: row.get(3)?,
        watched_episodes: row.get(4)?,
        progress: row.get(5)?,
        total_size: row.get(6)?,
        rating: row.get(7)?,
        is_favorite: row.get::<_, i32>(8)? != 0,
        last_watched_at: row.get(9)?,
        updated_at: row.get(10)?,
        tags: Vec::new(),
    })
}

/// 一集算"看过"的判定：watch_progress >= 99.9，与前端 VideoList.watched() 保持一致。
/// 剧集聚合（已看集数 / 进度）必须同源，否则卡片会同时显示"已看 1/46 集"和"进度条 100%"——
/// 旧实现 progress 取 MAX(单集进度)，看 1 集就满格，与旁边的集数自相矛盾。
fn series_progress_expr(series_alias: &str) -> String {
    format!(
        "(SELECT CASE WHEN COUNT(*) = 0 THEN 0.0 \
         ELSE 100.0 * SUM(CASE WHEN v.watch_progress >= 99.9 THEN 1 ELSE 0 END) / COUNT(*) END \
         FROM videos v WHERE v.series_id = {}.id)",
        series_alias
    )
}

fn series_overview_cols() -> String {
    format!(
        "se.id, se.name, se.folder_path,
         (SELECT COUNT(*) FROM videos v WHERE v.series_id = se.id) AS total_episodes,
         (SELECT COUNT(*) FROM videos v WHERE v.series_id = se.id AND v.watch_progress >= 99.9) AS watched_episodes,
         {} AS progress,
         (SELECT COALESCE(SUM(v.file_size), 0) FROM videos v WHERE v.series_id = se.id) AS total_size,
         se.rating, se.is_favorite, se.last_watched_at, se.updated_at",
        series_progress_expr("se")
    )
}

// ============================================
// 视频 CRUD（含移动匹配迁移）
// ============================================
/// upsert_video：
/// 1. file_path 精确命中 → 更新
/// 2. 未命中但 file_name 唯一命中（且新路径在磁盘上已存在）→ 移动迁移，保留标记
/// 3. 都无 → 插入
pub fn upsert_video(
    tx: &rusqlite::Transaction,
    p: &VideoUpsertParams,
    move_match: bool,
) -> Result<UpsertResult, AppError> {
    // 1. 精确路径
    if let Ok(row) = tx.query_row(
        "SELECT id FROM videos WHERE file_path = ?1",
        params![p.file_path],
        |r| r.get::<_, i64>(0),
    ) {
        tx.execute(
            "UPDATE videos SET title=?1, file_size=?2, duration=COALESCE(duration, ?3),
             kind = CASE WHEN user_kind IS NOT NULL THEN user_kind ELSE ?4 END,
             confidence=?5, series_id=?6, season=?7, episode=?8,
             updated_at=datetime('now','localtime') WHERE id=?9",
            params![p.title, p.file_size, p.duration, p.kind, p.confidence, p.series_id, p.season, p.episode, row],
        )?;
        return Ok(UpsertResult::Updated(row));
    }

    // 2. 移动匹配（两级，旧文件已不在磁盘才认领）：
    //    2a. 剧集锚点：新文件带剧集归属时，只在本剧集内找同名（文件夹=剧集，杜绝跨剧集重名串记录）
    //    2b. 全库唯一同名：无歧义时认领，保留历史 series_id 归属（电影移动 / 剧集文件夹改名场景）
    // 注意：rusqlite 的 params![] 是按**位置顺序**绑定到 1..n，不是按 SQL 里写的 ?N 编号。
    // 因此 SQL 中的参数序号必须连续；跳号（如 ?1?2?3?5）会让 bind_parameter_count 大于实参个数，
    // 直接抛 InvalidParameterCount。改动这两条 UPDATE 时务必重编号。
    if move_match {
        if let Some(sid) = p.series_id {
            let hit: Option<(i64, String)> = tx
                .query_row(
                    "SELECT id, file_path FROM videos WHERE file_name = ?1 AND series_id = ?2 ORDER BY id LIMIT 1",
                    params![p.file_name, sid],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((old_id, old_path)) = hit {
                if old_path != p.file_path && !std::path::Path::new(&old_path).exists() {
                    tx.execute(
                        "UPDATE videos SET file_path=?1, title=?2, file_size=?3, series_id=?4,
                         season=COALESCE(?5, season), episode=COALESCE(?6, episode),
                         user_kind=COALESCE(user_kind, ?7),
                         updated_at=datetime('now','localtime') WHERE id=?8",
                        params![p.file_path, p.title, p.file_size, sid, p.season, p.episode, p.user_kind, old_id],
                    )?;
                    return Ok(UpsertResult::Updated(old_id));
                }
            }
        }
        // 2b. 全库唯一同名才认领（多条同名 = 有歧义，宁插入不串记录）
        let same_name: Vec<(i64, String)> = {
            let mut stmt = tx.prepare(
                "SELECT id, file_path FROM videos WHERE file_name = ?1 ORDER BY id",
            )?;
            let rows = stmt.query_map(params![p.file_name], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
            let mut v = Vec::new();
            for r in rows {
                if let Ok(x) = r {
                    v.push(x);
                }
            }
            v
        };
        if same_name.len() == 1 {
            let (old_id, old_path) = same_name[0].clone();
            if old_path != p.file_path && !std::path::Path::new(&old_path).exists() {
                tx.execute(
                    "UPDATE videos SET file_path=?1, title=?2, file_size=?3,
                     season=COALESCE(?4, season), episode=COALESCE(?5, episode),
                     user_kind=COALESCE(user_kind, ?6),
                     updated_at=datetime('now','localtime') WHERE id=?7",
                    params![p.file_path, p.title, p.file_size, p.season, p.episode, p.user_kind, old_id],
                )?;
                return Ok(UpsertResult::Updated(old_id));
            }
        }
    }

    // 3. 插入
    tx.execute(
        "INSERT INTO videos (title, file_path, file_name, file_size, duration, kind, confidence, user_kind, series_id, season, episode, last_watched_at, watch_progress)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            p.title, p.file_path, p.file_name, p.file_size, p.duration,
            p.kind, p.confidence, p.user_kind, p.series_id, p.season, p.episode,
            p.default_watched.then(|| chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()),
            if p.default_watched { 100.0 } else { 0.0 }
        ],
    )?;
    let id = tx.last_insert_rowid();
    if p.default_watched {
        tx.execute(
            "INSERT INTO watch_history (video_id, watched_at, progress) VALUES (?1, datetime('now','localtime'), 100)",
            params![id],
        )?;
    }
    Ok(UpsertResult::Inserted(id))
}

// ============================================
// 分页查询
// ============================================
fn load_tags_for(conn: &Connection, video_ids: &[i64]) -> Result<std::collections::HashMap<i64, Vec<Tag>>, AppError> {
    let mut map: std::collections::HashMap<i64, Vec<Tag>> = std::collections::HashMap::new();
    if video_ids.is_empty() {
        return Ok(map);
    }
    let placeholders: Vec<String> = video_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT vt.video_id, t.id, t.name, t.color FROM video_tags vt
         JOIN tags t ON t.id = vt.tag_id
         WHERE vt.video_id IN ({}) ORDER BY t.sort_order, t.name",
        placeholders.join(",")
    );
    let mut stmt = conn.prepare(&sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> =
        video_ids.iter().map(|v| v as &dyn rusqlite::types::ToSql).collect();
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        Ok((row.get::<_, i64>(0)?, Tag { id: row.get(1)?, name: row.get(2)?, color: row.get(3)?, video_count: None }))
    })?;
    for r in rows {
        if let Ok((vid, tag)) = r {
            map.entry(vid).or_default().push(tag);
        }
    }
    Ok(map)
}

pub fn query_videos_paginated(
    conn: &Connection,
    conditions: &[String],
    cond_params: &[&dyn rusqlite::types::ToSql],
    page: i64,
    page_size: i64,
    min_rating: f64,
    sort: &str,
) -> Result<PaginatedVideos, AppError> {
    let where_sql = if conditions.is_empty() {
        "WHERE (v.kind = 'movie' OR v.kind = 'unknown' OR v.kind = 'series')".to_string()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };
    let where_sql = if min_rating > 0.0 {
        format!("{} AND v.rating >= {}", where_sql, min_rating)
    } else {
        where_sql
    };
    let order = match sort {
        "name" => "ORDER BY v.title COLLATE NOCASE, v.id",
        "size" => "ORDER BY v.file_size DESC, v.id",
        "rating" => "ORDER BY v.rating DESC, v.id",
        "updated" => "ORDER BY v.updated_at DESC, v.id",
        _ => "ORDER BY COALESCE(v.last_watched_at, '') DESC, v.id",
    };

    let total: i64 = {
        let sql = format!("SELECT COUNT(*) FROM videos v LEFT JOIN series s ON s.id = v.series_id {}", where_sql);
        let mut stmt = conn.prepare(&sql)?;
        stmt.query_row(cond_params, |r| r.get(0))?
    };

    let sql = format!(
        "SELECT {} FROM videos v LEFT JOIN series s ON s.id = v.series_id {} {} LIMIT ?{} OFFSET ?{}",
        VIDEO_COLS, where_sql, order,
        cond_params.len() + 1, cond_params.len() + 2
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    for p in cond_params {
        params_vec.push(Box::new(*p));
    }
    params_vec.push(Box::new(page_size));
    params_vec.push(Box::new(page * page_size));
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();

    let rows = stmt.query_map(param_refs.as_slice(), row_to_video)?;
    let mut videos = Vec::new();
    let mut ids = Vec::new();
    for r in rows {
        let v = r?;
        ids.push(v.id);
        videos.push(v);
    }
    let tag_map = load_tags_for(conn, &ids)?;
    let items = videos
        .into_iter()
        .map(|video| VideoWithTags {
            tags: tag_map.get(&video.id).cloned().unwrap_or_default(),
            video,
        })
        .collect();

    Ok(PaginatedVideos { items, total, page, page_size, has_more: (page + 1) * page_size < total })
}

pub fn get_recently_added_videos(
    conn: &Connection,
    page: i64,
    page_size: i64,
    min_rating: f64,
) -> Result<PaginatedVideos, AppError> {
    // min_rating 由 query_videos_paginated 内部追加，这里不要再 push 一次（否则 SQL 出现重复条件）
    let conditions = vec!["datetime(v.created_at) >= datetime('now', '-7 days')".to_string()];
    let params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    query_videos_paginated(conn, &conditions, &param_refs, page, page_size, min_rating, "updated")
}

pub fn get_video_by_id(conn: &Connection, video_id: i64) -> Result<VideoWithTags, AppError> {
    let video = conn
        .query_row(
            &format!("SELECT {} FROM videos v LEFT JOIN series s ON s.id = v.series_id WHERE v.id = ?1", VIDEO_COLS),
            params![video_id],
            row_to_video,
        )?;
    let tags = load_tags_for(conn, &[video_id])?.remove(&video_id).unwrap_or_default();
    Ok(VideoWithTags { video, tags })
}

// ============================================
// 剧集
// ============================================
pub fn upsert_series(conn: &Connection, name: &str, folder: Option<&str>) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO series (name, folder_path) VALUES (?1, ?2)
         ON CONFLICT(name) DO UPDATE SET folder_path = COALESCE(excluded.folder_path, series.folder_path)",
        params![name, folder],
    )?;
    Ok(conn.query_row("SELECT id FROM series WHERE name = ?1", params![name], |r| r.get(0))?)
}

pub fn get_series_overview(conn: &Connection) -> Result<Vec<SeriesOverview>, AppError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM series se ORDER BY se.name COLLATE NOCASE",
        series_overview_cols()
    ))?;
    let rows = stmt.query_map([], row_to_series_overview)?;
    let mut list = Vec::new();
    let mut ids = Vec::new();
    for r in rows {
        let mut so = r?;
        ids.push(so.id);
        so.tags = Vec::new();
        list.push(so);
    }
    // 剧集标签
    let mut tag_map: std::collections::HashMap<i64, Vec<Tag>> = std::collections::HashMap::new();
    if !ids.is_empty() {
        let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "SELECT st.series_id, t.id, t.name, t.color FROM series_tags st
             JOIN tags t ON t.id = st.tag_id WHERE st.series_id IN ({}) ORDER BY t.sort_order, t.name",
            placeholders.join(",")
        );
        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            ids.iter().map(|v| v as &dyn rusqlite::types::ToSql).collect();
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok((row.get::<_, i64>(0)?, Tag { id: row.get(1)?, name: row.get(2)?, color: row.get(3)?, video_count: None }))
        })?;
        for r in rows {
            if let Ok((sid, tag)) = r {
                tag_map.entry(sid).or_default().push(tag);
            }
        }
    }
    for so in list.iter_mut() {
        if let Some(tags) = tag_map.remove(&so.id) {
            so.tags = tags;
        }
    }
    Ok(list)
}

pub fn get_series_episodes(conn: &Connection, series_id: i64) -> Result<Vec<VideoWithTags>, AppError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM videos v LEFT JOIN series s ON s.id = v.series_id
         WHERE v.series_id = ?1
         ORDER BY COALESCE(v.season, 1), COALESCE(v.episode, 0), v.file_name",
        VIDEO_COLS
    ))?;
    let rows = stmt.query_map(params![series_id], row_to_video)?;
    let mut videos = Vec::new();
    let mut ids = Vec::new();
    for r in rows {
        let v = r?;
        ids.push(v.id);
        videos.push(v);
    }
    let tag_map = load_tags_for(conn, &ids)?;
    Ok(videos
        .into_iter()
        .map(|video| VideoWithTags {
            tags: tag_map.get(&video.id).cloned().unwrap_or_default(),
            video,
        })
        .collect())
}

// ============================================
// 操作
// ============================================
pub fn update_rating(conn: &Connection, video_id: i64, rating: f64) -> Result<(), AppError> {
    conn.execute(
        "UPDATE videos SET rating = ?1, updated_at = datetime('now','localtime') WHERE id = ?2",
        params![rating, video_id],
    )?;
    Ok(())
}

pub fn toggle_favorite(conn: &Connection, video_id: i64) -> Result<bool, AppError> {
    conn.execute(
        "UPDATE videos SET is_favorite = 1 - is_favorite, updated_at = datetime('now','localtime') WHERE id = ?1",
        params![video_id],
    )?;
    let fav: i32 = conn.query_row(
        "SELECT is_favorite FROM videos WHERE id = ?1",
        params![video_id],
        |r| r.get(0),
    )?;
    Ok(fav != 0)
}

pub fn update_watch_progress(conn: &Connection, video_id: i64, progress: f64) -> Result<(), AppError> {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    conn.execute(
        "UPDATE videos SET watch_progress = ?1, last_watched_at = ?2, updated_at = datetime('now','localtime') WHERE id = ?3",
        params![progress, now, video_id],
    )?;
    conn.execute(
        "INSERT INTO watch_history (video_id, watched_at, progress) VALUES (?1, ?2, ?3)",
        params![video_id, now, progress],
    )?;
    // 每个视频最多保留最近 50 条历史，防 DB 无限膨胀
    trim_history(conn, video_id)?;
    // 剧集进度同步到实体
    if let Some(sid) = conn
        .query_row("SELECT series_id FROM videos WHERE id = ?1", params![video_id], |r| r.get::<_, Option<i64>>(0))
        .optional()?
        .flatten()
    {
        conn.execute(
            &format!(
                "UPDATE series SET last_watched_at = ?1, watch_progress = {}, updated_at = datetime('now','localtime') WHERE id = ?2",
                series_progress_expr("series")
            ),
            params![now, sid],
        )?;
    }
    Ok(())
}

/// 标记"刚刚点击播放"：只刷新最近播放时间 + 写一条观看历史，不动 watch_progress 百分比。
/// 这是"从 CineVault 点播放 → 立即成为最近观看"的最可靠记录时机，不依赖播放器状态。
pub fn mark_played(conn: &Connection, video_id: i64) -> Result<(), AppError> {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    conn.execute(
        "UPDATE videos SET last_watched_at = ?1, updated_at = datetime('now','localtime') WHERE id = ?2",
        params![now, video_id],
    )?;
    // 历史记录里记当前进度，表示"点播放时看到哪"
    let prog: f64 = conn
        .query_row("SELECT watch_progress FROM videos WHERE id = ?1", params![video_id], |r| r.get(0))
        .unwrap_or(0.0);
    conn.execute(
        "INSERT OR IGNORE INTO watch_history (video_id, watched_at, progress) VALUES (?1, ?2, ?3)",
        params![video_id, now, prog],
    )?;
    trim_history(conn, video_id)?;
    sync_series_aggregate(conn, video_id)?;
    Ok(())
}

/// 每个视频的 watch_history 最多保留最近 50 条，防 DB 无限膨胀
pub fn trim_history(conn: &Connection, video_id: i64) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM watch_history WHERE id IN (
            SELECT id FROM watch_history WHERE video_id = ?1 ORDER BY id DESC LIMIT -1 OFFSET 50
        )",
        params![video_id],
    )?;
    Ok(())
}

/// 视频进度/最近播放变化后，把剧集聚合字段刷到最新（剧集卡片排序/进度条数据源）
pub fn sync_series_aggregate(conn: &Connection, video_id: i64) -> Result<(), AppError> {
    if let Some(sid) = conn
        .query_row("SELECT series_id FROM videos WHERE id = ?1", params![video_id], |r| r.get::<_, Option<i64>>(0))
        .optional()?
        .flatten()
    {
        conn.execute(
            &format!(
                "UPDATE series SET
                    last_watched_at = (SELECT MAX(v.last_watched_at) FROM videos v WHERE v.series_id = series.id),
                    watch_progress = {},
                    updated_at = datetime('now','localtime')
                 WHERE id = ?1",
                series_progress_expr("series")
            ),
            params![sid],
        )?;
    }
    Ok(())
}

/// 最近看过的片子（UI 用）：按 video_id 去重，每个视频只保留最近一条，并带回完整视频数据。
/// 不去重的话同一部片子反复播放会在列表里刷屏（生产库曾出现单视频 6 条、平均 2 条/视频）。
pub fn get_history_entries(conn: &Connection, limit: i64) -> Result<Vec<HistoryEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT video_id, watched_at, progress FROM (
            SELECT video_id, watched_at, progress,
                   ROW_NUMBER() OVER (PARTITION BY video_id ORDER BY watched_at DESC, id DESC) AS rn
            FROM watch_history
         ) WHERE rn = 1
         ORDER BY watched_at DESC, video_id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, f64>(2)?))
    })?;
    let mut picked: Vec<(i64, String, f64)> = Vec::new();
    for r in rows {
        if let Ok(x) = r {
            picked.push(x);
        }
    }

    let ids: Vec<i64> = picked.iter().map(|(id, _, _)| *id).collect();
    let mut videos: HashMap<i64, VideoWithTags> = HashMap::new();
    if !ids.is_empty() {
        let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "SELECT {} FROM videos v LEFT JOIN series s ON s.id = v.series_id WHERE v.id IN ({})",
            VIDEO_COLS,
            placeholders.join(",")
        );
        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            ids.iter().map(|v| v as &dyn rusqlite::types::ToSql).collect();
        let rows = stmt.query_map(refs.as_slice(), row_to_video)?;
        for r in rows {
            if let Ok(v) = r {
                videos.insert(v.id, VideoWithTags { video: v, tags: Vec::new() });
            }
        }
    }
    let tag_map = load_tags_for(conn, &ids)?;

    Ok(picked
        .into_iter()
        .filter_map(|(vid, watched_at, progress)| {
            videos.remove(&vid).map(|mut vwt| {
                vwt.tags = tag_map.get(&vid).cloned().unwrap_or_default();
                HistoryEntry { video: vwt, watched_at, progress }
            })
        })
        .collect())
}

pub fn get_watch_history(conn: &Connection, limit: i64) -> Result<Vec<WatchHistory>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT h.id, h.video_id, h.watched_at, h.progress, v.title, v.file_path
         FROM watch_history h LEFT JOIN videos v ON v.id = h.video_id
         ORDER BY h.watched_at DESC, h.id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |r| {
        Ok(WatchHistory {
            id: r.get(0)?,
            video_id: r.get(1)?,
            watched_at: r.get(2)?,
            progress: r.get(3)?,
            video_title: r.get(4)?,
            video_path: r.get(5)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

// ============================================
// 标签
// ============================================
pub fn get_all_tags(conn: &Connection) -> Result<Vec<Tag>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.name, t.color, (SELECT COUNT(*) FROM video_tags vt WHERE vt.tag_id = t.id) + (SELECT COUNT(*) FROM series_tags st WHERE st.tag_id = t.id) AS cnt
         FROM tags t ORDER BY t.sort_order, t.name",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Tag { id: r.get(0)?, name: r.get(1)?, color: r.get(2)?, video_count: Some(r.get(3)?) })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn create_tag(conn: &Connection, name: &str, color: &str) -> Result<Tag, AppError> {
    conn.execute("INSERT INTO tags (name, color) VALUES (?1, ?2)", params![name, color])?;
    Ok(Tag {
        id: conn.last_insert_rowid(),
        name: name.to_string(),
        color: color.to_string(),
        video_count: Some(0),
    })
}

pub fn update_tag(conn: &Connection, tag_id: i64, name: Option<&str>, color: Option<&str>) -> Result<(), AppError> {
    match (name, color) {
        (Some(n), Some(c)) => {
            conn.execute("UPDATE tags SET name = ?1, color = ?2 WHERE id = ?3", params![n, c, tag_id])?;
        }
        (Some(n), None) => {
            conn.execute("UPDATE tags SET name = ?1 WHERE id = ?2", params![n, tag_id])?;
        }
        (None, Some(c)) => {
            conn.execute("UPDATE tags SET color = ?1 WHERE id = ?2", params![c, tag_id])?;
        }
        (None, None) => {}
    }
    Ok(())
}

pub fn delete_tag(conn: &Connection, tag_id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM tags WHERE id = ?1", params![tag_id])?;
    Ok(())
}

pub fn add_tag_to_video(conn: &Connection, video_id: i64, tag_id: i64) -> Result<(), AppError> {
    conn.execute(
        "INSERT OR IGNORE INTO video_tags (video_id, tag_id) VALUES (?1, ?2)",
        params![video_id, tag_id],
    )?;
    Ok(())
}

pub fn remove_tag_from_video(conn: &Connection, video_id: i64, tag_id: i64) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM video_tags WHERE video_id = ?1 AND tag_id = ?2",
        params![video_id, tag_id],
    )?;
    Ok(())
}

pub fn add_series_tag(conn: &Connection, series_id: i64, tag_id: i64) -> Result<(), AppError> {
    conn.execute(
        "INSERT OR IGNORE INTO series_tags (series_id, tag_id) VALUES (?1, ?2)",
        params![series_id, tag_id],
    )?;
    Ok(())
}

pub fn remove_series_tag(conn: &Connection, series_id: i64, tag_id: i64) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM series_tags WHERE series_id = ?1 AND tag_id = ?2",
        params![series_id, tag_id],
    )?;
    Ok(())
}

// ============================================
// 待确认队列
// ============================================
pub fn get_pending_videos(conn: &Connection) -> Result<Vec<PendingItem>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT v.id, v.title, v.file_path, v.confidence, v.kind FROM videos v
         WHERE v.user_kind IS NULL AND v.kind = 'unknown'
         ORDER BY v.confidence DESC, v.title",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(PendingItem {
            video_id: r.get(0)?,
            title: r.get(1)?,
            file_path: r.get(2)?,
            confidence: r.get(3)?,
            suggested: r.get(4)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 无剧集名时用父文件夹名兜底；都没有才建"未命名剧集"（避免留下空壳）
fn series_name_from_folder(tx: &rusqlite::Transaction, video_id: i64) -> Result<i64, AppError> {
    let path: String = tx.query_row(
        "SELECT file_path FROM videos WHERE id = ?1",
        params![video_id],
        |r| r.get(0),
    )?;
    if let Some(n) = std::path::Path::new(&path)
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        return upsert_series(tx, n, None);
    }
    upsert_series(tx, "未命名剧集", None)
}

/// 用户裁决待确认项：kind = movie/series，series_name 可选（仅 series）
pub fn resolve_pending(
    conn: &Connection,
    video_id: i64,
    kind: &str,
    series_name: Option<&str>,
) -> Result<(), AppError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE videos SET user_kind = ?1, kind = ?1, updated_at = datetime('now','localtime') WHERE id = ?2",
        params![kind, video_id],
    )?;
    if kind == "series" {
        let sid = match series_name.map(str::trim).filter(|s| !s.is_empty()) {
            Some(name) => upsert_series(&tx, name, None)?,
            None => series_name_from_folder(&tx, video_id)?,
        };
        tx.execute(
            "UPDATE videos SET series_id = ?1, updated_at = datetime('now','localtime') WHERE id = ?2",
            params![sid, video_id],
        )?;
    } else {
        tx.execute(
            "UPDATE videos SET series_id = NULL WHERE id = ?1",
            params![video_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

// ============================================
// 删除 / 重命名
// ============================================
pub fn delete_video(conn: &Connection, video_id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM videos WHERE id = ?1", params![video_id])?;
    Ok(())
}

pub fn delete_series(conn: &Connection, series_id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM videos WHERE series_id = ?1", params![series_id])?;
    conn.execute("DELETE FROM series WHERE id = ?1", params![series_id])?;
    Ok(())
}

pub fn rename_video(conn: &Connection, video_id: i64, new_title: String) -> Result<(), AppError> {
    conn.execute(
        "UPDATE videos SET title = ?1, updated_at = datetime('now','localtime') WHERE id = ?2",
        params![new_title, video_id],
    )?;
    Ok(())
}

pub fn rename_series(conn: &Connection, series_id: i64, new_name: String) -> Result<(), AppError> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err(AppError::validation("剧集名不能为空"));
    }
    conn.execute(
        "UPDATE series SET name = ?1, updated_at = datetime('now','localtime') WHERE id = ?2",
        params![new_name, series_id],
    )?;
    Ok(())
}

/// 全剧标记已看/未看（同步所有单集进度 + 剧集实体）
pub fn mark_series_watched(conn: &Connection, series_id: i64, watched: bool) -> Result<i64, AppError> {
    let p: f64 = if watched { 100.0 } else { 0.0 };
    let count = conn.execute(
        "UPDATE videos SET watch_progress = ?1, updated_at = datetime('now','localtime') WHERE series_id = ?2",
        params![p, series_id],
    )?;
    conn.execute(
        "UPDATE series SET watch_progress = ?1, updated_at = datetime('now','localtime') WHERE id = ?2",
        params![p, series_id],
    )?;
    Ok(count as i64)
}

// ============================================
// 统计
// ============================================
pub fn get_stats(conn: &Connection) -> Result<VideoStats, AppError> {
    let total_videos: i64 = conn.query_row("SELECT COUNT(*) FROM videos", [], |r| r.get(0))?;
    let total_series: i64 = conn.query_row("SELECT COUNT(*) FROM series", [], |r| r.get(0))?;
    let total_size: i64 = conn.query_row("SELECT COALESCE(SUM(file_size), 0) FROM videos", [], |r| r.get(0))?;
    let favorites_count: i64 = conn.query_row("SELECT COUNT(*) FROM videos WHERE is_favorite = 1", [], |r| r.get(0))?;
    let watched_count: i64 = conn.query_row("SELECT COUNT(DISTINCT video_id) FROM watch_history", [], |r| r.get(0))?;
    let average_rating: f64 = conn.query_row("SELECT COALESCE(AVG(rating), 0) FROM videos WHERE rating > 0", [], |r| r.get(0))?;
    let movie_count: i64 = conn.query_row("SELECT COUNT(*) FROM videos WHERE kind = 'movie'", [], |r| r.get(0))?;
    Ok(VideoStats {
        total_videos,
        total_series,
        total_size,
        favorites_count,
        watched_count,
        average_rating,
        movie_count,
    })
}

// ============================================
// 扫描目录
// ============================================
pub fn upsert_scan_folder(conn: &Connection, folder_path: &str) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO scan_folders (folder_path, last_scanned_at) VALUES (?1, datetime('now','localtime'))
         ON CONFLICT(folder_path) DO UPDATE SET last_scanned_at = datetime('now','localtime')",
        params![folder_path],
    )?;
    Ok(())
}

pub fn get_scan_folders(conn: &Connection) -> Result<Vec<ScanFolder>, AppError> {
    let mut stmt = conn.prepare("SELECT id, folder_path, last_scanned_at FROM scan_folders ORDER BY folder_path")?;
    let rows = stmt.query_map([], |r| {
        Ok(ScanFolder { id: r.get(0)?, folder_path: r.get(1)?, last_scanned_at: r.get(2)? })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn delete_scan_folder(conn: &Connection, folder_id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM scan_folders WHERE id = ?1", params![folder_id])?;
    Ok(())
}

// ============================================
// 导出 / 导入（JSON 分享格式）
// ============================================
pub fn export_data(conn: &Connection) -> Result<ExportData, AppError> {
    // 全量视频 + 标签
    let videos: Vec<VideoWithTags> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM videos v LEFT JOIN series s ON s.id = v.series_id ORDER BY v.id",
            VIDEO_COLS
        ))?;
        let rows = stmt.query_map([], row_to_video)?;
        let mut ids = Vec::new();
        let mut raw: Vec<Video> = Vec::new();
        for r in rows {
            let v = r?;
            ids.push(v.id);
            raw.push(v);
        }
        let tag_map = load_tags_for(conn, &ids)?;
        raw.into_iter()
            .map(|video| VideoWithTags {
                tags: tag_map.get(&video.id).cloned().unwrap_or_default(),
                video,
            })
            .collect()
    };

    let series = get_series_overview(conn)?;
    let tags = get_all_tags(conn)?;
    let watch_history = get_watch_history(conn, i64::MAX)?;
    let scan_folders = get_scan_folders(conn)?;

    Ok(ExportData {
        app: "cinevault".into(),
        version: "1.0".into(),
        exported_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        videos,
        series,
        tags,
        watch_history,
        scan_folders,
    })
}

/// 导入 JSON（upsert 语义：同名剧集/标签合并，同路径视频更新）
pub fn import_data(conn: &Connection, data: &ExportData) -> Result<ImportResult, AppError> {
    let tx = conn.unchecked_transaction()?;

    // 1. 扫描目录
    let mut folders = 0i64;
    for f in &data.scan_folders {
        tx.execute(
            "INSERT OR IGNORE INTO scan_folders (folder_path, last_scanned_at) VALUES (?1, ?2)",
            params![f.folder_path, f.last_scanned_at],
        )?;
        folders += 1;
    }

    // 2. 标签（name 唯一）
    let mut tag_map: HashMap<String, i64> = HashMap::new();
    let mut tags = 0i64;
    for t in &data.tags {
        tx.execute(
            "INSERT INTO tags (name, color) VALUES (?1, ?2)
             ON CONFLICT(name) DO UPDATE SET color = excluded.color",
            params![t.name, t.color],
        )?;
        let id: i64 = tx.query_row("SELECT id FROM tags WHERE name = ?1", params![t.name], |r| r.get(0))?;
        tag_map.insert(t.name.clone(), id);
        tags += 1;
    }

    // 3. 剧集（name 唯一，保留实体元数据）
    let mut series_id_by_name: HashMap<String, i64> = HashMap::new();
    let mut series_count = 0i64;
    for s in &data.series {
        tx.execute(
            "INSERT INTO series (name, folder_path, rating, is_favorite, last_watched_at, watch_progress)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(name) DO UPDATE SET
                rating = MAX(series.rating, excluded.rating),
                is_favorite = MAX(series.is_favorite, excluded.is_favorite),
                folder_path = COALESCE(series.folder_path, excluded.folder_path)",
            params![
                s.name,
                s.folder_path,
                s.rating,
                if s.is_favorite { 1 } else { 0 },
                s.last_watched_at,
                s.progress
            ],
        )?;
        let id: i64 = tx.query_row("SELECT id FROM series WHERE name = ?1", params![s.name], |r| r.get(0))?;
        series_id_by_name.insert(s.name.clone(), id);
        // 剧集标签
        for t in &s.tags {
            if let Some(tid) = tag_map.get(&t.name) {
                tx.execute(
                    "INSERT OR IGNORE INTO series_tags (series_id, tag_id) VALUES (?1, ?2)",
                    params![id, tid],
                )?;
            }
        }
        series_count += 1;
    }

    // 4. 视频（同路径 upsert，不启用移动匹配——导入来自另一台机器不应改写本地路径）
    let mut video_count = 0i64;
    // 导出库 video_id -> 本库 video_id。播放历史必须走这层映射：
    // id 是 AUTOINCREMENT，导出方的 123 号在本库是另一部片子，直接用会把历史挂错视频。
    let mut video_id_map: HashMap<i64, i64> = HashMap::new();
    for vwt in &data.videos {
        let v = &vwt.video;
        let series_id = v.series_name.as_ref().and_then(|n| series_id_by_name.get(n).copied());
        let result = upsert_video(
            &tx,
            &VideoUpsertParams {
                title: &v.title,
                file_path: &v.file_path,
                file_name: &v.file_name,
                file_size: v.file_size,
                duration: v.duration,
                kind: &v.kind,
                confidence: v.confidence,
                user_kind: v.user_kind.as_deref(),
                series_id,
                season: v.season,
                episode: v.episode,
                default_watched: false,
            },
            false,
        )?;
        let vid = result.id();
        video_id_map.insert(v.id, vid);
        // 评分/收藏/进度（导入值优先）
        tx.execute(
            "UPDATE videos SET rating = ?1, is_favorite = ?2, watch_progress = ?3, last_watched_at = COALESCE(last_watched_at, ?4)
             WHERE id = ?5",
            params![v.rating, if v.is_favorite { 1 } else { 0 }, v.watch_progress, v.last_watched_at, vid],
        )?;
        // 标签
        for t in &vwt.tags {
            if let Some(tid) = tag_map.get(&t.name) {
                tx.execute(
                    "INSERT OR IGNORE INTO video_tags (video_id, tag_id) VALUES (?1, ?2)",
                    params![vid, tid],
                )?;
            }
        }
        video_count += 1;
    }

    // 5. 播放历史（经 video_id_map 换算成本库 id 再写；映射不到说明视频未导入，丢弃）
    let mut history_count = 0i64;
    for h in &data.watch_history {
        if let Some(&vid) = video_id_map.get(&h.video_id) {
            tx.execute(
                "INSERT OR IGNORE INTO watch_history (video_id, watched_at, progress) VALUES (?1, ?2, ?3)",
                params![vid, h.watched_at, h.progress],
            )?;
            history_count += 1;
        }
    }

    tx.commit()?;

    Ok(ImportResult {
        imported_videos: video_count,
        imported_series: series_count,
        imported_tags: tags,
        imported_history: history_count,
        imported_folders: folders,
    })
}

// ============================================
// 工具
// ============================================
/// 全部视频路径（供增量扫描）
pub fn get_all_file_paths(conn: &Connection) -> Result<Vec<String>, AppError> {
    let mut stmt = conn.prepare("SELECT file_path FROM videos")?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    let mut out = Vec::new();
    for r in rows {
        if let Ok(v) = r {
            out.push(v);
        }
    }
    Ok(out)
}

pub fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        initialize_database(&conn).unwrap();
        conn
    }

    fn vp<'a>(title: &'a str, path: &'a str, name: &'a str) -> VideoUpsertParams<'a> {
        VideoUpsertParams {
            title,
            file_path: path,
            file_name: name,
            file_size: 1024,
            duration: None,
            kind: "movie",
            confidence: 80,
            user_kind: None,
            series_id: None,
            season: None,
            episode: None,
            default_watched: false,
        }
    }

    /// 回归：2b 分支的 UPDATE 曾写成 `?1?2?3?5?6?7?8`（删 series_id=?4 时漏重编号）却只传 7 个值，
    /// rusqlite 按位置绑定后抛 InvalidParameterCount(7,8)，命令层当成普通错误吞掉 →
    /// 移动过的电影重新扫描时既不认领也不插入，记录凭空消失。
    #[test]
    fn move_match_claims_orphan_record() {
        let mut conn = mem_db();
        let old = {
            let mut p = std::env::temp_dir();
            p.push("cinevault_test_never_created");
            p.push("Inception.2010.mkv");
            p.to_string_lossy().to_string()
        };
        assert!(
            !std::path::Path::new(&old).exists(),
            "旧路径必须不存在才会触发移动匹配"
        );

        let tx = conn.transaction().unwrap();
        let first = upsert_video(&tx, &vp("Inception.2010", &old, "Inception.2010.mkv"), false).unwrap();
        assert!(first.is_inserted());
        let id = first.id();
        tx.commit().unwrap();

        // 文件挪到新位置后重新 upsert：应认领旧记录，而不是插一条新的
        let new = "E:/电影房/Inception.2010.mkv";
        let tx = conn.transaction().unwrap();
        let second = upsert_video(&tx, &vp("Inception.2010", new, "Inception.2010.mkv"), true).unwrap();
        assert!(!second.is_inserted(), "应认领旧记录而非插入新行");
        assert_eq!(second.id(), id);
        tx.commit().unwrap();

        let stored: String = conn
            .query_row("SELECT file_path FROM videos WHERE id = ?1", params![id], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, new, "记录的 file_path 应迁移到新位置");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM videos WHERE file_name = 'Inception.2010.mkv'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "不应产生重复行");
    }

    /// 剧集进度 = 已看集数 / 总集数，与 watched_episodes 同源。
    /// 旧实现取 MAX(单集进度)：看 1/46 集就显示整剧 100%，与卡片上的"已看 1/46 集"自相矛盾。
    #[test]
    fn series_progress_tracks_watched_ratio() {
        let mut conn = mem_db();
        let tx = conn.transaction().unwrap();
        let sid = upsert_series(&tx, "测试剧", None).unwrap();
        let mut ids = Vec::new();
        for ep in 1..=4 {
            let p = VideoUpsertParams {
                title: "测试剧",
                file_path: &format!("/t/测试剧.S01E0{}.mkv", ep),
                file_name: &format!("测试剧.S01E0{}.mkv", ep),
                file_size: 100,
                duration: None,
                kind: "series",
                confidence: 90,
                user_kind: None,
                series_id: Some(sid),
                season: Some(1),
                episode: Some(ep),
                default_watched: false,
            };
            ids.push(upsert_video(&tx, &p, false).unwrap().id());
        }
        tx.commit().unwrap();

        let ratio = |conn: &Connection| -> (i64, i64, f64) {
            let list = get_series_overview(conn).unwrap();
            let s = &list[0];
            (s.watched_episodes, s.total_episodes, s.progress)
        };

        assert_eq!(ratio(&conn), (0, 4, 0.0), "都没看 → 0%");

        // 看完 1 集 → 25%，而不是 100%
        conn.execute("UPDATE videos SET watch_progress = 100 WHERE id = ?1", params![ids[0]]).unwrap();
        assert_eq!(ratio(&conn), (1, 4, 25.0), "看 1/4 集 → 25%");

        // 再看一集看到一半（不计入已看）→ 仍是 25%
        conn.execute("UPDATE videos SET watch_progress = 50 WHERE id = ?1", params![ids[1]]).unwrap();
        assert_eq!(ratio(&conn), (1, 4, 25.0), "半看的集不计入已看数");

        // 全剧标记已看 → 100%（mark_series_watched 只改 watch_progress，也必须反映到已看集数）
        mark_series_watched(&conn, sid, true).unwrap();
        assert_eq!(ratio(&conn), (4, 4, 100.0), "全剧标记已看 → 4/4 且 100%");
    }

    /// 播放历史按视频去重：同一部片子反复播放只保留一条，且带回完整视频数据
    #[test]
    fn history_entries_dedup_per_video() {
        let mut conn = mem_db();
        let tx = conn.transaction().unwrap();
        let a = upsert_video(&tx, &vp("A", "/t/A.mkv", "A.mkv"), false).unwrap().id();
        let b = upsert_video(&tx, &vp("B", "/t/B.mkv", "B.mkv"), false).unwrap().id();
        tx.commit().unwrap();

        for (i, vid) in [a, a, b, a].iter().enumerate() {
            conn.execute(
                "INSERT OR IGNORE INTO watch_history (video_id, watched_at, progress) VALUES (?1, ?2, ?3)",
                params![vid, format!("2026-08-28 00:00:0{}", i), i as f64],
            )
            .unwrap();
        }

        let entries = get_history_entries(&conn, 50).unwrap();
        assert_eq!(entries.len(), 2, "两个视频应各只出现一次");
        // 按最近时间倒序：A 的最后一条是 00:00:03（进度 3）
        // VideoWithTags 是 flatten 结构，Rust 侧要写 .video.video.id（序列化后是扁平的）
        assert_eq!(entries[0].video.video.id, a);
        assert!((entries[0].progress - 3.0).abs() < f64::EPSILON, "应取该视频最近一条的进度");
        assert_eq!(entries[0].video.video.file_size, 1024, "应带回完整视频数据");
        assert_eq!(entries[1].video.video.id, b);
    }
}
