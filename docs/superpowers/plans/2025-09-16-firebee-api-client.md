# Firebee API Client 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现一个 Rust + egui 的桌面 API client（精简版 Postman）：HTTP 请求发送、集合管理、环境变量、认证、历史记录、curl/Python 导出。

**Architecture:** 分层架构——Core 层（models/storage/http/vars/export，不依赖 UI、可单测）+ UI 层（egui 三栏界面）。HTTP 请求在独立 tokio worker 线程执行，通过 `std::sync::mpsc` channel 与 UI 通信，UI 永不阻塞。存储为 JSON 文件（原子写入 + 损坏恢复）。

**Tech Stack:** eframe/egui 0.32、reqwest 0.12 (rustls)、tokio 1、serde/serde_json、egui_extras (syntect)、uuid、chrono、dirs、thiserror、tracing；dev: wiremock、tempfile。

**参考 Spec:** `docs/superpowers/specs/2025-09-16-firebee-api-client-design.md`

**注意:** Cargo 依赖版本号以 `cargo add` / crates.io 当时的最新兼容版本为准；egui API 若与文中示例有出入（如 `from_id_salt`），以编译器提示为准调整。

---

## 文件结构

```
Cargo.toml
src/
  main.rs              # 入口：日志初始化、eframe 启动、worker 启动
  core/
    mod.rs             # pub mod models; storage; vars; export; http;
    models.rs          # 全部数据模型（serde）
    storage.rs         # JSON 持久化：原子写入、损坏恢复、历史上限
    vars.rs            # {{var}} 替换：substitute / substitute_request
    export.rs          # to_curl / to_python
    http.rs            # build_url / execute（reqwest + 超时 + 取消 + 错误映射）
  worker.rs            # worker 线程：收 Job → execute → 回 JobResult
  ui/
    mod.rs             # pub mod app; sidebar; request_panel; response_panel; env_bar; export_dialog;
    app.rs             # FirebeeApp 状态与 eframe::App 实现
    sidebar.rs         # 集合树 + 历史记录（Action 队列模式）
    request_panel.rs   # 方法/URL/发送/导出 + Params/Headers/Body/Auth 编辑
    response_panel.rs  # 状态/耗时/大小 + Body(JSON高亮)/Headers
    env_bar.rs         # 顶栏环境切换、超时设置、环境管理窗口
    export_dialog.rs   # 导出代码展示窗口（一键复制）
```

---

### Task 1: 项目脚手架

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `src/core/mod.rs`、`src/ui/mod.rs`、`src/worker.rs`（空壳）

- [ ] **Step 1: 初始化项目并写 Cargo.toml**

```bash
cd /Users/gq.guo/Study/firebee
cargo init --name firebee
```

`Cargo.toml`（在生成基础上补全 dependencies）：

```toml
[package]
name = "firebee"
version = "0.1.0"
edition = "2021"

[dependencies]
eframe = { version = "0.32", default-features = false, features = ["default_fonts"] }
egui_extras = { version = "0.32", features = ["syntect"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time"] }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
dirs = "6"
thiserror = "2"
tracing = "0.1"
tracing-subscriber = "0.3"
tracing-appender = "0.2"

[dev-dependencies]
wiremock = "0.6"
tempfile = "3"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

- [ ] **Step 2: 写模块骨架**

`src/core/mod.rs`：

```rust
pub mod models;
pub mod storage;
pub mod vars;
pub mod export;
pub mod http;
```

先创建空文件：`src/core/models.rs`、`src/core/storage.rs`、`src/core/vars.rs`、`src/core/export.rs`、`src/core/http.rs`，内容暂为 `// TODO`；`src/ui/mod.rs` 暂为 `// TODO`；`src/worker.rs` 暂为 `// TODO`。

`src/main.rs` 暂时最小化（后续 Task 7 完善）：

```rust
mod core;
mod ui;
mod worker;

fn main() {
    println!("firebee scaffold ok");
}
```

- [ ] **Step 3: 验证编译**

