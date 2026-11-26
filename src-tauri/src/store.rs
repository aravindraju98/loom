//! JSON persistence under %APPDATA%\harness.
use crate::util::{write_atomic, LockExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let d = base.join("harness");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn chats_dir() -> PathBuf {
    let d = data_dir().join("chats");
    let _ = std::fs::create_dir_all(&d);
    d
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub model_dirs: Vec<String>,
    pub polystrata_dir: String,
    pub llama_server: String,
    pub workspace: String,
    pub ctx: u32,
    pub max_turns: u32,
    pub auto_approve: bool,
    pub temperature: f32,
    pub system_prompt: String,
    /// model id (first shard path) -> "polystrata" | "llama"
    pub engine_overrides: HashMap<String, String>,

    // --- hosting (LM Studio "server" options); applied the next time a model is loaded ---
    /// false = 127.0.0.1 only, true = 0.0.0.0 (reachable from other devices on the network)
    pub serve_on_network: bool,
    /// when set, the engine requires it on /v1/* (Bearer); the built-in chat sends it automatically
    pub api_key: String,
    pub poly_port: u16,
    pub llama_port: u16,
    /// llama.cpp: -1 = auto (--fit), otherwise an exact `-ngl`
    pub gpu_layers: i32,
    /// llama.cpp flash attention: "auto" | "on" | "off"
    pub flash_attn: String,
    /// llama.cpp KV cache types ("" = default f16), e.g. q8_0
    pub cache_type_k: String,
    pub cache_type_v: String,
    /// llama.cpp parallel slots (0 = default)
    pub parallel: u32,
    /// extra llama-server arguments, appended last (quotes allowed)
    pub llama_extra_args: String,
    /// remote OpenAI-compatible endpoints
    pub providers: Vec<Provider>,
    /// optional Hugging Face access token (gated models)
    pub hf_token: String,
    /// keys above are kept in Windows Credential Manager, not in settings.json
    pub secrets_in_keyring: bool,

    // --- permissions ---
    /// shell commands starting with one of these run without asking (e.g. "git status", "npm test")
    pub shell_allow: Vec<String>,
    /// shell commands containing one of these are refused
    pub shell_deny: Vec<String>,
    /// tools trusted in a workspace without asking: "tool_name@C:\path\to\workspace"
    pub trusted: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Provider {
    pub id: String,
    pub name: String,
    /// e.g. https://api.openai.com/v1 , http://192.168.1.5:1234/v1 , https://openrouter.ai/api/v1
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            // machine-specific paths are chosen in Settings on first run, not baked in
            model_dirs: vec![],
            polystrata_dir: String::new(),
            llama_server: String::new(),
            workspace: String::new(),
            ctx: 32768,
            max_turns: 25,
            auto_approve: false,
            temperature: 0.7,
            system_prompt: "You are a helpful assistant running locally. When tools are available, use them to inspect and change files in the workspace instead of guessing. Be concise.".into(),
            engine_overrides: HashMap::new(),
            serve_on_network: false,
            api_key: String::new(),
            poly_port: 8090,
            llama_port: 8081,
            gpu_layers: -1,
            flash_attn: "auto".into(),
            cache_type_k: String::new(),
            cache_type_v: String::new(),
            parallel: 0,
            llama_extra_args: String::new(),
            providers: vec![],
            hf_token: String::new(),
            secrets_in_keyring: false,
            shell_allow: vec![],
            shell_deny: vec![],
            trusted: vec![],
        }
    }
}

fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

fn load_settings_file() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
}

pub fn load_settings() -> Settings {
    let mut s = load_settings_file();
    if s.secrets_in_keyring {
        s.api_key = kr_get("api_key").unwrap_or_default();
        s.hf_token = kr_get("hf_token").unwrap_or_default();
        for p in s.providers.iter_mut() {
            p.api_key = kr_get(&format!("provider:{}", p.id)).unwrap_or_default();
        }
    }
    s
}

// ---- secrets in the OS credential store ------------------------------------------------------

const KR_SERVICE: &str = "harness";

fn kr_get(name: &str) -> Option<String> {
    keyring::Entry::new(KR_SERVICE, name).ok()?.get_password().ok()
}

