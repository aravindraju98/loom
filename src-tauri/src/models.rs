//! Model library: scan GGUF files, read their headers, and decide which engine runs them.
use crate::{gguf, store};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone, Debug)]
pub struct ModelEntry {
    /// first shard path; unique id
    pub id: String,
    pub name: String,
    pub arch: String,
    pub quant: String,
    pub size_bytes: u64,
    pub ctx_train: u64,
    pub moe: bool,
    pub parts: Vec<String>,
    pub mmproj: Option<String>,
    /// "polystrata" | "llama" (after override)
    pub engine: String,
    /// "polystrata" | "llama" (before override)
    pub auto_engine: String,
    pub profile: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub path: PathBuf,
    pub display: String,
    pub model_file: String, // lowercase file name of the main model gguf
}

pub fn strata_profiles(polystrata_dir: &str) -> Vec<Profile> {
    let dir = Path::new(polystrata_dir).join("terminal");
    let mut out = vec![];
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let p = e.path();
        let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
        if !(n.starts_with("strata-") && n.ends_with(".json")) {
            continue;
        }
        let Ok(t) = std::fs::read_to_string(&p) else { continue };
        let Ok(j) = serde_json::from_str::<serde_json::Value>(t.trim_start_matches('\u{feff}')) else { continue };
        let args: Vec<String> = j["args"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        // the main model follows --model or --native
        let mut model = None;
        for w in args.windows(2) {
            if w[0] == "--model" || w[0] == "--native" {
                model = Some(w[1].clone());
                break;
            }
        }
        let Some(m) = model else { continue };
        let file = Path::new(&m).file_name().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
        let display = j["display_name"].as_str().or(j["model_name"].as_str()).unwrap_or(&n).to_string();
        out.push(Profile { path: p, display, model_file: file });
    }
    out
}

fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    if depth > 6 {
        return;
    }
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, depth + 1, out);
        } else if p.extension().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case("gguf")).unwrap_or(false) {
            out.push(p);
        }
    }
}

/// "foo-00002-of-00005.gguf" -> Some((2, 5))
fn shard_of(name: &str) -> Option<(u32, u32)> {
    let stem = name.strip_suffix(".gguf")?;
    let i = stem.rfind("-of-")?;
    let total: u32 = stem[i + 4..].parse().ok()?;
    let head = &stem[..i];
    let j = head.rfind('-')?;
    let idx: u32 = head[j + 1..].parse().ok()?;
    Some((idx, total))
}

fn quant_from_name(name: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?i)((?:UD-)?(?:IQ\d_[A-Z0-9]+|Q\d_[A-Z0-9_]+|Q\d_\d|BF16|F16|F32))").ok()?;
    re.captures(name).map(|c| c[1].to_uppercase())
}

