import React, { useCallback, useEffect, useRef, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import type {
  SeriesOverview,
  Tag,
  VideoStats,
  VideoWithTags,
  ViewKey,
  SortKey,
  WatchStatus,
  WatchHistory,
} from "./types";
import { api } from "./api";
import { Sidebar } from "./components/Sidebar";
import { TopBar } from "./components/TopBar";
import { VideoList } from "./components/VideoList";
import { SeriesGrid } from "./components/SeriesGrid";
import { SeriesDetailView } from "./components/SeriesDetailView";
import { DetailModal } from "./components/DetailModal";
import { ScanModal } from "./components/ScanModal";
import { AddSeriesModal } from "./components/AddSeriesModal";
import { PendingModal } from "./components/PendingModal";
import { TagManager } from "./components/TagManager";
import { MigrateBanner } from "./components/MigrateBanner";
import { ToastHost, toast, formatSize } from "./components/ui";

const PAGE_SIZE = 80;

export default function App() {
  // 视图与筛选
  const [view, setView] = useState<ViewKey>("all");
  const [query, setQuery] = useState("");
  const [kind, setKind] = useState("all");
  const [minRating, setMinRating] = useState(0);
  const [sort, setSort] = useState<SortKey>("last_watched");
  const [watchStatus, setWatchStatus] = useState<WatchStatus>("all");
  const [selectedTagIds, setSelectedTagIds] = useState<number[]>([]);

  // 数据
  const [videos, setVideos] = useState<VideoWithTags[]>([]);
  const [total, setTotal] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [page, setPage] = useState(0);
  const [loading, setLoading] = useState(false);
  const [series, setSeries] = useState<SeriesOverview[]>([]);
  const [history, setHistory] = useState<WatchHistory[]>([]);
  const [tags, setTags] = useState<Tag[]>([]);
  const [stats, setStats] = useState<VideoStats | null>(null);
  const [pendingCount, setPendingCount] = useState(0);
  const [migrationPath, setMigrationPath] = useState<string | null>(null);

  // 当前剧集详情
  const [currentSeries, setCurrentSeries] = useState<SeriesOverview | null>(null);

  // 弹窗
  const [showScan, setShowScan] = useState(false);
  const [showAddSeries, setShowAddSeries] = useState(false);
  const [showPending, setShowPending] = useState(false);
  const [showTagManager, setShowTagManager] = useState(false);
  const [detailVideo, setDetailVideo] = useState<VideoWithTags | null>(null);

  const debounceRef = useRef<number>(0);

  // ===== 数据加载 =====
  const fetchTags = useCallback(async () => {
    try {
      setTags(await api.getTags());
    } catch (e) {
      toast.err(String(e));
    }
  }, []);

  const fetchStats = useCallback(async () => {
    try {
      setStats(await api.getStats());
    } catch (e) {
      /* 忽略 */
    }
  }, []);

  const fetchPendingCount = useCallback(async () => {
    try {
      setPendingCount((await api.getPending()).length);
    } catch (e) {
      /* 忽略 */
    }
  }, []);

  const fetchVideos = useCallback(
    async (p = 0, append = false) => {
      setLoading(true);
      try {
        const res = await api.getVideos({
          page: p,
          pageSize: PAGE_SIZE,
          query,
          // "电影"视图强制类型=电影（顶栏联动 + 强制兜底）
          kind: view === "movies" ? "movie" : kind,
          tagIds: selectedTagIds,
          favoritesOnly: view === "favorites",
          minRating,
          sort,
          watchStatus,
        });
        setVideos((prev) => (append ? [...prev, ...res.items] : res.items));
        setTotal(res.total);
        setHasMore(res.has_more);
        setPage(p);
      } catch (e) {
        toast.err(String(e));
      } finally {
        setLoading(false);
      }
    },
    [query, kind, selectedTagIds, view, minRating, sort, watchStatus]
  );

  const fetchSeries = useCallback(async () => {
    try {
      setSeries(await api.getSeriesOverview());
    } catch (e) {
      toast.err(String(e));
    }
  }, []);

  const fetchHistory = useCallback(async () => {
    try {
      setHistory(await api.getHistory(100));
    } catch (e) {
      toast.err(String(e));
    }
  }, []);

  const checkMigration = useCallback(async () => {
    try {
      setMigrationPath(await api.checkMigration());
    } catch (e) {
      /* 忽略 */
    }
  }, []);

  // 初始化
  useEffect(() => {
    checkMigration();
    fetchTags();
    fetchStats();
    fetchPendingCount();
  }, [checkMigration, fetchTags, fetchStats, fetchPendingCount]);

  // 搜索框只需更新 query；防抖由下方 useEffect 处理（避免 setTimeout 闭包陷阱）
  const onQueryChange = (q: string) => setQuery(q);

  useEffect(() => {
    if (view === "series") {
      fetchSeries();
    } else if (view === "history") {
      fetchHistory();
    } else if (view === "recent") {
      setLoading(true);
      api
        .getRecentlyAdded(0, PAGE_SIZE)
        .then((res) => {
          setVideos(res.items);
          setTotal(res.total);
          setHasMore(res.has_more);
          setPage(0);
        })
        .catch((e) => toast.err(String(e)))
        .finally(() => setLoading(false));
    } else if (query !== "") {
      // 搜索框防抖：350ms 内无新输入才查库
      const id = window.setTimeout(() => fetchVideos(0), 350);
      return () => window.clearTimeout(id);
    } else {
      fetchVideos(0);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view, query, kind, selectedTagIds, minRating, sort, watchStatus, fetchVideos, fetchSeries, fetchHistory]);

  // 从 PotPlayer 播放列表回填播放进度（无记录时静默返回 0）
  const syncPlayerProgress = useCallback(async () => {
    try {
      const n = await api.syncProgressFromPlayer();
      if (n > 0) toast.ok(`已从 PotPlayer 同步 ${n} 条播放进度`);
    } catch (e) {
      // 命令不存在等异常不再静默，便于发现版本不匹配
      toast.err(`进度同步失败：${e}`);
    }
  }, []);

  // 切回窗口（从 PotPlayer 等外部应用返回）→ 自动同步进度并按当前视图刷新
  useEffect(() => {
    const onVis = () => {
      if (document.visibilityState !== "visible") return;
      syncPlayerProgress().then(() => {
        if (view === "series") {
          fetchSeries();
        } else if (view === "history") {
          fetchHistory();
        } else if (view === "recent") {
          api
            .getRecentlyAdded(0, PAGE_SIZE)
            .then((res) => {
              setVideos(res.items);
              setTotal(res.total);
              setHasMore(res.has_more);
              setPage(0);
            })
            .catch(() => {});
        } else {
          fetchVideos(0);
        }
      });
    };
    document.addEventListener("visibilitychange", onVis);
    return () => document.removeEventListener("visibilitychange", onVis);
  }, [syncPlayerProgress, fetchVideos, fetchSeries, fetchHistory, view]);

  const refreshAll = useCallback(async () => {
    try {
      const r = await api.refreshAllScans(false);
      toast.ok(
        `刷新完成：新增 ${r.new_added} · 移动 ${r.moved} · 移除失效 ${r.removed}`
      );
      if (r.pending.length > 0) {
        toast.ok(`${r.pending.length} 个待确认`);
      }
      await syncPlayerProgress();
      await Promise.all([fetchVideos(0), fetchSeries(), fetchTags(), fetchStats(), fetchPendingCount()]);
    } catch (e) {
      toast.err(String(e));
    }
  }, [fetchVideos, fetchSeries, fetchTags, fetchStats, fetchPendingCount, syncPlayerProgress]);

  const afterDataChange = useCallback(() => {
    fetchVideos(0);
    fetchSeries();
    fetchTags();
    fetchStats();
    fetchPendingCount();
  }, [fetchVideos, fetchSeries, fetchTags, fetchStats, fetchPendingCount]);

  // ===== 播放 =====
  // 交给系统默认播放器（PotPlayer），续播由 PotPlayer 内置记忆位置负责
  const play = async (v: VideoWithTags) => {
    if (!v.file_path) {
      toast.err("该记录无文件路径（可能已删除），无法播放");
      return;
    }
    try {
      await api.openFile(v.file_path);
    } catch (e) {
      toast.err(String(e));
    }
  };

  // ===== 导出 / 导入 =====
  const doExport = async () => {
    try {
      const data = await api.exportJson();
      const path = await save({
        defaultPath: `cinevault-backup-${new Date().toISOString().slice(0, 10)}.json`,
        filters: [{ name: "JSON 备份", extensions: ["json"] }],
      });
      if (!path) return;
      await api.writeTextFile(path, JSON.stringify(data, null, 2));
      toast.ok(`已导出：${data.videos.length} 视频 · ${data.series.length} 剧集`);
    } catch (e) {
      toast.err(String(e));
    }
  };

  const doImport = async () => {
    const path = await open({
      multiple: false,
      filters: [{ name: "JSON 备份", extensions: ["json"] }],
    });
    if (typeof path !== "string" || !path) return;
    try {
      const content = await api.readTextFile(path);
      const r = await api.importJson(content);
      toast.ok(
        `导入完成：视频 ${r.imported_videos} · 剧集 ${r.imported_series} · 标签 ${r.imported_tags}`
      );
      afterDataChange();
    } catch (e) {
      toast.err(String(e));
    }
  };

  // ===== 统计 =====
  const counts = {
    all: stats?.total_videos ?? 0,
    movies: stats?.movie_count ?? 0,
    series: stats?.total_series ?? 0,
    favorites: stats?.favorites_count ?? 0,
  };

  const viewTitle: Record<ViewKey, string> = {
    all: "全部视频",
    movies: "电影",
    series: "剧集",
    favorites: "收藏夹",
    recent: "最近更新",
    history: "播放历史",
  };

  // ===== 渲染 =====
  // 前端兜底过滤：剧集/历史/最近更新视图不走后端 query，这里按关键词过滤
  const q = query.trim().toLowerCase();
  const matchV = (v: VideoWithTags) =>
    !q ||
    v.title.toLowerCase().includes(q) ||
    v.file_name.toLowerCase().includes(q) ||
    v.file_path.toLowerCase().includes(q);
  const matchS = (s: SeriesOverview) => !q || s.name.toLowerCase().includes(q);

  return (
    <div className="app">
      <Sidebar
        view={view}
        onView={(v) => {
          setView(v);
          setCurrentSeries(null);
          // 左侧"电影"强制类型=电影；"全部视频"重置回全部，与顶栏筛选联动
          if (v === "movies") setKind("movie");
          else if (v === "all") setKind("all");
        }}
        tags={tags}
        selectedTagIds={selectedTagIds}
        onToggleTag={(id) =>
          setSelectedTagIds((prev) =>
            prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]
          )
        }
        counts={counts}
        pendingCount={pendingCount}
        onOpenTagManager={() => setShowTagManager(true)}
        onOpenScan={() => setShowScan(true)}
        onOpenAddSeries={() => setShowAddSeries(true)}
        onOpenPending={() => setShowPending(true)}
        onExport={doExport}
        onImport={doImport}
      />

      <main className="main">
        <TopBar
          query={query}
          onQuery={onQueryChange}
          kind={kind}
          onKind={setKind}
          minRating={minRating}
          onMinRating={setMinRating}
          sort={sort}
          onSort={setSort}
          watchStatus={watchStatus}
          onWatchStatus={setWatchStatus}
        />

        <div className="stats-bar">
          <span>
            共 <b>{stats?.total_videos ?? 0}</b> 个视频 ·{" "}
            <b>{stats?.total_series ?? 0}</b> 部剧集
          </span>
          <span>总大小 {formatSize(stats?.total_size ?? 0)}</span>
          {stats && stats.average_rating > 0 && (
            <span>均分 {stats.average_rating.toFixed(1)}</span>
          )}
          <button
            className="btn btn-ghost"
            style={{ marginLeft: "auto", padding: "3px 12px", fontSize: 12 }}
            onClick={refreshAll}
          >
            ↻ 刷新扫描
          </button>
        </div>

        <div className="content">
          {migrationPath && (
            <MigrateBanner oldPath={migrationPath} onDone={() => { setMigrationPath(null); afterDataChange(); }} />
          )}

          {currentSeries ? (
            <SeriesDetailView
              series={currentSeries}
              onBack={() => setCurrentSeries(null)}
              tags={tags}
              onChanged={afterDataChange}
              onOpenVideo={(v) => setDetailVideo(v)}
              onPlayVideo={play}
            />
          ) : (
            <>
              <div className="view-title">
                {viewTitle[view]}
                <span className="hint">
                  {view === "all" || view === "movies" || view === "favorites" || (view === "recent" && !q)
                    ? `${total} 条`
                    : ""}
                </span>
              </div>

              {view === "series" ? (
                <SeriesGrid
                  series={series.filter(matchS)}
                  sort={sort}
                  onOpen={(s) => {
                    setCurrentSeries(s);
                    setSelectedTagIds([]);
                  }}
                  tags={tags}
                  onChanged={afterDataChange}
                />
              ) : view === "history" ? (
                <VideoList
                  videos={historyToVideos(history).filter(matchV)}
                  onOpen={setDetailVideo}
                  onPlay={play}
                  onChanged={afterDataChange}
                />
              ) : (
                <>
                  <VideoList
                    videos={view === "recent" ? videos.filter(matchV) : videos}
                    onOpen={setDetailVideo}
                    onPlay={play}
                    onChanged={afterDataChange}
                  />
                  {/* 前端过滤时禁用"加载更多"（分页数据未过滤，避免错乱） */}
                  {hasMore && !q && (
                    <div className="load-more">
                      <button className="btn btn-ghost" onClick={() => fetchVideos(page + 1, true)}>
                        {loading ? "加载中…" : "加载更多"}
                      </button>
                    </div>
                  )}
                </>
              )}
            </>
          )}
        </div>
      </main>

      {/* 弹窗 */}
      {showScan && (
        <ScanModal
          onClose={() => setShowScan(false)}
          onScanned={() => {
            syncPlayerProgress();
            afterDataChange();
          }}
        />
      )}
      {showAddSeries && (
        <AddSeriesModal
          onClose={() => setShowAddSeries(false)}
          onAdded={() => {
            syncPlayerProgress();
            afterDataChange();
          }}
        />
      )}
      {showPending && (
        <PendingModal
          onClose={() => setShowPending(false)}
          onChanged={afterDataChange}
        />
      )}
      {showTagManager && (
        <TagManager onClose={() => setShowTagManager(false)} onChanged={fetchTags} />
      )}
      {detailVideo && (
        <DetailModal
          video={detailVideo}
          tags={tags}
          onClose={() => setDetailVideo(null)}
          onChanged={afterDataChange}
          onPlay={(v) => {
            play(v);
            setDetailVideo(null);
          }}
        />
      )}

      <ToastHost />
    </div>
  );
}

// 历史记录 → 视频行展示
function historyToVideos(history: WatchHistory[]): VideoWithTags[] {
  return history
    .filter((h) => h.video_title || h.video_path)
    .map((h) => ({
      id: h.video_id,
      title: h.video_title ?? h.video_path ?? "未知",
      file_path: h.video_path ?? "",
      file_name: h.video_path ?? "",
      file_size: 0,
      duration: null,
      rating: 0,
      is_favorite: false,
      kind: "unknown" as const,
      confidence: 0,
      user_kind: null,
      series_id: null,
      series_name: null,
      season: null,
      episode: null,
      last_watched_at: h.watched_at,
      watch_progress: h.progress,
      created_at: "",
      updated_at: "",
      tags: [],
    }));
}
