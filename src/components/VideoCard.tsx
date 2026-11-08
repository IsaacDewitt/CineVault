import { useRef, useState } from "react";
import type {
  CSSProperties,
  KeyboardEvent,
  MouseEvent,
  ReactNode,
} from "react";
import type { VideoKind, VideoWithTags } from "../types";
import { api } from "../api";
import {
  isPosterRefreshing,
  refreshThumbnail,
  useThumbnail,
} from "../hooks/useThumbnail";
import { formatDuration } from "../lib/library";
import {
  ConfirmDialog,
  ContextMenu,
  KindBadge,
  Modal,
  RatingPicker,
  Stars,
  episodeLabel,
  formatSize,
  toast,
} from "./ui";
import type { MenuItem } from "./ui";
import { Icon } from "./icons";

function hueFor(videoId: number | null | undefined, title: string): number {
  if (videoId) return (videoId * 47) % 360;
  let n = 0;
  for (const ch of title) n = (n * 31 + ch.codePointAt(0)!) % 360;
  return n;
}

function fileExtension(fileName: string): string {
  const ext = fileName.split(".").pop();
  return ext && ext.length <= 5 ? ext.toUpperCase() : "";
}

function technicalLabel(v: VideoWithTags): string {
  const lower = v.file_name.toLowerCase();
  const quality = /2160p|\b4k\b/.test(lower)
    ? "4K"
    : /1080p/.test(lower)
      ? "1080p"
      : /720p/.test(lower)
        ? "720p"
        : "";
  const ext = fileExtension(v.file_name);
  return [quality, ext].filter(Boolean).join(" · ");
}

function cardSubtitle(v: VideoWithTags): string {
  if (v.series_name || v.episode !== null) {
    const series = [
      v.series_name,
      v.season ? `S${v.season}` : "",
      episodeLabel(v),
    ]
      .filter(Boolean)
      .join(" ");
    if (series) return series;
  }
  if (v.kind === "movie") {
    return technicalLabel(v) || "本地文件";
  }
  if (v.kind === "unknown") {
    return ["待定", technicalLabel(v)].filter(Boolean).join(" · ");
  }
  return v.file_name;
}

/**
 * 海报主体：有 ffmpeg 就用真实静帧，没有就画确定性的“片头字幕卡”。
 * 文字卡不是破图占位，而是设计的一部分：深色底、大首字、类型标记、细颗粒。
 */
export function PosterArt({
  videoId,
  title,
  label,
  kind = "unknown",
  children,
  className = "",
}: {
  videoId?: number | null;
  title: string;
  label: string;
  kind?: VideoKind;
  children?: ReactNode;
  className?: string;
}) {
  const { ref, url } = useThumbnail(videoId);
  const hue = hueFor(videoId, title);
  const initial = title.trim().slice(0, 1).toUpperCase() || "C";
  const style = { "--poster-hue": hue } as CSSProperties;

  return (
    <div
      ref={ref}
      className={`poster-art${url ? " has-image" : ""}${className ? ` ${className}` : ""}`}
      style={style}
    >
      {url ? (
        <img
          className="poster-image"
          src={url}
          alt=""
          loading="lazy"
          decoding="async"
          draggable={false}
        />
      ) : (
        <div className="poster-slate" aria-hidden="true">
          <span className="poster-slate-mark">
            <Icon name={kind === "series" ? "tv" : "film"} size={18} />
          </span>
          <span className="poster-slate-letter">{initial}</span>
          <span className="poster-slate-label">{label}</span>
        </div>
      )}
      <div className="poster-shade" aria-hidden="true" />
      {children}
    </div>
  );
}

export function VideoCard({
  video,
  onOpen,
  onPlay,
  onContextMenu,
}: {
  video: VideoWithTags;
  onOpen: () => void;
  onPlay: () => void;
  onContextMenu: (e: MouseEvent) => void;
}) {
  const pct = Math.max(0, Math.min(100, video.watch_progress));
  const watched = pct >= 99.9;
  const sub = cardSubtitle(video);
  const duration = formatDuration(video.duration);
  const posterLabel =
    video.kind === "series" ? "SERIES" : "LOCAL";

  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onOpen();
    }
  };

  return (
    <article
      className="video-card"
      onClick={onOpen}
      onContextMenu={onContextMenu}
      onKeyDown={onKeyDown}
      tabIndex={0}
      role="button"
      aria-label={`${video.title}${watched ? "，已看完" : pct > 0 ? `，已看 ${Math.round(pct)}%` : ""}`}
      title={video.file_path}
    >
      <PosterArt
        videoId={video.id}
        title={video.title}
        label={posterLabel}
        kind={video.kind}
      >
        {video.kind !== "movie" && (
          <span className="poster-kind">
            <KindBadge kind={video.kind} />
          </span>
        )}
        <button
          type="button"
          className="poster-play"
          aria-label={`播放 ${video.title}`}
          onClick={(e) => {
            e.stopPropagation();
            onPlay();
          }}
        >
          <Icon name="play" size={22} />
        </button>
        {pct > 0 && (
          <div className={`poster-progress${watched ? " done" : ""}`}>
            <span style={{ width: `${pct}%` }} />
          </div>
        )}
      </PosterArt>

      <div className="video-card-body">
        <div className="video-card-title">{video.title}</div>
        <div className="video-card-sub">{sub}</div>
        <div className="video-card-meta">
          <Stars rating={video.rating} compact />
          {duration && (
            <>
              <span className="meta-sep">·</span>
              <span>{duration}</span>
            </>
          )}
          <span className="meta-sep">·</span>
          <span className="size">{formatSize(video.file_size)}</span>
          {watched && (
            <span className="watched-tag">
              <Icon name="check" size={11} strokeWidth={2.2} />
              已看
            </span>
          )}
        </div>
      </div>
    </article>
  );
}

