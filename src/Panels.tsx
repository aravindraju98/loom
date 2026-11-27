import { useEffect, useState } from "preact/hooks";
import { api, fmtBytes } from "./api";
import {
  engine, loadError, loadModel, mcp, models, modelsLoading, refreshMcp, refreshModels, saveSettings, scanError,
  settings, unloadModel,
} from "./state";

import { Icon } from "./icons";
import { RemoteEndpoints } from "./ServerPanel";

const ENGINE_LABEL: Record<string, string> = { polystrata: "PolyStrata", llama: "llama.cpp", openai: "remote" };

/* ---------------- Models ---------------- */

export function ModelsPanel() {
  const [log, setLog] = useState<string[] | null>(null);
  const st = engine.value;
  useEffect(() => {
    if (log === null) return;
    const t = setInterval(() => void api.engineLog().then(setLog), 1500);
    return () => clearInterval(t);
  }, [log === null]);

  const setOverride = async (id: string, eng: string) => {
    const s = settings.value;
    if (!s) return;
    if (await saveSettings({ engine_overrides: { ...s.engine_overrides, [id]: eng } })) await refreshModels();
  };

  return (
    <div class="panel-body">
      <div class="row">
        <span class="section-title grow" style={{ margin: 0 }}>Library · {models.value.length}</span>
        <button class="icon-btn" title="Rescan model folders" aria-label="Rescan model folders" onClick={() => void refreshModels()} disabled={modelsLoading.value}>
          <span class={modelsLoading.value ? "spinning" : ""}><Icon name="refresh" /></span>
        </button>
        <button class={`icon-btn ${log !== null ? "on" : ""}`} title="Engine log" aria-label="Engine log" aria-pressed={log !== null} onClick={() => setLog(log === null ? [] : null)}>
          <Icon name="terminal" />
        </button>
      </div>
      {st.state === "error" && <div class="error">{st.error}</div>}
      {loadError.value && st.state !== "error" && <div class="error">{loadError.value}</div>}
      {log !== null && <pre class="log">{log.join("\n") || "(no output yet)"}</pre>}
      {scanError.value && <div class="error">{scanError.value}</div>}
      {modelsLoading.value && models.value.length === 0 && <div class="hint">Scanning model folders…</div>}
      {models.value.length === 0 && !modelsLoading.value && !scanError.value && (
        <div class="hint">No .gguf models found in {(settings.value?.model_dirs ?? []).join(", ")}. Add folders in Settings or download from the Hub tab.</div>
      )}
      {models.value.map((m) => {
        const active = st.model_id === m.id && st.state !== "idle";
        return (
          <div class={`mrow ${active ? "active" : ""}`} key={m.id}>
            <div class="grow">
              <div class="menu-name ellipsis" title={m.name}>{m.name}</div>
              <div class="menu-sub">
                {m.quant} · {fmtBytes(m.size_bytes)}{m.moe ? " · MoE" : ""}
                {m.mmproj ? <> · <Icon name="eye" size={11} /></> : null}
              </div>
            </div>
            <select
              class="mini"
              value={m.engine}
              onChange={(e) => void setOverride(m.id, (e.target as HTMLSelectElement).value)}
              title="Engine that runs this model"
              aria-label="Engine"
            >
              <option value="polystrata" disabled={!m.profile}>PolyStrata</option>
              <option value="llama">llama.cpp</option>
            </select>
            {active ? (
              <button class="icon-btn danger" title="Unload" aria-label="Unload" onClick={() => void unloadModel()}><Icon name="stop" /></button>
            ) : (
              <button class="icon-btn primary" title="Load" aria-label="Load" disabled={st.state === "loading"} onClick={() => void loadModel(m.id, m.engine)}><Icon name="play" /></button>
            )}
          </div>
        );
      })}
      <details class="fold">
        <summary>Remote endpoints</summary>
        <RemoteEndpoints />
      </details>
    </div>
  );
}

/* ---------------- MCP ---------------- */

export function McpPanel() {
  const [text, setText] = useState("");
  const [msg, setMsg] = useState("");
  const [editing, setEditing] = useState(false);
  useEffect(() => { void api.mcpGetConfig().then(setText).catch((e) => setMsg(String(e))); }, []);
  const jsonErr = (() => {
    if (!editing || !text.trim()) return "";
    try {
      const v = JSON.parse(text) as { mcpServers?: unknown };
      return v && typeof v.mcpServers === "object" && v.mcpServers !== null ? "" : 'The config needs an "mcpServers" object.';
    } catch (e) {
      return `Invalid JSON: ${(e as Error).message}`;
    }
  })();

  const save = async () => {
    try {
      await api.mcpSaveConfig(text);
      setMsg("Saved. Reconnecting…");
      setEditing(false);
    } catch (e) {
      setMsg(String(e));
    }
  };
  const example = `{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "D:\\\\some\\\\folder"]
    },
    "remote": { "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer ..." } }
  }
}`;
  return (
    <div class="panel-body">
      <div class="row">
        <button onClick={() => setEditing(!editing)}>{editing ? "Close editor" : "Edit config"}</button>
        <button onClick={() => void api.mcpReload().then(refreshMcp)}>Reconnect</button>
      </div>
      {editing && (
        <>
          <textarea class="code-edit" aria-label="MCP config (JSON)" aria-invalid={!!jsonErr} value={text} spellcheck={false} onInput={(e) => setText((e.target as HTMLTextAreaElement).value)} />
          {jsonErr && <div class="error small" role="alert">{jsonErr}</div>}
          <div class="row">
            <button class="primary" disabled={!!jsonErr} onClick={() => void save()}>Save</button>
            <button onClick={() => setText(example)}>Insert example</button>
          </div>
        </>
      )}
      {msg && <div class="hint">{msg}</div>}
      {mcp.value.length === 0 && <div class="hint">No MCP servers configured. Use the standard <code>mcpServers</code> format (same as Claude Desktop / Cursor).</div>}
      {mcp.value.map((s) => (
        <div class="model" key={s.name}>
          <div class="row">
            <div class="model-title grow">{s.name}</div>
            <span class={`pill ${s.state === "ready" ? "ready" : s.state === "error" ? "error" : "loading"}`}>{s.state}</span>
          </div>
          <div class="model-meta">{s.transport}{s.state === "ready" ? ` · ${s.tools.length} tools` : ""}</div>
          {s.error && <div class="error small">{s.error}</div>}
          {s.tools.length > 0 && (
            <details>
              <summary>Tools</summary>
              <ul class="toollist">{s.tools.map((t) => <li key={t.name} title={t.description}><b>{t.name}</b> {t.description.slice(0, 80)}</li>)}</ul>
            </details>
          )}
        </div>
      ))}
    </div>
  );
}
