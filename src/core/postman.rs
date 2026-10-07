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
                    is_file: false,
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
        // noauth 在 Postman 里是"明确不带凭据"，映射成 None 会变成"继承"，
        // 于是集合级的 prod token 被发给了一个本来不该带凭据的请求
        "noauth" => Auth::Off,
        // 只接 client_credentials；别的 grant 需要浏览器回调，先当 None
        "oauth2" if get("grant_type") == "client_credentials" => Auth::OAuth2 {
            token_url: get("accessTokenUrl"),
            client_id: get("clientId"),
            client_secret: get("clientSecret"),
            scope: get("scope"),
        },
        _ => Auth::None,
    })
}

fn request(item: &Value) -> Request {
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
    req.auth = auth(r.get("auth")).unwrap_or(Auth::None); // None = 发送时按容器链继承
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
                req.body_type = BodyType::Multipart;
                req.form = kvs(body.get("formdata"));
                // Postman 文件字段：{type:"file", src:"/path"}，src 可能是数组（多文件）
                if let Some(a) = body.get("formdata").and_then(Value::as_array) {
                    for (f, x) in req
                        .form
                        .iter_mut()
                        .zip(a.iter().filter(|x| x.get("key").is_some()))
                    {
                        if x.get("type").and_then(Value::as_str) == Some("file") {
                            // 别人集合里的本机路径不能一点 Send 就读：先关掉，用户看过再开
                            f.is_file = true;
                            f.enabled = false;
                            f.value = match x.get("src") {
                                Some(Value::Array(v)) => v.first().map(s).unwrap_or_default(),
                                Some(v) => s(v),
                                None => String::new(),
                            };
                        }
                    }
                }
            }
            _ => {}
        }
    }
    req
}

/// Postman 的集合 / 文件夹级 auth 与 header 映射到同级的容器上（Firebee 有同样的继承机制），
/// 不再压到每个请求里——这样在 Firebee 里改一次就全改了，和 Postman 里的结构对得上。
/// 容器上的 auth：Off 只在请求上有意义（容器不决定"带不带凭据"），折回 None
fn container_auth(v: Option<&Value>) -> Auth {
    match auth(v).unwrap_or(Auth::None) {
        Auth::Off => Auth::None,
        a => a,
    }
}

fn walk(items: &[Value], folders: &mut Vec<Folder>, requests: &mut Vec<Request>) {
    for it in items {
        if let Some(children) = it.get("item").and_then(Value::as_array) {
            let mut f = Folder::new(s(&it["name"]));
            f.auth = container_auth(it.get("auth"));
            f.headers = kvs(it.get("header"));
            walk(children, &mut f.folders, &mut f.requests);
            folders.push(f);
        } else if it.get("request").is_some() {
            requests.push(request(it));
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
    col.auth = container_auth(v.get("auth"));
    col.headers = kvs(v.get("header"));
    let items = v["item"].as_array().cloned().unwrap_or_default();
    walk(&items, &mut col.folders, &mut col.requests);
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
                        is_file: false,
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::Inherited;

    const SAMPLE: &str = r##"{
      "info": {"name": "Shop API", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
      "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "{{token}}"}]},
      "header": [{"key": "X-Col", "value": "c"}],
      "item": [
        {"name": "Users",
         "auth": {"type": "apikey", "apikey": [{"key": "key", "value": "X-Key"}, {"key": "value", "value": "k1"}]},
         "header": [{"key": "X-Folder", "value": "f"}],
         "item": [
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
        // 集合 / 文件夹级的 auth 与 header 落在容器上，由 merge_inherited 在发送时合并，
        // 不再压进每个请求——在 Firebee 里改一次就全改了
        assert_eq!(
            c.auth,
            Auth::Bearer {
                token: "{{token}}".into()
            }
        );
        assert_eq!(c.headers[0].key, "X-Col");
        assert_eq!(
            c.folders[0].auth,
            Auth::ApiKey {
                key: "X-Key".into(),
                value: "k1".into(),
                in_query: false
            }
        );
        assert_eq!(c.folders[0].headers[0].key, "X-Folder");
        assert_eq!(get.auth, Auth::None); // 请求自己没设，发送时继承文件夹的
        assert_eq!(get.headers[0].key, "X-Trace");

        // 端到端：合并后文件夹的 auth 赢，三层 header 都在
        let merged = crate::core::models::merge_inherited(
            get,
            &[
                Inherited {
                    headers: c.headers.clone(),
                    auth: c.auth.clone(),
                },
                Inherited {
                    headers: c.folders[0].headers.clone(),
                    auth: c.folders[0].auth.clone(),
                },
            ],
        );
        assert_eq!(
            merged.auth,
            Auth::ApiKey {
                key: "X-Key".into(),
                value: "k1".into(),
                in_query: false
            }
        );
        let names: Vec<&str> = merged.headers.iter().map(|h| h.key.as_str()).collect();
        assert_eq!(names, vec!["X-Col", "X-Folder", "X-Trace"]);

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
    fn postman_noauth_request_does_not_inherit_the_collection_token() {
        let v: Value = serde_json::from_str(
            r#"{"info":{"name":"c","schema":"x"},
                "auth":{"type":"bearer","bearer":[{"key":"token","value":"PROD"}]},
                "item":[{"name":"third party","request":{"method":"GET","url":"https://third.example/x",
                         "auth":{"type":"noauth"}}}]}"#,
        )
        .unwrap();
        let c = to_collection(&v);
        assert_eq!(c.requests[0].auth, Auth::Off);
        let merged = crate::core::models::merge_inherited(
            &c.requests[0],
            &[Inherited {
                headers: c.headers.clone(),
                auth: c.auth.clone(),
            }],
        );
        assert_eq!(merged.auth, Auth::None, "把集合的 prod token 发给第三方了");
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
