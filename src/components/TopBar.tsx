import { useEffect, useRef, useState } from "react";
import type { SortDir, SortSpec, WatchState } from "../types";
import type { RatingRange } from "../lib/library";
import { Icon } from "./icons";

const SORT_OPTIONS: { key: SortSpec["key"]; label: string }[] = [
  { key: "last_watched", label: "最近播放" },
  { key: "updated", label: "最近更新" },
  { key: "name", label: "按名称" },
  { key: "size", label: "按大小" },
  { key: "rating", label: "按评分" },
  { key: "duration", label: "按时长" },
];

// 切换维度时的默认方向（延续旧版固定方向的用户直觉）；
// "最近"类是"最近 N"语义，后端恒降序，不显示方向按钮
const DEFAULT_DIR: Partial<Record<SortSpec["key"], SortDir>> = {
  name: "asc",
  size: "desc",
  rating: "desc",
  duration: "desc",
};
const FIXED_DESC: Partial<Record<SortSpec["key"], true>> = {
  last_watched: true,
  updated: true,
};

const STATE_LABELS: Record<WatchState, string> = {
  unwatched: "未看",
  partial: "在看",
  watched: "已看",
};
const ALL_STATES: WatchState[] = ["unwatched", "partial", "watched"];

export function TopBar({
  query,
  onQuery,
  sort,
  onSort,
  watchStates,
  onWatchStates,
  ratingRange,
  onRatingRange,
  hideFilters = false,
}: {
  query: string;
  onQuery: (q: string) => void;
  sort: SortSpec;
  onSort: (s: SortSpec) => void;
  watchStates: Set<WatchState>;
  onWatchStates: (s: Set<WatchState>) => void;
  ratingRange: RatingRange;
  onRatingRange: (r: RatingRange) => void;
  /** 排序/筛选对结果无意义的视图（如随机推荐）隐藏这两个控件——摆着按不动比不摆更误导 */
  hideFilters?: boolean;
}) {
  const [popOpen, setPopOpen] = useState(false);
  const popRef = useRef<HTMLDivElement>(null);

  // 切到隐藏筛选的视图时收起弹层，避免切回时弹层莫名其妙自己开着
  useEffect(() => {
    if (hideFilters) setPopOpen(false);
  }, [hideFilters]);

  // 点击弹层外部关闭（mousedown 捕获，从按下到松开都在弹层外才判为"外部"）
  useEffect(() => {
    if (!popOpen) return;
    const onDown = (e: MouseEvent) => {
      if (popRef.current && !popRef.current.contains(e.target as Node)) {
        setPopOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [popOpen]);

  const toggleState = (st: WatchState) => {
    const next = new Set(watchStates);
    if (next.has(st)) next.delete(st);
    else next.add(st);
    onWatchStates(next);
  };

  const pickSortKey = (key: SortSpec["key"]) =>
    onSort({ key, dir: DEFAULT_DIR[key] ?? "desc" });

  // 激活筛选数：被排除的状态数 + 启用的评分区间（漏斗徽章）
  const activeCount =
    (ALL_STATES.length - watchStates.size) + (ratingRange.enabled ? 1 : 0);

  // 双滑块防交叉：拖动的把手夹在对侧之内
  const slideLo = (v: number) =>
    onRatingRange({ ...ratingRange, min: Math.min(v, ratingRange.max) });
  const slideHi = (v: number) =>
    onRatingRange({ ...ratingRange, max: Math.max(v, ratingRange.min) });

  const resetFilters = () => {
    onWatchStates(new Set(ALL_STATES));
    onRatingRange({ enabled: false, min: 0, max: 10 });
  };

  const fullRange = ratingRange.min === 0 && ratingRange.max === 10;
  const pct = (n: number) => `${n * 10}%`;

  return (
    <header className="topbar">
      <div className="search-box">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
          <circle cx="11" cy="11" r="7" />
          <path d="M21 21l-4.35-4.35" />
        </svg>
        <input
          placeholder="搜索标题 / 文件名…"
          value={query}
          onChange={(e) => onQuery(e.target.value)}
        />
      </div>
      <div className={`topbar-right${hideFilters ? " no-filters" : ""}`}>
        {/* 排序 = 维度下拉 + ▲▼ 方向切换；"最近"类恒降序，方向按钮隐藏 */}
        <div className="fb-combo">
          <div className="fb-selwrap">
            <select
              className="fb-sel"
              aria-label="排序方式"
              value={sort.key}
              onChange={(e) => pickSortKey(e.target.value as SortSpec["key"])}
            >
              {SORT_OPTIONS.map((o) => (
                <option key={o.key} value={o.key}>
                  {o.label}
                </option>
              ))}
            </select>
          </div>
          {!FIXED_DESC[sort.key] && (
            <div className="fb-dir">
              <button
                className={sort.dir === "asc" ? "on" : ""}
                onClick={() => onSort({ ...sort, dir: "asc" })}
                title="升序"
                aria-label="升序"
                aria-pressed={sort.dir === "asc"}
              >
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M6 15l6-6 6 6" />
                </svg>
              </button>
              <button
                className={sort.dir === "desc" ? "on" : ""}
                onClick={() => onSort({ ...sort, dir: "desc" })}
                title="降序"
                aria-label="降序"
                aria-pressed={sort.dir === "desc"}
              >
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M6 9l6 6 6-6" />
                </svg>
              </button>
            </div>
          )}
        </div>

        {/* 筛选漏斗 + 弹层（观看状态 / 评分区间） */}
        <div className="fb-anchor" ref={popRef}>
          <button
            className={`fb-funnel${activeCount > 0 ? " active" : ""}`}
            onClick={() => setPopOpen((v) => !v)}
            aria-expanded={popOpen}
            aria-haspopup="dialog"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
              <path d="M3 5h18l-7 8v5.5l-4 2V13z" />
            </svg>
            筛选
            {activeCount > 0 && <span className="fb-badge">{activeCount}</span>}
          </button>
          {popOpen && (
            <div className="fb-pop" role="dialog" aria-label="筛选">
              <div className="fb-pop-title">观看状态</div>
              <div className="fb-chips">
                {ALL_STATES.map((st) => (
                  <button
                    key={st}
                    className={`fb-chip${watchStates.has(st) ? " on" : ""}`}
                    onClick={() => toggleState(st)}
                    aria-pressed={watchStates.has(st)}
                  >
                    <span className="fb-ck">
                      <Icon name="check" size={9} strokeWidth={2.4} />
                    </span>
                    {STATE_LABELS[st]}
                  </button>
                ))}
              </div>
              <div className="fb-pop-title" style={{ marginTop: 14 }}>
                评分范围
              </div>
              <div className={`fb-rating${ratingRange.enabled ? " on" : " off"}`}>
                <button
                  type="button"
                  className={`fb-cbx${ratingRange.enabled ? " on" : ""}`}
                  onClick={() =>
                    onRatingRange({ ...ratingRange, enabled: !ratingRange.enabled })
                  }
                  title={ratingRange.enabled ? "停用评分筛选" : "启用评分筛选"}
                  role="checkbox"
                  aria-checked={ratingRange.enabled}
                  aria-label="启用评分筛选"
                >
                  <svg viewBox="0 0 24 24" fill="none" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M4 12l6 6L20 6" />
                  </svg>
                </button>
                <Icon name="star" size={13} className="fb-star" />
                <div className="fb-range">
                  <div className="fb-range-track">
                    <div
                      className="fb-range-fill"
                      style={{
                        left: pct(ratingRange.min),
                        width: pct(ratingRange.max - ratingRange.min),
                      }}
                    />
                  </div>
                  <input
                    type="range"
                    min={0}
                    max={10}
                    step={1}
                    value={ratingRange.min}
                    disabled={!ratingRange.enabled}
                    aria-label="最低评分"
                    onChange={(e) => slideLo(Number(e.target.value))}
                  />
                  <input
                    type="range"
                    min={0}
                    max={10}
                    step={1}
                    value={ratingRange.max}
                    disabled={!ratingRange.enabled}
                    aria-label="最高评分"
                    onChange={(e) => slideHi(Number(e.target.value))}
                  />
                </div>
                <span className="fb-rlabel">
                  {ratingRange.enabled
                    ? fullRange
                      ? "0 – 10 全部"
                      : `${ratingRange.min} – ${ratingRange.max}`
                    : "未启用"}
                </span>
              </div>
              <div className="fb-pop-actions">
                <button onClick={resetFilters}>重置筛选</button>
              </div>
            </div>
          )}
        </div>
      </div>
    </header>
  );
}
