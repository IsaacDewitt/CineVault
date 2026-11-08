import type { ReactNode } from "react";

export type IconName =
  | "list"
  | "grid"
  | "play"
  | "folder"
  | "info"
  | "star"
  | "clock"
  | "eye"
  | "eye-off"
  | "trash"
  | "settings"
  | "smartphone"
  | "download"
  | "upload"
  | "refresh"
  | "dice"
  | "film"
  | "tv"
  | "check"
  | "close"
  | "plus"
  | "tag"
  | "database"
  | "arrow-left"
  | "edit"
  | "warning"
  | "scan";

const PATHS: Record<IconName, ReactNode> = {
  list: <path d="M4 6h16M4 12h16M4 18h10" />,
  grid: (
    <>
      <rect x="4" y="4" width="6" height="6" rx="1.4" />
      <rect x="14" y="4" width="6" height="6" rx="1.4" />
      <rect x="4" y="14" width="6" height="6" rx="1.4" />
      <rect x="14" y="14" width="6" height="6" rx="1.4" />
    </>
  ),
  play: <path d="M8.5 6.8v10.4L18 12z" fill="currentColor" stroke="none" />,
  folder: <path d="M3.5 7.5h6l1.7 2h9.3v8.2a1.8 1.8 0 0 1-1.8 1.8H5.3a1.8 1.8 0 0 1-1.8-1.8z" />,
  info: (
    <>
      <circle cx="12" cy="12" r="8.8" />
      <path d="M12 10.8v5.4M12 7.8h.01" />
    </>
  ),
  star: <path d="M12 3.7l2.55 5.16 5.7.83-4.12 4.02.97 5.67L12 16.7l-5.1 2.68.97-5.67-4.12-4.02 5.7-.83z" />,
  clock: (
    <>
      <circle cx="12" cy="12" r="8.8" />
      <path d="M12 7v5.3l3.6 2.1" />
    </>
  ),
  eye: (
    <>
      <path d="M2.8 12s3.3-5.2 9.2-5.2 9.2 5.2 9.2 5.2-3.3 5.2-9.2 5.2S2.8 12 2.8 12z" />
      <circle cx="12" cy="12" r="2.5" />
    </>
  ),
  "eye-off": (
    <>
      <path d="M4.2 4.2l15.6 15.6" />
      <path d="M9.6 6.9A10.7 10.7 0 0 1 12 6.8c5.9 0 9.2 5.2 9.2 5.2a15.4 15.4 0 0 1-2.2 2.7M6.5 8.3A16.2 16.2 0 0 0 2.8 12s3.3 5.2 9.2 5.2c1.1 0 2.1-.2 3-.5" />
    </>
  ),
  trash: (
    <>
      <path d="M5 7h14M9 7V4.8h6V7M7.2 7l.8 12h8l.8-12M10.2 10.5v5.2M13.8 10.5v5.2" />
    </>
  ),
  settings: (
    <>
      <circle cx="12" cy="12" r="3.1" />
      <path d="M12 3.6v2.1M12 18.3v2.1M20.4 12h-2.1M5.7 12H3.6M17.9 6.1l-1.5 1.5M7.6 16.4l-1.5 1.5M17.9 17.9l-1.5-1.5M7.6 7.6L6.1 6.1" />
    </>
  ),
  smartphone: (
    <>
      <rect x="6.8" y="3.2" width="10.4" height="17.6" rx="2.4" />
      <path d="M10.5 17.8h3" />
    </>
  ),
  download: <path d="M12 3.8v10.4M8 10.6l4 4 4-4M4.5 18.8h15" />,
  upload: <path d="M12 20.2V9.8M8 13.4l4-4 4 4M4.5 5.2h15" />,
  refresh: (
    <>
      <path d="M20 11.2A8.2 8.2 0 0 0 5.6 6.9L3.8 9" />
      <path d="M4 11.2a8.2 8.2 0 0 0 14.4 4.3l1.8-2.1" />
      <path d="M3.8 4.8V9h4.2M20.2 19.2V15H16" />
    </>
  ),
  dice: (
    <>
      <rect x="4" y="4" width="16" height="16" rx="3.2" />
      <circle cx="8.5" cy="8.5" r="1.15" fill="currentColor" stroke="none" />
      <circle cx="15.5" cy="8.5" r="1.15" fill="currentColor" stroke="none" />
      <circle cx="12" cy="12" r="1.15" fill="currentColor" stroke="none" />
      <circle cx="8.5" cy="15.5" r="1.15" fill="currentColor" stroke="none" />
      <circle cx="15.5" cy="15.5" r="1.15" fill="currentColor" stroke="none" />
    </>
  ),
  film: (
    <>
      <rect x="3.2" y="4.8" width="17.6" height="14.4" rx="2.2" />
      <path d="M7.5 4.8v14.4M16.5 4.8v14.4M3.2 9.2h4.3M3.2 14.8h4.3M16.5 9.2h4.3M16.5 14.8h4.3" />
    </>
  ),
  tv: (
    <>
      <rect x="3" y="4.8" width="18" height="12.4" rx="2.2" />
      <path d="M8.4 20.4h7.2M12 17.2v3.2" />
    </>
  ),
  check: <path d="M4.5 12.5l5 5 10-11" />,
  close: <path d="M6 6l12 12M18 6L6 18" />,
  plus: <path d="M12 5v14M5 12h14" />,
  tag: (
    <>
      <path d="M4 5.2h7.2l8.6 8.6-6.8 6.8L4 11.4z" />
      <circle cx="8.1" cy="8.3" r="1.2" />
    </>
  ),
  database: (
    <>
      <ellipse cx="12" cy="5.7" rx="7.2" ry="2.8" />
      <path d="M4.8 5.7v6.2c0 1.5 3.2 2.8 7.2 2.8s7.2-1.3 7.2-2.8V5.7" />
      <path d="M4.8 11.9v6.2c0 1.5 3.2 2.8 7.2 2.8s7.2-1.3 7.2-2.8v-6.2" />
    </>
  ),
  "arrow-left": <path d="M19 12H5M10.5 6.2L4.7 12l5.8 5.8" />,
  edit: (
    <>
      <path d="M4 20l4.2-1 10.6-10.6a2.1 2.1 0 0 0-3-3L5.2 16z" />
      <path d="M13.4 6.8l3.8 3.8" />
    </>
  ),
  warning: (
    <>
      <path d="M12 3.7l9 16.1H3z" />
      <path d="M12 9.2v4.5M12 16.8h.01" />
    </>
  ),
  scan: (
    <>
      <path d="M4 8V5.2A1.2 1.2 0 0 1 5.2 4H8M16 4h2.8A1.2 1.2 0 0 1 20 5.2V8M20 16v2.8a1.2 1.2 0 0 1-1.2 1.2H16M8 20H5.2A1.2 1.2 0 0 1 4 18.8V16" />
      <circle cx="12" cy="11.6" r="3.2" />
      <path d="M14.4 14l2.1 2.1" />
    </>
  ),
};

export function Icon({
  name,
  size = 16,
  className,
  strokeWidth = 1.7,
}: {
  name: IconName;
  size?: number;
  className?: string;
  strokeWidth?: number;
}) {
  return (
    <svg
      className={className}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {PATHS[name]}
    </svg>
  );
}
