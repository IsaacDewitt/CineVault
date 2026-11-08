import { invoke } from "@tauri-apps/api/core";
import type {
  BackfillProgress,
  BackfillResult,
  ExportData,
  ExportFileResult,
  FfprobeStatus,
  ImportResult,
  PaginatedVideos,
  PendingItem,
  RefreshResult,
  ScanConfig,
  ScanFolder,
  ScanResult,
  HistoryEntry,
  LanStatus,
  SeriesOverview,
  Tag,
  VideoKind,
  VideoStats,
  VideoWithTags,
} from "./types";

// ============================================
// 后端调用封装
// ============================================

export const api = {
  // 扫描
  scanFolder: (dirPath: string, incremental: boolean, defaultWatched: boolean) =>
    invoke<ScanResult>("scan_folder", { dirPath, incremental, defaultWatched }),
  addSeriesFromFolder: (dirPath: string, defaultWatched: boolean) =>
    invoke<ScanResult>("add_series_from_folder", { dirPath, defaultWatched }),
  refreshAllScans: (defaultWatched: boolean) =>
    invoke<RefreshResult>("refresh_all_scans", { defaultWatched }),

  // 查询
  getVideos: (p: {
    page?: number;
    pageSize?: number;
    query?: string;
    kind?: string;
    tagIds?: number[];
    /** 评分区间下限；0 = 不看下限。启用与否由调用方决定（未启用传 0） */
    minRating?: number;
    /** 评分区间上限；不传/10 = 不看上限 */
    maxRating?: number;
    sort?: string;
    order?: string;
    /** 观看状态勾选集合；全选时不传（= 不过滤），空数组 = 恒假（空结果） */
    watchStatuses?: string[];
  }) =>
    invoke<PaginatedVideos>("get_videos", {
      page: p.page ?? 0,
      pageSize: p.pageSize ?? 60,
      query: p.query ?? "",
      kind: p.kind,
      tagIds: p.tagIds ?? [],
      minRating: p.minRating ?? 0,
      maxRating: p.maxRating,
      sort: p.sort ?? "last_watched",
      order: p.order ?? "desc",
      watchStatuses: p.watchStatuses,
    }),
  getRecentlyAdded: (page = 0, pageSize = 60) =>
    invoke<PaginatedVideos>("get_recently_added", { page, pageSize }),
  getSeriesOverview: () => invoke<SeriesOverview[]>("get_series_overview"),
  getSeriesEpisodes: (seriesId: number) =>
    invoke<VideoWithTags[]>("get_series_episodes", { seriesId }),
  getVideoDetail: (videoId: number) =>
    invoke<VideoWithTags>("get_video_detail", { videoId }),
  getHistory: (limit = 60) => invoke<HistoryEntry[]>("get_history", { limit }),
  getStats: () => invoke<VideoStats>("get_stats"),

  // 标签
  getTags: () => invoke<Tag[]>("get_tags"),
  createTag: (name: string, color: string) =>
    invoke<Tag>("create_tag", { name, color }),
  updateTag: (tagId: number, name: string | null, color: string | null) =>
    invoke("update_tag", { tagId, name, color }),
  deleteTag: (tagId: number) => invoke("delete_tag", { tagId }),
  addVideoTag: (videoId: number, tagId: number) =>
    invoke("add_video_tag", { videoId, tagId }),
  removeVideoTag: (videoId: number, tagId: number) =>
    invoke("remove_video_tag", { videoId, tagId }),
  addSeriesTag: (seriesId: number, tagId: number) =>
    invoke("add_series_tag", { seriesId, tagId }),
  removeSeriesTag: (seriesId: number, tagId: number) =>
    invoke("remove_series_tag", { seriesId, tagId }),

  // 操作
  setRating: (videoId: number, rating: number) =>
    invoke("set_rating", { videoId, rating }),
  setSeriesRating: (seriesId: number, rating: number) =>
    invoke("set_series_rating", { seriesId, rating }),
  markSeriesWatched: (seriesId: number, watched: boolean) =>
    invoke<number>("mark_series_watched", { seriesId, watched }),
  updateProgress: (videoId: number, progress: number) =>
    invoke("update_progress", { videoId, progress }),
  markPlayed: (videoId: number) => invoke("mark_played", { videoId }),
  openFile: (path: string) => invoke("open_file", { path }),
  syncProgressFromPlayer: () => invoke<number>("sync_progress_from_player"),
  openFolder: (path: string) => invoke("open_folder", { path }),
  renameVideo: (videoId: number, newTitle: string) =>
    invoke("rename_video", { videoId, newTitle }),
  renameSeries: (seriesId: number, newName: string) =>
    invoke("rename_series", { seriesId, newName }),
  deleteVideo: (videoId: number) => invoke("delete_video", { videoId }),
  deleteSeries: (seriesId: number) => invoke("delete_series", { seriesId }),
  deleteVideoWithFile: (videoId: number) =>
    invoke<string>("delete_video_with_file", { videoId }),

  // 待确认
  getPending: () => invoke<PendingItem[]>("get_pending"),
  resolvePending: (videoId: number, kind: VideoKind, seriesName: string | null) =>
    invoke("resolve_pending", { videoId, kind, seriesName }),

  // 扫描目录
  getScanFolders: () => invoke<ScanFolder[]>("get_scan_folders"),
  deleteScanFolder: (folderId: number) =>
    invoke("delete_scan_folder", { folderId }),
  getScanConfig: () => invoke<ScanConfig>("get_scan_config"),
  setScanConfig: (minSizeMb: number, exemptPrefixes: string[]) =>
    invoke<ScanConfig>("set_scan_config", { minSizeMb, exemptPrefixes }),

  // 导出 / 导入
  exportJson: () => invoke<ExportData>("export_json"),
  exportJsonToFile: (filePath: string) =>
    invoke<ExportFileResult>("export_json_to_file", { filePath }),
  importJson: (jsonStr: string) => invoke<ImportResult>("import_json", { jsonStr }),
  importJsonFromFile: (filePath: string) =>
    invoke<ImportResult>("import_json_from_file", { filePath }),
  readTextFile: (filePath: string) => invoke<string>("read_text_file", { filePath }),
  writeTextFile: (filePath: string, content: string) =>
    invoke("write_text_file", { filePath, content }),

  // 本地静帧（无 ffmpeg / 解码失败时返回 null，由前端画文字片头卡）
  getVideoThumbnail: (videoId: number) =>
    invoke<string | null>("get_video_thumbnail", { videoId }),
  // 刷新封面：后端删掉旧 jpg 并在随机位置重新抽一帧；返回 null = 旧图已删但没生成成功
  refreshVideoThumbnail: (videoId: number) =>
    invoke<string | null>("refresh_video_thumbnail", { videoId }),

  // 时长补全
  backfillDurations: () => invoke<BackfillResult>("backfill_durations"),
  cancelDurationBackfill: () => invoke("cancel_duration_backfill"),
  getFfprobeStatus: () => invoke<FfprobeStatus>("get_ffprobe_status"),

  // 系统
  getDrives: () => invoke<{ letter: string; path: string; label: string }[]>("get_drives"),

  // 局域网手机访问
  getLanStatus: () => invoke<LanStatus>("get_lan_status"),
  setLanEnabled: (enabled: boolean) =>
    invoke<LanStatus>("set_lan_enabled", { enabled }),
  regenerateLanToken: () => invoke<LanStatus>("regenerate_lan_token"),
  rotateLanPairCode: () => invoke<LanStatus>("rotate_lan_pair_code"),
};

// 事件负载类型（backfill-progress）仅此一处引用，单独导出避免被当未使用
export type { BackfillProgress };
