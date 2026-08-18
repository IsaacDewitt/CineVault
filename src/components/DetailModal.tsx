import React, { useEffect, useState } from "react";
import type { Tag, VideoWithTags } from "../types";
import { api } from "../api";
import { Modal, RatingPicker, toast } from "./ui";

export function DetailModal({
  video,
  tags,
  onClose,
  onChanged,
  onPlay,
}: {
  video: VideoWithTags;
  tags: Tag[];
  onClose: () => void;
  onChanged: () => void;
  onPlay: (v: VideoWithTags) => void;
}) {
  const [title, setTitle] = useState(video.title);
  const [rating, setRating] = useState(video.rating);
  const [progress, setProgress] = useState(video.watch_progress);
  const [videoTags, setVideoTags] = useState(video.tags ?? []);
  const [confirmDelete, setConfirmDelete] = useState(false);

  useEffect(() => {
    setTitle(video.title);
    setRating(video.rating);
    setProgress(video.watch_progress);
    setVideoTags(video.tags ?? []);
  }, [video]);

  const saveProgress = async (p: number) => {
    setProgress(p);
    try {
      await api.updateProgress(video.id, p);
      onChanged();
    } catch (e) {
      toast.err(String(e));
    }
  };

  const attached = new Set(videoTags.map((t) => t.id));

  const toggleTag = async (tagId: number) => {
    try {
      if (attached.has(tagId)) {
        await api.removeVideoTag(video.id, tagId);
      } else {
        await api.addVideoTag(video.id, tagId);
      }
      const d = await api.getVideoDetail(video.id);
      setVideoTags(d.tags);
      onChanged();
    } catch (e) {
      toast.err(String(e));
    }
  };

  const saveRating = async (r: number) => {
    setRating(r);
    try {
      await api.setRating(video.id, r);
      onChanged();
    } catch (e) {
      toast.err(String(e));
    }
  };

  const saveTitle = async () => {
    const t = title.trim();
    if (!t || t === video.title) return;
    try {
      await api.renameVideo(video.id, t);
      toast.ok("已改名");
      onChanged();
    } catch (e) {
      toast.err(String(e));
    }
  };

  return (
    <Modal
      title="视频详情"
      onClose={onClose}
      wide
      footer={
        <>
          <button className="btn btn-ghost" onClick={onClose}>
            关闭
          </button>
          {!confirmDelete ? (
            <button
              className="btn btn-danger"
              onClick={() => setConfirmDelete(true)}
            >
              删除记录
            </button>
          ) : (
            <>
              <button className="btn btn-ghost" onClick={() => setConfirmDelete(false)}>
                取消
              </button>
              <button
                className="btn btn-danger"
                onClick={async () => {
                  try {
                    await api.deleteVideo(video.id);
                    toast.ok("已删除");
                    onChanged();
                    onClose();
                  } catch (e) {
                    toast.err(String(e));
                  }
                }}
              >
                确认删除
              </button>
            </>
          )}
        </>
      }
    >
      <div className="detail-row">
        <label>标题</label>
        <div style={{ flex: 1, display: "flex", gap: 8 }}>
          <input
            className="input"
            style={{ flex: 1 }}
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            onBlur={saveTitle}
            onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
          />
        </div>
      </div>
      <div className="detail-row">
        <label>类型</label>
        <span className="val">
          {video.kind === "movie" ? "电影" : video.kind === "series" ? "剧集" : "待定"}
          {video.series_name ? ` · ${video.series_name}` : ""}
          {video.season ? ` S${video.season}` : ""}
          {video.episode ? `E${video.episode}` : ""}
        </span>
      </div>
      <div className="detail-row">
        <label>路径</label>
        <span className="val" style={{ fontFamily: "var(--mono)", fontSize: 11.5 }}>
          {video.file_path}
        </span>
      </div>
      <div className="detail-row">
        <label>大小</label>
        <span className="val">
          {(video.file_size / 1024 / 1024 / 1024).toFixed(2)} GB
          {video.duration ? ` · ${Math.round(video.duration / 60)} 分钟` : ""}
        </span>
      </div>
      <div className="detail-row">
        <label>评分</label>
        <RatingPicker value={rating} onChange={saveRating} />
      </div>
      <div className="detail-row">
        <label>进度</label>
        <div style={{ flex: 1, display: "flex", alignItems: "center", gap: 10 }}>
          <input
            className="progress-slider"
            type="range"
            min={0}
            max={100}
            step={5}
            value={Math.round(progress)}
            onChange={(e) => saveProgress(Number(e.target.value))}
          />
          <span className="rating-num" style={{ minWidth: 42 }}>
            {Math.round(progress)}%
          </span>
          <button className="progress-quick" onClick={() => saveProgress(0)}>
            未看
          </button>
          <button className="progress-quick" onClick={() => saveProgress(50)}>
            一半
          </button>
          <button className="progress-quick" onClick={() => saveProgress(100)}>
            已看完
          </button>
        </div>
      </div>
      <div className="detail-row">
        <label>标签</label>
        <div className="tag-selector">
          {tags.map((t) => (
            <span
              key={t.id}
              className={`tag-pick${attached.has(t.id) ? " on" : ""}`}
              onClick={() => toggleTag(t.id)}
            >
              <span className="tag-dot" style={{ background: t.color }} />
              {t.name}
            </span>
          ))}
        </div>
      </div>
      <div className="detail-actions">
        <button
          className="btn btn-primary"
          onClick={() => {
            onPlay(video);
          }}
        >
          ▶ 播放
        </button>
        <button
          className="btn btn-ghost"
          onClick={async () => {
            try {
              await api.openFolder(video.file_path);
            } catch (e) {
              toast.err(String(e));
            }
          }}
        >
          📁 所在文件夹
        </button>
        <button
          className="btn btn-ghost"
          onClick={async () => {
            try {
              await api.toggleFavorite(video.id);
              onChanged();
            } catch (e) {
              toast.err(String(e));
            }
          }}
        >
          {video.is_favorite ? "★ 取消收藏" : "☆ 收藏"}
        </button>
      </div>
    </Modal>
  );
}
