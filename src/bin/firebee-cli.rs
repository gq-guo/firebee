//! `firebee-cli`：GUI 和终端共用同一份请求定义。
//!
//!   firebee-cli list   [--project DIR | --collection NAME]
//!   firebee-cli send   <request name> [--project DIR | --collection NAME] [--env NAME | --env-file F] [--var k=v]... [--json]
//!   firebee-cli run    [--project DIR | --collection NAME] [--folder NAME] [--env NAME | --env-file F] [--var k=v]... [--json]
//!
//! 不带 --project 时读 Firebee app 自己的数据目录（collections.json / environments.json）。
//! run：按集合里的顺序依次发送，Capture 的值传给后面的请求；非 2xx/3xx 算失败，退出码 1。
//! ponytail: 参数解析手写，不上 clap —— 三条子命令、六个开关，够了。

use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use firebee_lib::core::cookies::Cookies;
use firebee_lib::core::http::{execute_streaming, Net};
use firebee_lib::core::jsonpath;
use firebee_lib::core::models::{
    merge_inherited, Collection, Environment, Folder, Inherited, Request,
};
use firebee_lib::core::storage::Storage;
use firebee_lib::core::vars::substitute_request;
use firebee_lib::core::{postman, project};

struct Opts {
    cmd: String,
    positional: Vec<String>,
    project: Option<String>,
    collection: Option<String>,
    folder: Option<String>,
    env: Option<String>,
    env_file: Option<String>,
    vars: Vec<(String, String)>,
    json: bool,
    timeout: u64,
    insecure: bool,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        cmd: args.first().cloned().unwrap_or_default(),
        positional: vec![],
        project: None,
        collection: None,
        folder: None,
        env: None,
        env_file: None,
        vars: vec![],
        json: false,
        timeout: 30,
        insecure: false,
    };
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--project" | "-p" => o.project = Some(val()?),
            "--collection" | "-c" => o.collection = Some(val()?),
            "--folder" | "-f" => o.folder = Some(val()?),
            "--env" | "-e" => o.env = Some(val()?),
            "--env-file" => o.env_file = Some(val()?),
            "--var" | "-v" => {
                let v = val()?;
                let (k, v) = v.split_once('=').ok_or("--var needs k=v")?;
                o.vars.push((k.into(), v.into()));
            }
            "--json" => o.json = true,
            "--insecure" | "-k" => o.insecure = true,
            "--timeout" => o.timeout = val()?.parse().map_err(|_| "--timeout needs seconds")?,
            s if s.starts_with('-') => return Err(format!("Unknown option {s}")),
            s => o.positional.push(s.into()),
        }
    }
    Ok(o)
}

const USAGE: &str = "usage:
  firebee-cli list [--project DIR | --collection NAME]
  firebee-cli send <request> [--project DIR | --collection NAME] [--env NAME | --env-file FILE] [--var k=v]... [--json] [-k] [--timeout S]
  firebee-cli run  [--project DIR | --collection NAME] [--folder NAME] [--env NAME | --env-file FILE] [--var k=v]... [--json] [-k] [--timeout S]

