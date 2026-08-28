import React, { useCallback, useEffect, useRef, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import type {
  HistoryEntry,
  SeriesOverview,
  Tag,
  VideoStats,
  VideoWithTags,
  ViewKey,
  SortKey,
  WatchStatus,
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
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [tags, setTags] = useState<Tag[]>([]);
  const [stats, setStats] = useState<VideoStats | null>(null);
  const [pendingCount, setPendingCount] = useState(0);

  // 当前剧集详情：只存 id，实体从 series 派生——
  // 否则数据刷新后详情页仍拿着旧快照（改评分/标签后 UI 不更新，RatingPicker 与 Stars 显示两个分数）
  const [currentSeriesId, setCurrentSeriesId] = useState<number | null>(null);
  const currentSeries = currentSeriesId
    ? series.find((s) => s.id === currentSeriesId) ?? null
    : null;

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

  // 初始化
  useEffect(() => {
    fetchTags();
    fetchStats();
    fetchPendingCount();
  }, [fetchTags, fetchStats, fetchPendingCount]);

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

  // 从 PotPlayer 播放列表回填播放进度，返回实际更新条数（无变化时为 0）。
  // noisy=false 用于定时轮询：失败不弹 toast，否则后端一异常就每分钟弹一次。
  const syncPlayerProgress = useCallback(async (noisy = true): Promise<number> => {
    try {
      const n = await api.syncProgressFromPlayer();
      if (n > 0) toast.ok(`已从 PotPlayer 同步 ${n} 条播放进度`);
      return n;
    } catch (e) {
      if (noisy) {
        toast.err(`进度同步失败：${e}`);
      } else {
        console.warn("[CineVault] 进度同步失败：", e);
      }
      return 0;
    }
  }, []);

  // 同步 PotPlayer 播放记录并刷新当前视图，三路触发：
  // - visibilitychange：最小化/切走再切回（仅此场景可靠触发）
  // - window focus：点击窗口 / Alt-Tab 切回（窗口被遮挡时 visibilityState 不变，
  //   必须靠 focus 兜住"切回窗口"这个最常见动作）
  // - 60s 定时轮询：窗口一直开着没切走也能自动同步（后端增量快照幂等，开销极小）
  // 只有真的同步到新记录才刷新视图：无条件刷新会把"加载更多"攒下的分页重置回第 0 页。
  useEffect(() => {
    const refreshView = () => {
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
    };
    const onSync = async (noisy: boolean) => {
      const n = await syncPlayerProgress(noisy);
      if (n > 0) refreshView();
    };
    const onVis = () => {
      if (document.visibilityState === "visible") void onSync(true);
    };
    const onFocus = () => void onSync(true);
    const onTick = () => void onSync(false);
    document.addEventListener("visibilitychange", onVis);
    window.addEventListener("focus", onFocus);
    const timer = window.setInterval(onTick, 60_000);
    return () => {
      document.removeEventListener("visibilitychange", onVis);
      window.removeEventListener("focus", onFocus);
      window.clearInterval(timer);
    };
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
      // 点击播放即标记"最近观看"（失败不阻塞播放）
      void api.markPlayed(v.id).catch(() => {});
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
          setCurrentSeriesId(null);
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
          {currentSeries ? (
            <SeriesDetailView
              series={currentSeries}
              onBack={() => setCurrentSeriesId(null)}
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
                    setCurrentSeriesId(s.id);
                    setSelectedTagIds([]);
                  }}
                  tags={tags}
                  onChanged={afterDataChange}
                />
              ) : view === "history" ? (
                <VideoList
                  videos={history.map((h) => h.video).filter(matchV)}
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
            void syncPlayerProgress();
            afterDataChange();
          }}
        />
      )}
      {showAddSeries && (
        <AddSeriesModal
          onClose={() => setShowAddSeries(false)}
          onAdded={() => {
            void syncPlayerProgress();
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
