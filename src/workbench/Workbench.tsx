import { useEffect, useRef } from "preact/hooks";
import { onFsChanged } from "../api";
import { Icon } from "../icons";
import {
  agentTouched, busy, contextUse, engine, lastUsage, openSettings, runStatus, settings, settingsOpen, unseenErrors,
} from "../state";
import { ChatPanel } from "./ChatPanel";
import { EditorArea } from "./Editor";
import { Explorer } from "./Explorer";
import { FileIcon } from "./fileIcons";
import { SettingsPage } from "./SettingsPage";
import { active, dirty, layout, onDiskChange, reviewAgentChange, setLayout, tabs, watchAgentChanges } from "./state";
import { baseName } from "./tabs";

/** Drag handle between two columns. `side` = which column it resizes. */
function Splitter({ side }: { side: "explorer" | "chat" }) {
  const start = useRef<{ x: number; w: number } | null>(null);
  const min = side === "explorer" ? 180 : 320;
  const max = side === "explorer" ? 520 : 760;
  const onDown = (e: PointerEvent) => {
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    start.current = { x: e.clientX, w: layout.value[side] };
    document.body.classList.add("resizing");
  };
  const onMove = (e: PointerEvent) => {
    if (!start.current) return;
    const dx = e.clientX - start.current.x;
    const w = side === "explorer" ? start.current.w + dx : start.current.w - dx;
    setLayout({ [side]: Math.max(min, Math.min(max, w)) });
  };
  const onUp = () => {
    start.current = null;
    document.body.classList.remove("resizing");
  };
  const onKey = (e: KeyboardEvent) => {
    const step = e.shiftKey ? 40 : 10;
    const dir = e.key === "ArrowLeft" ? -1 : e.key === "ArrowRight" ? 1 : 0;
    if (!dir) return;
    e.preventDefault();
    const w = layout.value[side] + (side === "explorer" ? dir : -dir) * step;
    setLayout({ [side]: Math.max(min, Math.min(max, w)) });
  };
  return (
    <div
      class="splitter"
      role="separator"
      aria-orientation="vertical"
      aria-label={side === "explorer" ? "Resize explorer" : "Resize chat"}
      aria-valuenow={layout.value[side]}
      tabIndex={0}
      onPointerDown={onDown}
      onPointerMove={onMove}
      onPointerUp={onUp}
      onDblClick={() => setLayout({ [side]: side === "explorer" ? 260 : 440 })}
      onKeyDown={onKey}
    />
  );
}

function Welcome() {
  const touched = [...agentTouched.value.entries()].filter(([, kind]) => kind !== "delete").slice(-8).reverse();
  const ws = settings.value?.workspace;
  return (
    <div class="welcome">
      <div class="welcome-inner">
        <h2>{ws ? ws.split("\\").filter(Boolean).pop() : "No folder open"}</h2>
        <p class="hint">
          {ws ? "Pick a file in the explorer, or ask the agent in the chat. Its edits open here as diffs you can keep or revert." : "Open a folder from the explorer to start."}
        </p>
        {touched.length > 0 && (
          <>
            <div class="section-title">Changed by the agent in this chat</div>
            <ul class="welcome-list">
              {touched.map(([p]) => (
                <li key={p}>
                  <button class="welcome-file" onClick={() => void reviewAgentChange(p)}>
                    <FileIcon name={baseName(p)} size={14} />
                    <span class="ellipsis">{ws ? p.slice(ws.length).replace(/^[\\/]/, "") : p}</span>
                  </button>
                </li>
              ))}
            </ul>
          </>
        )}
        <div class="section-title">Shortcuts</div>
        <dl class="keys">
          <dt><kbd>Ctrl</kbd>+<kbd>S</kbd></dt><dd>Save</dd>
          <dt><kbd>Ctrl</kbd>+<kbd>F</kbd></dt><dd>Find in file</dd>
          <dt><kbd>Ctrl</kbd>+<kbd>B</kbd></dt><dd>Toggle explorer</dd>
          <dt><kbd>Ctrl</kbd>+<kbd>J</kbd></dt><dd>Toggle chat</dd>
          <dt><kbd>Ctrl</kbd>+<kbd>L</kbd></dt><dd>Focus chat input</dd>
          <dt><kbd>Ctrl</kbd>+<kbd>,</kbd></dt><dd>Settings</dd>
        </dl>
      </div>
    </div>
  );
}

