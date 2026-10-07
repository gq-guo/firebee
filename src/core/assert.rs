//! 声明式断言，一行一条：`<subject> <op> [expected]`。不引入 JS 运行时。
//!
//!   subject：status | time | body | header <name> | $.json.path
//!   op：= != < <= > >= contains !contains exists !exists matches
//!
//! 存法复用 KeyValue：key = subject，value = "op expected"。GUI 和 CLI 走同一个 check。

use serde::Serialize;
use serde_json::Value;

use super::jsonpath;
use super::models::KeyValue;

#[derive(Serialize, Debug, PartialEq)]
pub struct Outcome {
    pub rule: String,
    pub ok: bool,
    /// 失败原因；通过时为空
    pub message: String,
}

pub struct Resp<'a> {
    pub status: u16,
    pub headers: &'a [(String, String)],
    pub body: &'a str,
    pub duration_ms: u128,
}

pub fn check_all(rules: &[KeyValue], resp: &Resp) -> Vec<Outcome> {
    let json: Option<Value> = serde_json::from_str(resp.body).ok();
    rules
        .iter()
        .filter(|r| r.enabled && !r.key.trim().is_empty())
        .map(|r| {
            let rule = format!("{} {}", r.key.trim(), r.value.trim());
            match check(r.key.trim(), r.value.trim(), resp, json.as_ref()) {
                Ok(()) => Outcome {
                    rule,
                    ok: true,
                    message: String::new(),
                },
                Err(m) => Outcome {
                    rule,
                    ok: false,
                    message: m,
                },
            }
        })
        .collect()
}

/// 被断言的实际值。None = 不存在（header 没有、JSONPath 没命中）
fn actual(subject: &str, resp: &Resp, json: Option<&Value>) -> Result<Option<Value>, String> {
    let s = subject.trim();
    let lower = s.to_ascii_lowercase();
    Ok(match lower.as_str() {
        "status" => Some(Value::from(resp.status)),
        "time" | "duration" => Some(Value::from(resp.duration_ms as u64)),
        "body" => Some(Value::from(resp.body)),
        _ if lower.starts_with("header ") || lower.starts_with("header:") => {
            let name = s[7..].trim();
            resp.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| Value::from(v.as_str()))
        }
        _ if s.starts_with('$') => {
            let json = json.ok_or("response body is not JSON")?;
            jsonpath::query(json, s)?.first().map(|v| (*v).clone())
        }
        _ => {
            return Err(format!(
                "unknown subject “{s}” (status, time, body, header <name>, or a $.json.path)"
            ))
        }
    })
}

fn check(subject: &str, cond: &str, resp: &Resp, json: Option<&Value>) -> Result<(), String> {
    let (op, expected) = match cond.split_once(char::is_whitespace) {
        Some((op, rest)) => (op, rest.trim()),
        None => (cond, ""),
    };
    // 引号只是给人看的：`= "abc"` 和 `contains "abc"` 都按 abc 比
    let expected = expected
        .strip_prefix('"')
        .and_then(|e| e.strip_suffix('"'))
        .unwrap_or(expected);
    let got = actual(subject, resp, json)?;
    let show = |v: &Option<Value>| match v {
        None => "nothing".to_string(),
        Some(Value::String(s)) => format!("“{}”", s.chars().take(80).collect::<String>()),
        Some(v) => v.to_string().chars().take(80).collect(),
    };
    match op {
        "exists" => {
            return if got.is_some() {
                Ok(())
            } else {
                Err("not found".into())
            }
        }
        "!exists" | "missing" => {
            return if got.is_none() {
                Ok(())
            } else {
                Err(format!("found {}", show(&got)))
            }
        }
        "" => return Err(
            "missing operator (=, !=, <, <=, >, >=, contains, !contains, exists, !exists, matches)"
                .into(),
        ),
        _ => {}
    }
    let Some(got) = got else {
        return Err("not found".into());
    };
    let got_s = match &got {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    };
    // 数字比较：两边都能当数字就按数字比；否则按字符串
    let num = |s: &str| s.trim().trim_end_matches("ms").trim().parse::<f64>().ok();
    let got_n = got.as_f64().or_else(|| num(&got_s));
    let exp_n = num(expected);
    let ord = match (got_n, exp_n) {
        (Some(a), Some(b)) => Some(a.partial_cmp(&b)),
        _ => None,
    };
    let fail = |why: &str| Err(format!("{why} (got {})", show(&Some(got.clone()))));
    let eq = || match (got_n, exp_n) {
        (Some(a), Some(b)) => a == b,
        _ => got_s == expected,
    };
    match op {
        "=" | "==" | "is" => {
            if eq() {
                Ok(())
            } else {
                fail(&format!("expected {expected}"))
            }
        }
        "!=" | "not" => {
            if !eq() {
                Ok(())
            } else {
                fail(&format!("expected not {expected}"))
            }
        }
        "<" | "<=" | ">" | ">=" => {
            let Some(Some(o)) = ord else {
                return fail("not a number");
            };
            use std::cmp::Ordering::*;
            let ok = match op {
                "<" => o == Less,
                "<=" => o != Greater,
                ">" => o == Greater,
                _ => o != Less,
            };
            if ok {
                Ok(())
            } else {
                fail(&format!("expected {op} {expected}"))
            }
        }
        "contains" | "has" => {
            if got_s.contains(expected) {
                Ok(())
            } else {
                fail(&format!("expected to contain {expected}"))
            }
        }
        "!contains" => {
            if !got_s.contains(expected) {
                Ok(())
            } else {
                fail(&format!("expected not to contain {expected}"))
            }
        }
        "matches" | "~" => {
            // ponytail: 不上 regex crate；* 当通配，整串锚定（没有 * 就是相等）
            if glob(expected.trim_matches('/'), &got_s) {
                Ok(())
            } else {
                fail(&format!("expected to match {expected}"))
            }
        }
        other => Err(format!("unknown operator “{other}”")),
    }
}

