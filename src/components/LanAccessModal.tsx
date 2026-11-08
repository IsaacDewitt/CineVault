import { useCallback, useEffect, useRef, useState } from "react";
import type { LanStatus } from "../types";
import { api } from "../api";
import { Modal, toast } from "./ui";
import { Icon } from "./icons";

async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    // 某些 WebView 环境不给 Clipboard API，退化到隐藏 textarea。
    const el = document.createElement("textarea");
    el.value = text;
    el.style.position = "fixed";
    el.style.opacity = "0";
    document.body.appendChild(el);
    el.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(el);
    return ok;
  }
}

function CopyButton({ text, label = "复制" }: { text: string; label?: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      className="btn btn-ghost lan-copy"
      onClick={async () => {
        if (await copyText(text)) {
          setDone(true);
          window.setTimeout(() => setDone(false), 1400);
        } else {
          toast.err("复制失败，请手动选择文本");
        }
      }}
    >
      {done ? "已复制" : label}
    </button>
  );
}

export function LanAccessModal({ onClose }: { onClose: () => void }) {
  const [status, setStatus] = useState<LanStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [showToken, setShowToken] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  const requestSeq = useRef(0);
  const busyRef = useRef(false);

  const load = useCallback(async () => {
    if (busyRef.current) return;
    const seq = ++requestSeq.current;
    try {
      const s = await api.getLanStatus();
      if (seq === requestSeq.current && !busyRef.current) {
        setStatus(s);
        setError(null);
      }
    } catch (e) {
      if (seq === requestSeq.current) setError(String(e));
    }
  }, []);

  useEffect(() => {
    void load();
    const id = window.setInterval(() => {
      setNow(Math.floor(Date.now() / 1000));
      void load();
    }, 3000);
    return () => window.clearInterval(id);
  }, [load]);

  const mutate = async (action: () => Promise<LanStatus>) => {
    busyRef.current = true;
    setBusy(true);
    const seq = ++requestSeq.current;
    try {
      const s = await action();
      if (seq === requestSeq.current) setStatus(s);
      return true;
    } catch (e) {
      toast.err(String(e));
      return false;
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  const toggle = async () => {
    if (!status) return;
    await mutate(() => api.setLanEnabled(!status.enabled));
  };

  const resetToken = async () => {
    if (await mutate(api.regenerateLanToken)) {
      toast.ok("访问令牌已重置，手机端需要重新配对");
    }
  };

  const rotateCode = async () => {
    if (await mutate(api.rotateLanPairCode)) toast.ok("配对码已更换");
  };

  const urls = status?.urls ?? [];
  const pairExpired = !!status && status.pair_expires_at <= now;
  const pairLeftSec = status ? Math.max(0, status.pair_expires_at - now) : 0;

  return (
    <Modal
      title="手机观看"
      onClose={onClose}
      wide
      footer={
        <>
          <button className="btn btn-ghost" onClick={() => void load()}>
            刷新状态
          </button>
          <button className="btn btn-primary" onClick={onClose}>
            完成
          </button>
        </>
      }
    >
      {error && (
        <div className="lan-error">
          无法读取局域网服务状态：{error}
        </div>
      )}

      {!status ? (
        <div className="lan-loading">正在读取服务状态…</div>
      ) : (
        <>
          <div className={`lan-hero${status.running ? " on" : ""}`}>
            <div className="lan-hero-icon">
              <Icon name="smartphone" size={20} />
            </div>
            <div style={{ flex: 1, minWidth: 0 }}>
              <div className="lan-hero-title">
                {status.running ? "手机访问已开启" : status.enabled ? "服务未能启动" : "手机访问已关闭"}
              </div>
              <div className="lan-hero-sub">
                {status.running
                  ? "手机与电脑需在同一个 Wi-Fi / 局域网。播放时由 VLC 直接从电脑读取原片，不转码、不同步记录。"
                  : status.enabled
                    ? status.error ?? "端口可能被其它程序或安全软件占用，请检查后重试。"
                    : "开启后仅在本机运行时提供只读访问；关闭后手机立即无法连接。"}
              </div>
            </div>
            <button
              className={`lan-switch${status.enabled ? " on" : ""}`}
              onClick={() => void toggle()}
              disabled={busy}
              aria-label={status.enabled ? "关闭手机访问" : "开启手机访问"}
            >
              <span />
            </button>
          </div>

          {status.error && status.running && (
            <div className="lan-warning">
              自动发现暂不可用：{status.error}。仍可通过下方地址手动连接。
            </div>
          )}

          <div className="lan-section-title">连接方式</div>
          <div className="lan-card">
            {urls.length === 0 ? (
              <div className="lan-row">
                <span className="lan-label">电脑地址</span>
                <span className="lan-value">未检测到局域网 IPv4 地址</span>
              </div>
            ) : (
              urls.map((u, i) => (
                <div className="lan-row" key={u}>
                  <span className="lan-label">{i === 0 ? "电脑地址" : "备用地址"}</span>
                  <span className="lan-value">{u}</span>
                  <CopyButton text={u} />
                </div>
              ))
            )}
            <div className="lan-row">
              <span className="lan-label">配对码</span>
              <span className="lan-pair" style={{ opacity: pairExpired ? 0.45 : 1 }}>
                {status.pair_code}
              </span>
              <span className="lan-hint">
                {pairExpired
                  ? "已过期"
                  : `剩余 ${Math.floor(pairLeftSec / 60)}:${String(pairLeftSec % 60).padStart(2, "0")}`}
              </span>
              <button className="btn btn-ghost lan-copy" onClick={() => void rotateCode()} disabled={busy}>
                {pairExpired ? "生成新码" : "换一个"}
              </button>
            </div>
            <div className="lan-row">
              <span className="lan-label">访问令牌</span>
              <span className="lan-token">
                {showToken ? status.token : "••••••••••••••••••••••••••••••••"}
              </span>
              <button className="btn btn-ghost lan-copy" onClick={() => setShowToken((v) => !v)}>
                {showToken ? "隐藏" : "显示"}
              </button>
              {showToken && <CopyButton text={status.token} />}
            </div>
          </div>

          <ol className="lan-steps">
            <li>安装 CineVault 手机端 APK，确保手机连接同一个 Wi-Fi。</li>
            <li>打开 App，先输入上方配对码再点「自动发现电脑」；若网络禁止广播，就手动填写电脑地址并配对。</li>
            <li>配对成功后即可浏览全部记录。点任意一集会直接打开 VLC 播放。</li>
          </ol>

          <div className="lan-note">
            <b>只读边界：</b>手机端不能改评分、标签、进度，也不能删除或扫描。
            手机播放不会回写 CineVault 的观看记录；这些记录仍以电脑端为准。
            播放期间电脑不能休眠，外接硬盘必须保持连接。
          </div>

          <div className="lan-warning">
            <b>网络安全：</b>本功能面向可信的家庭/专用局域网，当前链路未启用 TLS。
            不要在公司、酒店或公共 Wi-Fi 开启；不确定网络是否可信时请关闭「手机观看」。
          </div>

          <div className="lan-actions">
            <button className="btn btn-ghost" onClick={() => void resetToken()} disabled={busy}>
              重置访问令牌
            </button>
            <span className="lan-hint">重置后所有已配对手机都会失去访问权限，需要重新配对。</span>
          </div>
        </>
      )}
    </Modal>
  );
}
