//! Insomnia v5 导出（YAML / JSON，`type: collection.insomnia.rest/5.x` 或 `spec.insomnia.rest/5.x`）导入。
//! 只转我们支持的字段，其余丢弃（meta / settings / cookieJar / spec / 脚本）。

use serde_json::Value;

use crate::core::models::{
    Auth, BodyType, Collection, Environment, Folder, HttpMethod, KeyValue, Request,
};

pub fn is_export(v: &Value) -> bool {
    v.get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| t.contains("insomnia.rest/5"))
}

/// Insomnia 的变量写法 `{{ _.name }}` → `{{name}}`；别的模板原样留着
fn vars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let Some(j) = after.find("}}") else {
            out.push_str(&rest[i..]);
            return out;
        };
        match after[..j].trim().strip_prefix("_.") {
            Some(name) => {
                out.push_str("{{");
                out.push_str(name);
                out.push_str("}}");
            }
            None => out.push_str(&rest[i..i + 2 + j + 2]),
        }
        rest = &after[j + 2..];
    }
    out.push_str(rest);
    out
}

fn s(v: &Value) -> String {
    match v {
        Value::String(x) => vars(x),
        Value::Null => String::new(),
        other => vars(&other.to_string()),
    }
}

/// [{name, value, disabled}] → Vec<KeyValue>
fn kvs(v: Option<&Value>) -> Vec<KeyValue> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|x| x.get("name").is_some())
                .map(|x| KeyValue {
                    enabled: !x["disabled"].as_bool().unwrap_or(false),
                    key: s(&x["name"]),
                    value: s(&x["value"]),
                    is_file: false,
                    secret: false,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn auth(v: Option<&Value>) -> Auth {
    let Some(v) = v else {
        return Auth::None;
    };
    let g = |k: &str| s(&v[k]);
    // Insomnia 里关掉的认证 = 无效，继续往父级找（不是"明确不带"）
    if v["disabled"].as_bool().unwrap_or(false) {
        return Auth::None;
    }
    match v["type"].as_str().unwrap_or("") {
        "" | "inherit" => Auth::None,
        "none" => Auth::Off,
        // prefix 默认 Bearer；NO_PREFIX 是裸 token；别的前缀（Token / JWT …）只能按原样写进 Authorization 头
        "bearer" => match g("prefix").as_str() {
            "" | "Bearer" => Auth::Bearer { token: g("token") },
            prefix => Auth::ApiKey {
                key: "Authorization".into(),
                value: if prefix == "NO_PREFIX" {
                    g("token")
                } else {
                    format!("{prefix} {}", g("token"))
                },
                in_query: false,
            },
        },
        "basic" => Auth::Basic {
            username: g("username"),
            password: g("password"),
        },
        "apikey" => Auth::ApiKey {
            key: g("key"),
            value: g("value"),
            in_query: g("addTo") == "queryParams",
        },
        "oauth2" if g("grantType") == "client_credentials" => Auth::OAuth2 {
            token_url: g("accessTokenUrl"),
            client_id: g("clientId"),
            client_secret: g("clientSecret"),
            scope: g("scope"),
        },
        // oauth1 / digest / ntlm / 要浏览器回调的 oauth2：不支持。按"不带凭据"而不是继承——
        // 把文件夹的 bearer 发给一个本该走 oauth1 的接口，比不发更糟
        _ => Auth::Off,
    }
}

fn request(v: &Value) -> Request {
    let mut r = Request::new(s(&v["name"]));
    let m = s(&v["method"]).to_ascii_uppercase();
    r.method = HttpMethod::ALL
        .into_iter()
        .find(|x| x.as_str() == m)
        .unwrap_or(HttpMethod::Get);
    r.url = s(&v["url"]);
    // /users/:id + pathParameters [{name: id, value}] → 直接替换进 URL。
    // 长名字先替换，免得 :id 吃掉 :ids 的前缀
    let mut path_params = kvs(v.get("pathParameters"));
    path_params.sort_by_key(|p| std::cmp::Reverse(p.key.len()));
    for p in path_params.iter().filter(|p| !p.key.is_empty()) {
        r.url = r.url.replace(&format!(":{}", p.key), &p.value);
    }
    r.params = kvs(v.get("parameters"));
    r.headers = kvs(v.get("headers"));
    r.auth = auth(v.get("authentication"));
    let b = &v["body"];
    let strip_content_type = |r: &mut Request| {
        r.headers
            .retain(|h| !h.key.eq_ignore_ascii_case("content-type"))
    };
    match s(&b["mimeType"]).as_str() {
        // text 是 {"query": "...", "variables": {...}} 的 JSON 文本
        "application/graphql" => {
            r.body_type = BodyType::GraphQL;
            r.method = HttpMethod::Post;
            strip_content_type(&mut r);
            let text = s(&b["text"]);
            match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(o)) => {
                    r.body = o.get("query").map(s).unwrap_or_default();
                    r.graphql_variables = match o.get("variables") {
                        None | Some(Value::Null) => String::new(),
                        Some(Value::String(x)) => vars(x),
                        Some(x) => x.to_string(),
                    };
                }
                _ => r.body = text,
            }
        }
        "application/x-www-form-urlencoded" => {
            r.body_type = BodyType::Form;
            r.form = kvs(b.get("params"));
        }
        "multipart/form-data" => {
            r.body_type = BodyType::Multipart;
            r.form = kvs(b.get("params"));
            if let Some(a) = b["params"].as_array() {
                for (f, x) in r
                    .form
                    .iter_mut()
                    .zip(a.iter().filter(|x| x.get("name").is_some()))
                {
                    if x["type"] == "file" {
                        // 别人集合里的本机路径不能一点 Send 就读：先关掉，用户看过再开
                        f.is_file = true;
                        f.enabled = false;
                        f.value = s(&x["fileName"]);
                    }
                }
            }
        }
        "application/octet-stream" => {
            r.body_type = BodyType::Binary;
            r.body = s(&b["fileName"]);
        }
        mime => {
            if let Some(t) = b.get("text") {
                r.body = s(t);
                if mime.contains("json") || serde_json::from_str::<Value>(&r.body).is_ok() {
                    r.body_type = BodyType::Json;
                    strip_content_type(&mut r);
                } else {
                    r.body_type = BodyType::Text;
                }
            }
        }
    }
    r
}

