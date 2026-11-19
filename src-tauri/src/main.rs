// AvG — оптимизатор Android-устройств через ADB.
// Copyright (C) 2026 DezFix
// SPDX-License-Identifier: GPL-3.0-or-later

// Без консольного окна в релизной сборке под Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod state;

use std::sync::Arc;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let state = state::AppState::init(app.handle())?;
            app.manage(Arc::new(state));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::set_adb_path,
            commands::discover_adb,
            commands::download_adb,
            commands::list_devices,
            commands::scan_device,
            commands::get_plan,
            commands::apply_plan,
            commands::cancel_apply,
            commands::list_snapshots,
            commands::revert_change,
            commands::revert_snapshot,
            commands::revert_all,
            commands::list_apps,
            commands::app_action,
            commands::install_apk,
            commands::pull_apk,
            commands::run_shell,
            commands::scan_apks,
            commands::install_queue,
            commands::stop_install,
            commands::load_icons,
        ])
        .run(tauri::generate_context!())
        .expect("не удалось запустить AvG");
}
