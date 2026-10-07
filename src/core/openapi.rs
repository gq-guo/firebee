//! OpenAPI 3.x / Swagger 2.0 → 集合。目标不是编辑器，是"给我规范，立刻有能发的请求"：
//! 按第一个 tag 分文件夹，path 参数变 {{var}}，query/header 参数带上（必填的才启用），
//! body 按 schema 生成示例 JSON，servers[0] 落到环境变量 baseUrl。

use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::models::{
    Auth, BodyType, Collection, Environment, Folder, HttpMethod, KeyValue, Request,
};

pub fn is_openapi(v: &Value) -> bool {
    v.get("openapi").is_some() || v.get("swagger").is_some()
}

/// 返回 (集合, 环境)。环境里只有 baseUrl；规范没写 server 就没有环境。
pub fn to_collection(v: &Value) -> (Collection, Option<Environment>) {
    let v3 = v.get("openapi").is_some();
    let title = v
        .pointer("/info/title")
        .and_then(Value::as_str)
        .unwrap_or("OpenAPI")
        .to_string();
    let mut c = Collection::new(title.clone());
    c.auth = security(v, v3);

    let base = if v3 {
        // server URL 模板 {region} 用 variables 的 default / enum[0] 填上
        v.pointer("/servers/0/url")
            .and_then(Value::as_str)
            .map(|u| {
                let mut url = u.to_string();
                if let Some(vars) = v.pointer("/servers/0/variables").and_then(Value::as_object) {
                    for (name, def) in vars {
                        let val = def
                            .get("default")
                            .or_else(|| def.pointer("/enum/0"))
                            .map(scalar)
                            .unwrap_or_default();
                        url = url.replace(&format!("{{{name}}}"), &val);
                    }
                }
                url
            })
    } else {
        v.get("host").and_then(Value::as_str).map(|h| {
            let scheme = v
                .pointer("/schemes/0")
                .and_then(Value::as_str)
                .unwrap_or("https");
            let bp = v.get("basePath").and_then(Value::as_str).unwrap_or("");
            format!("{scheme}://{h}{}", bp.trim_end_matches('/'))
        })
    };
    let env = base.map(|url| Environment {
        id: Uuid::new_v4(),
        name: title,
        variables: vec![KeyValue::new("baseUrl", url)],
    });

    let mut folders: Vec<Folder> = vec![];
    let paths = v.get("paths").and_then(Value::as_object);
    for (path, item) in paths.into_iter().flatten() {
        let Some(item) = item.as_object() else {
            continue;
        };
        // path 级参数对下面所有方法生效
        let shared = item
            .get("parameters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (m, op) in item {
            let Some(method) = method(m) else { continue };
            let mut req = request(v, v3, path, method, op, &shared);
            if req.auth == Auth::None
                && op.get("security").map(|s| s == &json!([])).unwrap_or(false)
            {
                req.auth = Auth::Off; // 显式 security: [] 的接口不带凭据
            }
            match op.pointer("/tags/0").and_then(Value::as_str) {
                Some(tag) => {
                    let f = match folders.iter_mut().find(|f| f.name == tag) {
                        Some(f) => f,
                        None => {
                            folders.push(Folder::new(tag));
                            folders.last_mut().unwrap()
                        }
                    };
                    f.requests.push(req);
                }
                None => c.requests.push(req),
            }
        }
    }
    c.folders = folders;
    (c, env)
}

fn method(m: &str) -> Option<HttpMethod> {
    Some(match m {
        "get" => HttpMethod::Get,
        "post" => HttpMethod::Post,
        "put" => HttpMethod::Put,
        "delete" => HttpMethod::Delete,
        "patch" => HttpMethod::Patch,
        "head" => HttpMethod::Head,
        "options" => HttpMethod::Options,
        _ => return None,
    })
}

