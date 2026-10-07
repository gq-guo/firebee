use std::sync::Arc;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::core::models::{Auth, BodyType, Request, ResponseMeta};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("Request timed out ({0:?})")]
    Timeout(Duration),
    #[error("Request cancelled")]
    Cancelled,
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),
    #[error("{0}")]
    InvalidBody(String),
    #[error("Network error: {0}")]
    Network(String),
}

/// 由请求的 url + 启用的 params + query 型 ApiKey 构造最终 URL。
pub fn build_url(req: &Request) -> Result<String, HttpError> {
    let mut url =
        reqwest::Url::parse(&req.url).map_err(|_| HttpError::InvalidUrl(req.url.clone()))?;
    {
        let mut qp = url.query_pairs_mut();
        for p in req.params.iter().filter(|p| p.enabled && !p.key.is_empty()) {
            qp.append_pair(&p.key, &p.value);
        }
        if let Auth::ApiKey {
            key,
            value,
            in_query: true,
        } = &req.auth
        {
            if !key.is_empty() {
                qp.append_pair(key, value);
            }
        }
    }
    // query_pairs_mut 在没有 append 时会留下空的 "?"，去掉
    if url.query() == Some("") {
        url.set_query(None);
    }
    Ok(url.to_string())
}

/// 执行 HTTP 请求。取消方式：向 cancel watch channel 发送 true（底层 future 被 drop，请求中断）。
/// jar：跨请求共享的 Cookie 罐（None 则不保存 Cookie）。
pub async fn execute(
    req: &Request,
    timeout: Duration,
    cancel: tokio::sync::watch::Receiver<bool>,
    jar: Option<Arc<reqwest::cookie::Jar>>,
    follow_redirects: bool,
    insecure: bool,
) -> Result<ResponseMeta, HttpError> {
    execute_streaming(req, timeout, cancel, jar, follow_redirects, insecure, None).await
}

/// 流式响应（SSE / NDJSON）边收边推给界面的事件
pub enum StreamEvent {
    /// 响应头已到：状态码 + 头
    Start(u16, Vec<(String, String)>),
    Chunk(Vec<u8>),
}

pub type OnStream = Box<dyn Fn(StreamEvent) + Send + Sync>;

fn is_streaming(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("content-type")
            && (v.starts_with("text/event-stream") || v.starts_with("application/x-ndjson"))
    })
}

