use egui;

use crate::core::models::{Collection, Folder, Request};
use crate::ui::app::{FirebeeApp, SidebarTab};

/// 定位树中一个节点：第 col 个集合，沿 folders 下标链进入文件夹，req 为 Some 时表示叶子是请求。
#[derive(Clone, PartialEq, Debug)]
pub struct TreePath {
    pub col: usize,
    pub folders: Vec<usize>,
    pub req: Option<usize>,
}

enum Action {
    NewCollection,
    NewFolder(TreePath), // 目标容器（req 忽略）
    NewRequest(TreePath),
    Delete(TreePath), // col+空folders+无req = 删除整个集合
    StartRename(TreePath),
    CommitRename(TreePath, String),
    CancelRename,
    Select(TreePath),
    LoadHistory(usize),
}

// ---------- 树访问助手 ----------

fn folder_ref<'a>(folders: &'a [Folder], path: &[usize]) -> Option<&'a Folder> {
    let (&first, rest) = path.split_first()?;
    let f = folders.get(first)?;
    if rest.is_empty() {
        Some(f)
    } else {
        folder_ref(&f.folders, rest)
    }
}

fn folder_mut<'a>(folders: &'a mut [Folder], path: &[usize]) -> Option<&'a mut Folder> {
    let (&first, rest) = path.split_first()?;
    let f = folders.get_mut(first)?;
    if rest.is_empty() {
        Some(f)
    } else {
        folder_mut(&mut f.folders, rest)
    }
}

/// path 为空 → 集合根；否则 → 指定文件夹。返回其 folders 列表。
fn folders_container<'a>(col: &'a mut Collection, path: &[usize]) -> Option<&'a mut Vec<Folder>> {
    if path.is_empty() {
        Some(&mut col.folders)
    } else {
        folder_mut(&mut col.folders, path).map(|f| &mut f.folders)
    }
}

fn requests_container<'a>(col: &'a mut Collection, path: &[usize]) -> Option<&'a mut Vec<Request>> {
    if path.is_empty() {
        Some(&mut col.requests)
    } else {
        folder_mut(&mut col.folders, path).map(|f| &mut f.requests)
    }
}

fn name_at(app: &FirebeeApp, p: &TreePath) -> Option<String> {
    let col = app.collections.get(p.col)?;
    if let Some(ri) = p.req {
        let reqs = if p.folders.is_empty() {
            &col.requests
        } else {
            &folder_ref(&col.folders, &p.folders)?.requests
        };
        reqs.get(ri).map(|r| r.name.clone())
    } else if p.folders.is_empty() {
        Some(col.name.clone())
    } else {
        folder_ref(&col.folders, &p.folders).map(|f| f.name.clone())
    }
}

fn request_at<'a>(app: &'a FirebeeApp, p: &TreePath) -> Option<&'a Request> {
    let ri = p.req?;
    let col = app.collections.get(p.col)?;
    let reqs = if p.folders.is_empty() {
        &col.requests
    } else {
        &folder_ref(&col.folders, &p.folders)?.requests
    };
    reqs.get(ri)
}

// ---------- 动作应用 ----------

