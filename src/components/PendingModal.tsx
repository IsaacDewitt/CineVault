import { useCallback, useEffect, useState } from "react";
import type { PendingItem } from "../types";
import { api } from "../api";
import { Modal, toast } from "./ui";
import { Icon } from "./icons";

export function PendingModal({
  onClose,
  onChanged,
}: {
  onClose: () => void;
  onChanged: () => void;
}) {
  const [items, setItems] = useState<PendingItem[]>([]);
  const [busy, setBusy] = useState(false);
  // 剧集命名弹窗（WebView2 不支持 window.prompt，必须用自定义输入框）
  const [naming, setNaming] = useState<PendingItem | null>(null);
  const [seriesName, setSeriesName] = useState("");

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

  const submit = async (item: PendingItem, kind: "movie" | "series", name: string | null) => {
    setBusy(true);
    try {
      await api.resolvePending(item.video_id, kind, name);
      toast.ok(kind === "series" ? "已归入剧集" : "已归为电影");
      await load();
      onChanged();
    } catch (e) {
      toast.err(String(e));
    } finally {
      setBusy(false);
    }
  };

  const confirmSeriesName = async () => {
    if (!naming) return;
    const item = naming;
    setNaming(null);
    await submit(item, "series", seriesName.trim() || null);
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
          <div className="empty-icon">
            <Icon name="check" size={28} />
          </div>
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
              onClick={() => submit(it, "movie", null)}
            >
              是电影
            </button>
            <button
              className="btn btn-primary"
              style={{ fontSize: 11.5, padding: "4px 10px" }}
              disabled={busy}
              onClick={() => {
                setNaming(it);
                setSeriesName(it.title);
              }}
            >
              是剧集
            </button>
          </div>
        ))
      )}

      {naming && (
        <Modal
          title={`归入剧集 · ${naming.title}`}
          onClose={() => setNaming(null)}
          footer={
            <>
              <button className="btn btn-ghost" onClick={() => setNaming(null)}>
                取消
              </button>
              <button className="btn btn-primary" onClick={confirmSeriesName} disabled={busy}>
                确认
              </button>
            </>
          }
        >
          <p style={{ fontSize: 12.5, color: "var(--text-3)", marginBottom: 10, lineHeight: 1.7 }}>
            输入剧集名；留空则使用所在文件夹名。
          </p>
          <input
            className="input"
            autoFocus
            placeholder="剧集名"
            value={seriesName}
            onChange={(e) => setSeriesName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && confirmSeriesName()}
          />
        </Modal>
      )}
    </Modal>
  );
}
