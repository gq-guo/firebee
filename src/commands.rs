//! Tauri commands：前端持有全部 UI 状态，后端只做存储 / HTTP / 变量替换 / 导出。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use serde::Serialize;
use tauri::{Emitter, State};
use tokio::sync::watch;

use crate::core::export::{effective_headers, to_curl, to_python};
use crate::core::http::{build_url, execute_streaming, StreamEvent};
use crate::core::models::{
    merge_inherited, Collection, Environment, HistoryEntry, Inherited, Request,
};
use crate::core::storage::Storage;
use crate::core::vars::substitute_request;

/// 进行中的请求：job_id → 取消信号
#[derive(Default)]
pub struct Pending(Mutex<HashMap<u64, watch::Sender<bool>>>);

/// 应用生命周期内共享的 Cookie 罐（登录后会话接口能连着调）；Clear 即换新罐
pub struct Cookies(pub Mutex<Arc<reqwest::cookie::Jar>>);
impl Default for Cookies {
    fn default() -> Self {
        Self(Mutex::new(Arc::new(reqwest::cookie::Jar::default())))
    }
}

#[derive(Serialize)]
pub struct AppData {
    collections: Vec<Collection>,
    environments: Vec<Environment>,
    history: Vec<HistoryEntry>,
}

/// ResponseMeta 的前端视图：文本 body 以字符串下发；图片或非 UTF-8 走 body_base64
#[derive(Serialize)]
pub struct ResponseDto {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    body_base64: Option<String>,
    duration_ms: u128,
    size_bytes: usize,
    redirects: Vec<String>,
}

fn vars(env: &Option<Environment>) -> HashMap<String, String> {
    env.as_ref().map(|e| e.var_map()).unwrap_or_default()
}

/// 内置动态变量清单（前端补全用，和 vars.rs 同一份来源，避免两头维护）
#[tauri::command]
pub fn dynamic_vars() -> &'static [(&'static str, &'static str)] {
    crate::core::vars::DYNAMIC_VARS
}

