import { useCallback, useEffect, useRef, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import type {
  BackfillProgress,
  SeriesOverview,
  Tag,
  VideoStats,
  VideoWithTags,
  ViewKey,
  SortSpec,
  WatchState,
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
import { LanAccessModal } from "./components/LanAccessModal";
import { SettingsDrawer } from "./components/SettingsDrawer";
import { ContinueWatching } from "./components/ContinueWatching";
import { ConfirmDialog, ToastHost, toast, formatSize } from "./components/ui";
import { Icon } from "./components/icons";
import {
  hasActiveFilters,
  matchesSeriesFilters,
  matchesVideoQuery,
  type RatingRange,
} from "./lib/library";

const PAGE_SIZE = 80;
// 随机推荐一批的条数（固定 20，不参与分页——随机序翻页会重抽且必然重复）
const RANDOM_COUNT = 20;

export default function App() {
  // 视图与筛选
  const [view, setView] = useState<ViewKey>("all");
  const [query, setQuery] = useState("");
  // 顶栏筛选重设计：类型筛选删掉（与左侧栏重复，"电影"视图由 view 强制）；
  // 状态 = 勾选集合（全选 = 不过滤）；评分 = 启用开关 + 区间；排序 = 维度 + 方向
  const [watchStates, setWatchStates] = useState<Set<WatchState>>(
    () => new Set(["unwatched", "partial", "watched"])
  );
  const [ratingRange, setRatingRange] = useState<RatingRange>({
    enabled: false,
    min: 0,
    max: 10,
  });
  const [sort, setSort] = useState<SortSpec>({ key: "last_watched", dir: "desc" });
  const [selectedTagIds, setSelectedTagIds] = useState<number[]>([]);

  // 数据
  const [videos, setVideos] = useState<VideoWithTags[]>([]);
  // 随机推荐独立持有一批结果：塞进 videos 会被主列表的刷新逻辑覆盖
  const [randomVideos, setRandomVideos] = useState<VideoWithTags[]>([]);
  const [randomLoading, setRandomLoading] = useState(false);
  // 递增即重抽一次（「换一批」按钮）
  const [randomTicket, setRandomTicket] = useState(0);
  const [continueVideos, setContinueVideos] = useState<VideoWithTags[]>([]);
  const [total, setTotal] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [page, setPage] = useState(0);
  const [loading, setLoading] = useState(false);
  const [series, setSeries] = useState<SeriesOverview[]>([]);
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
  const [showLanAccess, setShowLanAccess] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [detailVideo, setDetailVideo] = useState<VideoWithTags | null>(null);
  const [pendingImport, setPendingImport] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  // 非空即代表补全正在跑（按钮切换成进度条 + 中断）
  const [backfill, setBackfill] = useState<BackfillProgress | null>(null);
  // PotPlayer 同步成功后的剧集详情重载信号：详情页集数列表是子组件私有 state，
  // 顶层 fetchSeries/fetchVideos 刷新不到它，靠此递增 key 强制其重新拉取
  const [episodeReloadKey, setEpisodeReloadKey] = useState(0);

  // 每个数据源独立代际号：快速切换筛选时，旧响应回来对不上号就直接丢弃。
  const videoReq = useRef(0);
  const seriesReq = useRef(0);
  const randomReq = useRef(0);
  const continueReq = useRef(0);

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

  // P-10：视频列表查询参数的唯一构造点，fetchVideos 与保分页刷新共用，防止两边漂移
  const videoQuery = useCallback(
    () => ({
      pageSize: PAGE_SIZE,
      query,
      // "电影"视图强制类型=电影；其余视图不做类型过滤（顶栏类型下拉已删）
      kind: view === "movies" ? "movie" : undefined,
      tagIds: selectedTagIds,
      minRating: ratingRange.enabled ? ratingRange.min : 0,
      maxRating: ratingRange.enabled ? ratingRange.max : undefined,
      sort: sort.key,
      order: sort.dir,
      // 全选 = 不过滤（后端 None）；空集合 = 恒假（后端 1=0）
      watchStatuses:
        watchStates.size === 3 ? undefined : ([...watchStates] as string[]),
    }),
    [query, selectedTagIds, view, ratingRange, sort, watchStates]
  );

  const fetchVideos = useCallback(
    async (p = 0, append = false) => {
      const req = ++videoReq.current;
      setLoading(true);
      try {
        const res = await api.getVideos({ page: p, ...videoQuery() });
        if (req !== videoReq.current) return;
        setVideos((prev) => {
          if (!append) return res.items;
          const seen = new Set(prev.map((v) => v.id));
          return [...prev, ...res.items.filter((v) => !seen.has(v.id))];
        });
        setTotal(res.total);
        setHasMore(res.has_more);
        setPage(p);
      } catch (e) {
        if (req === videoReq.current) toast.err(String(e));
      } finally {
        if (req === videoReq.current) setLoading(false);
      }
    },
    [videoQuery]
  );

  // P-10：小改动（评分）后的"原位刷新"——把已加载的全部页重拉合并。
  // 旧的 afterDataChange 一律 fetchVideos(0)，会把"加载更多"攒下的分页重置回第 0 页。
  const refreshVideosKeepPage = useCallback(async () => {
    const req = ++videoReq.current;
    setLoading(true);
    try {
      const merged: VideoWithTags[] = [];
      let nextTotal = 0;
      let nextHasMore = false;
      for (let i = 0; i <= page; i++) {
        const res = await api.getVideos({ page: i, ...videoQuery() });
        if (req !== videoReq.current) return;
        merged.push(...res.items);
        nextTotal = res.total;
        nextHasMore = res.has_more;
        if (!res.has_more) break;
      }
      setVideos(merged);
      setTotal(nextTotal);
      setHasMore(nextHasMore);
      // page 不动：已加载页数不变
    } catch (e) {
      if (req === videoReq.current) toast.err(String(e));
    } finally {
      if (req === videoReq.current) setLoading(false);
    }
  }, [page, videoQuery]);

  const fetchSeries = useCallback(async () => {
    const req = ++seriesReq.current;
    try {
      const next = await api.getSeriesOverview();
      if (req === seriesReq.current) setSeries(next);
    } catch (e) {
      if (req === seriesReq.current) toast.err(String(e));
    }
  }, []);

  const fetchContinue = useCallback(async () => {
    const req = ++continueReq.current;
    try {
      const res = await api.getVideos({
        page: 0,
        pageSize: 10,
        watchStatuses: ["partial"],
        sort: "last_watched",
        order: "desc",
      });
      if (req === continueReq.current) setContinueVideos(res.items);
    } catch {
      // 继续观看是增强信息，失败时静默隐藏，不打扰主列表。
      if (req === continueReq.current) setContinueVideos([]);
    }
  }, []);

  // ===== 随机推荐 =====
  // 规则写死：只抽电影（kind=movie），且仅「未看过 / 在看」（watch_progress < 100）。
  // 排序交给后端 ORDER BY RANDOM()，不落库不缓存——「换一批」就是再抽一次。
  const fetchRandom = useCallback(async () => {
    const req = ++randomReq.current;
    setRandomLoading(true);
    try {
      const res = await api.getVideos({
        page: 0,
        pageSize: RANDOM_COUNT,
        kind: "movie",
        watchStatuses: ["unwatched", "partial"],
        sort: "random",
        order: "desc",
      });
      if (req === randomReq.current) setRandomVideos(res.items);
    } catch (e) {
      if (req === randomReq.current) toast.err(String(e));
    } finally {
      if (req === randomReq.current) setRandomLoading(false);
    }
  }, []);

  // 改评分/进度后的原位刷新：不重抽（否则刚改完列表就整批换掉），
  // 只按当前批次 id 重取详情——顺带剔掉已删除的，以及被改成「已看」后不再符合条件的。
  const refreshRandomInPlace = useCallback(async () => {
    const ids = randomVideos.map((v) => v.id);
    if (ids.length === 0) return;
    const req = randomReq.current;
    const fresh = await Promise.all(
      ids.map((id) => api.getVideoDetail(id).catch(() => null))
    );
    if (req !== randomReq.current) return;
    setRandomVideos(
      fresh.filter((v): v is VideoWithTags => !!v && v.watch_progress < 100)
    );
  }, [randomVideos]);

  // 随机推荐取数：只在切进视图或点「换一批」时抽签。
  // 不能并进下面那个主 effect——它的依赖带 query，打字会让这批 20 部每按一键就重抽。
  useEffect(() => {
    if (view !== "random") return;
    void fetchRandom();
  }, [view, randomTicket, fetchRandom]);

  // 初始化
  useEffect(() => {
    fetchTags();
    fetchStats();
    fetchPendingCount();
    fetchContinue();
  }, [fetchTags, fetchStats, fetchPendingCount, fetchContinue]);

  // 搜索框只需更新 query；防抖由下方 useEffect 处理（避免 setTimeout 闭包陷阱）
  const onQueryChange = (q: string) => setQuery(q);

  // 剧集视图只在切进去时取数：搜索、筛选、排序都在内存里做，
  // 这样打字不会每键都打一遍数据库，也不会把筛选结果和旧请求搅在一起。
  useEffect(() => {
    if (view === "series") fetchSeries();
  }, [view, fetchSeries]);

  // 主列表（全部 / 电影）：后端负责筛选与分页。
  // 搜索保留 350ms 防抖；筛选控件变化立即重查。
  useEffect(() => {
    if (view !== "all" && view !== "movies") return;
    // 搜索 350ms；筛选/排序 120ms。拖动评分滑条时会连续触发，
    // 小防抖能把十几次查询压成一次，同时保留“松手即见”的手感。
    const delay = query.trim() !== "" ? 350 : 120;
    const id = window.setTimeout(() => fetchVideos(0), delay);
    return () => window.clearTimeout(id);
  }, [view, query, selectedTagIds, ratingRange, sort, watchStates, fetchVideos]);

  const refreshCurrentView = useCallback(
    (keepPage = false) => {
      if (view === "series") {
        fetchSeries();
      } else if (view !== "random") {
        if (keepPage) void refreshVideosKeepPage();
        else void fetchVideos(0);
      }
    },
    [view, fetchVideos, fetchSeries, refreshVideosKeepPage]
  );

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
    const onSync = async (noisy: boolean) => {
      const n = await syncPlayerProgress(noisy);
      if (n > 0) {
        // 随机视图保持当前批次：后台进度同步不该让正在看的推荐列表整批换掉。
        refreshCurrentView(false);
        fetchContinue();
        // 剧集详情页的集数列表（含每集进度条/已看标记）由子组件自行持有，
        // 顶层刷新够不到——用户症状：提示"已同步 N 条"但详情页旧，退出重进才见新。
        // 这里递增信号，让正在展示的 SeriesDetailView 重新拉取集数。
        setEpisodeReloadKey((k) => k + 1);
      }
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
  }, [syncPlayerProgress, refreshCurrentView, fetchContinue]);

  const refreshAll = useCallback(async () => {
    if (refreshing) return;
    setRefreshing(true);
    toast.ok("正在刷新扫描…");
    try {
      const r = await api.refreshAllScans(false);
      toast.ok(
        `刷新完成：新增 ${r.new_added} · 移动 ${r.moved} · 移除失效 ${r.removed}`
      );
      if (r.pending.length > 0) {
        toast.ok(`${r.pending.length} 个待确认`);
      }
      if (r.backup_path) {
        toast.ok(`清理前快照：${r.backup_path}`);
      }
      if (r.errors.length > 0) {
        toast.err(`刷新完成但有 ${r.errors.length} 条提示：${r.errors[0]}`);
      }
      await syncPlayerProgress();
      refreshCurrentView(false);
      if (view === "random") void refreshRandomInPlace();
      fetchSeries();
      fetchTags();
      fetchStats();
      fetchPendingCount();
      fetchContinue();
    } catch (e) {
      toast.err(String(e));
    } finally {
      setRefreshing(false);
    }
  }, [refreshing, view, syncPlayerProgress, refreshCurrentView, refreshRandomInPlace, fetchSeries, fetchTags, fetchStats, fetchPendingCount, fetchContinue]);

  const afterDataChange = useCallback(
    (opts?: { keepPage?: boolean }) => {
      // P-10：keepPage（评分等小改动）走原位刷新保留分页；
      // 默认（扫描/新增/导入，列表内容可能整体变化）重置回第 0 页。
      refreshCurrentView(opts?.keepPage === true);
      fetchSeries();
      fetchTags();
      fetchStats();
      fetchPendingCount();
      fetchContinue();
    },
    [refreshCurrentView, fetchSeries, fetchTags, fetchStats, fetchPendingCount, fetchContinue]
  );

  // ===== 播放 =====
  // 交给系统默认播放器（PotPlayer），续播由 PotPlayer 内置记忆位置负责
  const play = async (v: VideoWithTags) => {
    if (!v.file_path) {
      toast.err("该记录无文件路径（可能已删除），无法播放");
      return;
    }
    try {
      // 点击播放即标记"最近观看"（失败不阻塞播放），成功后刷新继续观看轨道。
      void api
        .markPlayed(v.id)
        .then(() => fetchContinue())
        .catch(() => {});
      await api.openFile(v.file_path);
    } catch (e) {
      toast.err(String(e));
    }
  };

  // ===== 时长批量补全 =====
  // 背景：扫描流程从不写 duration，导致 PotPlayer 同步只能拿到"看到第几毫秒"却算不出百分比，
  // 进度功能形同虚设。这里一次性把存量的片长补齐，进度百分比才能真正显示出来。
  const doBackfill = useCallback(async () => {
    setBackfill({ done: 0, total: 0, filled: 0, failed: 0, current: "正在统计…" });
    // 必须先挂监听再发命令，否则开头几条进度事件会漏掉
    const unlisten = await listen<BackfillProgress>("backfill-progress", (e) => {
      setBackfill(e.payload);
    });
    try {
      const r = await api.backfillDurations();
      if (r.total === 0) {
        toast.ok("所有视频都已有片长，无需补全");
      } else if (r.cancelled) {
        toast.ok(`已中断：成功 ${r.filled} · 失败 ${r.failed}（已完成的会保留）`);
      } else {
        toast.ok(`补全完成：成功 ${r.filled} · 失败 ${r.failed}`);
      }
      // 时长补齐后再同步一次，让播放进度立刻按新时长算出来；最后统一刷新一次。
      await syncPlayerProgress(false);
      afterDataChange();
    } catch (e) {
      toast.err(String(e));
    } finally {
      unlisten();
      setBackfill(null);
    }
  }, [afterDataChange, syncPlayerProgress]);

  // ===== 导出 / 导入 =====
  // 序列化与写盘都在后端 blocking 线程完成，WebView 只收结果，不再 stringify 几十 MB。
  const doExport = async () => {
    try {
      const path = await save({
        defaultPath: `cinevault-backup-${new Date().toISOString().slice(0, 10)}.json`,
        filters: [{ name: "JSON 备份", extensions: ["json"] }],
      });
      if (!path) return;
      toast.ok("正在生成备份文件…");
      const r = await api.exportJsonToFile(path);
      toast.ok(`已导出：${r.videos} 视频 · ${r.series} 剧集`);
    } catch (e) {
      toast.err(String(e));
    }
  };

  const doImport = async () => {
    try {
      const path = await open({
        multiple: false,
        filters: [{ name: "JSON 备份", extensions: ["json"] }],
      });
      if (typeof path !== "string" || !path) return;
      setPendingImport(path);
    } catch (e) {
      toast.err(String(e));
    }
  };

  const runImport = async (path: string) => {
    try {
      toast.ok("正在校验备份并生成当前库快照…");
      const r = await api.importJsonFromFile(path);
      toast.ok(
        `导入完成：视频 ${r.imported_videos} · 剧集 ${r.imported_series} · 标签 ${r.imported_tags}`
      );
      if (r.backup_path) {
        toast.ok(`导入前快照：${r.backup_path}`);
      }
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
  };

  const viewTitle: Record<ViewKey, string> = {
    all: "全部视频",
    movies: "电影",
    series: "剧集",
    random: "随机推荐",
  };

  // ===== 渲染 =====
  const filters = { query, selectedTagIds, watchStates, ratingRange };
  const filtersActive = hasActiveFilters(filters);
  const filteredSeries = series.filter((s) => matchesSeriesFilters(s, filters));
  // 随机推荐只吃搜索词：观看状态/评分筛选对它无意义，控件也已隐藏。
  const filteredRandom = randomVideos.filter((v) => matchesVideoQuery(v, query));
  const librarySummary = stats
    ? `${stats.total_series} 部剧集 · ${formatSize(stats.total_size)}${
        stats.average_rating > 0 ? ` · 均分 ${stats.average_rating.toFixed(1)}` : ""
      }`
    : "";
  const countLabel =
    view === "series"
      ? `${filteredSeries.length} 部`
      : view === "random"
        ? `${filteredRandom.length} 部`
        : `${total} 条`;
  const viewSub =
    view === "series"
      ? `共 ${series.length} 部剧集${filtersActive ? ` · 当前显示 ${filteredSeries.length} 部` : ""}`
      : view === "random"
        ? "只抽未看 / 在看的电影"
        : librarySummary;

  return (
    <div className="app">
      <Sidebar
        view={view}
        onView={(v) => {
          setView(v);
          setCurrentSeriesId(null);
          // 类型筛选已并入视图："电影"视图在后端强制 kind=movie，侧栏无需再联动顶栏
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
        onOpenPending={() => setShowPending(true)}
        onTagsChanged={fetchTags}
        onOpenSettings={() => setShowSettings(true)}
      />

      <main className="main">
        <TopBar
          query={query}
          onQuery={onQueryChange}
          sort={sort}
          onSort={setSort}
          watchStates={watchStates}
          onWatchStates={setWatchStates}
          ratingRange={ratingRange}
          onRatingRange={setRatingRange}
          hideFilters={view === "random"}
        />

        <div className="content">
          {currentSeries ? (
            <SeriesDetailView
              series={currentSeries}
              reloadKey={episodeReloadKey}
              onBack={() => setCurrentSeriesId(null)}
              tags={tags}
              onChanged={() => afterDataChange({ keepPage: true })}
              onOpenVideo={(v) => setDetailVideo(v)}
              onPlayVideo={play}
            />
          ) : (
            <>
              <div className="view-header">
                <div className="view-heading">
                  <div className="view-title-row">
                    <h1 className="view-title">{viewTitle[view]}</h1>
                    <span className="view-count">{countLabel}</span>
                  </div>
                  {viewSub && <div className="view-sub">{viewSub}</div>}
                </div>
                {view === "random" && (
                  <div className="view-actions">
                    <button
                      type="button"
                      className="btn btn-primary view-action-btn"
                      onClick={() => setRandomTicket((t) => t + 1)}
                      disabled={randomLoading}
                      title="重新随机抽取 20 部（仅未看过 / 在看）"
                    >
                      <Icon name="dice" size={14} />
                      {randomLoading ? "抽取中…" : "换一批"}
                    </button>
                  </div>
                )}
              </div>

              {!filtersActive && (view === "all" || view === "movies") && (
                <ContinueWatching
                  videos={
                    view === "movies"
                      ? continueVideos.filter((v) => v.kind === "movie")
                      : continueVideos
                  }
                  onOpen={setDetailVideo}
                  onPlay={play}
                  onChanged={() => afterDataChange({ keepPage: true })}
                />
              )}

              {view === "series" ? (
                <SeriesGrid
                  series={filteredSeries}
                  sort={sort}
                  onOpen={(s) => {
                    setCurrentSeriesId(s.id);
                    setSelectedTagIds([]);
                  }}
                  tags={tags}
                  onChanged={afterDataChange}
                />
              ) : view === "random" ? (
                randomVideos.length === 0 ? (
                  <div className="empty">
                    <div className="empty-icon">
                      <Icon name="dice" size={28} />
                    </div>
                    <p>{randomLoading ? "正在抽取…" : "暂时没有可推荐的电影"}</p>
                    <span>未看过和在看的电影都已抽完，或库中尚无电影</span>
                  </div>
                ) : (
                  <VideoList
                    videos={filteredRandom}
                    onOpen={setDetailVideo}
                    onPlay={play}
                    onChanged={() => {
                      void refreshRandomInPlace();
                      afterDataChange({ keepPage: true });
                    }}
                  />
                )
              ) : (
                <>
                  <VideoList
                    videos={videos}
                    onOpen={setDetailVideo}
                    onPlay={play}
                    onChanged={() => afterDataChange({ keepPage: true })}
                    loading={loading}
                  />
                  {hasMore && (
                    <div className="load-more">
                      <button
                        type="button"
                        className="btn btn-ghost"
                        onClick={() => fetchVideos(page + 1, true)}
                        disabled={loading}
                      >
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
        <TagManager
          onClose={() => setShowTagManager(false)}
          onChanged={() => afterDataChange({ keepPage: true })}
        />
      )}
      {showLanAccess && <LanAccessModal onClose={() => setShowLanAccess(false)} />}
      {showSettings && (
        <SettingsDrawer
          onClose={() => setShowSettings(false)}
          pendingCount={pendingCount}
          onOpenScan={() => setShowScan(true)}
          onOpenAddSeries={() => setShowAddSeries(true)}
          onOpenPending={() => setShowPending(true)}
          onRefreshAll={() => void refreshAll()}
          refreshing={refreshing}
          onExport={doExport}
          onImport={doImport}
          backfill={backfill}
          onBackfill={() => void doBackfill()}
          onCancelBackfill={() => void api.cancelDurationBackfill()}
          onOpenLanAccess={() => setShowLanAccess(true)}
        />
      )}
      {pendingImport && (
        <ConfirmDialog
          title="导入备份"
          message={`将从「${pendingImport.split(/[\\/]/).pop() ?? pendingImport}」导入数据。\n\n同名视频的评分、进度与历史会以备份值为准；本地新增的其他记录会保留。\n\n导入前会自动生成当前数据库快照，可随时回滚。`}
          confirmText="备份并导入"
          onConfirm={() => {
            const path = pendingImport;
            setPendingImport(null);
            if (path) void runImport(path);
          }}
          onCancel={() => setPendingImport(null)}
        />
      )}
      {detailVideo && (
        <DetailModal
          video={detailVideo}
          tags={tags}
          onClose={() => setDetailVideo(null)}
          onChanged={() => {
            afterDataChange({ keepPage: true });
            // 详情弹窗可能从剧集详情页打开；App 的 series 刷新够不到子组件的集列表，
            // 递增信号让它同步刷新每集进度/已看状态。
            setEpisodeReloadKey((k) => k + 1);
          }}
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
