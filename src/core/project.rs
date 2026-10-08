//! Git-friendly 的集合落盘格式：一个目录，`firebee.json` 放树的骨架（名字 / 顺序 / 共享 header 与 auth），
//! 每个请求一个文件放在 `requests/`。diff 一眼能看，两个人改不同的请求也不会冲突。
//!
//! collections.json 里仍然保留一份完整副本（目录没了也不丢数据）；目录在启动时读回来，
//! 以目录为准 —— git pull 拉下来的改动要能看到。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::models::{Auth, Collection, Folder, KeyValue, Request};

pub const MANIFEST: &str = "firebee.json";
const REQUESTS: &str = "requests";

#[derive(Serialize, Deserialize)]
struct Manifest {
    /// 格式版本
    firebee: u32,
    id: Uuid,
    name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    headers: Vec<KeyValue>,
    #[serde(default, skip_serializing_if = "Auth::is_none")]
    auth: Auth,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    folders: Vec<Node>,
    /// requests/ 下的文件名，按显示顺序
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    requests: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct Node {
    id: Uuid,
    name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    headers: Vec<KeyValue>,
    #[serde(default, skip_serializing_if = "Auth::is_none")]
    auth: Auth,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    folders: Vec<Node>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    requests: Vec<String>,
}

impl Auth {
    fn is_none(&self) -> bool {
        matches!(self, Auth::None)
    }
}

/// 文件名：名字转 slug + id 前 8 位。改名文件名会变（git 里是一次 rename），同名请求不会撞
pub fn file_name(r: &Request) -> String {
    let mut slug: String = r
        .name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    slug.truncate(60);
    if slug.is_empty() {
        slug = "request".into();
    }
    format!("{slug}-{}.json", &r.id.simple().to_string()[..8])
}

pub fn is_project_dir(dir: &Path) -> bool {
    dir.join(MANIFEST).is_file()
}

/// 写整个集合到目录。requests/ 里不再属于集合的文件会删掉（移走的请求、删掉的请求）。
pub fn write(dir: &Path, c: &Collection) -> std::io::Result<()> {
    let req_dir = dir.join(REQUESTS);
    fs::create_dir_all(&req_dir)?;
    let mut keep = BTreeSet::new();
    fn node(f: &Folder, req_dir: &Path, keep: &mut BTreeSet<String>) -> std::io::Result<Node> {
        Ok(Node {
            id: f.id,
            name: f.name.clone(),
            headers: f.headers.clone(),
            auth: f.auth.clone(),
            folders: f
                .folders
                .iter()
                .map(|s| node(s, req_dir, keep))
                .collect::<Result<_, _>>()?,
            requests: write_requests(&f.requests, req_dir, keep)?,
        })
    }
    let m = Manifest {
        firebee: 1,
        id: c.id,
        name: c.name.clone(),
        headers: c.headers.clone(),
        auth: c.auth.clone(),
        folders: c
            .folders
            .iter()
            .map(|f| node(f, &req_dir, &mut keep))
            .collect::<Result<_, _>>()?,
        requests: write_requests(&c.requests, &req_dir, &mut keep)?,
    };
    write_atomic(&dir.join(MANIFEST), &serde_json::to_vec_pretty(&m)?)?;
    for entry in fs::read_dir(&req_dir)? {
        let p = entry?.path();
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
        if p.extension().is_some_and(|e| e == "json") && name.is_some_and(|n| !keep.contains(&n)) {
            fs::remove_file(&p)?;
        }
    }
    Ok(())
}

fn write_requests(
    rs: &[Request],
    req_dir: &Path,
    keep: &mut BTreeSet<String>,
) -> std::io::Result<Vec<String>> {
    let mut names = Vec::with_capacity(rs.len());
    for r in rs {
        let name = file_name(r);
        let path = req_dir.join(&name);
        let data = serde_json::to_vec_pretty(r)?;
        // 内容没变就不写：别让 mtime 到处跳，也省得 git status 一直脏
        if fs::read(&path).ok().as_deref() != Some(&data[..]) {
            write_atomic(&path, &data)?;
        }
        keep.insert(name.clone());
        names.push(name);
    }
    Ok(names)
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", Uuid::new_v4().simple()));
    let r = fs::write(&tmp, data).and_then(|_| fs::rename(&tmp, path));
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

/// 从目录读集合。请求文件缺了就跳过（别人删了文件但没改 manifest），不算错。
pub fn read(dir: &Path) -> std::io::Result<Collection> {
    let m: Manifest = serde_json::from_slice(&fs::read(dir.join(MANIFEST))?)?;
    if m.firebee != 1 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{MANIFEST} is format version {}, this Firebee reads 1",
                m.firebee
            ),
        ));
    }
    let req_dir = dir.join(REQUESTS);
    // manifest 列了文件但一个都读不到（requests/ 被 gitignore、只拷了一半）：按坏目录处理，
    // 调用方会退回本地副本，而不是把空集合存成新的事实
    fn count(n: &Node) -> usize {
        n.requests.len() + n.folders.iter().map(count).sum::<usize>()
    }
    let listed = m.requests.len() + m.folders.iter().map(count).sum::<usize>();
    if listed > 0 && !req_dir.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{MANIFEST} lists {listed} requests but {REQUESTS}/ is missing"),
        ));
    }
    fn reqs(names: &[String], req_dir: &Path) -> Vec<Request> {
        names
            .iter()
            .filter_map(|n| {
                // 只认 requests/ 下的裸文件名，manifest 里写 ../ 不能读到别处去
                let n = Path::new(n).file_name()?;
                let bytes = fs::read(req_dir.join(n)).ok()?;
                serde_json::from_slice(&bytes)
                    .inspect_err(|e| tracing::warn!("skip {}: {e}", n.to_string_lossy()))
                    .ok()
            })
            .collect()
    }
    fn folder(n: Node, req_dir: &Path) -> Folder {
        Folder {
            id: n.id,
            name: n.name,
            headers: n.headers,
            auth: n.auth,
            requests: reqs(&n.requests, req_dir),
            folders: n.folders.into_iter().map(|s| folder(s, req_dir)).collect(),
        }
    }
    Ok(Collection {
        id: m.id,
        name: m.name,
        headers: m.headers,
        auth: m.auth,
        requests: reqs(&m.requests, &req_dir),
        folders: m.folders.into_iter().map(|f| folder(f, &req_dir)).collect(),
        project_dir: Some(dir.to_string_lossy().into_owned()),
    })
}

