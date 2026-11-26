import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { save } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import type { LogEntry, RunSummary, TraceEvent } from "./api";
import { Icon } from "./icons";
import { EvalsView } from "./EvalsView";
import { chat, errText, inspectRun, logs, openChat, toast, unseenErrors } from "./state";

const LEVELS = ["info", "warn", "error"] as const;
const RANK: Record<string, number> = { debug: 0, info: 1, warn: 2, error: 3 };

function clock(ts: number): string {
  const d = new Date(ts);
  const p = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`;
}

function when(ts: number): string {
  const d = new Date(ts);
  const today = new Date();
  const same = d.toDateString() === today.toDateString();
  return same ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : d.toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

function dur(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  return `${Math.floor(ms / 60_000)}m ${Math.round((ms % 60_000) / 1000)}s`;
}

function k(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n);
}

/* ---------------- live log ---------------- */

function LiveLog() {
  const [level, setLevel] = useState<string>("info");
  const [source, setSource] = useState("all");
  const [q, setQ] = useState("");
  const [paused, setPaused] = useState<LogEntry[] | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const all = paused ?? logs.value;

  const sources = useMemo(() => {
    const s = new Set<string>();
    for (const e of logs.value) s.add(e.source.startsWith("mcp:") ? "mcp" : e.source);
    return ["all", ...Array.from(s).sort()];
  }, [logs.value.length]);

  const rows = useMemo(() => {
    const ql = q.trim().toLowerCase();
    const min = RANK[level] ?? 1;
    const out = all.filter(
      (e) =>
        (RANK[e.level] ?? 1) >= min &&
        (source === "all" || e.source === source || (source === "mcp" && e.source.startsWith("mcp:"))) &&
        (!ql || e.msg.toLowerCase().includes(ql) || e.source.toLowerCase().includes(ql)),
    );
    return out.slice(-600);
  }, [all, level, source, q]);

  // follow the tail unless the user scrolled up
  useEffect(() => {
    const el = box.current;
    if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 80) el.scrollTop = el.scrollHeight;
  }, [rows.length]);

  const exportLog = async () => {
    try {
      const dest = await save({ defaultPath: `harness-log-${new Date().toISOString().slice(0, 10)}.jsonl`, filters: [{ name: "JSON lines", extensions: ["jsonl"] }] });
      if (!dest) return;
      const n = await api.logsExport(dest);
      toast(`Exported ${n} log entries`, "success");
    } catch (e) {
      toast(`Export failed: ${errText(e)}`, "error");
    }
  };

  return (
    <div class="logs">
      <div class="log-toolbar">
        <select class="mini" aria-label="Minimum level" value={level} onChange={(e) => setLevel((e.target as HTMLSelectElement).value)}>
          {LEVELS.map((l) => <option value={l}>{l === "info" ? "All levels" : l === "warn" ? "Warnings+" : "Errors"}</option>)}
        </select>
        <select class="mini" aria-label="Source" value={source} onChange={(e) => setSource((e.target as HTMLSelectElement).value)}>
          {sources.map((s) => <option value={s}>{s === "all" ? "All sources" : s}</option>)}
        </select>
        <span class="spacer" />
        <button class={`icon-btn ${paused ? "on" : ""}`} title={paused ? "Resume" : "Pause"} aria-label={paused ? "Resume live log" : "Pause live log"} aria-pressed={!!paused} onClick={() => setPaused(paused ? null : logs.value)}>
          <Icon name={paused ? "play" : "pause"} size={14} />
        </button>
        <button class="icon-btn" title="Export" aria-label="Export log" onClick={() => void exportLog()}><Icon name="save" size={14} /></button>
        <button class="icon-btn" title="Open log folder" aria-label="Open log folder" onClick={() => void api.logsOpenDir().catch((e) => toast(errText(e), "error"))}><Icon name="folder" size={14} /></button>
        <button class="icon-btn" title="Clear view" aria-label="Clear log view" onClick={() => { void api.logsClear(); logs.value = []; }}><Icon name="trash" size={14} /></button>
      </div>
      <div class="searchbar log-search">
        <Icon name="search" size={13} />
        <input placeholder="Filter…" aria-label="Filter log" value={q} onInput={(e) => setQ((e.target as HTMLInputElement).value)} />
      </div>
      <div class="log-list" ref={box} role="log" aria-live="off">
        {rows.length === 0 && <div class="hint log-empty">{all.length === 0 ? "Nothing logged yet." : "No entries match the filter."}</div>}
        {rows.map((e) => (
          <div key={e.seq} class={`log-row lv-${e.level}`}>
            <span class="log-time">{clock(e.ts)}</span>
            <span class="log-src" title={e.source}>{e.source}</span>
            <span class="log-msg">
              {e.msg}
              {e.run_id && (
                <button class="log-link" onClick={() => (inspectRun.value = e.run_id!)} title="Open this run">run</button>
              )}
            </span>
          </div>
        ))}
      </div>
      {paused && <div class="hint small">Paused · {logs.value.length - paused.length} new entries</div>}
    </div>
  );
}

/* ---------------- runs ---------------- */

function RunList() {
  const [runs, setRuns] = useState<RunSummary[] | null>(null);
  const [onlyChat, setOnlyChat] = useState(false);
  const [err, setErr] = useState("");
  const load = () => {
    setErr("");
    api.traceList(100, onlyChat ? chat.value.id : undefined).then(setRuns).catch((e) => setErr(errText(e)));
  };
  useEffect(load, [onlyChat, chat.value.id]);
  return (
    <div class="runs">
      <div class="log-toolbar">
        <label class="toggle small-toggle">
          <input type="checkbox" checked={onlyChat} onChange={(e) => setOnlyChat((e.target as HTMLInputElement).checked)} />
          This chat only
        </label>
        <span class="spacer" />
        <button class="icon-btn" title="Refresh" aria-label="Refresh runs" onClick={load}><Icon name="refresh" size={14} /></button>
      </div>
      {err && <div class="error small">{err}</div>}
      {!runs && !err && <div class="hint">Loading…</div>}
      {runs && runs.length === 0 && <div class="hint">No runs recorded yet. Every message you send is traced here.</div>}
      <div class="run-list">
        {runs?.map((r) => {
          const cache = r.prompt_tokens > 0 ? Math.round((r.cached_tokens / r.prompt_tokens) * 100) : null;
          const bad = r.reason === "error";
          return (
            <button key={r.run_id} class="run-item" onClick={() => (inspectRun.value = r.run_id)}>
              <span class={`dot ${bad ? "error" : r.reason === "done" ? "ready" : "loading"}`} />
              <span class="grow run-main">
                <span class="run-title ellipsis">{r.model ?? "unknown model"}</span>
                <span class="run-sub">
                  {when(r.started)} · {dur(r.ms)} · {r.turns} turn{r.turns === 1 ? "" : "s"} · {r.tool_calls} tool{r.tool_calls === 1 ? "" : "s"}
                  {r.tool_errors ? ` (${r.tool_errors} failed)` : ""}
                  {cache != null ? ` · ${cache}% cached` : ""}
                </span>
              </span>
              <span class={`run-reason ${bad ? "bad" : ""}`}>{r.reason}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div class="stat">
      <div class="stat-value">{value}</div>
      <div class="stat-label">{label}</div>
    </div>
  );
}

function RunInspector({ id }: { id: string }) {
  const [data, setData] = useState<{ summary: RunSummary; events: TraceEvent[] } | null>(null);
  const [err, setErr] = useState("");
  useEffect(() => {
    setData(null);
    setErr("");
    api.traceGet(id).then(setData).catch((e) => setErr(errText(e)));
  }, [id]);
  const back = () => (inspectRun.value = null);
  if (err) return <div class="runs"><button class="chip-btn" onClick={back}><Icon name="back" size={12} /> Runs</button><div class="error small">{err}</div></div>;
  if (!data) return <div class="hint">Loading run…</div>;
  const s = data.summary;
  const total = Math.max(s.ms, 1);
  const cache = s.prompt_tokens > 0 ? `${Math.round((s.cached_tokens / s.prompt_tokens) * 100)}%` : "–";
  let turn = 0;
  return (
    <div class="inspector">
      <div class="log-toolbar">
        <button class="chip-btn" onClick={back}><Icon name="back" size={12} /> Runs</button>
        <span class="spacer" />
        {s.chat_id && s.chat_id !== chat.value.id && <button class="chip-btn" onClick={() => void openChat(s.chat_id)}>Open chat</button>}
      </div>
      <div class="insp-head">
        <div class="run-title">{s.model ?? "unknown model"}</div>
        <div class="run-sub">{new Date(s.started).toLocaleString()} · {s.engine}{s.ctx ? ` · ctx ${k(s.ctx)}` : ""}{s.tier ? ` · ${s.tier} tier` : ""}</div>
        {s.error && <div class="error small">{s.error}</div>}
      </div>
      <div class="stats">
        <Stat label="duration" value={dur(s.ms)} />
        <Stat label="turns" value={String(s.turns)} />
        <Stat label="prompt" value={k(s.prompt_tokens)} />
        <Stat label="cached" value={cache} />
        <Stat label="output" value={k(s.completion_tokens)} />
        <Stat label="prefill" value={dur(s.prompt_ms)} />
      </div>
      <div class="section-title">Timeline</div>
      <ol class="timeline">
        {data.events.map((e, i) => {
          const left = `${(e.t / total) * 100}%`;
          if (e.kind === "turn") {
            turn++;
            const pt = Number(e.prompt_tokens ?? 0);
            const cn = Number(e.cache_n ?? 0);
            const secs = Number(e.secs ?? 0);
            const width = `${Math.max(((secs * 1000) / total) * 100, 0.8)}%`;
            const start = `${Math.max(((e.t - secs * 1000) / total) * 100, 0)}%`;
            return (
              <li key={i} class="tl-item tl-turn">
                <div class="tl-line">
                  <b>Turn {turn}</b>
                  <span class="tl-meta">
                    {dur(secs * 1000)}
                    {e.ttft_ms != null ? ` · first token ${dur(Number(e.ttft_ms))}` : ""}
                    {pt ? ` · prompt ${k(pt)}${pt ? ` (${Math.round((cn / pt) * 100)}% cached)` : ""}` : ""}
                    {e.prompt_ms != null ? ` · prefill ${dur(Number(e.prompt_ms))}` : ""}
                    {` · ${Number(e.tokens ?? 0)} tok at ${Number(e.tok_s ?? 0).toFixed(1)}/s`}
                    {e.finish === "length" ? " · cut off" : ""}
                  </span>
                </div>
                <div class="tl-bar"><i style={{ left: start, width }} /></div>
              </li>
            );
          }
          if (e.kind === "tool") {
            const ms = Number(e.ms ?? 0);
            const width = `${Math.max((ms / total) * 100, 0.8)}%`;
            const start = `${Math.max(((e.t - ms) / total) * 100, 0)}%`;
            const ch = e.change as { path?: string; added?: number; removed?: number } | null;
            return (
              <li key={i} class={`tl-item tl-tool ${e.ok === false ? "bad" : ""}`}>
                <div class="tl-line">
                  <code>{String(e.name)}</code>
                  <span class="tl-meta">
                    {dur(ms)}
                    {ch?.path ? ` · ${ch.path} +${ch.added ?? 0} −${ch.removed ?? 0}` : ""}
                    {e.ok === false ? " · failed" : ""}
                  </span>
                </div>
                <div class="tl-bar"><i style={{ left: start, width }} /></div>
                {e.error != null && <div class="tl-err">{String(e.error)}</div>}
              </li>
            );
          }
          return (
            <li key={i} class="tl-item tl-note" style={{ "--at": left } as Record<string, string>}>
              <div class="tl-line"><span class="tl-meta">{e.kind === "retry" ? `Retry ${String(e.attempt)}: ${String(e.reason)}` : e.kind === "prune" ? `Context trimmed: ${String(e.removed)} old tool outputs removed` : e.kind === "recovered_text_calls" ? "Recovered tool call written as text" : e.kind}</span></div>
            </li>
          );
        })}
        {data.events.length === 0 && <li class="hint">No events.</li>}
      </ol>
    </div>
  );
}

export function LogsPanel() {
  const [view, setView] = useState<"live" | "runs" | "evals">("live");
  useEffect(() => {
    unseenErrors.value = 0;
  }, []);
  useEffect(() => {
    if (inspectRun.value) setView("runs");
  }, [inspectRun.value]);
  return (
    <div class="panel-body logs-panel">
      <div class="segmented" role="tablist" aria-label="Logs view">
        <button role="tab" aria-selected={view === "live"} class={view === "live" ? "on" : ""} onClick={() => { setView("live"); inspectRun.value = null; }}>Live log</button>
        <button role="tab" aria-selected={view === "runs"} class={view === "runs" ? "on" : ""} onClick={() => setView("runs")}>Runs</button>
        <button role="tab" aria-selected={view === "evals"} class={view === "evals" ? "on" : ""} onClick={() => { setView("evals"); inspectRun.value = null; }}>Evals</button>
      </div>
      {view === "live" ? <LiveLog /> : view === "evals" ? <EvalsView /> : inspectRun.value ? <RunInspector id={inspectRun.value} /> : <RunList />}
    </div>
  );
}
