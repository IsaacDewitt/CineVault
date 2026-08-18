import React, { useState } from "react";
import { api } from "../api";
import { toast } from "./ui";

export function MigrateBanner({
  oldPath,
  onDone,
}: {
  oldPath: string;
  onDone: () => void;
}) {
  const [busy, setBusy] = useState(false);

  const migrate = async () => {
    setBusy(true);
    try {
      const r = await api.migrateFromOld(oldPath);
      toast.ok(
        `迁移完成：视频 ${r.videos} · 剧集 ${r.series} · 标签 ${r.tags} · 历史 ${r.history}`
      );
      onDone();
    } catch (e) {
      toast.err(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="migrate-banner">
      <span style={{ fontSize: 20 }}>📦</span>
      <div style={{ flex: 1 }}>
        <div style={{ fontWeight: 600, fontSize: 13.5 }}>检测到 MovieUI 旧数据</div>
        <div style={{ fontSize: 12, color: "var(--text-3)", marginTop: 2, wordBreak: "break-all" }}>
          {oldPath}（可重复执行：同步观看状态，不会产生重复数据）
        </div>
      </div>
      <button className="btn btn-primary" onClick={migrate} disabled={busy}>
        {busy ? "同步中…" : "迁移 / 同步"}
      </button>
    </div>
  );
}
