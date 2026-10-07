use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl HttpMethod {
    pub const ALL: [HttpMethod; 7] = [
        Self::Get,
        Self::Post,
        Self::Put,
        Self::Delete,
        Self::Patch,
        Self::Head,
        Self::Options,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Patch => "PATCH",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValue {
    pub enabled: bool,
    pub key: String,
    pub value: String,
    /// 仅 Multipart body 用：true 时 value 是本机文件路径，作为文件 part 发送
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_file: bool,
    /// 仅环境变量用：值存钥匙串，不进 JSON / 导出 / 项目目录；界面遮罩
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secret: bool,
}

impl KeyValue {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            enabled: true,
            key: key.into(),
            value: value.into(),
            is_file: false,
            secret: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BodyType {
    None,
    Json,
    Text,
    Form,
    /// multipart/form-data：form 里 is_file 的行按文件发
    Multipart,
    /// body 是本机文件路径，整个文件作为 body 原样发送
    Binary,
    /// body 是 GraphQL query，变量在 graphql_variables；发送时组装成 JSON
    GraphQL,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Auth {
    /// 请求上 = 跟随集合 / 文件夹；容器上 = 这一层不提供，继续往外层找。
    /// 两种情况下都找不到，就是不带认证。
    #[default]
    None,
    /// 只能出现在请求上：明确不带任何凭据，连继承来的 Authorization 头一起丢掉。
    /// 集合配了 prod token，里面又要放一个打第三方接口的请求时用这个。
    Off,
    Bearer {
        token: String,
    },
    Basic {
        username: String,
        password: String,
    },
    /// in_query=true 时放 query string，否则放 header
    ApiKey {
        key: String,
        value: String,
        in_query: bool,
    },
    /// OAuth 2.0 Client Credentials：发送前先去 token_url 换 access_token，当 Bearer 用。
    /// token 只在内存里缓存到过期，不进存档。
    OAuth2 {
        token_url: String,
        client_id: String,
        client_secret: String,
        scope: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: Uuid,
    pub name: String,
    pub method: HttpMethod,
    pub url: String,
    pub params: Vec<KeyValue>,
    pub headers: Vec<KeyValue>,
    pub body_type: BodyType,
    pub body: String,
    pub form: Vec<KeyValue>,
    pub auth: Auth,
    /// 置顶到侧栏 Pinned 区；老的存档文件没有这个字段
    #[serde(default)]
    pub pinned: bool,
    /// 响应后把值提取到环境变量：key = 变量名，value = JSONPath（复用 KeyValue，前端表格直接沿用）
    #[serde(default)]
    pub captures: Vec<KeyValue>,
    /// GraphQL 变量（JSON 文本），仅 body_type == GraphQL 时使用
    #[serde(default)]
    pub graphql_variables: String,
}

impl Request {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            method: HttpMethod::Get,
            url: String::new(),
            params: vec![],
            headers: vec![],
            body_type: BodyType::None,
            body: String::new(),
            form: vec![],
            auth: Auth::None,
            pinned: false,
            graphql_variables: String::new(),
            captures: vec![],
        }
    }

    /// GraphQL 一律 POST（存档里的 method 可能来自 Postman 导入的 GET，界面上又不可见）
    pub fn effective_method(&self) -> HttpMethod {
        if self.body_type == BodyType::GraphQL {
            HttpMethod::Post
        } else {
            self.method
        }
    }

    /// 用户是否已启用某个 header（大小写不敏感），用来避免重复加默认头
    pub fn has_header(&self, name: &str) -> bool {
        self.headers
            .iter()
            .any(|h| h.enabled && h.key.trim().eq_ignore_ascii_case(name))
    }

    /// GraphQL 请求体：{"query": body, "variables": {...}}；变量为空则省略，非法 JSON 报错
    pub fn graphql_payload(&self) -> Result<String, String> {
        let mut doc = serde_json::json!({ "query": self.body });
        if !self.graphql_variables.trim().is_empty() {
            let vars: serde_json::Value = serde_json::from_str(&self.graphql_variables)
                .map_err(|e| format!("GraphQL variables aren't valid JSON: {e}"))?;
            doc["variables"] = vars;
        }
        Ok(doc.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: Uuid,
    pub name: String,
    pub folders: Vec<Folder>,
    pub requests: Vec<Request>,
    /// 下属请求共用的 header；请求自己同名的覆盖它
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    /// 下属请求的默认认证；请求自己设了非 None 就用自己的
    #[serde(default)]
    pub auth: Auth,
}

impl Folder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            folders: vec![],
            requests: vec![],
            headers: vec![],
            auth: Auth::None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub id: Uuid,
    pub name: String,
    pub folders: Vec<Folder>,
    pub requests: Vec<Request>,
    /// 下属请求共用的 header；请求自己同名的覆盖它
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    /// 下属请求的默认认证；请求自己设了非 None 就用自己的
    #[serde(default)]
    pub auth: Auth,
    /// 关联的项目目录（见 core::project）：保存时同步写进去，启动时从那里读回来
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_dir: Option<String>,
}

impl Collection {
    /// 给整棵树换一套新 id。导入 Firebee 自己导出的文件时必须换：
    /// 前端的标签页、响应缓存、折叠状态全按 request.id 索引，两份同 id 的请求
    /// 会让 findById 永远返回第一份 —— 你在编辑 B，界面却把 A 认成当前请求。
    pub fn reid(&mut self) {
        fn walk(folders: &mut [Folder], requests: &mut [Request]) {
            for r in requests.iter_mut() {
                r.id = Uuid::new_v4();
            }
            for f in folders.iter_mut() {
                f.id = Uuid::new_v4();
                let (sub, reqs) = (&mut f.folders, &mut f.requests);
                walk(sub, reqs);
            }
        }
        self.id = Uuid::new_v4();
        walk(&mut self.folders, &mut self.requests);
    }

    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            folders: vec![],
            requests: vec![],
            headers: vec![],
            auth: Auth::None,
            project_dir: None,
        }
    }
}

/// 请求所在集合 / 文件夹链上的公共配置，前端按 外层→内层 的顺序传过来
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Inherited {
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    #[serde(default)]
    pub auth: Auth,
}

/// 把集合 / 文件夹链上的公共 header 与 auth 合进请求。
/// header：越靠内层越优先，同名（忽略大小写）整组覆盖外层；请求自己的最优先。
/// auth：请求自己设了非 None 就用自己的，否则取最内层非 None 的那个。
pub fn merge_inherited(req: &Request, chain: &[Inherited]) -> Request {
    let is_auth_header = |k: &str| k.trim().eq_ignore_ascii_case("authorization");
    // 明确的凭据头。Off 时连这些一起丢；自定义的（X-API-Key 之类）认不出来，
    // ponytail: 只处理标准的三个，其余靠在请求上覆盖同名头
    let is_credential_header = |k: &str| {
        matches!(
            k.trim().to_ascii_lowercase().as_str(),
            "authorization" | "cookie" | "proxy-authorization"
        )
    };

    let mut out = req.clone();
    let mut headers: Vec<KeyValue> = vec![];
    for src in chain
        .iter()
        .map(|i| &i.headers)
        .chain(std::iter::once(&req.headers))
    {
        let enabled: Vec<KeyValue> = src
            .iter()
            .filter(|h| h.enabled && !h.key.trim().is_empty())
            // key 存成 trim 过的：带空格的名字 reqwest 会当成非法 header name，
            // 报出来只是一句 "builder error"，用户根本看不出是哪一行
            .map(|h| KeyValue {
                enabled: true,
                key: h.key.trim().to_string(),
                value: h.value.clone(),
                is_file: false,
                secret: false,
            })
            .collect();
        headers.retain(|kept| {
            !enabled
                .iter()
                .any(|h| h.key.eq_ignore_ascii_case(&kept.key))
        });
        headers.extend(enabled);
    }
    // 请求自己在 Headers 里敲的 Authorization —— 最明确的意图，永远保留
    let own_auth_header = req
        .headers
        .iter()
        .any(|h| h.enabled && is_auth_header(&h.key));

    if out.auth == Auth::Off {
        // 明确不带凭据：不继承 auth，继承来的标准凭据头也一起丢
        out.auth = Auth::None;
        headers.retain(|h| {
            !is_credential_header(&h.key) || (own_auth_header && is_auth_header(&h.key))
        });
        out.headers = headers;
        return out;
    }

    let inherited_auth = (out.auth == Auth::None)
        .then(|| {
            chain
                .iter()
                .rev()
                .map(|i| &i.auth)
                .find(|a| **a != Auth::None)
        })
        .flatten()
        .cloned();
    if own_auth_header {
        // 手写的 Authorization 头已经说明了要带什么，就别再把上层的凭据叠上去
        // （reqwest 的 header() 是追加，叠上去会发出两个 Authorization）
        out.headers = headers;
        return out;
    }
    if let Some(a) = inherited_auth {
        out.auth = a;
    }
    // auth 生效时丢掉继承来的 Authorization，同样是为了不发出两个
    if effective(&out.auth) {
        headers.retain(|h| !is_auth_header(&h.key));
    }
    out.headers = headers;
    out
}

/// auth 是否真的会往请求上加东西。ApiKey 的 key 为空时 http 层会跳过，
/// 这里也得算"没生效"，否则会白白丢掉继承来的 Authorization 却什么都不加
fn effective(a: &Auth) -> bool {
    match a {
        Auth::None | Auth::Off => false,
        Auth::ApiKey { key, .. } => !key.trim().is_empty(),
        _ => true,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub id: Uuid,
    pub name: String,
    pub variables: Vec<KeyValue>,
}

impl Environment {
    pub fn var_map(&self) -> std::collections::HashMap<String, String> {
        self.variables
            .iter()
            .filter(|v| v.enabled && !v.key.is_empty())
            .map(|v| (v.key.clone(), v.value.clone()))
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseMeta {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub duration_ms: u128,
    /// 发出到收到响应头的时间；和 duration_ms 的差就是下载时间
    pub ttfb_ms: u128,
    pub size_bytes: usize,
    /// 实际跟过的重定向目标，按顺序；没有重定向就是空的
    pub redirects: Vec<String>,
}

/// 存进历史的响应：字段与前端 ResponseDto 同名，重开历史时直接喂给响应面板。
/// body 超过上限会被前端截断（truncated=true），二进制响应不存 body。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub duration_ms: u128,
    /// 老历史没有这个字段：None，界面不显示 TTFB
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u128>,
    pub size_bytes: usize,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub request: Request,
    pub status: Option<u16>,
    pub duration_ms: Option<u128>,
    /// 老的 history.json 没有这个字段
    #[serde(default)]
    pub response: Option<StoredResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_as_str_roundtrip() {
        assert_eq!(HttpMethod::Get.as_str(), "GET");
        assert_eq!(HttpMethod::Patch.as_str(), "PATCH");
        assert_eq!(HttpMethod::ALL.len(), 7);
    }

    #[test]
    fn request_serde_roundtrip() {
        let mut r = Request::new("登录");
        r.url = "https://api.example.com/login".into();
        r.method = HttpMethod::Post;
        r.headers = vec![KeyValue::new("Content-Type", "application/json")];
        r.body_type = BodyType::Json;
        r.body = r#"{"u":"a"}"#.into();
        r.auth = Auth::Bearer {
            token: "t123".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "登录");
        assert_eq!(back.method, HttpMethod::Post);
        assert_eq!(
            back.auth,
            Auth::Bearer {
                token: "t123".into()
            }
        );
    }

    #[test]
    fn request_without_pinned_field_loads() {
        // 0.1.1 之前存下来的 collections.json 里没有 pinned
        let old = r#"{"id":"00000000-0000-0000-0000-000000000001","name":"a","method":"Get",
            "url":"","params":[],"headers":[],"body_type":"None","body":"","form":[],"auth":"None"}"#;
        let r: Request = serde_json::from_str(old).unwrap();
        assert!(!r.pinned);
        assert!(r.captures.is_empty());
    }

    #[test]
    fn graphql_payload_wraps_query_and_variables() {
        let mut r = Request::new("q");
        r.body_type = BodyType::GraphQL;
        r.body = "query { me { id } }".into();
        assert_eq!(
            r.graphql_payload().unwrap(),
            r#"{"query":"query { me { id } }"}"#
        );
        r.graphql_variables = r#"{"id": 1}"#.into();
        let v: serde_json::Value = serde_json::from_str(&r.graphql_payload().unwrap()).unwrap();
        assert_eq!(v["variables"]["id"], 1);
        r.graphql_variables = "{nope".into();
        assert!(r.graphql_payload().is_err());
        // method 存的是 GET 也按 POST 发
        assert_eq!(r.effective_method(), HttpMethod::Post);
        r.headers = vec![KeyValue::new(" content-TYPE", "application/json")];
        assert!(r.has_header("Content-Type"));
    }

    fn inh(headers: &[(&str, &str)], auth: Auth) -> Inherited {
        Inherited {
            headers: headers.iter().map(|(k, v)| KeyValue::new(*k, *v)).collect(),
            auth,
        }
    }

    #[test]
    fn inherited_headers_inner_wins_request_wins_all() {
        let mut r = Request::new("r");
        r.headers = vec![KeyValue::new("X-Trace", "req")];
        let chain = [
            inh(&[("Authorization", "col"), ("X-Trace", "col")], Auth::None),
            inh(&[("X-Trace", "folder"), ("X-Only-Folder", "f")], Auth::None),
        ];
        let m = merge_inherited(&r, &chain);
        let get = |k: &str| {
            m.headers
                .iter()
                .find(|h| h.key.eq_ignore_ascii_case(k))
                .map(|h| h.value.clone())
        };
        assert_eq!(get("X-Trace").unwrap(), "req");
        assert_eq!(get("Authorization").unwrap(), "col");
        assert_eq!(get("X-Only-Folder").unwrap(), "f");
        // 同名只留一份
        assert_eq!(m.headers.iter().filter(|h| h.key == "X-Trace").count(), 1);
    }

    #[test]
    fn inherited_headers_skip_disabled_and_keep_same_source_dupes() {
        let r = Request::new("r");
        let chain = [Inherited {
            headers: vec![
                KeyValue::new("X-Tag", "a"),
                KeyValue::new("X-Tag", "b"),
                KeyValue {
                    enabled: false,
                    key: "X-Off".into(),
                    value: "x".into(),
                    is_file: false,
                    secret: false,
                },
            ],
            auth: Auth::None,
        }];
        let m = merge_inherited(&r, &chain);
        assert_eq!(m.headers.len(), 2);
        assert!(!m.headers.iter().any(|h| h.key == "X-Off"));
    }

    #[test]
    fn auth_off_sends_no_credentials_at_all() {
        // 集合有 prod token 和公共 Authorization 头；这个请求要打第三方接口，什么都不能带
        let mut r = Request::new("third-party");
        r.auth = Auth::Off;
        let chain = [inh(
            &[("Authorization", "Bearer col"), ("X-Trace", "keep-me")],
            Auth::Bearer {
                token: "prod".into(),
            },
        )];
        let m = merge_inherited(&r, &chain);
        assert_eq!(m.auth, Auth::None);
        assert!(!m
            .headers
            .iter()
            .any(|h| h.key.eq_ignore_ascii_case("authorization")));
        // 非凭据的公共头照常继承
        assert!(m.headers.iter().any(|h| h.key == "X-Trace"));
    }

    #[test]
    fn auth_off_keeps_the_request_s_own_authorization_header() {
        // 选了 No auth 又自己敲了一行 Authorization —— 那是明确意图，不能吃掉
        let mut r = Request::new("third-party");
        r.auth = Auth::Off;
        r.headers = vec![KeyValue::new("Authorization", "Basic bWluZQ==")];
        let chain = [inh(
            &[("Authorization", "Bearer col")],
            Auth::Bearer {
                token: "prod".into(),
            },
        )];
        let m = merge_inherited(&r, &chain);
        assert_eq!(m.auth, Auth::None);
        let auths: Vec<&str> = m
            .headers
            .iter()
            .filter(|h| h.key.eq_ignore_ascii_case("authorization"))
            .map(|h| h.value.as_str())
            .collect();
        assert_eq!(auths, vec!["Basic bWluZQ=="], "自己写的那行被吃掉了");
    }

    #[test]
    fn inherited_header_names_match_case_and_space_insensitively() {
        let mut r = Request::new("r");
        r.headers = vec![KeyValue::new("content-type", "text/plain")];
        let chain = [inh(&[(" Content-Type ", "application/json")], Auth::None)];
        let m = merge_inherited(&r, &chain);
        assert_eq!(
            m.headers.len(),
            1,
            "大小写/空白不同也应算同名: {:?}",
            m.headers
        );
        assert_eq!(m.headers[0].value, "text/plain");
    }

    #[test]
    fn effective_auth_drops_inherited_authorization_header() {
        // 集合配了 Authorization 头、文件夹又配了 Bearer —— 不能发出两个 Authorization
        let r = Request::new("r");
        let chain = [
            inh(&[("Authorization", "Bearer col-header")], Auth::None),
            inh(
                &[],
                Auth::Bearer {
                    token: "folder".into(),
                },
            ),
        ];
        let m = merge_inherited(&r, &chain);
        assert!(!m
            .headers
            .iter()
            .any(|h| h.key.eq_ignore_ascii_case("authorization")));
        // 请求自己手写的那一行保留（明确覆盖）
        let mut own = Request::new("r");
        own.headers = vec![KeyValue::new("Authorization", "Bearer mine")];
        let m2 = merge_inherited(&own, &chain);
        assert_eq!(
            m2.headers
                .iter()
                .filter(|h| h.key.eq_ignore_ascii_case("authorization"))
                .count(),
            1
        );
        assert_eq!(m2.headers[0].value, "Bearer mine");
        // 手写了 Authorization 就别再把上层的凭据叠上来 —— reqwest 会发出两个
        assert_eq!(m2.auth, Auth::None, "继承的 auth 和手写的头同时发出去了");
    }

    #[test]
    fn empty_api_key_does_not_strip_the_inherited_authorization() {
        // 选了 API key 但名字还没填，http 层会跳过它；这时不能把继承的凭据丢掉
        let mut r = Request::new("r");
        r.auth = Auth::ApiKey {
            key: "  ".into(),
            value: "v".into(),
            in_query: false,
        };
        let chain = [inh(&[("Authorization", "Bearer col")], Auth::None)];
        let m = merge_inherited(&r, &chain);
        assert_eq!(m.headers.len(), 1);
        assert_eq!(m.headers[0].value, "Bearer col");
    }

    #[test]
    fn auth_off_drops_inherited_cookie_but_keeps_ordinary_headers() {
        let mut r = Request::new("r");
        r.auth = Auth::Off;
        let chain = [inh(
            &[
                ("Authorization", "Bearer col"),
                ("Cookie", "sid=secret"),
                ("X-Trace", "keep"),
            ],
            Auth::None,
        )];
        let m = merge_inherited(&r, &chain);
        let names: Vec<&str> = m.headers.iter().map(|h| h.key.as_str()).collect();
        assert_eq!(names, vec!["X-Trace"]);
    }

    #[test]
    fn merged_header_keys_are_trimmed() {
        // 带空格的 header 名 reqwest 只会回一句 builder error，用户看不出是哪一行
        let r = Request::new("r");
        let chain = [inh(&[(" X-Api-Key ", "k")], Auth::None)];
        let m = merge_inherited(&r, &chain);
        assert_eq!(m.headers[0].key, "X-Api-Key");
    }

    #[test]
    fn inherited_auth_innermost_non_none_then_request() {
        let r = Request::new("r");
        let chain = [
            inh(
                &[],
                Auth::Bearer {
                    token: "col".into(),
                },
            ),
            inh(
                &[],
                Auth::Bearer {
                    token: "folder".into(),
                },
            ),
        ];
        assert_eq!(
            merge_inherited(&r, &chain).auth,
            Auth::Bearer {
                token: "folder".into()
            }
        );
        let mut own = r.clone();
        own.auth = Auth::Bearer {
            token: "own".into(),
        };
        assert_eq!(
            merge_inherited(&own, &chain).auth,
            Auth::Bearer {
                token: "own".into()
            }
        );
        // 链上全是 None → 保持 None
        assert_eq!(
            merge_inherited(&r, &[Inherited::default()]).auth,
            Auth::None
        );
    }

    #[test]
    fn history_entry_without_response_loads() {
        // 0.1.2 之前存下来的 history.json 没有 response
        let old = r#"{"timestamp":"2026-09-01T10:00:00+08:00","status":200,"duration_ms":12,
            "request":{"id":"00000000-0000-0000-0000-000000000001","name":"a","method":"Get",
            "url":"","params":[],"headers":[],"body_type":"None","body":"","form":[],"auth":"None"}}"#;
        let e: HistoryEntry = serde_json::from_str(old).unwrap();
        assert!(e.response.is_none());
    }

    #[test]
    fn reid_replaces_every_id_in_the_tree() {
        let mut c = Collection::new("c");
        c.requests = vec![Request::new("r1")];
        let mut f = Folder::new("f");
        f.requests = vec![Request::new("r2")];
        let mut deep = Folder::new("deep");
        deep.requests = vec![Request::new("r3")];
        f.folders = vec![deep];
        c.folders = vec![f];

        let before: Vec<Uuid> = vec![
            c.id,
            c.requests[0].id,
            c.folders[0].id,
            c.folders[0].requests[0].id,
            c.folders[0].folders[0].id,
            c.folders[0].folders[0].requests[0].id,
        ];
        c.reid();
        let after: Vec<Uuid> = vec![
            c.id,
            c.requests[0].id,
            c.folders[0].id,
            c.folders[0].requests[0].id,
            c.folders[0].folders[0].id,
            c.folders[0].folders[0].requests[0].id,
        ];
        for (b, a) in before.iter().zip(&after) {
            assert_ne!(b, a, "有 id 没换");
        }
        assert_eq!(
            after.iter().collect::<std::collections::HashSet<_>>().len(),
            after.len(),
            "换出来的 id 有重复"
        );
    }

    #[test]
    fn environment_var_map_filters() {
        let env = Environment {
            id: uuid::Uuid::new_v4(),
            name: "dev".into(),
            variables: vec![
                KeyValue::new("base_url", "https://dev.api.com"),
                KeyValue {
                    enabled: false,
                    key: "skip".into(),
                    value: "x".into(),
                    is_file: false,
                    secret: false,
                },
                KeyValue::new("", "no-key"),
            ],
        };
        let map = env.var_map();
        assert_eq!(map.get("base_url").unwrap(), "https://dev.api.com");
        assert!(!map.contains_key("skip"));
        assert!(!map.contains_key(""));
    }
}
