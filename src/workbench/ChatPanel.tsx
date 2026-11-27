import { useEffect, useRef, useState } from "preact/hooks";
import { save } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import { ChatView } from "../ChatView";
import { Icon } from "../icons";
import { ModelPicker } from "../ModelPicker";
import { busy, chat, chatFilter, chatMarkdown, chats, errText, openChat, removeChat, renameChat, startNewChat, toast } from "../state";
import { confirmDialog } from "../ui/Confirm";

function when(ts: number): string {
  const d = Date.now() - ts;
  if (d < 60_000) return "now";
  if (d < 3_600_000) return `${Math.floor(d / 60_000)}m`;
  if (d < 86_400_000) return `${Math.floor(d / 3_600_000)}h`;
  if (d < 7 * 86_400_000) return `${Math.floor(d / 86_400_000)}d`;
  return new Date(ts).toLocaleDateString([], { month: "short", day: "numeric" });
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

async function askDelete(id: string, title: string) {
  const ok = await confirmDialog({
    title: "Delete chat?",
    body: `"${title || "Untitled"}" and its restore points will be deleted. Files in your workspace are not affected.`,
    confirm: "Delete",
    danger: true,
  });
  if (ok) await removeChat(id);
}

/** Past chats: search, open, rename, delete. */
function ChatMenu({ onClose }: { onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [cursor, setCursor] = useState(0);
  const q = chatFilter.value.trim().toLowerCase();
  const list = q ? chats.value.filter((c) => (c.title ?? "").toLowerCase().includes(q)) : chats.value;

  useEffect(() => {
    ref.current?.querySelector<HTMLInputElement>("input")?.focus();
    const off = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && !(e.target as HTMLElement).closest(".chat-title-btn") && onClose();
    window.addEventListener("mousedown", off);
    return () => window.removeEventListener("mousedown", off);
  }, []);

  const pick = (id: string) => {
    onClose();
    void openChat(id);
  };

  return (
    <div class="chat-menu" ref={ref} role="dialog" aria-label="Chats">
      <div class="searchbar">
        <Icon name="search" size={13} />
        <input
          placeholder="Search chats"
          aria-label="Search chats"
          value={chatFilter.value}
          onInput={(e) => { chatFilter.value = (e.target as HTMLInputElement).value; setCursor(0); }}
          onKeyDown={(e) => {
            if (e.key === "Escape") onClose();
            else if (e.key === "ArrowDown") { e.preventDefault(); setCursor(Math.min(cursor + 1, list.length - 1)); }
            else if (e.key === "ArrowUp") { e.preventDefault(); setCursor(Math.max(cursor - 1, 0)); }
            else if (e.key === "Enter" && list[cursor]) pick(list[cursor].id);
          }}
        />
      </div>
      <div class="chat-menu-list" role="listbox" aria-label="Chat history">
        {chats.value.length === 0 && <div class="menu-empty">No chats yet.</div>}
        {chats.value.length > 0 && list.length === 0 && <div class="menu-empty">No chats match.</div>}
        {list.map((c, i) => {
          const sel = c.id === chat.value.id;
          return (
            <div key={c.id} class={`chat-row ${sel ? "sel" : ""} ${i === cursor ? "cursor" : ""}`} role="option" aria-selected={sel}>
              {renaming === c.id ? (
                <input
                  class="chatitem-edit"
                  aria-label="Chat title"
                  value={c.title ?? ""}
                  ref={(el) => el?.focus()}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") { void renameChat(c.id, (e.target as HTMLInputElement).value); setRenaming(null); }
                    if (e.key === "Escape") setRenaming(null);
                  }}
                  onBlur={(e) => { void renameChat(c.id, (e.target as HTMLInputElement).value); setRenaming(null); }}
                />
              ) : (
                <button class="chat-row-open" onClick={() => pick(c.id)} onMouseEnter={() => setCursor(i)}>
                  <span class="ellipsis grow">{c.title || "Untitled"}</span>
                  <span class="chat-row-when">{when(c.updated)}</span>
                </button>
              )}
              <button class="icon-btn sm" title="Rename" aria-label={`Rename ${c.title || "chat"}`} onClick={() => setRenaming(c.id)}>
                <Icon name="pencil" size={12} />
              </button>
              <button class="icon-btn sm danger" title="Delete" aria-label={`Delete ${c.title || "chat"}`} onClick={() => void askDelete(c.id, c.title ?? "")}>
                <Icon name="trash" size={12} />
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
}

export function ChatPanel() {
  const [menu, setMenu] = useState(false);
  const title = chat.value.messages.length ? chat.value.title : "New chat";
  return (
    <section class="chat-panel" aria-label="Chat">
      <div class="pane-head chat-head">
        <button class="chat-title-btn" aria-expanded={menu} aria-haspopup="dialog" disabled={busy.value} title="Switch chat" onClick={() => setMenu(!menu)}>
          <span class="ellipsis">{title}</span>
          <Icon name="chevron" size={12} />
        </button>
        <span class="spacer" />
        {chat.value.messages.length > 0 && (
          <button class="icon-btn sm" title="Export as Markdown" aria-label="Export chat as Markdown" onClick={() => void exportChat()}>
            <Icon name="save" size={14} />
          </button>
        )}
        <button class="icon-btn sm" title="New chat" aria-label="New chat" disabled={busy.value} onClick={startNewChat}>
          <Icon name="plus" size={15} />
        </button>
        {menu && <ChatMenu onClose={() => setMenu(false)} />}
      </div>
      <div class="chat-model"><ModelPicker /></div>
      <ChatView />
    </section>
  );
}