Run: `cargo build`
Expected: 编译成功（允许 unused 警告）

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "chore: 项目脚手架与依赖"
```

---

### Task 2: core/models.rs — 数据模型

**Files:**
- Create: `src/core/models.rs`

- [ ] **Step 1: 写失败测试**（文件末尾 `#[cfg(test)]`）

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_as_str_roundtrip() {
        assert_eq!(HttpMethod::Get.as_str(), "GET");
        assert_eq!(HttpMethod::Patch.as_str(), "PATCH");
        assert_eq!(HttpMethod::ALL.len(), 7);
    }

    #[test]
    fn request_serde_roundtrip() {
        let mut r = Request::new("登录");
        r.url = "https://api.example.com/login".into();
        r.method = HttpMethod::Post;
        r.headers = vec![KeyValue::new("Content-Type", "application/json")];
        r.body_type = BodyType::Json;
        r.body = r#"{"u":"a"}"#.into();
        r.auth = Auth::Bearer { token: "t123".into() };
        let json = serde_json::to_string(&r).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "登录");
        assert_eq!(back.method, HttpMethod::Post);
        assert_eq!(back.auth, Auth::Bearer { token: "t123".into() });
    }

    #[test]
    fn environment_var_map_filters() {
        let env = Environment {
            id: uuid::Uuid::new_v4(),
            name: "dev".into(),
            variables: vec![
                KeyValue::new("base_url", "https://dev.api.com"),
                KeyValue { enabled: false, key: "skip".into(), value: "x".into() },
                KeyValue::new("", "no-key"),
            ],
        };
        let map = env.var_map();
        assert_eq!(map.get("base_url").unwrap(), "https://dev.api.com");
        assert!(!map.contains_key("skip"));
        assert!(!map.contains_key(""));
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test models`
Expected: FAIL（类型未定义，编译错误）

- [ ] **Step 3: 实现 models.rs**（测试代码保留在文件末尾）

```rust
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl HttpMethod {
    pub const ALL: [HttpMethod; 7] = [
        Self::Get,
        Self::Post,
        Self::Put,
        Self::Delete,
        Self::Patch,
        Self::Head,
        Self::Options,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Patch => "PATCH",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValue {
    pub enabled: bool,
    pub key: String,
    pub value: String,
}

impl KeyValue {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self { enabled: true, key: key.into(), value: value.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BodyType {
    None,
    Json,
    Text,
    Form,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Auth {
    None,
    Bearer { token: String },
    Basic { username: String, password: String },
    /// in_query=true 时放 query string，否则放 header
    ApiKey { key: String, value: String, in_query: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: Uuid,
    pub name: String,
    pub method: HttpMethod,
    pub url: String,
    pub params: Vec<KeyValue>,
    pub headers: Vec<KeyValue>,
    pub body_type: BodyType,
    pub body: String,
    pub form: Vec<KeyValue>,
    pub auth: Auth,
}

impl Request {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            method: HttpMethod::Get,
            url: String::new(),
            params: vec![],
            headers: vec![],
            body_type: BodyType::None,
            body: String::new(),
            form: vec![],
            auth: Auth::None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: Uuid,
    pub name: String,
    pub folders: Vec<Folder>,
    pub requests: Vec<Request>,
}

impl Folder {
    pub fn new(name: impl Into<String>) -> Self {
        Self { id: Uuid::new_v4(), name: name.into(), folders: vec![], requests: vec![] }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub id: Uuid,
    pub name: String,
    pub folders: Vec<Folder>,
    pub requests: Vec<Request>,
}

impl Collection {
    pub fn new(name: impl Into<String>) -> Self {
        Self { id: Uuid::new_v4(), name: name.into(), folders: vec![], requests: vec![] }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub id: Uuid,
    pub name: String,
    pub variables: Vec<KeyValue>,
}

impl Environment {
    pub fn var_map(&self) -> std::collections::HashMap<String, String> {
        self.variables
            .iter()
            .filter(|v| v.enabled && !v.key.is_empty())
            .map(|v| (v.key.clone(), v.value.clone()))
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseMeta {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub duration_ms: u128,
    pub size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub request: Request,
    pub status: Option<u16>,
    pub duration_ms: Option<u128>,
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test models`
Expected: PASS（3 passed）

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(core): 数据模型"
```

---

### Task 3: core/storage.rs — JSON 持久化

**Files:**
- Create: `src/core/storage.rs`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::Collection;

    #[test]
    fn collections_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Storage::new(tmp.path().to_path_buf());
        let cols = vec![Collection::new("我的集合")];
        s.save_collections(&cols).unwrap();
        let loaded = s.load_collections();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "我的集合");
    }

    #[test]
    fn missing_file_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Storage::new(tmp.path().to_path_buf());
        assert!(s.load_collections().is_empty());
    }

    #[test]
    fn corrupted_file_backed_up_and_defaulted() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("collections.json"), b"{broken").unwrap();
        let s = Storage::new(tmp.path().to_path_buf());
        assert!(s.load_collections().is_empty());
        assert!(tmp.path().join("collections.bak").exists());
        // 再次加载不 panic、不重复改名失败
        assert!(s.load_collections().is_empty());
    }

    #[test]
    fn history_trimmed_to_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Storage::new(tmp.path().to_path_buf());
        let entries: Vec<HistoryEntry> = (0..600)
            .map(|_| HistoryEntry {
                timestamp: chrono::Local::now(),
                request: crate::core::models::Request::new("r"),
                status: Some(200),
                duration_ms: Some(1),
            })
            .collect();
        s.save_history(&entries).unwrap();
        assert_eq!(s.load_history().len(), 500);
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test storage`
Expected: FAIL（Storage 未定义）

- [ ] **Step 3: 实现 storage.rs**

```rust
use std::fs;
use std::path::PathBuf;

use crate::core::models::{Collection, Environment, HistoryEntry};

pub const HISTORY_LIMIT: usize = 500;

pub struct Storage {
    dir: PathBuf,
}

impl Storage {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// 系统标准数据目录，如 macOS ~/Library/Application Support/firebee
    pub fn default_dir() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("firebee")
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn load<T: serde::de::DeserializeOwned + Default>(&self, name: &str) -> T {
        let path = self.path(name);
        let Ok(bytes) = fs::read(&path) else {
            return T::default();
        };
        match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("{name} 解析失败（{e}），备份为 .bak 并以空数据启动");
                let _ = fs::rename(&path, self.dir.join(name.replace(".json", ".bak")));
                T::default()
            }
        }
    }

    /// 原子写入：先写临时文件再 rename，避免半截文件
    fn save<T: serde::Serialize>(&self, name: &str, value: &T) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let tmp = self.path(&format!("{name}.tmp"));
        let data = serde_json::to_vec_pretty(value)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(&tmp, data)?;
        fs::rename(&tmp, self.path(name))?;
        Ok(())
    }

    pub fn load_collections(&self) -> Vec<Collection> {
        self.load("collections.json")
    }

    pub fn save_collections(&self, collections: &[Collection]) -> std::io::Result<()> {
        self.save("collections.json", &collections)
    }

    pub fn load_environments(&self) -> Vec<Environment> {
        self.load("environments.json")
    }

    pub fn save_environments(&self, envs: &[Environment]) -> std::io::Result<()> {
        self.save("environments.json", &envs)
    }

    pub fn load_history(&self) -> Vec<HistoryEntry> {
        self.load("history.json")
    }

    /// 只保留最后 HISTORY_LIMIT 条（FIFO 淘汰最旧的）
    pub fn save_history(&self, history: &[HistoryEntry]) -> std::io::Result<()> {
        let start = history.len().saturating_sub(HISTORY_LIMIT);
        self.save("history.json", &&history[start..])
    }
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test storage`
Expected: PASS（4 passed）

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(core): JSON 存储（原子写入+损坏恢复+历史上限）"
```

---

### Task 4: core/vars.rs — 变量替换

**Files:**
- Create: `src/core/vars.rs`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::{Auth, KeyValue, Request};
    use std::collections::HashMap;

    fn vars() -> HashMap<String, String> {
        HashMap::from([
            ("base_url".to_string(), "https://api.dev".to_string()),
            ("token".to_string(), "abc".to_string()),
        ])
    }

    #[test]
    fn substitutes_known_vars() {
        let r = substitute("{{base_url}}/users/{{uid}}", &vars());
        assert_eq!(r.output, "https://api.dev/users/{{uid}}");
        assert_eq!(r.missing, vec!["uid".to_string()]);
    }

    #[test]
    fn no_closing_brace_left_as_is() {
        let r = substitute("{{base_url", &vars());
        assert_eq!(r.output, "{{base_url");
        assert!(r.missing.is_empty());
    }

    #[test]
    fn missing_deduplicated_and_kept_in_output() {
        let r = substitute("{{a}}/{{a}}/{{ b }}", &vars());
        assert_eq!(r.output, "{{a}}/{{a}}/{{b}}");
        assert_eq!(r.missing, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn substitute_request_covers_all_fields() {
        let mut req = Request::new("t");
        req.url = "{{base_url}}/login".into();
        req.headers = vec![KeyValue::new("X-Env", "{{mode}}")];
        req.body = "{\"t\":\"{{token}}\"}".into();
        req.auth = Auth::Bearer { token: "{{token}}".into() };
        let (out, missing) = substitute_request(&req, &vars());
        assert_eq!(out.url, "https://api.dev/login");
        assert_eq!(out.headers[0].value, "{{mode}}");
        assert_eq!(out.body, "{\"t\":\"abc\"}");
        assert_eq!(out.auth, Auth::Bearer { token: "abc".into() });
        assert_eq!(missing, vec!["mode".to_string()]);
        // 原请求不被修改
        assert_eq!(req.url, "{{base_url}}/login");
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test vars`
Expected: FAIL

- [ ] **Step 3: 实现 vars.rs**

```rust
use std::collections::HashMap;

use crate::core::models::{Auth, Request};

pub struct SubstituteResult {
    pub output: String,
    pub missing: Vec<String>,
}

/// 把 {{name}} 替换为 vars[name]；未定义的变量原样保留并记入 missing（去重、按出现顺序）。
pub fn substitute(input: &str, vars: &HashMap<String, String>) -> SubstituteResult {
    let mut output = String::with_capacity(input.len());
    let mut missing: Vec<String> = Vec::new();
    let mut rest = input;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let name = after[..end].trim();
                match vars.get(name) {
                    Some(v) => output.push_str(v),
                    None => {
                        if !missing.iter().any(|m| m == name) {
                            missing.push(name.to_string());
                        }
                        output.push_str("{{");
                        output.push_str(name);
                        output.push_str("}}");
                    }
                }
                rest = &after[end + 2..];
            }
            None => {
                output.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    output.push_str(rest);
    SubstituteResult { output, missing }
}

/// 对请求的 URL / params / headers / form / body / auth 全部字段做替换，
/// 返回替换后的新请求（不修改原请求）与缺失变量列表。
pub fn substitute_request(req: &Request, vars: &HashMap<String, String>) -> (Request, Vec<String>) {
    let mut out = req.clone();
    let mut missing: Vec<String> = Vec::new();

    fn apply(s: &str, vars: &HashMap<String, String>, missing: &mut Vec<String>) -> String {
        let r = substitute(s, vars);
        for m in r.missing {
            if !missing.contains(&m) {
                missing.push(m);
            }
        }
        r.output
    }

    out.url = apply(&out.url, vars, &mut missing);
    for kv in out
        .params
        .iter_mut()
        .chain(out.headers.iter_mut())
        .chain(out.form.iter_mut())
    {
        kv.key = apply(&kv.key, vars, &mut missing);
        kv.value = apply(&kv.value, vars, &mut missing);
    }
    out.body = apply(&out.body, vars, &mut missing);
    match &mut out.auth {
        Auth::Bearer { token } => *token = apply(token, vars, &mut missing),
        Auth::Basic { username, password } => {
            *username = apply(username, vars, &mut missing);
            *password = apply(password, vars, &mut missing);
        }
        Auth::ApiKey { key, value, .. } => {
            *key = apply(key, vars, &mut missing);
            *value = apply(value, vars, &mut missing);
        }
        Auth::None => {}
    }
    (out, missing)
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test vars`
Expected: PASS（4 passed）

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(core): {{var}} 变量替换"
```

---

### Task 5: core/http.rs — 请求执行

**Files:**
- Create: `src/core/http.rs`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::{Auth, BodyType, KeyValue, Request};
    use std::time::Duration;
    use wiremock::matchers::{body_string, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn no_cancel() -> tokio::sync::watch::Receiver<bool> {
        let (_tx, rx) = tokio::sync::watch::channel(false);
        rx
    }

    #[tokio::test]
    async fn get_with_params_and_headers() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users"))
            .and(query_param("page", "1"))
            .and(header("x-token", "abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&server)
            .await;

        let mut req = Request::new("t");
        req.url = format!("{}/users", server.uri());
        req.params = vec![KeyValue::new("page", "1")];
        req.headers = vec![KeyValue::new("x-token", "abc")];
        let resp = execute(&req, Duration::from_secs(5), no_cancel()).await.unwrap();
        assert_eq!(resp.status, 200);
        assert!(String::from_utf8(resp.body).unwrap().contains("\"ok\":true"));
        assert!(resp.size_bytes > 0);
    }

    #[tokio::test]
    async fn post_json_body_and_bearer() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/login"))
            .and(header("authorization", "Bearer t123"))
            .and(header("content-type", "application/json"))
            .and(body_string(r#"{"u":"a"}"#))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;

        let mut req = Request::new("t");
        req.method = crate::core::models::HttpMethod::Post;
        req.url = format!("{}/login", server.uri());
        req.body_type = BodyType::Json;
        req.body = r#"{"u":"a"}"#.into();
        req.auth = Auth::Bearer { token: "t123".into() };
        let resp = execute(&req, Duration::from_secs(5), no_cancel()).await.unwrap();
        assert_eq!(resp.status, 201);
    }

    #[tokio::test]
    async fn api_key_in_query() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/data"))
            .and(query_param("api_key", "k9"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let mut req = Request::new("t");
        req.url = format!("{}/data", server.uri());
        req.auth = Auth::ApiKey { key: "api_key".into(), value: "k9".into(), in_query: true };
        let resp = execute(&req, Duration::from_secs(5), no_cancel()).await.unwrap();
        assert_eq!(resp.status, 200);
    }

    #[tokio::test]
    async fn timeout_maps_to_timeout_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
            .mount(&server)
            .await;

        let mut req = Request::new("t");
        req.url = server.uri();
        let err = execute(&req, Duration::from_millis(100), no_cancel()).await.unwrap_err();
        assert!(matches!(err, HttpError::Timeout(_)));
    }

    #[tokio::test]
    async fn cancelled_before_send() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
            .mount(&server)
            .await;

        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut req = Request::new("t");
        req.url = server.uri();
        let handle = tokio::spawn(async move { execute(&req, Duration::from_secs(10), rx).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.send(true).unwrap();
        let err = handle.await.unwrap().unwrap_err();
        assert!(matches!(err, HttpError::Cancelled));
    }

    #[tokio::test]
    async fn invalid_url_error() {
        let mut req = Request::new("t");
        req.url = "not a url".into();
        let err = execute(&req, Duration::from_secs(5), no_cancel()).await.unwrap_err();
        assert!(matches!(err, HttpError::InvalidUrl(_)));
    }

    #[tokio::test]
    async fn connection_refused_is_network_error() {
        let mut req = Request::new("t");
        req.url = "http://127.0.0.1:1/".into();
        let err = execute(&req, Duration::from_secs(2), no_cancel()).await.unwrap_err();
        assert!(matches!(err, HttpError::Network(_)));
    }

    #[test]
    fn build_url_appends_enabled_params_only() {
        let mut req = Request::new("t");
        req.url = "https://api.dev/users".into();
        req.params = vec![
            KeyValue::new("a", "1"),
            KeyValue { enabled: false, key: "b".into(), value: "2".into() },
            KeyValue::new("", "skip"),
        ];
        assert_eq!(build_url(&req).unwrap(), "https://api.dev/users?a=1");
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test http`
Expected: FAIL（函数未定义）

- [ ] **Step 3: 实现 http.rs**

```rust
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::core::models::{Auth, BodyType, Request, ResponseMeta};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("请求超时（{0:?}）")]
    Timeout(Duration),
    #[error("请求已取消")]
    Cancelled,
    #[error("无效的 URL：{0}")]
    InvalidUrl(String),
    #[error("网络错误：{0}")]
    Network(String),
}

/// 由请求的 url + 启用的 params + query 型 ApiKey 构造最终 URL。
pub fn build_url(req: &Request) -> Result<String, HttpError> {
    let mut url =
        reqwest::Url::parse(&req.url).map_err(|_| HttpError::InvalidUrl(req.url.clone()))?;
    {
        let mut qp = url.query_pairs_mut();
        for p in req.params.iter().filter(|p| p.enabled && !p.key.is_empty()) {
            qp.append_pair(&p.key, &p.value);
        }
        if let Auth::ApiKey { key, value, in_query: true } = &req.auth {
            if !key.is_empty() {
                qp.append_pair(key, value);
            }
        }
    }
    Ok(url.to_string())
}

/// 执行 HTTP 请求。取消方式：向 cancel watch channel 发送 true（底层 future 被 drop，请求中断）。
pub async fn execute(
    req: &Request,
    timeout: Duration,
    cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<ResponseMeta, HttpError> {
    let url = build_url(req)?;
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| HttpError::Network(e.to_string()))?;
    let method = reqwest::Method::from_bytes(req.method.as_str().as_bytes())
        .map_err(|_| HttpError::InvalidUrl(req.method.as_str().to_string()))?;

    let mut builder = client.request(method, url);
    for h in req.headers.iter().filter(|h| h.enabled && !h.key.is_empty()) {
        builder = builder.header(&h.key, &h.value);
    }
    match &req.auth {
        Auth::Bearer { token } => builder = builder.bearer_auth(token),
        Auth::Basic { username, password } => {
            builder = builder.basic_auth(username, Some(password))
        }
        Auth::ApiKey { key, value, in_query: false } => {
            if !key.is_empty() {
                builder = builder.header(key, value);
            }
        }
        _ => {}
    }
    match &req.body_type {
        BodyType::Json => {
            builder = builder
                .header("Content-Type", "application/json")
                .body(req.body.clone());
        }
        BodyType::Text => {
            if !req.body.is_empty() {
                builder = builder.body(req.body.clone());
            }
        }
        BodyType::Form => {
            let form: Vec<(String, String)> = req
                .form
                .iter()
                .filter(|f| f.enabled && !f.key.is_empty())
                .map(|f| (f.key.clone(), f.value.clone()))
                .collect();
            builder = builder.form(&form);
        }
        BodyType::None => {}
    }

    let start = Instant::now();
    let send = builder.send();
    tokio::pin!(send);
    let resp = tokio::select! {
        r = &mut send => r.map_err(|e| map_reqwest_err(&e, timeout))?,
        _ = wait_cancelled(cancel) => return Err(HttpError::Cancelled),
    };
    let status = resp.status().as_u16();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| HttpError::Network(e.to_string()))?;
    let duration_ms = start.elapsed().as_millis();
    Ok(ResponseMeta {
        status,
        headers,
        size_bytes: bytes.len(),
        body: bytes.to_vec(),
        duration_ms,
    })
}

async fn wait_cancelled(mut rx: tokio::sync::watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            // sender 已 drop（不可能取消），永不返回，让 select 走 send 分支
            std::future::pending::<()>().await;
        }
    }
}

fn map_reqwest_err(e: &reqwest::Error, timeout: Duration) -> HttpError {
    if e.is_timeout() {
        HttpError::Timeout(timeout)
    } else if e.is_connect() {
        HttpError::Network("无法连接到服务器（连接被拒绝或 DNS 失败）".to_string())
    } else {
        HttpError::Network(e.to_string())
    }
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test http`
Expected: PASS（8 passed）

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(core): HTTP 执行（reqwest + 超时 + 取消 + 错误映射）"
```

---

### Task 6: core/export.rs — curl / Python 导出

约定：导出函数接收 `req` 与**最终 URL**（调用方先 `substitute_request` 再 `build_url`）。

**Files:**
- Create: `src/core/export.rs`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::{Auth, BodyType, HttpMethod, KeyValue, Request};

    fn sample() -> Request {
        let mut r = Request::new("登录");
        r.method = HttpMethod::Post;
        r.url = "https://api.dev/login".into();
        r.headers = vec![KeyValue::new("X-App", "firebee")];
        r.body_type = BodyType::Json;
        r.body = r#"{"u":"a","p":"b's"}"#.into();
        r.auth = Auth::Bearer { token: "t123".into() };
        r
    }

    #[test]
    fn curl_contains_all_parts() {
        let c = to_curl(&sample(), "https://api.dev/login");
        assert!(c.contains("curl -X POST"), "{c}");
        assert!(c.contains("'https://api.dev/login'"), "{c}");
        assert!(c.contains("-H 'X-App: firebee'"), "{c}");
        assert!(c.contains("-H 'Authorization: Bearer t123'"), "{c}");
        assert!(c.contains("Content-Type: application/json"), "{c}");
        assert!(c.contains(r#"-d '{"u":"a","p":"b'\\''s"}'"#), "{c}");
    }

    #[test]
    fn curl_basic_auth() {
        let mut r = Request::new("t");
        r.url = "https://api.dev/x".into();
        r.auth = Auth::Basic { username: "u".into(), password: "p".into() };
        let c = to_curl(&r, "https://api.dev/x");
        assert!(c.contains("-u 'u:p'"), "{c}");
        assert!(!c.contains("Authorization"), "{c}");
    }

    #[test]
    fn curl_form() {
        let mut r = Request::new("t");
        r.method = HttpMethod::Post;
        r.body_type = BodyType::Form;
        r.form = vec![KeyValue::new("name", "张三")];
        let c = to_curl(&r, "https://api.dev/f");
        assert!(c.contains("--data-urlencode 'name=张三'"), "{c}");
    }

    #[test]
    fn python_contains_all_parts() {
        let p = to_python(&sample(), "https://api.dev/login");
        assert!(p.contains("import requests"), "{p}");
        assert!(p.contains(r#""POST""#), "{p}");
        assert!(p.contains(r#""https://api.dev/login""#), "{p}");
        assert!(p.contains(r#""X-App": "firebee""#), "{p}");
        assert!(p.contains(r#""Authorization": "Bearer t123""#), "{p}");
        assert!(p.contains("print(response.status_code)"), "{p}");
    }

    #[test]
    fn python_form_uses_data_dict() {
        let mut r = Request::new("t");
        r.method = HttpMethod::Post;
        r.body_type = BodyType::Form;
        r.form = vec![KeyValue::new("name", "张三")];
        let p = to_python(&r, "https://api.dev/f");
        assert!(p.contains(r#""name": "张三""#), "{p}");
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test export`
Expected: FAIL

- [ ] **Step 3: 实现 export.rs**

```rust
use crate::core::models::{Auth, BodyType, Request};

/// shell 单引号转义：' → '\''
fn sh(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Rust Debug 字符串字面量 ≈ Python 双引号字面量（MVP 足够）
fn py(s: &str) -> String {
    format!("{s:?}")
}

/// 收集最终生效的 header（含认证与 Content-Type），供 curl/python 共用。
fn effective_headers(req: &Request) -> Vec<(String, String)> {
    let mut hs: Vec<(String, String)> = req
        .headers
        .iter()
        .filter(|h| h.enabled && !h.key.is_empty())
        .map(|h| (h.key.clone(), h.value.clone()))
        .collect();
    match &req.auth {
        Auth::Bearer { token } => {
            hs.push(("Authorization".into(), format!("Bearer {token}")))
        }
        Auth::ApiKey { key, value, in_query: false } => {
            if !key.is_empty() {
                hs.push((key.clone(), value.clone()));
            }
        }
        _ => {}
    }
    if req.body_type == BodyType::Json && !req.body.is_empty() {
        hs.push(("Content-Type".into(), "application/json".into()));
    }
    hs
}

pub fn to_curl(req: &Request, url: &str) -> String {
    let mut parts = vec![format!("curl -X {}", req.method.as_str()), sh(url)];
    match &req.auth {
        Auth::Basic { username, password } => {
            parts.push(format!("-u {}", sh(&format!("{username}:{password}"))));
        }
        _ => {}
    }
    for (k, v) in effective_headers(req) {
        parts.push(format!("-H {}", sh(&format!("{k}: {v}"))));
    }
    match &req.body_type {
        BodyType::Json | BodyType::Text => {
            if !req.body.is_empty() {
                parts.push(format!("-d {}", sh(&req.body)));
            }
        }
        BodyType::Form => {
            for f in req.form.iter().filter(|f| f.enabled && !f.key.is_empty()) {
                parts.push(format!("--data-urlencode {}", sh(&format!("{}={}", f.key, f.value))));
            }
        }
        BodyType::None => {}
    }
    parts.join(" \\\n  ")
}

pub fn to_python(req: &Request, url: &str) -> String {
    let mut s = String::from("import requests\n\n");
    s.push_str("response = requests.request(\n");
    s.push_str(&format!("    {},\n    {},\n", py(req.method.as_str()), py(url)));
    let hs = effective_headers(req);
    if !hs.is_empty() {
        s.push_str("    headers={\n");
        for (k, v) in hs {
            s.push_str(&format!("        {}: {},\n", py(&k), py(&v)));
        }
        s.push_str("    },\n");
    }
    if let Auth::Basic { username, password } = &req.auth {
        s.push_str(&format!("    auth=({}, {}),\n", py(username), py(password)));
    }
    match &req.body_type {
        BodyType::Json | BodyType::Text => {
            if !req.body.is_empty() {
                s.push_str(&format!("    data={},\n", py(&req.body)));
            }
        }
        BodyType::Form => {
            s.push_str("    data={\n");
            for f in req.form.iter().filter(|f| f.enabled && !f.key.is_empty()) {
                s.push_str(&format!("        {}: {},\n", py(&f.key), py(&f.value)));
            }
            s.push_str("    },\n");
        }
        BodyType::None => {}
    }
    s.push_str(")\n\nprint(response.status_code)\nprint(response.text)\n");
    s
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test export`
Expected: PASS（5 passed）

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(core): curl / Python 导出"
```

---

### Task 7: worker 线程 + main.rs + app 骨架（可运行的空窗口）

Core 层完成。本任务打通 UI↔worker 的 channel，先跑出一个能显示的空壳窗口。

**Files:**
- Create: `src/worker.rs`
- Modify: `src/main.rs`
- Create: `src/ui/app.rs`
- Modify: `src/ui/mod.rs`

- [ ] **Step 1: 实现 worker.rs**

```rust
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use crate::core::http::{execute, HttpError};
use crate::core::models::{Request, ResponseMeta};

pub struct Job {
    pub id: u64,
    pub request: Request,
    pub timeout: Duration,
    pub cancel: tokio::sync::watch::Receiver<bool>,
}

pub struct JobResult {
    pub id: u64,
    pub result: Result<ResponseMeta, HttpError>,
}

/// 专用线程：持有 tokio runtime，每个 Job spawn 一个异步任务（支持并发与取消）。
pub fn spawn_worker(job_rx: Receiver<Job>, result_tx: Sender<JobResult>) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");
        while let Ok(job) = job_rx.recv() {
            let tx = result_tx.clone();
            rt.spawn(async move {
                let result = execute(&job.request, job.timeout, job.cancel).await;
                let _ = tx.send(JobResult { id: job.id, result });
            });
        }
    });
}
```

- [ ] **Step 2: 实现 app.rs 骨架**（本任务先含状态与收发逻辑；面板函数在后续任务填充，先留最小占位）

```rust
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
            ui.heading("Firebee");
            ui.label("骨架已运行，面板待实现");
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.storage.save_collections(&self.collections);
        let _ = self.storage.save_environments(&self.environments);
        let _ = self.storage.save_history(&self.history);
    }
}
```

`src/ui/mod.rs`：

```rust
pub mod app;
pub mod env_bar;
pub mod export_dialog;
pub mod request_panel;
pub mod response_panel;
pub mod sidebar;
```

同时创建 4 个占位文件（`env_bar.rs`、`export_dialog.rs`、`request_panel.rs`、`response_panel.rs`、`sidebar.rs`），内容暂为 `// TODO`（后续任务填充并从 mod 引用；若想先编译通过，mod.rs 里暂时只保留 `pub mod app;`，随任务逐个取消注释）。

- [ ] **Step 3: 实现 main.rs**

```rust
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
```

- [ ] **Step 4: 验证**

Run: `cargo build && cargo test`
Expected: 编译成功，既有测试全 PASS

Run: `cargo run`
Expected: 弹出窗口，显示 "Firebee / 骨架已运行"；Ctrl+C 或关窗退出码 0

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: worker 线程、channel 通信与应用骨架"
```

---

### Task 8: 请求面板（request_panel.rs）

**Files:**
- Modify: `src/ui/request_panel.rs`
- Modify: `src/ui/app.rs`（CentralPanel 调用 request_panel）

- [ ] **Step 1: 实现 request_panel.rs**

```rust
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
    if ui.button("+ 添加").clicked() {
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
                .hint_text("https://api.example.com/path，支持 {{变量}}")
                .desired_width(ui.available_width() - 220.0),
        );
        if app.pending.is_some() {
            if ui.button("取消").clicked() {
                app.cancel();
            }
            ui.spinner();
        } else if ui.button("发送").clicked() {
            app.send();
        }
        ui.menu_button("导出 ▾", |ui| {
            if ui.button("curl 命令").clicked() {
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
            format!("未定义变量：{}（再次点击「发送」将忽略并原样发送）", app.missing_vars.join(", ")),
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
            dirty = kv_table(ui, "params", &mut app.current.params, "参数名", "值");
        }
        ReqTab::Headers => {
            dirty = kv_table(ui, "headers", &mut app.current.headers, "Header", "值");
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
                    dirty = kv_table(ui, "form", &mut app.current.form, "字段名", "值");
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
        for (v, label) in [(0, "无"), (1, "Bearer Token"), (2, "Basic Auth"), (3, "API Key")] {
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
                ui.label("用户名:");
                changed |= ui.text_edit_singleline(username).changed();
                ui.label("密码:");
                changed |= ui.add(egui::TextEdit::singleline(password).password(true)).changed();
            });
        }
        Auth::ApiKey { key, value, in_query } => {
            ui.horizontal(|ui| {
                ui.label("Key:");
                changed |= ui.text_edit_singleline(key).changed();
                ui.label("Value:");
                changed |= ui.text_edit_singleline(value).changed();
                changed |= ui.checkbox(in_query, "放在 Query 参数").changed();
            });
        }
        Auth::None => {}
    }
    changed
}
```

- [ ] **Step 2: app.rs 的 update 接入请求面板**

把 `impl eframe::App for FirebeeApp` 中 `update` 的 CentralPanel 改为：

```rust
egui::CentralPanel::default().show(ctx, |ui| {
    crate::ui::request_panel::show(ui, self);
    ui.separator();
    // response_panel 下个任务接入
});
```

- [ ] **Step 3: 验证**

Run: `cargo clippy && cargo test`
Expected: 无错误（警告尽量清零），测试全 PASS

Run: `cargo run` 手动冒烟：能切方法、输入 URL、编辑 Params/Headers/Body/Auth；点导出弹窗暂存（export_dialog 在 Task 11 实现——本任务先把 `export_dialog.rs` 占位为：）

```rust
// TODO: Task 11 实现窗口；以下为临时实现
pub enum Kind { Curl, Python }

pub fn render(app: &mut crate::ui::app::FirebeeApp, kind: Kind) -> String {
    let (req, _) = crate::core::vars::substitute_request(&app.current, &app.env_vars());
    let url = crate::core::http::build_url(&req).unwrap_or_else(|_| req.url.clone());
    match kind {
        Kind::Curl => crate::core::export::to_curl(&req, &url),
        Kind::Python => crate::core::export::to_python(&req, &url),
    }
}
```

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat(ui): 请求面板（方法/URL/Params/Headers/Body/Auth）"
```

---

### Task 9: 响应面板（response_panel.rs）

**Files:**
- Modify: `src/ui/response_panel.rs`
- Modify: `src/ui/app.rs`（接入）

- [ ] **Step 1: 实现 response_panel.rs**

```rust
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
    let theme = egui_extras::syntax_highlighting::CodeTheme::from_memory(ui.ctx(), &ui.style());
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        let mut job = egui_extras::syntax_highlighting::highlight(
            ui.ctx(),
            &ui.style(),
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

fn show_response(ui: &mut egui::Ui, app: &mut FirebeeApp, resp: &ResponseMeta) {
    ui.horizontal(|ui| {
        ui.colored_label(status_color(resp.status), format!("{}", resp.status));
        ui.label(format!("{} ms", resp.duration_ms));
        ui.label(fmt_size(resp.size_bytes));
        if ui.button("复制 Body").clicked() {
            ui.ctx().copy_text(String::from_utf8_lossy(&resp.body).to_string());
        }
    });
    ui.horizontal(|ui| {
        ui.selectable_value(&mut app.resp_tab, RespTab::Body, "Body");
        ui.selectable_value(&mut app.resp_tab, RespTab::Headers, "Headers");
    });
    ui.separator();
    match app.resp_tab {
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
    ui.heading("响应");
    match &app.response {
        None => {
            if app.pending.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("请求中…");
                });
            } else {
                ui.weak("尚未发送请求");
            }
        }
        Some(Err(e)) => {
            ui.colored_label(egui::Color32::from_rgb(220, 60, 60), e);
        }
        Some(Ok(resp)) => show_response(ui, app, resp),
    }
}
```

- [ ] **Step 2: app.rs 接入**

`update` 的 CentralPanel 末尾加：

```rust
crate::ui::response_panel::show(ui, self);
```

- [ ] **Step 3: 验证**

Run: `cargo clippy && cargo test && cargo run`
Expected: 编译干净；手动冒烟：向 `https://httpbin.org/get` 发 GET，能看到状态码/耗时/大小与 JSON 高亮 Body；断网后发请求，红色错误提示显示在面板中。

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat(ui): 响应面板（状态/耗时/大小/JSON 高亮/Headers）"
```

---

### Task 10: 侧边栏集合树（sidebar.rs）

模式：UI 不产生任何直接变更，只把操作推入 `Vec<Action>`，渲染结束后统一应用，避开 borrow 冲突。

**Files:**
- Modify: `src/ui/sidebar.rs`
- Modify: `src/ui/app.rs`（接入 + 增加 renaming 状态）

- [ ] **Step 1: app.rs 增加重命名状态**

`FirebeeApp` 结构体加两个字段（并在 `new` 初始化）：

```rust
    pub renaming: Option<TreePath>,
    pub rename_buf: String,
```

`new` 中：`renaming: None, rename_buf: String::new(),`。`TreePath` 定义在 sidebar.rs（见下），app.rs 顶部 `use crate::ui::sidebar::TreePath;`。

- [ ] **Step 2: 实现 sidebar.rs**

```rust
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
    NewFolder(Vec<usize>),       // 路径：col + folders
    NewRequest(Vec<usize>),
    Delete(TreePath),
    StartRename(TreePath),
    CommitRename(TreePath, String),
    Select(TreePath),
    LoadHistory(usize),
}

fn folder_slot<'a>(col: &'a mut Collection, path: &[usize]) -> Option<&'a mut Folder> {
    let mut cur: Option<&mut Folder> = None;
    for (depth, &idx) in path.iter().enumerate() {
        let list: &mut Vec<Folder> = match cur {
            None => &mut col.folders,
            Some(f) => &mut f.folders,
        };
        cur = Some(list.get_mut(idx)?);
        let _ = depth;
    }
    cur
}

fn apply(app: &mut FirebeeApp, actions: Vec<Action>) {
    for a in actions {
        match a {
            Action::NewCollection => {
                app.collections
                    .push(Collection::new(format!("集合 {}", app.collections.len() + 1)));
                app.mark_dirty();
            }
            Action::NewFolder(mut path) => {
                let col_idx = path.remove(0);
                if let Some(col) = app.collections.get_mut(col_idx) {
                    let slot: &mut Vec<Folder> = match folder_slot(col, &path) {
                        Some(f) => &mut f.folders,
                        None if path.is_empty() => &mut col.folders,
                        None => continue,
                    };
                    slot.push(Folder::new("新文件夹"));
                    app.mark_dirty();
                }
            }
            Action::NewRequest(mut path) => {
                let col_idx = path.remove(0);
                if let Some(col) = app.collections.get_mut(col_idx) {
                    let slot: &mut Vec<Request> = match folder_slot(col, &path) {
                        Some(f) => &mut f.requests,
                        None if path.is_empty() => &mut col.requests,
                        None => continue,
                    };
                    slot.push(Request::new("新请求"));
                    app.mark_dirty();
                }
            }
            Action::Delete(p) => {
                if let Some(col) = app.collections.get_mut(p.col) {
                    if let Some(ri) = p.req {
                        let slot = match folder_slot(col, &p.folders) {
                            Some(f) => &mut f.requests,
                            None if p.folders.is_empty() => &mut col.requests,
                            None => continue,
                        };
                        if ri < slot.len() {
                            slot.remove(ri);
                            app.mark_dirty();
                        }
                    } else if let Some(&last) = p.folders.last() {
                        let parent = &p.folders[..p.folders.len() - 1];
                        let slot = match folder_slot(col, parent) {
                            Some(f) => &mut f.folders,
                            None if parent.is_empty() => &mut col.folders,
                            None => continue,
                        };
                        if last < slot.len() {
                            slot.remove(last);
                            app.mark_dirty();
                        }
                    } else {
                        app.collections.remove(p.col);
                        app.mark_dirty();
                    }
                }
            }
            Action::StartRename(p) => {
                app.rename_buf = name_at(app, &p).unwrap_or_default();
                app.renaming = Some(p);
            }
            Action::CommitRename(p, name) => {
                if !name.trim().is_empty() {
                    if let Some(col) = app.collections.get_mut(p.col) {
                        if let Some(ri) = p.req {
                            let slot = match folder_slot(col, &p.folders) {
                                Some(f) => &mut f.requests,
                                None if p.folders.is_empty() => &mut col.requests,
                                None => continue,
                            };
                            if let Some(r) = slot.get_mut(ri) {
                                r.name = name.trim().to_string();
                                app.mark_dirty();
                            }
                        } else if let Some(&last) = p.folders.last() {
                            let parent = &p.folders[..p.folders.len() - 1];
                            let slot = match folder_slot(col, parent) {
                                Some(f) => &mut f.folders,
                                None if parent.is_empty() => &mut col.folders,
                                None => continue,
                            };
                            if let Some(f) = slot.get_mut(last) {
                                f.name = name.trim().to_string();
                                app.mark_dirty();
                            }
                        } else {
                            col.name = name.trim().to_string();
                            app.mark_dirty();
                        }
                    }
                }
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

fn name_at(app: &FirebeeApp, p: &TreePath) -> Option<String> {
    let col = app.collections.get(p.col)?;
    if let Some(ri) = p.req {
        let mut folders = &col.folders;
        for &i in &p.folders {
            folders = &folders.get(i)?.folders;
            // 注意：requests 取法在下方统一处理
        }
        let mut f_ref: Option<&Folder> = None;
        let mut list = &col.folders;
        for &i in &p.folders {
            f_ref = list.get(i);
            list = &f_ref?.folders;
        }
        let reqs: &Vec<Request> = match f_ref {
            Some(f) => &f.requests,
            None => &col.requests,
        };
        let _ = folders;
        reqs.get(ri).map(|r| r.name.clone())
    } else if p.folders.is_empty() {
        Some(col.name.clone())
    } else {
        let mut list = &col.folders;
        let mut f_ref: Option<&Folder> = None;
        for &i in &p.folders {
            f_ref = list.get(i);
            list = &f_ref?.folders;
        }
        f_ref.map(|f| f.name.clone())
    }
}

fn request_at(app: &FirebeeApp, p: &TreePath) -> Option<&Request> {
    let ri = p.req?;
    let col = app.collections.get(p.col)?;
    let mut list = &col.folders;
    let mut f_ref: Option<&Folder> = None;
    for &i in &p.folders {
        f_ref = list.get(i);
        list = &f_ref?.folders;
    }
    match f_ref {
        Some(f) => f.requests.get(ri),
        None => col.requests.get(ri),
    }
}

fn request_row(ui: &mut egui::Ui, path: TreePath, name: &str, renaming: bool, rename_buf: &mut String, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        if renaming {
            let resp = ui.text_edit_singleline(rename_buf);
            resp.request_focus();
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                actions.push(Action::CommitRename(path.clone(), rename_buf.clone()));
            } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                actions.push(Action::CommitRename(path.clone(), String::new()));
            }
        } else {
            let resp = ui.button(format!("• {name}"));
            if resp.clicked() {
                actions.push(Action::Select(path.clone()));
            }
            resp.context_menu(|ui| {
                if ui.button("重命名").clicked() {
                    actions.push(Action::StartRename(path.clone()));
                    ui.close();
                }
                if ui.button("删除").clicked() {
                    actions.push(Action::Delete(path.clone()));
                    ui.close();
                }
            });
        }
    });
}

fn folder_ui(ui: &mut egui::Ui, app: &FirebeeApp, path: &mut Vec<usize>, folder: &Folder, actions: &mut Vec<Action>) {
    let idx = *path.last().unwrap();
    let my_path = TreePath { col: path[0], folders: path[1..].to_vec(), req: None };
    let renaming = app.renaming.as_ref() == Some(&my_path);
    let header = egui::CollapsingHeader::new(if renaming { "✏️" } else { &folder.name })
        .id_salt(("folder", folder.id))
        .default_open(true);
    header.show(ui, |ui| {
        if renaming {
            let mut buf = app.rename_buf.clone();
            if ui.text_edit_singleline(&mut buf).lost_focus() {
                actions.push(Action::CommitRename(my_path.clone(), buf));
            }
        }
        ui.horizontal(|ui| {
            if ui.small_button("＋请求").clicked() {
                actions.push(Action::NewRequest(my_path.folders.clone().into_iter().fold(vec![my_path.col], |mut v, x| { v.push(x); v })));
            }
            if ui.small_button("＋文件夹").clicked() {
                let mut p = vec![my_path.col];
                p.extend(&my_path.folders);
                actions.push(Action::NewFolder(p));
            }
            if ui.small_button("重命名").clicked() {
                actions.push(Action::StartRename(my_path.clone()));
            }
            if ui.small_button("🗑").clicked() {
                actions.push(Action::Delete(my_path.clone()));
            }
            let _ = idx;
        });
        for (i, sub) in folder.folders.iter().enumerate() {
            path.push(i);
            folder_ui(ui, app, path, sub, actions);
            path.pop();
        }
        for (i, r) in folder.requests.iter().enumerate() {
            let rp = TreePath { col: my_path.col, folders: my_path.folders.clone(), req: Some(i) };
            let renaming = app.renaming.as_ref() == Some(&rp);
            let mut buf = app.rename_buf.clone();
            request_row(ui, rp, &r.name, renaming, &mut buf, actions);
        }
    });
}

pub fn show(ctx: &egui::Context, app: &mut FirebeeApp) {
    let mut actions = Vec::new();
    egui::SidePanel::left("sidebar")
        .default_width(240.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.selectable_label(matches!(app.sidebar_tab, SidebarTab::Collections), "集合").clicked() {
                    app.sidebar_tab = SidebarTab::Collections;
                }
                if ui.selectable_label(matches!(app.sidebar_tab, SidebarTab::History), "历史记录").clicked() {
                    app.sidebar_tab = SidebarTab::History;
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                match app.sidebar_tab {
                    SidebarTab::Collections => {
                        if ui.button("＋ 新建集合").clicked() {
                            actions.push(Action::NewCollection);
                        }
                        for (ci, col) in app.collections.iter().enumerate() {
                            let col_path = TreePath { col: ci, folders: vec![], req: None };
                            let renaming = app.renaming.as_ref() == Some(&col_path);
                            egui::CollapsingHeader::new(if renaming { "✏️" } else { &col.name })
                                .id_salt(("col", col.id))
                                .default_open(true)
                                .show(ui, |ui| {
                                    if renaming {
                                        let mut buf = app.rename_buf.clone();
                                        if ui.text_edit_singleline(&mut buf).lost_focus() {
                                            actions.push(Action::CommitRename(col_path.clone(), buf));
                                        }
                                    }
                                    ui.horizontal(|ui| {
                                        if ui.small_button("＋请求").clicked() {
                                            actions.push(Action::NewRequest(vec![ci]));
                                        }
                                        if ui.small_button("＋文件夹").clicked() {
                                            actions.push(Action::NewFolder(vec![ci]));
                                        }
                                        if ui.small_button("重命名").clicked() {
                                            actions.push(Action::StartRename(col_path.clone()));
                                        }
                                        if ui.small_button("🗑").clicked() {
                                            actions.push(Action::Delete(col_path.clone()));
                                        }
                                    });
                                    let mut path = vec![ci];
                                    for (i, f) in col.folders.iter().enumerate() {
                                        path.push(i);
                                        folder_ui(ui, app, &mut path, f, &mut actions);
                                        path.pop();
                                    }
                                    for (i, r) in col.requests.iter().enumerate() {
                                        let rp = TreePath { col: ci, folders: vec![], req: Some(i) };
                                        let renaming = app.renaming.as_ref() == Some(&rp);
                                        let mut buf = app.rename_buf.clone();
                                        request_row(ui, rp, &r.name, renaming, &mut buf, &mut actions);
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
        format!("{}…", s.chars().take(max).collect::<String>())
    } else {
        s.to_string()
    }
}
```

- [ ] **Step 3: app.rs 接入**

`update` 中在 CentralPanel 之前加：

```rust
crate::ui::sidebar::show(ctx, self);
```

注意 `NewFolder` / `NewRequest` 的 Action payload 约定：`vec![col_idx, folder_idx...]`（首元素为集合下标），`apply` 中 `path.remove(0)` 取出集合下标。

- [ ] **Step 4: 验证**

Run: `cargo clippy && cargo test && cargo run`
Expected: 编译干净；手动冒烟：新建集合 → 新建文件夹 → 新建请求 → 点击请求载入编辑器 → 右键重命名/删除 → 发送一次请求后历史记录出现条目且可点击回填；重启应用数据仍在。

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(ui): 侧边栏集合树与历史记录"
```

---

### Task 11: 环境栏与导出窗口（env_bar.rs + export_dialog.rs）

**Files:**
- Modify: `src/ui/env_bar.rs`
- Modify: `src/ui/export_dialog.rs`
- Modify: `src/ui/app.rs`（接入）

- [ ] **Step 1: 实现 env_bar.rs**

```rust
use std::time::Instant;

use egui;

use crate::core::models::Environment;
use crate::ui::app::FirebeeApp;
use crate::ui::request_panel::kv_table;

pub fn show(ctx: &egui::Context, app: &mut FirebeeApp) {
    egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("环境:");
            let current = app
                .active_env
                .and_then(|id| app.environments.iter().find(|e| e.id == id))
                .map(|e| e.name.clone())
                .unwrap_or_else(|| "无环境".to_string());
            egui::ComboBox::from_id_salt("env")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(app.active_env.is_none(), "无环境").clicked() {
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
            if ui.button("⚙ 管理环境").clicked() {
                app.show_env_window = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label("超时(s)");
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
    egui::Window::new("环境管理")
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
                    if ui.button("＋ 新环境").clicked() {
                        app.environments.push(Environment {
                            id: uuid::Uuid::new_v4(),
                            name: format!("环境 {}", app.environments.len() + 1),
                            variables: vec![],
                        });
                        app.env_sel = Some(app.environments.len() - 1);
                        app.dirty_since = Some(Instant::now());
                    }
                    if let Some(i) = app.env_sel {
                        if i < app.environments.len() && ui.button("🗑 删除").clicked() {
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
                        ui.label("变量（在请求中用 {{变量名}} 引用）：");
                        changed |= kv_table(ui, "env_vars", &mut env.variables, "变量名", "值");
                        changed
                    };
                    if changed {
                        app.dirty_since = Some(Instant::now());
                    }
                } else {
                    ui.weak("← 选择或新建一个环境");
                }
            });
        });
    if !open {
        app.show_env_window = false;
    }
}
```

- [ ] **Step 2: 实现 export_dialog.rs**（替换 Task 8 的临时版，保留 `render` 并新增窗口）

```rust
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
    egui::Window::new("导出")
        .open(&mut open)
        .default_size([560.0, 360.0])
        .show(ctx, |ui| {
            if ui.button("📋 复制").clicked() {
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
```

- [ ] **Step 3: app.rs 接入**

`update` 中（sidebar 之后、CentralPanel 之前）：

```rust
crate::ui::env_bar::show(ctx, self);
crate::ui::export_dialog::show(ctx, self);
```

- [ ] **Step 4: 验证**

Run: `cargo clippy && cargo test && cargo run`
Expected: 编译干净；手动冒烟：新建环境 "dev"，加变量 `base_url=https://httpbin.org`；顶栏切到 dev；URL 输入 `{{base_url}}/get` 发送成功；URL 输入 `{{nope}}/x` 发送时出现黄色未定义变量提示，再点一次强制发送并得到网络错误；导出的 curl 含最终 URL。

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(ui): 环境管理与导出窗口"
```

---

### Task 12: 收尾 — 全量验证与冒烟清单

**Files:**
- Modify: 视验证结果而定
- Create: `README.md`

- [ ] **Step 1: 全量测试与静态检查**

Run: `cargo test && cargo clippy -- -D warnings`
Expected: 全部 PASS，clippy 无警告（若有，修复后 commit）

- [ ] **Step 2: release 构建与性能验证**

Run: `cargo build --release && ls -lh target/release/firebee && time target/release/firebee`
Expected: 启动到窗口可见 < 1s；二进制约 10–20MB

- [ ] **Step 3: 手动冒烟清单**（逐项人工确认）

- [ ] 发送 GET/POST（JSON body）到 httpbin.org，状态/耗时/大小/Body 高亮正常
- [ ] Bearer / Basic / API Key（header 与 query 两种）生效（用 httpbin.org/headers、/basic-auth 验证）
- [ ] 请求中点「取消」，显示"请求已取消"
- [ ] 超时设为 1s 请求慢接口，显示超时错误
- [ ] 集合：新建/嵌套文件夹/请求、点击载入、右键重命名与删除、重启后仍在
- [ ] 环境：新建/删除/切换，变量替换生效，未定义变量二次确认
- [ ] 历史：发送多条后出现，点击回填，超过 500 条截断（可跳过实测，由单测覆盖）
- [ ] 导出：curl 与 Python 复制到剪贴板，粘贴到终端可执行（curl 实测）
- [ ] 破坏 collections.json（手写乱码），重启后正常启动且生成 .bak
- [ ] 日志文件存在于数据目录

- [ ] **Step 4: 写 README.md**

```markdown
# Firebee

用 Rust + egui 编写的轻量桌面 API client（精简版 Postman）。设计目标：快、稳定、克制。

## 功能

- HTTP 请求：全部常用方法，Params / Headers / Body（JSON、Text、Form）
- 响应展示：JSON 语法高亮、状态码、耗时、大小、响应头
- 集合管理：文件夹嵌套、右键重命名/删除，JSON 文件持久化
- 环境变量：多环境切换，`{{variable}}` 替换
- 认证：Bearer Token、Basic Auth、API Key（Header / Query）
- 历史记录：最近 500 条，点击回填
- 导出：curl 命令、Python (requests) 代码

## 运行

cargo run --release

## 测试

cargo test

## 数据位置

macOS: ~/Library/Application Support/firebee/（collections.json / environments.json / history.json / 日志）
```

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "docs: README 与收尾"
```

---

## Self-Review 记录

- Spec 覆盖：HTTP 请求(T5/T8)、响应展示(T9)、集合(T10)、环境变量(T4/T11)、认证(T5/T8)、历史(T7/T10)、导出(T6/T8/T11)、超时/取消(T5/T7/T8/T11)、存储健壮性(T3)、日志(T7)、测试策略(T2–T6 单测 + T12 冒烟)——均有对应任务。
- 类型一致性：`KeyValue.enabled/key/value`、`Auth::{None,Bearer,Basic,ApiKey}`、`BodyType`、`TreePath{col,folders,req}`、`Job{id,request,timeout,cancel}`、`JobResult{id,result}`、`FirebeeApp` 公共字段在 UI 各模块间一致。
- 已知取舍：egui 版本 API 名称以编译器提示为准；Python 导出用 Rust Debug 字面量近似 Python 字符串（MVP 可接受）；`name_at` 函数若 clippy 报冗余可精简，以编译为准。