pub fn scan(settings: &store::Settings) -> Vec<ModelEntry> {
    let profiles = strata_profiles(&settings.polystrata_dir);
    let mut files = vec![];
    for d in &settings.model_dirs {
        walk(Path::new(d), 0, &mut files);
    }
    let mmprojs: Vec<PathBuf> = files
        .iter()
        .filter(|p| p.file_name().and_then(|s| s.to_str()).map(|s| s.to_lowercase().contains("mmproj")).unwrap_or(false))
        .cloned()
        .collect();
    let mut seen: HashSet<(String, u64)> = HashSet::new();
    let mut out = vec![];
    for f in &files {
        let fname = f.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
        if fname.to_lowercase().contains("mmproj") {
            continue;
        }
        let shard = shard_of(&fname);
        if matches!(shard, Some((i, _)) if i != 1) {
            continue;
        }
        let mut parts = vec![f.clone()];
        if let Some((_, total)) = shard {
            let prefix = &fname[..fname.rfind("-of-").unwrap()];
            let prefix = &prefix[..prefix.rfind('-').unwrap()];
            for k in 2..=total {
                let p = f.with_file_name(format!("{prefix}-{k:05}-of-{total:05}.gguf"));
                if p.exists() {
                    parts.push(p);
                }
            }
        }
        let size: u64 = parts.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
        if !seen.insert((fname.to_lowercase(), size)) {
            continue; // same file reachable from two folders
        }
        let info = gguf::read_info(f).unwrap_or_default();
        let quant = quant_from_name(&fname)
            .or_else(|| info.file_type.and_then(gguf::file_type_name).map(String::from))
            .unwrap_or_else(|| "?".into());
        let dir = f.parent();
        let mmproj = mmprojs
            .iter()
            .find(|m| m.parent() == dir)
            .or_else(|| {
                // a projector for the same family anywhere in the library
                let stem = fname.split(['-', '_']).next().unwrap_or("").to_lowercase();
                mmprojs.iter().find(|m| {
                    !stem.is_empty()
                        && m.file_name().and_then(|s| s.to_str()).map(|s| s.to_lowercase().contains(&stem)).unwrap_or(false)
                })
            })
            .map(|m| m.to_string_lossy().into_owned());
        let id = f.to_string_lossy().into_owned();
        let profile = profiles.iter().find(|p| p.model_file == fname.to_lowercase());
        let auto_engine = if profile.is_some() { "polystrata" } else { "llama" };
        let mut engine = settings.engine_overrides.get(&id).map(String::as_str).unwrap_or(auto_engine).to_string();
        if engine == "polystrata" && profile.is_none() {
            engine = "llama".into(); // no launch profile: PolyStrata can't start it
        }
        // some GGUFs carry a useless general.name ("Hf"); fall back to the file name then
        let name = if info.name.chars().count() >= 5 { info.name.clone() } else { fname.trim_end_matches(".gguf").to_string() };
        out.push(ModelEntry {
            id,
            name: profile.map(|p| p.display.clone()).unwrap_or(name),
            arch: info.arch,
            quant,
            size_bytes: size,
            ctx_train: info.ctx_train.unwrap_or(0),
            moe: info.expert_count.unwrap_or(0) > 1,
            parts: parts.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
            mmproj,
            engine,
            auto_engine: auto_engine.into(),
            profile: profile.map(|p| p.path.to_string_lossy().into_owned()),
        });
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

static CACHE: std::sync::Mutex<Vec<ModelEntry>> = std::sync::Mutex::new(Vec::new());

/// The last scan's result, scanning once if there is none yet. Loading a model uses this so Load starts at once.
pub fn cached_or_scan(settings: &store::Settings) -> Vec<ModelEntry> {
    {
        let c = CACHE.lock().unwrap();
        if !c.is_empty() {
            return c.clone();
        }
    }
    let v = scan(settings);
    *CACHE.lock().unwrap() = v.clone();
    v
}

#[tauri::command]
pub async fn models_scan() -> Result<Vec<ModelEntry>, String> {
    let s = store::load_settings();
    let v = tauri::async_runtime::spawn_blocking(move || scan(&s))
        .await
        .map_err(|e| format!("scan crashed: {e}"))?;
    *CACHE.lock().unwrap() = v.clone();
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shard_parse() {
        assert_eq!(shard_of("Model-Q2_0-00002-of-00005.gguf"), Some((2, 5)));
        assert_eq!(shard_of("Model-Q4_K_M.gguf"), None);
    }
    #[test]
    fn quant_parse() {
        assert_eq!(quant_from_name("Qwen3.6-35B-A3B-UD-Q4_K_M.gguf").as_deref(), Some("UD-Q4_K_M"));
        assert_eq!(quant_from_name("x-GSQ-RCO-Q2_0-00001-of-00002.gguf").as_deref(), Some("Q2_0"));
    }
}

#[cfg(test)]
mod scan_debug {
    #[test]
    fn scan_default_settings() {
        let t = std::time::Instant::now();
        let r = std::panic::catch_unwind(|| super::scan(&crate::store::Settings::default()));
        println!("scan took {:?}", t.elapsed());
        match r {
            Ok(v) => { println!("{} models", v.len()); for m in v { println!("{} | {} | {}", m.name, m.engine, m.id); } }
            Err(_) => println!("PANIC in scan"),
        }
    }
}
