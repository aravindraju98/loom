import { useState } from "preact/hooks";
import type { Settings } from "./api";
import { changedFields, refreshModels, saveSettings, settings, settingsOpen } from "./state";
import { Modal } from "./ui/Modal";

export function SettingsModal() {
  // the values when the dialog opened: only fields edited here are saved
  const [orig] = useState<Settings>(() => ({ ...(settings.value as Settings) }));
  const [s, setS] = useState<Settings>(() => ({ ...orig }));
  const [saving, setSaving] = useState(false);
  const set = <K extends keyof Settings>(k: K, v: Settings[K]) => setS({ ...s, [k]: v });
  const close = () => (settingsOpen.value = false);
  const patch = changedFields(orig, s);
  const dirty = Object.keys(patch).length > 0;
  const invalid =
    !Number.isFinite(s.temperature) || s.temperature < 0 || s.temperature > 2 ? "Temperature must be between 0 and 2." :
    !Number.isInteger(s.max_turns) || s.max_turns < 1 || s.max_turns > 500 ? "Max tool turns must be a whole number from 1 to 500." :
    "";
  const save = async () => {
    if (invalid) return;
    setSaving(true);
    const ok = await saveSettings(patch);
    setSaving(false);
    if (!ok) return;
    if ("model_dirs" in patch) await refreshModels();
    close();
  };
  return (
    <Modal
      title="Settings"
      onClose={close}
      footer={
        <>
          {invalid && <span class="hint" role="alert">{invalid}</span>}
          <span class="spacer" />
          <button onClick={close}>Cancel</button>
          <button class="primary" disabled={!dirty || !!invalid || saving} onClick={() => void save()}>{saving ? "Saving…" : "Save"}</button>
        </>
      }
    >
      <div class="section-title">Engines</div>
      <label>Model folders (one per line)
        <textarea rows={3} value={s.model_dirs.join("\n")} placeholder={"D:\\models"} onInput={(e) => set("model_dirs", (e.target as HTMLTextAreaElement).value.split("\n").map((x) => x.trim()).filter(Boolean))} />
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
        <textarea rows={4} value={s.system_prompt} onInput={(e) => set("system_prompt", (e.target as HTMLTextAreaElement).value)} />
      </label>
      <label class="toggle">
        <input type="checkbox" checked={s.auto_approve} onChange={(e) => set("auto_approve", (e.target as HTMLInputElement).checked)} />
        Auto-approve file changes, shell commands and MCP tools (no prompts)
      </label>
      {s.auto_approve && <div class="hint">File changes can still be undone with Restore checkpoint; shell commands cannot.</div>}
    </Modal>
  );
}
