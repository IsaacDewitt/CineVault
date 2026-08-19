# CineVault - 本地影视库管理器

Rust + Tauri 2 重写的本地视频管理工具（前身 MovieUI）。纯文字信息墙 UI，不依赖网络刮削。

## 特性

- **智能分类**：三层判定引擎（文件名规则 → 目录上下文 → 概率融合），输出类型 + 置信度，拿不准的进"待确认队列"由用户裁决，裁决结果永久记忆
- **增量扫描 + 移动匹配**：文件/剧集文件夹搬家不丢评分、标签、进度、收藏
- **剧集实体**：评分/标签/收藏挂在剧集上，不依赖单集
- **旧库一键迁移**：直接读 MovieUI 的 `movieui.db` 全量导入（视频/剧集/标签/历史/扫描目录）
- **播放历史/进度/续播**：打开即记录，进度条随行
- **纯文字信息墙**：类型徽章 + 评分 + 标签 + 进度，深色琥珀金设计系统

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

> 注意：本机 cargo 全局镜像为清华 tuna 且 git 全局代理 127.0.0.1:7890 常关闭。
> 拉依赖失败时用：`NO_PROXY="*" no_proxy="*" cargo build`

## 结构

```
cine-vault/
├── src/                # React 19 + TS 前端
│   ├── components/     # 侧栏/列表墙/剧集/弹窗/待确认/迁移
│   └── styles/theme.css# 设计系统
└── src-tauri/
    ├── src/
    │   ├── classifier.rs # 三层分类引擎（核心）
    │   ├── scanner.rs    # 扫描 + 同级上下文
    │   ├── db.rs         # 数据库层（schema v1）
    │   ├── migrator.rs   # 旧库迁移
    │   ├── commands.rs   # Tauri 命令
    │   └── models.rs
    └── Cargo.toml
```

## 数据

- 新库：`%APPDATA%/com.cinevault.app/cinevault.db`
- 旧库（可迁移）：`%APPDATA%/com.movieui.app/movieui.db`