/// execute 的完整版：on_stream 给了且响应是 text/event-stream / x-ndjson 时，
/// 每个 chunk 到达即回调；最终返回值仍是完整响应（历史、Capture 不用改）。
#[allow(clippy::too_many_arguments)]
pub async fn execute_streaming(
    req: &Request,
    timeout: Duration,
    cancel: tokio::sync::watch::Receiver<bool>,
    jar: Option<Arc<reqwest::cookie::Jar>>,
    follow_redirects: bool,
    insecure: bool,
    on_stream: Option<OnStream>,
) -> Result<ResponseMeta, HttpError> {
    let url = build_url(req)?;
    // 跟过的重定向记下来给界面显示 —— 否则"到底跳去哪了"完全看不见
    let trail: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    // referer(false)：reqwest 默认在重定向时带上 Referer，且只剥掉用户名/密码/fragment，
    // query 原样保留 —— ApiKey in_query 的密钥会被 302 的目标站点收到。
    //
    // 重定向只跟同一 host，且不跟 https→http 的降级（端口变化和 http→https 升级照跟）。
    // reqwest 换 host 时只剥 Authorization / Cookie，
    // X-API-Key 这类自定义鉴权头会原样发给新 host；一个被控制的接口用 302
    // 就能把密钥取走。跨 host 时停下来，把 3xx 和 Location 交给用户自己看。
    let rec = trail.clone();
    // insecure：跳过证书校验，自签名 / 内网测试环境用；默认关
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .danger_accept_invalid_certs(insecure)
        .referer(false)
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if !follow_redirects {
                return attempt.stop();
            }
            if attempt.previous().len() >= 10 {
                return attempt.error("too many redirects");
            }
            let Some(prev) = attempt.previous().last() else {
                return attempt.stop();
            };
            // 换 host 要停（见上面的注释）；https→http 也要停 ——
            // 同一个 host 也不能把 X-API-Key 降级成明文发出去
            if prev.host_str() != attempt.url().host_str()
                || (prev.scheme() == "https" && attempt.url().scheme() != "https")
            {
                return attempt.stop();
            }
            rec.lock().unwrap().push(attempt.url().to_string());
            attempt.follow()
        }));
    if let Some(jar) = jar {
        builder = builder.cookie_provider(jar);
    }
    let client = builder
        .build()
        .map_err(|e| HttpError::Network(e.to_string()))?;
    let m = req.effective_method().as_str();
    let method =
        reqwest::Method::from_bytes(m.as_bytes()).map_err(|_| HttpError::InvalidUrl(m.into()))?;

    let mut builder = client.request(method, url);
    for h in req
        .headers
        .iter()
        .filter(|h| h.enabled && !h.key.is_empty())
    {
        builder = builder.header(&h.key, &h.value);
    }
    match &req.auth {
        Auth::Bearer { token } => builder = builder.bearer_auth(token),
        Auth::Basic { username, password } => {
            builder = builder.basic_auth(username, Some(password))
        }
        Auth::ApiKey {
            key,
            value,
            in_query: false,
        } if !key.is_empty() => {
            builder = builder.header(key, value);
        }
        _ => {}
    }
    match &req.body_type {
        BodyType::Json => {
            builder = builder.body(req.body.clone());
        }
        BodyType::Text => {
            if !req.body.is_empty() {
                builder = builder.body(req.body.clone());
            }
        }
        BodyType::Form => {
            let form: Vec<(String, String)> = req
                .form
                .iter()
                .filter(|f| f.enabled && !f.key.is_empty())
                .map(|f| (f.key.clone(), f.value.clone()))
                .collect();
            builder = builder.form(&form);
        }
        BodyType::Multipart => {
            let mut form = reqwest::multipart::Form::new();
            for f in req.form.iter().filter(|f| f.enabled && !f.key.is_empty()) {
                form = if f.is_file {
                    form.part(f.key.clone(), file_part(&f.value).await?)
                } else {
                    form.text(f.key.clone(), f.value.clone())
                };
            }
            builder = builder.multipart(form);
        }
        BodyType::Binary => {
            if !req.body.trim().is_empty() {
                builder = builder.body(read_file(req.body.trim()).await?);
            }
        }
        BodyType::GraphQL => {
            builder = builder.body(req.graphql_payload().map_err(HttpError::InvalidBody)?);
        }
        BodyType::None => {}
    }
    // reqwest 的 header() 是追加：用户自己填了 Content-Type 就不再加默认的
    if matches!(req.body_type, BodyType::Json | BodyType::GraphQL)
        && !req.has_header("content-type")
    {
        builder = builder.header("Content-Type", "application/json");
    }

    let start = Instant::now();
    let send = builder.send();
    tokio::pin!(send);
    let mut resp = tokio::select! {
        r = &mut send => r.map_err(|e| map_reqwest_err(&e, timeout))?,
        _ = wait_cancelled(cancel.clone()) => return Err(HttpError::Cancelled),
    };
    let status = resp.status().as_u16();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let bytes: Vec<u8> = match on_stream.filter(|_| is_streaming(&headers)) {
        None => resp
            .bytes()
            .await
            .map_err(|e| HttpError::Network(e.to_string()))?
            .to_vec(),
        Some(cb) => {
            cb(StreamEvent::Start(status, headers.clone()));
            let mut buf = Vec::new();
            loop {
                let next = tokio::select! {
                    c = resp.chunk() => c.map_err(|e| HttpError::Network(e.to_string()))?,
                    _ = wait_cancelled(cancel.clone()) => return Err(HttpError::Cancelled),
                };
                let Some(c) = next else { break };
                buf.extend_from_slice(&c);
                cb(StreamEvent::Chunk(c.to_vec()));
            }
            buf
        }
    };
    let duration_ms = start.elapsed().as_millis();
    let redirects = std::mem::take(&mut *trail.lock().unwrap());
    Ok(ResponseMeta {
        status,
        headers,
        size_bytes: bytes.len(),
        body: bytes,
        duration_ms,
        redirects,
    })
}

