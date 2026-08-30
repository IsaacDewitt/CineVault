// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod classifier;
mod commands;
mod db;
mod media_info;
mod migrator;
mod models;
mod scanner;

use rusqlite::Connection;
use std::fs;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to get app data dir");
            fs::create_dir_all(&data_dir).expect("failed to create data dir");

            let db_path = data_dir.join("cinevault.db");
            let conn = Connection::open(&db_path).expect("failed to open database");
            // synchronous=NORMAL 必须显式设置：WAL 模式下 SQLite 默认是 FULL，
            // 每个事务提交都要等磁盘 fsync。实测（生产库副本 300 条 UPDATE）：
            //   逐条提交 FULL 31.62 ms/条 → NORMAL 0.29 ms/条（109 倍）。
            // WAL + NORMAL 是 SQLite 官方推荐的桌面应用配置，断电最坏只丢最后一次提交，
            // 对"重新扫一遍就能重建"的媒体库数据完全可接受。
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=NORMAL;
                 PRAGMA foreign_keys=ON;
                 PRAGMA busy_timeout=5000;",
            )
            .expect("failed to set pragmas");

            db::initialize_database(&conn).expect("failed to initialize database");
            // 存量重复历史清理（同一观影会话被拆成多条）：幂等，跑过一次就记 settings 跳过
            if let Err(e) = db::dedupe_watch_history_sessions(&conn) {
                eprintln!("[cinevault] 历史去重跳过: {}", e);
            }
            app.manage(db::DbConn::init(conn));
            app.manage(commands::BackfillCancel::default());
            app.manage(commands::SyncInFlight::default());

            // ===== 系统托盘 =====
            let show_item = MenuItemBuilder::with_id("show", "显示窗口").build(app)?;
            let exit_item = MenuItemBuilder::with_id("exit", "退出").build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&show_item)
                .separator()
                .item(&exit_item)
                .build()?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().expect("no default icon"))
                .tooltip("CineVault")
                .menu(&menu)
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::DoubleClick { .. } = event {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "exit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            // ===== 窗口关闭事件拦截 =====
            let window = app
                .get_webview_window("main")
                .expect("failed to get main window");
            let app_handle = app.handle().clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let Some(window) = app_handle.get_webview_window("main") else {
                        return;
                    };
                    let handle = app_handle.clone();
                    window
                        .dialog()
                        .message("您希望最小化到托盘还是直接退出？")
                        .title("关闭确认")
                        .kind(MessageDialogKind::Info)
                        .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancelCustom(
                            "最小化到托盘".to_string(),
                            "退出".to_string(),
                        ))
                        .show(move |answer| {
                            if answer {
                                let _ = window.hide();
                            } else {
                                handle.exit(0);
                            }
                        });
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::scan_folder,
            commands::add_series_from_folder,
            commands::refresh_all_scans,
            commands::get_videos,
            commands::get_recently_added,
            commands::get_series_overview,
            commands::get_series_episodes,
            commands::get_video_detail,
            commands::get_history,
            commands::get_stats,
            commands::get_tags,
            commands::create_tag,
            commands::update_tag,
            commands::delete_tag,
            commands::add_video_tag,
            commands::remove_video_tag,
            commands::add_series_tag,
            commands::remove_series_tag,
            commands::set_rating,
            commands::set_series_rating,
            commands::toggle_favorite,
            commands::toggle_series_favorite,
            commands::mark_series_watched,
            commands::update_progress,
            commands::mark_played,
            commands::open_file,
            commands::sync_progress_from_player,
            commands::backfill_durations,
            commands::cancel_duration_backfill,
            commands::get_ffprobe_status,
            commands::open_folder,
            commands::rename_video,
            commands::rename_series,
            commands::delete_video,
            commands::delete_series,
            commands::delete_video_with_file,
            commands::get_pending,
            commands::resolve_pending,
            commands::get_scan_folders,
            commands::delete_scan_folder,
            commands::check_migration,
            commands::migrate_from_old,
            commands::export_json,
            commands::import_json,
            commands::read_text_file,
            commands::write_text_file,
            commands::get_drives,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
