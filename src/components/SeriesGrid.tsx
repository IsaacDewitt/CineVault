import { useState } from "react";
import type { KeyboardEvent, MouseEvent } from "react";
import type { SeriesOverview, SortSpec, Tag } from "../types";
import { api } from "../api";
import {
  ConfirmDialog,
  ContextMenu,
  Modal,
  RatingPicker,
  Stars,
  TagChips,
  formatSize,
  toast,
} from "./ui";
import type { MenuItem } from "./ui";
import { PosterArt, refreshPoster } from "./VideoCard";
import { sortSeriesList } from "../lib/library";
import { Icon } from "./icons";

export function SeriesGrid({
  series,
  sort,
  onOpen,
  tags,
  onChanged,
}: {
  series: SeriesOverview[];
  sort: SortSpec;
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
        <div className="empty-icon">
          <Icon name="tv" size={28} />
        </div>
        <p>没有符合条件的剧集</p>
        <span>换个关键词，或调整观看状态与评分范围</span>
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

  const menuItems = (s: SeriesOverview): MenuItem[] => [
    {
      icon: <Icon name="play" size={14} />,
      label: "打开剧集",
      onClick: () => onOpen(s),
    },
    {
      icon: <Icon name="folder" size={14} />,
      label: "打开所在文件夹",
      onClick: () => openFolder(s),
    },
    {
      icon: <Icon name="refresh" size={14} />,
      label: "刷新封面图",
      onClick: () => void refreshPoster(s.cover_video_id),
    },
    { sep: true },
    {
      icon: <Icon name="star" size={14} />,
      label: "评分…",
      onClick: () => setRatingItem(s),
    },
    {
      icon: <Icon name="tag" size={14} />,
      label: "标签…",
      onClick: () => setTagItem(s),
    },
    {
      icon: <Icon name="eye" size={14} />,
      label: "全部标记为已看过",
      onClick: async () => {
        try {
          const n = await api.markSeriesWatched(s.id, true);
          toast.ok(`已标记 ${n} 集`);
          onChanged();
        } catch (e) {
          toast.err(String(e));
        }
      },
    },
    {
      icon: <Icon name="eye-off" size={14} />,
      label: "全部标记为未看过",
      onClick: async () => {
        try {
          const n = await api.markSeriesWatched(s.id, false);
          toast.ok(`已标记 ${n} 集为未看过`);
          onChanged();
        } catch (e) {
          toast.err(String(e));
        }
      },
    },
    { sep: true },
    {
      icon: <Icon name="trash" size={14} />,
      label: "删除剧集（记录）",
      danger: true,
      onClick: () => setConfirmDel(s),
    },
  ];

  const sorted = sortSeriesList(series, sort);

  return (
    <div>
      <div className="series-grid">
        {sorted.map((s) => {
          const pct = Math.max(0, Math.min(100, s.progress));
          const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              onOpen(s);
            }
          };
          return (
            <article
              key={s.id}
              className="series-card"
              onClick={() => onOpen(s)}
              onContextMenu={(e: MouseEvent) => {
                e.preventDefault();
                setMenu({ x: e.clientX, y: e.clientY, item: s });
              }}
              onKeyDown={onKeyDown}
              tabIndex={0}
              role="button"
              aria-label={`${s.name}，已看 ${s.watched_episodes}/${s.total_episodes} 集`}
            >
              <PosterArt
                videoId={s.cover_video_id}
                title={s.name}
                label="剧集"
                kind="series"
              >
                <span className="poster-kind">
                  <span className="series-count-badge">{s.total_episodes} 集</span>
                </span>
                {pct > 0 && (
                  <div className={`poster-progress${pct >= 99.9 ? " done" : ""}`}>
                    <span style={{ width: `${pct}%` }} />
                  </div>
                )}
              </PosterArt>
              <div className="series-body">
                <div className="s-title">{s.name}</div>
                <div className="s-meta">
                  <span>
                    已看 {s.watched_episodes}/{s.total_episodes} 集
                  </span>
                  <span className="meta-sep">·</span>
                  <span>{formatSize(s.total_size)}</span>
                </div>
                <div className="s-foot">
                  <Stars rating={s.rating} />
                  <div className="s-tags">
                    <TagChips tags={s.tags} />
                  </div>
                </div>
              </div>
            </article>
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
                <button
                  type="button"
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
                </button>
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
