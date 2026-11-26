//! The chat/agent loop: stream a completion, run requested tools (with approval), repeat.
use crate::{engine::Engine, mcp::Mcp, tools};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{oneshot, Notify};

#[derive(Default)]
pub struct Agent {
    runs: Mutex<HashMap<String, Arc<RunCtl>>>,
    perms: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    always: Mutex<HashSet<(String, String)>>,
}

#[derive(Default)]
struct RunCtl {
    cancelled: AtomicBool,
    notify: Notify,
}

#[derive(Deserialize, Clone)]
pub struct RunArgs {
    pub run_id: String,
    pub chat_id: String,
    /// OpenAI-format history (user/assistant/tool messages), without the system prompt
    pub messages: Vec<Value>,
    pub workspace: String,
    pub system_prompt: String,
    pub temperature: f32,
    pub max_turns: u32,
    pub auto_approve: bool,
    pub use_tools: bool,
    /// extra top-level request fields (e.g. chat_template_kwargs)
    pub extra: Option<Value>,
}

#[derive(Default, Debug)]
pub struct Assembler {
    pub content: String,
    pub reasoning: String,
    pub calls: BTreeMap<usize, (String, String, String)>, // index -> (id, name, args)
    pub finish: Option<String>,
    pub completion_tokens: Option<u64>,
    pub server_tok_s: Option<f64>,
    pub chunks: u64,
}

pub enum Delta {
    Content(String),
    Reasoning(String),
    /// a tool call is being generated: (tool name, argument characters so far)
    ToolArgs(String, usize),
}

impl Assembler {
    /// Feed one parsed SSE chunk; returns text deltas to show.
    pub fn feed(&mut self, v: &Value) -> Vec<Delta> {
        let mut out = vec![];
        if let Some(u) = v.get("usage").and_then(|u| u.get("completion_tokens")).and_then(|x| x.as_u64()) {
            self.completion_tokens = Some(u);
        }
        if let Some(t) = v.get("timings").and_then(|t| t.get("predicted_per_second")).and_then(|x| x.as_f64()) {
            self.server_tok_s = Some(t);
        }
        let Some(ch) = v["choices"].get(0) else { return out };
        if let Some(f) = ch["finish_reason"].as_str() {
            self.finish = Some(f.to_string());
        }
        let d = &ch["delta"];
        if let Some(t) = d["content"].as_str() {
            if !t.is_empty() {
                self.content.push_str(t);
                self.chunks += 1;
                out.push(Delta::Content(t.to_string()));
            }
        }
        let r = d["reasoning_content"].as_str().or_else(|| d["reasoning"].as_str());
        if let Some(t) = r {
            if !t.is_empty() {
                self.reasoning.push_str(t);
                self.chunks += 1;
                out.push(Delta::Reasoning(t.to_string()));
            }
        }
        if let Some(tc) = d["tool_calls"].as_array() {
            for c in tc {
                let idx = c["index"].as_u64().unwrap_or(0) as usize;
                let e = self.calls.entry(idx).or_default();
                if let Some(id) = c["id"].as_str() {
                    if !id.is_empty() {
                        e.0 = id.to_string();
                    }
                }
                if let Some(n) = c["function"]["name"].as_str() {
                    e.1.push_str(n);
                }
                if let Some(a) = c["function"]["arguments"].as_str() {
                    e.2.push_str(a);
                }
                out.push(Delta::ToolArgs(e.1.clone(), e.2.chars().count()));
            }
        }
        out
    }
}

fn ev(app: &AppHandle, run_id: &str, ty: &str, mut payload: Value) {
    payload["run_id"] = json!(run_id);
    payload["type"] = json!(ty);
    let _ = app.emit("agent-event", payload);
}

fn strip_for_send(m: &Value) -> Value {
    let mut m = m.clone();
    if let Some(o) = m.as_object_mut() {
        o.remove("reasoning_content");
        o.remove("ui");
        if o.get("role").and_then(|r| r.as_str()) == Some("assistant") && o.get("content").map(|c| c.is_null()).unwrap_or(false) {
            o.insert("content".into(), json!(""));
        }
    }
    m
}

