//! The chat/agent loop: stream a completion, run requested tools (with approval), repeat.
use crate::util::LockExt;
use crate::{checkpoint, context, engine::Engine, mcp::Mcp, policy, toolparse, tools, trace};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Map, Value};
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

impl RunCtl {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
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
    /// restore point for file changes made while answering this message
    #[serde(default)]
    pub checkpoint_id: Option<String>,
}

#[derive(Default, Debug)]
pub struct Assembler {
    pub content: String,
    pub reasoning: String,
    pub calls: BTreeMap<usize, (String, String, String)>, // index -> (id, name, args)
    pub finish: Option<String>,
    pub completion_tokens: Option<u64>,
    pub prompt_tokens: Option<u64>,
    pub server_tok_s: Option<f64>,
    /// llama.cpp `timings`: prompt tokens evaluated this turn, ms spent on them, tokens reused from the KV cache
    pub prompt_n: Option<u64>,
    pub prompt_ms: Option<f64>,
    pub cache_n: Option<u64>,
    pub chunks: u64,
    /// time from sending the request to the first streamed token
    pub ttft_ms: Option<u64>,
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
        if let Some(u) = v.get("usage") {
            if let Some(x) = u.get("completion_tokens").and_then(|x| x.as_u64()) {
                self.completion_tokens = Some(x);
            }
            if let Some(x) = u.get("prompt_tokens").and_then(|x| x.as_u64()) {
                self.prompt_tokens = Some(x);
            }
        }
        if let Some(t) = v.get("timings") {
            if let Some(x) = t.get("predicted_per_second").and_then(|x| x.as_f64()) {
                self.server_tok_s = Some(x);
            }
            if let Some(x) = t.get("prompt_n").and_then(|x| x.as_u64()) {
                self.prompt_n = Some(x);
            }
            if let Some(x) = t.get("prompt_ms").and_then(|x| x.as_f64()) {
                self.prompt_ms = Some(x);
            }
            if let Some(x) = t.get("cache_n").and_then(|x| x.as_u64()) {
                self.cache_n = Some(x);
            }
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
    let elided = context::is_elided(m).then(|| context::stub(m));
    let mut m = m.clone();
    if let Some(s) = elided {
        m["content"] = json!(s);
    }
    if let Some(o) = m.as_object_mut() {
        o.remove("reasoning_content");
        o.remove("ui");
        if o.get("role").and_then(|r| r.as_str()) == Some("assistant") && o.get("content").map(|c| c.is_null()).unwrap_or(false) {
            o.insert("content".into(), json!(""));
        }
    }
    m
}

pub const CANCELLED_TOOL: &str = "Cancelled by the user before this tool ran.";

/// Make history valid for strict OpenAI-style servers: every assistant `tool_calls` entry must be
/// followed by one `tool` message per call id, and no `tool` message may lack its call.
/// Old chats saved after a mid-tool cancel are repaired here instead of failing with a 400.
pub fn repair_history(msgs: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(msgs.len());
    let mut i = 0;
    while i < msgs.len() {
        let m = &msgs[i];
        let role = m["role"].as_str().unwrap_or("");
        if role == "tool" {
            // orphan: its assistant call was handled (or never existed)
            i += 1;
            continue;
        }
        out.push(m.clone());
        i += 1;
        if role != "assistant" {
            continue;
        }
        let ids: Vec<(String, String)> = m["tool_calls"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| (c["id"].as_str().unwrap_or("").to_string(), c["function"]["name"].as_str().unwrap_or("").to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if ids.is_empty() {
            continue;
        }
        let mut answered: HashMap<String, Value> = HashMap::new();
        while i < msgs.len() && msgs[i]["role"].as_str() == Some("tool") {
            if let Some(id) = msgs[i]["tool_call_id"].as_str() {
                answered.entry(id.to_string()).or_insert_with(|| msgs[i].clone());
            }
            i += 1;
        }
        for (id, name) in ids {
            match answered.remove(&id) {
                Some(t) => out.push(t),
                None => out.push(json!({"role":"tool","tool_call_id":id,"name":name,"content":CANCELLED_TOOL,"ui":{"ok":false,"cancelled":true}})),
            }
        }
    }
    out
}

/// What went wrong talking to the engine, classified so the loop can react.
#[derive(Debug, PartialEq)]
enum EngineErr {
    /// worth retrying: connection refused/reset, 429, 502/503/504
    Transient(String),
    /// the prompt no longer fits the model's context window
    ContextOverflow(String),
    Fatal(String),
}

fn classify_status(status: u16, body: &str) -> EngineErr {
    let low = body.to_ascii_lowercase();
    let short: String = body.chars().take(400).collect();
    let msg = format!("engine returned {status}: {short}");
    let overflow = (low.contains("context") && (low.contains("exceed") || low.contains("too long") || low.contains("length")))
        || low.contains("n_ctx")
        || low.contains("maximum context")
        || low.contains("context_length_exceeded");
    if overflow {
        EngineErr::ContextOverflow(msg)
    } else if matches!(status, 429 | 502 | 503 | 504) || (status == 500 && low.contains("loading")) {
        EngineErr::Transient(msg)
    } else {
        EngineErr::Fatal(msg)
    }
}

enum Streamed {
    Done(Assembler),
    /// the user pressed Stop mid-stream; whatever arrived so far
    Cancelled(Assembler),
}

async fn stream_turn(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    body: &Value,
    ctl: &RunCtl,
    app: &AppHandle,
    run_id: &str,
) -> Result<Streamed, EngineErr> {
    let mut rq = client.post(format!("{base}/v1/chat/completions")).json(body);
    if !key.is_empty() {
        rq = rq.bearer_auth(key);
    }
    let send = tokio::select! {
        r = rq.send() => Some(r),
        _ = ctl.notify.notified() => None,
    };
    let Some(resp) = send else { return Ok(Streamed::Cancelled(Assembler::default())) };
    let resp = resp.map_err(|e| {
        if e.is_connect() || e.is_timeout() {
            EngineErr::Transient(format!("could not reach the engine: {e}"))
        } else {
            EngineErr::Fatal(format!("request failed: {e}"))
        }
    })?;
    if !resp.status().is_success() {
        let st = resp.status().as_u16();
        let t = resp.text().await.unwrap_or_default();
        return Err(classify_status(st, &t));
    }
    let mut asm = Assembler::default();
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    let mut first_token = true;
    let started = Instant::now();
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = vec![];
    loop {
        let next = tokio::select! {
            n = stream.next() => n,
            _ = ctl.notify.notified() => None,
        };
        if ctl.is_cancelled() {
            return Ok(Streamed::Cancelled(asm));
        }
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|e| EngineErr::Fatal(format!("stream interrupted: {e}")))?;
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                return Ok(Streamed::Done(asm));
            }
            if let Ok(v) = serde_json::from_str::<Value>(data) {
                if let Some(err) = v.get("error") {
                    let m = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error").to_string();
                    return Err(classify_status(500, &m));
                }
                for d in asm.feed(&v) {
                    if first_token {
                        first_token = false;
                        let ms = started.elapsed().as_millis() as u64;
                        asm.ttft_ms = Some(ms);
                        ev(app, run_id, "first_token", json!({"ms": ms}));
                    }
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
    Ok(Streamed::Done(asm))
}

#[allow(clippy::too_many_arguments)]
async fn await_permission(
    app: &AppHandle,
    agent: &Agent,
    ctl: &RunCtl,
    run_id: &str,
    call_id: &str,
    name: &str,
    args: &Value,
    preview: Option<Value>,
) -> bool {
    let (tx, rx) = oneshot::channel();
    agent.perms.lk().insert(call_id.to_string(), tx);
    ev(app, run_id, "permission", json!({"call_id":call_id,"name":name,"args":args,"preview":preview}));
    let allowed = tokio::select! {
        r = rx => r.unwrap_or(false),
        _ = ctl.notify.notified() => false,
    };
    agent.perms.lk().remove(call_id);
    allowed && !ctl.is_cancelled()
}

fn trunc_out(s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let cut = s.floor_char_boundary(max);
    format!("{}\n...[output truncated: {} more characters]", &s[..cut], s.len() - cut)
}

/// Same tool + same arguments this many times in one run means the model is stuck.
const REPEAT_LIMIT: usize = 2;

fn tool_props(defs: &[Value], name: &str) -> Option<Map<String, Value>> {
    defs.iter()
        .find(|d| d["function"]["name"].as_str() == Some(name))
        .and_then(|d| d["function"]["parameters"]["properties"].as_object().cloned())
}

/// Delay before retry `n` (0-based): 1s, 2s, 4s.
fn backoff(n: u32) -> Duration {
    Duration::from_millis(1000 * (1u64 << n.min(4)))
}

const MAX_RETRIES: u32 = 3;

async fn run_loop(app: AppHandle, a: RunArgs, ctl: Arc<RunCtl>, run: &trace::Run) -> Result<&'static str, String> {
    let eng = app.state::<Engine>();
    let mcp = app.state::<Arc<Mcp>>();
    let agent = app.state::<Agent>();
    let ep = eng.endpoint().await.ok_or("no model loaded: load one from the Models tab")?;
    run.set(json!({"model": ep.model, "engine": ep.engine, "ctx": ep.ctx, "local": ep.local}));
    let client = reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())?;

    let ws = if a.workspace.is_empty() { None } else { Some(PathBuf::from(&a.workspace)) };
    let tier = tools::Tier::from_ctx(ep.ctx, ep.local);
    let recorder = a.checkpoint_id.as_ref().map(|id| checkpoint::Recorder { chat: a.chat_id.clone(), id: id.clone() });
    let tctx = ws.as_ref().map(|w| tools::ToolCtx::new(w.clone(), tier, recorder));
    let mut tool_defs: Vec<Value> = vec![];
    let mut mcp_ro: HashSet<String> = HashSet::new();
    if a.use_tools {
        if ws.is_some() {
            tool_defs.extend(tools::schemas(tier));
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
    let known: HashSet<String> = tool_defs.iter().filter_map(|d| d["function"]["name"].as_str().map(String::from)).collect();
    run.set(json!({"tier": format!("{tier:?}").to_lowercase(), "tools": known.len()}));

    // Built once: the system prompt and tool list must stay byte-identical across turns so a
    // local server can reuse its KV cache for the whole prefix instead of re-reading it.
    let mut sys = a.system_prompt.clone();
    if let Some(w) = &ws {
        sys.push_str(&format!(
            "\n\nWorkspace folder: {}\nShell: PowerShell on Windows.\nTo change an existing file use edit_file (or edit_lines with line numbers from read_file); never rewrite a whole file to change a few lines. For a big file, read_file with outline=true first, then read only the part you need. Large new files: write the first part, then write_file with append=true for each next part (about 150 lines per call).",
            w.display()
        ));
    }

    let perms = crate::store::load_settings();
    let mut msgs: Vec<Value> = repair_history(&a.messages);
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut budget = (ep.ctx > 0).then(|| context::Budget::new(ep.ctx as usize, sys.len() + Value::from(tool_defs.clone()).to_string().len()));
    for _turn in 0..a.max_turns.max(1) {
        if let Some(b) = &budget {
            let idx = context::plan_prune(&msgs, b, 2);
            if !idx.is_empty() {
                context::apply(&mut msgs, &idx);
                let ids: Vec<&str> = idx.iter().filter_map(|&i| msgs[i]["tool_call_id"].as_str()).collect();
                run.event("prune", json!({"removed": ids.len()}));
                ev(&app, &a.run_id, "pruned", json!({"call_ids":ids}));
                ev(&app, &a.run_id, "notice", json!({"text":format!("Context nearly full: removed {} old tool outputs from the prompt.", ids.len())}));
                if let Some(c) = &tctx {
                    c.forget_reads();
                }
            }
        }
        let mut send: Vec<Value> = vec![json!({"role":"system","content":sys})];
        send.extend(msgs.iter().map(strip_for_send));
        let mut body = json!({
            "model": ep.model, "messages": send, "stream": true,
            "stream_options": {"include_usage": true}, "temperature": a.temperature,
        });
        if ep.local {
            body["cache_prompt"] = json!(true);
        }
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
        let mut attempt = 0;
        let streamed = loop {
            match stream_turn(&client, &ep.base, &ep.key, &body, &ctl, &app, &a.run_id).await {
                Ok(s) => break s,
                Err(EngineErr::Transient(e)) if attempt < MAX_RETRIES && !ctl.is_cancelled() => {
                    let wait = backoff(attempt);
                    attempt += 1;
                    crate::log::write("warn", "agent", &format!("retry {attempt}/{MAX_RETRIES}: {e}"), Some(&a.run_id), None);
                    run.event("retry", json!({"attempt":attempt,"reason":e}));
                    ev(&app, &a.run_id, "retry", json!({"attempt":attempt,"max":MAX_RETRIES,"wait_ms":wait.as_millis() as u64,"reason":e}));
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {},
                        _ = ctl.notify.notified() => {},
                    }
                    if ctl.is_cancelled() {
                        return Ok("cancelled");
                    }
                }
                Err(EngineErr::Transient(e)) | Err(EngineErr::Fatal(e)) => return Err(e),
                Err(EngineErr::ContextOverflow(e)) => {
                    return Err(format!("context_overflow: the conversation no longer fits the model's context window. Start a new chat or load the model with a larger context. ({e})"))
                }
            }
        };
        let asm = match streamed {
            Streamed::Done(asm) => asm,
            Streamed::Cancelled(asm) => {
                // keep what the user already saw; drop half-generated tool calls
                if !asm.content.trim().is_empty() || !asm.reasoning.trim().is_empty() {
                    let mut am = json!({"role":"assistant","content":asm.content,"ui":{"cancelled":true}});
                    if !asm.reasoning.is_empty() {
                        am["reasoning_content"] = json!(asm.reasoning);
                    }
                    ev(&app, &a.run_id, "message", json!({"message":am}));
                }
                return Ok("cancelled");
            }
        };
        // learn this model's real chars-per-token so the budget gets accurate
        let prompt_total = asm.prompt_tokens.or(asm.prompt_n.zip(asm.cache_n).map(|(p, c)| p + c));
        if let (Some(b), Some(pt)) = (budget.as_mut(), prompt_total) {
            let sent: usize = msgs.iter().map(context::sent_chars).sum::<usize>();
            let fixed_chars = sys.len() + Value::from(tool_defs.clone()).to_string().len();
            if pt > 200 {
                b.chars_per_token = Some((sent + fixed_chars) as f64 / pt as f64);
                b.fixed = b.tokens(fixed_chars);
            }
        }
        let secs = started.elapsed().as_secs_f64();
        let toks = asm.completion_tokens.unwrap_or(asm.chunks);
        let tok_s = asm.server_tok_s.unwrap_or(if secs > 0.0 { toks as f64 / secs } else { 0.0 });
        ev(
            &app,
            &a.run_id,
            "usage",
            json!({
                "tokens": toks, "secs": secs, "tok_s": tok_s,
                "prompt_tokens": asm.prompt_tokens.or(asm.prompt_n.zip(asm.cache_n).map(|(p, c)| p + c)),
                "prompt_n": asm.prompt_n, "prompt_ms": asm.prompt_ms, "cache_n": asm.cache_n,
                "ctx": ep.ctx,
            }),
        );

        run.event(
            "turn",
            json!({
                "prompt_tokens": prompt_total, "prompt_n": asm.prompt_n, "cache_n": asm.cache_n, "prompt_ms": asm.prompt_ms,
                "ttft_ms": asm.ttft_ms, "tokens": toks, "tok_s": tok_s, "secs": secs, "finish": asm.finish,
                "calls": asm.calls.len(), "chars": asm.content.len(), "reasoning_chars": asm.reasoning.len(),
            }),
        );
        let mut content = asm.content.clone();
        let mut calls: Vec<(String, String, String)> = asm
            .calls
            .values()
            .filter(|(_, n, _)| !n.is_empty())
            .map(|(id, n, args)| {
                let id = if id.is_empty() { format!("call_{}", uuid::Uuid::new_v4().simple()) } else { id.clone() };
                (id, n.clone(), args.clone())
            })
            .collect();
        // small models often write the call as text instead of using the tool_calls field
        if calls.is_empty() && !known.is_empty() {
            let (found, rest) = toolparse::extract_text_calls(&content, &|n| known.contains(n));
            if !found.is_empty() {
                content = rest;
                calls = found
                    .into_iter()
                    .map(|c| (format!("call_{}", uuid::Uuid::new_v4().simple()), c.name, c.args.to_string()))
                    .collect();
                run.event("recovered_text_calls", json!({"count": calls.len()}));
                crate::log::write("info", "agent", "recovered tool call(s) written as text", Some(&a.run_id), None);
                ev(&app, &a.run_id, "notice", json!({"text":"Recovered a tool call the model wrote as text."}));
            }
        }

        let mut am = json!({"role":"assistant","content":content});
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
            if asm.finish.as_deref() == Some("length") {
                return Ok("length");
            }
            return Ok("done");
        }

        let mut cancelled = false;
        for (id, name, args_s) in calls {
            if cancelled || ctl.is_cancelled() {
                cancelled = true;
                let tm = json!({"role":"tool","tool_call_id":id,"name":name,"content":CANCELLED_TOOL,"ui":{"ok":false,"cancelled":true}});
                msgs.push(tm.clone());
                ev(&app, &a.run_id, "message", json!({"message":tm}));
                continue;
            }
            let mut args = toolparse::repair_json(&args_s).unwrap_or(Value::Null);
            if let (Some(props), true) = (tool_props(&tool_defs, &name), args.is_object()) {
                toolparse::normalize_args(&mut args, &props);
            }
            ev(&app, &a.run_id, "tool_start", json!({"call_id":id,"name":name,"args":args}));
            let t0 = Instant::now();
            let key = format!("{name}\u{1}{args}");
            let n_seen = {
                let c = seen.entry(key).or_insert(0);
                *c += 1;
                *c
            };
            let builtin = tools::is_builtin(&name);
            // file changes are worked out before asking, so a bad edit fails fast and a good one
            // is shown to the user as a diff
            let planned = match (&tctx, builtin && tools::needs_approval(&name) && name != "run_shell") {
                (Some(c), true) if args.is_object() => Some(tools::plan(&name, &args, c).map(|ch| ch.preview())),
                _ => None,
            };
            let (ok, output, extra) = if !known.contains(&name) {
                (false, format!("Unknown tool '{name}'. Available tools: {}.", known.iter().cloned().collect::<Vec<_>>().join(", ")), None)
            } else if !args.is_object() {
                (false, format!("Arguments must be a JSON object; could not parse: {}", args_s.chars().take(300).collect::<String>()), None)
            } else if n_seen > REPEAT_LIMIT {
                (false, format!("You already called {name} with these exact arguments {} times; the result will not change. Try a different approach or answer the user.", n_seen - 1), None)
            } else if let Some(Err(e)) = &planned {
                (false, e.clone(), None)
            } else if let Some(d) = (name == "run_shell").then(|| policy::denied_by(args["command"].as_str().unwrap_or(""), &perms.shell_deny)).flatten() {
                (false, format!("Blocked: the command matches \"{d}\" on the user's shell deny list. Do not retry it; find another way or ask the user."), None)
            } else {
                let risky = if builtin { tools::needs_approval(&name) } else { !mcp_ro.contains(&name) };
                let cmd = args["command"].as_str().unwrap_or("");
                let dangerous = name == "run_shell" && policy::is_dangerous(cmd);
                let trusted = perms.trusted.contains(&policy::trust_key(&name, &a.workspace));
                let shell_ok = name == "run_shell" && policy::allowed_by(cmd, &perms.shell_allow);
                let pre_ok = !risky
                    || (!dangerous && (a.auto_approve || trusted || shell_ok || agent.always.lk().contains(&(a.chat_id.clone(), name.clone()))));
                let mut preview = planned.as_ref().and_then(|p| p.as_ref().ok()).cloned();
                if name == "run_shell" {
                    preview = Some(json!({"kind":"shell","command":cmd,"dangerous":dangerous,"prefix":policy::suggest_prefix(cmd)}));
                }
                let allowed = pre_ok || await_permission(&app, &agent, &ctl, &a.run_id, &id, &name, &args, preview).await;
                if ctl.is_cancelled() {
                    (false, CANCELLED_TOOL.to_string(), None)
                } else if !allowed {
                    (false, "The user denied this tool call.".to_string(), None)
                } else if builtin {
                    match &tctx {
                        Some(c) => match tools::run(&name, &args, c).await {
                            Ok(o) => (true, o.text, o.ui),
                            Err(e) => (false, e, None),
                        },
                        None => (false, "no workspace folder is selected".to_string(), None),
                    }
                } else {
                    match mcp.call(&name, args.clone()).await {
                        Ok(o) => (true, trunc_out(o, tier.max_out()), None),
                        Err(e) => (false, e, None),
                    }
                }
            };
            let ms = t0.elapsed().as_millis() as u64;
            if ctl.is_cancelled() {
                cancelled = true;
            }
            let args_short: String = args.to_string().chars().take(400).collect();
            run.event(
                "tool",
                json!({
                    "name": name, "args": args_short, "ok": ok, "ms": ms, "out_chars": output.len(),
                    "error": (!ok).then(|| output.chars().take(300).collect::<String>()),
                    "change": extra.as_ref().map(|c| json!({"path": c["path"], "added": c["added"], "removed": c["removed"]})),
                }),
            );
            if !ok && !cancelled {
                crate::log::write("warn", "agent", &format!("{name} failed: {}", output.chars().take(200).collect::<String>()), Some(&a.run_id), None);
            }
            ev(&app, &a.run_id, "tool_result", json!({"call_id":id,"ok":ok,"output":output,"ms":ms,"change":extra}));
            let mut ui = json!({"ok":ok,"ms":ms,"cancelled":cancelled});
            if let Some(x) = extra {
                ui["change"] = x;
            }
            let tm = json!({"role":"tool","tool_call_id":id,"name":name,"content":output,"ui":ui});
            msgs.push(tm.clone());
            ev(&app, &a.run_id, "message", json!({"message":tm}));
        }
        if cancelled {
            return Ok("cancelled");
        }
    }
    Ok("max_turns")
}

#[tauri::command]
pub async fn agent_run(app: AppHandle, agent: State<'_, Agent>, args: RunArgs) -> Result<(), String> {
    let ctl = Arc::new(RunCtl::default());
    agent.runs.lk().insert(args.run_id.clone(), ctl.clone());
    let run_id = args.run_id.clone();
    let app2 = app.clone();
    let run = trace::Run::start(
        &run_id,
        json!({"chat_id": args.chat_id, "messages": args.messages.len(), "use_tools": args.use_tools, "workspace": args.workspace}),
    );
    crate::log::write("info", "agent", "run started", Some(&run_id), Some(json!({"chat_id": args.chat_id})));
    tauri::async_runtime::spawn(async move {
        let r = run_loop(app2.clone(), args, ctl, &run).await;
        match &r {
            Ok(reason) => {
                run.finish(reason, None);
                crate::log::write("info", "agent", &format!("run finished: {reason}"), Some(&run_id), None);
                ev(&app2, &run_id, "done", json!({"reason":reason}));
            }
            Err(e) => {
                run.finish("error", Some(e));
                crate::log::write("error", "agent", &format!("run failed: {e}"), Some(&run_id), None);
                ev(&app2, &run_id, "error", json!({"message":e}));
            }
        }
        app2.state::<Agent>().runs.lk().remove(&run_id);
    });
    Ok(())
}

#[tauri::command]
pub fn agent_cancel(agent: State<'_, Agent>, run_id: String) {
    if let Some(c) = agent.runs.lk().get(&run_id) {
        c.cancelled.store(true, Ordering::SeqCst);
        c.notify.notify_waiters();
        // wake a waiter that registered after notify_waiters
        c.notify.notify_one();
    }
}

/// `scope`: "once" | "chat" (until the app closes) | "workspace" (saved: trust this tool here) |
/// "prefix" (saved: run_shell commands starting with `prefix` no longer ask).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn permission_reply(
    agent: State<'_, Agent>,
    call_id: String,
    chat_id: String,
    tool: String,
    allow: bool,
    always: bool,
    scope: Option<String>,
    workspace: Option<String>,
    prefix: Option<String>,
) {
    let scope = scope.unwrap_or_else(|| if always { "chat".into() } else { "once".into() });
    if allow {
        match scope.as_str() {
            "chat" => {
                agent.always.lk().insert((chat_id, tool.clone()));
            }
            "workspace" => {
                if let Some(ws) = workspace.filter(|w| !w.is_empty()) {
                    let mut s = crate::store::load_settings();
                    let k = policy::trust_key(&tool, &ws);
                    if !s.trusted.contains(&k) {
                        s.trusted.push(k);
                        let _ = crate::store::patch_settings(&json!({"trusted": s.trusted}));
                    }
                }
            }
            "prefix" => {
                if let Some(p) = prefix.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
                    let mut s = crate::store::load_settings();
                    if !s.shell_allow.iter().any(|x| x.eq_ignore_ascii_case(&p)) {
                        s.shell_allow.push(p);
                        let _ = crate::store::patch_settings(&json!({"shell_allow": s.shell_allow}));
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(tx) = agent.perms.lk().remove(&call_id) {
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
            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"completion_tokens":42,"prompt_tokens":900},
                   "timings":{"prompt_n":100,"prompt_ms":250.0,"cache_n":800,"predicted_per_second":30.5}}),
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
        assert_eq!((a.prompt_tokens, a.prompt_n, a.cache_n), (Some(900), Some(100), Some(800)));
        assert_eq!(a.server_tok_s, Some(30.5));
    }

    #[test]
    fn repair_fills_missing_tool_results_and_drops_orphans() {
        let h = vec![
            json!({"role":"user","content":"hi"}),
            json!({"role":"assistant","content":"","tool_calls":[
                {"id":"a","type":"function","function":{"name":"read_file","arguments":"{}"}},
                {"id":"b","type":"function","function":{"name":"list_dir","arguments":"{}"}}]}),
            json!({"role":"tool","tool_call_id":"a","content":"ok"}),
            // cancelled before "b" ran; then the user typed again
            json!({"role":"user","content":"again"}),
            json!({"role":"tool","tool_call_id":"zzz","content":"orphan"}),
        ];
        let r = repair_history(&h);
        let roles: Vec<&str> = r.iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["user", "assistant", "tool", "tool", "user"]);
        assert_eq!(r[2]["content"], "ok");
        assert_eq!(r[3]["tool_call_id"], "b");
        assert_eq!(r[3]["content"], CANCELLED_TOOL);
        // valid history is unchanged
        assert_eq!(repair_history(&r[..4]), r[..4].to_vec());
    }

    #[test]
    fn classifies_engine_errors() {
        assert!(matches!(classify_status(503, "Loading model"), EngineErr::Transient(_)));
        assert!(matches!(classify_status(429, "slow down"), EngineErr::Transient(_)));
        assert!(matches!(
            classify_status(400, "the request exceeds the available context size, try increasing it"),
            EngineErr::ContextOverflow(_)
        ));
        assert!(matches!(classify_status(400, "This model's maximum context length is 8192"), EngineErr::ContextOverflow(_)));
        assert!(matches!(classify_status(401, "bad key"), EngineErr::Fatal(_)));
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert_eq!(backoff(2), Duration::from_secs(4));
    }
}
