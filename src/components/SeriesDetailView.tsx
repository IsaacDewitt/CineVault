import React, { useCallback, useEffect, useState } from "react";
import type { SeriesOverview, Tag, VideoWithTags } from "../types";
import { api } from "../api";
import { Modal, RatingPicker, Stars, TagChips, toast } from "./ui";
import { VideoList } from "./VideoList";

export function SeriesDetailView({
  series,
  onBack,
  tags,
  onChanged,
  onOpenVideo,
  onPlayVideo,
}: {
  series: SeriesOverview;
  onBack: () => void;
  tags: Tag[];
  onChanged: () => void;
  onOpenVideo: (v: VideoWithTags) => void;
  onPlayVideo: (v: VideoWithTags) => void;
}) {
  const [episodes, setEpisodes] = useState<VideoWithTags[]>([]);
  const [editingName, setEditingName] = useState(false);
  const [name, setName] = useState(series.name);
  const [rating, setRating] = useState(series.rating);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [manageTags, setManageTags] = useState(false);

  const load = useCallback(async () => {
    try {
      setEpisodes(await api.getSeriesEpisodes(series.id));
    } catch (e) {
      toast.err(String(e));
    }
  }, [series.id]);

  useEffect(() => {
    load();
  }, [load]);

  const attached = new Set((series.tags ?? []).map((t) => t.id));

  return (
    <div>
      <div className="breadcrumb">
        <button onClick={onBack}>← 返回剧集</button>
        <span style={{ color: "var(--text-3)" }}>/</span>
        {editingName ? (
          <input
            className="input"
            style={{ width: 260 }}
            value={name}
            autoFocus
            onChange={(e) => setName(e.target.value)}
            onBlur={async () => {
              setEditingName(false);
              const n = name.trim();
              if (n && n !== series.name) {
                try {
                  await api.renameSeries(series.id, n);
                  toast.ok("已改名");
                  onChanged();
                } catch (e) {
                  toast.err(String(e));
                }
              }
            }}
            onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
          />
        ) : (
          <span
            className="current"
            style={{ cursor: "pointer" }}
            onClick={() => setEditingName(true)}
            title="点击改名"
          >
            {series.name} ✏
          </span>
        )}
      </div>

      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 14,
          marginBottom: 16,
          flexWrap: "wrap",
        }}
      >
        <RatingPicker value={rating} onChange={async (r) => {
          setRating(r);
          try {
            await api.setSeriesRating(series.id, r);
            onChanged();
          } catch (e) {
            toast.err(String(e));
          }
        }} />
        <Stars rating={series.rating} />
        <span style={{ color: "var(--text-3)", fontSize: 12 }}>
          已看 {series.watched_episodes}/{series.total_episodes} 集
        </span>
        <TagChips tags={series.tags ?? []} />
        <span style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
          <button
            className="btn btn-ghost"
            onClick={async () => {
              try {
                await api.toggleSeriesFavorite(series.id);
                onChanged();
              } catch (e) {
                toast.err(String(e));
              }
            }}
          >
            {series.is_favorite ? "★ 取消收藏" : "☆ 收藏"}
          </button>
          <button className="btn btn-ghost" onClick={() => setManageTags(true)}>
            🏷 标签
          </button>
          {!confirmDelete ? (
            <button className="btn btn-danger" onClick={() => setConfirmDelete(true)}>
              删除剧集
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
                    await api.deleteSeries(series.id);
                    toast.ok("已删除");
                    onChanged();
                    onBack();
                  } catch (e) {
                    toast.err(String(e));
                  }
                }}
              >
                确认删除
              </button>
            </>
          )}
        </span>
      </div>

      <VideoList videos={episodes} onOpen={onOpenVideo} onPlay={onPlayVideo} onChanged={load} />

      {manageTags && (
        <Modal title={`为「${series.name}」选择标签`} onClose={() => setManageTags(false)}>
          <div className="tag-selector">
            {tags.map((t) => (
              <span
                key={t.id}
                className={`tag-pick${attached.has(t.id) ? " on" : ""}`}
                onClick={async () => {
                  try {
                    if (attached.has(t.id)) {
                      await api.removeSeriesTag(series.id, t.id);
                    } else {
                      await api.addSeriesTag(series.id, t.id);
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
            ))}
          </div>
        </Modal>
      )}
    </div>
  );
}
