//! File explorer commands for the Files panel. Confined to the chosen workspace folder so the
//! webview (which renders model output) can never be used to read arbitrary files.
use serde::Serialize;
use std::path::{Path, PathBuf};

fn in_workspace(path: &str) -> Result<PathBuf, String> {
    let ws = crate::store::load_settings().workspace;
    if ws.trim().is_empty() {
        return Err("choose a workspace folder first".into());
    }
    crate::tools::resolve(Path::new(&ws), path)
}

#[derive(Serialize)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

const HIDDEN: &[&str] = &[".git", "node_modules", "$RECYCLE.BIN", "System Volume Information"];

#[tauri::command]
pub fn fs_list(path: String) -> Result<Vec<FsEntry>, String> {
    let path = in_workspace(&path)?;
    let mut out = vec![];
    for e in std::fs::read_dir(&path).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if HIDDEN.contains(&name.as_str()) {
            continue;
        }
        let md = e.metadata().ok();
        out.push(FsEntry {
            name,
            path: e.path().to_string_lossy().into_owned(),
            is_dir: md.as_ref().map(|m| m.is_dir()).unwrap_or(false),
            size: md.map(|m| m.len()).unwrap_or(0),
        });
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
