import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import type { ScanResult } from "../types";
import { api } from "../api";
import { Modal, toast } from "./ui";

export function AddSeriesModal({
  onClose,
  onAdded,
}: {
  onClose: () => void;
  onAdded: (r: ScanResult) => void;
}) {
  const [path, setPath] = useState("");
  const [defaultWatched, setDefaultWatched] = useState(false);
  const [adding, setAdding] = useState(false);
  const [result, setResult] = useState<ScanResult | null>(null);

  const browse = async () => {
    const dir = await open({ directory: true, multiple: false });
    if (dir) setPath(dir);
  };

  const start = async () => {
    if (!path.trim()) {
      toast.err("请选择剧集文件夹");
      return;
    }
    setAdding(true);
    setResult(null);
    try {
      const r = await api.addSeriesFromFolder(path.trim(), defaultWatched);
      setResult(r);
      onAdded(r);
      toast.ok(`已添加剧集：新增 ${r.new_added} · 归集 ${r.updated}`);
    } catch (e) {
      toast.err(String(e));
    } finally {
      setAdding(false);
    }
  };

  return (
    <Modal
      title="添加剧集"
      onClose={onClose}
      footer={
        <>
          <button className="btn btn-ghost" onClick={onClose}>
            关闭
          </button>
          <button className="btn btn-primary" onClick={start} disabled={adding}>
            {adding ? "添加中…" : "添加剧集"}
          </button>
        </>
      }
    >
      <div style={{ fontSize: 12.5, color: "var(--text-2)", marginBottom: 10, lineHeight: 1.7 }}>
        选择剧集文件夹：文件夹名即剧集名，其中<strong>所有视频</strong>（含子文件夹）都将归入该剧集。
        已入库的同名视频会一并改挂，不产生待确认项。
      </div>

      <div style={{ display: "flex", gap: 8, marginBottom: 12 }}>
        <input
          className="input"
          style={{ flex: 1 }}
          placeholder="选择剧集文件夹"
          value={path}
          readOnly
          onClick={browse}
        />
        <button className="btn btn-ghost" onClick={browse}>
          浏览…
        </button>
      </div>

      <div style={{ marginBottom: 12 }}>
        <label className="checkbox">
          <input type="checkbox" checked={defaultWatched} onChange={(e) => setDefaultWatched(e.target.checked)} />
          新视频默认标记为已看过
        </label>
      </div>

      {result && (
        <div
          style={{
            background: "var(--bg-2)",
            borderRadius: 10,
            padding: 12,
            fontSize: 12.5,
            lineHeight: 1.8,
            marginBottom: 12,
            color: "var(--text-2)",
          }}
        >
          发现 <b style={{ color: "var(--text-1)" }}>{result.total_found}</b> 个视频 · 新增{" "}
          <b style={{ color: "var(--green)" }}>{result.new_added}</b> · 归集{" "}
          <b style={{ color: "var(--accent)" }}>{result.updated}</b>
          {result.errors.length > 0 && (
            <div style={{ color: "var(--red)", marginTop: 6, maxHeight: 100, overflowY: "auto" }}>
              {result.errors.slice(0, 8).map((e, i) => (
                <div key={i}>{e}</div>
              ))}
            </div>
          )}
        </div>
      )}
    </Modal>
  );
}
