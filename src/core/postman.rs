//! Postman Collection v2.1 / Environment 导入。只转我们支持的字段，其余丢弃。

use serde_json::Value;

use crate::core::models::{
    Auth, BodyType, Collection, Environment, Folder, HttpMethod, KeyValue, Request,
};

fn s(v: &Value) -> String {
    match v {
        Value::String(x) => x.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// [{key, value, disabled}] → Vec<KeyValue>
fn kvs(v: Option<&Value>) -> Vec<KeyValue> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|x| x.get("key").is_some())
                .map(|x| KeyValue {
                    enabled: !x.get("disabled").and_then(Value::as_bool).unwrap_or(false),
                    key: s(&x["key"]),
                    value: s(&x["value"]),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Postman 的 auth 对象：{type, <type>: [{key, value}]}
fn auth(v: Option<&Value>) -> Option<Auth> {
    let v = v?;
    let kind = v.get("type")?.as_str()?;
    let get = |k: &str| -> String {
        v.get(kind)
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|x| x["key"] == k))
            .map(|x| s(&x["value"]))
            .unwrap_or_default()
    };
    Some(match kind {
        "bearer" => Auth::Bearer {
            token: get("token"),
        },
        "basic" => Auth::Basic {
            username: get("username"),
            password: get("password"),
        },
        "apikey" => Auth::ApiKey {
            key: get("key"),
            value: get("value"),
            in_query: get("in") == "query",
        },
        _ => Auth::None,
    })
}

fn request(item: &Value, inherited: &Auth) -> Request {
    let r = &item["request"];
    let mut req = Request::new(s(&item["name"]));
    // 字符串形式的 request 只有 URL
    if let Value::String(url) = r {
        req.url = url.clone();
        return req;
    }
    let m = s(&r["method"]).to_ascii_uppercase();
    req.method = HttpMethod::ALL
        .into_iter()
        .find(|x| x.as_str() == m)
        .unwrap_or(HttpMethod::Get);
    match &r["url"] {
        Value::String(u) => req.url = u.clone(),
        u @ Value::Object(_) => {
            // raw 含 query；去掉 query 部分，query[] 单独进 params（保留 disabled 状态）
            let raw = s(&u["raw"]);
            req.url = raw.split('?').next().unwrap_or("").to_string();
            req.params = kvs(u.get("query"));
        }
        _ => {}
    }
    req.headers = kvs(r.get("header"));
    req.auth = auth(r.get("auth")).unwrap_or_else(|| inherited.clone());
    if let Some(body) = r.get("body") {
        match s(&body["mode"]).as_str() {
            "raw" => {
                req.body = s(&body["raw"]);
                let lang = s(&body["options"]["raw"]["language"]);
                let is_json = lang == "json" || serde_json::from_str::<Value>(&req.body).is_ok();
                req.body_type = if is_json {
                    BodyType::Json
                } else {
                    BodyType::Text
                };
                if is_json {
                    req.headers
                        .retain(|h| !h.key.eq_ignore_ascii_case("content-type"));
                }
            }
            "graphql" => {
                req.body_type = BodyType::GraphQL;
                req.method = HttpMethod::Post;
                req.headers
                    .retain(|h| !h.key.eq_ignore_ascii_case("content-type"));
                req.body = s(&body["graphql"]["query"]);
                req.graphql_variables = s(&body["graphql"]["variables"]);
            }
            "urlencoded" => {
                req.body_type = BodyType::Form;
                req.form = kvs(body.get("urlencoded"));
            }
            "formdata" => {
                // 文件字段不支持，只保留文本字段
                req.body_type = BodyType::Form;
                req.form = kvs(body.get("formdata"));
            }
            _ => {}
        }
    }
    req
}

fn walk(items: &[Value], inherited: &Auth, folders: &mut Vec<Folder>, requests: &mut Vec<Request>) {
    for it in items {
        if let Some(children) = it.get("item").and_then(Value::as_array) {
            let mut f = Folder::new(s(&it["name"]));
            let a = auth(it.get("auth")).unwrap_or_else(|| inherited.clone());
            walk(children, &a, &mut f.folders, &mut f.requests);
            folders.push(f);
        } else if it.get("request").is_some() {
            requests.push(request(it, inherited));
        }
    }
}

pub fn is_collection(v: &Value) -> bool {
    v.get("info").is_some() && v.get("item").is_some()
}

pub fn is_environment(v: &Value) -> bool {
    v.get("values").and_then(Value::as_array).is_some() && v.get("item").is_none()
}

pub fn to_collection(v: &Value) -> Collection {
    let mut col = Collection::new(s(&v["info"]["name"]));
    if col.name.is_empty() {
        col.name = "Imported collection".into();
    }
    let root_auth = auth(v.get("auth")).unwrap_or(Auth::None);
    let items = v["item"].as_array().cloned().unwrap_or_default();
    walk(&items, &root_auth, &mut col.folders, &mut col.requests);
    col
}

pub fn to_environment(v: &Value) -> Environment {
    Environment {
        id: uuid::Uuid::new_v4(),
        name: {
            let n = s(&v["name"]);
            if n.is_empty() {
                "Imported environment".into()
            } else {
                n
            }
        },
        variables: v["values"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|x| KeyValue {
                        enabled: x.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                        key: s(&x["key"]),
                        value: s(&x["value"]),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"{
      "info": {"name": "Shop API", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
      "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "{{token}}"}]},
      "item": [
        {"name": "Users", "item": [
          {"name": "Get user", "request": {"method": "GET",
            "url": {"raw": "{{base}}/users/1?expand=1&x=2", "query": [{"key": "expand", "value": "1"}, {"key": "x", "value": "2", "disabled": true}]},
            "header": [{"key": "X-Trace", "value": "abc"}]}}
        ]},
        {"name": "Login", "request": {"method": "POST", "url": "{{base}}/login",
          "auth": {"type": "basic", "basic": [{"key": "username", "value": "u"}, {"key": "password", "value": "p"}]},
          "header": [{"key": "Content-Type", "value": "application/json"}],
          "body": {"mode": "raw", "raw": "{\"u\":\"a\"}", "options": {"raw": {"language": "json"}}}}},
        {"name": "Form", "request": {"method": "PUT", "url": "{{base}}/f",
          "body": {"mode": "urlencoded", "urlencoded": [{"key": "a", "value": "1"}, {"key": "b", "value": "2", "disabled": true}]}}}
      ]
    }"##;

    #[test]
    fn converts_tree_auth_query_and_bodies() {
        let v: Value = serde_json::from_str(SAMPLE).unwrap();
        assert!(is_collection(&v));
        let c = to_collection(&v);
        assert_eq!(c.name, "Shop API");
        assert_eq!(c.folders.len(), 1);
        let get = &c.folders[0].requests[0];
        assert_eq!(get.method, HttpMethod::Get);
        assert_eq!(get.url, "{{base}}/users/1");
        assert_eq!(get.params.len(), 2);
        assert!(!get.params[1].enabled);
        assert_eq!(
            get.auth,
            Auth::Bearer {
                token: "{{token}}".into()
            }
        ); // 继承集合级 auth
        assert_eq!(get.headers[0].key, "X-Trace");

        let login = &c.requests[0];
        assert_eq!(login.method, HttpMethod::Post);
        assert_eq!(login.body_type, BodyType::Json);
        assert_eq!(login.body, r#"{"u":"a"}"#);
        assert!(login.headers.is_empty()); // JSON 的 Content-Type 由发送时补
        assert_eq!(
            login.auth,
            Auth::Basic {
                username: "u".into(),
                password: "p".into()
            }
        );

        let form = &c.requests[1];
        assert_eq!(form.body_type, BodyType::Form);
        assert_eq!(form.form.len(), 2);
        assert!(!form.form[1].enabled);
    }

    #[test]
    fn converts_environment() {
        let v: Value = serde_json::from_str(
            r#"{"name":"dev","values":[{"key":"base","value":"https://d","enabled":true},{"key":"off","value":"x","enabled":false}]}"#,
        )
        .unwrap();
        assert!(is_environment(&v));
        let e = to_environment(&v);
        assert_eq!(e.name, "dev");
        assert_eq!(e.variables.len(), 2);
        assert!(!e.variables[1].enabled);
    }
}
