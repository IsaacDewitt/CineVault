import React from "react";
import type { SortKey, WatchStatus } from "../types";

export function TopBar({
  query,
  onQuery,
  kind,
  onKind,
  minRating,
  onMinRating,
  sort,
  onSort,
  watchStatus,
  onWatchStatus,
}: {
  query: string;
  onQuery: (q: string) => void;
  kind: string;
  onKind: (k: string) => void;
  minRating: number;
  onMinRating: (r: number) => void;
  sort: SortKey;
  onSort: (s: SortKey) => void;
  watchStatus: WatchStatus;
  onWatchStatus: (w: WatchStatus) => void;
}) {
  return (
    <header className="topbar">
      <div className="search-box">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
          <circle cx="11" cy="11" r="7" />
          <path d="M21 21l-4.35-4.35" />
        </svg>
        <input
          placeholder="搜索标题 / 文件名…"
          value={query}
          onChange={(e) => onQuery(e.target.value)}
        />
      </div>
      <div className="topbar-right">
        <select className="select" value={kind} onChange={(e) => onKind(e.target.value)}>
          <option value="all">全部类型</option>
          <option value="movie">仅电影</option>
          <option value="series">仅剧集</option>
          <option value="unknown">待定</option>
        </select>
        <select className="select" value={watchStatus} onChange={(e) => onWatchStatus(e.target.value as WatchStatus)}>
          <option value="all">全部状态</option>
          <option value="unwatched">未看过</option>
          <option value="partial">看了一半</option>
          <option value="watched">已看过</option>
        </select>
        <select className="select" value={minRating} onChange={(e) => onMinRating(Number(e.target.value))}>
          <option value={0}>全部评分</option>
          <option value={6}>6 分以上</option>
          <option value={8}>8 分以上</option>
        </select>
        <select className="select" value={sort} onChange={(e) => onSort(e.target.value as SortKey)}>
          <option value="last_watched">最近播放</option>
          <option value="updated">最近更新</option>
          <option value="name">按名称</option>
          <option value="size">按大小</option>
          <option value="rating">按评分</option>
        </select>
      </div>
    </header>
  );
}
