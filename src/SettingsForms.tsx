import { useState } from "preact/hooks";
import type { Settings } from "./api";
import { changedFields, refreshModels, saveSettings, settings } from "./state";

const lines = (v: string) => v.split("\n").map((x) => x.trim()).filter(Boolean);

/**
 * Edit a group of settings inline with a Save bar. List fields are edited as raw text (so you can
 * type newlines) and split into lines on save. Only fields changed here are saved.
 */
function useForm() {
  const [orig, setOrig] = useState<Settings>(() => ({ ...(settings.value as Settings) }));
  const [s, setS] = useState<Settings>(() => ({ ...orig }));
  const [raw, setRaw] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);
  const set = <K extends keyof Settings>(k: K, v: Settings[K]) => setS({ ...s, [k]: v });
  const listField = (k: "model_dirs" | "shell_allow" | "shell_deny") => ({
    value: raw[k] ?? (s[k] ?? []).join("\n"),
    onInput: (e: Event) => setRaw({ ...raw, [k]: (e.target as HTMLTextAreaElement).value }),
  });
  const current = (): Settings => {
    const n = { ...s };
    for (const [k, v] of Object.entries(raw)) (n as unknown as Record<string, string[]>)[k] = lines(v);
    return n;
  };
  const patch = changedFields(orig, current());
  const dirty = Object.keys(patch).length > 0;
  const save = async () => {
    setSaving(true);
    const ok = await saveSettings(patch);
    setSaving(false);
    if (!ok) return;
    if ("model_dirs" in patch) await refreshModels();
    const n = current();
    setOrig(n);
    setS(n);
    setRaw({});
  };
  const reset = () => {
    setS({ ...orig });
    setRaw({});
  };
  return { s, set, listField, dirty, save, saving, reset };
}

function SaveBar({ dirty, saving, invalid, onSave, onReset }: { dirty: boolean; saving: boolean; invalid?: string; onSave: () => void; onReset: () => void }) {
  if (!dirty && !invalid) return null;
  return (
    <div class="save-bar">
      {invalid ? <span class="hint" role="alert">{invalid}</span> : <span class="hint">Unsaved changes</span>}
      <span class="spacer" />
      <button disabled={saving} onClick={onReset}>Discard</button>
      <button class="primary" disabled={!dirty || !!invalid || saving} onClick={onSave}>{saving ? "Saving…" : "Save"}</button>
    </div>
  );
}

export function AgentSettings() {
  const { s, set, listField, dirty, save, saving, reset } = useForm();
  const invalid =
    !Number.isFinite(s.temperature) || s.temperature < 0 || s.temperature > 2 ? "Temperature must be between 0 and 2." :
    !Number.isInteger(s.max_turns) || s.max_turns < 1 || s.max_turns > 500 ? "Max tool turns must be a whole number from 1 to 500." :
    "";
  return (
    <div class="settings-form">
      <div class="section-title">Engines</div>
      <label>Model folders (one per line)
        <textarea rows={3} placeholder={"D:\\models"} {...listField("model_dirs")} />
      </label>
      <label>llama-server.exe
        <input value={s.llama_server} placeholder="C:\\llama.cpp\\llama-server.exe" onInput={(e) => set("llama_server", (e.target as HTMLInputElement).value)} />
      </label>
      <label>PolyStrata folder (optional)
        <input value={s.polystrata_dir} placeholder="D:\\PolyStrata" onInput={(e) => set("polystrata_dir", (e.target as HTMLInputElement).value)} />
      </label>

      <div class="section-title">Agent</div>
      <div class="grid2">
        <label>Temperature
          <input type="number" step={0.1} min={0} max={2} value={s.temperature} onInput={(e) => set("temperature", Number((e.target as HTMLInputElement).value))} />
        </label>
        <label>Max tool turns
          <input type="number" min={1} max={500} value={s.max_turns} onInput={(e) => set("max_turns", Number((e.target as HTMLInputElement).value))} />
        </label>
      </div>
      <label>System prompt
        <textarea rows={5} value={s.system_prompt} onInput={(e) => set("system_prompt", (e.target as HTMLTextAreaElement).value)} />
      </label>
      <div class="hint">Project instructions (AGENTS.md, CLAUDE.md, .cursorrules…) in the workspace are added automatically.</div>
      <SaveBar dirty={dirty} saving={saving} invalid={invalid} onSave={() => void save()} onReset={reset} />
    </div>
  );
}

export function PermissionSettings() {
  const { s, set, listField, dirty, save, saving, reset } = useForm();
  return (
    <div class="settings-form">
      <label class="toggle">
        <input type="checkbox" checked={s.auto_approve} onChange={(e) => set("auto_approve", (e.target as HTMLInputElement).checked)} />
        Auto-approve file changes, shell commands and MCP tools (no prompts)
      </label>
      {s.auto_approve && <div class="hint">File changes can still be undone with Restore checkpoint; shell commands cannot. Dangerous commands still ask.</div>}

      <div class="section-title">Shell commands</div>
      <label>Run without asking (one prefix per line)
        <textarea rows={4} class="code-edit-sm" placeholder={"git status\nnpm test"} {...listField("shell_allow")} />
      </label>
      <label>Always refuse (one per line, matched anywhere in the command)
        <textarea rows={3} class="code-edit-sm" placeholder={"git push\nInvoke-WebRequest"} {...listField("shell_deny")} />
      </label>
      <div class="hint">Commands that chain others (<code>;</code> <code>&&</code> <code>|</code> <code>&gt;</code>) always ask, even when their start is on the list.</div>

      <div class="section-title">Trusted tools</div>
      {(s.trusted ?? []).length === 0 ? (
        <div class="hint">None. Choose "Always allow … in this workspace" on an approval prompt to add one.</div>
      ) : (
        <div class="chiplist">
          {s.trusted.map((t) => (
            <span class="chip" key={t}>
              {t.replace("@", " in ")}
              <button class="x" aria-label={`Stop trusting ${t}`} onClick={() => set("trusted", s.trusted.filter((x) => x !== t))}>×</button>
            </span>
          ))}
        </div>
      )}
      <SaveBar dirty={dirty} saving={saving} onSave={() => void save()} onReset={reset} />
    </div>
  );
}
