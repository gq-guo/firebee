//! 从 curl 命令行导入请求。支持常见写法：单/双引号、`\` 换行续行、`$'…'`、
//! -X/-H/-d/--data-urlencode/-F/-u/-A/-e/-b/-G/--url，其余选项忽略。

use crate::core::models::{Auth, BodyType, HttpMethod, KeyValue, Request};

/// 按 POSIX shell 规则切词（够用版：不做变量展开）
fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('\n') | None => {}
                Some(n) => {
                    cur.push(n);
                    in_word = true;
                }
            },
            '\'' => {
                in_word = true;
                for n in chars.by_ref() {
                    if n == '\'' {
                        break;
                    }
                    cur.push(n);
                }
            }
            '"' => {
                in_word = true;
                while let Some(n) = chars.next() {
                    match n {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some(e @ ('"' | '\\' | '$' | '`')) => cur.push(e),
                            Some('\n') | None => {}
                            Some(e) => {
                                cur.push('\\');
                                cur.push(e);
                            }
                        },
                        _ => cur.push(n),
                    }
                }
            }
            '$' if chars.peek() == Some(&'\'') => {
                // ANSI-C quoting $'…'
                chars.next();
                in_word = true;
                while let Some(n) = chars.next() {
                    match n {
                        '\'' => break,
                        '\\' => match chars.next() {
                            Some('n') => cur.push('\n'),
                            Some('t') => cur.push('\t'),
                            Some('r') => cur.push('\r'),
                            Some(e) => cur.push(e),
                            None => {}
                        },
                        _ => cur.push(n),
                    }
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            _ => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    out
}

fn value(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    it.next().ok_or_else(|| format!("`{flag}` needs a value"))
}

fn parse_method(s: &str) -> Result<HttpMethod, String> {
    let up = s.to_ascii_uppercase();
    HttpMethod::ALL
        .into_iter()
        .find(|m| m.as_str() == up)
        .ok_or_else(|| format!("Unsupported method `{s}`"))
}

fn split_kv(s: &str) -> KeyValue {
    match s.split_once('=') {
        Some((k, v)) => KeyValue::new(k, v),
        None => KeyValue::new(s, ""),
    }
}

// 带值但我们不关心的选项：跳过它们的值，避免被当成 URL
const SKIP_WITH_VALUE: &[&str] = &[
    "-o", "--output", "-m", "--max-time", "--connect-timeout", "-w", "--write-out", "-x", "--proxy",
    "--retry", "-T", "--upload-file", "-c", "--cookie-jar", "--cacert", "--cert", "-E", "--key",
    "--resolve", "--interface", "--max-redirs", "-K", "--config", "--limit-rate", "-D", "--dump-header",
];

pub fn from_curl(text: &str) -> Result<Request, String> {
    let mut it = shell_words(text).into_iter();
    match it.next() {
        Some(w) if w == "curl" || w.ends_with("/curl") || w == "curl.exe" => {}
        _ => return Err("Not a curl command — it should start with `curl`".into()),
    }

    let mut req = Request::new("Imported request");
    let mut method: Option<HttpMethod> = None;
    let mut url: Option<String> = None;
    let mut user: Option<String> = None;
    let mut data: Vec<String> = Vec::new();
    let mut form: Vec<KeyValue> = Vec::new();
    let mut as_get = false;

    while let Some(w) = it.next() {
        match w.as_str() {
            "-X" | "--request" => method = Some(parse_method(&value(&mut it, &w)?)?),
            m if m.starts_with("-X") && m.len() > 2 => method = Some(parse_method(&m[2..])?),
            "-H" | "--header" => {
                let hv = value(&mut it, &w)?;
                if let Some((k, v)) = hv.split_once(':') {
                    req.headers.push(KeyValue::new(k.trim(), v.trim()));
                }
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" | "--json" => {
                let d = value(&mut it, &w)?;
                if w == "--json" {
                    req.headers.push(KeyValue::new("Content-Type", "application/json"));
                }
                data.push(d.strip_prefix('@').map(|f| format!("@{f}")).unwrap_or(d.clone()));
            }
            "--data-urlencode" | "-F" | "--form" | "--form-string" => form.push(split_kv(&value(&mut it, &w)?)),
            "-u" | "--user" => user = Some(value(&mut it, &w)?),
            "--url" => url = Some(value(&mut it, &w)?),
            "-A" | "--user-agent" => req.headers.push(KeyValue::new("User-Agent", value(&mut it, &w)?)),
            "-e" | "--referer" => req.headers.push(KeyValue::new("Referer", value(&mut it, &w)?)),
            "-b" | "--cookie" => req.headers.push(KeyValue::new("Cookie", value(&mut it, &w)?)),
            "-G" | "--get" => as_get = true,
            f if SKIP_WITH_VALUE.contains(&f) => {
                value(&mut it, &w)?;
            }
            f if f.starts_with('-') && f.len() > 1 => {} // -s -k -L -v -i --compressed …
            _ => {
                if url.is_none() {
                    url = Some(w);
                }
            }
        }
    }

    // URL：补 scheme，把 query 拆到 params
    let raw = url.ok_or("No URL found in the command")?;
    let raw = if raw.contains("://") { raw } else { format!("http://{raw}") };
    let mut parsed = reqwest::Url::parse(&raw).map_err(|e| format!("Invalid URL `{raw}`: {e}"))?;
    req.params = parsed.query_pairs().map(|(k, v)| KeyValue::new(k, v)).collect();
    parsed.set_query(None);
    req.url = parsed.to_string();

    // 认证
    if let Some(u) = user {
        let (name, pass) = u.split_once(':').unwrap_or((&u, ""));
        req.auth = Auth::Basic { username: name.into(), password: pass.into() };
    } else if let Some(i) = req.headers.iter().position(|h| h.key.eq_ignore_ascii_case("authorization")) {
        if let Some(tok) = req.headers[i].value.strip_prefix("Bearer ") {
            req.auth = Auth::Bearer { token: tok.trim().into() };
            req.headers.remove(i);
        }
    }

    // body
    let content_type = req
        .headers
        .iter()
        .find(|h| h.key.eq_ignore_ascii_case("content-type"))
        .map(|h| h.value.to_ascii_lowercase())
        .unwrap_or_default();
    let joined = data.join("&");
    if as_get {
        for kv in joined.split('&').filter(|s| !s.is_empty()) {
            req.params.push(split_kv(kv));
        }
        for kv in form {
            req.params.push(kv);
        }
    } else if !form.is_empty() {
        req.body_type = BodyType::Form;
        req.form = form;
        for kv in joined.split('&').filter(|s| !s.is_empty()) {
            req.form.push(split_kv(kv));
        }
    } else if !joined.is_empty() {
        if serde_json::from_str::<serde_json::Value>(&joined).is_ok() {
            req.body_type = BodyType::Json;
            // Firebee 发送 JSON 时自动带 Content-Type，去掉导入的以免重复
            req.headers.retain(|h| !h.key.eq_ignore_ascii_case("content-type"));
        } else if content_type.contains("x-www-form-urlencoded") {
            req.body_type = BodyType::Form;
            req.form = joined.split('&').filter(|s| !s.is_empty()).map(split_kv).collect();
        } else {
            req.body_type = BodyType::Text;
        }
        if req.body_type != BodyType::Form {
            req.body = joined;
        }
    }

    req.method = method.unwrap_or(if req.body_type == BodyType::None { HttpMethod::Get } else { HttpMethod::Post });
    req.name = format!("{} {}", req.method.as_str(), parsed.path());
    Ok(req)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_words_handles_quotes_and_continuations() {
        let w = shell_words("curl -X POST \\\n  'a b' \"c \\\"d\\\"\" $'x\\ny' plain");
        assert_eq!(w, vec!["curl", "-X", "POST", "a b", "c \"d\"", "x\ny", "plain"]);
    }

    #[test]
    fn imports_post_json_with_headers() {
        let cmd = r#"curl -X POST \
  'https://api.dev/v2/products/?page=2&x=y' \
  -H 'am-api-key: k1' \
  -H 'Content-Type: application/json' \
  -d '{"a":1,"b":[1,2]}'"#;
        let r = from_curl(cmd).unwrap();
        assert_eq!(r.method, HttpMethod::Post);
        assert_eq!(r.url, "https://api.dev/v2/products/");
        assert_eq!(r.params, vec![KeyValue::new("page", "2"), KeyValue::new("x", "y")]);
        assert_eq!(r.headers, vec![KeyValue::new("am-api-key", "k1")]); // Content-Type 被去掉
        assert_eq!(r.body_type, BodyType::Json);
        assert_eq!(r.body, r#"{"a":1,"b":[1,2]}"#);
        assert_eq!(r.name, "POST /v2/products/");
    }

    #[test]
    fn bare_url_is_get_and_bearer_header_becomes_auth() {
        let r = from_curl("curl -sL api.dev/x -H 'Authorization: Bearer t0k' --compressed").unwrap();
        assert_eq!(r.method, HttpMethod::Get);
        assert_eq!(r.url, "http://api.dev/x");
        assert_eq!(r.auth, Auth::Bearer { token: "t0k".into() });
        assert!(r.headers.is_empty());
    }

    #[test]
    fn basic_auth_form_and_attached_method() {
        let r = from_curl("curl -XPUT -u me:pw --data-urlencode 'q=a b' -d k=v https://api.dev/f").unwrap();
        assert_eq!(r.method, HttpMethod::Put);
        assert_eq!(r.auth, Auth::Basic { username: "me".into(), password: "pw".into() });
        assert_eq!(r.body_type, BodyType::Form);
        assert_eq!(r.form, vec![KeyValue::new("q", "a b"), KeyValue::new("k", "v")]);
    }

    #[test]
    fn data_without_method_is_post_text_and_get_flag_moves_to_params() {
        let r = from_curl("curl https://api.dev/t -d 'hello world'").unwrap();
        assert_eq!(r.method, HttpMethod::Post);
        assert_eq!(r.body_type, BodyType::Text);
        let g = from_curl("curl -G https://api.dev/s -d a=1 -d b=2").unwrap();
        assert_eq!(g.method, HttpMethod::Get);
        assert_eq!(g.params, vec![KeyValue::new("a", "1"), KeyValue::new("b", "2")]);
    }

    #[test]
    fn skips_valued_options_and_rejects_non_curl() {
        let r = from_curl("curl -o out.json -m 10 https://api.dev/z").unwrap();
        assert_eq!(r.url, "https://api.dev/z");
        assert!(from_curl("wget https://x").is_err());
        assert!(from_curl("curl -H 'a: b'").unwrap_err().contains("No URL"));
    }
}
