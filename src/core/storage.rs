use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::core::models::{Collection, Environment, HistoryEntry};
use crate::core::secrets::{account, SecretStore};

pub const HISTORY_LIMIT: usize = 500;

/// 数据文件里有 token、密码、API key，以及最近 100 条响应体——默认的 0644
/// 意味着同机器上任何别的用户都能读。~/Library/Application Support 在 macOS 上
/// 也不受 TCC 保护，未签名的第三方 app 读它不会弹任何提示。
#[cfg(unix)]
fn restrict(path: &std::path::Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}
#[cfg(not(unix))]
fn restrict(_path: &std::path::Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

/// 直接以 0600 创建，别先 File::create 出一个 0644 再 chmod —— 中间有个窗口
#[cfg(unix)]
fn create_private(path: &std::path::Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}
#[cfg(not(unix))]
fn create_private(path: &std::path::Path) -> std::io::Result<File> {
    File::create(path)
}

pub struct Storage {
    dir: PathBuf,
    /// 启动时读不出来（权限 / IO / 被锁，不含"文件不存在"）的文件。
    /// 这些文件拒绝写入：前端拿到空数据会立刻存盘，那就把还在磁盘上的
    /// 集合和凭据原地抹掉了 —— 宁可这次不保存，也不能覆盖。
    unreadable: Mutex<HashSet<String>>,
    secrets: Box<dyn SecretStore>,
}

impl Storage {
    pub fn new(dir: PathBuf) -> Self {
        Self::with_secrets(dir, crate::core::secrets::default_store())
    }

    pub fn with_secrets(dir: PathBuf, secrets: Box<dyn SecretStore>) -> Self {
        Self {
            dir,
            unreadable: Mutex::default(),
            secrets,
        }
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
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            // 文件不存在 = 首次启动，正常。其它错误（权限、IO、被锁）不能当成"没数据"：
            // 前端看到空集合会立刻 dirty() 写回，把还在那儿的数据原地抹掉。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return T::default(),
            Err(e) => {
                tracing::error!("{name} 读不出来（{e}）；本次运行拒绝写入该文件，避免覆盖");
                self.unreadable.lock().unwrap().insert(name.to_string());
                return T::default();
            }
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
        if self.unreadable.lock().unwrap().contains(name) {
            return Err(std::io::Error::other(format!(
                "{name} couldn't be read at startup, so Firebee won't overwrite it. \
                 Fix the file's permissions and restart."
            )));
        }
        fs::create_dir_all(&self.dir)?;
        restrict(&self.dir, 0o700)?;
        let data = if pretty {
            serde_json::to_vec_pretty(value)
        } else {
            serde_json::to_vec(value)
        }
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = self.path(&format!("{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
        // 失败时别把 tmp 留在数据目录里
        let write = || -> std::io::Result<()> {
            let mut f = create_private(&tmp)?;
            f.write_all(&data)?;
            f.sync_all()?;
            drop(f);
            fs::rename(&tmp, self.path(name))?;
            // rename 本身也要落盘，否则掉电后可能回到旧名字。
            // Windows 上打开目录句柄需要 FILE_FLAG_BACKUP_SEMANTICS，File::open 会直接失败。
            #[cfg(unix)]
            {
                File::open(&self.dir).and_then(|d| d.sync_all())?;
            }
            Ok(())
        };
        write().inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })
    }

    /// 关联了项目目录的集合以目录为准（git pull 的改动要能看到）；目录读不到就用本地副本
    pub fn load_collections(&self) -> Vec<Collection> {
        let mut cs: Vec<Collection> = self.load("collections.json");
        for c in cs.iter_mut() {
            if let Some(dir) = crate::core::project::dir_of(c) {
                match crate::core::project::read(&dir) {
                    Ok(fresh) => *c = fresh,
                    Err(e) => tracing::warn!("project {}: {e}; using local copy", dir.display()),
                }
            }
        }
        cs
    }

    /// 本地副本先落盘，再同步到各自的项目目录；目录写失败只报错不回滚（本地已经是对的）
    pub fn save_collections(&self, collections: &[Collection]) -> std::io::Result<()> {
        self.save("collections.json", &collections, true)?;
        for c in collections {
            if let Some(dir) = crate::core::project::dir_of(c) {
                crate::core::project::write(&dir, c).map_err(|e| {
                    std::io::Error::other(format!("Couldn't write project {}: {e}", dir.display()))
                })?;
            }
        }
        Ok(())
    }

    /// secret 变量的值从钥匙串填回来；钥匙串里没有就留空（界面上看得出要重填）
    pub fn load_environments(&self) -> Vec<Environment> {
        let mut envs: Vec<Environment> = self.load("environments.json");
        for e in envs.iter_mut() {
            for v in e.variables.iter_mut().filter(|v| v.secret) {
                v.value = self.secrets.get(&account(e.id, &v.key)).unwrap_or_default();
            }
        }
        envs
    }

    /// secret 变量：值写钥匙串（变了才写），JSON 里只留壳；上次有、这次没了的条目从钥匙串删掉
    pub fn save_environments(&self, envs: &[Environment]) -> std::io::Result<()> {
        let before: Vec<Environment> = self.load("environments.json");
        let mut stripped = envs.to_vec();
        let mut keep = HashSet::new();
        for e in stripped.iter_mut() {
            for v in e
                .variables
                .iter_mut()
                .filter(|v| v.secret && !v.key.is_empty())
            {
                let acct = account(e.id, &v.key);
                if self.secrets.get(&acct).as_deref() != Some(v.value.as_str()) {
                    self.secrets
                        .set(&acct, &v.value)
                        .map_err(std::io::Error::other)?;
                }
                keep.insert(acct);
                v.value.clear();
            }
        }
        for e in &before {
            for v in e.variables.iter().filter(|v| v.secret && !v.key.is_empty()) {
                let acct = account(e.id, &v.key);
                if !keep.contains(&acct) {
                    self.secrets.delete(&acct);
                }
            }
        }
        self.save("environments.json", &stripped, true)
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
    #[test]
    fn secret_values_stay_out_of_json_and_come_back_from_store() {
        use crate::core::models::{Environment, KeyValue};
        let tmp = tempfile::tempdir().unwrap();
        let s = Storage::with_secrets(
            tmp.path().to_path_buf(),
            Box::new(crate::core::secrets::Memory::default()),
        );
        let mut tok = KeyValue::new("token", "s3cr3t");
        tok.secret = true;
        let env = Environment {
            id: uuid::Uuid::new_v4(),
            name: "dev".into(),
            variables: vec![KeyValue::new("base", "https://x"), tok],
        };
        s.save_environments(&[env.clone()]).unwrap();
        let raw = std::fs::read_to_string(tmp.path().join("environments.json")).unwrap();
        assert!(!raw.contains("s3cr3t"), "{raw}");
        assert!(raw.contains("\"secret\": true"));
        let back = s.load_environments();
        assert_eq!(back[0].variables[1].value, "s3cr3t");
        assert_eq!(back[0].variables[0].value, "https://x");
        // 删掉变量后钥匙串条目也没了
        let mut env2 = env.clone();
        env2.variables.pop();
        s.save_environments(&[env2]).unwrap();
        assert_eq!(s.load_environments()[0].variables.len(), 1);
        assert!(s.secrets.get(&account(env.id, "token")).is_none());
    }

    use super::*;
    use crate::core::models::Collection;

    #[test]
    fn collections_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Storage::with_secrets(
            tmp.path().to_path_buf(),
            Box::new(crate::core::secrets::Memory::default()),
        );
        let cols = vec![Collection::new("我的集合")];
        s.save_collections(&cols).unwrap();
        let loaded = s.load_collections();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "我的集合");
    }

    #[test]
    fn missing_file_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Storage::with_secrets(
            tmp.path().to_path_buf(),
            Box::new(crate::core::secrets::Memory::default()),
        );
        assert!(s.load_collections().is_empty());
    }

    #[test]
    fn corrupted_file_backed_up_and_defaulted() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("collections.json"), b"{broken").unwrap();
        let s = Storage::with_secrets(
            tmp.path().to_path_buf(),
            Box::new(crate::core::secrets::Memory::default()),
        );
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
        let s = Storage::with_secrets(
            tmp.path().to_path_buf(),
            Box::new(crate::core::secrets::Memory::default()),
        );
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
        let s = Storage::with_secrets(
            tmp.path().to_path_buf(),
            Box::new(crate::core::secrets::Memory::default()),
        );
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

    #[cfg(unix)]
    #[test]
    fn saved_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let s = Storage::new(tmp.path().join("data"));
        s.save_collections(&[Collection::new("a")]).unwrap();
        let mode =
            |p: std::path::PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(tmp.path().join("data/collections.json")), 0o600);
        assert_eq!(mode(tmp.path().join("data")), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_file_is_never_overwritten() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("collections.json");
        let precious = r#"[{"id":"00000000-0000-0000-0000-000000000009","name":"keep-me","folders":[],"requests":[]}]"#;
        std::fs::write(&path, precious).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        let s = Storage::with_secrets(
            tmp.path().to_path_buf(),
            Box::new(crate::core::secrets::Memory::default()),
        );
        assert!(s.load_collections().is_empty(), "读不出来时返回空");
        // 前端见到空数据会立刻存盘——必须被拒绝
        let err = s.save_collections(&[Collection::new("empty")]).unwrap_err();
        assert!(err.to_string().contains("won't overwrite"), "{err}");

        // 磁盘上的原文件一字未动
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(String::from_utf8(std::fs::read(&path).unwrap())
            .unwrap()
            .contains("keep-me"));
    }

    #[test]
    fn history_trimmed_to_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Storage::with_secrets(
            tmp.path().to_path_buf(),
            Box::new(crate::core::secrets::Memory::default()),
        );
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
