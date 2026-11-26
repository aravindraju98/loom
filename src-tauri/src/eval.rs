//! Local-model evals: small, realistic agent tasks run against the loaded model, checked by
//! looking at the files afterwards. They show how well a model drives these tools (edit success,
//! whole-file rewrites, tool-call parse recoveries, turns, time) so settings can be chosen on data.
use crate::agent::{run_headless, RunArgs};
use crate::store::data_dir;
use crate::util::LockExt;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

type Check = fn(&Path, &str) -> Result<(), String>;

pub struct Scenario {
    pub id: &'static str,
    pub name: &'static str,
    pub prompt: &'static str,
    files: fn() -> Vec<(&'static str, String)>,
    check: Check,
}

fn read(ws: &Path, p: &str) -> Result<String, String> {
    std::fs::read_to_string(ws.join(p)).map(|s| s.replace("\r\n", "\n")).map_err(|_| format!("{p} is missing"))
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn big_js() -> String {
    let mut s = String::from("// utility helpers\n'use strict';\n\n");
    for i in 1..=60 {
        s.push_str(&format!("export function helper{i}(x) {{\n  // step {i}\n  return x * {i} + {};\n}}\n\n", i % 7));
    }
    s
}

fn big_config() -> String {
    let mut s = String::from("// generated settings\n");
    for i in 1..=400 {
        if i == 263 {
            s.push_str("export const MAX_RETRIES = 7;\n");
        } else {
            s.push_str(&format!("export const SETTING_{i} = {};\n", (i * 37) % 101));
        }
    }
    s
}

pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            id: "fix_bug",
            name: "Fix a one-line bug",
            prompt: "There is a bug in add() in calc.py: it subtracts instead of adding. Fix it without changing anything else.",
            files: || {
                vec![("calc.py", "def add(a, b):\n    \"\"\"Return the sum of a and b.\"\"\"\n    return a - b\n\n\ndef mul(a, b):\n    return a * b\n".into())]
            },
            check: |ws, _| {
                let t = read(ws, "calc.py")?;
                if !squash(&t).contains("return a + b") {
                    return Err("add() still doesn't return a + b".into());
                }
                if !t.contains("def mul(a, b):\n    return a * b") {
                    return Err("mul() was changed".into());
                }
                Ok(())
            },
        },
        Scenario {
            id: "rename",
            name: "Rename across two files",
            prompt: "Rename the function getUserName to fetchUserName everywhere in the src folder: the definition and every use.",
            files: || {
                vec![
                    (
                        "src/user.ts",
                        "export interface User { name: string; age: number }\n\nexport function getUserName(u: User): string {\n  return u.name;\n}\n\nexport function isAdult(u: User): boolean {\n  return u.age >= 18;\n}\n".into(),
                    ),
                    (
                        "src/app.ts",
                        "import { getUserName, isAdult } from \"./user\";\n\nconst a = { name: \"Ann\", age: 30 };\nconsole.log(getUserName(a));\nif (isAdult(a)) console.log(\"adult\", getUserName(a));\n".into(),
                    ),
                ]
            },
            check: |ws, _| {
                let u = read(ws, "src/user.ts")?;
                let a = read(ws, "src/app.ts")?;
                if u.contains("getUserName") || a.contains("getUserName") {
                    return Err("getUserName is still used".into());
                }
                if !u.contains("function fetchUserName") || a.matches("fetchUserName").count() < 3 {
                    return Err("fetchUserName is not defined and used everywhere".into());
                }
                if !u.contains("export function isAdult") {
                    return Err("unrelated code changed".into());
                }
                Ok(())
            },
        },
        Scenario {
            id: "insert_large",
            name: "Add a function to a 300-line file",
            prompt:
                "In utils.js, add a new exported function clamp(x, lo, hi) that returns x limited to the range lo..hi. Don't change anything else in the file.",
            files: || vec![("utils.js", big_js())],
            check: |ws, _| {
                let t = read(ws, "utils.js")?;
                if !t.contains("clamp") {
                    return Err("no clamp function".into());
                }
                let orig = big_js();
                let mut it = t.lines();
                for l in orig.lines().filter(|l| !l.trim().is_empty()) {
                    if !it.any(|x| x == l) {
                        return Err(format!("original line changed or lost: {l}"));
                    }
                }
                let grew = t.lines().count() as i64 - orig.lines().count() as i64;
                if grew > 25 {
                    return Err(format!("file grew by {grew} lines"));
                }
                Ok(())
            },
        },
        Scenario {
            id: "json_multi",
            name: "Change two settings in JSON",
            prompt: "In settings.json set port to 8080 and debug to false. Leave the other settings as they are.",
            files: || {
                vec![(
                    "settings.json",
                    "{\n  \"name\": \"demo\",\n  \"port\": 3000,\n  \"debug\": true,\n  \"workers\": 4,\n  \"tags\": [\"a\", \"b\"]\n}\n".into(),
                )]
            },
            check: |ws, _| {
                let v: Value = serde_json::from_str(&read(ws, "settings.json")?).map_err(|e| format!("settings.json is not valid JSON: {e}"))?;
                if v["port"] != json!(8080) || v["debug"] != json!(false) {
                    return Err(format!("port={} debug={}", v["port"], v["debug"]));
                }
                if v["name"] != json!("demo") || v["workers"] != json!(4) || v["tags"] != json!(["a", "b"]) {
                    return Err("other settings changed".into());
                }
                Ok(())
            },
        },
        Scenario {
            id: "find_value",
            name: "Find a value in a large file",
            prompt: "What is the value of MAX_RETRIES in config.ts? Answer with just the number.",
            files: || vec![("config.ts", big_config())],
            check: |_, answer| {
                let ok = answer.split(|c: char| !c.is_ascii_digit()).any(|n| n == "7");
                if ok {
                    Ok(())
                } else {
                    Err(format!("answer was: {}", answer.chars().take(80).collect::<String>()))
                }
            },
        },
        Scenario {
            id: "create_file",
            name: "Create a new file",
            prompt: "Create the file notes/todo.md whose entire content is this one line: - [ ] ship it",
            files: Vec::new,
            check: |ws, _| {
                let t = read(ws, "notes/todo.md")?;
                if t.trim() == "- [ ] ship it" {
                    Ok(())
                } else {
                    Err(format!("content was: {}", t.trim().chars().take(80).collect::<String>()))
                }
            },
        },
    ]
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct EvalResult {
    pub scenario: String,
    pub name: String,
    pub passed: bool,
    pub detail: String,
    pub ms: u64,
    pub end: String,
    pub turns: u64,
    pub tool_calls: u64,
    pub tool_errors: u64,
    /// edit_file / edit_lines calls that failed (no match, ambiguous, stale)
    pub edit_failures: u64,
    /// whole existing files replaced with write_file(overwrite)
    pub rewrites: u64,
    /// tool calls recovered from text instead of the tool_calls field
    pub recovered: u64,
    pub completion_tokens: u64,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn metrics(run_id: &str, r: &mut EvalResult) {
    let Ok(t) = crate::trace::trace_get(run_id.to_string()) else { return };
    let s = &t["summary"];
    r.turns = s["turns"].as_u64().unwrap_or(0);
    r.tool_calls = s["tool_calls"].as_u64().unwrap_or(0);
    r.tool_errors = s["tool_errors"].as_u64().unwrap_or(0);
    r.completion_tokens = s["completion_tokens"].as_u64().unwrap_or(0);
    for e in t["events"].as_array().into_iter().flatten() {
        match e["kind"].as_str() {
            Some("tool") => {
                let name = e["name"].as_str().unwrap_or("");
                if matches!(name, "edit_file" | "edit_lines") && e["ok"] == json!(false) {
                    r.edit_failures += 1;
                }
                if e["change"]["kind"] == json!("overwrite") {
                    r.rewrites += 1;
                }
            }
            Some("recovered_text_calls") => r.recovered += e["count"].as_u64().unwrap_or(1),
            _ => {}
        }
    }
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
static CURRENT: Mutex<Option<String>> = Mutex::new(None);

fn results_path() -> PathBuf {
    data_dir().join("evals").join("results.jsonl")
}

fn write_files(ws: &Path, files: &[(&str, String)]) -> Result<(), String> {
    for (p, content) in files {
        let f = ws.join(p);
        if let Some(d) = f.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&f, content).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn eval_list() -> Vec<Value> {
    scenarios().iter().map(|s| json!({"id": s.id, "name": s.name, "prompt": s.prompt})).collect()
}

/// Run the chosen scenarios (all if empty) one after another against the loaded model.
/// Progress arrives as `eval-progress` events; the finished batch is appended to results.jsonl.
#[tauri::command]
pub async fn eval_run(app: AppHandle, ids: Option<Vec<String>>) -> Result<Value, String> {
    let ep = app.state::<crate::engine::Engine>().endpoint().await.ok_or("load a model first")?;
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err("an eval is already running".into());
    }
    CANCEL.store(false, Ordering::SeqCst);
    let settings = crate::store::load_settings();
    let chosen: Vec<Scenario> = scenarios().into_iter().filter(|s| ids.as_ref().map(|v| v.is_empty() || v.iter().any(|i| i == s.id)).unwrap_or(true)).collect();
    let batch_id = format!("eval_{}", uuid::Uuid::new_v4().simple());
    let started = now_ms();
    let total = chosen.len();
    crate::log::info("eval", &format!("eval started: {total} scenarios on {}", ep.model));
    let mut results = vec![];
    for (i, sc) in chosen.iter().enumerate() {
        if CANCEL.load(Ordering::SeqCst) {
            break;
        }
        let _ = app.emit("eval-progress", json!({"batch": batch_id, "index": i, "total": total, "scenario": sc.id, "status": "running"}));
        let ws = data_dir().join("evals").join(format!("ws_{}_{}", sc.id, uuid::Uuid::new_v4().simple()));
        let mut r = EvalResult { scenario: sc.id.into(), name: sc.name.into(), ..Default::default() };
        let t0 = Instant::now();
        if let Err(e) = std::fs::create_dir_all(&ws).map_err(|e| e.to_string()).and_then(|_| write_files(&ws, &(sc.files)())) {
            r.detail = format!("setup failed: {e}");
        } else {
            let run_id = format!("{batch_id}_{}", sc.id);
            *CURRENT.lk() = Some(run_id.clone());
            let args = RunArgs {
                run_id: run_id.clone(),
                chat_id: batch_id.clone(),
                messages: vec![json!({"role":"user","content": sc.prompt})],
                workspace: ws.to_string_lossy().into_owned(),
                system_prompt: settings.system_prompt.clone(),
                temperature: 0.2,
                max_turns: 15,
                auto_approve: true,
                use_tools: true,
                extra: None,
                checkpoint_id: None,
            };
            let (end, transcript) = run_headless(&app, args).await;
            *CURRENT.lk() = None;
            r.ms = t0.elapsed().as_millis() as u64;
            let answer = transcript.iter().rev().find(|m| m["role"] == "assistant" && m["content"].as_str().map(|c| !c.trim().is_empty()).unwrap_or(false));
            let answer = answer.and_then(|m| m["content"].as_str()).unwrap_or("");
            match end {
                Ok(reason) => {
                    r.end = reason.into();
                    match (sc.check)(&ws, answer) {
                        Ok(()) => r.passed = true,
                        Err(e) => r.detail = e,
                    }
                }
                Err(e) => {
                    r.end = "error".into();
                    r.detail = e;
                }
            }
            metrics(&run_id, &mut r);
        }
        let _ = std::fs::remove_dir_all(&ws);
        let _ = app.emit("eval-progress", json!({"batch": batch_id, "index": i, "total": total, "scenario": sc.id, "status": "done", "result": r}));
        results.push(r);
    }
    let passed = results.iter().filter(|r| r.passed).count();
    let batch = json!({
        "id": batch_id, "started": started, "ms": now_ms() - started, "model": ep.model, "engine": ep.engine, "ctx": ep.ctx,
        "passed": passed, "total": results.len(), "cancelled": CANCEL.load(Ordering::SeqCst), "results": results,
    });
    let p = results_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        use std::io::Write;
        let _ = writeln!(f, "{batch}");
    }
    crate::log::info("eval", &format!("eval finished: {passed}/{} passed on {}", results.len(), ep.model));
    RUNNING.store(false, Ordering::SeqCst);
    Ok(batch)
}

#[tauri::command]
pub fn eval_cancel(app: AppHandle) {
    CANCEL.store(true, Ordering::SeqCst);
    if let Some(id) = CURRENT.lk().clone() {
        crate::agent::agent_cancel(app.state::<crate::agent::Agent>(), id);
    }
}

/// Past eval batches, newest first.
#[tauri::command]
pub fn eval_results(limit: Option<usize>) -> Vec<Value> {
    let text = std::fs::read_to_string(results_path()).unwrap_or_default();
    text.lines().rev().filter_map(|l| serde_json::from_str(l).ok()).take(limit.unwrap_or(30)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("harness_eval_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Each check fails on the untouched fixture and passes on a correct solution.
    #[test]
    fn checks_reject_fixtures_and_accept_solutions() {
        type Solve = (&'static str, fn(&Path));
        let solve: &[Solve] = &[
            ("fix_bug", |ws| {
                let t = read(ws, "calc.py").unwrap().replace("a - b", "a + b");
                std::fs::write(ws.join("calc.py"), t).unwrap();
            }),
            ("rename", |ws| {
                for f in ["src/user.ts", "src/app.ts"] {
                    let t = read(ws, f).unwrap().replace("getUserName", "fetchUserName");
                    std::fs::write(ws.join(f), t).unwrap();
                }
            }),
            ("insert_large", |ws| {
                let t = read(ws, "utils.js").unwrap() + "export function clamp(x, lo, hi) {\n  return Math.min(hi, Math.max(lo, x));\n}\n";
                std::fs::write(ws.join("utils.js"), t).unwrap();
            }),
            ("json_multi", |ws| {
                let t = read(ws, "settings.json").unwrap().replace("3000", "8080").replace("true", "false");
                std::fs::write(ws.join("settings.json"), t).unwrap();
            }),
            ("create_file", |ws| {
                std::fs::create_dir_all(ws.join("notes")).unwrap();
                std::fs::write(ws.join("notes/todo.md"), "- [ ] ship it\n").unwrap();
            }),
        ];
        for sc in scenarios() {
            let ws = tmp(sc.id);
            write_files(&ws, &(sc.files)()).unwrap();
            if sc.id == "find_value" {
                assert!((sc.check)(&ws, "It is 17").is_err());
                assert!((sc.check)(&ws, "7").is_ok());
                // deep in the file: past a small model's default read window
                assert!(big_config().lines().nth(263).unwrap().contains("MAX_RETRIES = 7"));
                continue;
            }
            assert!((sc.check)(&ws, "").is_err(), "{} passes without any change", sc.id);
            let (_, f) = solve.iter().find(|(id, _)| *id == sc.id).unwrap();
            f(&ws);
            assert!((sc.check)(&ws, "").is_ok(), "{} solution rejected: {:?}", sc.id, (sc.check)(&ws, ""));
        }
    }

    #[test]
    fn rewriting_a_line_in_the_big_file_is_caught() {
        let ws = tmp("insert_bad");
        let t = big_js().replace("return x * 3 + 3;", "return x * 3;") + "export function clamp(x, lo, hi) { return x; }\n";
        std::fs::write(ws.join("utils.js"), t).unwrap();
        let sc = scenarios().into_iter().find(|s| s.id == "insert_large").unwrap();
        assert!((sc.check)(&ws, "").unwrap_err().contains("changed or lost"));
    }
}
