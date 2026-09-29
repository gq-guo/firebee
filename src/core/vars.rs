use std::collections::HashMap;

use crate::core::models::{Auth, Request};

pub struct SubstituteResult {
    pub output: String,
    pub missing: Vec<String>,
}

/// 动态变量：`$` 开头，每次出现都重新生成。名字与 Postman 一致，导入的 collection 可直接跑；
/// 括号参数（`{{$randomInt(1,100)}}`、`{{$randomString(8)}}`）是 Firebee 扩展。
/// (名字, 补全里显示的说明)
pub const DYNAMIC_VARS: &[(&str, &str)] = &[
    ("$uuid", "UUID v4"),
    ("$guid", "UUID v4 (Postman alias)"),
    ("$randomUUID", "UUID v4 (Postman alias)"),
    ("$timestamp", "unix seconds"),
    ("$timestampMs", "unix milliseconds"),
    ("$isoTimestamp", "ISO 8601 UTC"),
    ("$randomInt", "0–1000, or (min,max)"),
    ("$randomString", "16 alphanumerics, or (len)"),
    ("$randomBoolean", "true / false"),
    ("$randomEmail", "jamie.brooks84@example.com"),
    ("$randomFirstName", "first name"),
    ("$randomLastName", "last name"),
    ("$randomFullName", "first + last name"),
    ("$randomUserName", "jamie_brooks84"),
    ("$randomPassword", "12 alphanumerics, or (len)"),
    ("$randomPhoneNumber", "555-123-4567"),
    ("$randomUrl", "https://example.com"),
    ("$randomDomainName", "example.com"),
    ("$randomCompanyName", "company name"),
    ("$randomStreetAddress", "742 Oak Street"),
    ("$randomCity", "city name"),
    ("$randomCountryCode", "ISO 3166 alpha-2"),
    ("$randomPrice", "0.00–999.99, or (min,max)"),
    ("$randomCurrencyCode", "ISO 4217"),
    ("$randomIP", "IPv4"),
    ("$randomLoremWord", "one lorem word"),
    ("$randomLoremSentence", "one lorem sentence"),
    ("$randomDatePast", "ISO 8601, within last year"),
    ("$randomDateFuture", "ISO 8601, within next year"),
];

// ponytail: 用 uuid v4 的随机字节当熵源（底层就是 getrandom），省一个 rand 依赖。
// 跳过 byte 6/8 —— 那两个字节含版本号与 variant 位，不是全随机。
fn rnd() -> u64 {
    let b = uuid::Uuid::new_v4().into_bytes();
    let mut x = [0u8; 8];
    x[..4].copy_from_slice(&b[..4]);
    x[4..].copy_from_slice(&b[12..]);
    u64::from_le_bytes(x)
}

/// [min, max] 闭区间；max <= min 时退化为 min。
/// 参数来自用户输入（`{{$randomInt(a,b)}}`），必须扛住 i64 极值：
/// max-min 会溢出，+1 后可能为 0，取模就 panic——而 Tauri command 里 panic
/// 不会变成 JS 的 reject，invoke 永不 settle，界面直接卡死。
fn between(min: i64, max: i64) -> i64 {
    if max <= min {
        return min;
    }
    // span = max-min+1，饱和到 u64 上界；wrapping_add 让 i64 全域也能取到
    let span = (max as i128 - min as i128 + 1).min(u64::MAX as i128) as u64;
    min.wrapping_add((rnd() % span) as i64)
}

fn pick(list: &[&str]) -> String {
    list[(rnd() % list.len() as u64) as usize].to_string()
}

fn chars(n: i64) -> String {
    const SET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    (0..n.clamp(1, 4096))
        .map(|_| SET[(rnd() % SET.len() as u64) as usize] as char)
        .collect()
}

