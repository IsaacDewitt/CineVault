import { useEffect, useRef, useState } from "react";
import type { RefObject } from "react";
import { api } from "../api";

type Job = { videoId: number; resolve: (url: string | null) => void };

const cache = new Map<number, string | null>();
const pending = new Map<number, Promise<string | null>>();
const queue: Job[] = [];
const MAX_CONCURRENT = 2;
let active = 0;

/** 内存缓存上限：每张 data URL 几十 KB，翻遍几千条卡片会吃到几百 MB。 */
const CACHE_LIMIT = 240;

/** 写缓存并按近似 LRU 淘汰：重新写入的键移到队尾，超限从最旧的开始丢。 */
function cacheSet(videoId: number, url: string | null) {
  if (cache.has(videoId)) cache.delete(videoId);
  cache.set(videoId, url);
  if (cache.size <= CACHE_LIMIT) return;
  for (const key of cache.keys()) {
    if (cache.size <= CACHE_LIMIT) break;
    if (key === videoId) continue;
    cache.delete(key);
  }
}

/** 同一 videoId 可能被主网格 / 继续观看 / 剧集卡同时渲染，刷新后要一起换图。 */
const listeners = new Map<number, Set<(url: string | null) => void>>();
/** 刷新中的任务：同一个 id 只跑一次 ffmpeg，连点复用同一个 Promise。 */
const refreshing = new Map<number, Promise<string | null>>();

function subscribe(videoId: number, fn: (url: string | null) => void): () => void {
  let subs = listeners.get(videoId);
  if (!subs) {
    subs = new Set();
    listeners.set(videoId, subs);
  }
  subs.add(fn);
  return () => {
    subs!.delete(fn);
    if (subs!.size === 0) listeners.delete(videoId);
  };
}

function broadcast(videoId: number, url: string | null) {
  cacheSet(videoId, url);
  const subs = listeners.get(videoId);
  if (subs) for (const fn of subs) fn(url);
}

function pump() {
  while (active < MAX_CONCURRENT && queue.length > 0) {
    const job = queue.shift()!;
    active += 1;
    void api
      .getVideoThumbnail(job.videoId)
      .then((url) => {
        // 常规加载期间若用户点了刷新，别让这张旧图写回缓存 / 抢在广播前显示。
        if (!refreshing.has(job.videoId)) cacheSet(job.videoId, url);
        job.resolve(url);
      })
      .catch(() => {
        if (!refreshing.has(job.videoId)) cacheSet(job.videoId, null);
        job.resolve(null);
      })
      .finally(() => {
        active -= 1;
        pending.delete(job.videoId);
        pump();
      });
  }
}

/** 全局限并发：可见卡片最多同时生成 2 张缩略图，避免一次 80 个 ffmpeg。 */
function loadThumbnail(videoId: number): Promise<string | null> {
  const cached = cache.get(videoId);
  if (cached !== undefined) return Promise.resolve(cached);
  const inFlight = pending.get(videoId);
  if (inFlight) return inFlight;

  const p = new Promise<string | null>((resolve) => {
    queue.push({ videoId, resolve });
    pump();
  });
  pending.set(videoId, p);
  return p;
}

/** 菜单点击前的同步去重判断（避免连点弹一堆 toast）。 */
export function isPosterRefreshing(videoId: number): boolean {
  return refreshing.has(videoId);
}

/**
 * 刷新一张封面：
 *  1. 立刻把该 id 的所有卡片降级成文字片头卡（旧 data URL 从内存缓存清掉）；
 *  2. 后端删掉 thumbnails/{id}.jpg 并在随机位置重抽一帧；
 *  3. 新图广播给所有挂载中的同 id 卡片（主网格 / 继续观看 / 剧集卡）。
 *
 * 失败时把该 id 从 cache 移除，下次卡片重新进入视口还能再试一次。
 */
export function refreshThumbnail(videoId: number): Promise<string | null> {
  const inFlight = refreshing.get(videoId);
  if (inFlight) return inFlight;

  broadcast(videoId, null);
  const p = api
    .refreshVideoThumbnail(videoId)
    .then((url) => {
      broadcast(videoId, url);
      return url;
    })
    .catch((e) => {
      cache.delete(videoId);
      throw e;
    })
    .finally(() => {
      refreshing.delete(videoId);
    });
  refreshing.set(videoId, p);
  return p;
}

/**
 * 懒加载缩略图：卡片进入视口前不触发 ffmpeg，离开后保留结果。
 * ref 必须挂在海报容器上；没有 IntersectionObserver 时立即加载。
 */
export function useThumbnail(videoId?: number | null): {
  ref: RefObject<HTMLDivElement | null>;
  url: string | null;
} {
  const ref = useRef<HTMLDivElement | null>(null);
  const [url, setUrl] = useState<string | null>(
    videoId ? cache.get(videoId) ?? null : null
  );

  useEffect(() => {
    setUrl(videoId ? cache.get(videoId) ?? null : null);
    if (!videoId) return;

    let alive = true;
    let observer: IntersectionObserver | null = null;
    // 订阅同 id 的刷新广播：一处点了「刷新封面图」，所有同 id 卡片一起换。
    const unsubscribe = subscribe(videoId, (next) => setUrl(next));

    const load = () => {
      void loadThumbnail(videoId).then((next) => {
        if (alive && !refreshing.has(videoId)) setUrl(next);
      });
    };

    const el = ref.current;
    if (!el || typeof IntersectionObserver === "undefined") {
      load();
    } else {
      observer = new IntersectionObserver(
        (entries) => {
          if (entries.some((entry) => entry.isIntersecting)) {
            observer?.disconnect();
            observer = null;
            load();
          }
        },
        { rootMargin: "320px 0px" }
      );
      observer.observe(el);
    }

    return () => {
      alive = false;
      unsubscribe();
      observer?.disconnect();
    };
  }, [videoId]);

  return { ref, url };
}