/// debug 构建（cargo tauri dev）才保留 WebView 默认右键菜单（Reload / Inspect Element）
#[tauri::command]
pub fn debug_build() -> bool {
    cfg!(debug_assertions)
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
pub fn save_collections(
    storage: State<Storage>,
    collections: Vec<Collection>,
) -> Result<(), String> {
    storage
        .save_collections(&collections)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_environments(
    storage: State<Storage>,
    environments: Vec<Environment>,
) -> Result<(), String> {
    storage
        .save_environments(&environments)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_history(storage: State<Storage>, history: Vec<HistoryEntry>) -> Result<(), String> {
    storage.save_history(&history).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn missing_vars(request: Request, env: Option<Environment>) -> Vec<String> {
    substitute_request(&request, &vars(&env)).1
}

/// 发送时的各种开关，单独一个结构：Tauri command 的参数表就是 IPC 契约，
/// 每加一个开关就多一个参数不好扩展（clippy 也会在第 8 个上拦下来）
#[derive(serde::Deserialize)]
pub struct SendOptions {
    pub timeout_secs: u64,
    #[serde(default)]
    pub inherited: Option<Vec<Inherited>>,
    #[serde(default)]
    pub follow_redirects: Option<bool>,
    #[serde(default)]
    pub insecure: bool,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn send_request(
    app: tauri::AppHandle,
    pending: State<'_, Pending>,
    cookies: State<'_, Cookies>,
    job_id: u64,
    request: Request,
    env: Option<Environment>,
    options: SendOptions,
) -> Result<ResponseDto, String> {
    let SendOptions {
        timeout_secs,
        inherited,
        follow_redirects,
        insecure,
    } = options;
    let request = merge_inherited(&request, &inherited.unwrap_or_default());
    let (req, _) = substitute_request(&request, &vars(&env));
    let (tx, rx) = watch::channel(false);
    pending.0.lock().unwrap().insert(job_id, tx);
    let jar = cookies.0.lock().unwrap().clone();
    // SSE / NDJSON：每个 chunk 立刻推给界面（事件 "stream"），完整响应仍走返回值
    let on_stream = Box::new(move |ev: StreamEvent| {
        let _ = match ev {
            StreamEvent::Start(status, headers) => app.emit(
                "stream",
                serde_json::json!({ "job_id": job_id, "kind": "start", "status": status, "headers": headers }),
            ),
            StreamEvent::Chunk(c) => app.emit(
                "stream",
                serde_json::json!({ "job_id": job_id, "kind": "chunk", "text": String::from_utf8_lossy(&c) }),
            ),
        };
    });
    let result = execute_streaming(
        &req,
        Duration::from_secs(timeout_secs.max(1)),
        rx,
        Some(jar),
        follow_redirects.unwrap_or(true),
        insecure,
        Some(on_stream),
    )
    .await;
    pending.0.lock().unwrap().remove(&job_id);
    result
        .map(|r| {
            let is_image = r
                .headers
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v.starts_with("image/"));
            let (body, body_base64) = match (is_image, String::from_utf8(r.body)) {
                (false, Ok(text)) => (text, None),
                (_, Ok(text)) => (
                    String::new(),
                    Some(base64::engine::general_purpose::STANDARD.encode(text.as_bytes())),
                ),
                (_, Err(e)) => (
                    String::new(),
                    Some(base64::engine::general_purpose::STANDARD.encode(e.as_bytes())),
                ),
            };
            ResponseDto {
                status: r.status,
                headers: r.headers,
                body,
                body_base64,
                duration_ms: r.duration_ms,
                size_bytes: r.size_bytes,
                redirects: r.redirects,
            }
        })
        .map_err(|e| e.to_string())
}

/// 最终实际会发出去的请求：变量替换、继承的 header/auth、默认 Content-Type、
/// 会话 Cookie 全部算完之后的样子。调"为什么 401 / 为什么没带上"用。
#[derive(Serialize)]
pub struct FinalRequest {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    cookies: Vec<String>,
    body: String,
}

#[tauri::command(rename_all = "snake_case")]
pub fn final_request(
    cookies: State<Cookies>,
    request: Request,
    env: Option<Environment>,
    inherited: Option<Vec<Inherited>>,
) -> Result<FinalRequest, String> {
    use crate::core::models::{Auth, BodyType};
    use reqwest::cookie::CookieStore;
    let request = merge_inherited(&request, &inherited.unwrap_or_default());
    let (req, _) = substitute_request(&request, &vars(&env));
    let url = build_url(&req).map_err(|e| e.to_string())?;
    let mut headers = effective_headers(&req);
    if let Auth::Basic { username, password } = &req.auth {
        let cred =
            base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
        headers.push(("Authorization".into(), format!("Basic {cred}")));
    }
    let parsed = reqwest::Url::parse(&url).map_err(|e| e.to_string())?;
    let cookies = cookies
        .0
        .lock()
        .unwrap()
        .cookies(&parsed)
        .map(|v| {
            v.to_str()
                .unwrap_or("")
                .split("; ")
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let on = |rows: &[crate::core::models::KeyValue]| {
        rows.iter()
            .filter(|f| f.enabled && !f.key.is_empty())
            .map(|f| format!("{}={}{}", f.key, if f.is_file { "@" } else { "" }, f.value))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let body = match req.body_type {
        BodyType::None => String::new(),
        BodyType::Json | BodyType::Text => req.body.clone(),
        BodyType::GraphQL => req.graphql_payload().unwrap_or_default(),
        BodyType::Form => {
            headers.push((
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            ));
            on(&req.form)
        }
        BodyType::Multipart => {
            headers.push((
                "Content-Type".into(),
                "multipart/form-data; boundary=…".into(),
            ));
            on(&req.form)
        }
        BodyType::Binary => format!("@{}", req.body.trim()),
    };
    Ok(FinalRequest {
        method: req.effective_method().as_str().into(),
        url,
        headers,
        cookies,
        body,
    })
}

#[tauri::command]
pub fn clear_cookies(cookies: State<Cookies>) {
    *cookies.0.lock().unwrap() = Arc::new(reqwest::cookie::Jar::default());
}

/// 把响应体保存到文件：文本直接写，二进制走 base64 解码
#[tauri::command(rename_all = "snake_case")]
pub fn save_file(
    path: String,
    text: Option<String>,
    base64_data: Option<String>,
) -> Result<(), String> {
    let bytes = match base64_data {
        Some(b) => base64::engine::general_purpose::STANDARD
            .decode(b)
            .map_err(|e| e.to_string())?,
        None => text.unwrap_or_default().into_bytes(),
    };
    std::fs::write(&path, bytes).map_err(|e| format!("Couldn't write {path}: {e}"))
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
    let v: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("Not valid JSON: {e}"))?;
    if v.get("firebee").is_some() {
        let mut collection: Collection = serde_json::from_value(v["collection"].clone())
            .map_err(|e| format!("Not a Firebee collection file: {e}"))?;
        // 换一套新 id：导进来的是副本，不是原件。同 id 会让前端把两份请求认成一份。
        collection.reid();
        return Ok(Imported {
            collection: Some(collection),
            ..Default::default()
        });
    }
    if crate::core::postman::is_collection(&v) {
        return Ok(Imported {
            collection: Some(crate::core::postman::to_collection(&v)),
            ..Default::default()
        });
    }
    if crate::core::postman::is_environment(&v) {
        return Ok(Imported {
            environment: Some(crate::core::postman::to_environment(&v)),
            ..Default::default()
        });
    }
    Err("Unrecognised file — expected a Firebee export, a Postman collection (v2.x) or a Postman environment".into())
}

#[tauri::command]
pub fn import_curl(text: String) -> Result<Request, String> {
    crate::core::import::from_curl(&text)
}

/// kind: "curl" | "python"。先变量替换，再构造最终 URL。
#[tauri::command]
pub fn export_code(
    request: Request,
    env: Option<Environment>,
    kind: String,
    inherited: Option<Vec<Inherited>>,
) -> String {
    let request = merge_inherited(&request, &inherited.unwrap_or_default());
    let (req, _) = substitute_request(&request, &vars(&env));
    let url = build_url(&req).unwrap_or_else(|_| req.url.clone());
    match kind.as_str() {
        "python" => to_python(&req, &url),
        _ => to_curl(&req, &url),
    }
}

/// 更新装好后重启生效
#[tauri::command]
pub fn restart_app(app: tauri::AppHandle) {
    app.restart();
}

/// 自动更新装不上（比如取消了管理员授权）时，打开 Release 页让用户手动下载
#[tauri::command]
pub fn open_releases() -> Result<(), String> {
    std::process::Command::new("open")
        .arg("https://github.com/gq-guo/firebee/releases/latest")
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}