fn walk(items: &[Value], folders: &mut Vec<Folder>, requests: &mut Vec<Request>) {
    for it in items {
        // 空文件夹导出时没有 children 字段，只剩 name / meta
        let is_folder = it.get("url").is_none() && it.get("method").is_none();
        if is_folder {
            let children = it["children"].as_array().cloned().unwrap_or_default();
            let mut f = Folder::new(s(&it["name"]));
            f.headers = kvs(it.get("headers"));
            walk(&children, &mut f.folders, &mut f.requests);
            match auth(it.get("authentication")) {
                // Insomnia 里文件夹设成 No Auth 会挡住更外层的认证；Firebee 的容器放不了 Off，
                // 就把下面所有自己没设认证的请求标成 Off（有自己认证的子文件夹不碰）
                Auth::Off => block_inherit(&mut f),
                a => f.auth = a,
            }
            folders.push(f);
        } else if it.get("method").is_some() {
            requests.push(request(it));
        }
        // 只有 url 没有 method 的是 gRPC / WebSocket / Socket.IO，不是 HTTP，跳过
    }
}

fn block_inherit(f: &mut Folder) {
    for r in f.requests.iter_mut().filter(|r| r.auth == Auth::None) {
        r.auth = Auth::Off;
    }
    for sub in f.folders.iter_mut().filter(|s| s.auth == Auth::None) {
        block_inherit(sub);
    }
}

pub fn to_collection(v: &Value) -> Collection {
    let mut c = Collection::new(s(&v["name"]));
    if c.name.is_empty() {
        c.name = "Imported collection".into();
    }
    let items = v["collection"].as_array().cloned().unwrap_or_default();
    walk(&items, &mut c.folders, &mut c.requests);
    c
}

