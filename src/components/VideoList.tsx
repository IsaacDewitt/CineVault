import React, { useState } from "react";
import type { VideoWithTags } from "../types";
import { api } from "../api";
import {
  ConfirmDialog,
  ContextMenu,
  KindBadge,
  Modal,
  RatingPicker,
  Stars,
  TagChips,
  formatSize,
  progressPct,
  toast,
} from "./ui";

export function VideoList({
  videos,
  onOpen,
  onPlay,
  onChanged,
}: {
  videos: VideoWithTags[];
  onOpen: (v: VideoWithTags) => void;
  onPlay: (v: VideoWithTags) => void;
  onChanged: () => void;
}) {
  const [menu, setMenu] = useState<{ x: number; y: number; video: VideoWithTags } | null>(null);
  const [ratingVideo, setRatingVideo] = useState<VideoWithTags | null>(null);
  const [progressVideo, setProgressVideo] = useState<VideoWithTags | null>(null);
  const [confirmDel, setConfirmDel] = useState<{ video: VideoWithTags; withFile: boolean } | null>(null);

  const watched = (v: VideoWithTags) => v.watch_progress >= 99.9;

  const setProgress = async (v: VideoWithTags, p: number) => {
    // 先本地预览，再持久化（拖动滑条即保存，与详情弹窗行为一致）
    setProgressVideo((prev) => (prev ? { ...prev, watch_progress: p } : prev));
    try {
      await api.updateProgress(v.id, p);
      toast.ok(`进度已设为 ${Math.round(p)}%`);
      onChanged();
    } catch (e) {
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

  const toggleFav = async (v: VideoWithTags) => {
    try {
      await api.toggleFavorite(v.id);
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

  if (videos.length === 0) {
    return (
      <div className="empty">
        <div className="emoji">🎬</div>
        <p>没有符合条件的视频</p>
      </div>
    );
  }

  return (
    <div className="video-list">
      {videos.map((v) => {
        const pct = progressPct(v.watch_progress);
        return (
          <div
            key={v.id}
            className="video-row"
            onClick={() => onOpen(v)}
            onContextMenu={(e) => {
              e.preventDefault();
              setMenu({ x: e.clientX, y: e.clientY, video: v });
            }}
            title={v.file_path}
          >
            {/* 观看进度：整行背景淡色填充 */}
            {pct > 0 && (
              <div
                className={`v-progress-bg${pct >= 100 ? " done" : " active"}`}
                style={{ width: `${pct}%` }}
              />
            )}
            <KindBadge kind={v.kind} />
            <div style={{ flex: 1, minWidth: 0 }}>
              <div className="v-title">{v.title}</div>
              <div className="v-sub">
                {v.series_name ? `${v.series_name}` : ""}
                {v.season ? ` S${v.season}` : ""}
                {v.episode ? `E${v.episode}` : ""}
                {v.series_name ? " · " : ""}
                {v.file_name}
              </div>
            </div>
            <div className="v-meta">
              <TagChips tags={v.tags} />
              <Stars rating={v.rating} />
              {v.is_favorite && <span style={{ color: "var(--accent)", fontSize: 13 }}>★</span>}
              {watched(v) && (
                <span style={{ color: "var(--green)", fontSize: 11 }}>✓ 已看</span>
              )}
              <span className="size">{formatSize(v.file_size)}</span>
            </div>
          </div>
        );
      })}

      {/* 右键菜单 */}
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={[
            { label: `▶ 播放${watched(menu.video) ? "（续播）" : ""}`, onClick: () => onPlay(menu.video) },
            { label: "📁 打开所在文件夹", onClick: () => openFolder(menu.video) },
            { label: "📋 详情 / 改名 / 标签", onClick: () => onOpen(menu.video) },
            { sep: true },
            { label: menu.video.is_favorite ? "★ 取消收藏" : "☆ 收藏", onClick: () => toggleFav(menu.video) },
            { label: "⭐ 评分…", onClick: () => setRatingVideo(menu.video) },
            { label: "⏱ 调节进度…", onClick: () => setProgressVideo(menu.video) },
            {
              label: watched(menu.video) ? "👁 标记为未看过" : "👁 标记为已看过",
              onClick: () => toggleWatched(menu.video),
            },
            { sep: true },
            { label: "🗑 删除记录", danger: true, onClick: () => setConfirmDel({ video: menu.video, withFile: false }) },
            { label: "🗑 删除记录和文件", danger: true, onClick: () => setConfirmDel({ video: menu.video, withFile: true }) },
          ]}
        />
      )}

      {/* 评分弹窗 */}
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
          <p style={{ color: "var(--text-3)", fontSize: 12, marginBottom: 10 }}>
            点击星星评分（再次点击同分取消）
          </p>
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

      {/* 进度弹窗 */}
      {progressVideo && (
        <Modal
          title={`观看进度 · ${progressVideo.title}`}
          onClose={() => setProgressVideo(null)}
          footer={
            <button className="btn btn-ghost" onClick={() => setProgressVideo(null)}>
              关闭
            </button>
          }
        >
          <div style={{ display: "flex", alignItems: "center", gap: 12, marginBottom: 14 }}>
            <input
              className="progress-slider"
              type="range"
              min={0}
              max={100}
              step={5}
              value={Math.round(progressVideo.watch_progress)}
              onChange={(e) => setProgress(progressVideo, Number(e.target.value))}
            />
            <span className="rating-num" style={{ minWidth: 42 }}>
              {Math.round(progressVideo.watch_progress)}%
            </span>
          </div>
          <div style={{ display: "flex", gap: 8 }}>
            <button className="progress-quick" onClick={() => setProgress(progressVideo, 0)}>
              未看
            </button>
            <button className="progress-quick" onClick={() => setProgress(progressVideo, 25)}>
              25%
            </button>
            <button className="progress-quick" onClick={() => setProgress(progressVideo, 50)}>
              一半
            </button>
            <button className="progress-quick" onClick={() => setProgress(progressVideo, 75)}>
              75%
            </button>
            <button className="progress-quick" onClick={() => setProgress(progressVideo, 100)}>
              已看完
            </button>
          </div>
        </Modal>
      )}

      {/* 删除确认 */}
      {confirmDel && (
        <ConfirmDialog
          title={confirmDel.withFile ? "删除记录和文件" : "删除记录"}
          message={
            confirmDel.withFile
              ? `确定删除「${confirmDel.video.title}」的记录并删除本地文件？\n\n⚠️ 此操作不可恢复！`
              : `确定删除「${confirmDel.video.title}」的记录？\n\n不会删除本地文件。`
          }
          danger
          confirmText="删除"
          onConfirm={() => {
            const { video, withFile } = confirmDel;
            setConfirmDel(null);
            doDelete(video, withFile);
          }}
          onCancel={() => setConfirmDel(null)}
        />
      )}
    </div>
  );
}