fn request(
    root: &Value,
    v3: bool,
    path: &str,
    method: HttpMethod,
    op: &Value,
    shared: &[Value],
) -> Request {
    let name = op
        .get("summary")
        .or_else(|| op.get("operationId"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{} {path}", method.as_str()));
    let mut req = Request::new(name);
    req.method = method;
    // {id} → {{id}}：界面上会标成未解析变量，一眼能看到要填什么
    req.url = format!(
        "{{{{baseUrl}}}}{}",
        path.replace('{', "{{").replace('}', "}}")
    );

    let own = op
        .get("parameters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut body_schema: Option<Value> = None;
    // 同 name+in 的参数，operation 级覆盖 path 级
    let ident = |p: &Value| {
        (
            p.get("name").and_then(Value::as_str).map(str::to_owned),
            p.get("in").and_then(Value::as_str).map(str::to_owned),
        )
    };
    let own: Vec<Value> = own.iter().map(|p| resolve(root, p)).collect();
    let mut params: Vec<Value> = shared
        .iter()
        .map(|p| resolve(root, p))
        .filter(|p| !own.iter().any(|o| ident(o) == ident(p)))
        .collect();
    params.extend(own);
    for p in params {
        let (Some(name), Some(loc)) = (
            p.get("name").and_then(Value::as_str),
            p.get("in").and_then(Value::as_str),
        ) else {
            continue;
        };
        let required = p.get("required").and_then(Value::as_bool).unwrap_or(false);
        // v3 的 schema 在 parameter.schema 里；v2 直接平铺在 parameter 上
        let schema = if v3 {
            p.get("schema").cloned().unwrap_or(Value::Null)
        } else {
            p.clone()
        };
        let sample = p
            .get("example")
            .cloned()
            .unwrap_or_else(|| example(root, &schema, 0));
        let text = match &sample {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        };
        let mut kv = KeyValue::new(name, text);
        kv.enabled = required;
        match loc {
            "query" => req.params.push(kv),
            "header" => req.headers.push(kv),
            "formData" => {
                req.body_type = BodyType::Form;
                req.form.push(kv);
            }
            "body" => body_schema = p.get("schema").cloned(),
            _ => {} // path 已经在 URL 里；cookie 不管
        }
    }
    if v3 {
        if let Some(content) = op
            .pointer("/requestBody/content")
            .and_then(Value::as_object)
        {
            // 优先 JSON；没有就拿第一个
            let (ct, media) = content
                .iter()
                .find(|(k, _)| k.contains("json"))
                .or_else(|| content.iter().next())
                .map(|(k, v)| (k.clone(), v.clone()))
                .unwrap_or_default();
            if ct.contains("form-urlencoded") || ct.contains("multipart") {
                req.body_type = if ct.contains("multipart") {
                    BodyType::Multipart
                } else {
                    BodyType::Form
                };
                let schema = resolve(root, media.get("schema").unwrap_or(&Value::Null));
                if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                    for (k, s) in props {
                        let mut kv = KeyValue::new(k, scalar(&example(root, s, 1)));
                        kv.is_file = ct.contains("multipart")
                            && s.get("format").and_then(Value::as_str) == Some("binary");
                        req.form.push(kv);
                    }
                }
            } else {
                body_schema = Some(media.get("schema").cloned().unwrap_or(Value::Null));
                if let Some(ex) = media.get("example").cloned().or_else(|| {
                    // examples 是 map，不是数组：拿第一个的 value
                    media
                        .get("examples")
                        .and_then(Value::as_object)
                        .and_then(|m| m.values().next())
                        .and_then(|e| e.get("value"))
                        .cloned()
                }) {
                    req.body_type = BodyType::Json;
                    req.body = serde_json::to_string_pretty(&ex).unwrap_or_default();
                    body_schema = None;
                }
            }
        }
    }
    if let Some(schema) = body_schema {
        req.body_type = BodyType::Json;
        req.body = serde_json::to_string_pretty(&example(root, &schema, 0)).unwrap_or_default();
    }
    req
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        o => o.to_string(),
    }
}

/// $ref 只认本文档内的 #/components/schemas/X 与 #/definitions/X
fn resolve(root: &Value, v: &Value) -> Value {
    match v.get("$ref").and_then(Value::as_str) {
        Some(r) if r.starts_with("#/") => root.pointer(&r[1..]).cloned().unwrap_or(Value::Null),
        _ => v.clone(),
    }
}

/// 按 schema 造一个示例值：example > default > enum[0] > 按 type 给占位。递归封顶 6 层，防止自引用。
fn example(root: &Value, schema: &Value, depth: usize) -> Value {
    example_budget(root, schema, depth, &mut 2000)
}

/// budget：总共最多造这么多节点。深度封顶只管深不管宽，10 个属性互相 $ref 就是 10^6 个节点。
fn example_budget(root: &Value, schema: &Value, depth: usize, budget: &mut usize) -> Value {
    if depth > 6 || *budget == 0 {
        return Value::Null;
    }
    *budget -= 1;
    let s = resolve(root, schema);
    for k in ["example", "default"] {
        if let Some(v) = s.get(k) {
            return v.clone();
        }
    }
    if let Some(e) = s.pointer("/enum/0") {
        return e.clone();
    }
    // allOf：把各段的 properties 合起来
    if let Some(parts) = s.get("allOf").and_then(Value::as_array) {
        let mut m = Map::new();
        for p in parts {
            if let Value::Object(o) = example_budget(root, p, depth + 1, budget) {
                m.extend(o);
            }
        }
        return Value::Object(m);
    }
    for k in ["oneOf", "anyOf"] {
        if let Some(first) = s.pointer(&format!("/{k}/0")) {
            return example_budget(root, first, depth + 1, budget);
        }
    }
    let ty = s
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or(if s.get("properties").is_some() {
            "object"
        } else {
            ""
        });
    match ty {
        "object" => {
            let mut m = Map::new();
            if let Some(props) = s.get("properties").and_then(Value::as_object) {
                for (k, ps) in props {
                    if *budget == 0 {
                        break; // 预算用完就截断，别再往外吐一堆 key: null
                    }
                    m.insert(k.clone(), example_budget(root, ps, depth + 1, budget));
                }
            }
            Value::Object(m)
        }
        "array" => json!([example(
            root,
            s.get("items").unwrap_or(&Value::Null),
            depth + 1
        )]),
        "integer" => json!(0),
        "number" => json!(0.0),
        "boolean" => json!(false),
        "string" => match s.get("format").and_then(Value::as_str) {
            Some("date-time") => json!("2024-01-01T00:00:00Z"),
            Some("date") => json!("2024-01-01"),
            Some("email") => json!("user@example.com"),
            Some("uuid") => json!("00000000-0000-0000-0000-000000000000"),
            Some("uri") | Some("url") => json!("https://example.com"),
            _ => json!("string"),
        },
        _ => Value::Null,
    }
}

/// 第一个安全方案落到集合级 auth；值用变量占位，用户在环境里填一次
fn security(v: &Value, v3: bool) -> Auth {
    let schemes = if v3 {
        v.pointer("/components/securitySchemes")
    } else {
        v.get("securityDefinitions")
    };
    let Some(schemes) = schemes.and_then(Value::as_object) else {
        return Auth::None;
    };
    // 按根级 security 的顺序找，没有就取第一个定义
    let preferred = v
        .pointer("/security/0")
        .and_then(Value::as_object)
        .and_then(|m| m.keys().next().cloned());
    let s = preferred
        .as_deref()
        .and_then(|k| schemes.get(k))
        .or_else(|| schemes.values().next());
    let Some(s) = s else { return Auth::None };
    let ty = s.get("type").and_then(Value::as_str).unwrap_or("");
    let scheme = s.get("scheme").and_then(Value::as_str).unwrap_or("");
    match (ty, scheme) {
        ("http", "bearer") | ("oauth2", _) => Auth::Bearer {
            token: "{{token}}".into(),
        },
        ("http", "basic") | ("basic", _) => Auth::Basic {
            username: "{{username}}".into(),
            password: "{{password}}".into(),
        },
        ("apiKey", _) => Auth::ApiKey {
            key: s
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("X-API-Key")
                .into(),
            value: "{{apiKey}}".into(),
            in_query: s.get("in").and_then(Value::as_str) == Some("query"),
        },
        _ => Auth::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OAS3: &str = r#"
openapi: 3.0.0
info: { title: Pets }
servers: [{ url: https://api.pets.dev/v1 }]
components:
  securitySchemes:
    bearer: { type: http, scheme: bearer }
  schemas:
    Pet:
      type: object
      properties:
        name: { type: string, example: Rex }
        age: { type: integer }
        tags: { type: array, items: { type: string } }
        owner: { $ref: '#/components/schemas/Owner' }
    Owner:
      type: object
      properties:
        email: { type: string, format: email }
paths:
  /pets:
    get:
      tags: [pets]
      summary: List pets
      parameters:
        - { name: limit, in: query, required: true, schema: { type: integer, default: 20 } }
        - { name: X-Trace, in: header, schema: { type: string } }
    post:
      tags: [pets]
      operationId: createPet
      requestBody:
        content:
          application/json:
            schema: { $ref: '#/components/schemas/Pet' }
  /pets/{id}:
    parameters:
      - { name: id, in: path, required: true, schema: { type: string } }
    delete:
      summary: Delete pet
      security: []
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                file: { type: string, format: binary }
                note: { type: string }
"#;

    #[test]
    fn oas3_yaml_to_collection() {
        let v: Value = serde_yaml::from_str(OAS3).unwrap();
        assert!(is_openapi(&v));
        let (c, env) = to_collection(&v);
        assert_eq!(c.name, "Pets");
        assert_eq!(env.unwrap().variables[0].value, "https://api.pets.dev/v1");
        assert_eq!(
            c.auth,
            Auth::Bearer {
                token: "{{token}}".into()
            }
        );
        assert_eq!(c.folders.len(), 1);
        let pets = &c.folders[0];
        assert_eq!(pets.name, "pets");
        let list = &pets.requests[0];
        assert_eq!(list.name, "List pets");
        assert_eq!(list.url, "{{baseUrl}}/pets");
        assert_eq!(list.params[0].key, "limit");
        assert_eq!(list.params[0].value, "20");
        assert!(list.params[0].enabled);
        assert!(!list.headers[0].enabled, "optional header stays off");
        let create = &pets.requests[1];
        assert_eq!(create.method, HttpMethod::Post);
        assert_eq!(create.body_type, BodyType::Json);
        let body: Value = serde_json::from_str(&create.body).unwrap();
        assert_eq!(body["name"], "Rex");
        assert_eq!(body["age"], 0);
        assert_eq!(body["tags"], json!(["string"]));
        assert_eq!(body["owner"]["email"], "user@example.com");
        // 没 tag 的进集合根
        let del = c.requests.iter().find(|r| r.name == "Delete pet").unwrap();
        assert_eq!(del.url, "{{baseUrl}}/pets/{{id}}");
        assert_eq!(del.method, HttpMethod::Delete);
        assert_eq!(del.auth, Auth::Off);
        let up = c
            .requests
            .iter()
            .find(|r| r.name == "POST /upload")
            .unwrap();
        assert_eq!(up.body_type, BodyType::Multipart);
        assert!(up.form.iter().any(|f| f.key == "file" && f.is_file));
    }

    #[test]
    fn swagger2_json_to_collection() {
        let v = json!({
            "swagger": "2.0",
            "info": { "title": "Old" },
            "host": "old.api", "basePath": "/v2", "schemes": ["http"],
            "securityDefinitions": { "key": { "type": "apiKey", "name": "api_key", "in": "header" } },
            "definitions": { "User": { "type": "object", "properties": { "id": { "type": "integer" } } } },
            "paths": { "/users": { "post": {
                "tags": ["users"],
                "parameters": [
                    { "name": "body", "in": "body", "schema": { "$ref": "#/definitions/User" } },
                    { "name": "dry", "in": "query", "type": "boolean" }
                ]
            } } }
        });
        let (c, env) = to_collection(&v);
        assert_eq!(env.unwrap().variables[0].value, "http://old.api/v2");
        assert_eq!(
            c.auth,
            Auth::ApiKey {
                key: "api_key".into(),
                value: "{{apiKey}}".into(),
                in_query: false
            }
        );
        let r = &c.folders[0].requests[0];
        assert_eq!(r.body_type, BodyType::Json);
        assert_eq!(serde_json::from_str::<Value>(&r.body).unwrap()["id"], 0);
        assert_eq!(r.params[0].value, "false");
    }

    #[test]
    fn operation_param_overrides_path_param_and_examples_map_is_used() {
        let v = json!({ "openapi": "3.0.0",
            "servers": [{ "url": "https://{region}.api.io/{ver}", "variables": { "region": { "default": "eu" }, "ver": { "enum": ["v2", "v1"] } } }],
            "paths": { "/x": {
                "parameters": [{ "name": "limit", "in": "query", "schema": { "default": 20 } }, { "name": "q", "in": "query" }],
                "post": {
                    "parameters": [{ "name": "limit", "in": "query", "required": true, "schema": { "default": 100 } }],
                    "requestBody": { "content": { "application/json": {
                        "schema": { "type": "object" },
                        "examples": { "basic": { "value": { "hello": "world" } } } } } }
                } } } });
        let (c, env) = to_collection(&v);
        assert_eq!(env.unwrap().variables[0].value, "https://eu.api.io/v2");
        let r = &c.requests[0];
        let limits: Vec<_> = r.params.iter().filter(|p| p.key == "limit").collect();
        assert_eq!(limits.len(), 1);
        assert_eq!((limits[0].value.as_str(), limits[0].enabled), ("100", true));
        assert!(r.params.iter().any(|p| p.key == "q"));
        assert_eq!(
            serde_json::from_str::<Value>(&r.body).unwrap()["hello"],
            "world"
        );
    }

    #[test]
    fn wide_self_reference_is_bounded() {
        let props: Map<String, Value> = (0..12)
            .map(|i| (format!("p{i}"), json!({ "$ref": "#/components/schemas/N" })))
            .collect();
        let v = json!({ "openapi": "3.0.0", "components": { "schemas": { "N": { "type": "object", "properties": props } } },
            "paths": { "/n": { "post": { "requestBody": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/N" } } } } } } } });
        let t = std::time::Instant::now();
        let (c, _) = to_collection(&v);
        assert!(t.elapsed() < std::time::Duration::from_secs(1));
        assert!(c.requests[0].body.len() < 200_000);
    }

    #[test]
    fn self_referencing_schema_terminates() {
        let v = json!({ "openapi": "3.0.0", "components": { "schemas": { "Node": { "type": "object",
            "properties": { "child": { "$ref": "#/components/schemas/Node" } } } } },
            "paths": { "/n": { "post": { "requestBody": { "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/Node" } } } } } } } });
        let (c, _) = to_collection(&v);
        assert!(c.requests[0].body.contains("child"));
    }
}