fn kr_set(name: &str, value: &str) -> Result<(), String> {
    let e = keyring::Entry::new(KR_SERVICE, name).map_err(|e| e.to_string())?;
    if value.is_empty() {
        match e.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(x) => Err(x.to_string()),
        }
    } else {
        e.set_password(value).map_err(|e| e.to_string())
    }
}

/// Store every secret in the credential store; returns a copy with secrets blanked for disk.
fn move_secrets_out(s: &Settings) -> Result<Settings, String> {
    let prev = load_settings_file();
    kr_set("api_key", &s.api_key)?;
    kr_set("hf_token", &s.hf_token)?;
    for p in &s.providers {
        kr_set(&format!("provider:{}", p.id), &p.api_key)?;
    }
    for old in prev.providers.iter().filter(|o| !s.providers.iter().any(|p| p.id == o.id)) {
        let _ = kr_set(&format!("provider:{}", old.id), "");
    }
    let mut disk = s.clone();
    disk.api_key.clear();
    disk.hf_token.clear();
    for p in disk.providers.iter_mut() {
        p.api_key.clear();
    }
    disk.secrets_in_keyring = true;
    Ok(disk)
}

/// Serialises every settings read-modify-write so two panels saving at once can't clobber each other.
static SETTINGS_LOCK: Mutex<()> = Mutex::new(());

pub fn save_settings(s: &Settings) -> Result<(), String> {
    let disk = match move_secrets_out(s) {
        Ok(d) => d,
        Err(e) => {
            crate::log::error("app", &format!("credential store unavailable, keys saved in settings.json: {e}"));
            Settings { secrets_in_keyring: false, ..s.clone() }
        }
    };
    let t = serde_json::to_string_pretty(&disk).map_err(|e| e.to_string())?;
    write_atomic(&settings_path(), t.as_bytes()).map_err(|e| format!("could not save settings: {e}"))?;
    // keep the log redactor in step with the keys it must hide
    let mut secrets = vec![s.api_key.clone(), s.hf_token.clone()];
    secrets.extend(s.providers.iter().map(|p| p.api_key.clone()));
    crate::log::set_secrets(secrets);
    Ok(())
}

/// Merge only the given top-level fields into the stored settings and return the result.
pub fn patch_settings(patch: &Value) -> Result<Settings, String> {
    let Some(p) = patch.as_object() else { return Err("settings patch must be an object".into()) };
    let _g = SETTINGS_LOCK.lk();
    let mut cur = serde_json::to_value(load_settings()).map_err(|e| e.to_string())?;
    let obj = cur.as_object_mut().ok_or("bad settings")?;
    for (k, v) in p {
        if !obj.contains_key(k) {
            return Err(format!("unknown setting '{k}'"));
        }
        obj.insert(k.clone(), v.clone());
    }
    let merged: Settings = serde_json::from_value(cur).map_err(|e| format!("invalid setting value: {e}"))?;
    save_settings(&merged)?;
    Ok(merged)
}

fn safe_id(id: &str) -> Result<&str, String> {
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        Ok(id)
    } else {
        Err("bad chat id".into())
    }
}

#[tauri::command]
pub fn settings_get() -> Settings {
    load_settings()
}

/// Kept for compatibility; replaces the whole settings object.
#[tauri::command]
pub fn settings_set(settings: Settings) -> Result<(), String> {
    let _g = SETTINGS_LOCK.lk();
    save_settings(&settings)
}

#[tauri::command]
pub fn settings_patch(patch: Value) -> Result<Settings, String> {
    patch_settings(&patch)
}

// ---- chats -------------------------------------------------------------------------------
// `index.json` holds {id: {title, updated}} so listing chats doesn't parse every chat file.

static CHATS_LOCK: Mutex<()> = Mutex::new(());

fn index_path(dir: &Path) -> PathBuf {
    dir.join("index.json")
}

fn summary(j: &Value) -> Option<(String, Value)> {
    let id = j.get("id")?.as_str()?.to_string();
    Some((id, json!({"title": j.get("title").cloned().unwrap_or(Value::Null), "updated": j.get("updated").cloned().unwrap_or(json!(0))})))
}

