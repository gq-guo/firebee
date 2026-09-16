use egui;

use crate::core::models::ResponseMeta;
use crate::ui::app::{FirebeeApp, RespTab};

fn status_color(status: u16) -> egui::Color32 {
    match status {
        200..=299 => egui::Color32::from_rgb(60, 170, 90),
        300..=399 => egui::Color32::from_rgb(80, 140, 220),
        400..=499 => egui::Color32::from_rgb(220, 140, 40),
        _ => egui::Color32::from_rgb(220, 60, 60),
    }
}

fn fmt_size(n: usize) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

/// 只读代码视图，带语法高亮
fn code_view(ui: &mut egui::Ui, id: &str, code: &str, language: &str) {
    let theme = egui_extras::syntax_highlighting::CodeTheme::from_memory(ui.ctx(), ui.style());
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        let mut job = egui_extras::syntax_highlighting::highlight(
            ui.ctx(),
            ui.style(),
            &theme,
            buf.as_str(),
            language,
        );
        job.wrap.max_width = wrap_width;
        ui.fonts(|f| f.layout_job(job))
    };
    let mut owned = code.to_string();
    egui::ScrollArea::both().id_salt(id).show(ui, |ui| {
        ui.add(
            egui::TextEdit::multiline(&mut owned)
                .font(egui::TextStyle::Monospace)
                .layouter(&mut layouter)
                .interactive(false)
                .desired_width(f32::INFINITY),
        );
    });
}

fn show_response(ui: &mut egui::Ui, resp_tab: &mut RespTab, resp: &ResponseMeta) {
    ui.horizontal(|ui| {
        ui.colored_label(status_color(resp.status), format!("{}", resp.status));
        ui.label(format!("{} ms", resp.duration_ms));
        ui.label(fmt_size(resp.size_bytes));
        if ui.button("Copy Body").clicked() {
            ui.ctx().copy_text(String::from_utf8_lossy(&resp.body).to_string());
        }
    });
    ui.horizontal(|ui| {
        ui.selectable_value(resp_tab, RespTab::Body, "Body");
        ui.selectable_value(resp_tab, RespTab::Headers, "Headers");
    });
    ui.separator();
    match resp_tab {
        RespTab::Body => {
            let text = String::from_utf8_lossy(&resp.body);
            // 尝试 JSON 美化
            let (pretty, lang) = match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) => (serde_json::to_string_pretty(&v).unwrap_or_else(|_| text.to_string()), "json"),
                Err(_) => (text.to_string(), "txt"),
            };
            code_view(ui, "resp_body", &pretty, lang);
        }
        RespTab::Headers => {
            egui::Grid::new("resp_headers").num_columns(2).striped(true).show(ui, |ui| {
                for (k, v) in &resp.headers {
                    ui.monospace(k);
                    ui.monospace(v);
                    ui.end_row();
                }
            });
        }
    }
}

pub fn show(ui: &mut egui::Ui, app: &mut FirebeeApp) {
    ui.heading("Response");
    let pending = app.pending.is_some();
    let Some(res) = &app.response else {
        if pending {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Sending...");
            });
        } else {
            ui.weak("No request sent yet");
        }
        return;
    };
    match res {
        Err(e) => {
            let e = e.clone();
            ui.colored_label(egui::Color32::from_rgb(220, 60, 60), e);
        }
        Ok(resp) => show_response(ui, &mut app.resp_tab, resp),
    }
}
