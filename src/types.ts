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
  is_favorite: boolean;
  kind: VideoKind;
  confidence: number;
  user_kind: string | null;
  series_id: number | null;
  series_name: string | null;
  season: number | null;
  episode: number | null;
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
  is_favorite: boolean;
  last_watched_at: string | null;
  updated_at: string;
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
  favorites_count: number;
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

export type ViewKey =
  | "all"
  | "movies"
  | "series"
  | "favorites"
  | "recent"
  | "history";

export type SortKey = "last_watched" | "updated" | "name" | "size" | "rating";

export type WatchStatus = "all" | "unwatched" | "partial" | "watched";

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
