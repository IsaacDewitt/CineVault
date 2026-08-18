import React, { useCallback, useEffect, useState } from "react";
import type { PendingItem } from "../types";
import { api } from "../api";
import { Modal, toast } from "./ui";

export function PendingModal({
  onClose,
  onChanged,
}: {
  onClose: () => void;
  onChanged: () => void;
}) {
  const [items, setItems] = useState<PendingItem[]>([]);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setItems(await api.getPending());
    } catch (e) {
      toast.err(String(e));
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const decide = async (item: PendingItem, kind: "movie" | "series") => {
    setBusy(true);
    try {
      if (kind === "series") {
        const name = prompt("剧集名（留空则用所在文件夹名）:", item.title) ?? "";
        await api.resolvePending(item.video_id, "series", name.trim() || null);
      } else {
        await api.resolvePending(item.video_id, "movie", null);
      }
      toast.ok(kind === "series" ? "已归入剧集" : "已归为电影");
      await load();
      onChanged();
    } catch (e) {
      toast.err(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title={`待确认分类（${items.length}）`}
      onClose={onClose}
      footer={
        <button className="btn btn-ghost" onClick={onClose}>
          关闭
        </button>
      }
      wide
    >
      <p style={{ fontSize: 12.5, color: "var(--text-3)", marginBottom: 14, lineHeight: 1.7 }}>
        这些视频的分类置信度不高（可能是剧集也可能是电影）。请确认归属；确认过的不会再询问。
      </p>
      {items.length === 0 ? (
        <div className="empty" style={{ padding: 40 }}>
          <div className="emoji">✅</div>
          <p>没有待确认项</p>
        </div>
      ) : (
        items.map((it) => (
          <div key={it.video_id} className="pending-item">
            <div className="p-info">
              <div className="p-title">{it.title}</div>
              <div className="p-path">{it.file_path}</div>
            </div>
            <span className="conf-badge">置信 {it.confidence}</span>
            <button
              className="btn btn-ghost"
              style={{ fontSize: 11.5, padding: "4px 10px" }}
              disabled={busy}
              onClick={() => decide(it, "movie")}
            >
              是电影
            </button>
            <button
              className="btn btn-primary"
              style={{ fontSize: 11.5, padding: "4px 10px" }}
              disabled={busy}
              onClick={() => decide(it, "series")}
            >
              是剧集
            </button>
          </div>
        ))
      )}
    </Modal>
  );
}
