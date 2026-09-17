//! Tauri commands：前端持有全部 UI 状态，后端只做存储 / HTTP / 变量替换 / 导出。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::State;
use tokio::sync::watch;

use crate::core::export::{to_curl, to_python};
use crate::core::http::{build_url, execute};
use crate::core::models::{Collection, Environment, HistoryEntry, Request};
use crate::core::storage::Storage;
use crate::core::vars::substitute_request;

/// 进行中的请求：job_id → 取消信号
#[derive(Default)]
pub struct Pending(Mutex<HashMap<u64, watch::Sender<bool>>>);

#[derive(Serialize)]
pub struct AppData {
    collections: Vec<Collection>,
    environments: Vec<Environment>,
    history: Vec<HistoryEntry>,
}

/// ResponseMeta 的前端视图：body 以字符串下发（Vec<u8> 序列化成 JSON 数组太大）
#[derive(Serialize)]
pub struct ResponseDto {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    duration_ms: u128,
    size_bytes: usize,
}

fn vars(env: &Option<Environment>) -> HashMap<String, String> {
    env.as_ref().map(|e| e.var_map()).unwrap_or_default()
}

#[tauri::command]
pub fn load_data(storage: State<Storage>) -> AppData {
    AppData {
        collections: storage.load_collections(),
        environments: storage.load_environments(),
        history: storage.load_history(),
    }
}

#[tauri::command]
pub fn save_collections(storage: State<Storage>, collections: Vec<Collection>) -> Result<(), String> {
    storage.save_collections(&collections).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_environments(storage: State<Storage>, environments: Vec<Environment>) -> Result<(), String> {
    storage.save_environments(&environments).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_history(storage: State<Storage>, history: Vec<HistoryEntry>) -> Result<(), String> {
    storage.save_history(&history).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn missing_vars(request: Request, env: Option<Environment>) -> Vec<String> {
    substitute_request(&request, &vars(&env)).1
}

#[tauri::command(rename_all = "snake_case")]
pub async fn send_request(
    pending: State<'_, Pending>,
    job_id: u64,
    request: Request,
    env: Option<Environment>,
    timeout_secs: u64,
) -> Result<ResponseDto, String> {
    let (req, _) = substitute_request(&request, &vars(&env));
    let (tx, rx) = watch::channel(false);
    pending.0.lock().unwrap().insert(job_id, tx);
    let result = execute(&req, Duration::from_secs(timeout_secs.max(1)), rx).await;
    pending.0.lock().unwrap().remove(&job_id);
    result
        .map(|r| ResponseDto {
            status: r.status,
            headers: r.headers,
            body: String::from_utf8_lossy(&r.body).into_owned(),
            duration_ms: r.duration_ms,
            size_bytes: r.size_bytes,
        })
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub fn cancel_request(pending: State<Pending>, job_id: u64) {
    if let Some(tx) = pending.0.lock().unwrap().get(&job_id) {
        let _ = tx.send(true);
    }
}

/// 导出单个集合到文件（Firebee 原生格式，带版本号便于以后迁移）
#[tauri::command]
pub fn export_collection(collection: Collection, path: String) -> Result<(), String> {
    let doc = serde_json::json!({ "firebee": 1, "collection": collection });
    let data = serde_json::to_vec_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(&path, data).map_err(|e| format!("Couldn't write {path}: {e}"))
}

#[derive(Serialize, Default)]
pub struct Imported {
    collection: Option<Collection>,
    environment: Option<Environment>,
}

/// 导入文件：Firebee 导出、Postman Collection v2.x、Postman Environment
#[tauri::command]
pub fn import_file(path: String) -> Result<Imported, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("Couldn't read {path}: {e}"))?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| format!("Not valid JSON: {e}"))?;
    if v.get("firebee").is_some() {
        let collection = serde_json::from_value(v["collection"].clone())
            .map_err(|e| format!("Not a Firebee collection file: {e}"))?;
        return Ok(Imported { collection: Some(collection), ..Default::default() });
    }
    if crate::core::postman::is_collection(&v) {
        return Ok(Imported { collection: Some(crate::core::postman::to_collection(&v)), ..Default::default() });
    }
    if crate::core::postman::is_environment(&v) {
        return Ok(Imported { environment: Some(crate::core::postman::to_environment(&v)), ..Default::default() });
    }
    Err("Unrecognised file — expected a Firebee export, a Postman collection (v2.x) or a Postman environment".into())
}

#[tauri::command]
pub fn import_curl(text: String) -> Result<Request, String> {
    crate::core::import::from_curl(&text)
}

/// kind: "curl" | "python"。先变量替换，再构造最终 URL。
#[tauri::command]
pub fn export_code(request: Request, env: Option<Environment>, kind: String) -> String {
    let (req, _) = substitute_request(&request, &vars(&env));
    let url = build_url(&req).unwrap_or_else(|_| req.url.clone());
    match kind.as_str() {
        "python" => to_python(&req, &url),
        _ => to_curl(&req, &url),
    }
}
