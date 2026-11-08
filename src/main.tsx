import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import "./styles/theme.css";
import "./styles/cards.css";
import "./styles/settings.css";

// 这是桌面应用，不是浏览器：WebView2 自带的右键菜单（返回 / 刷新 / 另存为 / 打印…）
// 在这里全是噪音。在文档层用捕获阶段统一掐掉，任何角落都不会再漏出来；
// 需要菜单的地方（视频卡片 / 剧集卡片 / 侧栏）各自 preventDefault 后弹应用内菜单。
// 捕获阶段注册是为了兜住组件里可能出现的 stopPropagation。
document.addEventListener("contextmenu", (e) => e.preventDefault(), true);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>
);
