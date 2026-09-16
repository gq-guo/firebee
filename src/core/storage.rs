use std::fs;
use std::path::PathBuf;

use crate::core::models::{Collection, Environment, HistoryEntry};

pub const HISTORY_LIMIT: usize = 500;

pub struct Storage {
    dir: PathBuf,
}

impl Storage {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// 系统标准数据目录，如 macOS ~/Library/Application Support/firebee
    pub fn default_dir() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("firebee")
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn load<T: serde::de::DeserializeOwned + Default>(&self, name: &str) -> T {
        let path = self.path(name);
        let Ok(bytes) = fs::read(&path) else {
            return T::default();
        };
        match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("{name} 解析失败（{e}），备份为 .bak 并以空数据启动");
                let _ = fs::rename(&path, self.dir.join(name.replace(".json", ".bak")));
                T::default()
            }
        }
    }

    /// 原子写入：先写临时文件再 rename，避免半截文件
    fn save<T: serde::Serialize>(&self, name: &str, value: &T) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let tmp = self.path(&format!("{name}.tmp"));
        let data = serde_json::to_vec_pretty(value)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(&tmp, data)?;
        fs::rename(&tmp, self.path(name))?;
        Ok(())
    }

    pub fn load_collections(&self) -> Vec<Collection> {
        self.load("collections.json")
    }

    pub fn save_collections(&self, collections: &[Collection]) -> std::io::Result<()> {
        self.save("collections.json", &collections)
    }

    pub fn load_environments(&self) -> Vec<Environment> {
        self.load("environments.json")
    }

    pub fn save_environments(&self, envs: &[Environment]) -> std::io::Result<()> {
        self.save("environments.json", &envs)
    }

    pub fn load_history(&self) -> Vec<HistoryEntry> {
        self.load("history.json")
    }

    /// 只保留最后 HISTORY_LIMIT 条（FIFO 淘汰最旧的）
    pub fn save_history(&self, history: &[HistoryEntry]) -> std::io::Result<()> {
        let start = history.len().saturating_sub(HISTORY_LIMIT);
        self.save("history.json", &&history[start..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::Collection;

    #[test]
    fn collections_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Storage::new(tmp.path().to_path_buf());
        let cols = vec![Collection::new("我的集合")];
        s.save_collections(&cols).unwrap();
        let loaded = s.load_collections();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "我的集合");
    }

    #[test]
    fn missing_file_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Storage::new(tmp.path().to_path_buf());
        assert!(s.load_collections().is_empty());
    }

    #[test]
    fn corrupted_file_backed_up_and_defaulted() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("collections.json"), b"{broken").unwrap();
        let s = Storage::new(tmp.path().to_path_buf());
        assert!(s.load_collections().is_empty());
        assert!(tmp.path().join("collections.bak").exists());
        // 再次加载不 panic、不重复改名失败
        assert!(s.load_collections().is_empty());
    }

    #[test]
    fn history_trimmed_to_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Storage::new(tmp.path().to_path_buf());
        let entries: Vec<HistoryEntry> = (0..600)
            .map(|_| HistoryEntry {
                timestamp: chrono::Local::now(),
                request: crate::core::models::Request::new("r"),
                status: Some(200),
                duration_ms: Some(1),
            })
            .collect();
        s.save_history(&entries).unwrap();
        assert_eq!(s.load_history().len(), 500);
    }
}