/// `*` 匹配任意串，其余字面；整串锚定
fn glob(pat: &str, s: &str) -> bool {
    match pat.split_once('*') {
        None => pat == s,
        Some((head, rest)) => {
            let Some(after) = s.strip_prefix(head) else {
                return false;
            };
            if rest.is_empty() {
                return true;
            }
            // 让 * 吃任意长度前缀，剩下的递归
            (0..=after.len())
                .filter(|&i| after.is_char_boundary(i))
                .any(|i| glob(rest, &after[i..]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp<'a>(headers: &'a [(String, String)]) -> Resp<'a> {
        Resp {
            status: 201,
            headers,
            body: r#"{"success":true,"data":{"id":"abc","n":3,"tags":[]}}"#,
            duration_ms: 120,
        }
    }

    fn rule(k: &str, v: &str) -> KeyValue {
        KeyValue::new(k, v)
    }

    #[test]
    fn operators() {
        let hs = vec![(
            "content-type".to_string(),
            "application/json; charset=utf-8".to_string(),
        )];
        let r = resp(&hs);
        let ok = |k: &str, v: &str| assert!(check_all(&[rule(k, v)], &r)[0].ok, "{k} {v}");
        let bad = |k: &str, v: &str| assert!(!check_all(&[rule(k, v)], &r)[0].ok, "{k} {v}");
        ok("status", "= 201");
        ok("Status", "< 300");
        bad("status", "= 200");
        ok("time", "< 1000 ms");
        bad("time", "> 1000");
        ok("header content-type", "contains json");
        ok("header Content-Type", "exists");
        bad("header x-missing", "exists");
        ok("header x-missing", "!exists");
        ok("$.success", "= true");
        ok("$.data.id", "exists");
        ok("$.data.id", "= abc");
        ok("$.data.id", "= \"abc\"");
        ok("$.data.n", ">= 3");
        bad("$.data.nope", "exists");
        ok("$.data.tags", "= []");
        ok("body", "contains \"id\":\"abc\"");
        ok("body", "matches *\"n\":3*");
        bad("body", "matches *zzz*");
        bad("$.data.id", "matches a");
        ok("$.data.id", "matches abc");
        ok("$.data.id", "matches a*c");
        bad("$.data.id", "matches a*b");
        ok("$.data.id", "contains \"ab\"");
        bad("$.data.id", "");
        bad("nonsense", "= 1");
    }

    #[test]
    fn messages_name_the_actual_value() {
        let hs = vec![];
        let out = check_all(
            &[rule("status", "= 200"), rule("$.data.id", "= xyz")],
            &resp(&hs),
        );
        assert_eq!(out[0].message, "expected 200 (got 201)");
        assert_eq!(out[1].message, "expected xyz (got “abc”)");
    }

    #[test]
    fn non_json_body_with_jsonpath_rule() {
        let hs = vec![];
        let r = Resp {
            status: 200,
            headers: &hs,
            body: "<html>",
            duration_ms: 1,
        };
        let out = check_all(&[rule("$.x", "exists"), rule("status", "= 200")], &r);
        assert!(!out[0].ok && out[0].message.contains("not JSON"));
        assert!(out[1].ok);
    }
}
