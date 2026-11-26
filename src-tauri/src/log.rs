//! Application log: one stream for the app, the agent, the model engine and MCP servers.
//!
//! - kept in memory (last few thousand entries) for the Logs panel
//! - appended to %APPDATA%\harness\logs\harness-YYYY-MM-DD.jsonl, kept for 14 days
//! - pushed to the UI in small batches (`log-batch` event)
//! - secrets (API keys, tokens) are masked before anything is stored or shown
use crate::store::data_dir;
use crate::util::LockExt;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const RING: usize = 4000;
const KEEP_DAYS: u64 = 14;

#[derive(Serialize, Clone, Debug)]
pub struct Entry {
    pub seq: u64,
    /// ms since epoch
    pub ts: u128,
    /// "debug" | "info" | "warn" | "error"
    pub level: &'static str,
    /// "app" | "agent" | "engine" | "mcp:<server>"
    pub source: String,
    pub msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

struct State {
    ring: VecDeque<Entry>,
    pending: Vec<Entry>,
    file: Option<(String, std::fs::File)>,
    seq: u64,
    secrets: Vec<String>,
}

static STATE: Mutex<State> = Mutex::new(State { ring: VecDeque::new(), pending: vec![], file: None, seq: 0, secrets: vec![] });
static APP: OnceLock<AppHandle> = OnceLock::new();

pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// Days since 1970-01-01 -> (y, m, d), proleptic Gregorian (Howard Hinnant's algorithm).
pub fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn date_of(ms: u128) -> String {
    let (y, m, d) = civil((ms / 86_400_000) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Mask secrets: configured keys verbatim, plus anything that looks like a bearer token or key=value.
pub fn redact(s: &str, secrets: &[String]) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"(?i)(bearer\s+|api[_-]?key["'\s:=]+|token["'\s:=]+|authorization["'\s:=]+)([A-Za-z0-9._\-]{8,})"#).unwrap());
    let mut out = s.to_string();
    for k in secrets {
        if k.len() >= 6 {
            out = out.replace(k.as_str(), "••••");
        }
    }
    re.replace_all(&out, "${1}••••").into_owned()
}

/// Tell the logger which values must never appear in logs (API keys, HF token…).
pub fn set_secrets(list: Vec<String>) {
    STATE.lk().secrets = list.into_iter().filter(|s| s.len() >= 6).collect();
}

pub fn init(app: AppHandle) {
    let _ = APP.set(app.clone());
    let s = crate::store::load_settings();
    let mut secrets = vec![s.api_key.clone(), s.hf_token.clone()];
    secrets.extend(s.providers.iter().map(|p| p.api_key.clone()));
    set_secrets(secrets);
    // drop old files
    if let Ok(rd) = std::fs::read_dir(logs_dir()) {
        let cutoff = SystemTime::now() - Duration::from_secs(KEEP_DAYS * 86_400);
        for e in rd.flatten() {
            if e.metadata().and_then(|m| m.modified()).map(|t| t < cutoff).unwrap_or(false) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    // flush batches to the UI a few times a second instead of one IPC message per line
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let batch = std::mem::take(&mut STATE.lk().pending);
            if !batch.is_empty() {
                let _ = app.emit("log-batch", batch);
            }
        }
    });
}

pub fn write(level: &'static str, source: &str, msg: &str, run_id: Option<&str>, data: Option<Value>) {
    let mut st = STATE.lk();
    st.seq += 1;
    let msg = redact(msg, &st.secrets);
    let data = data.map(|d| match serde_json::from_str(&redact(&d.to_string(), &st.secrets)) {
        Ok(v) => v,
        Err(_) => d,
    });
    let e = Entry { seq: st.seq, ts: now_ms(), level, source: source.to_string(), msg, run_id: run_id.map(String::from), data };
    // file (opened lazily, one per day)
    let day = date_of(e.ts);
    // unit tests must not write into the user's real log folder
    if !cfg!(test) && st.file.as_ref().map(|(d, _)| d != &day).unwrap_or(true) {
        let _ = std::fs::create_dir_all(logs_dir());
        st.file = std::fs::OpenOptions::new().create(true).append(true).open(logs_dir().join(format!("harness-{day}.jsonl"))).ok().map(|f| (day, f));
    }
    if let Some((_, f)) = st.file.as_mut() {
        if let Ok(line) = serde_json::to_string(&e) {
            let _ = writeln!(f, "{line}");
        }
    }
    if st.ring.len() >= RING {
        st.ring.pop_front();
    }
    st.ring.push_back(e.clone());
    if APP.get().is_some() && st.pending.len() < 2000 {
        st.pending.push(e);
    }
}

pub fn info(source: &str, msg: &str) {
    write("info", source, msg, None, None)
}
pub fn error(source: &str, msg: &str) {
    write("error", source, msg, None, None)
}

/// Guess a level for a line of engine/MCP output.
pub fn level_of(line: &str) -> &'static str {
    let l = line.to_ascii_lowercase();
    if l.contains("error") || l.contains("failed") || l.contains("panic") || l.contains("exception") || l.contains("traceback") {
        "error"
    } else if l.contains("warn") {
        "warn"
    } else {
        "info"
    }
}

#[tauri::command]
pub fn logs_tail(limit: Option<usize>, after: Option<u64>) -> Vec<Entry> {
    let st = STATE.lk();
    let after = after.unwrap_or(0);
    let v: Vec<Entry> = st.ring.iter().filter(|e| e.seq > after).cloned().collect();
    let n = limit.unwrap_or(1000).min(v.len());
    v[v.len() - n..].to_vec()
}

#[tauri::command]
pub fn logs_clear() {
    STATE.lk().ring.clear();
}

/// Write the in-memory log to `dest` as JSON lines.
#[tauri::command]
pub fn logs_export(dest: String) -> Result<usize, String> {
    let st = STATE.lk();
    let mut out = String::new();
    for e in &st.ring {
        out.push_str(&serde_json::to_string(e).map_err(|e| e.to_string())?);
        out.push('\n');
    }
    std::fs::write(&dest, out).map_err(|e| format!("could not write {dest}: {e}"))?;
    Ok(st.ring.len())
}

#[tauri::command]
pub fn logs_open_dir() -> Result<(), String> {
    let d = logs_dir();
    let _ = std::fs::create_dir_all(&d);
    std::process::Command::new("explorer").arg(&d).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_redaction() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(date_of(1_732_579_200_000), "2024-11-26");
        assert_eq!(civil(19_782), (2024, 2, 29));
        let s = redact("auth: Bearer sk-abc123456789 and key=mysecretvalue api_key=\"zzzzzzzzzz\"", &["mysecretvalue".into()]);
        assert!(!s.contains("sk-abc123456789") && !s.contains("mysecretvalue") && !s.contains("zzzzzzzzzz"), "{s}");
        assert_eq!(level_of("llama_model_load: error loading"), "error");
        assert_eq!(level_of("srv  update_slots: all slots are idle"), "info");
    }

    #[test]
    fn ring_and_tail() {
        write("info", "app", "hello", None, None);
        write("warn", "agent", "careful", Some("r1"), Some(serde_json::json!({"k":1})));
        let t = logs_tail(Some(10), None);
        assert!(t.iter().any(|e| e.msg == "careful" && e.run_id.as_deref() == Some("r1")));
        let last = t.last().unwrap().seq;
        assert!(logs_tail(None, Some(last)).is_empty());
    }
}
