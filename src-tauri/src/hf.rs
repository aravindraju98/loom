//! Hugging Face model search + GGUF download (user-initiated; talks only to huggingface.co).
use crate::store;
use crate::util::LockExt;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};
use tokio::io::AsyncWriteExt;

#[derive(Default)]
pub struct Hf {
    jobs: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("harness-app")
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())
}

fn auth(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let t = store::load_settings().hf_token;
    if t.trim().is_empty() { rb } else { rb.bearer_auth(t.trim()) }
}

/// `owner/name` using only characters Hugging Face allows.
fn valid_repo(r: &str) -> bool {
    let mut it = r.split('/');
    let (Some(a), Some(b), None) = (it.next(), it.next(), it.next()) else { return false };
    let ok = |s: &str| !s.is_empty() && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    ok(a) && ok(b)
}

/// A relative path made only of normal components (no `..`, drive letters or leading slash).
fn safe_rel(p: &str) -> Option<PathBuf> {
    let pb = PathBuf::from(p);
    if p.is_empty() || !pb.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(pb)
}

/// Accept "owner/name" or a huggingface.co URL and return "owner/name".
pub fn parse_repo(s: &str) -> Option<String> {
    let t = s.trim().trim_end_matches('/');
    let t = t.strip_prefix("https://huggingface.co/").or_else(|| t.strip_prefix("http://huggingface.co/")).unwrap_or(t);
    let parts: Vec<&str> = t.split('/').collect();
    if parts.len() >= 2 {
        let r = format!("{}/{}", parts[0], parts[1]);
        if valid_repo(&r) {
            return Some(r);
        }
    }
    None
}

#[derive(Serialize)]
pub struct HfModel {
    pub id: String,
    pub downloads: u64,
    pub likes: u64,
}

