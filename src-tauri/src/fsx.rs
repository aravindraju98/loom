//! File explorer commands for the Files panel. Confined to the chosen workspace folder so the
//! webview (which renders model output) can never be used to read arbitrary files.
use crate::textfile;
use crate::util::{write_atomic, LockExt};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

fn workspace() -> Result<PathBuf, String> {
    let ws = crate::store::load_settings().workspace;
    if ws.trim().is_empty() {
        return Err("choose a workspace folder first".into());
    }
    Ok(PathBuf::from(ws))
}

fn in_workspace(path: &str) -> Result<PathBuf, String> {
    crate::tools::resolve(&workspace()?, path)
}

/// Files bigger than this are not opened in the editor.
const MAX_EDIT: u64 = 5 * 1024 * 1024;

#[derive(Serialize, Debug)]
pub struct OpenFile {
    pub path: String,
    pub text: String,
    /// content hash on disk when opened; pass back to `fs_save`
    pub version: String,
    pub size: u64,
    pub crlf: bool,
    pub bom: bool,
    pub encoding: String,
    pub readonly: bool,
}

pub fn open_in(ws: &Path, path: &str) -> Result<OpenFile, String> {
    let p = crate::tools::resolve(ws, path)?;
    let md = std::fs::metadata(&p).map_err(|_| "file not found".to_string())?;
    if md.is_dir() {
        return Err("that is a folder".into());
    }
    if md.len() > MAX_EDIT {
        return Err(format!("file is too large to edit here ({} MB)", md.len() / 1_048_576));
    }
    let bytes = std::fs::read(&p).map_err(|e| e.to_string())?;
    if textfile::looks_binary(&bytes) {
        return Err("binary file".into());
    }
    let tf = textfile::decode(&bytes);
    Ok(OpenFile {
        path: p.to_string_lossy().into_owned(),
        version: textfile::version_of(&bytes),
        size: md.len(),
        crlf: tf.crlf,
        bom: tf.bom,
        encoding: format!("{:?}", tf.encoding).to_lowercase(),
        // saving a lossily-decoded file would corrupt it
        readonly: tf.lossy || md.permissions().readonly(),
        text: tf.text,
    })
}

#[derive(Serialize, Debug)]
pub struct Saved {
    pub version: String,
}

/// Save with the file's own encoding and line endings. `expected` is the version the editor
/// opened; if the file changed on disk since, nothing is written and the error starts with "conflict".
pub fn save_in(ws: &Path, path: &str, text: &str, expected: Option<&str>) -> Result<Saved, String> {
    let p = crate::tools::resolve(ws, path)?;
    let like = match std::fs::read(&p) {
        Ok(bytes) => {
            if let Some(e) = expected {
                if textfile::version_of(&bytes) != e {
                    return Err("conflict: the file changed on disk since it was opened".into());
                }
            }
            let tf = textfile::decode(&bytes);
            if tf.lossy {
                return Err("this file isn't valid UTF-8; saving would corrupt it".into());
            }
            tf
        }
        Err(_) => textfile::new_file_like(text),
    };
    let bytes = textfile::encode(&text.replace("\r\n", "\n"), &like);
    write_atomic(&p, &bytes).map_err(|e| format!("could not save: {e}"))?;
    Ok(Saved { version: textfile::version_of(&bytes) })
}

#[tauri::command]
pub fn fs_open(path: String) -> Result<OpenFile, String> {
    open_in(&workspace()?, &path)
}

#[tauri::command]
pub fn fs_save(path: String, text: String, expected_version: Option<String>) -> Result<Saved, String> {
    save_in(&workspace()?, &path, &text, expected_version.as_deref())
}

#[tauri::command]
pub fn fs_create(path: String, is_dir: bool) -> Result<String, String> {
    let p = in_workspace(&path)?;
    if p.exists() {
        return Err(format!("{} already exists", p.display()));
    }
    if is_dir {
        std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
    } else {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&p, b"").map_err(|e| e.to_string())?;
    }
    Ok(p.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn fs_rename(from: String, to: String) -> Result<String, String> {
    let a = in_workspace(&from)?;
    let b = in_workspace(&to)?;
    if !a.exists() {
        return Err("not found".into());
    }
    if b.exists() && !a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()) {
        return Err(format!("{} already exists", b.display()));
    }
    std::fs::rename(&a, &b).map_err(|e| format!("rename failed: {e}"))?;
    Ok(b.to_string_lossy().into_owned())
}

/// Delete to the Recycle Bin, so it can be recovered.
#[tauri::command]
pub fn fs_delete(path: String) -> Result<(), String> {
    let p = in_workspace(&path)?;
    let ws = workspace()?.canonicalize().map_err(|e| e.to_string())?;
    if p == ws {
        return Err("refusing to delete the workspace folder itself".into());
    }
    trash::delete(&p).map_err(|e| format!("could not move to Recycle Bin: {e}"))
}

// ---- watching the workspace ------------------------------------------------------------------

static WATCHER: Mutex<Option<(PathBuf, notify::RecommendedWatcher)>> = Mutex::new(None);

/// Noise the UI never needs to hear about.
fn ignored_change(p: &Path) -> bool {
    p.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        matches!(s.as_ref(), ".git" | "node_modules" | "target" | "__pycache__" | ".venv")
    }) || p.file_name().map(|n| n.to_string_lossy().ends_with(".tmp")).unwrap_or(false)
}

