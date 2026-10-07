//! Secret 变量的值不进 environments.json，存系统钥匙串（macOS Keychain）。
//! JSON 里只留 key 和 secret=true 的壳；启动时从钥匙串填回来。

use std::collections::HashMap;
use std::sync::Mutex;

pub trait SecretStore: Send + Sync {
    fn get(&self, account: &str) -> Option<String>;
    fn set(&self, account: &str, value: &str) -> Result<(), String>;
    fn delete(&self, account: &str);
}

const SERVICE: &str = "Firebee";

#[cfg(target_os = "macos")]
pub struct Keychain;

#[cfg(target_os = "macos")]
impl SecretStore for Keychain {
    fn get(&self, account: &str) -> Option<String> {
        security_framework::passwords::get_generic_password(SERVICE, account)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
    }
    fn set(&self, account: &str, value: &str) -> Result<(), String> {
        security_framework::passwords::set_generic_password(SERVICE, account, value.as_bytes())
            .map_err(|e| format!("Keychain: {e}"))
    }
    fn delete(&self, account: &str) {
        let _ = security_framework::passwords::delete_generic_password(SERVICE, account);
    }
}

/// 测试用 / 没有钥匙串的平台用：只在内存里
#[derive(Default)]
pub struct Memory(Mutex<HashMap<String, String>>);

impl SecretStore for Memory {
    fn get(&self, account: &str) -> Option<String> {
        self.0.lock().unwrap().get(account).cloned()
    }
    fn set(&self, account: &str, value: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(account.into(), value.into());
        Ok(())
    }
    fn delete(&self, account: &str) {
        self.0.lock().unwrap().remove(account);
    }
}

pub fn default_store() -> Box<dyn SecretStore> {
    #[cfg(target_os = "macos")]
    {
        Box::new(Keychain)
    }
    #[cfg(not(target_os = "macos"))]
    {
        // ponytail: 其他平台还没发布；真要支持再接 keyring
        tracing::warn!(
            "no keychain on this platform: secret variables live only in memory for this session"
        );
        Box::new(Memory::default())
    }
}

/// 钥匙串里的条目名：环境 id + 变量名。同名变量在不同环境里是不同的秘密
pub fn account(env_id: uuid::Uuid, key: &str) -> String {
    format!("{}/{key}", env_id.simple())
}