const FIRST: &[&str] = &[
    "Jamie", "Alex", "Taylor", "Jordan", "Casey", "Riley", "Morgan", "Avery", "Quinn", "Sam",
    "Chris", "Dana", "Ellis", "Frankie", "Harper", "Reese",
];
const LAST: &[&str] = &[
    "Brooks", "Chen", "Diaz", "Evans", "Foster", "Garcia", "Hayes", "Ito", "Jensen", "Khan",
    "Lopez", "Miller", "Novak", "Okafor", "Patel", "Silva",
];
const DOMAINS: &[&str] = &["example.com", "example.org", "test.dev", "mail.example.net"];
const COMPANY_A: &[&str] = &[
    "Northwind",
    "Acme",
    "Globex",
    "Initech",
    "Umbrella",
    "Stark",
    "Wayne",
    "Soylent",
];
const COMPANY_B: &[&str] = &[
    "Labs",
    "Group",
    "Industries",
    "Systems",
    "Holdings",
    "Works",
];
const STREETS: &[&str] = &[
    "Oak", "Maple", "Cedar", "Pine", "Elm", "Birch", "Willow", "Sunset", "Lake", "Hill",
];
const STREET_SUFFIX: &[&str] = &["Street", "Avenue", "Road", "Lane", "Boulevard"];
const CITIES: &[&str] = &[
    "Springfield",
    "Riverside",
    "Fairview",
    "Kingston",
    "Georgetown",
    "Ashland",
    "Clinton",
    "Salem",
    "Madison",
    "Bristol",
];
const COUNTRY_CODES: &[&str] = &[
    "US", "CN", "JP", "DE", "FR", "GB", "SG", "AU", "CA", "BR", "IN", "NL",
];
const CURRENCY_CODES: &[&str] = &[
    "USD", "CNY", "EUR", "JPY", "GBP", "SGD", "AUD", "CAD", "HKD", "KRW",
];
const LOREM: &[&str] = &[
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipiscing",
    "elit",
    "sed",
    "do",
    "eiusmod",
    "tempor",
    "incididunt",
    "labore",
    "dolore",
    "magna",
    "aliqua",
    "enim",
    "minim",
    "veniam",
    "quis",
    "nostrud",
    "ullamco",
    "laboris",
];

fn dynamic(name: &str) -> Option<String> {
    // `$randomInt(1,100)` → base = "$randomInt", args = "1,100"
    let (base, args) = match name.strip_suffix(')').and_then(|n| n.split_once('(')) {
        Some((b, a)) => (b.trim_end(), a),
        None => (name, ""),
    };
    let arg = |i: usize, default: i64| -> i64 {
        args.split(',')
            .nth(i)
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(default)
    };
    let now = chrono::Utc::now();
    let iso =
        |t: chrono::DateTime<chrono::Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    Some(match base {
        "$uuid" | "$guid" | "$randomUUID" => uuid::Uuid::new_v4().to_string(),
        "$timestamp" => now.timestamp().to_string(),
        "$timestampMs" => now.timestamp_millis().to_string(),
        "$isoTimestamp" => iso(now),
        "$randomInt" => between(arg(0, 0), arg(1, 1000)).to_string(),
        "$randomString" => chars(arg(0, 16)),
        "$randomBoolean" => rnd().is_multiple_of(2).to_string(),
        "$randomFirstName" => pick(FIRST),
        "$randomLastName" => pick(LAST),
        "$randomFullName" => format!("{} {}", pick(FIRST), pick(LAST)),
        "$randomEmail" => format!(
            "{}.{}{}@{}",
            pick(FIRST).to_lowercase(),
            pick(LAST).to_lowercase(),
            between(1, 99),
            pick(DOMAINS)
        ),
        "$randomUserName" => format!(
            "{}_{}{}",
            pick(FIRST).to_lowercase(),
            pick(LAST).to_lowercase(),
            between(1, 99)
        ),
        "$randomPassword" => chars(arg(0, 12)),
        "$randomPhoneNumber" => format!("555-{:03}-{:04}", between(100, 999), between(0, 9999)),
        "$randomDomainName" => pick(DOMAINS),
        "$randomUrl" => format!("https://{}", pick(DOMAINS)),
        "$randomCompanyName" => format!("{} {}", pick(COMPANY_A), pick(COMPANY_B)),
        "$randomStreetAddress" => format!(
            "{} {} {}",
            between(1, 9999),
            pick(STREETS),
            pick(STREET_SUFFIX)
        ),
        "$randomCity" => pick(CITIES),
        "$randomCountryCode" => pick(COUNTRY_CODES),
        "$randomCurrencyCode" => pick(CURRENCY_CODES),
        "$randomPrice" => format!(
            "{:.2}",
            between(
                arg(0, 0).saturating_mul(100),
                arg(1, 999).saturating_mul(100).saturating_add(99),
            ) as f64
                / 100.0
        ),
        "$randomIP" => format!(
            "{}.{}.{}.{}",
            between(1, 255),
            between(0, 255),
            between(0, 255),
            between(1, 254)
        ),
        "$randomLoremWord" => pick(LOREM),
        "$randomLoremSentence" => {
            let mut s = (0..between(6, 12))
                .map(|_| pick(LOREM))
                .collect::<Vec<_>>()
                .join(" ");
            s[..1].make_ascii_uppercase();
            s.push('.');
            s
        }
        "$randomDatePast" => iso(now - chrono::Duration::seconds(between(1, 365 * 86400))),
        "$randomDateFuture" => iso(now + chrono::Duration::seconds(between(1, 365 * 86400))),
        _ => return None,
    })
}

