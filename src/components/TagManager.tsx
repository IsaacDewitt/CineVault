import React, { useCallback, useEffect, useState } from "react";
import type { Tag } from "../types";
import { api } from "../api";
import { Modal, toast } from "./ui";

export function TagManager({ onClose, onChanged }: { onClose: () => void; onChanged: () => void }) {
  const [tags, setTags] = useState<Tag[]>([]);
  const [name, setName] = useState("");
  const [color, setColor] = useState("#e8b34b");

  const load = useCallback(async () => {
    try {
      setTags(await api.getTags());
    } catch (e) {
      toast.err(String(e));
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const create = async () => {
    const n = name.trim();
    if (!n) return;
    try {
      await api.createTag(n, color);
      setName("");
      await load();
      onChanged();
    } catch (e) {
      toast.err(String(e));
    }
  };

  return (
    <Modal
      title="管理标签"
      onClose={onClose}
      footer={
        <button className="btn btn-ghost" onClick={onClose}>
          关闭
        </button>
      }
    >
      <div style={{ display: "flex", gap: 8, marginBottom: 14 }}>
        <input
          className="input"
          style={{ flex: 1 }}
          placeholder="新标签名"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && create()}
        />
        <input
          type="color"
          value={color}
          onChange={(e) => setColor(e.target.value)}
          style={{
            width: 42,
            height: 34,
            background: "var(--bg-2)",
            border: "1px solid var(--border)",
            borderRadius: 8,
            padding: 2,
            cursor: "pointer",
          }}
        />
        <button className="btn btn-primary" onClick={create}>
          添加
        </button>
      </div>

      <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
        {tags.map((t) => (
          <div
            key={t.id}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 10,
              padding: "7px 12px",
              background: "var(--bg-2)",
              borderRadius: 8,
              fontSize: 13,
            }}
          >
            <span className="tag-dot" style={{ background: t.color }} />
            <span style={{ flex: 1 }}>{t.name}</span>
            <span style={{ color: "var(--text-3)", fontSize: 11.5 }}>{t.video_count ?? 0} 个</span>
            <input
              type="color"
              value={t.color}
              onChange={async (e) => {
                try {
                  await api.updateTag(t.id, null, e.target.value);
                  await load();
                  onChanged();
                } catch (err) {
                  toast.err(String(err));
                }
              }}
              style={{ width: 26, height: 24, border: "none", background: "transparent", cursor: "pointer", padding: 0 }}
            />
            <button
              className="icon-btn"
              style={{ width: 24, height: 24, fontSize: 13 }}
              onClick={async () => {
                try {
                  await api.deleteTag(t.id);
                  await load();
                  onChanged();
                } catch (err) {
                  toast.err(String(err));
                }
              }}
            >
              🗑
            </button>
          </div>
        ))}
        {tags.length === 0 && (
          <div style={{ color: "var(--text-3)", fontSize: 12, textAlign: "center", padding: 20 }}>
            还没有标签
          </div>
        )}
      </div>
    </Modal>
  );
}