#[tauri::command]
pub async fn hf_search(query: String) -> Result<Vec<HfModel>, String> {
    // pasting a repo id or URL goes straight to that repo
    if let Some(r) = parse_repo(&query) {
        if query.contains('/') {
            return Ok(vec![HfModel { id: r, downloads: 0, likes: 0 }]);
        }
    }
    let rb = client()?.get("https://huggingface.co/api/models").query(&[
        ("search", query.trim()),
        ("filter", "gguf"),
        ("sort", "downloads"),
        ("direction", "-1"),
        ("limit", "30"),
    ]);
    let r = auth(rb).send().await.map_err(|e| format!("cannot reach huggingface.co: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("Hugging Face returned {}", r.status()));
    }
    let v: Value = r.json().await.map_err(|e| e.to_string())?;
    Ok(v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    Some(HfModel {
                        id: m["id"].as_str()?.to_string(),
                        downloads: m["downloads"].as_u64().unwrap_or(0),
                        likes: m["likes"].as_u64().unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

#[derive(Serialize, Deserialize, Clone)]
pub struct HfPart {
    pub path: String,
    pub size: u64,
}

#[derive(Serialize)]
pub struct HfGroup {
    pub label: String,
    /// "model" | "mmproj"
    pub kind: String,
    pub files: Vec<HfPart>,
    pub total: u64,
}

#[tauri::command]
pub async fn hf_files(repo: String) -> Result<Vec<HfGroup>, String> {
    let repo = parse_repo(&repo).ok_or("not a valid Hugging Face repo (expected owner/name)")?;
    let rb = client()?.get(format!("https://huggingface.co/api/models/{repo}/tree/main")).query(&[("recursive", "true")]);
    let r = auth(rb).send().await.map_err(|e| format!("cannot reach huggingface.co: {e}"))?;
    if r.status() == 401 || r.status() == 403 {
        return Err("this repo is gated or private: add a Hugging Face token in the Hub tab".into());
    }
    if !r.status().is_success() {
        return Err(format!("Hugging Face returned {}", r.status()));
    }
    let v: Value = r.json().await.map_err(|e| e.to_string())?;
    let shard = regex::Regex::new(r"^(.*)-(\d{5})-of-(\d{5})\.gguf$").unwrap();
    let mut groups: BTreeMap<String, HfGroup> = BTreeMap::new();
    for f in v.as_array().cloned().unwrap_or_default() {
        if f["type"] != "file" {
            continue;
        }
        let Some(path) = f["path"].as_str() else { continue };
        if !path.to_lowercase().ends_with(".gguf") || safe_rel(path).is_none() {
            continue;
        }
        let size = f["lfs"]["size"].as_u64().or_else(|| f["size"].as_u64()).unwrap_or(0);
        let label = shard.captures(path).map(|c| c[1].to_string()).unwrap_or_else(|| path.trim_end_matches(".gguf").trim_end_matches(".GGUF").to_string());
        let kind = if path.to_lowercase().contains("mmproj") { "mmproj" } else { "model" };
        let g = groups.entry(label.clone()).or_insert(HfGroup { label, kind: kind.into(), files: vec![], total: 0 });
        g.total += size;
        g.files.push(HfPart { path: path.to_string(), size });
    }
    let mut out: Vec<HfGroup> = groups.into_values().collect();
    for g in out.iter_mut() {
        g.files.sort_by(|a, b| a.path.cmp(&b.path));
    }
    out.sort_by(|a, b| (a.kind.clone(), a.total).cmp(&(b.kind.clone(), b.total)));
    Ok(out)
}

fn emit(app: &AppHandle, id: &str, state: &str, extra: Value) {
    let mut p = json!({"id": id, "state": state});
    if let (Some(o), Some(e)) = (p.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            o.insert(k.clone(), v.clone());
        }
    }
    let _ = app.emit("hf-progress", p);
}

/// Start a background download of the given files into `<dest_dir>/<owner>/<name>/`. Returns the job id.
#[tauri::command]
pub async fn hf_download(app: AppHandle, hf: State<'_, Hf>, repo: String, files: Vec<HfPart>, dest_dir: String) -> Result<String, String> {
    let repo = parse_repo(&repo).ok_or("not a valid Hugging Face repo")?;
    if files.is_empty() {
        return Err("no files selected".into());
    }
    let mut rels = vec![];
    for f in &files {
        let rel = safe_rel(&f.path).filter(|_| f.path.to_lowercase().ends_with(".gguf")).ok_or_else(|| format!("refusing unsafe file path: {}", f.path))?;
        rels.push(rel);
    }
    if dest_dir.trim().is_empty() {
        return Err("choose a download folder (Settings → Model folders)".into());
    }
    let base = Path::new(dest_dir.trim()).join(repo.replace('/', std::path::MAIN_SEPARATOR_STR));
    let id = format!("{repo}::{}", files[0].path);
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut j = hf.jobs.lk();
        if j.contains_key(&id) {
            return Err("already downloading".into());
        }
        j.insert(id.clone(), cancel.clone());
    }
    let (job_id, app2) = (id.clone(), app.clone());
    tauri::async_runtime::spawn(async move {
        let res = run_download(&app2, &job_id, &repo, &files, &rels, &base, &cancel).await;
        let st = app2.state::<Hf>();
        st.jobs.lk().remove(&job_id);
        match res {
            Ok(true) => emit(&app2, &job_id, "done", json!({"dir": base.to_string_lossy()})),
            Ok(false) => emit(&app2, &job_id, "cancelled", json!({})),
            Err(e) => emit(&app2, &job_id, "error", json!({"error": e})),
        }
    });
    Ok(id)
}

use tauri::Manager;

async fn run_download(app: &AppHandle, id: &str, repo: &str, files: &[HfPart], rels: &[PathBuf], base: &Path, cancel: &AtomicBool) -> Result<bool, String> {
    let client = client()?;
    let total: u64 = files.iter().map(|f| f.size).sum();
    let mut finished: u64 = 0;
    for (i, (f, rel)) in files.iter().zip(rels).enumerate() {
        let dest = base.join(rel);
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("cannot create {}: {e}", p.display()))?;
        }
        if f.size > 0 && std::fs::metadata(&dest).map(|m| m.len() == f.size).unwrap_or(false) {
            finished += f.size;
            continue; // already complete
        }
        let part = PathBuf::from(format!("{}.part", dest.display()));
        let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let url = format!("https://huggingface.co/{repo}/resolve/main/{}", f.path);
        let mut rb = client.get(&url);
        if have > 0 {
            rb = rb.header("Range", format!("bytes={have}-"));
        }
        let resp = auth(rb).send().await.map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status();
        if status == 401 || status == 403 {
            return Err("access denied: this model is gated. Accept its terms on huggingface.co and add a token in the Hub tab".into());
        }
        if status == 416 {
            // part is already complete
            std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
            finished += f.size;
            continue;
        }
        if !status.is_success() {
            return Err(format!("{} returned {status}", f.path));
        }
        let resumed = status == 206;
        if !resumed {
            have = 0;
        }
        let mut out = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .open(&part)
            .await
            .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
        let mut done = have;
        let mut last = Instant::now();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            if cancel.load(Ordering::SeqCst) {
                let _ = out.flush().await;
                return Ok(false);
            }
            let chunk = chunk.map_err(|e| format!("download interrupted: {e}"))?;
            out.write_all(&chunk).await.map_err(|e| format!("disk write failed: {e}"))?;
            done += chunk.len() as u64;
            if last.elapsed() > Duration::from_millis(250) {
                last = Instant::now();
                emit(app, id, "downloading", json!({"file": f.path, "idx": i + 1, "count": files.len(), "done": finished + done, "total": total}));
            }
        }
        out.flush().await.map_err(|e| e.to_string())?;
        drop(out);
        if f.size > 0 && done != f.size {
            return Err(format!("{} ended early ({done} of {} bytes); press download again to resume", f.path, f.size));
        }
        std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
        finished += f.size.max(done);
    }
    Ok(true)
}

#[tauri::command]
pub fn hf_cancel(hf: State<'_, Hf>, id: String) {
    if let Some(c) = hf.jobs.lk().get(&id) {
        c.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_and_path_validation() {
        assert_eq!(parse_repo("https://huggingface.co/unsloth/Qwen3-GGUF/tree/main").as_deref(), Some("unsloth/Qwen3-GGUF"));
        assert_eq!(parse_repo("unsloth/Qwen3-GGUF").as_deref(), Some("unsloth/Qwen3-GGUF"));
        assert!(parse_repo("qwen").is_none());
        assert!(parse_repo("../evil/x").is_none());
        assert!(safe_rel("Q4/model-00001-of-00002.gguf").is_some());
        assert!(safe_rel("../x.gguf").is_none());
        assert!(safe_rel("/abs/x.gguf").is_none());
        assert!(safe_rel("C:\\x.gguf").is_none());
    }
}