fn apply(app: &mut FirebeeApp, actions: Vec<Action>) {
    for a in actions {
        match a {
            Action::NewCollection => {
                app.collections
                    .push(Collection::new(format!("Collection {}", app.collections.len() + 1)));
                app.mark_dirty();
            }
            Action::NewFolder(p) => {
                if let Some(col) = app.collections.get_mut(p.col) {
                    if let Some(slot) = folders_container(col, &p.folders) {
                        slot.push(Folder::new("New Folder"));
                        app.mark_dirty();
                    }
                }
            }
            Action::NewRequest(p) => {
                if let Some(col) = app.collections.get_mut(p.col) {
                    if let Some(slot) = requests_container(col, &p.folders) {
                        slot.push(Request::new("New Request"));
                        app.mark_dirty();
                    }
                }
            }
            Action::Delete(p) => {
                let Some(col) = app.collections.get_mut(p.col) else { continue };
                if let Some(ri) = p.req {
                    if let Some(slot) = requests_container(col, &p.folders) {
                        if ri < slot.len() {
                            slot.remove(ri);
                            app.mark_dirty();
                        }
                    }
                } else if let Some(&last) = p.folders.last() {
                    let parent = &p.folders[..p.folders.len() - 1];
                    if let Some(slot) = folders_container(col, parent) {
                        if last < slot.len() {
                            slot.remove(last);
                            app.mark_dirty();
                        }
                    }
                } else {
                    app.collections.remove(p.col);
                    app.mark_dirty();
                }
            }
            Action::StartRename(p) => {
                app.rename_buf = name_at(app, &p).unwrap_or_default();
                app.renaming = Some(p);
            }
            Action::CommitRename(p, name) => {
                let name = name.trim().to_string();
                if !name.is_empty() {
                    if let Some(col) = app.collections.get_mut(p.col) {
                        if let Some(ri) = p.req {
                            if let Some(slot) = requests_container(col, &p.folders) {
                                if let Some(r) = slot.get_mut(ri) {
                                    r.name = name;
                                    app.mark_dirty();
                                }
                            }
                        } else if let Some(&last) = p.folders.last() {
                            let parent = &p.folders[..p.folders.len() - 1];
                            if let Some(slot) = folders_container(col, parent) {
                                if let Some(f) = slot.get_mut(last) {
                                    f.name = name;
                                    app.mark_dirty();
                                }
                            }
                        } else {
                            col.name = name;
                            app.mark_dirty();
                        }
                    }
                }
                app.renaming = None;
            }
            Action::CancelRename => {
                app.renaming = None;
            }
            Action::Select(p) => {
                if let Some(req) = request_at(app, &p) {
                    app.current = req.clone();
                    app.response = None;
                    app.missing_vars.clear();
                }
            }
            Action::LoadHistory(i) => {
                if let Some(h) = app.history.get(i) {
                    app.current = h.request.clone();
                    app.response = None;
                    app.missing_vars.clear();
                }
            }
        }
    }
}

// ---------- 渲染 ----------

/// 重命名输入框：Enter 提交，Esc 取消
fn rename_editor(
    ui: &mut egui::Ui,
    path: &TreePath,
    buf: &mut String,
    actions: &mut Vec<Action>,
) {
    let resp = ui.text_edit_singleline(buf);
    resp.request_focus();
    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        actions.push(Action::CommitRename(path.clone(), buf.clone()));
    } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        actions.push(Action::CancelRename);
    }
    let _ = resp;
}

fn container_buttons(ui: &mut egui::Ui, path: &TreePath, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        if ui.small_button("+ Req").clicked() {
            actions.push(Action::NewRequest(path.clone()));
        }
        if ui.small_button("+ Folder").clicked() {
            actions.push(Action::NewFolder(path.clone()));
        }
        if ui.small_button("Rename").clicked() {
            actions.push(Action::StartRename(path.clone()));
        }
        if ui.small_button("Del").clicked() {
            actions.push(Action::Delete(path.clone()));
        }
    });
}

fn request_row(
    ui: &mut egui::Ui,
    path: TreePath,
    name: &str,
    renaming: bool,
    rename_buf: &str,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        if renaming {
            let mut buf = rename_buf.to_string();
            rename_editor(ui, &path, &mut buf, actions);
        } else {
            let resp = ui.button(format!("- {name}"));
            if resp.clicked() {
                actions.push(Action::Select(path.clone()));
            }
            resp.context_menu(|ui| {
                if ui.button("Rename").clicked() {
                    actions.push(Action::StartRename(path.clone()));
                    ui.close();
                }
                if ui.button("Delete").clicked() {
                    actions.push(Action::Delete(path.clone()));
                    ui.close();
                }
            });
        }
    });
}