fn rebuild_index(dir: &Path) -> Map<String, Value> {
    let mut m = Map::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") || p.file_name() == Some("index.json".as_ref()) {
            continue;
        }
        let Ok(t) = std::fs::read_to_string(&p) else { continue };
        let Ok(j) = serde_json::from_str::<Value>(&t) else { continue };
        if let Some((id, s)) = summary(&j) {
            m.insert(id, s);
        }
    }
    let _ = write_atomic(&index_path(dir), Value::Object(m.clone()).to_string().as_bytes());
    m
}

fn read_index(dir: &Path) -> Map<String, Value> {
    match std::fs::read_to_string(index_path(dir)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(Value::Object(m)) => m,
        _ => rebuild_index(dir),
    }
}

fn write_index(dir: &Path, m: &Map<String, Value>) -> Result<(), String> {
    write_atomic(&index_path(dir), Value::Object(m.clone()).to_string().as_bytes()).map_err(|e| e.to_string())
}

pub fn list_chats_in(dir: &Path) -> Vec<Value> {
    let _g = CHATS_LOCK.lk();
    let mut v: Vec<Value> = read_index(dir)
        .into_iter()
        .map(|(id, s)| json!({"id": id, "title": s["title"], "updated": s["updated"]}))
        .collect();
    v.sort_by_key(|j| std::cmp::Reverse(j["updated"].as_u64().unwrap_or(0)));
    v
}

pub fn save_chat_in(dir: &Path, id: &str, chat: &Value) -> Result<(), String> {
    let id = safe_id(id)?;
    let text = serde_json::to_string(chat).map_err(|e| e.to_string())?;
    let _g = CHATS_LOCK.lk();
    write_atomic(&dir.join(format!("{id}.json")), text.as_bytes()).map_err(|e| format!("could not save chat: {e}"))?;
    let mut idx = read_index(dir);
    if let Some((_, s)) = summary(chat) {
        idx.insert(id.to_string(), s);
    }
    write_index(dir, &idx)
}

pub fn delete_chat_in(dir: &Path, id: &str) -> Result<(), String> {
    let id = safe_id(id)?;
    let _g = CHATS_LOCK.lk();
    let p = dir.join(format!("{id}.json"));
    if p.exists() {
        std::fs::remove_file(&p).map_err(|e| format!("could not delete chat: {e}"))?;
    }
    let mut idx = read_index(dir);
    idx.remove(id);
    write_index(dir, &idx)
}

#[tauri::command]
pub fn chat_list() -> Vec<Value> {
    list_chats_in(&chats_dir())
}

#[tauri::command]
pub fn chat_load(id: String) -> Result<Value, String> {
    let p = chats_dir().join(format!("{}.json", safe_id(&id)?));
    let t = std::fs::read_to_string(p).map_err(|e| format!("could not open chat: {e}"))?;
    serde_json::from_str(&t).map_err(|e| format!("chat file is damaged: {e}"))
}

#[tauri::command]
pub fn chat_save(id: String, chat: Value) -> Result<(), String> {
    save_chat_in(&chats_dir(), &id, &chat)
}

#[tauri::command]
pub fn chat_delete(id: String) -> Result<(), String> {
    delete_chat_in(&chats_dir(), &id)?;
    crate::checkpoint::delete_chat(&id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("harness_store_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn chat_index_tracks_saves_and_deletes() {
        let d = tmp("idx");
        save_chat_in(&d, "a1", &json!({"id":"a1","title":"First","updated":1,"messages":[]})).unwrap();
        save_chat_in(&d, "b2", &json!({"id":"b2","title":"Second","updated":5,"messages":[]})).unwrap();
        let l = list_chats_in(&d);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0]["id"], "b2"); // newest first
        delete_chat_in(&d, "b2").unwrap();
        assert_eq!(list_chats_in(&d).len(), 1);
        // a missing or corrupt index is rebuilt from the chat files
        std::fs::write(d.join("index.json"), "{broken").unwrap();
        assert_eq!(list_chats_in(&d)[0]["title"], "First");
        assert!(save_chat_in(&d, "../evil", &json!({})).is_err());
    }
}