const clampProgress = (p: number) =>
  Math.max(0, Math.min(100, Math.round(Number.isFinite(p) ? p : 0)));

/**
 * 右键「刷新封面图」的动作：删旧图 + 随机重抽一帧。
 * 视频卡片与剧集卡片共用（剧集封面借的是 cover_video_id 那一集）。
 * 只动封面缓存，不触发整列表重刷，分页与滚动位置都保持不变。
 */
export async function refreshPoster(
  videoId: number | null | undefined
): Promise<void> {
  if (!videoId) {
    toast.err("这张卡片没有可用的封面源");
    return;
  }
  if (isPosterRefreshing(videoId)) {
    toast.ok("这张封面正在刷新…");
    return;
  }
  try {
    const url = await refreshThumbnail(videoId);
    toast.ok(url ? "封面图已刷新" : "旧封面已清除，但未能生成新图");
  } catch (e) {
    toast.err(`刷新封面失败：${String(e)}`);
  }
}

/**
 * 视频卡片的右键菜单与配套弹窗（评分 / 进度 / 删除）。
 * 主网格与「继续观看」轨道共用这一份逻辑，避免两处菜单项和行为漂移。
 *
 * 用法：
 *   const { openMenu, menuNode } = useVideoCardMenu({ onOpen, onPlay, onChanged });
 *   卡片：onContextMenu={(e) => openMenu(e, v)}；组件末尾渲染 {menuNode}
 */
