mod core;
mod ui;
mod worker;

fn main() -> eframe::Result<()> {
    init_logging();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Firebee",
        options,
        Box::new(|cc| Ok(Box::new(ui::app::FirebeeApp::new(cc)))),
    )
}

fn init_logging() {
    let dir = core::storage::Storage::default_dir();
    let _ = std::fs::create_dir_all(&dir);
    let file_appender = tracing_appender::rolling::daily(dir, "firebee.log");
    tracing_subscriber::fmt()
        .with_writer(file_appender)
        .with_ansi(false)
        .init();
}
