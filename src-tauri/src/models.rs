use serde::{Deserialize, Serialize};

// ============================================
// 错误枚举 (#20: 结构化错误处理)
// ============================================
#[derive(Debug, Clone)]
pub struct AppError {
    pub kind: String,
    pub message: String,
}

impl AppError {
    pub fn db(msg: impl Into<String>) -> Self {
        Self { kind: "database".into(), message: msg.into() }
    }
    pub fn validation(msg: impl Into<String>) -> Self {
        Self { kind: "validation".into(), message: msg.into() }
    }
    pub fn io(msg: impl Into<String>) -> Self {
        Self { kind: "io".into(), message: msg.into() }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.kind, self.message)
    }
}

impl std::error::Error for AppError {}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        Self::db(e.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::io(e.to_string())
    }
}

// 供 Tauri command 的 ? 操作符使用
impl From<AppError> for String {
    fn from(e: AppError) -> String {
        e.to_string()
    }
}

// ============================================
// 数据模型
// ============================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Video {
    pub id: i64,
    pub title: String,
    pub file_path: String,
    pub file_name: String,
    pub file_size: i64,
    pub duration: Option<f64>,
    pub rating: f64,
    pub is_favorite: bool,
    pub video_type: String,
    pub series_name: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub thumbnail_path: Option<String>,
    pub last_watched_at: Option<String>,
    pub watch_progress: f64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoWithTags {
    #[serde(flatten)]
    pub video: Video,
    pub tags: Vec<Tag>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub video_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchHistory {
    pub id: i64,
    pub video_id: i64,
    pub watched_at: String,
    pub progress: f64,
    pub video_title: Option<String>,
    pub video_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub total_found: i64,
    pub new_added: i64,
    pub updated: i64,
    pub errors: Vec<String>,
    pub series_detected: Vec<SeriesInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesInfo {
    pub name: String,
    pub episode_count: i64,
    pub folder_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoStats {
    pub total_videos: i64,
    pub total_series: i64,
    pub total_size: i64,
    pub favorites_count: i64,
    pub watched_count: i64,
    pub average_rating: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportData {
    pub version: String,
    pub exported_at: String,
    pub videos: Vec<VideoWithTags>,
    pub watch_history: Vec<WatchHistory>,
}

/// 导入结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub imported: i64,
    pub skipped: i64,
}

/// 剧集概览（用于剧集列表页面）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesOverview {
    pub name: String,
    pub total_episodes: i64,
    pub watched_episodes: i64,
    pub progress: f64,
    pub total_size: i64,
    pub rating: f64,
}

/// 分页查询结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedVideos {
    pub items: Vec<VideoWithTags>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
    pub has_more: bool,
}

/// 已扫描文件夹记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanFolder {
    pub id: i64,
    pub folder_path: String,
    pub video_count: i64,
    pub last_scanned_at: String,
}

/// #1/#7: upsert_video 返回值，区分插入和更新
#[derive(Debug, Clone, PartialEq)]
pub enum UpsertResult {
    Inserted(i64),
    Updated(i64),
}

impl UpsertResult {
    pub fn id(&self) -> i64 {
        match self {
            UpsertResult::Inserted(id) | UpsertResult::Updated(id) => *id,
        }
    }
    pub fn is_inserted(&self) -> bool {
        matches!(self, UpsertResult::Inserted(_))
    }
}
