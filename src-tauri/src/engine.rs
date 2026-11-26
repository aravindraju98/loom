//! Starts and stops the one active inference engine: PolyStrata's server or llama.cpp's llama-server.
use crate::util::LockExt;
use crate::{models, store};
use serde::Serialize;
use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Not 8080: Steam's web helper holds it on this machine, and PolyStrata refuses to start on a busy port.
pub const POLY_PORT: u16 = 8090;
pub const LLAMA_PORT: u16 = 8081;

struct Running {
    child: Child,
    pid: u32,
    engine: String,
    model_id: String,
    model_name: String,
    port: u16,
    started: Instant,
    api_key: String,
    ctx: u32,
}

/// A remote OpenAI-compatible endpoint used instead of a local engine.
#[derive(Clone)]
pub struct Remote {
    pub id: String,
    pub name: String,
    pub base: String,
    pub model: String,
    pub key: String,
}

/// Where the chat client sends requests: base URL, model name, optional bearer key.
pub struct Endpoint {
    pub base: String,
    pub model: String,
    pub key: String,
    /// a server this app started (llama.cpp / PolyStrata): llama.cpp-only request fields are safe
    pub local: bool,
    /// "llama" | "polystrata" | "remote"
    pub engine: String,
    /// context window the server was started with (0 = unknown)
    pub ctx: u32,
}

/// Accepts `https://host/v1`, `https://host/v1/`, `https://host` and returns `https://host` (we append /v1/...).
pub fn norm_base(s: &str) -> String {
    let t = s.trim().trim_end_matches('/');
    t.strip_suffix("/v1").unwrap_or(t).trim_end_matches('/').to_string()
}

/// Split a command-line style string into args, honouring "double" and 'single' quotes.
pub fn split_args(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut q, mut has) = (vec![], String::new(), None::<char>, false);
    for c in s.chars() {
        match (q, c) {
            (Some(x), c) if c == x => q = None,
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => {
                q = Some(c);
                has = true;
            }
            (None, c) if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[derive(Default)]
pub struct Engine {
    remote: Mutex<Option<Remote>>,
    running: tokio::sync::Mutex<Option<Running>>,
    /// "idle" | "loading" | "ready" | "error"
    state: Mutex<String>,
    error: Mutex<String>,
    log: Arc<Mutex<VecDeque<String>>>,
    /// bumped on every load/unload so a stale readiness wait can notice it was superseded
    generation: Mutex<u64>,
}

#[derive(Serialize, Clone, Default)]
pub struct EngineStatus {
    pub state: String,
    pub engine: String,
    pub model_id: String,
    pub model_name: String,
    pub port: u16,
    pub base_url: String,
    pub error: String,
    pub uptime_secs: u64,
}

fn kill_tree(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags_hidden()
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

trait HiddenExt {
    fn creation_flags_hidden(&mut self) -> &mut Self;
}
impl HiddenExt for std::process::Command {
    fn creation_flags_hidden(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(CREATE_NO_WINDOW)
    }
}

impl Engine {
    fn push_log(&self, line: String) {
        let mut l = self.log.lk();
        if l.len() >= 400 {
            l.pop_front();
        }
        l.push_back(line);
    }

    fn set_state(&self, app: &AppHandle, st: &str, err: &str) {
        match st {
            "error" => crate::log::error("engine", &format!("engine error: {err}")),
            "ready" => crate::log::info("engine", "engine ready"),
            "loading" => crate::log::info("engine", "loading model…"),
            _ => crate::log::info("engine", &format!("engine {st}")),
        }
        *self.state.lk() = st.to_string();
        *self.error.lk() = err.to_string();
        let _ = app.emit("engine-changed", ());
    }

    pub async fn status(&self) -> EngineStatus {
        if let Some(r) = self.remote.lk().clone() {
            return EngineStatus {
                state: "ready".into(),
                engine: "openai".into(),
                model_id: format!("remote:{}", r.id),
                model_name: format!("{} · {}", r.name, r.model),
                base_url: r.base,
                ..Default::default()
            };
        }
        let state = self.state.lk().clone();
        let error = self.error.lk().clone();
        let g = self.running.lock().await;
        match g.as_ref() {
            Some(r) => EngineStatus {
                state: if state.is_empty() { "idle".into() } else { state },
                engine: r.engine.clone(),
                model_id: r.model_id.clone(),
                model_name: r.model_name.clone(),
                port: r.port,
                base_url: format!("http://127.0.0.1:{}", r.port),
                error,
                uptime_secs: r.started.elapsed().as_secs(),
            },
            None => EngineStatus {
                state: if state.is_empty() || state == "ready" || state == "loading" { "idle".into() } else { state },
                error,
                ..Default::default()
            },
        }
    }

    /// Port + served model name of the ready engine, for the chat client.
    pub async fn endpoint(&self) -> Option<Endpoint> {
        if let Some(r) = self.remote.lk().clone() {
            return Some(Endpoint { base: r.base, model: r.model, key: r.key, local: false, engine: "remote".into(), ctx: 0 });
        }
        if *self.state.lk() != "ready" {
            return None;
        }
        let g = self.running.lock().await;
        // PolyStrata serves its loaded model under any name ("strata" is its documented default);
        // a different name could trigger its switch-on-request path.
        g.as_ref().map(|r| {
            let served = if r.engine == "polystrata" { "strata".to_string() } else { r.model_name.clone() };
            Endpoint {
                base: format!("http://127.0.0.1:{}", r.port),
                model: served,
                key: r.api_key.clone(),
                local: true,
                engine: r.engine.clone(),
                ctx: r.ctx,
            }
        })
    }

    pub async fn stop(&self, app: Option<&AppHandle>) {
        *self.remote.lk() = None;
        *self.generation.lk() += 1;
        let mut g = self.running.lock().await;
        if let Some(mut r) = g.take() {
            kill_tree(r.pid);
            let _ = r.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), r.child.wait()).await;
        }
        drop(g);
        *self.state.lk() = "idle".into();
        self.error.lk().clear();
        if let Some(a) = app {
            let _ = a.emit("engine-changed", ());
        }
    }

    /// Synchronous kill for app exit.
    pub fn kill_blocking(&self) {
        if let Ok(mut g) = self.running.try_lock() {
            if let Some(r) = g.take() {
                kill_tree(r.pid);
            }
        }
    }
}

fn pipe_logs(engine: &Engine, child: &mut Child) {
    fn forward<R: tokio::io::AsyncRead + Unpin + Send + 'static>(r: R, log: Arc<Mutex<VecDeque<String>>>) {
        tokio::spawn(async move {
            let mut l = BufReader::new(r).lines();
            while let Ok(Some(line)) = l.next_line().await {
                if !line.trim().is_empty() {
                    crate::log::write(crate::log::level_of(&line), "engine", &line, None, None);
                }
                let mut g = log.lk();
                if g.len() >= 400 {
                    g.pop_front();
                }
                g.push_back(line);
            }
        });
    }
    if let Some(o) = child.stdout.take() {
        forward(o, engine.log.clone());
    }
    if let Some(o) = child.stderr.take() {
        forward(o, engine.log.clone());
    }
}

