// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod db;
mod models;
mod scanner;
mod thumbnail;

use rusqlite::Connection;
use std::fs;
use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // 获取数据目录
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to get app data dir");
            fs::create_dir_all(&data_dir).expect("failed to create data dir");

            // 初始化数据库
            let db_path = data_dir.join("movieui.db");
            let conn = Connection::open(&db_path).expect("failed to open database");

            // 启用 WAL 模式和外键约束
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA foreign_keys=ON;
                 PRAGMA busy_timeout=5000;"
            ).expect("failed to set pragmas");

            db::initialize_database(&conn).expect("failed to initialize database");

            // 注册数据库连接
            app.manage(db::DbConn::init(conn));

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::scan_videos,
            commands::scan_series,
            commands::get_movies,
            commands::get_series_overview,
            commands::get_series_episodes,
            commands::mark_series_watched,
            commands::set_series_rating,
            commands::get_videos,
            commands::search_videos,
            commands::get_videos_by_tag,
            commands::get_videos_by_tags,
            commands::get_favorites,
            commands::get_series,
            commands::set_rating,
            commands::toggle_favorite,
            commands::update_progress,
            commands::get_history,
            commands::get_video_detail,
            commands::get_tags,
            commands::create_tag,
            commands::delete_tag,
            commands::update_tag,
            commands::add_video_tag,
            commands::remove_video_tag,
            commands::update_tag_orders,
            commands::make_thumbnail,
            commands::open_file,
            commands::open_folder,
            commands::get_stats,
            commands::export_json,
            commands::read_text_file,
            commands::import_json,
            commands::toggle_watched,
            commands::rename_video,
            commands::rename_series,
            commands::delete_video,
            commands::delete_series,
            commands::add_series_tag,
            commands::delete_series_with_files,
            commands::delete_video_with_file,
            commands::delete_videos_by_folder,
            commands::clean_non_videos,
            commands::get_drives,
            commands::get_scan_folders,
            commands::delete_scan_folder,
            commands::refresh_all_scans,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
