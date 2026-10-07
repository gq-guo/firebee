//! 和 ui/app.js 的 jsonPath_ 同一套子集：`$.a.b`、`$.items[0]`、`$.items[-1]`、`$.*`、`$..key`、`$['a b']`。
//! CLI 的 Capture / Assert 用；界面那边仍然是 JS 实现，两边规则必须一致。

use serde_json::Value;

enum Tok {
    Key(String),
    Deep(String),
}

fn tokenize(path: &str) -> Result<Vec<Tok>, String> {
    let p = path.trim();
    let Some(mut rest) = p.strip_prefix('$') else {
        return Err("Path must start with $".into());
    };
    let mut toks = vec![];
    let ident = |s: &str| -> usize {
        s.chars()
            .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '$' | '-' | '*'))
            .map(char::len_utf8)
            .sum()
    };
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("..") {
            let n = ident(r);
            if n == 0 {
                return Err(format!("Can't read “{}”", &rest[..rest.len().min(10)]));
            }
            toks.push(Tok::Deep(r[..n].to_string()));
            rest = &r[n..];
        } else if let Some(r) = rest.strip_prefix('.') {
            let n = ident(r);
            if n == 0 {
                return Err(format!("Can't read “{}”", &rest[..rest.len().min(10)]));
            }
            toks.push(Tok::Key(r[..n].to_string()));
            rest = &r[n..];
        } else if let Some(r) = rest.strip_prefix('[') {
            let Some(end) = r.find(']') else {
                return Err("Missing ]".into());
            };
            let inner = &r[..end];
            let key = inner
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .or_else(|| inner.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
                .unwrap_or(inner);
            toks.push(Tok::Key(key.to_string()));
            rest = &r[end + 1..];
        } else {
            return Err(format!("Can't read “{}”", &rest[..rest.len().min(10)]));
        }
    }
    Ok(toks)
}

fn children(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) => o.values().collect(),
        _ => vec![],
    }
}

fn get<'a>(v: &'a Value, k: &str) -> Vec<&'a Value> {
    if k == "*" {
        return children(v);
    }
    match v {
        Value::Array(a) => {
            let Ok(n) = k.parse::<i64>() else {
                return vec![];
            };
            let i = if n < 0 { a.len() as i64 + n } else { n };
            usize::try_from(i)
                .ok()
                .and_then(|i| a.get(i))
                .into_iter()
                .collect()
        }
        Value::Object(o) => o.get(k).into_iter().collect(),
        _ => vec![],
    }
}

fn descend<'a>(v: &'a Value, acc: &mut Vec<&'a Value>) {
    acc.push(v);
    for c in children(v) {
        descend(c, acc);
    }
}

pub fn query<'a>(root: &'a Value, path: &str) -> Result<Vec<&'a Value>, String> {
    let mut cur = vec![root];
    for t in tokenize(path)? {
        cur = match &t {
            Tok::Key(k) => cur.iter().flat_map(|v| get(v, k)).collect(),
            Tok::Deep(k) => cur
                .iter()
                .flat_map(|v| {
                    let mut all = vec![];
                    descend(v, &mut all);
                    all.into_iter().flat_map(|d| get(d, k)).collect::<Vec<_>>()
                })
                .collect(),
        };
    }
    Ok(cur)
}

/// Capture 的取值规则：第一个命中；字符串原样，其他类型 JSON 序列化；null 当没命中
pub fn capture(root: &Value, path: &str) -> Result<Option<String>, String> {
    let hits = query(root, path)?;
    Ok(match hits.first() {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(v) => Some(v.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths() {
        let j = json!({ "data": { "token": "t", "orders": [ { "id": 1, "app": { "key": "a" } }, { "id": 2, "app": { "key": "b" } } ] }, "a b": 7 });
        let s = |p: &str| {
            query(&j, p)
                .unwrap()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(s("$.data.token"), vec![json!("t")]);
        assert_eq!(s("$.data.orders.*.app.key"), vec![json!("a"), json!("b")]);
        assert_eq!(s("$.data.orders[1].id"), vec![json!(2)]);
        assert_eq!(s("$.data.orders[-1].id"), vec![json!(2)]);
        assert_eq!(s("$.data.orders.1.id"), vec![json!(2)]);
        assert_eq!(s("$..key"), vec![json!("a"), json!("b")]);
        assert_eq!(s("$['a b']"), vec![json!(7)]);
        assert_eq!(s("$.nope.x"), Vec::<Value>::new());
        assert!(query(&j, "data.token").is_err());
        assert_eq!(capture(&j, "$.data.token").unwrap().as_deref(), Some("t"));
        assert_eq!(
            capture(&j, "$.data.orders[0].app").unwrap().as_deref(),
            Some(r#"{"key":"a"}"#)
        );
        assert_eq!(capture(&json!({"t": null}), "$.t").unwrap(), None);
    }
}