/// Profiles written by PolyStrata's setup can point at model files that have since moved. Write a copy (under
/// %APPDATA%\harness\profiles) where every missing .gguf path is replaced by the same file name found in the library.
/// PolyStrata's own files are never modified.
pub fn patched_profile(profile: &str, all: &[models::ModelEntry]) -> Result<String, String> {
    use serde_json::Value;
    let text = std::fs::read_to_string(profile).map_err(|e| format!("cannot read profile: {e}"))?;
    let mut json: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("bad profile JSON: {e}"))?;
    let mut by_name: std::collections::HashMap<String, String> = Default::default();
    for m in all {
        for p in m.parts.iter().chain(m.mmproj.iter()) {
            if let Some(n) = Path::new(p).file_name().and_then(|s| s.to_str()) {
                by_name.entry(n.to_lowercase()).or_insert_with(|| p.clone());
            }
        }
    }
    fn walk(v: &mut Value, by_name: &std::collections::HashMap<String, String>, changed: &mut Vec<String>) {
        match v {
            Value::String(s) => {
                if s.to_lowercase().ends_with(".gguf") && !Path::new(s.as_str()).exists() {
                    let name = Path::new(s.as_str()).file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
                    if let Some(found) = by_name.get(&name) {
                        changed.push(format!("{s} -> {found}"));
                        *s = found.clone();
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, by_name, changed)),
            Value::Object(o) => o.values_mut().for_each(|x| walk(x, by_name, changed)),
            _ => {}
        }
    }
    let mut changed = vec![];
    walk(&mut json, &by_name, &mut changed);
    let dir = store::data_dir().join("profiles");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let out = dir.join(Path::new(profile).file_name().unwrap_or_default());
    crate::util::write_atomic(&out, serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?.as_bytes()).map_err(|e| e.to_string())?;
    Ok(out.to_string_lossy().into_owned())
}

pub fn bind_host(s: &store::Settings) -> &'static str {
    if s.serve_on_network { "0.0.0.0" } else { "127.0.0.1" }
}

