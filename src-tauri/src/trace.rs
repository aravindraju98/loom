//! Run traces: what happened in each agent run, turn by turn, for the Run inspector.
//!
//! traces\<run_id>.json   full event list of one run
//! traces\index.jsonl     one summary line per run (newest last); trimmed to the last 500 runs
use crate::store::data_dir;
use crate::util::{write_atomic, LockExt};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const KEEP_RUNS: usize = 500;
static INDEX_LOCK: Mutex<()> = Mutex::new(());

fn dir() -> PathBuf {
    data_dir().join("traces")
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

fn safe(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[derive(Default)]
struct Totals {
    turns: u64,
    tool_calls: u64,
    tool_errors: u64,
    prompt_tokens: u64,
    cached_tokens: u64,
    completion_tokens: u64,
    prompt_ms: f64,
}

pub struct Run {
    pub id: String,
    head: Mutex<Value>,
    started: Instant,
    started_ms: u128,
    events: Mutex<Vec<Value>>,
    totals: Mutex<Totals>,
}

impl Run {
    pub fn start(id: &str, head: Value) -> Run {
        Run {
            id: id.to_string(),
            head: Mutex::new(head),
            started: Instant::now(),
            started_ms: now_ms(),
            events: Mutex::new(vec![]),
            totals: Mutex::new(Totals::default()),
        }
    }

    /// Add fields to the run summary (model, engine… once known).
    pub fn set(&self, fields: Value) {
        let mut h = self.head.lk();
        if let (Some(h), Some(f)) = (h.as_object_mut(), fields.as_object()) {
            for (k, v) in f {
                h.insert(k.clone(), v.clone());
            }
        }
    }

    fn at(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// Record an event; `t` (ms since run start) is added.
    pub fn event(&self, kind: &str, mut data: Value) {
        data["kind"] = json!(kind);
        data["t"] = json!(self.at());
        let mut tot = self.totals.lk();
        match kind {
            "turn" => {
                tot.turns += 1;
                tot.prompt_tokens += data["prompt_tokens"].as_u64().unwrap_or(0);
                tot.cached_tokens += data["cache_n"].as_u64().unwrap_or(0);
                tot.completion_tokens += data["tokens"].as_u64().unwrap_or(0);
                tot.prompt_ms += data["prompt_ms"].as_f64().unwrap_or(0.0);
            }
            "tool" => {
                tot.tool_calls += 1;
                if data["ok"] == json!(false) {
                    tot.tool_errors += 1;
                }
            }
            _ => {}
        }
        drop(tot);
        self.events.lk().push(data);
    }

    pub fn finish(&self, reason: &str, error: Option<&str>) {
        let t = self.totals.lk();
        let mut summary = self.head.lk().clone();
        let extra = json!({
            "run_id": self.id, "started": self.started_ms as u64, "ms": self.at(), "reason": reason, "error": error,
            "turns": t.turns, "tool_calls": t.tool_calls, "tool_errors": t.tool_errors,
            "prompt_tokens": t.prompt_tokens, "cached_tokens": t.cached_tokens, "completion_tokens": t.completion_tokens,
            "prompt_ms": t.prompt_ms,
        });
        if let (Some(s), Some(x)) = (summary.as_object_mut(), extra.as_object()) {
            for (k, v) in x {
                s.insert(k.clone(), v.clone());
            }
        }
        drop(t);
        let full = json!({"summary": summary, "events": *self.events.lk()});
        let _ = save(&self.id, &full, &summary);
    }
}

fn save(id: &str, full: &Value, summary: &Value) -> Result<(), String> {
    if !safe(id) {
        return Err("bad run id".into());
    }
    let d = dir();
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    write_atomic(&d.join(format!("{id}.json")), full.to_string().as_bytes()).map_err(|e| e.to_string())?;
    let _g = INDEX_LOCK.lk();
    let idx = d.join("index.jsonl");
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&idx).map_err(|e| e.to_string())?;
    writeln!(f, "{summary}").map_err(|e| e.to_string())?;
    drop(f);
    // trim occasionally: keep the newest KEEP_RUNS
    let text = std::fs::read_to_string(&idx).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() > KEEP_RUNS + 50 {
        let (old, keep) = lines.split_at(lines.len() - KEEP_RUNS);
        for l in old {
            if let Some(rid) = serde_json::from_str::<Value>(l).ok().and_then(|v| v["run_id"].as_str().map(String::from)) {
                if safe(&rid) {
                    let _ = std::fs::remove_file(d.join(format!("{rid}.json")));
                }
            }
        }
        let _ = write_atomic(&idx, (keep.join("\n") + "\n").as_bytes());
    }
    Ok(())
}

/// Newest runs first.
#[tauri::command]
pub fn trace_list(limit: Option<usize>, chat_id: Option<String>) -> Vec<Value> {
    let text = std::fs::read_to_string(dir().join("index.jsonl")).unwrap_or_default();
    text.lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| chat_id.as_ref().map(|c| v["chat_id"].as_str() == Some(c)).unwrap_or(true))
        .take(limit.unwrap_or(100))
        .collect()
}

#[tauri::command]
pub fn trace_get(run_id: String) -> Result<Value, String> {
    if !safe(&run_id) {
        return Err("bad run id".into());
    }
    let t = std::fs::read_to_string(dir().join(format!("{run_id}.json"))).map_err(|_| "trace not found (it may have been cleaned up)".to_string())?;
    serde_json::from_str(&t).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_accumulate() {
        let r = Run::start("t1", json!({"model":"m"}));
        r.event("turn", json!({"prompt_tokens":100,"cache_n":80,"tokens":20,"prompt_ms":50.0}));
        r.event("turn", json!({"prompt_tokens":150,"cache_n":140,"tokens":10}));
        r.event("tool", json!({"name":"read_file","ok":true}));
        r.event("tool", json!({"name":"edit_file","ok":false}));
        let t = r.totals.lk();
        assert_eq!((t.turns, t.prompt_tokens, t.cached_tokens, t.completion_tokens), (2, 250, 220, 30));
        assert_eq!((t.tool_calls, t.tool_errors), (2, 1));
        assert_eq!(r.events.lk().len(), 4);
    }
}