/// Watch the workspace and emit `fs-changed` (paths, batched every ~300 ms) on any change.
#[tauri::command]
pub fn fs_watch(app: AppHandle) -> Result<(), String> {
    use notify::{RecursiveMode, Watcher};
    let ws = workspace()?;
    let mut g = WATCHER.lk();
    if g.as_ref().map(|(p, _)| p == &ws).unwrap_or(false) {
        return Ok(());
    }
    *g = None; // stop watching the old folder
    let (tx, rx) = std::sync::mpsc::channel::<PathBuf>();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            for p in ev.paths {
                let _ = tx.send(p);
            }
        }
    })
    .map_err(|e| e.to_string())?;
    w.watch(&ws, RecursiveMode::Recursive).map_err(|e| format!("cannot watch {}: {e}", ws.display()))?;
    std::thread::spawn(move || {
        // debounce: collect until things are quiet for 300 ms, then emit once
        while let Ok(first) = rx.recv() {
            let mut batch = BTreeSet::new();
            batch.insert(first);
            while let Ok(p) = rx.recv_timeout(Duration::from_millis(300)) {
                batch.insert(p);
                if batch.len() > 500 {
                    break;
                }
            }
            let paths: Vec<String> = batch.into_iter().filter(|p| !ignored_change(p)).map(|p| p.to_string_lossy().into_owned()).collect();
            if !paths.is_empty() {
                let _ = app.emit("fs-changed", paths);
            }
        }
        // channel closed: the watcher was replaced
    });
    *g = Some((ws, w));
    Ok(())
}

#[derive(Serialize)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// matched by the workspace .gitignore (shown dimmed)
    pub ignored: bool,
}

const HIDDEN: &[&str] = &[".git", "node_modules", "$RECYCLE.BIN", "System Volume Information"];

#[tauri::command]
pub fn fs_list(path: String) -> Result<Vec<FsEntry>, String> {
    let path = in_workspace(&path)?;
    let ws = workspace()?.canonicalize().map_err(|e| e.to_string())?;
    let (gi, _) = ignore::gitignore::Gitignore::new(ws.join(".gitignore"));
    let mut out = vec![];
    for e in std::fs::read_dir(&path).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if HIDDEN.contains(&name.as_str()) {
            continue;
        }
        let md = e.metadata().ok();
        let is_dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let ignored = e.path().strip_prefix(&ws).map(|rel| gi.matched_path_or_any_parents(rel, is_dir).is_ignore()).unwrap_or(false);
        out.push(FsEntry { name, path: e.path().to_string_lossy().into_owned(), is_dir, size: md.map(|m| m.len()).unwrap_or(0), ignored });
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

#[derive(Serialize)]
pub struct Preview {
    pub text: String,
    pub truncated: bool,
    pub binary: bool,
    pub size: u64,
}

#[tauri::command]
pub fn fs_read(path: String) -> Result<Preview, String> {
    use std::io::Read;
    let path = in_workspace(&path)?;
    let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    let mut f = std::fs::File::open(&path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 200_000.min(size as usize)];
    f.read_exact(&mut buf).map_err(|e| e.to_string())?;
    let binary = crate::textfile::looks_binary(&buf);
    Ok(Preview { text: if binary { String::new() } else { crate::textfile::decode(&buf).text }, truncated: size as usize > buf.len(), binary, size })
}

/// Open an http(s) link from rendered markdown in the default browser.
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    let ok = (url.starts_with("http://") || url.starts_with("https://")) && !url.chars().any(|c| c.is_control() || c == ' ' || c == '"');
    if !ok {
        return Err("only plain http(s) links can be opened".into());
    }
    std::process::Command::new("explorer").arg(&url).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn fs_reveal(path: String) -> Result<(), String> {
    let p = in_workspace(&path)?;
    if !p.exists() {
        return Err("not found".into());
    }
    let mut c = std::process::Command::new("explorer");
    if p.is_file() {
        c.arg(format!("/select,{}", p.display()));
    } else {
        c.arg(p);
    }
    c.spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("harness_fsx_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn open_and_save_keep_format_and_detect_conflicts() {
        let ws = tmp("save");
        std::fs::write(ws.join("a.txt"), b"\xEF\xBB\xBFone\r\ntwo\r\n").unwrap();
        let f = open_in(&ws, "a.txt").unwrap();
        assert_eq!(f.text, "one\ntwo\n");
        assert!(f.crlf && f.bom && !f.readonly);
        let s = save_in(&ws, "a.txt", "one\n2\n", Some(&f.version)).unwrap();
        assert_eq!(std::fs::read(ws.join("a.txt")).unwrap(), b"\xEF\xBB\xBFone\r\n2\r\n");
        // stale version: someone else changed it
        std::fs::write(ws.join("a.txt"), b"outside edit").unwrap();
        assert!(save_in(&ws, "a.txt", "mine", Some(&s.version)).unwrap_err().starts_with("conflict"));
        assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "outside edit");
        // new file and refusals
        save_in(&ws, "new/b.rs", "fn main() {}\n", None).unwrap();
        assert!(ws.join("new/b.rs").exists());
        assert!(open_in(&ws, "..\\..\\Windows\\win.ini").is_err());
        assert!(save_in(&ws, "C:\\evil.txt", "x", None).is_err());
        std::fs::write(ws.join("bin.dat"), b"\0\x01\x02").unwrap();
        assert_eq!(open_in(&ws, "bin.dat").unwrap_err(), "binary file");
    }

    #[test]
    fn ignores_noisy_paths() {
        assert!(ignored_change(Path::new("D:\\p\\.git\\index")));
        assert!(ignored_change(Path::new("D:\\p\\target\\debug\\x")));
        assert!(ignored_change(Path::new("D:\\p\\.a.json.123.tmp")));
        assert!(!ignored_change(Path::new("D:\\p\\src\\main.rs")));
    }
}
