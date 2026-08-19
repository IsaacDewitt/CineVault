import React, { useState } from "react";
import type { SeriesOverview, SortKey, Tag } from "../types";
import { api } from "../api";
import {
  ConfirmDialog,
  ContextMenu,
  Modal,
  RatingPicker,
  Stars,
  TagChips,
  formatSize,
  progressPct,
  toast,
} from "./ui";

export function SeriesGrid({
  series,
  sort,
  onOpen,
  tags,
  onChanged,
}: {
  series: SeriesOverview[];
  sort: SortKey;
  onOpen: (s: SeriesOverview) => void;
  tags: Tag[];
  onChanged: () => void;
}) {
  const [menu, setMenu] = useState<{ x: number; y: number; item: SeriesOverview } | null>(null);
  const [ratingItem, setRatingItem] = useState<SeriesOverview | null>(null);
  const [tagItem, setTagItem] = useState<SeriesOverview | null>(null);
  const [confirmDel, setConfirmDel] = useState<SeriesOverview | null>(null);

  if (series.length === 0) {
    return (
      <div className="empty">
        <div className="emoji">📺</div>
        <p>还没有剧集</p>
      </div>
    );
  }

  const openFolder = async (s: SeriesOverview) => {
    if (!s.folder_path) {
      toast.err("该剧集没有记录文件夹路径");
      return;
    }
    try {
      await api.openFolder(s.folder_path);
    } catch (e) {
      toast.err(String(e));
    }
  };

  const menuItems = (s: SeriesOverview) => [
    { label: "▶ 打开剧集", onClick: () => onOpen(s) },
    { label: "📁 打开所在文件夹", onClick: () => openFolder(s) },
    { sep: true },
    { label: s.is_favorite ? "★ 取消收藏" : "☆ 收藏", onClick: async () => {
        try {
          await api.toggleSeriesFavorite(s.id);
          onChanged();
        } catch (e) {
          toast.err(String(e));
        }
      } },
    { label: "⭐ 评分…", onClick: () => setRatingItem(s) },
    { label: "🏷 标签…", onClick: () => setTagItem(s) },
    { label: "👁 全部标记为已看过", onClick: async () => {
        try {
          const n = await api.markSeriesWatched(s.id, true);
          toast.ok(`已标记 ${n} 集`);
          onChanged();
        } catch (e) {
          toast.err(String(e));
        }
      } },
    { label: "👁 全部标记为未看过", onClick: async () => {
        try {
          const n = await api.markSeriesWatched(s.id, false);
          toast.ok(`已标记 ${n} 集为未看过`);
          onChanged();
        } catch (e) {
          toast.err(String(e));
        }
      } },
    { sep: true },
    { label: "🗑 删除剧集（记录）", danger: true, onClick: () => setConfirmDel(s) },
  ];

  // 顶栏排序：剧集全量在内存排序（updated_at 为 SQLite "YYYY-MM-DD HH:MM:SS"，字符串比较即时间序）
  const sorted = [...series].sort((a, b) => {
    switch (sort) {
      case "updated":
        return b.updated_at.localeCompare(a.updated_at);
      case "last_watched":
        return (b.last_watched_at ?? "").localeCompare(a.last_watched_at ?? "");
      case "name":
        return a.name.localeCompare(b.name, "zh-Hans-CN");
      case "size":
        return b.total_size - a.total_size;
      case "rating":
        return b.rating - a.rating;
      default:
        return 0;
    }
  });

  return (
    <div>
      <div className="series-grid">
        {sorted.map((s) => {
          const pct = progressPct(s.progress);
          return (
            <div
              key={s.id}
              className="series-card"
              onClick={() => onOpen(s)}
              onContextMenu={(e) => {
                e.preventDefault();
                setMenu({ x: e.clientX, y: e.clientY, item: s });
              }}
            >
              <div className="s-title">
                {s.is_favorite && <span style={{ color: "var(--accent)", marginRight: 5 }}>★</span>}
                {s.name}
              </div>
              <div className="s-meta">
                <span>
                  已看 {s.watched_episodes}/{s.total_episodes} 集
                </span>
                <span>·</span>
                <span>{formatSize(s.total_size)}</span>
                <span style={{ marginLeft: "auto" }}>
                  <Stars rating={s.rating} />
                </span>
              </div>
              <div className="s-meta" style={{ marginTop: 4 }}>
                <TagChips tags={s.tags} />
              </div>
              <div className="s-bar">
                <div className="s-bar-fill" style={{ width: `${pct}%` }} />
              </div>
            </div>
          );
        })}
      </div>

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={menuItems(menu.item)}
        />
      )}

      {ratingItem && (
        <Modal
          title={`评分 · ${ratingItem.name}`}
          onClose={() => setRatingItem(null)}
          footer={
            <button className="btn btn-ghost" onClick={() => setRatingItem(null)}>
              取消
            </button>
          }
        >
          <RatingPicker
            value={ratingItem.rating}
            onChange={async (r) => {
              try {
                await api.setSeriesRating(ratingItem.id, r);
                toast.ok(r > 0 ? `已评 ${r} 分` : "已取消评分");
                onChanged();
              } catch (e) {
                toast.err(String(e));
              }
              setRatingItem(null);
            }}
          />
        </Modal>
      )}

      {tagItem && (
        <Modal
          title={`为「${tagItem.name}」选择标签`}
          onClose={() => setTagItem(null)}
          footer={
            <button className="btn btn-ghost" onClick={() => setTagItem(null)}>
              关闭
            </button>
          }
        >
          <div className="tag-selector">
            {tags.map((t) => {
              const attached = (tagItem.tags ?? []).some((x) => x.id === t.id);
              return (
                <span
                  key={t.id}
                  className={`tag-pick${attached ? " on" : ""}`}
                  onClick={async () => {
                    try {
                      if (attached) {
                        await api.removeSeriesTag(tagItem.id, t.id);
                      } else {
                        await api.addSeriesTag(tagItem.id, t.id);
                      }
                      onChanged();
                    } catch (e) {
                      toast.err(String(e));
                    }
                  }}
                >
                  <span className="tag-dot" style={{ background: t.color }} />
                  {t.name}
                </span>
              );
            })}
          </div>
        </Modal>
      )}

      {confirmDel && (
        <ConfirmDialog
          title="删除剧集"
          message={`确定删除「${confirmDel.name}」的所有记录？\n\n不会删除本地文件。`}
          danger
          confirmText="删除"
          onConfirm={async () => {
            const s = confirmDel;
            setConfirmDel(null);
            try {
              await api.deleteSeries(s.id);
              toast.ok("剧集已删除");
              onChanged();
            } catch (e) {
              toast.err(String(e));
            }
          }}
          onCancel={() => setConfirmDel(null)}
        />
      )}
    </div>
  );
}
