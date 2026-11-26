//! Postman Collection v2.1 / Environment 导入。只转我们支持的字段，其余丢弃。

use serde::Serialize;
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
                    secret: false,
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

fn request(item: &Value, label: &str, rep: &mut ScriptReport) -> Request {
    let r = &item["request"];
    let mut req = Request::new(s(&item["name"]));
    convert_scripts(item, label, &mut req, rep);
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

fn walk(
    items: &[Value],
    prefix: &str,
    folders: &mut Vec<Folder>,
    requests: &mut Vec<Request>,
    rep: &mut ScriptReport,
) {
    for it in items {
        let label = format!("{prefix}{}", s(&it["name"]));
        if let Some(children) = it.get("item").and_then(Value::as_array) {
            let mut f = Folder::new(s(&it["name"]));
            f.auth = container_auth(it.get("auth"));
            f.headers = kvs(it.get("header"));
            if has_any_script(it) {
                rep.containers.push(label.clone());
            }
            walk(
                children,
                &format!("{label} / "),
                &mut f.folders,
                &mut f.requests,
                rep,
            );
            folders.push(f);
        } else if it.get("request").is_some() {
            requests.push(request(it, &label, rep));
        }
    }
}

pub fn is_collection(v: &Value) -> bool {
    v.get("info").is_some() && v.get("item").is_some()
}

pub fn is_environment(v: &Value) -> bool {
    v.get("values").and_then(Value::as_array).is_some() && v.get("item").is_none()
}

/// 返回集合与脚本转换摘要（没有任何脚本时摘要为空，见 `ScriptReport::is_empty`）
pub fn to_collection(v: &Value) -> (Collection, ScriptReport) {
    let mut col = Collection::new(s(&v["info"]["name"]));
    if col.name.is_empty() {
        col.name = "Imported collection".into();
    }
    col.auth = container_auth(v.get("auth"));
    col.headers = kvs(v.get("header"));
    let mut rep = ScriptReport::default();
    if has_any_script(v) {
        rep.containers.push(col.name.clone());
    }
    let items = v["item"].as_array().cloned().unwrap_or_default();
    walk(&items, "", &mut col.folders, &mut col.requests, &mut rep);
    (col, rep)
}

pub fn to_environment(v: &Value) -> Environment {
    Environment {
        color: None,
        protected: false,
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
                        secret: false,
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

// ---------- Postman 脚本 → 声明式 Capture / Tests ----------
//
// 只做模式匹配，绝不执行 JS，脚本原文也不保存。认得的语句转成 Capture / Test，
// 认不得的一律不转、只记下是哪个请求——宁可少转，不能转错：一条意思变了的断言比没有更糟。

/// 导入时脚本转换的摘要。集合里一行脚本都没有时为空，前端什么都不多说
#[derive(Serialize, Default, Debug, PartialEq)]
pub struct ScriptReport {
    /// 转出来的 Capture 条数（一律关闭，和其他导入进来的 capture 一样看过再开）
    pub captures: usize,
    /// 转出来的 Test 条数
    pub tests: usize,
    /// 脚本没能完全转换的请求（含 pre-request 脚本），名字带文件夹路径：`Users / Get user`
    pub requests: Vec<String>,
    /// 带脚本的文件夹 / 集合（这一级的脚本一律不转）
    pub containers: Vec<String>,
}

impl ScriptReport {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// 某个 listen（test / prerequest）下所有未禁用脚本拼起来的源码
fn script_src(node: &Value, listen: &str) -> String {
    let mut out = String::new();
    for e in node
        .get("event")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if e["listen"] != listen || e["disabled"].as_bool() == Some(true) {
            continue;
        }
        match &e["script"]["exec"] {
            Value::Array(lines) => lines.iter().for_each(|l| {
                out.push_str(l.as_str().unwrap_or(""));
                out.push('\n');
            }),
            Value::String(x) => {
                out.push_str(x);
                out.push('\n');
            }
            _ => {}
        }
    }
    out
}

/// 有没有真正的语句（Postman 导出里大量 `"exec": [""]` 的空壳，不算脚本）
fn has_any_script(node: &Value) -> bool {
    ["test", "prerequest"]
        .iter()
        .any(|l| !statements(&script_src(node, l)).is_empty())
}

fn convert_scripts(item: &Value, label: &str, req: &mut Request, rep: &mut ScriptReport) {
    let mut complete = statements(&script_src(item, "prerequest")).is_empty();
    let mut binds = std::collections::HashMap::new();
    for st in statements(&script_src(item, "test")) {
        let out = if !st.trusted {
            None // if / else / 循环 / 普通回调里的语句：执行条件变了，不转
        } else if st.header {
            is_test_header(&squash(&st.text)).then_some(Conv::Neutral)
        } else {
            classify(&st.text, &binds)
        };
        match out {
            None => complete = false,
            Some(Conv::Neutral) => {}
            Some(Conv::Bind(name, path)) => {
                binds.insert(name, path);
            }
            Some(Conv::Unbind(name)) => {
                binds.remove(&name);
                complete = false;
            }
            Some(Conv::Capture(k, path)) => {
                if !req.captures.iter().any(|c| c.key == k && c.value == path) {
                    let mut kv = KeyValue::new(k, path);
                    kv.enabled = false; // capture 会改写环境变量：导入进来的一律先关
                    req.captures.push(kv);
                    rep.captures += 1;
                }
            }
            Some(Conv::Test(subject, cond)) => {
                if !req
                    .asserts
                    .iter()
                    .any(|a| a.key == subject && a.value == cond)
                {
                    req.asserts.push(KeyValue::new(subject, cond));
                    rep.tests += 1;
                }
            }
        }
    }
    if !complete {
        rep.requests.push(label.to_string());
    }
}

struct Stmt {
    text: String,
    /// 后面紧跟 `{` 的块头（`pm.test(..., function ()`、`if (x)` 等）
    header: bool,
    /// 只在顶层或 pm.test 回调里为 true
    trusted: bool,
}

/// 把脚本切成语句：去注释，按 `;`、`{`、`}` 和顶层换行切，字符串里的符号不算。
/// 每进一个块记下块头，只有 pm.test 的回调算可信。
fn statements(src: &str) -> Vec<Stmt> {
    let cs: Vec<char> = src.chars().collect();
    let mut out = vec![];
    let mut buf = String::new();
    let mut depth = 0usize; // 当前块内的 ( [ 嵌套
    let mut trusted = true;
    let mut stack: Vec<(usize, bool)> = vec![];
    let flush = |buf: &mut String, out: &mut Vec<Stmt>, header: bool, trusted: bool| {
        let t = buf.trim();
        if !t.is_empty() {
            out.push(Stmt {
                text: t.to_string(),
                header,
                trusted,
            });
        }
        buf.clear();
    };
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        let next = cs.get(i + 1).copied();
        match c {
            '"' | '\'' | '`' => {
                buf.push(c);
                i += 1;
                while i < cs.len() && cs[i] != c && (c == '`' || cs[i] != '\n') {
                    if cs[i] == '\\' && i + 1 < cs.len() {
                        buf.push(cs[i]);
                        i += 1;
                    }
                    buf.push(cs[i]);
                    i += 1;
                }
                if i < cs.len() && cs[i] == c {
                    buf.push(c);
                    i += 1;
                }
                continue;
            }
            '/' if next == Some('/') => {
                while i < cs.len() && cs[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if next == Some('*') => {
                i += 2;
                while i < cs.len() && !(cs[i] == '*' && cs.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
                continue;
            }
            '(' | '[' => {
                depth += 1;
                buf.push(c);
            }
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                buf.push(c);
            }
            '{' => {
                let child = trusted && is_test_header(&squash(buf.trim()));
                flush(&mut buf, &mut out, true, trusted);
                stack.push((depth, trusted));
                depth = 0;
                trusted = child;
            }
            '}' => {
                flush(&mut buf, &mut out, false, trusted);
                (depth, trusted) = stack.pop().unwrap_or((0, true));
            }
            ';' if depth == 0 => flush(&mut buf, &mut out, false, trusted),
            '\n' if depth == 0 => {
                // 没写分号时按行切；但 `.to.eql(1)` 这种换行续写、`=` 结尾的行不切
                let prev = buf.trim_end().chars().last();
                let cont_next = cs[i + 1..].iter().find(|c| !c.is_whitespace()) == Some(&'.');
                let cont_prev = prev.is_some_and(|p| "=,+-*/&|?:.(".contains(p));
                if cont_next || cont_prev {
                    buf.push(' ');
                } else {
                    flush(&mut buf, &mut out, false, trusted);
                }
            }
            _ => buf.push(c),
        }
        i += 1;
    }
    flush(&mut buf, &mut out, false, trusted);
    out
}

/// 去掉字符串外的空白，便于按固定写法匹配
fn squash(s: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let mut esc = false;
    for c in s.chars() {
        match quote {
            Some(q) => {
                out.push(c);
                if esc {
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == q {
                    quote = None;
                }
            }
            None if c.is_whitespace() => {}
            None => {
                if matches!(c, '"' | '\'' | '`') {
                    quote = Some(c);
                }
                out.push(c);
            }
        }
    }
    out
}

/// 开头的 '…' / "…" 字面量；带转义或模板串的不认
fn lit_str(s: &str) -> Option<(String, &str)> {
    let q = s.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let end = s[1..].find(q)? + 1;
    let inner = &s[1..end];
    if inner.contains('\\') {
        return None;
    }
    Some((inner.to_string(), &s[end + 1..]))
}

/// `pm.test("name", function ()` / `pm.test('name', () =>`（已 squash）
fn is_test_header(h: &str) -> bool {
    h.strip_prefix("pm.test(")
        .and_then(lit_str)
        .and_then(|(_, r)| r.strip_prefix(','))
        .is_some_and(|r| r == "function()" || r == "()=>")
}

fn ident_len(s: &str) -> usize {
    s.chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
        .count()
}

/// 指向响应 JSON 的 JS 表达式 → JSONPath。`jsonData.data.items[0].id` → `$.data.items[0].id`
fn json_path(expr: &str, binds: &std::collections::HashMap<String, String>) -> Option<String> {
    const ROOTS: [&str; 3] = [
        "pm.response.json()",
        "JSON.parse(responseBody)",
        "JSON.parse(pm.response.text())",
    ];
    let (mut path, mut rest) = match ROOTS.iter().find_map(|r| expr.strip_prefix(r)) {
        Some(rest) => ("$".to_string(), rest),
        None => {
            let n = ident_len(expr);
            (binds.get(&expr[..n])?.clone(), &expr[n..])
        }
    };
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('.') {
            let n = ident_len(r);
            // `.length` 是 JS 的数组长度，不是 JSON 字段
            if n == 0 || &r[..n] == "length" {
                return None;
            }
            path.push('.');
            path.push_str(&r[..n]);
            rest = &r[n..];
        } else {
            let r = rest.strip_prefix('[')?;
            let digits = r.chars().take_while(char::is_ascii_digit).count();
            if digits > 0 && r[digits..].starts_with(']') {
                path.push_str(&format!("[{}]", &r[..digits]));
                rest = &r[digits + 1..];
            } else {
                let (key, r) = lit_str(r)?;
                let r = r.strip_prefix(']')?;
                if key.is_empty() || key.contains(['\'', ']']) {
                    return None;
                }
                if ident_len(&key) == key.len() {
                    path.push('.');
                    path.push_str(&key);
                } else {
                    path.push_str(&format!("['{key}']"));
                }
                rest = r;
            }
        }
    }
    Some(path)
}

#[derive(Debug, PartialEq)]
enum Lit {
    Num(String),
    Str(String),
    /// true / false / null
    Word(String),
}

/// 整串都是一个标量字面量
fn literal(s: &str) -> Option<Lit> {
    if let Some((v, "")) = lit_str(s) {
        // 首尾空白在断言里会被 trim 掉，比不准
        return (v.trim() == v).then_some(Lit::Str(v));
    }
    if matches!(s, "true" | "false" | "null") {
        return Some(Lit::Word(s.into()));
    }
    let digits = s.strip_prefix('-').unwrap_or(s);
    let ok = !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
        && digits.matches('.').count() <= 1
        && !digits.starts_with('.')
        && !digits.ends_with('.');
    ok.then(|| Lit::Num(s.into()))
}

fn int_lit(s: &str) -> Option<String> {
    (!s.is_empty() && s.chars().all(|c| c.is_ascii_digit())).then(|| s.to_string())
}

enum Conv {
    /// 不影响结果的语句（pm.test 的块头、收尾的 `)`、console.log）
    Neutral,
    /// `var jsonData = pm.response.json()`：记下变量指向的 JSONPath
    Bind(String, String),
    /// 变量被赋成了认不得的东西：之后用到它的语句都不能转
    Unbind(String),
    Capture(String, String),
    Test(String, String),
}

/// 被断言的对象
enum Subj {
    Status,
    Time,
    Body,
    Header(String),
    Json(String),
}

/// 一条语句 → 转换结果；None = 认不得
fn classify(text: &str, binds: &std::collections::HashMap<String, String>) -> Option<Conv> {
    let (decl, body) = ["var ", "let ", "const "]
        .iter()
        .find_map(|k| text.strip_prefix(k))
        .map_or((false, text), |b| (true, b));
    let n = squash(body);
    let n = n.as_str();
    if !decl && (n.chars().all(|c| c == ')') || n.starts_with("console.")) {
        return Some(Conv::Neutral);
    }
    // 变量绑定
    let name_len = ident_len(n);
    if name_len > 0 && n[name_len..].starts_with('=') && !n[name_len..].starts_with("==") {
        let name = n[..name_len].to_string();
        return Some(match json_path(&n[name_len + 1..], binds) {
            Some(p) => Conv::Bind(name, p),
            None => Conv::Unbind(name),
        });
    }
    if decl {
        return None;
    }
    // 存变量
    const SETTERS: [&str; 5] = [
        "pm.environment.set(",
        "pm.collectionVariables.set(",
        "pm.globals.set(",
        "postman.setEnvironmentVariable(",
        "postman.setGlobalVariable(",
    ];
    if let Some(r) = SETTERS.iter().find_map(|p| n.strip_prefix(p)) {
        let (key, r) = lit_str(r)?;
        let expr = r.strip_prefix(',')?.strip_suffix(')')?;
        if key.trim().is_empty() {
            return None;
        }
        return Some(Conv::Capture(key, json_path(expr, binds)?));
    }
    // pm.response.to.have.status(200) / .header("X")
    if let Some(r) = n.strip_prefix("pm.response.to.have.status(") {
        return Some(Conv::Test(
            "status".into(),
            format!("= {}", int_lit(r.strip_suffix(')')?)?),
        ));
    }
    if let Some(r) = n.strip_prefix("pm.response.to.have.header(") {
        let (name, r) = lit_str(r)?;
        return (r == ")" && !name.trim().is_empty())
            .then(|| Conv::Test(format!("header {name}"), "exists".into()));
    }
    // pm.expect(<subject>).to.<chain>(<literal>)
    if let Some(r) = n.strip_prefix("pm.expect(") {
        let close = matching_paren(r)?;
        let subj = subject(&r[..close], binds)?;
        let chain = &r[close + 1..];
        let (op, arg) = CHAINS.iter().find_map(|(c, op)| {
            chain
                .strip_prefix(c)
                .and_then(|a| a.strip_suffix(')'))
                .map(|a| (*op, a))
        })?;
        return test(subj, op, &literal(arg)?);
    }
    // 旧式：tests["name"] = <条件>
    if let Some(r) = n.strip_prefix("tests[") {
        let (_, r) = lit_str(r)?;
        let cond = r.strip_prefix("]=")?;
        if let Some(r) = cond.strip_prefix("responseBody.has(") {
            let (v, r) = lit_str(r)?;
            return (r == ")").then(|| Conv::Test("body".into(), format!("contains {v}")));
        }
        for (sym, op) in [("===", "="), ("==", "="), ("<", "<"), (">", ">")] {
            if let Some((l, rhs)) = cond.split_once(sym) {
                let subj = match l {
                    "responseCode.code" => Subj::Status,
                    "responseTime" => Subj::Time,
                    _ => Subj::Json(json_path(l, binds)?),
                };
                return test(subj, op, &literal(rhs)?);
            }
        }
    }
    None
}

/// chai 链 → 断言运算符。没收 `.not`、`.exist`、`.oneOf`、`.include`（数组）这些语义对不齐的
const CHAINS: [(&str, &str); 14] = [
    (".to.eql(", "="),
    (".to.equal(", "="),
    (".to.be.eql(", "="),
    (".to.be.equal(", "="),
    (".to.deep.equal(", "="),
    (".to.be.below(", "<"),
    (".to.be.lessThan(", "<"),
    (".to.be.above(", ">"),
    (".to.be.greaterThan(", ">"),
    (".to.be.at.most(", "<="),
    (".to.be.at.least(", ">="),
    (".to.include(", "contains"),
    (".to.contain(", "contains"),
    (".to.have.string(", "contains"),
];

/// `(` 已吃掉，找与之配对的 `)` 的位置（字符串里的括号不算）
fn matching_paren(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'' | '`') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') if depth == 0 => return Some(i),
            (None, ')') => depth -= 1,
            _ => {}
        }
    }
    None
}

fn subject(s: &str, binds: &std::collections::HashMap<String, String>) -> Option<Subj> {
    Some(match s {
        "pm.response.code" => Subj::Status,
        "pm.response.responseTime" => Subj::Time,
        "pm.response.text()" | "responseBody" => Subj::Body,
        _ => match s.strip_prefix("pm.response.headers.get(") {
            Some(r) => {
                let (name, r) = lit_str(r)?;
                if r != ")" || name.trim().is_empty() {
                    return None;
                }
                Subj::Header(name)
            }
            None => Subj::Json(json_path(s, binds)?),
        },
    })
}

/// 只组合 assert.rs 语义对得上的：status / time 只比整数，body / header 只做字符串 contains / =，
/// JSON 字段只做标量相等与数字大小（contains 对数组是成员判断，对不上，不转）
fn test(subj: Subj, op: &str, lit: &Lit) -> Option<Conv> {
    let cmp = matches!(op, "<" | ">" | "<=" | ">=");
    let (key, value) = match (subj, lit) {
        (Subj::Status, Lit::Num(n)) if op == "=" || cmp => ("status".into(), int_lit(n)?),
        (Subj::Time, Lit::Num(n)) if cmp => ("time".into(), format!("{} ms", int_lit(n)?)),
        (Subj::Body, Lit::Str(v)) if op == "contains" => ("body".into(), v.clone()),
        (Subj::Header(h), Lit::Str(v)) if op == "contains" || op == "=" => {
            (format!("header {h}"), v.clone())
        }
        (Subj::Json(p), Lit::Num(n)) if op == "=" || cmp => (p, n.clone()),
        (Subj::Json(p), Lit::Str(v)) if op == "=" => (p, format!("\"{v}\"")),
        (Subj::Json(p), Lit::Word(w)) if op == "=" => (p, w.clone()),
        _ => return None,
    };
    Some(Conv::Test(key, format!("{op} {value}")))
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
        let (c, _) = to_collection(&v);
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
        let (c, _) = to_collection(&v);
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

    // ---------- 脚本转换 ----------

    /// 一个请求带一段 test 脚本，转完返回 (请求, 摘要)
    fn with_test(script: &str) -> (Request, ScriptReport) {
        let v = serde_json::json!({
            "info": {"name": "c"},
            "item": [{"name": "r", "request": {"method": "GET", "url": "https://x"},
                      "event": [{"listen": "test", "script": {"type": "text/javascript", "exec": script.lines().collect::<Vec<_>>()}}]}]
        });
        let (c, rep) = to_collection(&v);
        (c.requests[0].clone(), rep)
    }

    fn rules(kvs: &[KeyValue]) -> Vec<(String, String)> {
        kvs.iter()
            .map(|k| (k.key.clone(), k.value.clone()))
            .collect()
    }

    fn pair(k: &str, v: &str) -> (String, String) {
        (k.into(), v.into())
    }

    #[test]
    fn script_environment_set_becomes_disabled_capture() {
        let (r, rep) = with_test(
            r#"var jsonData = pm.response.json();
pm.environment.set("token", jsonData.data.token);
pm.environment.set('first_id', jsonData.data.items[0].id)"#,
        );
        assert_eq!(
            rules(&r.captures),
            vec![
                pair("token", "$.data.token"),
                pair("first_id", "$.data.items[0].id")
            ]
        );
        assert!(
            r.captures.iter().all(|c| !c.enabled),
            "导入的 capture 必须先关"
        );
        assert_eq!((rep.captures, rep.tests), (2, 0));
        assert!(rep.requests.is_empty(), "全部转完的请求不该被列出");
    }

    #[test]
    fn script_setter_variants_and_json_roots() {
        let (r, rep) = with_test(
            r#"let res = JSON.parse(responseBody)
const json = pm.response.json()
pm.collectionVariables.set("a", res.a)
pm.globals.set("b", json["b"]["c d"])
postman.setEnvironmentVariable("c", res.list[2])
postman.setGlobalVariable("d", pm.response.json().x.y);
var data = json.data; pm.environment.set("e", data.id);"#,
        );
        assert_eq!(
            rules(&r.captures),
            vec![
                pair("a", "$.a"),
                pair("b", "$.b['c d']"),
                pair("c", "$.list[2]"),
                pair("d", "$.x.y"),
                pair("e", "$.data.id"),
            ]
        );
        assert!(rep.requests.is_empty());
    }

    #[test]
    fn script_status_checks() {
        let (r, rep) = with_test(
            r#"pm.test("Status code is 200", function () {
    pm.response.to.have.status(200);
});
pm.test('code', () => { pm.expect(pm.response.code).to.eql(201) })
pm.test("code2", function() {
  pm.expect(pm.response.code).to.equal(202);
});"#,
        );
        assert_eq!(
            rules(&r.asserts),
            vec![
                pair("status", "= 200"),
                pair("status", "= 201"),
                pair("status", "= 202")
            ]
        );
        assert!(r.asserts.iter().all(|a| a.enabled));
        assert_eq!((rep.captures, rep.tests), (0, 3));
        assert!(rep.requests.is_empty());
    }

    #[test]
    fn script_json_equality_checks() {
        let (r, rep) = with_test(
            r#"pm.test("body", function () {
    var jsonData = pm.response.json();
    pm.expect(jsonData.success).to.eql(true);
    pm.expect(jsonData.data.name).to.equal("Ada");
    pm.expect(jsonData.data.items[1].qty).to.eql(3);
    pm.expect(jsonData.data.deleted_at).to.eql(null);
    pm.expect(jsonData.data.score)
        .to.be.above(-1.5);
});"#,
        );
        assert_eq!(
            rules(&r.asserts),
            vec![
                pair("$.success", "= true"),
                pair("$.data.name", "= \"Ada\""),
                pair("$.data.items[1].qty", "= 3"),
                pair("$.data.deleted_at", "= null"),
                pair("$.data.score", "> -1.5"),
            ]
        );
        assert!(rep.requests.is_empty());
    }

    #[test]
    fn script_response_time_header_and_body() {
        let (r, rep) = with_test(
            r#"pm.test("fast", function () { pm.expect(pm.response.responseTime).to.be.below(500); });
pm.test("ct", function () {
  pm.response.to.have.header("Content-Type");
  pm.expect(pm.response.headers.get("Content-Type")).to.include("json");
  pm.expect(pm.response.text()).to.include("ok");
});"#,
        );
        assert_eq!(
            rules(&r.asserts),
            vec![
                pair("time", "< 500 ms"),
                pair("header Content-Type", "exists"),
                pair("header Content-Type", "contains json"),
                pair("body", "contains ok"),
            ]
        );
        assert!(rep.requests.is_empty());
    }

    #[test]
    fn script_legacy_tests_object() {
        let (r, rep) = with_test(
            r#"var jsonData = JSON.parse(responseBody);
tests["Status code is 200"] = responseCode.code === 200;
tests["fast"] = responseTime < 200;
tests["has ok"] = responseBody.has("ok");
tests["id"] = jsonData.id === "x1";"#,
        );
        assert_eq!(
            rules(&r.asserts),
            vec![
                pair("status", "= 200"),
                pair("time", "< 200 ms"),
                pair("body", "contains ok"),
                pair("$.id", "= \"x1\""),
            ]
        );
        assert!(rep.requests.is_empty());
    }

    #[test]
    fn converted_rules_evaluate_in_assert() {
        // 转出来的规则交给 assert.rs 能正常求值
        let (r, _) = with_test(
            r#"var j = pm.response.json();
pm.test("s", function () { pm.response.to.have.status(200); pm.expect(j.data.name).to.eql("Ada"); pm.expect(j.data.items[1].qty).to.eql(3); pm.expect(pm.response.responseTime).to.be.below(1000); });"#,
        );
        let hs = vec![("content-type".to_string(), "application/json".to_string())];
        let resp = crate::core::assert::Resp {
            status: 200,
            headers: &hs,
            body: r#"{"data":{"name":"Ada","items":[{"qty":1},{"qty":3}]}}"#,
            duration_ms: 12,
        };
        let out = crate::core::assert::check_all(&r.asserts, &resp);
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|o| o.ok), "{out:?}");
        let root: Value = serde_json::from_str(resp.body).unwrap();
        assert!(crate::core::jsonpath::query(&root, "$.data.items[1].qty").is_ok());
    }

    #[test]
    fn unrecognised_script_is_counted_not_mistranslated() {
        let (r, rep) = with_test(
            r#"var jsonData = pm.response.json();
// 条件里的语句执行条件变了，不能当成无条件规则
if (pm.response.code === 200) {
    pm.environment.set("token", jsonData.token);
    pm.expect(jsonData.ok).to.eql(true);
}
pm.test("len", function () {
    pm.expect(jsonData.items.length).to.eql(3);       // .length 是 JS 属性
    pm.expect(jsonData.tags).to.include("a");          // 数组成员判断，对不上 contains
    pm.expect(jsonData.ok).to.not.eql(false);          // .not 不转
    pm.expect(jsonData.obj).to.eql({a: 1});            // 对象深比较不转
    pm.expect(pm.response.code).to.be.oneOf([200, 201]);
});
var t = pm.environment.get("x");
pm.environment.set("y", t);
pm.environment.set("z", "literal");
pm.environment.set("w", jsonData.items[i].id);
pm.test("arrow without braces", () => pm.response.to.have.status(200));
pm.test("fine", function () { pm.response.to.have.status(201); });"#,
        );
        assert!(r.captures.is_empty(), "{:?}", rules(&r.captures));
        assert_eq!(rules(&r.asserts), vec![pair("status", "= 201")]);
        assert_eq!((rep.captures, rep.tests), (0, 1));
        assert_eq!(rep.requests, vec!["r".to_string()]);
    }

    #[test]
    fn prerequest_and_container_scripts_are_reported() {
        let v = serde_json::json!({
            "info": {"name": "Shop"},
            "event": [{"listen": "prerequest", "script": {"exec": ["pm.environment.set('ts', Date.now());"]}}],
            "item": [{"name": "Users",
                      "event": [{"listen": "test", "script": {"exec": "pm.response.to.have.status(200);"}}],
                      "item": [{"name": "Get", "request": {"method": "GET", "url": "https://x"},
                                "event": [{"listen": "prerequest", "script": {"exec": ["console.log('x')", "pm.variables.set('a', 1)"]}},
                                          {"listen": "test", "script": {"exec": ["pm.response.to.have.status(200)"]}}]}]}]
        });
        let (c, rep) = to_collection(&v);
        // 文件夹级脚本不下放到请求上
        assert_eq!(
            rules(&c.folders[0].requests[0].asserts),
            vec![pair("status", "= 200")]
        );
        assert_eq!(rep.tests, 1);
        assert_eq!(rep.requests, vec!["Users / Get".to_string()]);
        assert_eq!(
            rep.containers,
            vec!["Shop".to_string(), "Users".to_string()]
        );
    }

    #[test]
    fn collection_without_scripts_is_unchanged() {
        // 空壳 event（Postman 导出里很常见）、禁用的脚本、只有注释的脚本都不算
        let v = serde_json::json!({
            "info": {"name": "c"},
            "event": [{"listen": "test", "script": {"exec": [""]}}],
            "item": [{"name": "r", "request": {"method": "GET", "url": "https://x"},
                      "event": [{"listen": "test", "script": {"exec": ["", "// nothing here"]}},
                                {"listen": "test", "disabled": true, "script": {"exec": ["pm.response.to.have.status(200)"]}},
                                {"listen": "prerequest", "script": {"exec": [""]}}]}]
        });
        let (c, rep) = to_collection(&v);
        assert!(rep.is_empty(), "{rep:?}");
        assert!(c.requests[0].captures.is_empty() && c.requests[0].asserts.is_empty());
        let (_, rep) = to_collection(&serde_json::from_str(SAMPLE).unwrap());
        assert!(rep.is_empty());
    }

    #[test]
    fn strings_with_braces_and_semicolons_do_not_split_statements() {
        let (r, rep) = with_test(
            r#"pm.test("a; b { c } // d", function () { pm.expect(pm.response.json().msg).to.eql("x; {y} // z"); });"#,
        );
        assert_eq!(rules(&r.asserts), vec![pair("$.msg", "= \"x; {y} // z\"")]);
        assert!(rep.requests.is_empty());
    }
}