pub fn llama_args(m: &models::ModelEntry, s: &store::Settings, ctx: u32, port: u16, vision: bool) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-m".into(),
        m.id.clone(),
        "--host".into(),
        bind_host(s).into(),
        "--port".into(),
        port.to_string(),
        "-c".into(),
        ctx.to_string(),
        "--jinja".into(),
    ];
    if s.gpu_layers >= 0 {
        a.extend(["-ngl".into(), s.gpu_layers.to_string()]);
    } else {
        a.extend(["--fit".into(), "on".into()]);
    }
    if matches!(s.flash_attn.as_str(), "on" | "off") {
        a.extend(["-fa".into(), s.flash_attn.clone()]);
    }
    if !s.cache_type_k.is_empty() {
        a.extend(["-ctk".into(), s.cache_type_k.clone()]);
    }
    if !s.cache_type_v.is_empty() {
        a.extend(["-ctv".into(), s.cache_type_v.clone()]);
    }
    if s.parallel > 0 {
        a.extend(["-np".into(), s.parallel.to_string()]);
    }
    if !s.api_key.is_empty() {
        a.extend(["--api-key".into(), s.api_key.clone()]);
    }
    if vision {
        if let Some(p) = &m.mmproj {
            a.push("--mmproj".into());
            a.push(p.clone());
        }
    }
    a.extend(split_args(&s.llama_extra_args));
    a
}

