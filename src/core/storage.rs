use std::fs::{self, File};
use std::io::Write;
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
                let bak = self.free_backup_name(name);
                tracing::warn!("{name} 解析失败（{e}），备份为 {bak} 并以空数据启动");
                let _ = fs::rename(&path, self.dir.join(&bak));
                T::default()
            }
        }
    }

    /// 损坏文件的备份名：带时间戳，且不覆盖已有的备份——
    /// 连着坏两次时，把上一份好数据的备份盖掉就等于彻底丢了。
    fn free_backup_name(&self, name: &str) -> String {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let base = name.strip_suffix(".json").unwrap_or(name);
        for n in 1..1000 {
            let suffix = if n == 1 {
                String::new()
            } else {
                format!("-{n}")
            };
            let candidate = format!("{base}.{stamp}{suffix}.bak");
            if !self.dir.join(&candidate).exists() {
                return candidate;
            }
        }
        format!("{base}.{stamp}.bak")
    }

    /// 原子写入：写临时文件 → fsync → rename → fsync 目录。
    /// 少了文件的 fsync，掉电后可能 rename 已经可见而数据块还没落盘，
    /// 留下一个 0 字节的 collections.json —— 下次启动解析失败，数据就没了。
    /// 临时文件名带唯一后缀，两次重叠的保存不会写进同一个 tmp。
    /// pretty=false 用于 history.json：几 MB 的机器数据，没必要为缩进付序列化开销；
    /// collections / environments 保持可读，方便出问题时直接看和 diff
    fn save<T: serde::Serialize>(
        &self,
        name: &str,
        value: &T,
        pretty: bool,
    ) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let data = if pretty {
            serde_json::to_vec_pretty(value)
        } else {
            serde_json::to_vec(value)
        }
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = self.path(&format!("{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
        // 失败时别把 tmp 留在数据目录里
        let write = || -> std::io::Result<()> {
            let mut f = File::create(&tmp)?;
            f.write_all(&data)?;
            f.sync_all()?;
            drop(f);
            fs::rename(&tmp, self.path(name))?;
            // rename 本身也要落盘，否则掉电后可能回到旧名字
            File::open(&self.dir).and_then(|d| d.sync_all())
        };
        write().inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })
    }

    pub fn load_collections(&self) -> Vec<Collection> {
        self.load("collections.json")
    }

    pub fn save_collections(&self, collections: &[Collection]) -> std::io::Result<()> {
        self.save("collections.json", &collections, true)
    }

    pub fn load_environments(&self) -> Vec<Environment> {
        self.load("environments.json")
    }

    pub fn save_environments(&self, envs: &[Environment]) -> std::io::Result<()> {
        self.save("environments.json", &envs, true)
    }

    pub fn load_history(&self) -> Vec<HistoryEntry> {
        self.load("history.json")
    }

    /// 只保留最后 HISTORY_LIMIT 条（FIFO 淘汰最旧的）
    pub fn save_history(&self, history: &[HistoryEntry]) -> std::io::Result<()> {
        let start = history.len().saturating_sub(HISTORY_LIMIT);
        self.save("history.json", &&history[start..], false)
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
        let bak = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.starts_with("collections.") && n.ends_with(".bak")
            });
        assert!(bak.is_some(), "没有生成备份");
        // 再次加载不 panic、不重复改名失败
        assert!(s.load_collections().is_empty());
    }

    #[test]
    fn corrupt_file_twice_keeps_both_backups() {
        // 第二次损坏不能盖掉第一次的备份
        let tmp = tempfile::tempdir().unwrap();
        let s = Storage::new(tmp.path().to_path_buf());
        let baks = || {
            std::fs::read_dir(tmp.path())
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().ends_with(".bak"))
                .count()
        };
        std::fs::write(tmp.path().join("collections.json"), b"{broken-1").unwrap();
        assert!(s.load_collections().is_empty());
        assert_eq!(baks(), 1);
        std::fs::write(tmp.path().join("collections.json"), b"{broken-2").unwrap();
        assert!(s.load_collections().is_empty());
        assert_eq!(baks(), 2, "第二份备份把第一份盖掉了");
    }

    #[test]
    fn save_leaves_no_temp_files_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Storage::new(tmp.path().to_path_buf());
        s.save_collections(&[Collection::new("a")]).unwrap();
        s.save_collections(&[Collection::new("b")]).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件: {leftovers:?}");
        assert_eq!(s.load_collections()[0].name, "b");
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
                response: None,
            })
            .collect();
        s.save_history(&entries).unwrap();
        assert_eq!(s.load_history().len(), 500);
    }
}
