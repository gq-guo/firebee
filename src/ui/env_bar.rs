use std::time::Instant;

use egui;

use crate::core::models::Environment;
use crate::ui::app::FirebeeApp;
use crate::ui::request_panel::kv_table;

pub fn show(ctx: &egui::Context, app: &mut FirebeeApp) {
    egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Env:");
            let current = app
                .active_env
                .and_then(|id| app.environments.iter().find(|e| e.id == id))
                .map(|e| e.name.clone())
                .unwrap_or_else(|| "No Environment".to_string());
            egui::ComboBox::from_id_salt("env")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(app.active_env.is_none(), "No Environment").clicked() {
                        app.active_env = None;
                    }
                    for i in 0..app.environments.len() {
                        let id = app.environments[i].id;
                        let name = app.environments[i].name.clone();
                        if ui.selectable_label(app.active_env == Some(id), name).clicked() {
                            app.active_env = Some(id);
                        }
                    }
                });
            if ui.button("Manage").clicked() {
                app.show_env_window = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label("Timeout(s)");
                ui.add(egui::DragValue::new(&mut app.timeout_secs).range(1..=300));
            });
        });
    });

    if app.show_env_window {
        env_window(ctx, app);
    }
}

fn env_window(ctx: &egui::Context, app: &mut FirebeeApp) {
    let mut open = true;
    egui::Window::new("Environments")
        .open(&mut open)
        .default_size([560.0, 320.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_min_width(140.0);
                    for i in 0..app.environments.len() {
                        let name = app.environments[i].name.clone();
                        if ui.selectable_label(app.env_sel == Some(i), name).clicked() {
                            app.env_sel = Some(i);
                        }
                    }
                    if ui.button("+ New Environment").clicked() {
                        app.environments.push(Environment {
                            id: uuid::Uuid::new_v4(),
                            name: format!("Environment {}", app.environments.len() + 1),
                            variables: vec![],
                        });
                        app.env_sel = Some(app.environments.len() - 1);
                        app.dirty_since = Some(Instant::now());
                    }
                    if let Some(i) = app.env_sel {
                        if i < app.environments.len() && ui.button("Delete").clicked() {
                            let removed = app.environments.remove(i);
                            if app.active_env == Some(removed.id) {
                                app.active_env = None;
                            }
                            app.env_sel = None;
                            app.dirty_since = Some(Instant::now());
                        }
                    }
                });
                ui.separator();
                if let Some(i) = app.env_sel.filter(|&i| i < app.environments.len()) {
                    let changed = {
                        let env = &mut app.environments[i];
                        let mut changed = ui.text_edit_singleline(&mut env.name).changed();
                        ui.label("Variables (reference as {{name}} in requests):");
                        changed |= kv_table(ui, "env_vars", &mut env.variables, "Name", "Value");
                        changed
                    };
                    if changed {
                        app.dirty_since = Some(Instant::now());
                    }
                } else {
                    ui.weak("Select or create an environment");
                }
            });
        });
    if !open {
        app.show_env_window = false;
    }
}
