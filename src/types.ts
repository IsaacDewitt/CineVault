// ============================================
// CineVault 类型定义
// ============================================

export type VideoKind = "movie" | "series" | "unknown";

export interface Video {
  id: number;
  title: string;
  file_path: string;
  file_name: string;
  file_size: number;
  duration: number | null;
  rating: number;
  kind: VideoKind;
  confidence: number;
  user_kind: string | null;
  series_id: number | null;
  series_name: string | null;
  season: number | null;
  episode: number | null;
  /** 合体文件（一个文件含多集）的末集号，如 EP01-03 → 3；单集为 null */
  episode_end: number | null;
  last_watched_at: string | null;
  watch_progress: number;
  created_at: string;
  updated_at: string;
}

export interface VideoWithTags extends Video {
  tags: Tag[];
}

export interface Tag {
  id: number;
  name: string;
  color: string;
  video_count: number | null;
}

export interface SeriesOverview {
  id: number;
  name: string;
  folder_path: string | null;
  total_episodes: number;
  watched_episodes: number;
  progress: number;
  total_size: number;
  rating: number;
  last_watched_at: string | null;
  updated_at: string;
  /** 全部单集时长之和（秒），旧备份 JSON 导入时可能缺省 */
  total_duration?: number;
  /** 首集视频 id，用于生成剧集卡片封面；旧备份可能缺省 */
  cover_video_id?: number | null;
  tags: Tag[];
}

export interface WatchHistory {
  id: number;
  video_id: number;
  watched_at: string;
  progress: number;
  video_title: string | null;
  video_path: string | null;
}

/// 播放历史条目（界面用）：按视频去重，每个视频只出现一次，带完整视频数据
export interface HistoryEntry {
  video: VideoWithTags;
  watched_at: string;
  progress: number;
}

export interface PaginatedVideos {
  items: VideoWithTags[];
  total: number;
  page: number;
  page_size: number;
  has_more: boolean;
}

export interface VideoStats {
  total_videos: number;
  total_series: number;
  total_size: number;
  watched_count: number;
  average_rating: number;
  movie_count: number;
}

export interface ScanResult {
  total_found: number;
  new_added: number;
  updated: number;
  errors: string[];
  pending: PendingItem[];
}

export interface RefreshResult {
  removed: number;
  new_added: number;
  moved: number;
  folders_scanned: number;
  /** 本次自动收紧锚点的剧集数（为无锚点剧集补 folder_path） */
  anchors_tightened?: number;
  /** 发生删除前自动生成的整库快照 */
  backup_path?: string | null;
  errors: string[];
  pending: PendingItem[];
}

export interface PendingItem {
  video_id: number;
  title: string;
  file_path: string;
  confidence: number;
  suggested: string;
}

export interface ScanFolder {
  id: number;
  folder_path: string;
  last_scanned_at: string | null;
}

export interface ScanConfig {
  min_size_mb: number;
  exempt_prefixes: string[];
}

export type ViewKey = "all" | "movies" | "series" | "random";

export type SortKey = "last_watched" | "updated" | "name" | "size" | "rating" | "duration";

/** 排序方向；last_watched/updated 是"最近"语义，后端恒降序，不吃此方向 */
export type SortDir = "asc" | "desc";

export interface SortSpec {
  key: SortKey;
  dir: SortDir;
}

/** 观看状态（可多选过滤）；勾选集合即过滤集合，三态全选 = 不过滤 */
export type WatchState = "unwatched" | "partial" | "watched";

export interface ExportData {
  app: string;
  version: string;
  exported_at: string;
  videos: VideoWithTags[];
  series: SeriesOverview[];
  tags: Tag[];
  watch_history: WatchHistory[];
  scan_folders: ScanFolder[];
}

export interface ImportResult {
  imported_videos: number;
  imported_series: number;
  imported_tags: number;
  imported_history: number;
  imported_folders: number;
  /** import_json_from_file 自动生成的导入前整库备份 */
  backup_path?: string;
}

export interface ExportFileResult {
  path: string;
  videos: number;
  series: number;
}

// ============================================
// 时长批量补全
// ============================================
export interface BackfillProgress {
  done: number;
  total: number;
  filled: number;
  failed: number;
  current: string;
}

export interface BackfillResult {
  total: number;
  filled: number;
  failed: number;
  cancelled: boolean;
}

export interface FfprobeStatus {
  available: boolean;
  path: string | null;
}

// ============================================
// 局域网手机访问
// ============================================
export interface LanStatus {
  running: boolean;
  enabled: boolean;
  port: number;
  name: string;
  token: string;
  pair_code: string;
  /** Unix 秒；过期后需在桌面端更换配对码 */
  pair_expires_at: number;
  urls: string[];
  error: string | null;
}
