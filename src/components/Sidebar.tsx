import React, { useEffect, useRef, useState } from "react";
import type { BackfillProgress, Tag, ViewKey } from "../types";
import { api } from "../api";
import { toast } from "./ui";

// 导航图标：统一 24 网格 / 1.7 圆头描边（与「片库网格」图标体系同源）
const ICONS: Record<string, React.ReactNode> = {
  all: <path d="M4 6h16M4 12h16M4 18h10" />,
  movies: (
    <>
      <rect x="3" y="4.5" width="18" height="15" rx="2.6" />
      <path d="M8 4.5v15M16 4.5v15" />
    </>
  ),
  series: (
    <>
      <rect x="2.6" y="4.5" width="18.8" height="12.6" rx="2.6" />
      <path d="M8.4 20.5h7.2M12 17.1v3.4" />
      <path d="M10.6 8.1l4.3 2.7-4.3 2.7z" />
    </>
  ),
  favorites: (
    <path d="M12 20.4c0 0-7.6-4.8-7.6-10A4.7 4.7 0 0 1 12 7.6a4.7 4.7 0 0 1 7.6 2.8c0 5.2-7.6 10-7.6 10z" />
  ),
  recent: (
    <>
      <circle cx="12" cy="12" r="8.8" />
      <path d="M12 6.9V12l3.3 2.1" />
    </>
  ),
  history: (
    <>
      <path d="M3 3v5h5" />
      <path d="M3.05 13A9 9 0 1 0 6 5.3L3 8" />
      <path d="M12 7v5l4 2" />
    </>
  ),
  pending: (
    <>
      <circle cx="12" cy="12" r="8.8" />
      <path d="M12 7.6v5.2" />
      <path d="M12 16.4h.01" />
    </>
  ),
};

function NavIcon({ children }: { children: React.ReactNode }) {
  return (
    <svg className="nav-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
      {children}
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
  onTagsChanged,
  backfill,
  onBackfill,
  onCancelBackfill,
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
  onTagsChanged: () => void | Promise<void>;
  backfill: BackfillProgress | null;
  onBackfill: () => void;
  onCancelBackfill: () => void;
}) {
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState("");
  const [ffprobe, setFfprobe] = useState<boolean | null>(null);
  const [justCreatedId, setJustCreatedId] = useState<number | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // 新标签按 sort_order/name 排序后可能落在列表可视区之外，
  // 滚过去并闪一下——否则用户会以为"新建"没生效（它其实建成了，只是没被看到）。
  useEffect(() => {
    if (justCreatedId === null) return;
    document
      .querySelector(`[data-tag-id="${justCreatedId}"]`)
      ?.scrollIntoView({ block: "nearest" });
    const timer = window.setTimeout(() => setJustCreatedId(null), 1800);
    return () => window.clearTimeout(timer);
  }, [justCreatedId, tags]);

  // ffprobe 是否可用决定 rmvb/flv/avi/ts 等非主流格式能不能补出时长
  useEffect(() => {
    api
      .getFfprobeStatus()
      .then((s) => setFfprobe(s.available))
      .catch(() => setFfprobe(false));
  }, []);

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
    if (!n) {
      setAdding(false);
      return;
    }
    const dup = tags.some((t) => t.name.trim().toLowerCase() === n.toLowerCase());
    try {
      const created = await api.createTag(n, "#e8b34b");
      setJustCreatedId(created.id);
      setName("");
      setAdding(false);
      // 关键：不刷新父级 tags 的话，标签进了库却不在侧栏出现，看起来就是"新建无效"
      await onTagsChanged();
      toast.ok(dup ? `标签「${n}」已存在` : `标签「${n}」已创建`);
    } catch (e) {
      toast.err(String(e));
    }
  };

  return (
    <aside className="sidebar">
      <div className="logo">
        <div className="logo-mark" aria-hidden>
          {/* 应用图标「片库网格」迷你版：四格 + 右下播放键 */}
          <svg width="19" height="19" viewBox="0 0 24 24" fill="none">
            <rect x="5.44" y="5.44" width="6.19" height="6.19" rx="1.56" fill="#fff" fillOpacity="0.95" />
            <rect x="12.38" y="5.44" width="6.19" height="6.19" rx="1.56" fill="#fff" fillOpacity="0.5" />
            <rect x="5.44" y="12.38" width="6.19" height="6.19" rx="1.56" fill="#fff" fillOpacity="0.5" />
            <rect x="12.38" y="12.38" width="6.19" height="6.19" rx="1.56" fill="#fff" fillOpacity="0.95" />
            <path d="M14.84 13.97v3l2.27-1.5z" fill="var(--accent)" />
          </svg>
        </div>
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
            <NavIcon>{ICONS[it.icon]}</NavIcon>
            <span>{it.label}</span>
            {it.badge !== undefined && it.badge > 0 && (
              <span className="nav-badge">{it.badge}</span>
            )}
          </button>
        ))}

        {pendingCount > 0 && (
          <button className="nav-item" onClick={onOpenPending} style={{ color: "var(--accent)" }}>
            <NavIcon>{ICONS.pending}</NavIcon>
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
            data-tag-id={t.id}
            className={`tag-item${selectedTagIds.includes(t.id) ? " selected" : ""}${
              justCreatedId === t.id ? " just-created" : ""
            }`}
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
          <div style={{ padding: "4px 8px", display: "flex", gap: 6 }}>
            <input
              ref={inputRef}
              className="input"
              autoFocus
              placeholder="标签名"
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void submitTag();
                if (e.key === "Escape") {
                  setName("");
                  setAdding(false);
                }
              }}
            />
            <button
              className="btn btn-primary"
              style={{ padding: "0 10px", fontSize: 13 }}
              onClick={() => void submitTag()}
              title="创建（回车）"
            >
              ✓
            </button>
            <button
              className="btn btn-ghost"
              style={{ padding: "0 10px", fontSize: 13 }}
              onClick={() => {
                setName("");
                setAdding(false);
              }}
              title="取消（Esc）"
            >
              ✕
            </button>
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

      <div style={{ padding: "0 10px 10px" }}>
        {backfill ? (
          <div style={{ display: "flex", flexDirection: "column", gap: 5 }}>
            <div className="s-bar">
              <div
                className="s-bar-fill"
                style={{
                  width: `${backfill.total > 0 ? Math.round((backfill.done / backfill.total) * 100) : 0}%`,
                }}
              />
            </div>
            <div className="v-sub" title={backfill.current}>
              {backfill.total > 0
                ? `${backfill.done}/${backfill.total} · 成功 ${backfill.filled}`
                : "正在统计…"}
            </div>
            <button className="btn btn-ghost" style={{ fontSize: 11 }} onClick={onCancelBackfill}>
              中断补全
            </button>
          </div>
        ) : (
          <button
            className="btn btn-ghost"
            style={{ width: "100%" }}
            onClick={onBackfill}
            title={
              ffprobe === false
                ? "未检测到 ffprobe：mp4/mkv 仍可补全，rmvb/flv/avi/ts 等格式会跳过"
                : "为所有缺片长的视频补上时长，让播放进度百分比能算出来"
            }
          >
            ⏱ 补全片长
            {ffprobe === false && <span style={{ color: "var(--text-3)" }}>（无 ffprobe）</span>}
          </button>
        )}
      </div>
    </aside>
  );
}
