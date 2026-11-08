import React, { useCallback, useRef, useState } from "react";
import type { Tag, VideoKind } from "../types";
import { Icon } from "./icons";

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
  const closeRef = React.useRef(onClose);
  const dialogRef = React.useRef<HTMLDivElement>(null);
  const titleId = React.useId();

  // onClose 通常是父组件的内联闭包，每次渲染都变；放进 ref，
  // 避免焦点陷阱 effect 每次重跑、把用户正在点的按钮焦点抢走。
  React.useEffect(() => {
    closeRef.current = onClose;
  }, [onClose]);

  React.useEffect(() => {
    const t = token.current;
    modalStack.push(t);
    const previous = document.activeElement as HTMLElement | null;
    const focusables = () =>
      Array.from(
        dialogRef.current?.querySelectorAll<HTMLElement>(
          'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])'
        ) ?? []
      ).filter((el) => el.offsetParent !== null);

    const timer = window.setTimeout(() => {
      const preferred = dialogRef.current?.querySelector<HTMLElement>(
        ".modal-body input, .modal-body select, .modal-body textarea"
      );
      const first = preferred ?? focusables()[0];
      (first ?? dialogRef.current)?.focus();
    }, 0);

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && modalStack[modalStack.length - 1] === t) {
        closeRef.current();
        return;
      }
      if (e.key !== "Tab" || modalStack[modalStack.length - 1] !== t) return;
      const items = focusables();
      if (items.length === 0) {
        e.preventDefault();
        dialogRef.current?.focus();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement;
      if (e.shiftKey && (active === first || active === dialogRef.current)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", onKey);
    return () => {
      window.clearTimeout(timer);
      document.removeEventListener("keydown", onKey);
      const i = modalStack.lastIndexOf(t);
      if (i >= 0) modalStack.splice(i, 1);
      previous?.focus?.();
    };
  }, []);

  return (
    <div
      className="overlay"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className={`modal${wide ? " wide" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
      >
        <div className="modal-header">
          <h2 id={titleId}>{title}</h2>
          <button className="icon-btn" onClick={onClose} aria-label="关闭">
            <Icon name="close" size={16} />
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
      <p
        style={{
          color: "var(--text-2)",
          lineHeight: 1.7,
          fontSize: 13,
          whiteSpace: "pre-line",
        }}
      >
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
    <div className="toast-wrap" aria-live="polite" aria-atomic="false">
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

export function Stars({ rating, compact = false }: { rating: number; compact?: boolean }) {
  const r = Math.round(rating);
  if (r <= 0) {
    return (
      <span className="stars" style={{ color: "var(--text-3)" }} role="img" aria-label="未评分">
        —
      </span>
    );
  }
  // compact 模式：单星 + 数字，省 ~50px 宽度给标题（卡片正文用）
  if (compact) {
    return (
      <span className="stars" role="img" aria-label={`评分 ${rating.toFixed(1)} 分`}>
        ★<span className="rating-num" style={{ marginLeft: 3 }}>{rating.toFixed(1)}</span>
      </span>
    );
  }
  // 完整模式（详情/卡片用）：保留 10 星直观显示
  return (
    <span className="stars" role="img" aria-label={`评分 ${rating.toFixed(1)} 分`}>
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
    <div className="rating-stars" role="group" aria-label="评分，满分 10 分">
      {Array.from({ length: 10 }, (_, i) => i + 1).map((n) => (
        <button
          key={n}
          type="button"
          className={`rstar${n <= value ? " on" : ""}`}
          onClick={() => onChange(n === value ? 0 : n)}
          aria-label={`${n} 分${n === value ? "，再次点击取消评分" : ""}`}
          aria-pressed={n <= value}
        >
          ★
        </button>
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

/** 季集号文本：合体文件显示区间（E23-24 / E1-3），单集显示 E23，无集号返回空串。
 *  用 null 判断而非真值判断：试播集是 E0，真值判断会把它吞掉。 */
export function episodeLabel(v: {
  episode: number | null;
  episode_end?: number | null;
}): string {
  if (v.episode === null || v.episode === undefined) return "";
  return v.episode_end && v.episode_end > v.episode
    ? `E${v.episode}-${v.episode_end}`
    : `E${v.episode}`;
}

// ============================================
// 右键菜单
// ============================================
export type MenuItem = {
  label?: string;
  icon?: React.ReactNode;
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
  const menuRef = React.useRef<HTMLDivElement>(null);

  React.useEffect(() => {
    const onDocClick = () => onClose();
    // 右键点到别处时先关掉当前菜单（捕获阶段早于卡片冒泡阶段的 onContextMenu）。
    // 新菜单随后由目标卡片打开；否则「继续观看」与主网格各持一份状态的菜单会同时显示。
    const onDocContext = () => onClose();
    const onEsc = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    // 右键菜单可用键盘直接操作：打开后焦点落到第一项。
    menuRef.current?.querySelector<HTMLButtonElement>(".ctx-item")?.focus();
    document.addEventListener("click", onDocClick);
    document.addEventListener("contextmenu", onDocContext, true);
    document.addEventListener("keydown", onEsc);
    return () => {
      document.removeEventListener("click", onDocClick);
      document.removeEventListener("contextmenu", onDocContext, true);
      document.removeEventListener("keydown", onEsc);
    };
  }, [onClose]);

  const W = 210;
  const H = items.length * 34 + 16;
  const left = Math.min(x, window.innerWidth - W - 8);
  const top = Math.min(y, window.innerHeight - H - 8);

  return (
    <div
      ref={menuRef}
      className="ctx-menu"
      style={{ left: Math.max(8, left), top: Math.max(8, top) }}
      onClick={(e) => e.stopPropagation()}
      role="menu"
    >
      {items.map((it, i) =>
        it.sep ? (
          <div key={i} className="ctx-sep" role="separator" />
        ) : (
          <button
            key={i}
            className={`ctx-item${it.danger ? " danger" : ""}`}
            role="menuitem"
            onClick={() => {
              it.onClick?.();
              onClose();
            }}
          >
            {it.icon && <span className="ctx-ico">{it.icon}</span>}
            <span>{it.label}</span>
          </button>
        )
      )}
    </div>
  );
}