fn folder_ui(
    ui: &mut egui::Ui,
    app: &FirebeeApp,
    path: &mut Vec<usize>, // [col, folder, ...]
    folder: &Folder,
    actions: &mut Vec<Action>,
) {
    let my_path = TreePath { col: path[0], folders: path[1..].to_vec(), req: None };
    let renaming = app.renaming.as_ref() == Some(&my_path);
    egui::CollapsingHeader::new(if renaming { "[rename]" } else { &folder.name })
        .id_salt(("folder", folder.id))
        .default_open(true)
        .show(ui, |ui| {
            if renaming {
                let mut buf = app.rename_buf.clone();
                rename_editor(ui, &my_path, &mut buf, actions);
            }
            container_buttons(ui, &my_path, actions);
            for (i, sub) in folder.folders.iter().enumerate() {
                path.push(i);
                folder_ui(ui, app, path, sub, actions);
                path.pop();
            }
            for (i, r) in folder.requests.iter().enumerate() {
                let rp = TreePath { col: my_path.col, folders: my_path.folders.clone(), req: Some(i) };
                let renaming = app.renaming.as_ref() == Some(&rp);
                request_row(ui, rp, &r.name, renaming, &app.rename_buf, actions);
            }
        });
}

pub fn show(ctx: &egui::Context, app: &mut FirebeeApp) {
    let mut actions = Vec::new();
    egui::SidePanel::left("sidebar")
        .default_width(240.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(matches!(app.sidebar_tab, SidebarTab::Collections), "Collections")
                    .clicked()
                {
                    app.sidebar_tab = SidebarTab::Collections;
                }
                if ui
                    .selectable_label(matches!(app.sidebar_tab, SidebarTab::History), "History")
                    .clicked()
                {
                    app.sidebar_tab = SidebarTab::History;
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                match app.sidebar_tab {
                    SidebarTab::Collections => {
                        if ui.button("+ New Collection").clicked() {
                            actions.push(Action::NewCollection);
                        }
                        for (ci, col) in app.collections.iter().enumerate() {
                            let col_path = TreePath { col: ci, folders: vec![], req: None };
                            let renaming = app.renaming.as_ref() == Some(&col_path);
                            egui::CollapsingHeader::new(if renaming { "[rename]" } else { &col.name })
                                .id_salt(("col", col.id))
                                .default_open(true)
                                .show(ui, |ui| {
                                    if renaming {
                                        let mut buf = app.rename_buf.clone();
                                        rename_editor(ui, &col_path, &mut buf, &mut actions);
                                    }
                                    container_buttons(ui, &col_path, &mut actions);
                                    let mut path = vec![ci];
                                    for (i, f) in col.folders.iter().enumerate() {
                                        path.push(i);
                                        folder_ui(ui, app, &mut path, f, &mut actions);
                                        path.pop();
                                    }
                                    for (i, r) in col.requests.iter().enumerate() {
                                        let rp = TreePath { col: ci, folders: vec![], req: Some(i) };
                                        let renaming = app.renaming.as_ref() == Some(&rp);
                                        request_row(ui, rp, &r.name, renaming, &app.rename_buf, &mut actions);
                                    }
                                });
                        }
                    }
                    SidebarTab::History => {
                        for (i, h) in app.history.iter().enumerate().rev() {
                            let label = format!(
                                "{} {} {} {}",
                                h.timestamp.format("%H:%M"),
                                h.request.method.as_str(),
                                h.status.map(|s| s.to_string()).unwrap_or("ERR".into()),
                                truncate(&h.request.url, 30),
                            );
                            if ui.button(label).clicked() {
                                actions.push(Action::LoadHistory(i));
                            }
                        }
                    }
                }
            });
        });
    apply(app, actions);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!("{}...", s.chars().take(max).collect::<String>())
    } else {
        s.to_string()
    }
}
