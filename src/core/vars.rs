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