/// List model ids from a remote OpenAI-compatible endpoint (also serves as a connection test).
#[tauri::command]
pub async fn provider_models(base_url: String, api_key: String) -> Result<Vec<String>, String> {
    let url = format!("{}/v1/models", norm_base(&base_url));
    let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().map_err(|e| e.to_string())?;
    let mut rq = client.get(&url);
    if !api_key.is_empty() {
        rq = rq.bearer_auth(api_key);
    }
    let r = rq.send().await.map_err(|e| format!("cannot reach {url}: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("{url} returned {}", r.status()));
    }
    let v: serde_json::Value = r.json().await.map_err(|e| format!("not an OpenAI-style /v1/models response: {e}"))?;
    let mut ids: Vec<String> = v["data"].as_array().map(|a| a.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect()).unwrap_or_default();
    ids.sort();
    Ok(ids)
}

#[tauri::command]
pub async fn engine_status(eng: State<'_, Engine>) -> Result<EngineStatus, String> {
    Ok(eng.status().await)
}

#[tauri::command]
pub fn engine_log(eng: State<'_, Engine>, lines: Option<usize>) -> Vec<String> {
    let l = eng.log.lk();
    let n = lines.unwrap_or(120).min(l.len());
    l.iter().skip(l.len() - n).cloned().collect()
}

#[tauri::command]
pub async fn engine_unload(app: AppHandle, eng: State<'_, Engine>) -> Result<(), String> {
    eng.stop(Some(&app)).await;
    Ok(())
}

/// Load a model on the engine that suits it (or the `engine` override). Returns when it answers.
#[tauri::command]
pub async fn engine_load(
    app: AppHandle,
    eng: State<'_, Engine>,
    model_id: String,
    engine: Option<String>,
    ctx: Option<u32>,
    vision: Option<bool>,
) -> Result<(), String> {
    let settings = store::load_settings();

    // remote OpenAI-compatible endpoint: no process to start
    if let Some(pid) = model_id.strip_prefix("remote:") {
        let p = settings.providers.iter().find(|p| p.id == pid).ok_or("endpoint not found; check Models → Remote endpoints")?;
        if p.model.trim().is_empty() {
            return Err("pick a model name for this endpoint first".into());
        }
        eng.stop(None).await;
        *eng.remote.lk() = Some(Remote {
            id: p.id.clone(),
            name: p.name.clone(),
            base: norm_base(&p.base_url),
            model: p.model.trim().to_string(),
            key: p.api_key.clone(),
        });
        eng.set_state(&app, "ready", "");
        return Ok(());
    }

    let all = {
        let s = settings.clone();
        tauri::async_runtime::spawn_blocking(move || models::cached_or_scan(&s)).await.map_err(|e| e.to_string())?
    };
    let m = all
        .iter()
        .find(|m| m.id == model_id)
        .cloned()
        .ok_or_else(|| format!("model not in the library ({model_id}); press Rescan"))?;
    let use_engine = engine.unwrap_or_else(|| m.engine.clone());
    let ctx = ctx.unwrap_or(settings.ctx);

    eng.stop(None).await;
    eng.log.lk().clear();
    let my_gen = *eng.generation.lk();

    let (mut cmd, port, health, display) = if use_engine == "polystrata" {
        let profile = m.profile.clone().ok_or("no PolyStrata launch profile (strata-*.json) matches this model")?;
        let profile = patched_profile(&profile, &all)?;
        let root = Path::new(&settings.polystrata_dir);
        let py = root.join("terminal").join(".venv").join("Scripts").join("python.exe");
        let server = root.join("terminal").join("serve").join("server.py");
        if !py.exists() || !server.exists() {
            return Err(format!("PolyStrata not found under {}", settings.polystrata_dir));
        }
        // the profile's display name is the id the server serves
        let port = if settings.poly_port == 0 { POLY_PORT } else { settings.poly_port };
        let mut c = Command::new(py);
        c.current_dir(root.join("terminal"))
            .env("PYTHONUTF8", "1")
            .args(["-u"])
            .arg(&server)
            .args(["--engine", "strata", "--config", &profile, "--host", bind_host(&settings), "--port", &port.to_string()]);
        if !settings.api_key.is_empty() {
            c.env("STRATA_API_KEY", &settings.api_key); // env, not argv, so it isn't visible in the process list
        }
        (c, port, "/api/health", m.name.clone())
    } else {
        let exe = Path::new(&settings.llama_server);
        if !exe.exists() {
            return Err(format!("llama-server not found at {}", settings.llama_server));
        }
        let port = if settings.llama_port == 0 { LLAMA_PORT } else { settings.llama_port };
        let mut c = Command::new(exe);
        c.current_dir(exe.parent().unwrap_or(Path::new(".")))
            .args(llama_args(&m, &settings, ctx, port, vision.unwrap_or(true)));
        (c, port, "/health", m.name.clone())
    };
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    eng.push_log(format!("[harness] starting {use_engine} for {}", m.name));
    let mut child = cmd.spawn().map_err(|e| format!("could not start {use_engine}: {e}"))?;
    let pid = child.id().unwrap_or(0);
    pipe_logs(&eng, &mut child);
    {
        let mut g = eng.running.lock().await;
        *g = Some(Running {
            child,
            pid,
            engine: use_engine.clone(),
            model_id: m.id.clone(),
            model_name: display,
            port,
            started: Instant::now(),
            api_key: settings.api_key.clone(),
            ctx,
        });
    }
    eng.set_state(&app, "loading", "");

    // wait until it answers (model load can take minutes for big files)
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}{health}");
    let deadline = Instant::now() + Duration::from_secs(900);
    loop {
        tokio::time::sleep(Duration::from_millis(700)).await;
        if *eng.generation.lk() != my_gen {
            return Err("load cancelled".into());
        }
        {
            let mut g = eng.running.lock().await;
            match g.as_mut() {
                Some(r) => {
                    if let Ok(Some(status)) = r.child.try_wait() {
                        *g = None;
                        drop(g);
                        let tail: Vec<String> = {
                            let l = eng.log.lk();
                            l.iter().rev().take(6).rev().cloned().collect()
                        };
                        let msg = format!("{use_engine} exited ({status}) while loading. {}", tail.join(" | "));
                        eng.set_state(&app, "error", &msg);
                        return Err(msg);
                    }
                }
                None => return Err("load cancelled".into()),
            }
        }
        if let Ok(r) = client.get(&url).timeout(Duration::from_secs(3)).send().await {
            if r.status().is_success() {
                break;
            }
        }
        if Instant::now() > deadline {
            eng.stop(None).await;
            let msg = "timed out waiting for the engine to load the model".to_string();
            eng.set_state(&app, "error", &msg);
            return Err(msg);
        }
    }
    eng.set_state(&app, "ready", "");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_and_args_helpers() {
        assert_eq!(norm_base(" https://api.openai.com/v1/ "), "https://api.openai.com");
        assert_eq!(norm_base("http://h:1234"), "http://h:1234");
        assert_eq!(norm_base("https://openrouter.ai/api/v1"), "https://openrouter.ai/api");
        assert_eq!(split_args(r#"--a 1 --b "x y" 'p q' """#), vec!["--a", "1", "--b", "x y", "p q", ""]);
        assert!(split_args("  ").is_empty());
    }

    #[test]
    fn patches_missing_gguf_paths() {
        let dir = std::env::temp_dir().join("harness_prof_test");
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("Real-Model-Q4.gguf");
        std::fs::write(&real, b"x").unwrap();
        let prof = dir.join("strata-test.json");
        std::fs::write(
            &prof,
            r#"{"args":["--model","Z:/gone/Real-Model-Q4.gguf","--keep","Z:/gone/other.gguf"],"vision":{"model":"Z:/gone/real-model-q4.GGUF"}}"#,
        )
        .unwrap();
        let entry = models::ModelEntry {
            id: real.to_string_lossy().into(), name: "r".into(), arch: "".into(), quant: "".into(), size_bytes: 1,
            ctx_train: 0, moe: false, parts: vec![real.to_string_lossy().into()], mmproj: None,
            engine: "polystrata".into(), auto_engine: "polystrata".into(), profile: None,
        };
        let out = patched_profile(prof.to_str().unwrap(), &[entry]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out).unwrap()).unwrap();
        assert_eq!(v["args"][1].as_str().unwrap(), real.to_str().unwrap());
        assert_eq!(v["args"][3], "Z:/gone/other.gguf"); // unknown file left alone
        assert_eq!(v["vision"]["model"].as_str().unwrap(), real.to_str().unwrap());
    }
}
