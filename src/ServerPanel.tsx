import { useState } from "preact/hooks";
import { api } from "./api";
import type { Provider, Settings } from "./api";
import { engine, loadError, loadModel, saveSettings, settings, unloadModel } from "./state";

const copy = (t: string) => void navigator.clipboard.writeText(t);

/* ---------------- Server tab: hosting options ---------------- */

export function ServerPanel() {
  const cur = settings.value;
  const [s, setS] = useState<Settings | null>(cur ? { ...cur } : null);
  const [saved, setSaved] = useState(false);
  if (!s || !cur) return <div class="panel-body hint">Loading…</div>;
  const set = <K extends keyof Settings>(k: K, v: Settings[K]) => {
    setS({ ...s, [k]: v });
    setSaved(false);
  };
  const num = (e: Event) => Number((e.target as HTMLInputElement).value);
  const txt = (e: Event) => (e.target as HTMLInputElement | HTMLSelectElement).value;

  const st = engine.value;
  const local = st.state === "ready" && st.engine !== "openai";
  const host = cur.serve_on_network ? "<this-pc-ip>" : "127.0.0.1";
  const url = local ? `http://${host}:${st.port}/v1` : "";
  const curl = local
    ? `curl ${url}/chat/completions${cur.api_key ? ` -H "Authorization: Bearer ${cur.api_key}"` : ""} -H "Content-Type: application/json" -d "{\\"model\\":\\"${st.engine === "polystrata" ? "strata" : st.model_name}\\",\\"messages\\":[{\\"role\\":\\"user\\",\\"content\\":\\"hi\\"}]}"`
    : "";

  return (
    <div class="panel-body">
      <div class="section-title">OpenAI-compatible server</div>
      {local ? (
        <div class="endpoint">
          <div class="row">
            <code class="grow ellipsis" title={url}>{url}</code>
            <button onClick={() => copy(url)}>Copy</button>
          </div>
          <div class="hint">Running {st.model_name} on {st.engine === "polystrata" ? "PolyStrata" : "llama.cpp"}. Use any OpenAI client with this base URL{cur.api_key ? " and your API key" : ""}.</div>
          <div class="row"><button onClick={() => copy(curl)}>Copy curl example</button></div>
        </div>
      ) : st.state === "ready" ? (
        <div class="hint">A remote endpoint is active, so nothing is being hosted. Load a local model to host it.</div>
      ) : (
        <div class="hint">Load a model (Models tab) and its OpenAI-compatible server starts on the port below.</div>
      )}

      <label class="toggle">
        <input type="checkbox" checked={s.serve_on_network} onChange={(e) => set("serve_on_network", (e.target as HTMLInputElement).checked)} />
        Serve on the local network (0.0.0.0)
      </label>
      {s.serve_on_network && !s.api_key && <div class="error small">Anyone on your network could use this model. Set an API key.</div>}
      <label>API key (optional, required on /v1/* when set)
        <input type="text" placeholder="none" value={s.api_key} onInput={(e) => set("api_key", txt(e))} />
      </label>
      <div class="grid2">
        <label>PolyStrata port
          <input type="number" min={1} max={65535} value={s.poly_port} onInput={(e) => set("poly_port", num(e))} />
        </label>
        <label>llama.cpp port
          <input type="number" min={1} max={65535} value={s.llama_port} onInput={(e) => set("llama_port", num(e))} />
        </label>
      </div>

      <div class="section-title">llama.cpp options</div>
      <div class="grid2">
        <label>Context length
          <input type="number" min={1024} step={1024} value={s.ctx} onInput={(e) => set("ctx", num(e))} />
        </label>
        <label>GPU layers (-1 = auto fit)
          <input type="number" min={-1} value={s.gpu_layers} onInput={(e) => set("gpu_layers", num(e))} />
        </label>
        <label>Flash attention
          <select value={s.flash_attn} onChange={(e) => set("flash_attn", txt(e) as Settings["flash_attn"])}>
            <option value="auto">auto</option><option value="on">on</option><option value="off">off</option>
          </select>
        </label>
        <label>Parallel slots (0 = default)
          <input type="number" min={0} value={s.parallel} onInput={(e) => set("parallel", num(e))} />
        </label>
        <label>KV cache K
          <select value={s.cache_type_k} onChange={(e) => set("cache_type_k", txt(e))}>
            {["", "f16", "bf16", "q8_0", "q5_0", "q4_0"].map((t) => <option value={t}>{t || "default"}</option>)}
          </select>
        </label>
        <label>KV cache V
          <select value={s.cache_type_v} onChange={(e) => set("cache_type_v", txt(e))}>
            {["", "f16", "bf16", "q8_0", "q5_0", "q4_0"].map((t) => <option value={t}>{t || "default"}</option>)}
          </select>
        </label>
      </div>
      <label>Extra llama-server arguments
        <input placeholder="--n-cpu-moe 20 --threads 8" value={s.llama_extra_args} onInput={(e) => set("llama_extra_args", txt(e))} />
      </label>
      <div class="hint">PolyStrata models use their own profile for context and offload; only host, port and API key apply to them.</div>
      <div class="row">
        <button class="primary" onClick={() => void saveSettings(s).then(() => setSaved(true))}>Save</button>
        {saved && <span class="hint">Saved. Applies the next time a model is loaded.</span>}
      </div>
    </div>
  );
}

/* ---------------- Remote OpenAI-compatible endpoints (shown in the Models tab) ---------------- */

const PRESETS: { name: string; base_url: string }[] = [
  { name: "OpenAI", base_url: "https://api.openai.com/v1" },
  { name: "OpenRouter", base_url: "https://openrouter.ai/api/v1" },
  { name: "LM Studio (another PC)", base_url: "http://192.168.1.10:1234/v1" },
  { name: "Ollama", base_url: "http://127.0.0.1:11434/v1" },
];

export function RemoteEndpoints() {
  const list = settings.value?.providers ?? [];
  const [edit, setEdit] = useState<Provider | null>(null);
  const [ids, setIds] = useState<string[]>([]);
  const [msg, setMsg] = useState("");
  const st = engine.value;

  const persist = async (providers: Provider[]) => {
    const s = settings.value;
    if (s) await saveSettings({ ...s, providers });
  };
  const save = async () => {
    if (!edit) return;
    const p = { ...edit, name: edit.name.trim() || "Endpoint", base_url: edit.base_url.trim() };
    if (!p.base_url) return setMsg("Enter a base URL.");
    await persist(list.some((x) => x.id === p.id) ? list.map((x) => (x.id === p.id ? p : x)) : [...list, p]);
    setEdit(null);
    setMsg("");
  };
  const fetchModels = async () => {
    if (!edit) return;
    setMsg("Connecting…");
    try {
      const m = await api.providerModels(edit.base_url, edit.api_key);
      setIds(m);
      setMsg(`Connected. ${m.length} model${m.length === 1 ? "" : "s"} available.`);
      if (!edit.model && m.length) setEdit({ ...edit, model: m[0] });
    } catch (e) {
      setIds([]);
      setMsg(String(e));
    }
  };

  return (
    <div>
      {list.length === 0 && !edit && <div class="hint">Use OpenAI, OpenRouter, Ollama, or another LM Studio / llama.cpp server instead of a local model.</div>}
      {list.map((p) => {
        const active = st.model_id === `remote:${p.id}` && st.state === "ready";
        return (
          <div class={`model ${active ? "active" : ""}`} key={p.id}>
            <div class="model-title">{p.name}</div>
            <div class="model-meta ellipsis" title={p.base_url}>{p.base_url} · {p.model || "no model chosen"}</div>
            <div class="row">
              <button onClick={() => { setEdit({ ...p }); setIds([]); setMsg(""); }}>Edit</button>
              <button class="danger" onClick={() => void persist(list.filter((x) => x.id !== p.id))}>Delete</button>
              <span class="spacer" />
              {active ? (
                <button class="danger" onClick={() => void unloadModel()}>Disconnect</button>
              ) : (
                <button class="primary" disabled={st.state === "loading"} onClick={() => void loadModel(`remote:${p.id}`)}>Use</button>
              )}
            </div>
          </div>
        );
      })}
      {loadError.value && loadError.value.includes("endpoint") && <div class="error">{loadError.value}</div>}
      {edit ? (
        <div class="model">
          <div class="row">
            {PRESETS.map((p) => (
              <button key={p.name} class="chip" onClick={() => setEdit({ ...edit, name: edit.name || p.name, base_url: p.base_url })}>{p.name}</button>
            ))}
          </div>
          <label>Name
            <input value={edit.name} onInput={(e) => setEdit({ ...edit, name: (e.target as HTMLInputElement).value })} />
          </label>
          <label>Base URL
            <input placeholder="https://api.openai.com/v1" value={edit.base_url} onInput={(e) => setEdit({ ...edit, base_url: (e.target as HTMLInputElement).value })} />
          </label>
          <label>API key
            <input type="password" placeholder="sk-…  (blank for none)" value={edit.api_key} onInput={(e) => setEdit({ ...edit, api_key: (e.target as HTMLInputElement).value })} />
          </label>
          <label>Model
            <input list="remote-models" placeholder="gpt-4o-mini" value={edit.model} onInput={(e) => setEdit({ ...edit, model: (e.target as HTMLInputElement).value })} />
            <datalist id="remote-models">{ids.map((i) => <option value={i} />)}</datalist>
          </label>
          {msg && <div class={msg.startsWith("Connected") ? "hint" : "error small"}>{msg}</div>}
          <div class="row">
            <button onClick={() => void fetchModels()}>Test / fetch models</button>
            <span class="spacer" />
            <button onClick={() => { setEdit(null); setMsg(""); }}>Cancel</button>
            <button class="primary" onClick={() => void save()}>Save</button>
          </div>
          <div class="hint">The key is stored in plain text in settings.json on this PC.</div>
        </div>
      ) : (
        <button class="wide" onClick={() => { setEdit({ id: crypto.randomUUID().replace(/-/g, "").slice(0, 12), name: "", base_url: "", api_key: "", model: "" }); setIds([]); setMsg(""); }}>
          + Add endpoint
        </button>
      )}
    </div>
  );
}