async fn read_file(path: &str) -> Result<Vec<u8>, HttpError> {
    tokio::fs::read(path)
        .await
        .map_err(|e| HttpError::InvalidBody(format!("{path}: {e}")))
}

/// 文件 part：文件名取路径最后一段，MIME 按扩展名猜（猜不到就 application/octet-stream）
async fn file_part(path: &str) -> Result<reqwest::multipart::Part, HttpError> {
    let bytes = read_file(path).await?;
    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    Ok(reqwest::multipart::Part::bytes(bytes).file_name(name))
}

#[cfg(test)]
mod multipart_tests {
    use super::*;
    use crate::core::models::{HttpMethod, KeyValue};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn multipart_sends_text_and_file_parts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/up"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let fp = dir.path().join("hello.txt");
        std::fs::write(&fp, b"hi there").unwrap();
        let mut r = Request::new("up");
        r.method = HttpMethod::Post;
        r.url = format!("{}/up", server.uri());
        r.body_type = BodyType::Multipart;
        r.form = vec![KeyValue::new("name", "bob"), {
            let mut f = KeyValue::new("doc", fp.to_string_lossy());
            f.is_file = true;
            f
        }];
        let (tx, rx) = tokio::sync::watch::channel(false);
        drop(tx);
        execute(&r, Duration::from_secs(5), rx, None, true, false)
            .await
            .unwrap();
        let req = &server.received_requests().await.unwrap()[0];
        let ct = req.headers.get("content-type").unwrap().to_str().unwrap();
        assert!(ct.starts_with("multipart/form-data; boundary="), "{ct}");
        let body = String::from_utf8_lossy(&req.body);
        assert!(body.contains("name=\"name\"\r\n\r\nbob"), "{body}");
        assert!(
            body.contains("name=\"doc\"; filename=\"hello.txt\""),
            "{body}"
        );
        assert!(body.contains("hi there"), "{body}");
    }

    #[tokio::test]
    async fn sse_response_streams_chunks_and_still_returns_full_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sse"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("data: a\n\ndata: b\n\n", "text/event-stream"),
            )
            .mount(&server)
            .await;
        let mut r = Request::new("sse");
        r.url = format!("{}/sse", server.uri());
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let s2 = seen.clone();
        let cb: OnStream = Box::new(move |ev| {
            s2.lock().unwrap().push(match ev {
                StreamEvent::Start(st, _) => format!("start {st}"),
                StreamEvent::Chunk(c) => String::from_utf8_lossy(&c).into_owned(),
            })
        });
        let (tx, rx) = tokio::sync::watch::channel(false);
        drop(tx);
        let resp = execute_streaming(&r, Duration::from_secs(5), rx, None, true, false, Some(cb))
            .await
            .unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen.first().map(String::as_str),
            Some("start 200"),
            "headers={:?}",
            resp.headers
        );
        assert_eq!(seen[1..].concat(), "data: a\n\ndata: b\n\n");
        assert_eq!(resp.body, b"data: a\n\ndata: b\n\n");
    }

    #[tokio::test]
    async fn binary_missing_file_is_invalid_body() {
        let mut r = Request::new("b");
        r.method = HttpMethod::Post;
        r.url = "http://127.0.0.1:1/x".into();
        r.body_type = BodyType::Binary;
        r.body = "/nonexistent/file.bin".into();
        let (tx, rx) = tokio::sync::watch::channel(false);
        drop(tx);
        let err = execute(&r, Duration::from_secs(5), rx, None, true, false)
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::InvalidBody(_)), "{err:?}");
    }
}

async fn wait_cancelled(mut rx: tokio::sync::watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            // sender 已 drop（不可能取消），永不返回，让 select 走 send 分支
            std::future::pending::<()>().await;
        }
    }
}

