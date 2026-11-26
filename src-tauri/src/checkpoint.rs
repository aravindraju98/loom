//! Restore points for agent file changes ("go back to before this message").
//!
//! Each user message has a checkpoint id. While the agent works on that message, every file is
//! snapshotted right before its first change (`before`) and again after each change (`after`).
//! Snapshots are content-addressed blobs, so unchanged content is stored once.
//!
//! Layout: %APPDATA%\harness\checkpoints\<chat_id>\{blobs\<sha256>, cp\<checkpoint_id>.json}
//! It never touches git or anything inside the workspace except the files being restored.
use crate::store::data_dir;
use crate::textfile;
use crate::util::{write_atomic, LockExt};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Files bigger than this are not snapshotted (and so cannot be restored).
const MAX_SNAPSHOT: u64 = 8 * 1024 * 1024;

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct FileRec {
    /// content hash before this checkpoint first changed the file; None = file did not exist
    pub before: Option<String>,
    /// content hash after the latest change in this checkpoint; None = deleted
    pub after: Option<String>,
    /// too large to snapshot
    #[serde(default)]
    pub skipped: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Manifest {
    pub id: String,
    pub chat_id: String,
    /// ms since epoch; orders checkpoints within a chat
    pub created: u128,
    /// "turn" (agent work for one message) | "restore" (state saved before a restore, for undo)
    pub kind: String,
    pub files: BTreeMap<String, FileRec>,
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

fn safe(id: &str) -> Result<&str, String> {
    if !id.is_empty() && id.len() <= 80 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        Ok(id)
    } else {
        Err("bad id".into())
    }
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn default_root() -> PathBuf {
        data_dir().join("checkpoints")
    }

    pub fn new(root: PathBuf) -> Self {
        Store { root }
    }

    fn chat_dir(&self, chat: &str) -> Result<PathBuf, String> {
        Ok(self.root.join(safe(chat)?))
    }

    fn manifest_path(&self, chat: &str, id: &str) -> Result<PathBuf, String> {
        Ok(self.chat_dir(chat)?.join("cp").join(format!("{}.json", safe(id)?)))
    }

    fn blob_path(&self, chat: &str, sha: &str) -> Result<PathBuf, String> {
        if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("bad blob id".into());
        }
        Ok(self.chat_dir(chat)?.join("blobs").join(sha))
    }

    fn put_blob(&self, chat: &str, bytes: &[u8]) -> Result<String, String> {
        let sha = textfile::sha256_hex(bytes);
        let p = self.blob_path(chat, &sha)?;
        if !p.exists() {
            write_atomic(&p, bytes).map_err(|e| format!("checkpoint write failed: {e}"))?;
        }
        Ok(sha)
    }

    fn get_blob(&self, chat: &str, sha: &str) -> Result<Vec<u8>, String> {
        std::fs::read(self.blob_path(chat, sha)?).map_err(|e| format!("checkpoint data missing ({e})"))
    }

    fn load(&self, chat: &str, id: &str) -> Option<Manifest> {
        let t = std::fs::read_to_string(self.manifest_path(chat, id).ok()?).ok()?;
        serde_json::from_str(&t).ok()
    }

    fn save(&self, m: &Manifest) -> Result<(), String> {
        let t = serde_json::to_string_pretty(m).map_err(|e| e.to_string())?;
        write_atomic(&self.manifest_path(&m.chat_id, &m.id)?, t.as_bytes()).map_err(|e| format!("checkpoint write failed: {e}"))
    }

    /// Hash of what is on disk now: Ok(None) = missing, Err = too large / unreadable.
    fn current(&self, path: &Path) -> Result<Option<(String, Vec<u8>)>, String> {
        match std::fs::metadata(path) {
            Err(_) => Ok(None),
            Ok(md) if md.len() > MAX_SNAPSHOT => Err("too large".into()),
            Ok(_) => {
                let b = std::fs::read(path).map_err(|e| e.to_string())?;
                Ok(Some((textfile::sha256_hex(&b), b)))
            }
        }
    }

    /// Record the file's state before the agent changes it (first change in this checkpoint only).
    pub fn snapshot_before(&self, chat: &str, id: &str, path: &Path) -> Result<(), String> {
        let _g = LOCK.lk();
        let key = path.to_string_lossy().into_owned();
        let mut m = self.load(chat, id).unwrap_or_else(|| Manifest {
            id: id.to_string(),
            chat_id: chat.to_string(),
            created: now_ms(),
            kind: "turn".into(),
            files: BTreeMap::new(),
        });
        if m.files.contains_key(&key) {
            return Ok(());
        }
        let rec = match self.current(path) {
            Ok(None) => FileRec { before: None, after: None, skipped: false },
            Ok(Some((_, bytes))) => {
                let sha = self.put_blob(chat, &bytes)?;
                FileRec { before: Some(sha.clone()), after: Some(sha), skipped: false }
            }
            Err(_) => FileRec { before: None, after: None, skipped: true },
        };
        m.files.insert(key, rec);
        self.save(&m)
    }

    /// Record the file's state after an agent change.
    pub fn record_after(&self, chat: &str, id: &str, path: &Path) -> Result<(), String> {
        let _g = LOCK.lk();
        let key = path.to_string_lossy().into_owned();
        let Some(mut m) = self.load(chat, id) else { return Ok(()) };
        let Some(rec) = m.files.get(&key).cloned() else { return Ok(()) };
        let after = match self.current(path) {
            Ok(None) => None,
            Ok(Some((_, bytes))) => Some(self.put_blob(chat, &bytes)?),
            Err(_) => None,
        };
        m.files.insert(key, FileRec { after, ..rec });
        self.save(&m)
    }

    /// The "turn" manifests among `ids`, oldest first.
    fn selected(&self, chat: &str, ids: &[String]) -> Vec<Manifest> {
        let mut v: Vec<Manifest> = ids.iter().filter_map(|id| self.load(chat, id)).filter(|m| m.kind == "turn").collect();
        v.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
        v
    }

    /// What restoring to before the first of `ids` would do, file by file.
    pub fn plan(&self, chat: &str, ids: &[String]) -> Result<Vec<PlanItem>, String> {
        let ms = self.selected(chat, ids);
        // path -> (target = earliest before, expected = latest after, skipped)
        let mut acc: BTreeMap<String, (Option<String>, Option<String>, bool)> = BTreeMap::new();
        for m in &ms {
            for (p, r) in &m.files {
                acc.entry(p.clone())
                    .and_modify(|e| {
                        e.1 = r.after.clone();
                        e.2 |= r.skipped;
                    })
                    .or_insert((r.before.clone(), r.after.clone(), r.skipped));
            }
        }
        let mut out = vec![];
        for (path, (target, expected, skipped)) in acc {
            let cur = self.current(Path::new(&path));
            let (cur_sha, cur_bytes) = match &cur {
                Ok(Some((s, b))) => (Some(s.clone()), Some(b.clone())),
                _ => (None, None),
            };
            let action = match (&target, &cur_sha) {
                _ if skipped => "skip",
                (None, None) => "none",
                (None, Some(_)) => "delete",
                (Some(_), None) => "recreate",
                (Some(t), Some(c)) if t == c => "none",
                _ => "restore",
            };
            // edited outside the agent since its last change: restoring would discard that work
            let conflict = !skipped && cur.is_ok() && cur_sha != expected;
            let (added, removed) = if action == "none" || action == "skip" {
                (0, 0)
            } else {
                let now = cur_bytes.as_deref().map(|b| textfile::decode(b).text).unwrap_or_default();
                let then = match &target {
                    Some(t) => self.get_blob(chat, t).map(|b| textfile::decode(&b).text).unwrap_or_default(),
                    None => String::new(),
                };
                textfile::diff_stat(&now, &then)
            };
            out.push(PlanItem { path, action: action.into(), conflict, added, removed });
        }
        Ok(out)
    }

    /// Put files back to how they were before the first of `ids`. Returns an undo id.
    pub fn restore(&self, chat: &str, ids: &[String], force: bool) -> Result<RestoreResult, String> {
        let plan = self.plan(chat, ids)?;
        if !force {
            let c: Vec<&str> = plan.iter().filter(|p| p.conflict && p.action != "none").map(|p| p.path.as_str()).collect();
            if !c.is_empty() {
                return Err(format!("conflict: changed outside the agent since: {}", c.join(", ")));
            }
        }
        let ms = self.selected(chat, ids);
        let mut targets: BTreeMap<String, Option<String>> = BTreeMap::new();
        for m in &ms {
            for (p, r) in &m.files {
                if !r.skipped {
                    targets.entry(p.clone()).or_insert(r.before.clone());
                }
            }
        }
        let _g = LOCK.lk();
        // save what is there now so the restore itself can be undone
        let undo_id = format!("undo_{}", uuid::Uuid::new_v4().simple());
        let mut undo = Manifest { id: undo_id.clone(), chat_id: chat.into(), created: now_ms(), kind: "restore".into(), files: BTreeMap::new() };
        for p in targets.keys() {
            let before = match self.current(Path::new(p)) {
                Ok(Some((_, b))) => Some(self.put_blob(chat, &b)?),
                _ => None,
            };
            undo.files.insert(p.clone(), FileRec { before, after: None, skipped: false });
        }
        self.save(&undo)?;
        let mut restored = vec![];
        for (p, target) in &targets {
            let path = Path::new(p);
            self.write_state(chat, path, target.as_deref())?;
            restored.push(p.clone());
        }
        Ok(RestoreResult { undo_id, restored })
    }

    fn write_state(&self, chat: &str, path: &Path, sha: Option<&str>) -> Result<(), String> {
        match sha {
            None => {
                if path.exists() {
                    std::fs::remove_file(path).map_err(|e| format!("could not delete {}: {e}", path.display()))?;
                }
            }
            Some(s) => {
                let bytes = self.get_blob(chat, s)?;
                write_atomic(path, &bytes).map_err(|e| format!("could not write {}: {e}", path.display()))?;
            }
        }
        Ok(())
    }

    pub fn undo_restore(&self, chat: &str, undo_id: &str) -> Result<Vec<String>, String> {
        let _g = LOCK.lk();
        let m = self.load(chat, undo_id).filter(|m| m.kind == "restore").ok_or("nothing to undo")?;
        let mut out = vec![];
        for (p, r) in &m.files {
            self.write_state(chat, Path::new(p), r.before.as_deref())?;
            out.push(p.clone());
        }
        let _ = std::fs::remove_file(self.manifest_path(chat, undo_id)?);
        Ok(out)
    }

    /// Diff from what is on disk now to the restore target of one file.
    pub fn file_diff(&self, chat: &str, ids: &[String], path: &str) -> Result<String, String> {
        let ms = self.selected(chat, ids);
        let target = ms.iter().find_map(|m| m.files.get(path).map(|r| r.before.clone())).ok_or("file not in this checkpoint")?;
        let now = std::fs::read(path).map(|b| textfile::decode(&b).text).unwrap_or_default();
        let then = match target {
            Some(t) => textfile::decode(&self.get_blob(chat, &t)?).text,
            None => String::new(),
        };
        let name = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(textfile::unified(&now, &then, &name, 3))
    }

    /// Which checkpoints (of `ids`) changed files; used to badge messages in the UI.
    pub fn touched(&self, chat: &str, ids: &[String]) -> BTreeMap<String, usize> {
        ids.iter().filter_map(|id| self.load(chat, id).map(|m| (id.clone(), m.files.len()))).filter(|(_, n)| *n > 0).collect()
    }

    pub fn delete_chat(&self, chat: &str) {
        if let Ok(d) = self.chat_dir(chat) {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

#[derive(Serialize, Debug, Clone)]
pub struct PlanItem {
    pub path: String,
    /// "restore" | "delete" | "recreate" | "none" | "skip"
    pub action: String,
    pub conflict: bool,
    pub added: usize,
    pub removed: usize,
}

#[derive(Serialize, Debug)]
pub struct RestoreResult {
    pub undo_id: String,
    pub restored: Vec<String>,
}

fn store() -> Store {
    Store::new(Store::default_root())
}

/// Hook used by the tools: called before/after each file mutation in a run.
#[derive(Clone)]
pub struct Recorder {
    pub chat: String,
    pub id: String,
}

impl Recorder {
    pub fn before(&self, p: &Path) {
        if let Err(e) = store().snapshot_before(&self.chat, &self.id, p) {
            eprintln!("checkpoint: {e}");
        }
    }
    pub fn after(&self, p: &Path) {
        if let Err(e) = store().record_after(&self.chat, &self.id, p) {
            eprintln!("checkpoint: {e}");
        }
    }
}

#[tauri::command]
pub fn checkpoint_plan(chat_id: String, ids: Vec<String>) -> Result<Vec<PlanItem>, String> {
    store().plan(&chat_id, &ids)
}

#[tauri::command]
pub fn checkpoint_restore(chat_id: String, ids: Vec<String>, force: bool) -> Result<RestoreResult, String> {
    store().restore(&chat_id, &ids, force)
}

#[tauri::command]
pub fn checkpoint_undo(chat_id: String, undo_id: String) -> Result<Vec<String>, String> {
    store().undo_restore(&chat_id, &undo_id)
}

#[tauri::command]
pub fn checkpoint_diff(chat_id: String, ids: Vec<String>, path: String) -> Result<String, String> {
    store().file_diff(&chat_id, &ids, &path)
}

#[tauri::command]
pub fn checkpoint_touched(chat_id: String, ids: Vec<String>) -> BTreeMap<String, usize> {
    store().touched(&chat_id, &ids)
}

pub fn delete_chat(chat_id: &str) {
    store().delete_chat(chat_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("harness_cp_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Simulate the agent changing a file inside checkpoint `id`.
    fn agent_write(s: &Store, id: &str, p: &Path, content: Option<&str>) {
        s.snapshot_before("c1", id, p).unwrap();
        match content {
            Some(c) => std::fs::write(p, c).unwrap(),
            None => std::fs::remove_file(p).unwrap(),
        }
        s.record_after("c1", id, p).unwrap();
    }

    #[test]
    fn restore_to_earlier_message_and_undo() {
        let root = tmp("root");
        let ws = tmp("ws");
        let s = Store::new(root);
        let a = ws.join("a.txt");
        let b = ws.join("b.txt");
        let gone = ws.join("gone.txt");
        std::fs::write(&a, "a0").unwrap();
        std::fs::write(&gone, "keep me").unwrap();

        // message 1: edit a, create b
        agent_write(&s, "m1", &a, Some("a1"));
        agent_write(&s, "m1", &b, Some("b1"));
        // message 2: edit a again, delete gone
        agent_write(&s, "m2", &a, Some("a2"));
        agent_write(&s, "m2", &gone, None);

        // restoring to before message 2 only undoes message 2
        let ids2 = vec!["m2".to_string()];
        let plan = s.plan("c1", &ids2).unwrap();
        assert_eq!(plan.iter().find(|p| p.path.ends_with("a.txt")).unwrap().action, "restore");
        assert_eq!(plan.iter().find(|p| p.path.ends_with("gone.txt")).unwrap().action, "recreate");
        s.restore("c1", &ids2, false).unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a1");
        assert_eq!(std::fs::read_to_string(&gone).unwrap(), "keep me");

        // restoring to before message 1 (ids from message 1 onward) removes b and resets a
        let ids1 = vec!["m1".to_string(), "m2".to_string()];
        let r = s.restore("c1", &ids1, true).unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a0");
        assert!(!b.exists());

        // and the restore can be undone
        s.undo_restore("c1", &r.undo_id).unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a1");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b1");
    }

    #[test]
    fn detects_outside_edits_as_conflicts() {
        let s = Store::new(tmp("root2"));
        let ws = tmp("ws2");
        let f = ws.join("f.txt");
        std::fs::write(&f, "v0").unwrap();
        agent_write(&s, "m1", &f, Some("v1"));
        std::fs::write(&f, "user edit").unwrap();
        let ids = vec!["m1".to_string()];
        assert!(s.plan("c1", &ids).unwrap()[0].conflict);
        assert!(s.restore("c1", &ids, false).unwrap_err().starts_with("conflict"));
        s.restore("c1", &ids, true).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "v0");
        // diff preview and badges
        assert!(s.touched("c1", &ids).contains_key("m1"));
    }

    #[test]
    fn blobs_are_deduplicated() {
        let root = tmp("root3");
        let ws = tmp("ws3");
        let s = Store::new(root.clone());
        for (i, n) in ["x.txt", "y.txt"].iter().enumerate() {
            let p = ws.join(n);
            std::fs::write(&p, "same").unwrap();
            agent_write(&s, &format!("m{i}"), &p, Some("same2"));
        }
        let blobs = std::fs::read_dir(root.join("c1").join("blobs")).unwrap().count();
        assert_eq!(blobs, 2); // "same" and "same2", once each
    }
}
