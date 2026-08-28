import React, { useCallback, useRef, useState } from "react";
import type { Tag, VideoKind } from "../types";

// ============================================
// 通用 UI 件
// ============================================

// Esc 关闭只允许最上层弹窗响应：PendingModal 里是 Modal 套 Modal，
// 若无条件响应会把内外两层一起关掉。用栈记录层级，栈顶才吃 Esc。
const modalStack: symbol[] = [];

export function Modal({
  title,
  onClose,
  children,
  footer,
  wide,
}: {
  title: string;
  onClose: () => void;
  children: React.ReactNode;
  footer?: React.ReactNode;
  wide?: boolean;
}) {
  const token = React.useRef<symbol>(Symbol("modal"));

  React.useEffect(() => {
    const t = token.current;
    modalStack.push(t);
    const onEsc = (e: KeyboardEvent) => {
      if (e.key === "Escape" && modalStack[modalStack.length - 1] === t) {
        onClose();
      }
    };
    document.addEventListener("keydown", onEsc);
    return () => {
      document.removeEventListener("keydown", onEsc);
      const i = modalStack.lastIndexOf(t);
      if (i >= 0) modalStack.splice(i, 1);
    };
  }, [onClose]);

  return (
    <div
      className="overlay"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className={`modal${wide ? " wide" : ""}`}>
        <div className="modal-header">
          <h2>{title}</h2>
          <button className="icon-btn" onClick={onClose} aria-label="关闭">
            ✕
          </button>
        </div>
        <div className="modal-body">{children}</div>
        {footer && <div className="modal-footer">{footer}</div>}
      </div>
    </div>
  );
}

export function ConfirmDialog({
  title,
  message,
  confirmText = "确定",
  danger,
  onConfirm,
  onCancel,
}: {
  title: string;
  message: string;
  confirmText?: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <Modal
      title={title}
      onClose={onCancel}
      footer={
        <>
          <button className="btn btn-ghost" onClick={onCancel}>
            取消
          </button>
          <button
            className={`btn ${danger ? "btn-danger" : "btn-primary"}`}
            onClick={onConfirm}
          >
            {confirmText}
          </button>
        </>
      }
    >
      <p style={{ color: "var(--text-2)", lineHeight: 1.7, fontSize: 13 }}>
        {message}
      </p>
    </Modal>
  );
}

// ============================================
// Toast
// ============================================
type ToastItem = { id: number; msg: string; kind: "ok" | "err" };
let toastId = 0;

export function ToastHost() {
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const timers = useRef<Record<number, number>>({});
  const push = useCallback((msg: string, kind: "ok" | "err") => {
    const id = ++toastId;
    setToasts((t) => [...t, { id, msg, kind }]);
    timers.current[id] = window.setTimeout(() => {
      setToasts((t) => t.filter((x) => x.id !== id));
      delete timers.current[id];
    }, 2600);
  }, []);

  const api = useRef({ push });
  React.useEffect(() => {
    window.__toast = api.current;
  }, [api]);

  return (
    <div className="toast-wrap">
      {toasts.map((t) => (
        <div key={t.id} className={`toast ${t.kind}`}>
          {t.msg}
        </div>
      ))}
    </div>
  );
}

export const toast = {
  ok: (msg: string) => window.__toast?.push(msg, "ok"),
  err: (msg: string) => window.__toast?.push(msg, "err"),
};

declare global {
  interface Window {
    __toast?: { push: (msg: string, kind: "ok" | "err") => void };
  }
}

// ============================================
// 小件
// ============================================
export function KindBadge({ kind }: { kind: VideoKind }) {
  const map: Record<VideoKind, string> = {
    movie: "电影",
    series: "剧集",
    unknown: "待定",
  };
  return <span className={`badge-kind ${kind}`}>{map[kind] ?? kind}</span>;
}

export function Stars({ rating }: { rating: number }) {
  const r = Math.round(rating);
  if (r <= 0) return <span className="stars" style={{ color: "var(--text-3)" }}>—</span>;
  return (
    <span className="stars">
      {"★".repeat(Math.max(1, Math.min(10, r)))}
      <span className="rating-num" style={{ marginLeft: 3 }}>
        {rating.toFixed(1)}
      </span>
    </span>
  );
}

export function TagChips({ tags }: { tags: Tag[] }) {
  if (!tags?.length) return null;
  return (
    <span style={{ display: "inline-flex", gap: 4, flexWrap: "wrap" }}>
      {tags.slice(0, 3).map((t) => (
        <span key={t.id} className="chip">
          <span className="dot" style={{ background: t.color }} />
          {t.name}
        </span>
      ))}
    </span>
  );
}

export function RatingPicker({
  value,
  onChange,
}: {
  value: number;
  onChange: (v: number) => void;
}) {
  return (
    <div className="rating-stars">
      {Array.from({ length: 10 }, (_, i) => i + 1).map((n) => (
        <span
          key={n}
          className={`rstar${n <= value ? " on" : ""}`}
          onClick={() => onChange(n === value ? 0 : n)}
        >
          ★
        </span>
      ))}
    </div>
  );
}

export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  return (bytes / Math.pow(1024, i)).toFixed(1) + " " + units[i];
}

export function progressPct(p: number): number {
  return Math.max(0, Math.min(100, p));
}

// ============================================
// 右键菜单
// ============================================
export type MenuItem = {
  label?: string;
  danger?: boolean;
  sep?: boolean;
  onClick?: () => void;
};

export function ContextMenu({
  x,
  y,
  items,
  onClose,
}: {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
}) {
  React.useEffect(() => {
    const onDocClick = () => onClose();
    const onEsc = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("click", onDocClick);
    document.addEventListener("keydown", onEsc);
    return () => {
      document.removeEventListener("click", onDocClick);
      document.removeEventListener("keydown", onEsc);
    };
  }, [onClose]);

  const W = 210;
  const H = items.length * 34 + 16;
  const left = Math.min(x, window.innerWidth - W - 8);
  const top = Math.min(y, window.innerHeight - H - 8);

  return (
    <div
      className="ctx-menu"
      style={{ left: Math.max(8, left), top: Math.max(8, top) }}
      onClick={(e) => e.stopPropagation()}
    >
      {items.map((it, i) =>
        it.sep ? (
          <div key={i} className="ctx-sep" />
        ) : (
          <button
            key={i}
            className={`ctx-item${it.danger ? " danger" : ""}`}
            onClick={() => {
              it.onClick?.();
              onClose();
            }}
          >
            {it.label}
          </button>
        )
      )}
    </div>
  );
}