export function useVideoCardMenu({
  onOpen,
  onPlay,
  onChanged,
}: {
  onOpen: (v: VideoWithTags) => void;
  onPlay: (v: VideoWithTags) => void;
  onChanged: () => void;
}) {
  const [menu, setMenu] = useState<{ x: number; y: number; video: VideoWithTags } | null>(null);
  const [ratingVideo, setRatingVideo] = useState<VideoWithTags | null>(null);
  const [progressVideo, setProgressVideo] = useState<VideoWithTags | null>(null);
  const [confirmDel, setConfirmDel] = useState<{ video: VideoWithTags; withFile: boolean } | null>(null);
  const progressSavedRef = useRef(0);

  const watched = (v: VideoWithTags) => v.watch_progress >= 99.9;

  const openProgress = (v: VideoWithTags) => {
    progressSavedRef.current = clampProgress(v.watch_progress);
    setProgressVideo({ ...v, watch_progress: progressSavedRef.current });
  };

  const previewProgress = (p: number) => {
    const target = clampProgress(p);
    setProgressVideo((prev) => (prev ? { ...prev, watch_progress: target } : prev));
  };

  const commitProgress = async (v: VideoWithTags, p: number) => {
    const target = clampProgress(p);
    setProgressVideo((prev) => (prev ? { ...prev, watch_progress: target } : prev));
    if (target === progressSavedRef.current) return;
    try {
      await api.updateProgress(v.id, target);
      progressSavedRef.current = target;
      toast.ok(`进度已设为 ${target}%`);
      onChanged();
    } catch (e) {
      setProgressVideo((prev) =>
        prev ? { ...prev, watch_progress: progressSavedRef.current } : prev
      );
      toast.err(String(e));
    }
  };

  const toggleWatched = async (v: VideoWithTags) => {
    try {
      await api.updateProgress(v.id, watched(v) ? 0 : 100);
      toast.ok(watched(v) ? "已标记为未看过" : "已标记为已看过");
      onChanged();
    } catch (e) {
      toast.err(String(e));
    }
  };

  const openFolder = async (v: VideoWithTags) => {
    try {
      await api.openFolder(v.file_path);
    } catch (e) {
      toast.err(String(e));
    }
  };

  const doDelete = async (v: VideoWithTags, withFile: boolean) => {
    try {
      if (withFile) {
        toast.ok(await api.deleteVideoWithFile(v.id));
      } else {
        await api.deleteVideo(v.id);
        toast.ok("已删除");
      }
      onChanged();
    } catch (e) {
      toast.err(String(e));
    }
  };

  const openMenu = (e: MouseEvent, v: VideoWithTags) => {
    e.preventDefault();
    setMenu({ x: e.clientX, y: e.clientY, video: v });
  };

  const menuItems = (v: VideoWithTags): MenuItem[] => [
    {
      icon: <Icon name="play" size={14} />,
      label: `播放${watched(v) ? "" : v.watch_progress > 0 ? "（继续）" : ""}`,
      onClick: () => onPlay(v),
    },
    {
      icon: <Icon name="folder" size={14} />,
      label: "打开所在文件夹",
      onClick: () => openFolder(v),
    },
    {
      icon: <Icon name="info" size={14} />,
      label: "详情 / 改名 / 标签",
      onClick: () => onOpen(v),
    },
    {
      icon: <Icon name="refresh" size={14} />,
      label: "刷新封面图",
      onClick: () => void refreshPoster(v.id),
    },
    { sep: true },
    {
      icon: <Icon name="star" size={14} />,
      label: "评分…",
      onClick: () => setRatingVideo(v),
    },
    {
      icon: <Icon name="clock" size={14} />,
      label: "调节进度…",
      onClick: () => openProgress(v),
    },
    {
      icon: <Icon name={watched(v) ? "eye-off" : "eye"} size={14} />,
      label: watched(v) ? "标记为未看过" : "标记为已看过",
      onClick: () => toggleWatched(v),
    },
    { sep: true },
    {
      icon: <Icon name="trash" size={14} />,
      label: "删除记录",
      danger: true,
      onClick: () => setConfirmDel({ video: v, withFile: false }),
    },
    {
      icon: <Icon name="trash" size={14} />,
      label: "删除记录和文件",
      danger: true,
      onClick: () => setConfirmDel({ video: v, withFile: true }),
    },
  ];

  // 菜单/弹窗都是 position:fixed，渲染位置不影响布局；
  // 放在组件末尾可避免被横向滚动轨道（overflow-x:auto）裁切。
  const menuNode = (
    <>
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={menuItems(menu.video)}
        />
      )}

      {ratingVideo && (
        <Modal
          title={`评分 · ${ratingVideo.title}`}
          onClose={() => setRatingVideo(null)}
          footer={
            <button className="btn btn-ghost" onClick={() => setRatingVideo(null)}>
              取消
            </button>
          }
        >
          <p className="muted-note">点击星星评分，再次点击同分取消。</p>
          <RatingPicker
            value={ratingVideo.rating}
            onChange={async (r) => {
              try {
                await api.setRating(ratingVideo.id, r);
                toast.ok(r > 0 ? `已评 ${r} 分` : "已取消评分");
                onChanged();
              } catch (e) {
                toast.err(String(e));
              }
              setRatingVideo(null);
            }}
          />
        </Modal>
      )}

      {progressVideo && (
        <Modal
          title={`观看进度 · ${progressVideo.title}`}
          onClose={() => setProgressVideo(null)}
          footer={
            <button
              className="btn btn-ghost"
              onClick={() => {
                void commitProgress(progressVideo, progressVideo.watch_progress);
                setProgressVideo(null);
              }}
            >
              完成
            </button>
          }
        >
          <div className="progress-editor">
            <input
              className="progress-slider"
              type="range"
              min={0}
              max={100}
              step={1}
              value={Math.round(progressVideo.watch_progress)}
              aria-label="观看进度"
              onChange={(e) => previewProgress(Number(e.target.value))}
              onPointerUp={(e) =>
                void commitProgress(progressVideo, Number(e.currentTarget.value))
              }
              onKeyUp={(e) =>
                void commitProgress(progressVideo, Number(e.currentTarget.value))
              }
            />
            <span className="rating-num">{Math.round(progressVideo.watch_progress)}%</span>
          </div>
          <div className="progress-quick-row">
            {[
              ["未看", 0],
              ["25%", 25],
              ["一半", 50],
              ["75%", 75],
              ["已看完", 100],
            ].map(([label, value]) => (
              <button
                key={label}
                type="button"
                className="progress-quick"
                onClick={() => void commitProgress(progressVideo, Number(value))}
              >
                {label}
              </button>
            ))}
          </div>
          <p className="muted-note">拖动时只预览，松手后保存一次。</p>
        </Modal>
      )}

      {confirmDel && (
        <ConfirmDialog
          title={confirmDel.withFile ? "删除记录和文件" : "删除记录"}
          message={
            confirmDel.withFile
              ? `确定删除「${confirmDel.video.title}」的记录并删除本地文件？\n\n注意：此操作不可恢复。`
              : `确定删除「${confirmDel.video.title}」的记录？\n\n不会删除本地文件。`
          }
          danger
          confirmText="删除"
          onConfirm={() => {
            const { video, withFile } = confirmDel;
            setConfirmDel(null);
            void doDelete(video, withFile);
          }}
          onCancel={() => setConfirmDel(null)}
        />
      )}
    </>
  );

  return { openMenu, menuNode };
}
