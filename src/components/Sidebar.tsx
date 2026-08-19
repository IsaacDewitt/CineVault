import React, { useState } from "react";
import type { Tag, ViewKey } from "../types";
import { api } from "../api";
import { toast } from "./ui";

const ICONS: Record<string, string> = {
  all: "M4 6h16M4 12h16M4 18h10",
  movies: "M3 5l9-3 9 3v14l-9 3-9-3V5zm9-3v14m6-9.5L6 7",
  series: "M4 6H2v14c0 1.1.9 2 2 2h14v-2H4V6zm16-4H8c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm-8 12.5v-9l6 4.5-6 4.5z",
  favorites: "M12 17.27L18.18 21l-1.64-7.03L22 9.24l-7.19-.61L12 2 9.19 8.63 2 9.24l5.46 4.73L5.82 21z",
  recent: "M19 3H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm-2 10h-4v4h-2v-4H7v-2h4V7h2v4h4v2z",
  history: "M13 3c-4.97 0-9 4.03-9 9H1l3.89 3.89.07.14L9 12H6c0-3.87 3.13-7 7-7s7 3.13 7 7-3.13 7-7 7c-1.93 0-3.68-.79-4.94-2.06l-1.42 1.42C8.27 19.99 10.51 21 13 21c4.97 0 9-4.03 9-9s-4.03-9-9-9zm-1 5v5l4.28 2.54.72-1.21-3.5-2.08V8H12z",
  pending: "M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-2h2v2zm0-4h-2V7h2v6z",
};

function NavIcon({ path }: { path: string }) {
  return (
    <svg className="nav-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
      <path d={path} />
    </svg>
  );
}

export function Sidebar({
  view,
  onView,
  tags,
  selectedTagIds,
  onToggleTag,
  counts,
  pendingCount,
  onOpenTagManager,
  onOpenScan,
  onOpenAddSeries,
  onOpenPending,
  onExport,
  onImport,
}: {
  view: ViewKey;
  onView: (v: ViewKey) => void;
  tags: Tag[];
  selectedTagIds: number[];
  onToggleTag: (id: number) => void;
  counts: { all: number; movies: number; series: number; favorites: number };
  pendingCount: number;
  onOpenTagManager: () => void;
  onOpenScan: () => void;
  onOpenAddSeries: () => void;
  onOpenPending: () => void;
  onExport: () => void;
  onImport: () => void;
}) {
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState("");

  const items: { key: ViewKey; label: string; icon: string; badge?: number }[] = [
    { key: "all", label: "全部视频", icon: "all", badge: counts.all },
    { key: "movies", label: "电影", icon: "movies", badge: counts.movies },
    { key: "series", label: "剧集", icon: "series", badge: counts.series },
    { key: "favorites", label: "收藏夹", icon: "favorites", badge: counts.favorites },
    { key: "recent", label: "最近更新", icon: "recent" },
    { key: "history", label: "播放历史", icon: "history" },
  ];

  const submitTag = async () => {
    const n = name.trim();
    if (!n) return;
    try {
      await api.createTag(n, "#e8b34b");
      setName("");
      setAdding(false);
      toast.ok("标签已创建");
    } catch (e) {
      toast.err(String(e));
    }
  };

  return (
    <aside className="sidebar">
      <div className="logo">
        <div className="logo-mark">CV</div>
        <div>
          <div>CineVault</div>
          <div className="logo-sub">影视库</div>
        </div>
      </div>

      <nav className="nav">
        <div className="nav-group-title">媒体库</div>
        {items.map((it) => (
          <button
            key={it.key}
            className={`nav-item${view === it.key ? " active" : ""}`}
            onClick={() => onView(it.key)}
          >
            <NavIcon path={it.icon} />
            <span>{it.label}</span>
            {it.badge !== undefined && it.badge > 0 && (
              <span className="nav-badge">{it.badge}</span>
            )}
          </button>
        ))}

        {pendingCount > 0 && (
          <button className="nav-item" onClick={onOpenPending} style={{ color: "var(--accent)" }}>
            <NavIcon path="pending" />
            <span>待确认分类</span>
            <span className="nav-badge" style={{ background: "var(--accent-dim)", color: "var(--accent)" }}>
              {pendingCount}
            </span>
          </button>
        )}

        <div className="nav-group-title">标签</div>
        {tags.map((t) => (
          <div
            key={t.id}
            className={`tag-item${selectedTagIds.includes(t.id) ? " selected" : ""}`}
            onClick={() => onToggleTag(t.id)}
          >
            <span className="tag-dot" style={{ background: t.color }} />
            <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{t.name}</span>
            {typeof t.video_count === "number" && (
              <span className="tag-count">{t.video_count}</span>
            )}
          </div>
        ))}

        {adding ? (
          <div style={{ padding: "4px 8px" }}>
            <input
              className="input"
              autoFocus
              placeholder="标签名"
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") submitTag();
                if (e.key === "Escape") setAdding(false);
              }}
            />
          </div>
        ) : (
          <button className="tag-add" onClick={() => setAdding(true)}>
            ＋ 新建标签
          </button>
        )}
        <button className="tag-add" onClick={onOpenTagManager}>
          ⚙ 管理标签
        </button>
      </nav>

      <div style={{ padding: 10, borderTop: "1px solid var(--border)", display: "flex", gap: 6 }}>
        <button className="btn btn-ghost" style={{ flex: 1 }} onClick={onOpenScan}>
          ＋ 扫描
        </button>
        <button
          className="btn btn-ghost"
          style={{ flex: 1 }}
          onClick={onOpenAddSeries}
          title="选一个文件夹，其中所有视频归入同名剧集"
        >
          ＋ 剧集
        </button>
      </div>
      <div style={{ padding: "0 10px 10px", display: "flex", gap: 6 }}>
        <button className="btn btn-ghost" style={{ flex: 1 }} onClick={onExport} title="导出全部数据为 JSON（备份/迁移）">
          ⬇ 导出
        </button>
        <button className="btn btn-ghost" style={{ flex: 1 }} onClick={onImport} title="从 JSON 导入数据">
          ⬆ 导入
        </button>
      </div>
    </aside>
  );
}
