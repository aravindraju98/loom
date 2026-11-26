import { ChatView } from "./ChatView";
import { FilesPanel, McpPanel, ModelsPanel } from "./Panels";
import { SettingsModal } from "./SettingsModal";
import { useState } from "preact/hooks";
import { save } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import {
  chat, chatFilter, chatMarkdown, chats, errText, openChat, removeChat, renameChat, restoreReq, rightTab, settingsOpen, settings,
  startNewChat, toast, unseenErrors,
} from "./state";
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

function ChatItem({ id, title }: { id: string; title: string }) {
  const sel = id === chat.value.id;
  const [editing, setEditing] = useState(false);
  const done = (t: string | null) => {
    setEditing(false);
    if (t !== null && t.trim() && t.trim() !== title) void renameChat(id, t);
  };
  return (
    <div class={`chatitem ${sel ? "sel" : ""}`}>
      {editing ? (
        <input
          class="chatitem-edit"
          aria-label="Chat title"
          value={title}
          ref={(el) => el?.focus()}
          onKeyDown={(e) => {
            if (e.key === "Enter") done((e.target as HTMLInputElement).value);
            if (e.key === "Escape") done(null);
          }}
          onBlur={(e) => done((e.target as HTMLInputElement).value)}
        />
      ) : (
        <button
          class="chatitem-open"
          aria-current={sel ? "page" : undefined}
          onClick={() => void openChat(id)}
          onDblClick={() => setEditing(true)}
          onKeyDown={(e) => e.key === "F2" && setEditing(true)}
          title={`${title || "Untitled"} (double-click or F2 to rename)`}
        >
          <span class="ellipsis">{title || "Untitled"}</span>
        </button>
      )}
      {!editing && <button class="x" title="Delete chat" aria-label={`Delete chat ${title || "Untitled"}`} onClick={() => void askDelete(id, title)}>×</button>}
    </div>
  );
}

function Sidebar() {
  const q = chatFilter.value.trim().toLowerCase();
  const list = q ? chats.value.filter((c) => (c.title ?? "").toLowerCase().includes(q)) : chats.value;
  return (
    <aside class="sidebar" aria-label="Chats">
      <div class="brand">Harness</div>
      <button class="primary wide" onClick={startNewChat}><Icon name="plus" size={14} /> New chat</button>
      {chats.value.length > 6 && (
        <div class="searchbar side-search">
          <Icon name="search" size={13} />
          <input placeholder="Search chats" aria-label="Search chats" value={chatFilter.value} onInput={(e) => (chatFilter.value = (e.target as HTMLInputElement).value)} />
        </div>
      )}
      <nav class="chatlist" aria-label="Chat history">
        {chats.value.length === 0 && <div class="hint" style={{ padding: "4px 10px" }}>No chats yet.</div>}
        {chats.value.length > 0 && list.length === 0 && <div class="hint" style={{ padding: "4px 10px" }}>No chats match.</div>}
        {list.map((c) => <ChatItem key={c.id} id={c.id} title={c.title ?? ""} />)}
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
      {chat.value.messages.length > 0 && (
        <button class="icon-btn" title="Export chat as Markdown" aria-label="Export chat as Markdown" onClick={() => void exportChat()}>
          <Icon name="save" size={15} />
        </button>
      )}
    </div>
  );
}

async function exportChat() {
  const c = chat.value;
  const name = (c.title || "chat").replace(/[\\/:*?"<>|]+/g, " ").trim().slice(0, 60) || "chat";
  try {
    const dest = await save({ defaultPath: `${name}.md`, filters: [{ name: "Markdown", extensions: ["md"] }] });
    if (!dest) return;
    await api.saveTextFile(dest, chatMarkdown(c));
    toast("Chat exported", "success");
  } catch (e) {
    toast(`Export failed: ${errText(e)}`, "error");
  }
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
