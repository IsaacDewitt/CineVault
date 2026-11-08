# CineVault

本地影视库管理器。Rust + Tauri 2 桌面端，配一个零依赖的 Android 局域网观看端。

不做网络刮削：卡片封面由本机 ffmpeg 抽取视频静帧生成并缓存；没有 ffmpeg 或解码失败时自动降级为文字片头卡，不出现灰框与破图。

## 特性

- **智能分类**：三层判定引擎（文件名规则 → 目录上下文 → 概率融合），输出类型 + 置信度；拿不准的进「待确认队列」由用户裁决，裁决结果永久记忆
- **增量扫描 + 移动匹配**：文件或剧集文件夹搬家不丢评分、标签、观看进度
- **剧集实体**：评分与标签挂在剧集上，不依赖单集；支持合体文件集号区间（`S09E23-24` / `EP01-03` / `第31-32集`）
- **旧库一键迁移**：直接读 MovieUI 的 `movieui.db` 全量导入（视频 / 剧集 / 标签 / 历史 / 扫描目录）
- **播放历史与续播**：打开即记录，进度随行；5 分钟会话合并，同秒去重
- **本地静帧卡片**：统一 16:9 网格 + 继续观看轨道；缩略图只在本机生成，绝不上传；右键「刷新封面图」删掉旧静帧并在随机位置重抽一张
- **局域网手机观看**：桌面端起服务，手机 App 配对后直接播放，零第三方依赖的只读 HTTP + Range 分块
- **可配置扫描规则**：最小入库体积、体积豁免前缀可在「库管理」中调整
- **数据安全**：刷新扫描只清理本次可访问扫描根内的记录；导入或清理前自动生成整库快照

## 平台与依赖

| 项 | 要求 |
| --- | --- |
| 系统 | Windows 10 / 11（主要目标平台，`lan.rs` 的 socket 继承与 `winreg` 均为 Windows 专用路径） |
| WebView2 | Win11 自带；Win10 需装 Edge WebView2 Runtime |
| Node.js | 20 以上（开发与打包前端，实测 v22） |
| Rust | stable 工具链（edition 2021，实测 1.96） |
| ffmpeg / ffprobe | 可选但强烈建议。放 PATH，或与 ffprobe 同目录；缺失时封面降级、片长探测退回自研头解析 |

## 开发

```bash
npm install
npm run tauri dev
```

## 构建

```bash
# 便携版 exe（本项目约定，不产安装包）
npm run tauri build -- --no-bundle
# 产物：src-tauri/target/release/cine-vault.exe
```

> 本机 cargo 全局镜像为清华 tuna，git 全局代理 127.0.0.1:7890 常关。
> 拉依赖失败时用：`NO_PROXY="*" no_proxy="*" cargo build`

## 局域网手机观看

桌面端打开「局域网访问」生成配对码（10 分钟有效，全局与单 IP 双档限流），手机端输入配对码即可播放。播放票据 6 小时有效，发现应答走 HMAC-SHA256 防伪造抢答。

Android 端源码在 `android/`，构建不依赖 Gradle / AndroidX / 网络下载：

```bash
ANDROID_HOME=/path/to/Android/Sdk ./android/build.sh
```

需要 Android SDK（build-tools + android-35 平台包）与 JDK。APK 与签名密钥不入库。

## 结构

```
cine-vault/
├── src/                      # React 19 + TS 前端
│   ├── components/           # 侧栏/卡片墙/剧集/弹窗/设置抽屉
│   ├── hooks/                # 缩略图懒加载队列（并发 2，近 LRU 上限 240）
│   ├── lib/                  # 与后端同口径的筛选/排序
│   └── styles/               # theme / cards / settings
├── src-tauri/
│   └── src/
│       ├── classifier.rs     # 三层分类引擎（核心）
│       ├── consensus.rs      # 发布组名归一化
│       ├── scanner.rs        # 扫描 + 体积规则
│       ├── media_info.rs     # 自研容器头解析（mp4/mkv 片长）+ ffprobe 兜底
│       ├── thumbnail.rs      # ffmpeg 静帧缓存
│       ├── lan.rs            # 局域网只读 HTTP + 配对鉴权
│       ├── db.rs             # 数据库层（schema v1）
│       ├── migrator.rs       # 旧库迁移
│       ├── commands.rs       # Tauri 命令
│       └── models.rs
└── android/                  # 零 Gradle 的 Android 观看端
```

## 数据

- 新库：`%APPDATA%/com.cinevault.app/cinevault.db`
- 缩略图缓存：`%APPDATA%/com.cinevault.app/thumbnails/`
- 导入 / 清理前整库快照：`%APPDATA%/com.cinevault.app/backups/`
- 旧库（可迁移）：`%APPDATA%/com.movieui.app/movieui.db`

全程离线：除局域网自有服务外不发起任何网络请求，不上传文件名、路径与画面。

## 已知限制

- 主目标平台为 Windows；跨平台构建需自行替换 socket 继承与注册表读取路径
- 封面质量取决于 ffmpeg 能否在目标位置解出关键帧，部分元数据损坏的文件会退到片头帧
- 数据库为单文件 SQLite，不提供多机同步

## 许可证

本仓库暂未附带开源许可证（All rights reserved）。如需开源，补充 `LICENSE` 后生效。