async fn stream_turn(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    body: &Value,
    ctl: &RunCtl,
    app: &AppHandle,
    run_id: &str,
) -> Result<Assembler, String> {
    let mut rq = client.post(format!("{base}/v1/chat/completions")).json(body);
    if !key.is_empty() {
        rq = rq.bearer_auth(key);
    }
    let resp = rq.send().await.map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        let st = resp.status();
        let t = resp.text().await.unwrap_or_default();
        return Err(format!("engine returned {st}: {}", t.chars().take(400).collect::<String>()));
    }
    let mut asm = Assembler::default();
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = vec![];
    loop {
        let next = tokio::select! {
            n = stream.next() => n,
            _ = ctl.notify.notified() => None,
        };
        if ctl.cancelled.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|e| format!("stream error: {e}"))?;
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                return Ok(asm);
            }
            if let Ok(v) = serde_json::from_str::<Value>(data) {
                for d in asm.feed(&v) {
                    match d {
                        Delta::Content(t) => ev(app, run_id, "delta", json!({"kind":"content","text":t})),
                        Delta::Reasoning(t) => ev(app, run_id, "delta", json!({"kind":"reasoning","text":t})),
                        Delta::ToolArgs(name, chars) => {
                            // a long write_file can take minutes to generate; keep the UI informed (throttled)
                            if last_progress.elapsed() >= Duration::from_millis(400) {
                                last_progress = Instant::now();
                                ev(app, run_id, "tool_progress", json!({"name":name,"chars":chars}));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(asm)
}

async fn await_permission(
    app: &AppHandle,
    agent: &Agent,
    ctl: &RunCtl,
    run_id: &str,
    call_id: &str,
    name: &str,
    args: &Value,
) -> bool {
    let (tx, rx) = oneshot::channel();
    agent.perms.lock().unwrap().insert(call_id.to_string(), tx);
    ev(app, run_id, "permission", json!({"call_id":call_id,"name":name,"args":args}));
    let allowed = tokio::select! {
        r = rx => r.unwrap_or(false),
        _ = ctl.notify.notified() => false,
    };
    agent.perms.lock().unwrap().remove(call_id);
    allowed && !ctl.cancelled.load(Ordering::SeqCst)
}

async fn run_loop(app: AppHandle, a: RunArgs, ctl: Arc<RunCtl>) -> Result<&'static str, String> {
    let eng = app.state::<Engine>();
    let mcp = app.state::<Arc<Mcp>>();
    let agent = app.state::<Agent>();
    let ep = eng.endpoint().await.ok_or("no model loaded: load one from the Models tab")?;
    let (base, model, key) = (ep.base, ep.model, ep.key);
    let client = reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())?;

    let ws = if a.workspace.is_empty() { None } else { Some(PathBuf::from(&a.workspace)) };
    let mut tool_defs: Vec<Value> = vec![];
    let mut mcp_ro: HashSet<String> = HashSet::new();
    if a.use_tools {
        if ws.is_some() {
            tool_defs.extend(tools::schemas());
        }
        for (d, ro) in mcp.tool_defs() {
            if ro {
                if let Some(n) = d["function"]["name"].as_str() {
                    mcp_ro.insert(n.to_string());
                }
            }
            tool_defs.push(d);
        }
    }

    let mut msgs: Vec<Value> = a.messages.clone();
    for turn in 0..a.max_turns.max(1) {
        let mut sys = a.system_prompt.clone();
        if let Some(w) = &ws {
            sys.push_str(&format!(
                "\n\nWorkspace folder: {}\nShell: PowerShell on Windows.\nLarge files: create them in several steps (write_file for the first part, then write_file with append=true for each next part, about 150 lines per call). One giant tool call can take many minutes to generate.",
                w.display()
            ));
        }
        let mut send: Vec<Value> = vec![json!({"role":"system","content":sys})];
        send.extend(msgs.iter().map(strip_for_send));
        let mut body = json!({
            "model": model, "messages": send, "stream": true,
            "stream_options": {"include_usage": true}, "temperature": a.temperature,
        });
        if !tool_defs.is_empty() {
            body["tools"] = json!(tool_defs);
            body["tool_choice"] = json!("auto");
        }
        if let Some(Value::Object(x)) = &a.extra {
            for (k, v) in x {
                body[k] = v.clone();
            }
        }

        let started = Instant::now();
        let asm = stream_turn(&client, &base, &key, &body, &ctl, &app, &a.run_id).await;
        let asm = match asm {
            Ok(x) => x,
            Err(e) if e == "cancelled" => return Ok("cancelled"),
            Err(e) => return Err(e),
        };
        let secs = started.elapsed().as_secs_f64();
        let toks = asm.completion_tokens.unwrap_or(asm.chunks);
        let tok_s = asm.server_tok_s.unwrap_or(if secs > 0.0 { toks as f64 / secs } else { 0.0 });
        ev(&app, &a.run_id, "usage", json!({"tokens":toks,"secs":secs,"tok_s":tok_s}));

        let calls: Vec<(String, String, String)> = asm
            .calls
            .values()
            .map(|(id, n, args)| {
                let id = if id.is_empty() { format!("call_{}", uuid::Uuid::new_v4().simple()) } else { id.clone() };
                (id, n.clone(), args.clone())
            })
            .collect();
        let mut am = json!({"role":"assistant","content":asm.content});
        if !asm.reasoning.is_empty() {
            am["reasoning_content"] = json!(asm.reasoning);
        }
        if !calls.is_empty() {
            am["tool_calls"] = json!(calls
                .iter()
                .map(|(id, n, args)| json!({"id":id,"type":"function","function":{"name":n,"arguments":args}}))
                .collect::<Vec<_>>());
        }
        msgs.push(am.clone());
        ev(&app, &a.run_id, "message", json!({"message":am}));
        if calls.is_empty() {
            return Ok("done");
        }

        for (id, name, args_s) in calls {
            if ctl.cancelled.load(Ordering::SeqCst) {
                return Ok("cancelled");
            }
            let args: Value = if args_s.trim().is_empty() { json!({}) } else { serde_json::from_str(&args_s).unwrap_or(Value::Null) };
            ev(&app, &a.run_id, "tool_start", json!({"call_id":id,"name":name,"args":args}));
            let (ok, output) = if args.is_null() {
                (false, format!("Invalid JSON arguments: {args_s}"))
            } else {
                let builtin = tools::is_builtin(&name);
                let risky = if builtin { tools::needs_approval(&name) } else { !mcp_ro.contains(&name) };
                let pre_ok = !risky
                    || a.auto_approve
                    || agent.always.lock().unwrap().contains(&(a.chat_id.clone(), name.clone()));
                let allowed = pre_ok || await_permission(&app, &agent, &ctl, &a.run_id, &id, &name, &args).await;
                if ctl.cancelled.load(Ordering::SeqCst) {
                    return Ok("cancelled");
                }
                if !allowed {
                    (false, "The user denied this tool call.".to_string())
                } else if builtin {
                    match &ws {
                        Some(w) => match tools::run(&name, &args, w).await {
                            Ok(o) => (true, o),
                            Err(e) => (false, e),
                        },
                        None => (false, "no workspace folder is selected".to_string()),
                    }
                } else {
                    match mcp.call(&name, args.clone()).await {
                        Ok(o) => (true, o),
                        Err(e) => (false, e),
                    }
                }
            };
            ev(&app, &a.run_id, "tool_result", json!({"call_id":id,"ok":ok,"output":output}));
            let tm = json!({"role":"tool","tool_call_id":id,"name":name,"content":output,"ui":{"ok":ok}});
            msgs.push(tm.clone());
            ev(&app, &a.run_id, "message", json!({"message":tm}));
        }
        let _ = turn;
    }
    Ok("max_turns")
}

#[tauri::command]
pub async fn agent_run(app: AppHandle, agent: State<'_, Agent>, args: RunArgs) -> Result<(), String> {
    let ctl = Arc::new(RunCtl::default());
    agent.runs.lock().unwrap().insert(args.run_id.clone(), ctl.clone());
    let run_id = args.run_id.clone();
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let r = run_loop(app2.clone(), args, ctl).await;
        match r {
            Ok(reason) => ev(&app2, &run_id, "done", json!({"reason":reason})),
            Err(e) => ev(&app2, &run_id, "error", json!({"message":e})),
        }
        app2.state::<Agent>().runs.lock().unwrap().remove(&run_id);
    });
    Ok(())
}

