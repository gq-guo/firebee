//! 会话 Cookie 罐：reqwest 自带的 Jar 不能列出 / 单删，这里用 cookie_store 自己实现一遍
//! reqwest::cookie::CookieStore，界面上才有 Cookie 管理。

use std::sync::RwLock;

use reqwest::header::HeaderValue;

#[derive(Default)]
pub struct Cookies(pub RwLock<cookie_store::CookieStore>);

/// 给界面看的一条 cookie
#[derive(serde::Serialize, Debug, PartialEq)]
pub struct CookieView {
    pub domain: String,
    pub path: String,
    pub name: String,
    pub value: String,
    /// RFC3339；会话 cookie 为 None
    pub expires: Option<String>,
    pub secure: bool,
    pub http_only: bool,
}

fn domain_of(c: &cookie_store::Cookie<'_>) -> String {
    match &c.domain {
        cookie_store::CookieDomain::HostOnly(s) | cookie_store::CookieDomain::Suffix(s) => {
            s.clone()
        }
        _ => String::new(),
    }
}

impl Cookies {
    pub fn list(&self) -> Vec<CookieView> {
        let s = self.0.read().unwrap();
        let mut v: Vec<CookieView> = s
            .iter_unexpired()
            .map(|c| CookieView {
                domain: domain_of(c),
                path: c.path.as_ref().to_string(),
                name: c.name().to_string(),
                value: c.value().to_string(),
                expires: match &c.expires {
                    cookie_store::CookieExpiration::AtUtc(t) => {
                        chrono::DateTime::from_timestamp(t.unix_timestamp(), 0)
                            .map(|d| d.to_rfc3339())
                    }
                    cookie_store::CookieExpiration::SessionEnd => None,
                },
                secure: c.secure().unwrap_or(false),
                http_only: c.http_only().unwrap_or(false),
            })
            .collect();
        v.sort_by(|a, b| (&a.domain, &a.path, &a.name).cmp(&(&b.domain, &b.path, &b.name)));
        v
    }

    pub fn remove(&self, domain: &str, path: &str, name: &str) -> bool {
        self.0.write().unwrap().remove(domain, path, name).is_some()
    }

    pub fn clear(&self) {
        self.0.write().unwrap().clear();
    }
}

impl reqwest::cookie::CookieStore for Cookies {
    fn set_cookies(&self, headers: &mut dyn Iterator<Item = &HeaderValue>, url: &reqwest::Url) {
        let parsed = headers
            .filter_map(|v| std::str::from_utf8(v.as_bytes()).ok())
            .filter_map(|s| cookie_store::RawCookie::parse(s).ok())
            .map(|c| c.into_owned());
        self.0.write().unwrap().store_response_cookies(parsed, url);
    }

    fn cookies(&self, url: &reqwest::Url) -> Option<HeaderValue> {
        // 底层是 HashMap，顺序不稳；排一下让 Preview / 导出每次一样
        let mut pairs: Vec<String> = self
            .0
            .read()
            .unwrap()
            .get_request_values(url)
            .map(|(n, v)| format!("{n}={v}"))
            .collect();
        pairs.sort();
        let s = pairs.join("; ");
        if s.is_empty() {
            None
        } else {
            HeaderValue::from_str(&s).ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::cookie::CookieStore as _;

    #[test]
    fn store_list_remove() {
        let c = Cookies::default();
        let url = reqwest::Url::parse("https://api.example.com/login").unwrap();
        let hv = [
            HeaderValue::from_static("sid=abc; Path=/; HttpOnly"),
            HeaderValue::from_static("theme=dark; Path=/; Max-Age=3600"),
        ];
        c.set_cookies(&mut hv.iter(), &url);
        let list = c.list();
        assert_eq!(list.len(), 2);
        assert_eq!(
            (
                list[0].name.as_str(),
                list[0].domain.as_str(),
                list[0].http_only
            ),
            ("sid", "api.example.com", true)
        );
        assert!(list[1].expires.is_some() && list[0].expires.is_none());
        assert_eq!(c.cookies(&url).unwrap(), "sid=abc; theme=dark");
        assert!(c.remove("api.example.com", "/", "sid"));
        assert_eq!(c.cookies(&url).unwrap(), "theme=dark");
        c.clear();
        assert!(c.cookies(&url).is_none());
    }
}
