import { useEffect, useRef, useState } from "react";
import type { Tag, VideoWithTags } from "../types";
import { api } from "../api";
import { Modal, RatingPicker, episodeLabel, formatSize, toast } from "./ui";
import { formatDuration } from "../lib/library";
import { Icon } from "./icons";

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
  const progressSavedRef = useRef(video.watch_progress);
  const ratingSavedRef = useRef(video.rating);

  useEffect(() => {
    setTitle(video.title);
    setRating(video.rating);
    setProgress(video.watch_progress);
    setVideoTags(video.tags ?? []);
    progressSavedRef.current = video.watch_progress;
    ratingSavedRef.current = video.rating;
  }, [video]);

  const clampProgress = (p: number) =>
    Math.max(0, Math.min(100, Math.round(Number.isFinite(p) ? p : 0)));

  const commitProgress = async (p: number) => {
    const target = clampProgress(p);
    setProgress(target);
    if (target === progressSavedRef.current) return;
    try {
      await api.updateProgress(video.id, target);
      progressSavedRef.current = target;
      onChanged();
    } catch (e) {
      setProgress(progressSavedRef.current);
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
    const previous = ratingSavedRef.current;
    setRating(r);
    try {
      await api.setRating(video.id, r);
      ratingSavedRef.current = r;
      onChanged();
    } catch (e) {
      setRating(previous);
      toast.err(String(e));
    }
  };

  const saveTitle = async () => {
    const t = title.trim();
    if (!t) {
      setTitle(video.title);
      return;
    }
    if (t === video.title) return;
    try {
      await api.renameVideo(video.id, t);
      toast.ok("已改名");
      onChanged();
    } catch (e) {
      setTitle(video.title);
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
          {episodeLabel(video)}
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
          {formatSize(video.file_size)}
          {video.duration ? ` · ${formatDuration(video.duration)}` : ""}
        </span>
      </div>
      <div className="detail-row">
        <label>评分</label>
        <RatingPicker value={rating} onChange={saveRating} />
      </div>
      <div className="detail-row">
        <label>进度</label>
        <div className="progress-editor">
          <input
            className="progress-slider"
            type="range"
            min={0}
            max={100}
            step={1}
            value={Math.round(progress)}
            aria-label="观看进度"
            onChange={(e) => setProgress(clampProgress(Number(e.target.value)))}
            onPointerUp={(e) => void commitProgress(Number(e.currentTarget.value))}
            onKeyUp={(e) => void commitProgress(Number(e.currentTarget.value))}
          />
          <span className="rating-num">{Math.round(progress)}%</span>
          <div className="progress-quick-row">
            <button type="button" className="progress-quick" onClick={() => void commitProgress(0)}>
              未看
            </button>
            <button type="button" className="progress-quick" onClick={() => void commitProgress(50)}>
              一半
            </button>
            <button type="button" className="progress-quick" onClick={() => void commitProgress(100)}>
              已看完
            </button>
          </div>
        </div>
      </div>
      <div className="detail-row">
        <label>标签</label>
        <div className="tag-selector">
          {tags.map((t) => (
            <button
              type="button"
              key={t.id}
              className={`tag-pick${attached.has(t.id) ? " on" : ""}`}
              onClick={() => toggleTag(t.id)}
            >
              <span className="tag-dot" style={{ background: t.color }} />
              {t.name}
            </button>
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
          <Icon name="play" size={14} />
          播放
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
          <Icon name="folder" size={14} />
          所在文件夹
        </button>
      </div>
    </Modal>
  );
}
