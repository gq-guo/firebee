pub mod core;
mod commands;

use crate::core::storage::Storage;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Storage::new(Storage::default_dir()))
        .manage(commands::Pending::default())
        .manage(commands::Cookies::default())
        .invoke_handler(tauri::generate_handler![
            commands::load_data,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
