use egui;

use crate::core::models::{Auth, BodyType, HttpMethod, KeyValue};
use crate::ui::app::{FirebeeApp, ReqTab};

/// 通用 key-value 表格编辑器，返回是否有修改（供调用方 mark_dirty）。
pub fn kv_table(
    ui: &mut egui::Ui,
    id: &str,
    rows: &mut Vec<KeyValue>,
    key_hint: &str,
    val_hint: &str,
) -> bool {
    let mut changed = false;
    let mut remove: Option<usize> = None;
    egui::Grid::new(ui.id().with(id))
        .num_columns(4)
        .show(ui, |ui| {
            for (i, kv) in rows.iter_mut().enumerate() {
                changed |= ui.checkbox(&mut kv.enabled, "").changed();
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut kv.key).hint_text(key_hint).desired_width(180.0))
                    .changed();
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut kv.value).hint_text(val_hint).desired_width(320.0))
                    .changed();
                if ui.button("✕").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
    if let Some(i) = remove {
        rows.remove(i);
        changed = true;
    }
    if ui.button("+ Add").clicked() {
        rows.push(KeyValue::new("", ""));
        changed = true;
    }
    changed
}

pub fn show(ui: &mut egui::Ui, app: &mut FirebeeApp) {
    // 第一行：方法 + URL + 发送/取消 + 导出
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("method")
            .selected_text(app.current.method.as_str())
            .show_ui(ui, |ui| {
                for m in HttpMethod::ALL {
                    ui.selectable_value(&mut app.current.method, m, m.as_str());
                }
            });
        ui.add(
            egui::TextEdit::singleline(&mut app.current.url)
                .hint_text("https://api.example.com/path  ({{vars}} supported)")
                .desired_width(ui.available_width() - 220.0),
        );
        if app.pending.is_some() {
            if ui.button("Cancel").clicked() {
                app.cancel();
            }
            ui.spinner();
        } else if ui.button("Send").clicked() {
            app.send();
        }
        ui.menu_button("Export ▾", |ui| {
            if ui.button("curl command").clicked() {
                app.export_text = Some(crate::ui::export_dialog::render(app, crate::ui::export_dialog::Kind::Curl));
                ui.close();
            }
            if ui.button("Python (requests)").clicked() {
                app.export_text = Some(crate::ui::export_dialog::render(app, crate::ui::export_dialog::Kind::Python));
                ui.close();
            }
        });
    });

    if !app.missing_vars.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(220, 120, 40),
            format!("Undefined variables: {} (click Send again to send anyway)", app.missing_vars.join(", ")),
        );
    }

    // Tab 条
    ui.horizontal(|ui| {
        ui.selectable_value(&mut app.req_tab, ReqTab::Params, "Params");
        ui.selectable_value(&mut app.req_tab, ReqTab::Headers, "Headers");
        ui.selectable_value(&mut app.req_tab, ReqTab::Body, "Body");
        ui.selectable_value(&mut app.req_tab, ReqTab::Auth, "Auth");
    });
    ui.separator();

    let mut dirty = false;
    match app.req_tab {
        ReqTab::Params => {
            dirty = kv_table(ui, "params", &mut app.current.params, "Key", "Value");
        }
        ReqTab::Headers => {
            dirty = kv_table(ui, "headers", &mut app.current.headers, "Header", "Value");
        }
        ReqTab::Body => {
            ui.horizontal(|ui| {
                for (t, label) in [
                    (BodyType::None, "None"),
                    (BodyType::Json, "JSON"),
                    (BodyType::Text, "Text"),
                    (BodyType::Form, "Form"),
                ] {
                    dirty |= ui
                        .radio_value(&mut app.current.body_type, t, label)
                        .changed();
                }
            });
            match app.current.body_type {
                BodyType::Json | BodyType::Text => {
                    dirty |= egui::ScrollArea::vertical()
                        .id_salt("body_editor")
                        .max_height(180.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut app.current.body)
                                    .code_editor()
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(8),
                            )
                        })
                        .inner
                        .changed();
                }
                BodyType::Form => {
                    dirty = kv_table(ui, "form", &mut app.current.form, "Field", "Value");
                }
                BodyType::None => {}
            }
        }
        ReqTab::Auth => {
            dirty = auth_editor(ui, &mut app.current.auth);
        }
    }
    if dirty {
        app.mark_dirty();
    }
}

fn auth_editor(ui: &mut egui::Ui, auth: &mut Auth) -> bool {
    let mut changed = false;
    let kind = match auth {
        Auth::None => 0,
        Auth::Bearer { .. } => 1,
        Auth::Basic { .. } => 2,
        Auth::ApiKey { .. } => 3,
    };
    ui.horizontal(|ui| {
        for (v, label) in [(0, "None"), (1, "Bearer Token"), (2, "Basic Auth"), (3, "API Key")] {
            if ui.radio(kind == v, label).clicked() && kind != v {
                *auth = match v {
                    1 => Auth::Bearer { token: String::new() },
                    2 => Auth::Basic { username: String::new(), password: String::new() },
                    3 => Auth::ApiKey { key: String::new(), value: String::new(), in_query: false },
                    _ => Auth::None,
                };
                changed = true;
            }
        }
    });
    match auth {
        Auth::Bearer { token } => {
            ui.horizontal(|ui| {
                ui.label("Token:");
                changed |= ui.text_edit_singleline(token).changed();
            });
        }
        Auth::Basic { username, password } => {
            ui.horizontal(|ui| {
                ui.label("Username:");
                changed |= ui.text_edit_singleline(username).changed();
                ui.label("Password:");
                changed |= ui.add(egui::TextEdit::singleline(password).password(true)).changed();
            });
        }
        Auth::ApiKey { key, value, in_query } => {
            ui.horizontal(|ui| {
                ui.label("Key:");
                changed |= ui.text_edit_singleline(key).changed();
                ui.label("Value:");
                changed |= ui.text_edit_singleline(value).changed();
                changed |= ui.checkbox(in_query, "Send in query params").changed();
            });
        }
        Auth::None => {}
    }
    changed
}