function StatusBar() {
  const st = engine.value;
  const c = contextUse.value;
  const u = lastUsage.value;
  const cache = u?.prompt_tokens && u.cache_n != null ? Math.round((u.cache_n / u.prompt_tokens) * 100) : null;
  const dirtyCount = dirty.value.size;
  return (
    <footer class="statusbar">
      <button class="sb-item" onClick={() => openSettings("models")} title="Models">
        <span class={`dot ${st.state}`} />
        {st.state === "ready" ? st.model_name || "model ready" : st.state === "loading" ? "loading model…" : st.state === "error" ? "engine error" : "no model loaded"}
      </button>
      {busy.value && <span class="sb-item">{runStatus.value || "working…"}</span>}
      <span class="spacer" />
      {dirtyCount > 0 && <span class="sb-item">{dirtyCount} unsaved</span>}
      {c && <span class="sb-item" title="Prompt tokens of the last turn / context window">ctx {Math.round(c.used / 100) / 10}k / {Math.round(c.ctx / 1000)}k</span>}
      {cache != null && <span class="sb-item" title="Prompt reused from the KV cache on the last turn">cache {cache}%</span>}
      {u?.tok_s ? <span class="sb-item">{u.tok_s.toFixed(1)} tok/s</span> : null}
      {unseenErrors.value > 0 && (
        <button class="sb-item sb-error" onClick={() => openSettings("logs")} title="Open logs">⚠ {unseenErrors.value} error{unseenErrors.value === 1 ? "" : "s"}</button>
      )}
    </footer>
  );
}

function TitleBar() {
  const L = layout.value;
  const p = active.value;
  const ws = settings.value?.workspace ?? "";
  const rel = p && ws && p.toLowerCase().startsWith(ws.toLowerCase()) ? p.slice(ws.length).replace(/^[\\/]/, "") : p;
  return (
    <header class="titlebar">
      <div class="brand">Harness</div>
      <span class="title-path ellipsis">{ws ? ws.split("\\").filter(Boolean).pop() : ""}{rel ? ` › ${rel.replace(/\\/g, " › ")}` : ""}</span>
      <span class="spacer" />
      <button class={`icon-btn sm ${L.explorerOpen ? "on" : ""}`} title="Toggle explorer (Ctrl+B)" aria-label="Toggle explorer" aria-pressed={L.explorerOpen} onClick={() => setLayout({ explorerOpen: !L.explorerOpen })}>
        <Icon name="sidebar" size={15} />
      </button>
      <button class={`icon-btn sm ${L.chatOpen ? "on" : ""}`} title="Toggle chat (Ctrl+J)" aria-label="Toggle chat" aria-pressed={L.chatOpen} onClick={() => setLayout({ chatOpen: !L.chatOpen })}>
        <Icon name="chat" size={15} />
      </button>
      <button class={`icon-btn sm ${settingsOpen.value ? "on" : ""}`} title="Settings (Ctrl+,)" aria-label="Settings" onClick={() => (settingsOpen.value ? (settingsOpen.value = false) : openSettings())}>
        <Icon name="settings" size={15} />
      </button>
    </header>
  );
}

export function Workbench() {
  const L = layout.value;
  useEffect(() => {
    const stop = watchAgentChanges();
    const un = onFsChanged((paths) => void onDiskChange(paths));
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      const key = e.key.toLowerCase();
      if (key === "b") setLayout({ explorerOpen: !layout.value.explorerOpen });
      else if (key === "j") setLayout({ chatOpen: !layout.value.chatOpen });
      else if (key === ",") settingsOpen.value ? (settingsOpen.value = false) : openSettings();
      else if (key === "l") {
        setLayout({ chatOpen: true });
        requestAnimationFrame(() => document.querySelector<HTMLTextAreaElement>(".composer textarea")?.focus());
      } else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    // warn before closing the window with unsaved files
    const onUnload = (e: BeforeUnloadEvent) => {
      if (dirty.value.size > 0) e.preventDefault();
    };
    window.addEventListener("beforeunload", onUnload);
    return () => {
      stop();
      void un.then((f) => f());
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("beforeunload", onUnload);
    };
  }, []);

  const cols = [
    L.explorerOpen ? `${L.explorer}px` : "",
    L.explorerOpen ? "4px" : "",
    "minmax(280px, 1fr)",
    L.chatOpen ? "4px" : "",
    L.chatOpen ? `${L.chat}px` : "",
  ].filter(Boolean).join(" ");

  return (
    <div class="workbench">
      <TitleBar />
      <div class="wb-main">
        {settingsOpen.value ? (
          <SettingsPage />
        ) : (
          <div class="wb-grid" style={{ gridTemplateColumns: cols }}>
            {L.explorerOpen && <Explorer />}
            {L.explorerOpen && <Splitter side="explorer" />}
            <EditorArea welcome={tabs.value.length === 0 ? <Welcome /> : null} />
            {L.chatOpen && <Splitter side="chat" />}
            {L.chatOpen && <ChatPanel />}
          </div>
        )}
      </div>
      <StatusBar />
    </div>
  );
}
