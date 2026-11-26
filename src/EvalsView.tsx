import { useEffect, useState } from "preact/hooks";
import { signal } from "@preact/signals";
import { api, onEvalProgress } from "./api";
import type { EvalBatch, EvalResult, EvalScenario } from "./api";
import { Icon } from "./icons";
import { engine, errText, toast } from "./state";

/** live state survives switching tabs while an eval runs */
const running = signal(false);
const live = signal<Record<string, "running" | EvalResult>>({});
let listening = false;

function ensureListener() {
  if (listening) return;
  listening = true;
  void onEvalProgress((p) => {
    live.value = { ...live.value, [p.scenario]: p.status === "running" ? "running" : (p.result as EvalResult) };
  });
}

function secs(ms: number) {
  return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)} s`;
}

function avg(xs: number[]) {
  return xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : 0;
}

export function EvalsView() {
  const [scenarios, setScenarios] = useState<EvalScenario[]>([]);
  const [history, setHistory] = useState<EvalBatch[]>([]);
  const [err, setErr] = useState("");
  const ready = engine.value.state === "ready";

  const load = () => {
    api.evalList().then(setScenarios).catch((e) => setErr(errText(e)));
    api.evalResults(20).then(setHistory).catch(() => {});
  };
  useEffect(() => {
    ensureListener();
    load();
  }, []);

  const run = async () => {
    running.value = true;
    live.value = {};
    setErr("");
    try {
      const b = await api.evalRun();
      toast(`Evals: ${b.passed}/${b.total} passed on ${b.model}`, b.passed === b.total ? "success" : "info");
    } catch (e) {
      setErr(errText(e));
    } finally {
      running.value = false;
      load();
    }
  };

  return (
    <div class="runs evals">
      <div class="hint">
        Six small agent tasks run on the loaded model in a scratch folder (your workspace is not touched). Use them to compare models,
        quantizations and settings for editing reliability and speed.
      </div>
      <div class="log-toolbar">
        {running.value ? (
          <button class="danger" onClick={() => void api.evalCancel()}>Stop</button>
        ) : (
          <button class="primary" disabled={!ready} title={ready ? "" : "Load a model first"} onClick={() => void run()}>
            <Icon name="play" size={12} /> Run evals
          </button>
        )}
        <span class="spacer" />
        <button class="icon-btn" title="Refresh" aria-label="Refresh results" onClick={load}><Icon name="refresh" size={14} /></button>
      </div>
      {err && <div class="error small">{err}</div>}

      <ul class="filelist eval-list">
        {scenarios.map((s) => {
          const st = live.value[s.id];
          const r = typeof st === "object" ? st : null;
          return (
            <li key={s.id} title={s.prompt}>
              <span class={`dot ${st === "running" ? "loading" : r ? (r.passed ? "ready" : "error") : ""}`} />
              <span class="grow eval-main">
                <span class="ellipsis">{s.name}</span>
                {r && (
                  <span class="run-sub">
                    {secs(r.ms)} · {r.turns} turns
                    {r.edit_failures ? ` · ${r.edit_failures} edit misses` : ""}
                    {r.rewrites ? ` · ${r.rewrites} rewrites` : ""}
                    {r.recovered ? ` · ${r.recovered} recovered calls` : ""}
                    {!r.passed && r.detail ? ` · ${r.detail}` : ""}
                  </span>
                )}
              </span>
              <span class="hint">{st === "running" ? "running…" : r ? (r.passed ? "pass" : "fail") : ""}</span>
            </li>
          );
        })}
      </ul>

      {history.length > 0 && (
        <>
          <div class="section-title">History</div>
          <table class="eval-table">
            <thead>
              <tr><th>Model</th><th>Pass</th><th>Avg time</th><th>Edit misses</th><th>Rewrites</th></tr>
            </thead>
            <tbody>
              {history.map((b) => (
                <tr key={b.id} title={new Date(b.started).toLocaleString()}>
                  <td class="ellipsis" style={{ maxWidth: 120 }}>{b.model}{b.cancelled ? " (stopped)" : ""}</td>
                  <td class={b.passed === b.total ? "ok" : ""}>{b.passed}/{b.total}</td>
                  <td>{secs(Math.round(avg(b.results.map((r) => r.ms))))}</td>
                  <td>{b.results.reduce((a, r) => a + r.edit_failures, 0)}</td>
                  <td>{b.results.reduce((a, r) => a + r.rewrites, 0)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
    </div>
  );
}
