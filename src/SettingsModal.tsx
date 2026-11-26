import { useState } from "preact/hooks";
import type { Settings } from "./api";
import { refreshModels, saveSettings, settings, settingsOpen } from "./state";

export function SettingsModal() {
  const [s, setS] = useState<Settings>({ ...(settings.value as Settings) });
  const set = <K extends keyof Settings>(k: K, v: Settings[K]) => setS({ ...s, [k]: v });
  const save = async () => {
    await saveSettings(s);
    await refreshModels();
    settingsOpen.value = false;
  };
  return (
    <div class="modal-bg" onClick={() => (settingsOpen.value = false)}>
      <div class="modal" onClick={(e) => e.stopPropagation()}>
        <h3>Settings</h3>
        <label>Model folders (one per line)
          <textarea rows={3} value={s.model_dirs.join("\n")} onInput={(e) => set("model_dirs", (e.target as HTMLTextAreaElement).value.split("\n").map((x) => x.trim()).filter(Boolean))} />
        </label>
        <label>PolyStrata folder
          <input value={s.polystrata_dir} onInput={(e) => set("polystrata_dir", (e.target as HTMLInputElement).value)} />
        </label>
        <label>llama-server.exe
          <input value={s.llama_server} onInput={(e) => set("llama_server", (e.target as HTMLInputElement).value)} />
        </label>
        <div class="grid2">
          <label>Temperature
            <input type="number" step={0.1} min={0} max={2} value={s.temperature} onInput={(e) => set("temperature", Number((e.target as HTMLInputElement).value))} />
          </label>
          <label>Max tool turns
            <input type="number" min={1} max={200} value={s.max_turns} onInput={(e) => set("max_turns", Number((e.target as HTMLInputElement).value))} />
          </label>
        </div>
        <label>System prompt
          <textarea rows={4} value={s.system_prompt} onInput={(e) => set("system_prompt", (e.target as HTMLTextAreaElement).value)} />
        </label>
        <label class="toggle">
          <input type="checkbox" checked={s.auto_approve} onChange={(e) => set("auto_approve", (e.target as HTMLInputElement).checked)} />
          Auto-approve file writes, shell commands and MCP tools (no prompts)
        </label>
        <div class="row">
          <span class="spacer" />
          <button onClick={() => (settingsOpen.value = false)}>Cancel</button>
          <button class="primary" onClick={() => void save()}>Save</button>
        </div>
      </div>
    </div>
  );
}
