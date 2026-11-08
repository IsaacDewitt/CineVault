import { useEffect, useRef, useState } from "react";
import type { BackfillProgress } from "../types";
import { api } from "../api";
import { toast } from "./ui";
import { Icon } from "./icons";
import type { IconName } from "./icons";

function ActionRow({
  icon,
  title,
  desc,
  onClick,
  disabled,
  busy,
}: {
  icon: IconName;
  title: string;
  desc: string;
  onClick: () => void;
  disabled?: boolean;
  busy?: boolean;
}) {
  return (
    <button
      type="button"
      className="settings-row"
      onClick={onClick}
      disabled={disabled}
    >
      <span className="settings-row-icon">
        <Icon name={icon} size={18} />
      </span>
      <span className="settings-row-text">
        <span className="settings-row-title">{title}</span>
        <span className="settings-row-desc">{busy ? "处理中…" : desc}</span>
      </span>
      <span className="settings-row-chevron">›</span>
    </button>
  );
}

export function SettingsDrawer({
  onClose,
  pendingCount,
  onOpenScan,
  onOpenAddSeries,
  onOpenPending,
  onRefreshAll,
  refreshing,
  onExport,
  onImport,
  backfill,
  onBackfill,
  onCancelBackfill,
  onOpenLanAccess,
}: {
  onClose: () => void;
  pendingCount: number;
  onOpenScan: () => void;
  onOpenAddSeries: () => void;
  onOpenPending: () => void;
  onRefreshAll: () => void;
  refreshing: boolean;
  onExport: () => void;
  onImport: () => void;
  backfill: BackfillProgress | null;
  onBackfill: () => void;
  onCancelBackfill: () => void;
  onOpenLanAccess: () => void;
}) {
  const [ffprobe, setFfprobe] = useState<boolean | null>(null);
  const [minSizeMb, setMinSizeMb] = useState("50");
  const [exemptPrefixes, setExemptPrefixes] = useState("");
  const [savingScan, setSavingScan] = useState(false);
  const closeRef = useRef<HTMLButtonElement>(null);
  const drawerRef = useRef<HTMLElement>(null);
  const closeCb = useRef(onClose);

  useEffect(() => {
    closeCb.current = onClose;
  }, [onClose]);

  useEffect(() => {
    api
      .getFfprobeStatus()
      .then((s) => setFfprobe(s.available))
      .catch(() => setFfprobe(false));
    api
      .getScanConfig()
      .then((c) => {
        setMinSizeMb(String(c.min_size_mb));
        setExemptPrefixes(c.exempt_prefixes.join("，"));
      })
      .catch(() => {
        // 老版本后端没有该命令时，保持默认值，不阻塞设置面板。
      });
  }, []);

  const saveScanConfig = async () => {
    const min = Number(minSizeMb);
    if (!Number.isFinite(min) || min < 0 || min > 10240) {
      toast.err("最小入库体积需在 0–10240 MB 之间");
      return;
    }
    const prefixes = exemptPrefixes
      .split(/[,，\n\r]/)
      .map((p) => p.trim())
      .filter(Boolean);
    setSavingScan(true);
    try {
      const saved = await api.setScanConfig(Math.round(min), prefixes);
      setMinSizeMb(String(saved.min_size_mb));
      setExemptPrefixes(saved.exempt_prefixes.join("，"));
      toast.ok("扫描规则已保存，下次扫描生效");
    } catch (e) {
      toast.err(String(e));
    } finally {
      setSavingScan(false);
    }
  };

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    closeRef.current?.focus();
    const focusables = () =>
      Array.from(
        drawerRef.current?.querySelectorAll<HTMLElement>(
          'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])'
        ) ?? []
      ).filter((el) => el.offsetParent !== null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        closeCb.current();
        return;
      }
      if (e.key !== "Tab") return;
      const items = focusables();
      if (items.length === 0) {
        e.preventDefault();
        drawerRef.current?.focus();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      prev?.focus?.();
    };
  }, []);

  const runAndClose = (action: () => void) => {
    onClose();
    action();
  };

  const backfillPct =
    backfill && backfill.total > 0
      ? Math.round((backfill.done / backfill.total) * 100)
      : 0;

  return (
    <div
      className="drawer-overlay"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <aside
        ref={drawerRef}
        className="settings-drawer"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
        tabIndex={-1}
      >
        <header className="settings-header">
          <div>
            <span className="settings-kicker">CINEVAULT</span>
            <h2 id="settings-title">库管理</h2>
          </div>
          <button
            ref={closeRef}
            type="button"
            className="icon-btn"
            onClick={onClose}
            aria-label="关闭设置"
          >
            <Icon name="close" size={17} />
          </button>
        </header>

        <div className="settings-scroll">
          <section className="settings-section">
            <h3>媒体库</h3>
            <ActionRow
              icon="scan"
              title="扫描视频"
              desc="选择文件夹，增量识别电影与剧集"
              onClick={() => runAndClose(onOpenScan)}
            />
            <ActionRow
              icon="tv"
              title="添加剧集"
              desc="把整个文件夹锁定为同一个剧集"
              onClick={() => runAndClose(onOpenAddSeries)}
            />
            <ActionRow
              icon="refresh"
              title="刷新全部扫描"
              desc="同步新增、移动与失效记录"
              onClick={onRefreshAll}
              disabled={refreshing}
              busy={refreshing}
            />
            {pendingCount > 0 && (
              <ActionRow
                icon="info"
                title={`待确认分类 · ${pendingCount}`}
                desc="处理分类器拿不准的视频"
                onClick={() => runAndClose(onOpenPending)}
              />
            )}
          </section>

          <section className="settings-section">
            <h3>扫描规则</h3>
            <div className="settings-field">
              <label htmlFor="scan-min-size">最小入库体积（MB）</label>
              <input
                id="scan-min-size"
                className="input"
                type="number"
                min={0}
                max={10240}
                step={1}
                value={minSizeMb}
                onChange={(e) => setMinSizeMb(e.target.value)}
              />
              <span>小于该体积的文件不入库；填 0 表示不按体积过滤。</span>
            </div>
            <div className="settings-field">
              <label htmlFor="scan-exempt">体积豁免前缀</label>
              <input
                id="scan-exempt"
                className="input"
                value={exemptPrefixes}
                placeholder="欢乐正前方，Pingu"
                onChange={(e) => setExemptPrefixes(e.target.value)}
              />
              <span>用逗号分隔；这些文件名前缀不受最小体积限制。</span>
            </div>
            <button
              type="button"
              className="btn btn-ghost settings-save-btn"
              onClick={() => void saveScanConfig()}
              disabled={savingScan}
            >
              {savingScan ? "保存中…" : "保存扫描规则"}
            </button>
          </section>

          <section className="settings-section">
            <h3>数据与备份</h3>
            <ActionRow
              icon="download"
              title="导出备份"
              desc="完整 JSON，含评分、标签、进度与历史"
              onClick={() => runAndClose(onExport)}
            />
            <ActionRow
              icon="upload"
              title="导入备份"
              desc="导入前自动生成整库快照，可回滚"
              onClick={() => runAndClose(onImport)}
            />
          </section>

          <section className="settings-section">
            <h3>播放与时长</h3>
            {backfill ? (
              <div className="settings-progress">
                <div className="settings-progress-head">
                  <span>正在补全片长</span>
                  <span>{backfillPct}%</span>
                </div>
                <div className="settings-progress-track">
                  <span style={{ width: `${backfillPct}%` }} />
                </div>
                <div className="settings-progress-meta" title={backfill.current}>
                  {backfill.total > 0
                    ? `${backfill.done}/${backfill.total} · 成功 ${backfill.filled}`
                    : "正在统计…"}
                </div>
                <button
                  type="button"
                  className="btn btn-ghost settings-inline-btn"
                  onClick={onCancelBackfill}
                >
                  中断补全
                </button>
              </div>
            ) : (
              <ActionRow
                icon="clock"
                title="补全片长"
                desc={
                  ffprobe === false
                    ? "未检测到 ffprobe：mp4/mkv 可直接补，其余格式会跳过"
                    : "为缺时长的视频写入片长，让进度百分比可用"
                }
                onClick={onBackfill}
              />
            )}
          </section>

          <section className="settings-section">
            <h3>访问</h3>
            <ActionRow
              icon="smartphone"
              title="手机观看"
              desc="同一局域网内只读浏览，VLC 直接播放原片"
              onClick={() => runAndClose(onOpenLanAccess)}
            />
          </section>

          <div className="settings-footnote">
            CineVault 的所有元数据与评分只保存在本机，不上传云端。
            缩略图由本机 ffmpeg 按需生成，缺失时自动使用文字片头卡。
          </div>
        </div>
      </aside>
    </div>
  );
}
