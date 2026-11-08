import { useCallback, useEffect, useRef, useState } from "react";
import type { SeriesOverview, Tag, VideoWithTags } from "../types";
import { api } from "../api";
import { Modal, RatingPicker, Stars, TagChips, toast } from "./ui";
import { VideoList } from "./VideoList";
import { Icon } from "./icons";

export function SeriesDetailView({
  series,
  reloadKey,
  onBack,
  tags,
  onChanged,
  onOpenVideo,
  onPlayVideo,
}: {
  series: SeriesOverview;
  /** PotPlayer 进度同步成功后由 App 递增，触发集数列表重新拉取 */
  reloadKey: number;
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
  const ratingSavedRef = useRef(series.rating);

  useEffect(() => {
    ratingSavedRef.current = series.rating;
    if (!editingName) setName(series.name);
    setRating(series.rating);
  }, [series.name, series.rating, editingName]);

  const load = useCallback(async () => {
    try {
      setEpisodes(await api.getSeriesEpisodes(series.id));
    } catch (e) {
      toast.err(String(e));
    }
  }, [series.id]);

  useEffect(() => {
    load();
  }, [load, reloadKey]);

  const attached = new Set((series.tags ?? []).map((t) => t.id));

  return (
    <div>
      <div className="breadcrumb">
        <button onClick={onBack}>
          <Icon name="arrow-left" size={14} />
          返回剧集
        </button>
        <span className="breadcrumb-sep">/</span>
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
              if (!n) {
                setName(series.name);
                return;
              }
              if (n !== series.name) {
                try {
                  await api.renameSeries(series.id, n);
                  toast.ok("已改名");
                  onChanged();
                } catch (e) {
                  setName(series.name);
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
            {series.name}
            <Icon name="edit" size={13} className="inline-edit-icon" />
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
          const previous = ratingSavedRef.current;
          setRating(r);
          try {
            await api.setSeriesRating(series.id, r);
            ratingSavedRef.current = r;
            onChanged();
          } catch (e) {
            setRating(previous);
            toast.err(String(e));
          }
        }} />
        <Stars rating={series.rating} />
        <span style={{ color: "var(--text-3)", fontSize: 12 }}>
          已看 {series.watched_episodes}/{series.total_episodes} 集
        </span>
        <TagChips tags={series.tags ?? []} />
        <span style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
          <button className="btn btn-ghost" onClick={() => setManageTags(true)}>
            <Icon name="tag" size={14} />
            标签
          </button>
          {!confirmDelete ? (
            <button className="btn btn-danger" onClick={() => setConfirmDelete(true)}>
              <Icon name="trash" size={14} />
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

      <VideoList
        videos={episodes}
        onOpen={onOpenVideo}
        onPlay={onPlayVideo}
        onChanged={() => {
          // 集列表自己重拉；同时通知 App 刷新 series 聚合（已看集数/进度条/最近更新）。
          void load();
          onChanged();
        }}
      />

      {manageTags && (
        <Modal title={`为「${series.name}」选择标签`} onClose={() => setManageTags(false)}>
          <div className="tag-selector">
            {tags.map((t) => (
              <button
                type="button"
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
              </button>
            ))}
          </div>
        </Modal>
      )}
    </div>
  );
}