Without --project, requests come from the Firebee app's own data (same as the GUI).
run sends requests in order; values captured by one request are available to the next.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = match parse(&args) {
        Ok(o) if matches!(o.cmd.as_str(), "list" | "send" | "run") => o,
        Ok(_) => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let rt = tokio::runtime::Runtime::new().expect("tokio");
    match rt.block_on(run(opts)) {
        Ok(ok) => {
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

/// 一条可发送的请求：请求本身 + 它所在的继承链 + 显示用的路径
struct Item {
    path: String,
    req: Request,
    chain: Vec<Inherited>,
}

fn flatten(c: &Collection) -> Vec<Item> {
    fn walk(
        f_name: &str,
        folders: &[Folder],
        requests: &[Request],
        chain: &[Inherited],
        out: &mut Vec<Item>,
    ) {
        for r in requests {
            out.push(Item {
                path: if f_name.is_empty() {
                    r.name.clone()
                } else {
                    format!("{f_name}/{}", r.name)
                },
                req: r.clone(),
                chain: chain.to_vec(),
            });
        }
        for f in folders {
            let mut c = chain.to_vec();
            c.push(Inherited {
                headers: f.headers.clone(),
                auth: f.auth.clone(),
            });
            let name = if f_name.is_empty() {
                f.name.clone()
            } else {
                format!("{f_name}/{}", f.name)
            };
            walk(&name, &f.folders, &f.requests, &c, out);
        }
    }
    let root = vec![Inherited {
        headers: c.headers.clone(),
        auth: c.auth.clone(),
    }];
    let mut out = vec![];
    walk("", &c.folders, &c.requests, &root, &mut out);
    out
}

fn load_collection(o: &Opts, storage: &Storage) -> Result<Collection, String> {
    if let Some(dir) = &o.project {
        let p = Path::new(dir);
        if !project::is_project_dir(p) {
            return Err(format!("{dir}: no {} here", project::MANIFEST));
        }
        return project::read(p).map_err(|e| e.to_string());
    }
    let all = storage.load_collections();
    match (&o.collection, all.len()) {
        (Some(name), _) => all
            .into_iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("No collection named “{name}” in Firebee")),
        (None, 1) => Ok(all.into_iter().next().unwrap()),
        (None, 0) => Err("Firebee has no collections yet; use --project DIR".into()),
        (None, _) => Err(format!(
            "Several collections; pick one with --collection: {}",
            all.iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn load_vars(o: &Opts, storage: &Storage) -> Result<HashMap<String, String>, String> {
    let mut vars = if let Some(f) = &o.env_file {
        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(f).map_err(|e| format!("{f}: {e}"))?)
                .map_err(|e| format!("{f}: {e}"))?;
        let env: Environment = if postman::is_environment(&v) {
            postman::to_environment(&v)
        } else {
            serde_json::from_value(v)
                .map_err(|e| format!("{f}: not a Firebee or Postman environment: {e}"))?
        };
        env.var_map()
    } else if let Some(name) = &o.env {
        storage
            .load_environments()
            .into_iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("No environment named “{name}” in Firebee"))?
            .var_map()
    } else {
        HashMap::new()
    };
    for (k, v) in &o.vars {
        vars.insert(k.clone(), v.clone());
    }
    Ok(vars)
}

async fn run(o: Opts) -> Result<bool, String> {
    let storage = Storage::new(Storage::default_dir());
    let c = load_collection(&o, &storage)?;
    let items = flatten(&c);
    if o.cmd == "list" {
        for it in &items {
            println!("{:7} {}", it.req.effective_method().as_str(), it.path);
        }
        return Ok(true);
    }
    let mut vars = load_vars(&o, &storage)?;
    let net = Net {
        insecure: o.insecure,
        ..Net::default()
    };
    let jar: Arc<dyn reqwest::cookie::CookieStore> = Arc::new(Cookies::default());
    let timeout = Duration::from_secs(o.timeout.max(1));

    let selected: Vec<&Item> = match o.cmd.as_str() {
        "send" => {
            let name = o
                .positional
                .first()
                .ok_or("send needs a request name (see `list`)")?;
            let found = items
                .iter()
                .find(|i| {
                    i.path.eq_ignore_ascii_case(name) || i.req.name.eq_ignore_ascii_case(name)
                })
                .ok_or_else(|| format!("No request named “{name}” (see `list`)"))?;
            vec![found]
        }
        _ => items
            .iter()
            .filter(|i| {
                o.folder.as_ref().is_none_or(|f| {
                    i.path
                        .to_lowercase()
                        .starts_with(&format!("{}/", f.to_lowercase()))
                })
            })
            .collect(),
    };
    if selected.is_empty() {
        return Err("Nothing to run".into());
    }

    let mut all_ok = true;
    let mut results = vec![];
    for it in selected {
        let merged = merge_inherited(&it.req, &it.chain);
        let (req, missing) = substitute_request(&merged, &vars);
        if !missing.is_empty() {
            eprintln!(
                "warning: {}: unresolved {{{{{}}}}}",
                it.path,
                missing.join("}}, {{")
            );
        }
        let (tx, rx) = tokio::sync::watch::channel(false);
        drop(tx);
        let t0 = std::time::Instant::now();
        let r = execute_streaming(&req, timeout, rx, Some(jar.clone()), &net, None).await;
        match r {
            Ok(resp) => {
                let body = String::from_utf8_lossy(&resp.body).into_owned();
                // 有断言按断言算；没有就看状态码
                let outcomes = firebee_lib::core::assert::check_all(
                    &it.req.asserts,
                    &firebee_lib::core::assert::Resp {
                        status: resp.status,
                        headers: &resp.headers,
                        body: &body,
                        duration_ms: resp.duration_ms,
                    },
                );
                let ok = if outcomes.is_empty() {
                    resp.status < 400
                } else {
                    outcomes.iter().all(|o| o.ok)
                };
                all_ok &= ok;
                // Capture → 后面的请求能用
                let mut captured = vec![];
                if resp.status < 300 {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                        for c in it
                            .req
                            .captures
                            .iter()
                            .filter(|c| c.enabled && !c.key.trim().is_empty())
                        {
                            if let Ok(Some(v)) = jsonpath::capture(&json, &c.value) {
                                vars.insert(c.key.trim().to_string(), v);
                                captured.push(c.key.trim().to_string());
                            }
                        }
                    }
                }
                if o.json {
                    results.push(serde_json::json!({
                        "request": it.path, "method": req.effective_method().as_str(), "url": firebee_lib::core::http::build_url(&req).unwrap_or_default(),
                        "status": resp.status, "duration_ms": resp.duration_ms, "ttfb_ms": resp.ttfb_ms, "size_bytes": resp.size_bytes,
                        "headers": resp.headers, "body": body, "captured": captured, "ok": ok, "asserts": outcomes,
                    }));
                } else if o.cmd == "send" {
                    eprintln!(
                        "{} {} · {} ms · {} B{}",
                        resp.status,
                        it.path,
                        resp.duration_ms,
                        resp.size_bytes,
                        if captured.is_empty() {
                            String::new()
                        } else {
                            format!(" · captured {}", captured.join(", "))
                        }
                    );
                    println!("{body}");
                } else {
                    println!(
                        "{} {:>4} {:>6} ms  {}{}",
                        if ok { "PASS" } else { "FAIL" },
                        resp.status,
                        resp.duration_ms,
                        it.path,
                        if captured.is_empty() {
                            String::new()
                        } else {
                            format!("  (captured {})", captured.join(", "))
                        }
                    );
                }
            }
            Err(e) => {
                all_ok = false;
                if o.json {
                    results.push(serde_json::json!({ "request": it.path, "error": e.to_string(), "duration_ms": t0.elapsed().as_millis(), "ok": false }));
                } else {
                    println!(
                        "FAIL  err {:>6} ms  {}  {e}",
                        t0.elapsed().as_millis(),
                        it.path
                    );
                }
            }
        }
    }
    if o.json {
        let out = if o.cmd == "send" {
            results.pop().unwrap_or_default()
        } else {
            serde_json::Value::Array(results)
        };
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    }
    Ok(all_ok)
}
