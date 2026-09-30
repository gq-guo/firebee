mod commands;
pub mod core;

use tauri::menu::{Menu, MenuItemBuilder, MenuItemKind, SubmenuBuilder};
use tauri::Emitter;

use crate::core::storage::Storage;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Storage::new(Storage::default_dir()))
        .manage(commands::Pending::default())
        .manage(commands::Cookies::default())
        .setup(|app| {
            // 默认菜单（App / Edit 的复制粘贴等）+ 我们自己的 Request 菜单，快捷键在菜单栏可见
            let menu = Menu::default(app.handle())?;
            // 「检查更新…」放进 App 菜单，紧跟 About（macOS 惯例）
            if let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.into_iter().next() {
                app_menu.insert(
                    &MenuItemBuilder::with_id("check-updates", "Check for Updates…").build(app)?,
                    1,
                )?;
            }
            let request = SubmenuBuilder::new(app, "Request")
                .item(
                    &MenuItemBuilder::with_id("send", "Send")
                        .accelerator("CmdOrCtrl+Enter")
                        .build(app)?,
                )
                .item(
                    &MenuItemBuilder::with_id("new", "New Request")
                        .accelerator("CmdOrCtrl+N")
                        .build(app)?,
                )
                .separator()
                .item(
                    &MenuItemBuilder::with_id("find", "Find in Response")
                        .accelerator("CmdOrCtrl+F")
                        .build(app)?,
                )
                .item(
                    &MenuItemBuilder::with_id("filter", "Filter Collections")
                        .accelerator("CmdOrCtrl+Shift+F")
                        .build(app)?,
                )
                .build()?;
            menu.append(&request)?;
            app.set_menu(menu)?;
            app.on_menu_event(|handle, event| {
                let _ = handle.emit("menu", event.id().0.clone());
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::load_data,
            commands::dynamic_vars,
            commands::debug_build,
            commands::save_collections,
            commands::save_environments,
            commands::save_history,
            commands::missing_vars,
            commands::send_request,
            commands::cancel_request,
            commands::export_code,
            commands::import_curl,
            commands::export_collection,
            commands::import_file,
            commands::clear_cookies,
            commands::save_file,
            commands::restart_app,
            commands::open_releases,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
