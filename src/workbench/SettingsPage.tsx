import { useEffect } from "preact/hooks";
import { HubPanel } from "../HubPanel";
import { Icon } from "../icons";
import type { IconName } from "../icons";
import { LogsPanel } from "../LogsPanel";
import { McpPanel, ModelsPanel } from "../Panels";
import { AgentSettings, PermissionSettings } from "../SettingsForms";
import { ServerPanel } from "../ServerPanel";
import { settingsOpen, settingsSection, unseenErrors } from "../state";
import type { SettingsSection } from "../state";

const SECTIONS: { id: SettingsSection; label: string; icon: IconName; blurb: string }[] = [
  { id: "models", label: "Models", icon: "cube", blurb: "Models on this PC and remote endpoints. Load one to chat." },
  { id: "download", label: "Download", icon: "download", blurb: "Find and download GGUF models from Hugging Face." },
  { id: "server", label: "Server", icon: "server", blurb: "Serve the loaded model as an OpenAI-compatible API, and llama.cpp options." },
  { id: "mcp", label: "MCP", icon: "plug", blurb: "Extra tools for the agent from MCP servers." },
  { id: "agent", label: "Agent", icon: "wrench", blurb: "Engine paths, temperature, turn limit and system prompt." },
  { id: "permissions", label: "Permissions", icon: "check", blurb: "What the agent may do without asking." },
  { id: "logs", label: "Logs & evals", icon: "activity", blurb: "Live log, run traces and local-model evals." },
];

export function SettingsPage() {
  const sec = settingsSection.value;
  const cur = SECTIONS.find((s) => s.id === sec) ?? SECTIONS[0];
  const close = () => (settingsOpen.value = false);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !document.querySelector(".modal-bg")) close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  useEffect(() => {
    if (sec === "logs") unseenErrors.value = 0;
  }, [sec]);
  return (
    <div class="settings-page" role="region" aria-label="Settings">
      <nav class="settings-nav" aria-label="Settings sections">
        <button class="settings-back" onClick={close}><Icon name="back" size={14} /> Back to editor</button>
        <div class="settings-nav-title">Settings</div>
        {SECTIONS.map((s) => (
          <button key={s.id} class={`settings-link ${s.id === sec ? "on" : ""}`} aria-current={s.id === sec ? "page" : undefined} onClick={() => (settingsSection.value = s.id)}>
            <Icon name={s.icon} size={15} />
            <span class="grow">{s.label}</span>
            {s.id === "logs" && unseenErrors.value > 0 && <span class="count-badge">{unseenErrors.value}</span>}
          </button>
        ))}
      </nav>
      <main class="settings-main">
        <header class="settings-head">
          <h1>{cur.label}</h1>
          <p class="hint">{cur.blurb}</p>
        </header>
        <div class={`settings-body ${sec === "logs" ? "wide" : ""}`}>
          {sec === "models" && <ModelsPanel />}
          {sec === "download" && <HubPanel />}
          {sec === "server" && <ServerPanel />}
          {sec === "mcp" && <McpPanel />}
          {sec === "agent" && <AgentSettings />}
          {sec === "permissions" && <PermissionSettings />}
          {sec === "logs" && <LogsPanel />}
        </div>
      </main>
    </div>
  );
}
