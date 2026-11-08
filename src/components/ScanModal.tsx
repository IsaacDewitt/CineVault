import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import type { ScanFolder, ScanResult } from "../types";
import { api } from "../api";
import { Modal, toast } from "./ui";
import { Icon } from "./icons";

export function ScanModal({
  onClose,
  onScanned,
}: {
  onClose: () => void;
  onScanned: (r: ScanResult) => void;
}) {
  const [path, setPath] = useState("");
  const [incremental, setIncremental] = useState(true);
  const [defaultWatched, setDefaultWatched] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [folders, setFolders] = useState<ScanFolder[]>([]);
  const [result, setResult] = useState<ScanResult | null>(null);

  const loadFolders = async () => {
    try {
      setFolders(await api.getScanFolders());
    } catch (e) {
      toast.err(String(e));
    }
  };

  useEffect(() => {
    loadFolders();
  }, []);

  const browse = async () => {
    const dir = await open({ directory: true, multiple: false });
    if (dir) setPath(dir);
  };

  const start = async () => {
    if (!path.trim()) {
      toast.err("请选择要扫描的文件夹");
      return;
    }
    setScanning(true);
    setResult(null);
    try {
      const r = await api.scanFolder(path.trim(), incremental, defaultWatched);
      setResult(r);
      loadFolders();
      onScanned(r);
      if (r.pending.length > 0) {
        toast.ok(`发现 ${r.pending.length} 个待确认分类项`);
      }
    } catch (e) {
      toast.err(String(e));
    } finally {
      setScanning(false);
    }
  };

  return (
    <Modal
      title="扫描视频"
      onClose={onClose}
      footer={
        <>
          <button className="btn btn-ghost" onClick={onClose}>
            关闭
          </button>
          <button className="btn btn-primary" onClick={start} disabled={scanning}>
            {scanning ? "扫描中…" : "开始扫描"}
          </button>
        </>
      }
    >
      <div style={{ display: "flex", gap: 8, marginBottom: 12 }}>
        <input
          className="input"
          style={{ flex: 1 }}
          placeholder="选择要扫描的文件夹"
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
          <input type="checkbox" checked={incremental} onChange={(e) => setIncremental(e.target.checked)} />
          增量扫描（跳过已存在的文件）
        </label>
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
          新增 <b style={{ color: "var(--green)" }}>{result.new_added}</b> · 更新{" "}
          <b style={{ color: "var(--text-1)" }}>{result.updated}</b> · 待确认{" "}
          <b style={{ color: "var(--accent)" }}>{result.pending.length}</b>
          {result.errors.length > 0 && (
            <div style={{ color: "var(--red)", marginTop: 6, maxHeight: 100, overflowY: "auto" }}>
              {result.errors.slice(0, 8).map((e, i) => (
                <div key={i}>{e}</div>
              ))}
            </div>
          )}
        </div>
      )}

      <div style={{ fontSize: 12, color: "var(--text-3)", marginBottom: 6 }}>
        已扫描文件夹（{folders.length}）
      </div>
      <div style={{ maxHeight: 180, overflowY: "auto" }}>
        {folders.length === 0 && (
          <div style={{ color: "var(--text-3)", fontSize: 12, padding: 8 }}>还没有扫描过的文件夹</div>
        )}
        {folders.map((f) => (
          <div
            key={f.id}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              padding: "6px 10px",
              background: "var(--bg-2)",
              borderRadius: 8,
              marginBottom: 5,
              fontSize: 12,
            }}
          >
            <span style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {f.folder_path}
            </span>
            <button
              className="icon-btn"
              style={{ width: 22, height: 22, fontSize: 13 }}
              onClick={async () => {
                try {
                  await api.deleteScanFolder(f.id);
                  loadFolders();
                } catch (e) {
                  toast.err(String(e));
                }
              }}
            >
              <Icon name="close" size={13} />
            </button>
          </div>
        ))}
      </div>
    </Modal>
  );
}
