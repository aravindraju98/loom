//! JSON persistence under %APPDATA%\harness.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

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
            model_dirs: vec![
                r"E:\PolyStrata-data\models".into(),
                r"E:\home\aravind\.var\app\ai.lmstudio.lm-studio\.lmstudio\models".into(),
            ],
            polystrata_dir: r"D:\PolyStrata".into(),
            llama_server: r"D:\Linger for AMD\amd-test\engine\b11370-vulkan\llama-server.exe".into(),
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
        }
    }
}

fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn load_settings() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
}

pub fn save_settings(s: &Settings) -> Result<(), String> {
    let t = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(settings_path(), t).map_err(|e| e.to_string())
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

#[tauri::command]
pub fn settings_set(settings: Settings) -> Result<(), String> {
    save_settings(&settings)
}

#[tauri::command]
pub fn chat_list() -> Vec<Value> {
    let mut v: Vec<Value> = std::fs::read_dir(chats_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let t = std::fs::read_to_string(e.path()).ok()?;
            let j: Value = serde_json::from_str(&t).ok()?;
            Some(serde_json::json!({
                "id": j.get("id")?,
                "title": j.get("title").cloned().unwrap_or(Value::Null),
                "updated": j.get("updated").cloned().unwrap_or(Value::from(0)),
            }))
        })
        .collect();
    v.sort_by_key(|j| std::cmp::Reverse(j["updated"].as_u64().unwrap_or(0)));
    v
}

#[tauri::command]
pub fn chat_load(id: String) -> Result<Value, String> {
    let p = chats_dir().join(format!("{}.json", safe_id(&id)?));
    let t = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
    serde_json::from_str(&t).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn chat_save(id: String, chat: Value) -> Result<(), String> {
    let p = chats_dir().join(format!("{}.json", safe_id(&id)?));
    std::fs::write(p, serde_json::to_string(&chat).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn chat_delete(id: String) -> Result<(), String> {
    let p = chats_dir().join(format!("{}.json", safe_id(&id)?));
    std::fs::remove_file(p).map_err(|e| e.to_string())
}
