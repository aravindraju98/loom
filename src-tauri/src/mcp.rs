//! MCP client: stdio and streamable-HTTP servers, configured with the usual {"mcpServers": {...}} JSON.
use crate::store;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;

const PROTOCOL: &str = "2025-06-18";

#[derive(Serialize, Clone, Debug)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    #[serde(skip)]
    pub schema: Value,
    #[serde(skip)]
    pub read_only: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct ServerStatus {
    pub name: String,
    pub transport: String,
    /// "connecting" | "ready" | "error" | "disabled"
    pub state: String,
    pub error: String,
    pub tools: Vec<ToolInfo>,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

enum Transport {
    Stdio { stdin: tokio::sync::Mutex<ChildStdin>, pending: Pending, _child: tokio::sync::Mutex<Child>, pid: u32 },
    Http { url: String, headers: HashMap<String, String>, session: Mutex<Option<String>>, client: reqwest::Client },
}

struct Conn {
    t: Transport,
    next: AtomicU64,
}

impl Conn {
    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let msg = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        let reply = match &self.t {
            Transport::Stdio { stdin, pending, .. } => {
                let (tx, rx) = oneshot::channel();
                pending.lock().unwrap().insert(id, tx);
                let mut line = msg.to_string();
                line.push('\n');
                {
                    let mut w = stdin.lock().await;
                    if let Err(e) = w.write_all(line.as_bytes()).await {
                        pending.lock().unwrap().remove(&id);
                        return Err(format!("write failed: {e}"));
                    }
                    let _ = w.flush().await;
                }
                match tokio::time::timeout(Duration::from_secs(120), rx).await {
                    Ok(Ok(v)) => v,
                    Ok(Err(_)) => return Err("server closed the connection".into()),
                    Err(_) => {
                        pending.lock().unwrap().remove(&id);
                        return Err(format!("{method} timed out"));
                    }
                }
            }
            Transport::Http { .. } => self.http_post(&msg, Some(id)).await?.ok_or("no response")?,
        };
        if let Some(e) = reply.get("error") {
            return Err(e.get("message").and_then(|m| m.as_str()).unwrap_or("MCP error").to_string());
        }
        Ok(reply.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn notify(&self, method: &str, params: Value) {
        let msg = json!({"jsonrpc":"2.0","method":method,"params":params});
        match &self.t {
            Transport::Stdio { stdin, .. } => {
                let mut line = msg.to_string();
                line.push('\n');
                let mut w = stdin.lock().await;
                let _ = w.write_all(line.as_bytes()).await;
                let _ = w.flush().await;
            }
            Transport::Http { .. } => {
                let _ = self.http_post(&msg, None).await;
            }
        }
    }

    async fn http_post(&self, msg: &Value, want_id: Option<u64>) -> Result<Option<Value>, String> {
        let Transport::Http { url, headers, session, client } = &self.t else { return Err("not http".into()) };
        let mut rb = client
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL)
            .timeout(Duration::from_secs(120))
            .json(msg);
        for (k, v) in headers {
            rb = rb.header(k, v);
        }
        if let Some(s) = session.lock().unwrap().clone() {
            rb = rb.header("Mcp-Session-Id", s);
        }
        let resp = rb.send().await.map_err(|e| e.to_string())?;
        if let Some(s) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            *session.lock().unwrap() = Some(s.to_string());
        }
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }
        let Some(id) = want_id else { return Ok(None) };
        let is_sse = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.contains("event-stream"))
            .unwrap_or(false);
        let body = resp.text().await.map_err(|e| e.to_string())?;
        if is_sse {
            for line in body.lines() {
                if let Some(d) = line.strip_prefix("data:") {
                    if let Ok(v) = serde_json::from_str::<Value>(d.trim()) {
                        if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
                            return Ok(Some(v));
                        }
                    }
                }
            }
            Err("no response in event stream".into())
        } else {
            serde_json::from_str(&body).map(Some).map_err(|e| e.to_string())
        }
    }
}

fn spawn_stdio(cfg: &Value) -> Result<Conn, String> {
    let command = cfg["command"].as_str().ok_or("missing 'command'")?;
    let args: Vec<String> = cfg["args"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let mut cmd = if command.to_lowercase().ends_with(".exe") {
        let mut c = Command::new(command);
        c.args(&args);
        c
    } else {
        // npx, uvx, *.cmd shims: let cmd.exe resolve them
        let mut c = Command::new("cmd");
        c.arg("/C").arg(command).args(&args);
        c
    };
    if let Some(env) = cfg["env"].as_object() {
        for (k, v) in env {
            if let Some(v) = v.as_str() {
                cmd.env(k, v);
            }
        }
    }
    if let Some(d) = cfg["cwd"].as_str() {
        cmd.current_dir(d);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    {
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    let pid = child.id().unwrap_or(0);
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let p2 = pending.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v.get("method").is_none() {
                if let Some(id) = v.get("id").and_then(|x| x.as_u64()) {
                    if let Some(tx) = p2.lock().unwrap().remove(&id) {
                        let _ = tx.send(v);
                    }
                }
            }
        }
        // process ended: fail anything still waiting
        p2.lock().unwrap().clear();
    });
    Ok(Conn {
        t: Transport::Stdio { stdin: tokio::sync::Mutex::new(stdin), pending, _child: tokio::sync::Mutex::new(child), pid },
        next: AtomicU64::new(1),
    })
}

