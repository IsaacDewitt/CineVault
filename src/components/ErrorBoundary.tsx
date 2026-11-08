import React from "react";
import { Icon } from "./icons";

/**
 * 兜底错误边界：任一组件渲染抛异常时不至于整片白屏，至少给出提示与重载入口。
 * （Tauri 里没有 devtools 常驻，白屏很难排查。）
 */
export class ErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { message: string | null }
> {
  state = { message: null as string | null };

  static getDerivedStateFromError(error: unknown) {
    return { message: error instanceof Error ? error.message : String(error) };
  }

  componentDidCatch(error: unknown, info: React.ErrorInfo) {
    console.error("[CineVault] 渲染异常：", error, info.componentStack);
  }

  render() {
    if (this.state.message === null) return this.props.children;
    return (
      <div className="empty" style={{ padding: 60 }}>
        <div className="empty-icon">
          <Icon name="warning" size={28} />
        </div>
        <p>界面出错了</p>
        <p
          style={{
            color: "var(--text-3)",
            fontSize: 12,
            fontFamily: "var(--mono)",
            maxWidth: 520,
            wordBreak: "break-all",
          }}
        >
          {this.state.message}
        </p>
        <button className="btn btn-ghost" onClick={() => location.reload()}>
          重新加载
        </button>
      </div>
    );
  }
}