/// Base environment 自己算一个；每个 subEnvironment 合并 base（同名以 sub 为准）后各算一个
pub fn to_environments(v: &Value) -> Vec<Environment> {
    let Some(e) = v.get("environments").filter(|e| e.is_object()) else {
        return vec![];
    };
    let base = e["data"].as_object().cloned().unwrap_or_default();
    let env = |name: &Value, data: &serde_json::Map<String, Value>| Environment {
        id: uuid::Uuid::new_v4(),
        name: match s(name) {
            n if n.is_empty() => "Imported environment".into(),
            n => n,
        },
        variables: data
            .iter()
            .map(|(k, val)| KeyValue::new(k.clone(), s(val)))
            .collect(),
    };
    let mut out = vec![env(&e["name"], &base)];
    for sub in e["subEnvironments"].as_array().into_iter().flatten() {
        let mut d = base.clone();
        if let Some(o) = sub["data"].as_object() {
            d.extend(o.iter().map(|(k, val)| (k.clone(), val.clone())));
        }
        out.push(env(&sub["name"], &d));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
type: collection.insomnia.rest/5.0
name: Shop
collection:
  - name: Users
    headers: [{name: X-F, value: f}]
    authentication: {type: bearer, token: "{{ _.tok }}"}
    children:
      - name: Get user
        url: "{{ _.base }}/users/:id/:ids"
        method: GET
        pathParameters: [{name: id, value: "{{ _.uid }}"}, {name: ids, value: all}]
        parameters: [{name: a, value: "1"}, {name: b, value: 2026-01-01, disabled: true}]
        headers: [{name: H, value: "{{_.tok}}", disabled: true}, {name: Content-Type, value: application/json}]
        authentication: {type: basic, username: u, password: p}
        body: {mimeType: application/json, text: '{"id": "{{ _.id }}"}'}
      - name: disabled auth inherits
        url: https://x/d
        method: GET
        authentication: {type: basic, username: u, password: p, disabled: true}
      - name: prefixed
        url: https://x/p
        method: GET
        authentication: {type: bearer, token: abc, prefix: Token}
      - name: ws
        url: wss://x/ws
  - name: No auth folder
    authentication: {type: none}
    children:
      - name: blocked
        url: https://x/n
        method: GET
      - name: own auth
        authentication: {type: bearer, token: t}
        children:
          - name: keeps inherit
            url: https://x/o
            method: GET
  - name: gql
    url: https://x/g
    method: GET
    body: {mimeType: application/graphql, text: '{"query":"{ a }","variables":{"n":1}}'}
  - name: form
    url: https://x/f
    method: POST
    authentication: {type: none}
    body:
      mimeType: multipart/form-data
      params: [{name: t, value: v, type: text}, {name: f, fileName: /tmp/a, type: file}]
  - name: empty folder
    meta: {id: fld_1}
  - name: bin
    url: https://x/b
    method: PUT
    authentication: {type: oauth1, consumerKey: k}
    body: {mimeType: application/octet-stream, fileName: /tmp/b.csv}
environments:
  name: Base
  data: {base: https://b, tok: t0}
  subEnvironments:
    - name: dev
      data: {tok: t1, nested: {a: "{{ _.base }}/v1"}}
"#;

    fn parse() -> Value {
        serde_yaml::from_str(SAMPLE).unwrap()
    }

    #[test]
    fn detects_and_converts_tree_vars_auth_and_bodies() {
        let v = parse();
        assert!(is_export(&v));
        let c = to_collection(&v);
        assert_eq!(c.name, "Shop");
        let f = &c.folders[0];
        assert_eq!(f.headers, vec![KeyValue::new("X-F", "f")]);
        assert_eq!(
            f.auth,
            Auth::Bearer {
                token: "{{tok}}".into()
            }
        );
        let r = &f.requests[0];
        assert_eq!(r.url, "{{base}}/users/{{uid}}/all");
        assert_eq!(r.params[0], KeyValue::new("a", "1"));
        assert_eq!(r.params[1].value, "2026-01-01");
        assert!(!r.params[1].enabled);
        // 变量名两边没空格也认；JSON body 去掉 Content-Type
        assert_eq!(r.headers.len(), 1);
        assert_eq!(r.headers[0].value, "{{tok}}");
        assert!(!r.headers[0].enabled);
        assert_eq!(r.body_type, BodyType::Json);
        assert_eq!(r.body, r#"{"id": "{{id}}"}"#);
        assert_eq!(
            r.auth,
            Auth::Basic {
                username: "u".into(),
                password: "p".into()
            }
        );

        assert_eq!(f.requests[1].auth, Auth::None, "关掉的认证 = 继承");
        assert_eq!(
            f.requests[2].auth,
            Auth::ApiKey {
                key: "Authorization".into(),
                value: "Token abc".into(),
                in_query: false
            }
        );
        assert_eq!(f.requests.len(), 3, "没有 method 的 WebSocket 项跳过");
        let nf = &c.folders[1];
        assert_eq!(nf.auth, Auth::None);
        assert_eq!(nf.requests[0].auth, Auth::Off, "文件夹 No Auth 挡住外层");
        assert_eq!(
            nf.folders[0].requests[0].auth,
            Auth::None,
            "子文件夹有自己的认证，不碰"
        );

        let g = &c.requests[0];
        assert_eq!(g.body_type, BodyType::GraphQL);
        assert_eq!(g.method, HttpMethod::Post);
        assert_eq!(g.body, "{ a }");
        assert_eq!(g.graphql_variables, r#"{"n":1}"#);

        let fm = &c.requests[1];
        assert_eq!(fm.auth, Auth::Off);
        assert_eq!(fm.body_type, BodyType::Multipart);
        assert_eq!(fm.form[0], KeyValue::new("t", "v"));
        assert!(fm.form[1].is_file && !fm.form[1].enabled);
        assert_eq!(fm.form[1].value, "/tmp/a");

        assert_eq!(c.folders[2].name, "empty folder");
        assert!(c.folders[2].requests.is_empty());

        let b = &c.requests[2];
        assert_eq!(b.body_type, BodyType::Binary);
        assert_eq!(b.body, "/tmp/b.csv");
        assert_eq!(b.auth, Auth::Off, "oauth1 不支持：不带凭据，而不是继承错的");
    }

    #[test]
    fn environments_merge_base_into_each_sub() {
        let envs = to_environments(&parse());
        assert_eq!(envs.len(), 2);
        assert_eq!(envs[0].name, "Base");
        let dev = &envs[1];
        assert_eq!(dev.name, "dev");
        let get = |k: &str| {
            dev.variables
                .iter()
                .find(|v| v.key == k)
                .unwrap()
                .value
                .clone()
        };
        assert_eq!(get("base"), "https://b");
        assert_eq!(get("tok"), "t1");
        assert_eq!(get("nested"), r#"{"a":"{{base}}/v1"}"#);
    }

    #[test]
    fn vars_rewrites_only_insomnia_syntax() {
        assert_eq!(
            vars("{{ _.a }}/{{_.b}}/{{c}}/{{$uuid}}/{{"),
            "{{a}}/{{b}}/{{c}}/{{$uuid}}/{{"
        );
        assert_eq!(vars("{% response 'body' %}"), "{% response 'body' %}");
    }

    /// 手动跑：INSOMNIA_EXPORT=/path/to/Insomnia_xxx.yaml cargo test -- --ignored real_export
    #[test]
    #[ignore]
    fn real_export() {
        let path = std::env::var("INSOMNIA_EXPORT").unwrap();
        let v: Value = serde_yaml::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(is_export(&v));
        let c = to_collection(&v);
        fn count(f: &Folder) -> usize {
            f.requests.len() + f.folders.iter().map(count).sum::<usize>()
        }
        let n = c.requests.len() + c.folders.iter().map(count).sum::<usize>();
        let envs = to_environments(&v);
        eprintln!(
            "{}: {n} requests, {} folders, {} environments",
            c.name,
            c.folders.len(),
            envs.len()
        );
        assert!(n > 0);
        assert!(!serde_json::to_string(&(c, envs)).unwrap().contains("{{ _."));
    }
}
