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
        if let Auth::ApiKey { key, value, in_query: true } = &req.auth {
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
pub async fn execute(
    req: &Request,
    timeout: Duration,
    cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<ResponseMeta, HttpError> {
    let url = build_url(req)?;
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| HttpError::Network(e.to_string()))?;
    let method = reqwest::Method::from_bytes(req.method.as_str().as_bytes())
        .map_err(|_| HttpError::InvalidUrl(req.method.as_str().to_string()))?;

    let mut builder = client.request(method, url);
    for h in req.headers.iter().filter(|h| h.enabled && !h.key.is_empty()) {
        builder = builder.header(&h.key, &h.value);
    }
    match &req.auth {
        Auth::Bearer { token } => builder = builder.bearer_auth(token),
        Auth::Basic { username, password } => {
            builder = builder.basic_auth(username, Some(password))
        }
        Auth::ApiKey { key, value, in_query: false } if !key.is_empty() => {
            builder = builder.header(key, value);
        }
        _ => {}
    }
    match &req.body_type {
        BodyType::Json => {
            builder = builder
                .header("Content-Type", "application/json")
                .body(req.body.clone());
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
        BodyType::None => {}
    }

    let start = Instant::now();
    let send = builder.send();
    tokio::pin!(send);
    let resp = tokio::select! {
        r = &mut send => r.map_err(|e| map_reqwest_err(&e, timeout))?,
        _ = wait_cancelled(cancel) => return Err(HttpError::Cancelled),
    };
    let status = resp.status().as_u16();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| HttpError::Network(e.to_string()))?;
    let duration_ms = start.elapsed().as_millis();
    Ok(ResponseMeta {
        status,
        headers,
        size_bytes: bytes.len(),
        body: bytes.to_vec(),
        duration_ms,
    })
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
        HttpError::Network("Cannot connect to server (connection refused or DNS failure)".to_string())
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
        let resp = execute(&req, Duration::from_secs(5), no_cancel()).await.unwrap();
        assert_eq!(resp.status, 200);
        assert!(String::from_utf8(resp.body).unwrap().contains("\"ok\":true"));
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
        req.auth = Auth::Bearer { token: "t123".into() };
        let resp = execute(&req, Duration::from_secs(5), no_cancel()).await.unwrap();
        assert_eq!(resp.status, 201);
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
        req.auth = Auth::ApiKey { key: "api_key".into(), value: "k9".into(), in_query: true };
        let resp = execute(&req, Duration::from_secs(5), no_cancel()).await.unwrap();
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
        let err = execute(&req, Duration::from_millis(100), no_cancel()).await.unwrap_err();
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
        let handle = tokio::spawn(async move { execute(&req, Duration::from_secs(10), rx).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.send(true).unwrap();
        let err = handle.await.unwrap().unwrap_err();
        assert!(matches!(err, HttpError::Cancelled));
    }

    #[tokio::test]
    async fn invalid_url_error() {
        let mut req = Request::new("t");
        req.url = "not a url".into();
        let err = execute(&req, Duration::from_secs(5), no_cancel()).await.unwrap_err();
        assert!(matches!(err, HttpError::InvalidUrl(_)));
    }

    #[tokio::test]
    async fn connection_refused_is_network_error() {
        let mut req = Request::new("t");
        req.url = "http://127.0.0.1:1/".into();
        let err = execute(&req, Duration::from_secs(2), no_cancel()).await.unwrap_err();
        assert!(matches!(err, HttpError::Network(_)));
    }

    #[test]
    fn build_url_appends_enabled_params_only() {
        let mut req = Request::new("t");
        req.url = "https://api.dev/users".into();
        req.params = vec![
            KeyValue::new("a", "1"),
            KeyValue { enabled: false, key: "b".into(), value: "2".into() },
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