fn map_reqwest_err(e: &reqwest::Error, timeout: Duration) -> HttpError {
    if e.is_timeout() {
        HttpError::Timeout(timeout)
    } else if e.is_connect() {
        HttpError::Network(
            "Cannot connect to server (connection refused or DNS failure)".to_string(),
        )
    } else {
        HttpError::Network(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::{Auth, BodyType, KeyValue, Request};
    use std::time::Duration;
    use wiremock::matchers::{body_string, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn no_cancel() -> tokio::sync::watch::Receiver<bool> {
        let (_tx, rx) = tokio::sync::watch::channel(false);
        rx
    }

    #[tokio::test]
    async fn get_with_params_and_headers() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users"))
            .and(query_param("page", "1"))
            .and(header("x-token", "abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&server)
            .await;

        let mut req = Request::new("t");
        req.url = format!("{}/users", server.uri());
        req.params = vec![KeyValue::new("page", "1")];
        req.headers = vec![KeyValue::new("x-token", "abc")];
        let resp = execute(&req, Duration::from_secs(5), no_cancel(), None, true, false)
            .await
            .unwrap();
        assert_eq!(resp.status, 200);
        assert!(String::from_utf8(resp.body)
            .unwrap()
            .contains("\"ok\":true"));
        assert!(resp.size_bytes > 0);
    }

    #[tokio::test]
    async fn post_json_body_and_bearer() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/login"))
            .and(header("authorization", "Bearer t123"))
            .and(header("content-type", "application/json"))
            .and(body_string(r#"{"u":"a"}"#))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;

        let mut req = Request::new("t");
        req.method = crate::core::models::HttpMethod::Post;
        req.url = format!("{}/login", server.uri());
        req.body_type = BodyType::Json;
        req.body = r#"{"u":"a"}"#.into();
        req.auth = Auth::Bearer {
            token: "t123".into(),
        };
        let resp = execute(&req, Duration::from_secs(5), no_cancel(), None, true, false)
            .await
            .unwrap();
        assert_eq!(resp.status, 201);
    }

    #[tokio::test]
    async fn graphql_posts_json_payload() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(header("content-type", "application/json"))
            .and(body_string(
                r#"{"query":"{ me { id } }","variables":{"a":1}}"#,
            ))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let mut req = Request::new("t");
        req.url = format!("{}/graphql", server.uri());
        req.body_type = BodyType::GraphQL;
        req.method = crate::core::models::HttpMethod::Get; // 仍按 POST 发
        req.headers = vec![KeyValue::new("Content-Type", "application/json")];
        req.body = "{ me { id } }".into();
        req.graphql_variables = r#"{"a": 1}"#.into();
        let resp = execute(&req, Duration::from_secs(5), no_cancel(), None, true, false)
            .await
            .unwrap();
        assert_eq!(resp.status, 200);
    }

    #[tokio::test]
    async fn api_key_in_query() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/data"))
            .and(query_param("api_key", "k9"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let mut req = Request::new("t");
        req.url = format!("{}/data", server.uri());
        req.auth = Auth::ApiKey {
            key: "api_key".into(),
            value: "k9".into(),
            in_query: true,
        };
        let resp = execute(&req, Duration::from_secs(5), no_cancel(), None, true, false)
            .await
            .unwrap();
        assert_eq!(resp.status, 200);
    }

    #[tokio::test]
    async fn timeout_maps_to_timeout_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
            .mount(&server)
            .await;

        let mut req = Request::new("t");
        req.url = server.uri();
        let err = execute(
            &req,
            Duration::from_millis(100),
            no_cancel(),
            None,
            true,
            false,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, HttpError::Timeout(_)));
    }

    #[tokio::test]
    async fn cancelled_before_send() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
            .mount(&server)
            .await;

        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut req = Request::new("t");
        req.url = server.uri();
        let handle = tokio::spawn(async move {
            execute(&req, Duration::from_secs(10), rx, None, true, false).await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.send(true).unwrap();
        let err = handle.await.unwrap().unwrap_err();
        assert!(matches!(err, HttpError::Cancelled));
    }

    #[tokio::test]
    async fn invalid_url_error() {
        let mut req = Request::new("t");
        req.url = "not a url".into();
        let err = execute(&req, Duration::from_secs(5), no_cancel(), None, true, false)
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::InvalidUrl(_)));
    }

    #[tokio::test]
    async fn connection_refused_is_network_error() {
        let mut req = Request::new("t");
        req.url = "http://127.0.0.1:1/".into();
        let err = execute(&req, Duration::from_secs(2), no_cancel(), None, true, false)
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::Network(_)));
    }

    #[tokio::test]
    async fn cookie_jar_is_shared_across_requests() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/login"))
            .respond_with(ResponseTemplate::new(200).insert_header("set-cookie", "sid=abc; Path=/"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/me"))
            .and(header("cookie", "sid=abc"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let jar = Arc::new(reqwest::cookie::Jar::default());
        let mut login = Request::new("l");
        login.url = format!("{}/login", server.uri());
        execute(
            &login,
            Duration::from_secs(5),
            no_cancel(),
            Some(jar.clone()),
            true,
            false,
        )
        .await
        .unwrap();
        let mut me = Request::new("m");
        me.url = format!("{}/me", server.uri());
        let resp = execute(
            &me,
            Duration::from_secs(5),
            no_cancel(),
            Some(jar),
            true,
            false,
        )
        .await
        .unwrap();
        assert_eq!(resp.status, 200);
    }

    #[tokio::test]
    async fn same_host_redirect_is_followed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/old"))
            .respond_with(ResponseTemplate::new(301).insert_header("location", "/new"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/new"))
            .respond_with(ResponseTemplate::new(200).set_body_string("arrived"))
            .mount(&server)
            .await;
        let mut r = Request::new("r");
        r.url = format!("{}/old", server.uri());
        let resp = execute(&r, Duration::from_secs(5), no_cancel(), None, true, false)
            .await
            .unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(String::from_utf8(resp.body).unwrap(), "arrived");
    }

    #[tokio::test]
    async fn redirect_trail_is_recorded_and_can_be_turned_off() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/a"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/b"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/b"))
            .respond_with(ResponseTemplate::new(301).insert_header("location", "/c"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/c"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let mut r = Request::new("r");
        r.url = format!("{}/a", server.uri());

        let followed = execute(&r, Duration::from_secs(5), no_cancel(), None, true, false)
            .await
            .unwrap();
        assert_eq!(followed.status, 200);
        assert_eq!(followed.redirects.len(), 2, "跟过的每一跳都要记下来");
        assert!(followed.redirects[0].ends_with("/b"));
        assert!(followed.redirects[1].ends_with("/c"));

        // 关掉之后停在第一个 3xx，界面自己显示 Location
        let stopped = execute(&r, Duration::from_secs(5), no_cancel(), None, false, false)
            .await
            .unwrap();
        assert_eq!(stopped.status, 302);
        assert!(stopped.redirects.is_empty());
    }

    #[tokio::test]
    async fn cross_host_redirect_stops() {
        // 被控制的接口用 302 指向别的 host：X-API-Key 这类自定义头不能跟过去。
        // 目标用不可解析的域名 —— 真跟过去就会是 DNS 错误，而不是拿到 302。
        let api = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "http://evil.invalid/steal"),
            )
            .mount(&api)
            .await;
        let mut r = Request::new("r");
        r.url = api.uri();
        r.headers = vec![KeyValue::new("X-API-Key", "super-secret")];
        let resp = execute(&r, Duration::from_secs(5), no_cancel(), None, true, false)
            .await
            .unwrap();
        assert_eq!(resp.status, 302, "跨 host 的重定向不应该被跟随");
        assert!(resp
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("location") && v.contains("evil.invalid")));
    }

    #[test]
    fn build_url_appends_enabled_params_only() {
        let mut req = Request::new("t");
        req.url = "https://api.dev/users".into();
        req.params = vec![
            KeyValue::new("a", "1"),
            KeyValue {
                enabled: false,
                key: "b".into(),
                value: "2".into(),
                is_file: false,
            },
            KeyValue::new("", "skip"),
        ];
        assert_eq!(build_url(&req).unwrap(), "https://api.dev/users?a=1");
    }

    #[test]
    fn build_url_without_params_has_no_trailing_question_mark() {
        let mut req = Request::new("t");
        req.url = "https://api.dev/users/".into();
        assert_eq!(build_url(&req).unwrap(), "https://api.dev/users/");
    }
}
