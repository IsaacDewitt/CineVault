import { useEffect, useState } from "react";
import type { MouseEvent } from "react";
import type { Tag, ViewKey } from "../types";
import { api } from "../api";
import { ContextMenu, toast } from "./ui";
import { Icon } from "./icons";
import type { IconName } from "./icons";

export function Sidebar({
  view,
  onView,
  tags,
  selectedTagIds,
  onToggleTag,
  counts,
  pendingCount,
  onOpenTagManager,
  onOpenPending,
  onTagsChanged,
  onOpenSettings,
}: {
  view: ViewKey;
  onView: (v: ViewKey) => void;
  tags: Tag[];
  selectedTagIds: number[];
  onToggleTag: (id: number) => void;
  counts: { all: number; movies: number; series: number };
  pendingCount: number;
  onOpenTagManager: () => void;
  onOpenPending: () => void;
  onTagsChanged: () => void | Promise<void>;
  onOpenSettings: () => void;
}) {
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState("");
  const [justCreatedId, setJustCreatedId] = useState<number | null>(null);
  // 侧栏右键菜单：导航项与标签页共用一个，只提供标签相关操作
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);

  const openSidebarMenu = (e: MouseEvent<HTMLElement>) => {
    e.preventDefault();
    // 新建标签的输入框里不弹导航菜单（文本编辑交给系统快捷键）
    if ((e.target as HTMLElement).closest("input, textarea, [contenteditable='true']")) return;
    setMenu({ x: e.clientX, y: e.clientY });
  };

  useEffect(() => {
    if (justCreatedId === null) return;
    document
      .querySelector(`[data-tag-id="${justCreatedId}"]`)
      ?.scrollIntoView({ block: "nearest" });
    const timer = window.setTimeout(() => setJustCreatedId(null), 1800);
    return () => window.clearTimeout(timer);
  }, [justCreatedId, tags]);

  const items: { key: ViewKey; label: string; icon: IconName; badge?: number }[] = [
    { key: "all", label: "全部视频", icon: "list", badge: counts.all },
    { key: "movies", label: "电影", icon: "film", badge: counts.movies },
    { key: "series", label: "剧集", icon: "tv", badge: counts.series },
    { key: "random", label: "随机推荐", icon: "dice" },
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

      <nav className="nav" aria-label="媒体库导航" onContextMenu={openSidebarMenu}>
        <div className="nav-group-title">媒体库</div>
        {items.map((it) => (
          <button
            type="button"
            key={it.key}
            className={`nav-item${view === it.key ? " active" : ""}`}
            onClick={() => onView(it.key)}
            aria-current={view === it.key ? "page" : undefined}
          >
            <Icon name={it.icon} size={17} className="nav-ico" />
            <span>{it.label}</span>
            {it.badge !== undefined && it.badge > 0 && (
              <span className="nav-badge">{it.badge}</span>
            )}
          </button>
        ))}

        {pendingCount > 0 && (
          <button type="button" className="nav-item nav-pending" onClick={onOpenPending}>
            <Icon name="info" size={17} className="nav-ico" />
            <span>待确认分类</span>
            <span className="nav-badge">{pendingCount}</span>
          </button>
        )}

        <div className="nav-group-title">标签</div>
        {tags.map((t) => (
          <button
            type="button"
            key={t.id}
            data-tag-id={t.id}
            className={`tag-item${selectedTagIds.includes(t.id) ? " selected" : ""}${
              justCreatedId === t.id ? " just-created" : ""
            }`}
            onClick={() => onToggleTag(t.id)}
            aria-pressed={selectedTagIds.includes(t.id)}
          >
            <span className="tag-dot" style={{ background: t.color }} />
            <span className="tag-name">{t.name}</span>
            {typeof t.video_count === "number" && (
              <span className="tag-count">{t.video_count}</span>
            )}
          </button>
        ))}

        {adding ? (
          <div className="tag-create">
            <input
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
              type="button"
              className="btn btn-primary tag-create-btn"
              onClick={() => void submitTag()}
              title="创建（回车）"
              aria-label="创建标签"
            >
              <Icon name="check" size={14} />
            </button>
            <button
              type="button"
              className="btn btn-ghost tag-create-btn"
              onClick={() => {
                setName("");
                setAdding(false);
              }}
              title="取消（Esc）"
              aria-label="取消创建标签"
            >
              <Icon name="close" size={14} />
            </button>
          </div>
        ) : (
          <button type="button" className="tag-add" onClick={() => setAdding(true)}>
            <Icon name="plus" size={14} />
            新建标签
          </button>
        )}
        <button type="button" className="tag-add" onClick={onOpenTagManager}>
          <Icon name="settings" size={14} />
          管理标签
        </button>
      </nav>

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={[
            {
              label: "新建标签",
              icon: <Icon name="plus" size={13} />,
              onClick: () => setAdding(true),
            },
            {
              label: "管理标签",
              icon: <Icon name="settings" size={13} />,
              onClick: onOpenTagManager,
            },
          ]}
        />
      )}

      <div className="sidebar-footer">
        <button type="button" className="sidebar-settings" onClick={onOpenSettings}>
          <Icon name="settings" size={17} />
          <span>库管理</span>
          <span className="sidebar-settings-hint">扫描 · 备份 · 片长</span>
        </button>
      </div>
    </aside>
  );
}