#[tauri::command]
pub fn agent_cancel(agent: State<'_, Agent>, run_id: String) {
    if let Some(c) = agent.runs.lock().unwrap().get(&run_id) {
        c.cancelled.store(true, Ordering::SeqCst);
        c.notify.notify_waiters();
        // wake a waiter that registered after notify_waiters
        c.notify.notify_one();
    }
}

#[tauri::command]
pub fn permission_reply(agent: State<'_, Agent>, call_id: String, chat_id: String, tool: String, allow: bool, always: bool) {
    if allow && always {
        agent.always.lock().unwrap().insert((chat_id, tool));
    }
    if let Some(tx) = agent.perms.lock().unwrap().remove(&call_id) {
        let _ = tx.send(allow);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_streamed_tool_calls() {
        let mut a = Assembler::default();
        let chunks = [
            json!({"choices":[{"delta":{"reasoning_content":"hm "}}]}),
            json!({"choices":[{"delta":{"content":"Let me look."}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_","arguments":"{\"pa"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"file","arguments":"th\":\"a.txt\"}"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":1,"id":"c2","function":{"name":"list_dir","arguments":"{}"}}]}}]}),
            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"completion_tokens":42}}),
        ];
        let (mut shown, mut progress) = (0, vec![]);
        for c in &chunks {
            for d in a.feed(c) {
                match d {
                    Delta::ToolArgs(n, chars) => progress.push((n, chars)),
                    _ => shown += 1,
                }
            }
        }
        assert_eq!(shown, 2); // text deltas only
        assert_eq!(progress.last().unwrap().0, "list_dir"); // tool-generation progress is reported separately
        assert!(progress.iter().any(|(n, c)| n == "read_file" && *c == 16));
        assert_eq!(a.reasoning, "hm ");
        assert_eq!(a.content, "Let me look.");
        assert_eq!(a.calls[&0], ("c1".to_string(), "read_file".to_string(), "{\"path\":\"a.txt\"}".to_string()));
        assert_eq!(a.calls[&1].1, "list_dir");
        assert_eq!(a.finish.as_deref(), Some("tool_calls"));
        assert_eq!(a.completion_tokens, Some(42));
    }
}