fn make_http(cfg: &Value) -> Result<Conn, String> {
    let url = cfg["url"].as_str().ok_or("missing 'url'")?.to_string();
    let mut headers = HashMap::new();
    if let Some(h) = cfg["headers"].as_object() {
        for (k, v) in h {
            if let Some(v) = v.as_str() {
                headers.insert(k.clone(), v.to_string());
            }
        }
    }
    Ok(Conn {
        t: Transport::Http { url, headers, session: Mutex::new(None), client: reqwest::Client::new() },
        next: AtomicU64::new(1),
    })
}

async fn handshake(conn: &Conn) -> Result<Vec<ToolInfo>, String> {
    conn.request(
        "initialize",
        json!({"protocolVersion":PROTOCOL,"capabilities":{},"clientInfo":{"name":"harness","version":"0.1.0"}}),
    )
    .await?;
    conn.notify("notifications/initialized", json!({})).await;
    let mut tools = vec![];
    let mut cursor: Option<String> = None;
    loop {
        let params = match &cursor {
            Some(c) => json!({"cursor": c}),
            None => json!({}),
        };
        let r = conn.request("tools/list", params).await?;
        for t in r["tools"].as_array().cloned().unwrap_or_default() {
            tools.push(ToolInfo {
                name: t["name"].as_str().unwrap_or("").to_string(),
                description: t["description"].as_str().unwrap_or("").to_string(),
                schema: t.get("inputSchema").cloned().unwrap_or(json!({"type":"object","properties":{}})),
                read_only: t["annotations"]["readOnlyHint"].as_bool().unwrap_or(false),
            });
        }
        cursor = r["nextCursor"].as_str().map(String::from);
        if cursor.is_none() {
            break;
        }
    }
    Ok(tools)
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

#[derive(Default)]
pub struct Mcp {
    conns: tokio::sync::Mutex<HashMap<String, Arc<Conn>>>,
    status: Mutex<Vec<ServerStatus>>,
    generation: AtomicU64,
}

pub fn config_path() -> std::path::PathBuf {
    store::data_dir().join("mcp.json")
}

const DEFAULT_CONFIG: &str = "{\n  \"mcpServers\": {}\n}\n";

pub fn read_config_text() -> String {
    std::fs::read_to_string(config_path()).unwrap_or_else(|_| DEFAULT_CONFIG.to_string())
}

impl Mcp {
    fn set_status(&self, name: &str, f: impl FnOnce(&mut ServerStatus)) {
        let mut g = self.status.lock().unwrap();
        if let Some(s) = g.iter_mut().find(|s| s.name == name) {
            f(s);
        }
    }

    pub async fn reload(self: &Arc<Self>, app: AppHandle) {
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.shutdown().await;
        let text = read_config_text();
        let cfg: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or(json!({}));
        let servers = cfg.get("mcpServers").or_else(|| cfg.get("servers")).and_then(|v| v.as_object()).cloned().unwrap_or_default();
        {
            let mut st = self.status.lock().unwrap();
            st.clear();
            for (name, c) in &servers {
                let disabled = c["disabled"].as_bool().unwrap_or(false);
                st.push(ServerStatus {
                    name: name.clone(),
                    transport: if c.get("url").is_some() { "http".into() } else { "stdio".into() },
                    state: if disabled { "disabled".into() } else { "connecting".into() },
                    error: String::new(),
                    tools: vec![],
                });
            }
        }
        let _ = app.emit("mcp-changed", ());
        for (name, c) in servers {
            if c["disabled"].as_bool().unwrap_or(false) {
                continue;
            }
            let me = self.clone();
            let app = app.clone();
            tokio::spawn(async move {
                let conn = if c.get("url").is_some() { make_http(&c) } else { spawn_stdio(&c) };
                let result = match conn {
                    Ok(conn) => {
                        let conn = Arc::new(conn);
                        match tokio::time::timeout(Duration::from_secs(60), handshake(&conn)).await {
                            Ok(Ok(tools)) => Ok((conn, tools)),
                            Ok(Err(e)) => Err(e),
                            Err(_) => Err("handshake timed out".to_string()),
                        }
                    }
                    Err(e) => Err(e),
                };
                if me.generation.load(Ordering::SeqCst) != gen {
                    return; // a newer reload superseded this one
                }
                match result {
                    Ok((conn, tools)) => {
                        me.conns.lock().await.insert(name.clone(), conn);
                        me.set_status(&name, |s| {
                            s.state = "ready".into();
                            s.tools = tools;
                        });
                    }
                    Err(e) => me.set_status(&name, |s| {
                        s.state = "error".into();
                        s.error = e;
                    }),
                }
                let _ = app.emit("mcp-changed", ());
            });
        }
    }

    pub async fn shutdown(&self) {
        let mut g = self.conns.lock().await;
        for (_, c) in g.drain() {
            if let Transport::Stdio { pid, .. } = &c.t {
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }

    pub fn kill_blocking(&self) {
        if let Ok(mut g) = self.conns.try_lock() {
            for (_, c) in g.drain() {
                if let Transport::Stdio { pid, .. } = &c.t {
                    let _ = std::process::Command::new("taskkill")
                        .args(["/PID", &pid.to_string(), "/T", "/F"])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            }
        }
    }

    /// Tool schemas in OpenAI format, namespaced `server__tool`, plus which are read-only.
    pub fn tool_defs(&self) -> Vec<(Value, bool)> {
        let mut out = vec![];
        for s in self.status.lock().unwrap().iter().filter(|s| s.state == "ready") {
            for t in &s.tools {
                let name = format!("{}__{}", sanitize(&s.name), sanitize(&t.name));
                out.push((
                    json!({"type":"function","function":{
                        "name": name,
                        "description": format!("[{}] {}", s.name, t.description),
                        "parameters": t.schema}}),
                    t.read_only,
                ));
            }
        }
        out
    }

    pub async fn call(&self, namespaced: &str, args: Value) -> Result<String, String> {
        // find the server + original tool name
        let (server, tool) = {
            let st = self.status.lock().unwrap();
            let mut found = None;
            for s in st.iter() {
                for t in &s.tools {
                    if format!("{}__{}", sanitize(&s.name), sanitize(&t.name)) == namespaced {
                        found = Some((s.name.clone(), t.name.clone()));
                    }
                }
            }
            found.ok_or_else(|| format!("unknown MCP tool {namespaced}"))?
        };
        let conn = self.conns.lock().await.get(&server).cloned().ok_or("MCP server not connected")?;
        let r = conn.request("tools/call", json!({"name": tool, "arguments": args})).await?;
        let mut text = String::new();
        for c in r["content"].as_array().cloned().unwrap_or_default() {
            match c["type"].as_str() {
                Some("text") => {
                    text.push_str(c["text"].as_str().unwrap_or(""));
                    text.push('\n');
                }
                Some(other) => text.push_str(&format!("[{other} content omitted]\n")),
                None => {}
            }
        }
        if text.is_empty() {
            if let Some(sc) = r.get("structuredContent") {
                text = sc.to_string();
            }
        }
        if r["isError"].as_bool().unwrap_or(false) {
            Err(text.trim().to_string())
        } else {
            Ok(text.trim().to_string())
        }
    }
}

#[tauri::command]
pub fn mcp_status(mcp: State<'_, Arc<Mcp>>) -> Vec<ServerStatus> {
    mcp.status.lock().unwrap().clone()
}

#[tauri::command]
pub fn mcp_get_config() -> String {
    read_config_text()
}

#[tauri::command]
pub async fn mcp_save_config(app: AppHandle, mcp: State<'_, Arc<Mcp>>, text: String) -> Result<(), String> {
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("invalid JSON: {e}"))?;
    if !v.get("mcpServers").map(|s| s.is_object()).unwrap_or(false) {
        return Err("config needs an \"mcpServers\" object".into());
    }
    std::fs::write(config_path(), text).map_err(|e| e.to_string())?;
    let m = mcp.inner().clone();
    m.reload(app).await;
    Ok(())
}

#[tauri::command]
pub async fn mcp_reload(app: AppHandle, mcp: State<'_, Arc<Mcp>>) -> Result<(), String> {
    let m = mcp.inner().clone();
    m.reload(app).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stdio_roundtrip_with_stub_server() {
        // A tiny MCP server written in PowerShell would be slow to start; use python (installed here).
        let script = r#"
import sys, json
for line in sys.stdin:
    m = json.loads(line)
    if 'id' not in m: continue
    meth = m['method']
    if meth == 'initialize':
        r = {'protocolVersion':'2025-06-18','capabilities':{'tools':{}},'serverInfo':{'name':'stub','version':'1'}}
    elif meth == 'tools/list':
        r = {'tools':[{'name':'echo','description':'echo text','inputSchema':{'type':'object','properties':{'text':{'type':'string'}}}}]}
    elif meth == 'tools/call':
        r = {'content':[{'type':'text','text':'echo:'+m['params']['arguments'].get('text','')}]}
    else:
        r = {}
    print(json.dumps({'jsonrpc':'2.0','id':m['id'],'result':r}), flush=True)
"#;
        let path = std::env::temp_dir().join("harness_stub_mcp.py");
        std::fs::write(&path, script).unwrap();
        let py = "python.exe";
        let cfg = json!({"command": py, "args": [path.to_str().unwrap()]});
        let conn = spawn_stdio(&cfg).unwrap();
        let tools = handshake(&conn).await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        let r = conn.request("tools/call", json!({"name":"echo","arguments":{"text":"hi"}})).await.unwrap();
        assert_eq!(r["content"][0]["text"], "echo:hi");
    }
}