/// 把 {{name}} 替换为 vars[name]（或动态变量）；未定义的变量原样保留并记入 missing（去重、按出现顺序）。
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
                match vars.get(name).cloned().or_else(|| dynamic(name)) {
                    Some(v) => output.push_str(&v),
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
    out.graphql_variables = apply(&out.graphql_variables, vars, &mut missing);
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
    fn dynamic_vars_generate_fresh_values() {
        let r = substitute(
            "{{$uuid}}/{{$uuid}}/{{$timestamp}}/{{$randomInt}}/{{$nope}}",
            &HashMap::new(),
        );
        assert_eq!(r.missing, vec!["$nope".to_string()]);
        let parts: Vec<&str> = r.output.split('/').collect();
        assert_ne!(parts[0], parts[1]); // 每次出现都不同
        assert_eq!(parts[0].len(), 36);
        assert!(parts[2].parse::<i64>().unwrap() > 1_700_000_000);
        assert!(parts[3].parse::<u32>().unwrap() <= 1000);
        assert_eq!(parts[4], "{{$nope}}");
        // 用户定义的同名变量优先
        let vars = HashMap::from([("$uuid".to_string(), "fixed".to_string())]);
        assert_eq!(substitute("{{$uuid}}", &vars).output, "fixed");
    }

    #[test]
    fn extreme_dynamic_var_args_dont_panic() {
        // 参数来自用户随手敲的文本，i64 极值不能把命令打 panic（panic 会让 invoke 永不 settle）
        let v = HashMap::new();
        for src in [
            "{{$randomInt(-9223372036854775808,9223372036854775807)}}",
            "{{$randomInt(-9000000000000000000,9000000000000000000)}}",
            "{{$randomInt(9223372036854775807,-9223372036854775808)}}",
            "{{$randomPrice(0,99999999999999999)}}",
            "{{$randomPrice(-9223372036854775808,9223372036854775807)}}",
        ] {
            let out = substitute(src, &v).output;
            assert!(!out.contains("{{"), "{src} 没被替换：{out}");
        }
        // 正常区间仍然守约
        for _ in 0..200 {
            let n: i64 = substitute("{{$randomInt(-5,5)}}", &v)
                .output
                .parse()
                .unwrap();
            assert!((-5..=5).contains(&n));
        }
    }

    #[test]
    fn parameterized_dynamic_vars() {
        let v = HashMap::new();
        for _ in 0..50 {
            let n: i64 = substitute("{{$randomInt(5,7)}}", &v)
                .output
                .parse()
                .unwrap();
            assert!((5..=7).contains(&n));
        }
        assert_eq!(substitute("{{$randomString(8)}}", &v).output.len(), 8);
        assert_eq!(substitute("{{ $randomInt(3, 3) }}", &v).output, "3");
        // 参数非法 / 缺失 → 回落到默认区间
        assert!(
            substitute("{{$randomInt(x)}}", &v)
                .output
                .parse::<i64>()
                .unwrap()
                <= 1000
        );
        // 未知名字带括号仍算缺失
        assert_eq!(
            substitute("{{$nope(1)}}", &v).missing,
            vec!["$nope(1)".to_string()]
        );
    }

    #[test]
    fn faker_vars_look_sane() {
        let v = HashMap::new();
        let out = substitute(
            "{{$randomEmail}}|{{$randomPrice}}|{{$randomIP}}|{{$randomLoremSentence}}|{{$randomDatePast}}|{{$randomBoolean}}",
            &v,
        )
        .output;
        let p: Vec<&str> = out.split('|').collect();
        assert!(p[0].contains('@') && p[0].contains('.'));
        let price: f64 = p[1].parse().unwrap();
        assert!((0.0..1000.0).contains(&price));
        assert_eq!(
            p[2].split('.').filter(|o| o.parse::<u8>().is_ok()).count(),
            4
        );
        assert!(p[3].ends_with('.') && p[3].starts_with(char::is_uppercase));
        assert!(chrono::DateTime::parse_from_rfc3339(p[4]).unwrap() < chrono::Utc::now());
        assert!(p[5] == "true" || p[5] == "false");
        // 清单里每个名字都真能生成
        for (name, _) in DYNAMIC_VARS {
            assert!(dynamic(name).is_some(), "{name} not generated");
        }
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
        req.auth = Auth::Bearer {
            token: "{{token}}".into(),
        };
        let (out, missing) = substitute_request(&req, &vars());
        assert_eq!(out.url, "https://api.dev/login");
        assert_eq!(out.headers[0].value, "{{mode}}");
        assert_eq!(out.body, "{\"t\":\"abc\"}");
        assert_eq!(
            out.auth,
            Auth::Bearer {
                token: "abc".into()
            }
        );
        assert_eq!(missing, vec!["mode".to_string()]);
        // 原请求不被修改
        assert_eq!(req.url, "{{base_url}}/login");
    }
}
