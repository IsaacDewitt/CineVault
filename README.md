# MovieUI - 本地视频管理器

一个基于 Tauri (Rust + Web) 的本地视频文件管理应用。

## ✨ 功能特性

- **视频扫描** - 递归扫描磁盘中的所有视频文件，支持增量扫描
- **智能归类** - 自动识别电视剧文件夹，将剧集归为一组
- **标签系统** - 自定义标签，支持多标签、批量操作、按标签筛选
- **评分系统** - 0-10 分制精细评分，按评分筛选排序
- **播放历史** - 记录观看进度，剧集续播提醒
- **收藏夹** - 收藏喜爱的视频，快速访问
- **视频封面** - 自动从视频截取关键帧作为封面
- **数据导出** - 支持 JSON 格式导出所有数据
- **深色主题** - 现代化深色 UI 设计

## 🛠️ 技术栈

- **后端**: Rust + Tauri 2
- **前端**: Vanilla HTML/CSS/JS + Vite
- **数据库**: SQLite (本地存储)
- **打包**: Tauri 打包为原生 exe (~10MB)

## 📦 项目结构

```
Movie UI/
├── src-tauri/          # Rust 后端
│   ├── src/
│   │   ├── main.rs     # 入口
│   │   ├── db.rs       # 数据库操作
│   │   ├── commands.rs # Tauri 命令
│   │   ├── scanner.rs  # 视频扫描
│   │   ├── thumbnail.rs# 缩略图生成
│   │   └── models.rs   # 数据模型
│   ├── Cargo.toml
│   └── tauri.conf.json
├── src/                # 前端
│   ├── css/style.css
│   └── main.js
├── index.html
├── package.json
└── vite.config.js
```

## 🚀 开发

```bash
# 安装依赖
npm install

# 开发模式运行
npm run tauri dev

# 构建发布版本
npm run tauri build
```

## 📋 支持的视频格式

mp4, mkv, avi, rmvb, wmv, flv, mov, ts, m4v, mpg, mpeg, webm, vob, ogv, 3gp, f4v, mts, m2ts, divx, asf, rm, tp, dat

## 📁 数据存储

- 数据库文件: `%APPDATA%/com.movieui.app/movieui.db`
- 缩略图: `%APPDATA%/com.movieui.app/thumbnails/`
- 导出数据: JSON 格式
