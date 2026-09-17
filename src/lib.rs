pub mod core;
mod commands;

use crate::core::storage::Storage;

pub fn run() {
    tauri::Builder::default()
        .manage(Storage::new(Storage::default_dir()))
        .manage(commands::Pending::default())
        .invoke_handler(tauri::generate_handler![
            commands::load_data,
            commands::save_collections,
            commands::save_environments,
            commands::save_history,
            commands::missing_vars,
            commands::send_request,
            commands::cancel_request,
            commands::export_code,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