pub fn dir_of(c: &Collection) -> Option<PathBuf> {
    c.project_dir.as_deref().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::HttpMethod;

    fn sample() -> Collection {
        let mut c = Collection::new("My API");
        c.headers = vec![KeyValue::new("X-Team", "qa")];
        let mut r = Request::new("List users");
        r.url = "{{base}}/users".into();
        c.requests.push(r);
        let mut f = Folder::new("Auth");
        let mut login = Request::new("Login / with slash");
        login.method = HttpMethod::Post;
        f.requests.push(login);
        f.auth = Auth::Bearer {
            token: "{{t}}".into(),
        };
        c.folders.push(f);
        c
    }

    #[test]
    fn roundtrip_and_prunes_removed_requests() {
        let tmp = tempfile::tempdir().unwrap();
        let mut c = sample();
        write(tmp.path(), &c).unwrap();
        assert!(is_project_dir(tmp.path()));
        let files: Vec<_> = fs::read_dir(tmp.path().join("requests"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(files.len(), 2);
        assert!(
            files.iter().any(|f| f.starts_with("login-with-slash-")),
            "{files:?}"
        );

        let back = read(tmp.path()).unwrap();
        assert_eq!(back.id, c.id);
        assert_eq!(back.name, "My API");
        assert_eq!(back.headers[0].key, "X-Team");
        assert_eq!(back.requests[0].url, "{{base}}/users");
        assert_eq!(back.folders[0].name, "Auth");
        assert_eq!(
            back.folders[0].auth,
            Auth::Bearer {
                token: "{{t}}".into()
            }
        );
        assert_eq!(back.folders[0].requests[0].method, HttpMethod::Post);
        assert_eq!(
            back.project_dir.as_deref(),
            Some(tmp.path().to_str().unwrap())
        );

        // 删掉一个请求再写：文件被清掉
        c.folders[0].requests.clear();
        write(tmp.path(), &c).unwrap();
        let files: Vec<_> = fs::read_dir(tmp.path().join("requests")).unwrap().collect();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn unchanged_files_are_not_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let c = sample();
        write(tmp.path(), &c).unwrap();
        let p = tmp.path().join("requests").join(file_name(&c.requests[0]));
        let m1 = fs::metadata(&p).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(tmp.path(), &c).unwrap();
        assert_eq!(fs::metadata(&p).unwrap().modified().unwrap(), m1);
    }

    #[test]
    fn missing_requests_dir_is_an_error_not_an_empty_collection() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), &sample()).unwrap();
        fs::remove_dir_all(tmp.path().join("requests")).unwrap();
        assert!(read(tmp.path()).is_err());
    }

    #[test]
    fn missing_request_file_is_skipped_and_manifest_paths_are_confined() {
        let tmp = tempfile::tempdir().unwrap();
        let c = sample();
        write(tmp.path(), &c).unwrap();
        let mut m: serde_json::Value =
            serde_json::from_slice(&fs::read(tmp.path().join(MANIFEST)).unwrap()).unwrap();
        m["requests"] =
            serde_json::json!(["../../etc/passwd", "gone.json", file_name(&c.requests[0])]);
        fs::write(tmp.path().join(MANIFEST), serde_json::to_vec(&m).unwrap()).unwrap();
        let back = read(tmp.path()).unwrap();
        assert_eq!(back.requests.len(), 1);
    }
}
