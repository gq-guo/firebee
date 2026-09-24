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
}

impl KeyValue {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            enabled: true,
            key: key.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BodyType {
    None,
    Json,
    Text,
    Form,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Auth {
    None,
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
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: Uuid,
    pub name: String,
    pub folders: Vec<Folder>,
    pub requests: Vec<Request>,
}

impl Folder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            folders: vec![],
            requests: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub id: Uuid,
    pub name: String,
    pub folders: Vec<Folder>,
    pub requests: Vec<Request>,
}

impl Collection {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            folders: vec![],
            requests: vec![],
        }
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
    pub size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub request: Request,
    pub status: Option<u16>,
    pub duration_ms: Option<u128>,
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
