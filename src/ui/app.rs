use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use crate::core::http::HttpError;
use crate::core::models::*;
use crate::core::storage::{Storage, HISTORY_LIMIT};
use crate::core::vars::substitute_request;
use crate::worker::{Job, JobResult};

pub enum SidebarTab {
    Collections,
    History,
}

#[derive(PartialEq, Clone, Copy)]
pub enum ReqTab {
    Params,
    Headers,
    Body,
    Auth,
}

#[derive(PartialEq, Clone, Copy)]
pub enum RespTab {
    Body,
    Headers,
}

pub struct Pending {
    pub id: u64,
    pub cancel_tx: tokio::sync::watch::Sender<bool>,
}

pub struct FirebeeApp {
    pub storage: Storage,
    pub collections: Vec<Collection>,
    pub environments: Vec<Environment>,
    pub active_env: Option<uuid::Uuid>,
    pub history: Vec<HistoryEntry>,
    pub current: Request,
    pub response: Option<Result<ResponseMeta, String>>,
    pub pending: Option<Pending>,
    pub sidebar_tab: SidebarTab,
    pub req_tab: ReqTab,
    pub resp_tab: RespTab,
    pub show_env_window: bool,
    pub env_sel: Option<usize>,
    pub export_text: Option<String>,
    pub missing_vars: Vec<String>,
    pub timeout_secs: u64,
    pub dirty_since: Option<Instant>,
    job_tx: Sender<Job>,
    result_rx: Receiver<JobResult>,
    next_job_id: u64,
}

impl FirebeeApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let storage = Storage::new(Storage::default_dir());
        let (job_tx, job_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        crate::worker::spawn_worker(job_rx, result_tx);
        Self {
            collections: storage.load_collections(),
            environments: storage.load_environments(),
            history: storage.load_history(),
            storage,
            active_env: None,
            current: Request::new("新请求"),
            response: None,
            pending: None,
            sidebar_tab: SidebarTab::Collections,
            req_tab: ReqTab::Params,
            resp_tab: RespTab::Body,
            show_env_window: false,
            env_sel: None,
            export_text: None,
            missing_vars: vec![],
            timeout_secs: crate::core::http::DEFAULT_TIMEOUT.as_secs(),
            dirty_since: None,
            job_tx,
            result_rx,
            next_job_id: 1,
        }
    }

    pub fn env_vars(&self) -> HashMap<String, String> {
        self.active_env
            .and_then(|id| self.environments.iter().find(|e| e.id == id))
            .map(|e| e.var_map())
            .unwrap_or_default()
    }

    pub fn mark_dirty(&mut self) {
        self.dirty_since = Some(Instant::now());
    }

    /// 发送当前请求。有未定义变量时第一次点击只提示，第二次点击强制发送。
    pub fn send(&mut self) {
        let (substituted, missing) = substitute_request(&self.current, &self.env_vars());
        if !missing.is_empty() && self.missing_vars != missing {
            self.missing_vars = missing;
            return;
        }
        self.missing_vars.clear();
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let id = self.next_job_id;
        self.next_job_id += 1;
        let _ = self.job_tx.send(Job {
            id,
            request: substituted,
            timeout: Duration::from_secs(self.timeout_secs.max(1)),
            cancel: cancel_rx,
        });
        self.pending = Some(Pending { id, cancel_tx });
        self.response = None;
    }

    pub fn cancel(&mut self) {
        if let Some(p) = &self.pending {
            let _ = p.cancel_tx.send(true);
        }
    }

    fn poll_results(&mut self, ctx: &egui::Context) {
        while let Ok(res) = self.result_rx.try_recv() {
            self.pending = None;
            let (status, duration_ms) = match &res.result {
                Ok(r) => (Some(r.status), Some(r.duration_ms)),
                Err(_) => (None, None),
            };
            self.history.push(HistoryEntry {
                timestamp: chrono::Local::now(),
                request: self.current.clone(),
                status,
                duration_ms,
            });
            if self.history.len() > HISTORY_LIMIT {
                let n = self.history.len() - HISTORY_LIMIT;
                self.history.drain(..n);
            }
            if let Err(e) = self.storage.save_history(&self.history) {
                tracing::error!("保存历史失败: {e}");
            }
            self.response = Some(res.result.map_err(|e: HttpError| e.to_string()));
        }
        if self.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        // 防抖保存集合与环境
        if let Some(since) = self.dirty_since {
            if since.elapsed() > Duration::from_millis(500) {
                if let Err(e) = self.storage.save_collections(&self.collections) {
                    tracing::error!("保存集合失败: {e}");
                }
                if let Err(e) = self.storage.save_environments(&self.environments) {
                    tracing::error!("保存环境失败: {e}");
                }
                self.dirty_since = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(200));
            }
        }
    }
}

impl eframe::App for FirebeeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_results(ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            crate::ui::request_panel::show(ui, self);
            ui.separator();
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.storage.save_collections(&self.collections);
        let _ = self.storage.save_environments(&self.environments);
        let _ = self.storage.save_history(&self.history);
    }
}
