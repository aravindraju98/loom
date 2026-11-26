import { ChatView } from "./ChatView";
import { FilesPanel, McpPanel, ModelsPanel } from "./Panels";
import { SettingsModal } from "./SettingsModal";
import { chat, chats, openChat, removeChat, restoreReq, rightTab, settingsOpen, settings, startNewChat, unseenErrors } from "./state";
import { LogsPanel } from "./LogsPanel";
import { RestoreDialog } from "./RestoreDialog";
import type { Tab } from "./state";

import { HubPanel } from "./HubPanel";
import { Icon } from "./icons";
import type { IconName } from "./icons";
import { ModelPicker } from "./ModelPicker";
import { ServerPanel } from "./ServerPanel";
import { Toasts } from "./ui/Toasts";
import { ConfirmHost, confirmDialog } from "./ui/Confirm";

const TABS: { id: Tab; label: string; icon: IconName }[] = [
  { id: "models", label: "Models", icon: "cube" },
  { id: "hub", label: "Hub", icon: "download" },
  { id: "server", label: "Server", icon: "server" },
  { id: "files", label: "Files", icon: "folder" },
  { id: "mcp", label: "MCP", icon: "plug" },
  { id: "logs", label: "Logs", icon: "activity" },
];

async function askDelete(id: string, title: string) {
  const ok = await confirmDialog({
    title: "Delete chat?",
    body: `"${title || "Untitled"}" and its restore points will be deleted. Files in your workspace are not affected.`,
    confirm: "Delete",
    danger: true,
  });
  if (ok) await removeChat(id);
}

function Sidebar() {
  return (
    <aside class="sidebar" aria-label="Chats">
      <div class="brand">Harness</div>
      <button class="primary wide" onClick={startNewChat}><Icon name="plus" size={14} /> New chat</button>
      <nav class="chatlist" aria-label="Chat history">
        {chats.value.length === 0 && <div class="hint" style={{ padding: "4px 10px" }}>No chats yet.</div>}
        {chats.value.map((c) => {
          const sel = c.id === chat.value.id;
          return (
            <div class={`chatitem ${sel ? "sel" : ""}`} key={c.id}>
              <button class="chatitem-open" aria-current={sel ? "page" : undefined} onClick={() => void openChat(c.id)} title={c.title || "Untitled"}>
                <span class="ellipsis">{c.title || "Untitled"}</span>
              </button>
              <button class="x" title="Delete chat" aria-label={`Delete chat ${c.title || "Untitled"}`} onClick={() => void askDelete(c.id, c.title ?? "")}>×</button>
            </div>
          );
        })}
      </nav>
      <button class="wide" onClick={() => (settingsOpen.value = true)}><Icon name="settings" size={14} /> Settings</button>
    </aside>
  );
}

function TopBar() {
  return (
    <div class="topbar">
      <ModelPicker />
      <span class="spacer" />
      {settings.value?.workspace && (
        <span class="badge" title={settings.value.workspace}><Icon name="folder" size={12} /> {settings.value.workspace.split("\\").filter(Boolean).pop()}</span>
      )}
      <span class="hint ellipsis topbar-title">{chat.value.title}</span>
    </div>
  );
}

/** Arrow keys move between tabs (WAI-ARIA tabs pattern). */
function onTabKey(e: KeyboardEvent) {
  const i = TABS.findIndex((t) => t.id === rightTab.value);
  let j = -1;
  if (e.key === "ArrowRight") j = (i + 1) % TABS.length;
  else if (e.key === "ArrowLeft") j = (i - 1 + TABS.length) % TABS.length;
  else if (e.key === "Home") j = 0;
  else if (e.key === "End") j = TABS.length - 1;
  if (j < 0) return;
  e.preventDefault();
  rightTab.value = TABS[j].id;
  document.getElementById(`tab-${TABS[j].id}`)?.focus();
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
        <div class="tabs" role="tablist" aria-label="Panels" onKeyDown={onTabKey}>
          {TABS.map((t) => (
            <button
              key={t.id}
              id={`tab-${t.id}`}
              role="tab"
              aria-selected={tab === t.id}
              aria-controls="tabpanel"
              tabIndex={tab === t.id ? 0 : -1}
              title={t.label}
              aria-label={t.label}
              class={tab === t.id ? "on" : ""}
              onClick={() => (rightTab.value = t.id)}
            >
              <Icon name={t.icon} size={16} />
              {tab === t.id && <span>{t.label}</span>}
              {t.id === "logs" && tab !== "logs" && unseenErrors.value > 0 && <i class="tab-badge" aria-label={`${unseenErrors.value} new errors`} />}
            </button>
          ))}
        </div>
        <div id="tabpanel" role="tabpanel" aria-labelledby={`tab-${tab}`} class="tabpanel">
          {tab === "models" && <ModelsPanel />}
          {tab === "hub" && <HubPanel />}
          {tab === "server" && <ServerPanel />}
          {tab === "files" && <FilesPanel />}
          {tab === "mcp" && <McpPanel />}
          {tab === "logs" && <LogsPanel />}
        </div>
      </aside>
      {settingsOpen.value && <SettingsModal />}
      {restoreReq.value && <RestoreDialog />}
      <ConfirmHost />
      <Toasts />
    </div>
  );
}
