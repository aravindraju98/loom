import { ChatView } from "./ChatView";
import { FilesPanel, McpPanel, ModelsPanel } from "./Panels";
import { SettingsModal } from "./SettingsModal";
import { chat, chats, openChat, removeChat, rightTab, settingsOpen, settings, startNewChat } from "./state";
import type { Tab } from "./state";

import { HubPanel } from "./HubPanel";
import { Icon } from "./icons";
import type { IconName } from "./icons";
import { ModelPicker } from "./ModelPicker";
import { ServerPanel } from "./ServerPanel";

const TABS: { id: Tab; label: string; icon: IconName }[] = [
  { id: "models", label: "Models", icon: "cube" },
  { id: "hub", label: "Hub", icon: "download" },
  { id: "server", label: "Server", icon: "server" },
  { id: "files", label: "Files", icon: "folder" },
  { id: "mcp", label: "MCP", icon: "plug" },
];

const Folder = () => (
  <svg class="ico" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" /></svg>
);

function Sidebar() {
  return (
    <aside class="sidebar">
      <div class="brand">Harness</div>
      <button class="primary wide" onClick={startNewChat}>+ New chat</button>
      <div class="chatlist">
        {chats.value.map((c) => (
          <div class={`chatitem ${c.id === chat.value.id ? "sel" : ""}`} key={c.id} onClick={() => void openChat(c.id)}>
            <span class="grow ellipsis">{c.title || "Untitled"}</span>
            <button class="x" title="Delete chat" aria-label="Delete chat" onClick={(e) => { e.stopPropagation(); void removeChat(c.id); }}>×</button>
          </div>
        ))}
      </div>
      <button class="wide" onClick={() => (settingsOpen.value = true)}>Settings</button>
    </aside>
  );
}

function TopBar() {
  return (
    <div class="topbar">
      <ModelPicker />
      <span class="spacer" />
      {settings.value?.workspace && (
        <span class="badge" title={settings.value.workspace}><Folder /> {settings.value.workspace.split("\\").filter(Boolean).pop()}</span>
      )}
      <span class="hint ellipsis topbar-title">{chat.value.title}</span>
    </div>
  );
}

export function App() {
  const tab = rightTab.value;
  return (
    <div class="shell">
      <Sidebar />
      <main class="main">
        <TopBar />
        <ChatView />
      </main>
      <aside class="right">
        <div class="tabs" role="tablist">
          {TABS.map((t) => (
            <button key={t.id} role="tab" aria-selected={tab === t.id} title={t.label} aria-label={t.label} class={tab === t.id ? "on" : ""} onClick={() => (rightTab.value = t.id)}>
              <Icon name={t.icon} size={17} />
              {tab === t.id && <span>{t.label}</span>}
            </button>
          ))}
        </div>
        {tab === "models" && <ModelsPanel />}
        {tab === "hub" && <HubPanel />}
        {tab === "server" && <ServerPanel />}
        {tab === "files" && <FilesPanel />}
        {tab === "mcp" && <McpPanel />}
      </aside>
      {settingsOpen.value && <SettingsModal />}
    </div>
  );
}
