use serde::{Deserialize, Serialize};

// ============================================
// 错误类型
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
// 实体模型
// ============================================

/// 视频（电影或剧集单集）
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
    /// movie / series / unknown
    pub kind: String,
    /// 自动分类置信度 0-100
    pub confidence: i32,
    /// 用户手动指定类型（覆盖自动分类）
    pub user_kind: Option<String>,
    pub series_id: Option<i64>,
    pub series_name: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
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

/// 剧集实体（name 唯一键）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Series {
    pub id: i64,
    pub name: String,
    pub folder_path: Option<String>,
    pub rating: f64,
    pub is_favorite: bool,
    pub last_watched_at: Option<String>,
    pub watch_progress: f64,
    pub created_at: String,
    pub updated_at: String,
}

/// 剧集概览（剧集列表页）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesOverview {
    pub id: i64,
    pub name: String,
    pub folder_path: Option<String>,
    pub total_episodes: i64,
    pub watched_episodes: i64,
    pub progress: f64,
    pub total_size: i64,
    pub rating: f64,
    pub is_favorite: bool,
    pub last_watched_at: Option<String>,
    pub tags: Vec<Tag>,
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
    /// 待确认分类（置信度中等的疑似剧集）
    pub pending: Vec<PendingItem>,
}

/// 待确认分类项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingItem {
    pub video_id: i64,
    pub title: String,
    pub file_path: String,
    pub confidence: i32,
    pub suggested: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshResult {
    pub removed: i64,
    pub new_added: i64,
    pub moved: i64,
    pub folders_scanned: i64,
    pub errors: Vec<String>,
    pub pending: Vec<PendingItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedVideos {
    pub items: Vec<VideoWithTags>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoStats {
    pub total_videos: i64,
    pub total_series: i64,
    pub total_size: i64,
    pub favorites_count: i64,
    pub watched_count: i64,
    pub average_rating: f64,
    pub movie_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanFolder {
    pub id: i64,
    pub folder_path: String,
    pub last_scanned_at: Option<String>,
}

/// 旧库迁移结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationResult {
    pub videos: i64,
    pub series: i64,
    pub tags: i64,
    pub history: i64,
    pub scan_folders: i64,
    pub from_path: String,
}

/// upsert_video 参数
pub struct VideoUpsertParams<'a> {
    pub title: &'a str,
    pub file_path: &'a str,
    pub file_name: &'a str,
    pub file_size: i64,
    pub duration: Option<f64>,
    pub kind: &'a str,
    pub confidence: i32,
    pub user_kind: Option<&'a str>,
    pub series_id: Option<i64>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub default_watched: bool,
}

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

// ============================================
// 导出 / 导入
// ============================================
/// 全量导出结构（JSON 分享格式：比旧版 MovieUI 完整，含剧集/标签/扫描目录）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportData {
    pub app: String,
    pub version: String,
    pub exported_at: String,
    pub videos: Vec<VideoWithTags>,
    pub series: Vec<SeriesOverview>,
    pub tags: Vec<Tag>,
    pub watch_history: Vec<WatchHistory>,
    pub scan_folders: Vec<ScanFolder>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub imported_videos: i64,
    pub imported_series: i64,
    pub imported_tags: i64,
    pub imported_history: i64,
    pub imported_folders: i64,
}
