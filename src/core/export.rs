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
        Auth::Bearer { token } => hs.push(("Authorization".into(), format!("Bearer {token}"))),
        Auth::ApiKey {
            key,
            value,
            in_query: false,
        } if !key.is_empty() => {
            hs.push((key.clone(), value.clone()));
        }
        _ => {}
    }
    if (req.body_type == BodyType::Json && !req.body.is_empty())
        || req.body_type == BodyType::GraphQL
    {
        hs.push(("Content-Type".into(), "application/json".into()));
    }
    hs
}

/// curl 的 -d 用单行 JSON：几百行的美化 JSON 粘进终端会撑爆行编辑器，回车也发不出去。
/// 不是合法 JSON 时原样保留。
fn curl_body(req: &Request) -> String {
    if req.body_type == BodyType::GraphQL {
        return gql_body(req);
    }
    if req.body_type == BodyType::Json {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&req.body) {
            return v.to_string();
        }
    }
    req.body.clone()
}

/// ponytail: 变量不是合法 JSON 时导出只带 query（发送时会明确报错）
fn gql_body(req: &Request) -> String {
    req.graphql_payload()
        .unwrap_or_else(|_| serde_json::json!({ "query": req.body }).to_string())
}

pub fn to_curl(req: &Request, url: &str) -> String {
    let mut parts = vec![format!("curl -X {}", req.method.as_str()), sh(url)];
    if let Auth::Basic { username, password } = &req.auth {
        parts.push(format!("-u {}", sh(&format!("{username}:{password}"))));
    }
    for (k, v) in effective_headers(req) {
        parts.push(format!("-H {}", sh(&format!("{k}: {v}"))));
    }
    match &req.body_type {
        BodyType::Json | BodyType::Text => {
            if !req.body.is_empty() {
                parts.push(format!("-d {}", sh(&curl_body(req))));
            }
        }
        BodyType::GraphQL => parts.push(format!("-d {}", sh(&curl_body(req)))),
        BodyType::Form => {
            for f in req.form.iter().filter(|f| f.enabled && !f.key.is_empty()) {
                parts.push(format!(
                    "--data-urlencode {}",
                    sh(&format!("{}={}", f.key, f.value))
                ));
            }
        }
        BodyType::None => {}
    }
    parts.join(" \\\n  ")
}

pub fn to_python(req: &Request, url: &str) -> String {
    let mut s = String::from("import requests\n\n");
    s.push_str("response = requests.request(\n");
    s.push_str(&format!(
        "    {},\n    {},\n",
        py(req.method.as_str()),
        py(url)
    ));
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
        BodyType::GraphQL => s.push_str(&format!("    data={},\n", py(&gql_body(req)))),
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
        r.auth = Auth::Bearer {
            token: "t123".into(),
        };
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
        assert!(c.contains(r#"-d '{"u":"a","p":"b'\''s"}'"#), "{c}");
    }

    #[test]
    fn curl_minifies_pretty_json_body() {
        let mut r = sample();
        r.body = "{\n  \"a\": 1,\n  \"b\": [\n    1,\n    2\n  ]\n}".into();
        let c = to_curl(&r, "https://api.dev/login");
        assert!(c.contains(r#"-d '{"a":1,"b":[1,2]}'"#), "{c}");
        // 非法 JSON 原样保留
        r.body = "{not json\n}".into();
        assert!(to_curl(&r, "u").contains("-d '{not json\n}'"));
    }

    #[test]
    fn curl_basic_auth() {
        let mut r = Request::new("t");
        r.url = "https://api.dev/x".into();
        r.auth = Auth::Basic {
            username: "u".into(),
            password: "p".into(),
        };
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
    fn curl_graphql_sends_json_payload() {
        let mut r = Request::new("t");
        r.method = HttpMethod::Post;
        r.body_type = BodyType::GraphQL;
        r.body = "{ me { id } }".into();
        let c = to_curl(&r, "https://api.dev/graphql");
        assert!(c.contains("Content-Type: application/json"), "{c}");
        assert!(c.contains(r#"-d '{"query":"{ me { id } }"}'"#), "{c}");
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
