use egui;

use crate::core::http::build_url;
use crate::core::vars::substitute_request;
use crate::ui::app::FirebeeApp;

pub enum Kind {
    Curl,
    Python,
}

/// 生成导出文本：先变量替换，再构造最终 URL
pub fn render(app: &mut FirebeeApp, kind: Kind) -> String {
    let (req, _) = substitute_request(&app.current, &app.env_vars());
    let url = build_url(&req).unwrap_or_else(|_| req.url.clone());
    match kind {
        Kind::Curl => crate::core::export::to_curl(&req, &url),
        Kind::Python => crate::core::export::to_python(&req, &url),
    }
}

pub fn show(ctx: &egui::Context, app: &mut FirebeeApp) {
    let Some(text) = app.export_text.clone() else { return };
    let mut open = true;
    egui::Window::new("Export")
        .open(&mut open)
        .default_size([560.0, 360.0])
        .show(ctx, |ui| {
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(text.clone());
            }
            let mut t = text;
            egui::ScrollArea::both().show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut t)
                        .font(egui::TextStyle::Monospace)
                        .interactive(false)
                        .desired_width(f32::INFINITY),
                );
            });
        });
    if !open {
        app.export_text = None;
    }
}
