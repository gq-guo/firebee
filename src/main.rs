#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use firebee_lib::core::storage::Storage;

fn main() {
    init_logging();
    firebee_lib::run();
}

fn init_logging() {
    let dir = Storage::default_dir();
    let _ = std::fs::create_dir_all(&dir);
    let file_appender = tracing_appender::rolling::daily(dir, "firebee.log");
    tracing_subscriber::fmt()
        .with_writer(file_appender)
        .with_ansi(false)
        .init();
}
