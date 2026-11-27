import { Fragment } from "preact";
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { signal } from "@preact/signals";
import { open as pickFolder } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type { FsEntry } from "../api";
import { Icon } from "../icons";
import { agentTouched, attachments, errText, saveSettings, settings, toast } from "../state";
import { confirmDialog } from "../ui/Confirm";
import { FileIcon, FolderIcon } from "./fileIcons";
import { active, changedDirs, closeFile, fsTick, onRenamed, openFile, revealPath, tabs } from "./state";

const k = (p: string) => p.toLowerCase();
const parentOf = (p: string) => p.replace(/[\\/][^\\/]*$/, "");

/** folder path -> children (loaded lazily) */
const children = signal<Record<string, FsEntry[] | "error">>({});
const expanded = signal<Set<string>>(new Set());
const selected = signal<string | null>(null);
/** inline input: renaming an entry, or creating one inside a folder */
const editing = signal<{ mode: "rename"; path: string } | { mode: "new"; dir: string; isDir: boolean } | null>(null);

async function loadDir(dir: string) {
  try {
    const list = await api.fsList(dir);
    children.value = { ...children.value, [k(dir)]: list };
  } catch {
    children.value = { ...children.value, [k(dir)]: "error" };
  }
}

function toggle(dir: string, open?: boolean) {
  const s = new Set(expanded.value);
  const isOpen = s.has(k(dir));
  if (open ?? !isOpen) {
    s.add(k(dir));
    if (!children.value[k(dir)]) void loadDir(dir);
  } else s.delete(k(dir));
  expanded.value = s;
}

const BADGE: Record<string, { l: string; c: string; t: string }> = {
  edit: { l: "M", c: "var(--warn)", t: "Modified by the agent" },
  overwrite: { l: "M", c: "var(--warn)", t: "Rewritten by the agent" },
  append: { l: "M", c: "var(--warn)", t: "Modified by the agent" },
  create: { l: "A", c: "var(--ok)", t: "Created by the agent" },
  delete: { l: "D", c: "var(--bad)", t: "Deleted by the agent" },
  move: { l: "R", c: "var(--accent)", t: "Moved by the agent" },
};

interface Row {
  e: FsEntry;
  depth: number;
}

/** visible rows in order (for rendering and keyboard navigation) */
function flatten(root: string, filter: string): Row[] {
  const out: Row[] = [];
  const q = filter.trim().toLowerCase();
  const walk = (dir: string, depth: number): boolean => {
    const list = children.value[k(dir)];
    if (!list || list === "error") return false;
    let any = false;
    for (const e of list) {
      const start = out.length;
      out.push({ e, depth });
      let keep = !q || e.name.toLowerCase().includes(q);
      if (e.is_dir && (expanded.value.has(k(e.path)) || (q && children.value[k(e.path)]))) {
        const sub = walk(e.path, depth + 1);
        keep = keep || sub;
        if (!expanded.value.has(k(e.path)) && !q) out.length = start + 1;
      }
      if (!keep) out.length = start;
      any = any || keep;
    }
    return any;
  };
  walk(root, 0);
  return out;
}

function InlineInput({ initial, onDone }: { initial: string; onDone: (v: string | null) => void }) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.focus();
    const dot = initial.lastIndexOf(".");
    el.setSelectionRange(0, dot > 0 ? dot : initial.length);
  }, []);
  let done = false;
  const finish = (v: string | null) => {
    if (done) return;
    done = true;
    onDone(v);
  };
  return (
    <input
      ref={ref}
      class="tree-input"
      value={initial}
      aria-label="Name"
      onKeyDown={(e) => {
        if (e.key === "Enter") finish((e.target as HTMLInputElement).value.trim());
        if (e.key === "Escape") finish(null);
        e.stopPropagation();
      }}
      onBlur={(e) => finish((e.target as HTMLInputElement).value.trim())}
    />
  );
}

async function commitEdit(v: string | null) {
  const ed = editing.value;
  editing.value = null;
  if (!ed || !v) return;
  if (/[<>:"|?*]/.test(v) || v === "." || v === "..") {
    toast("That name isn't allowed on Windows.", "error");
    return;
  }
  try {
    if (ed.mode === "rename") {
      const parent = parentOf(ed.path);
      if (v === ed.path.slice(parent.length + 1)) return;
      const to = await api.fsRename(ed.path, `${parent}\\${v}`);
      onRenamed(ed.path, to);
      selected.value = to;
      await loadDir(parent);
    } else {
      const p = await api.fsCreate(`${ed.dir}\\${v}`, ed.isDir);
      toggle(ed.dir, true);
      await loadDir(ed.dir);
      selected.value = p;
      if (!ed.isDir) await openFile(p, true);
    }
  } catch (e) {
    toast(errText(e), "error");
  }
}

async function deleteEntry(e: FsEntry) {
  const ok = await confirmDialog({
    title: `Delete ${e.is_dir ? "folder" : "file"}?`,
    body: `"${e.name}" will be moved to the Recycle Bin.`,
    confirm: "Move to Recycle Bin",
    danger: true,
  });
  if (!ok) return;
  try {
    await api.fsDelete(e.path);
    for (const t of tabs.value) if (k(t.path) === k(e.path) || k(t.path).startsWith(k(e.path) + "\\")) closeFile(t.path);
    await loadDir(parentOf(e.path));
  } catch (x) {
    toast(errText(x), "error");
  }
}

interface Menu {
  x: number;
  y: number;
  e: FsEntry | null;
  dir: string;
}

function ContextMenu({ m, onClose }: { m: Menu; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>("button")?.focus();
    const off = (ev: MouseEvent) => !ref.current?.contains(ev.target as Node) && onClose();
    const esc = (ev: KeyboardEvent) => ev.key === "Escape" && onClose();
    window.addEventListener("mousedown", off);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", off);
      window.removeEventListener("keydown", esc);
    };
  }, []);
  const run = (f: () => void) => () => {
    onClose();
    f();
  };
  const e = m.e;
  const left = Math.min(m.x, window.innerWidth - 220);
  const top = Math.min(m.y, window.innerHeight - 260);
  return (
    <div class="ctx-menu" role="menu" ref={ref} style={{ left, top }}>
      <button role="menuitem" class="menu-item" onClick={run(() => (editing.value = { mode: "new", dir: m.dir, isDir: false }))}>New file…</button>
      <button role="menuitem" class="menu-item" onClick={run(() => (editing.value = { mode: "new", dir: m.dir, isDir: true }))}>New folder…</button>
      {e && (
        <>
          <div class="menu-sep" />
          {!e.is_dir && <button role="menuitem" class="menu-item" onClick={run(() => { if (!attachments.value.includes(e.path)) attachments.value = [...attachments.value, e.path]; })}>Add to chat</button>}
          <button role="menuitem" class="menu-item" onClick={run(() => void api.fsReveal(e.path).catch((x) => toast(errText(x), "error")))}>Reveal in File Explorer</button>
          <button role="menuitem" class="menu-item" onClick={run(() => void navigator.clipboard.writeText(e.path))}>Copy path</button>
          <div class="menu-sep" />
          <button role="menuitem" class="menu-item" onClick={run(() => (editing.value = { mode: "rename", path: e.path }))}>Rename <kbd>F2</kbd></button>
          <button role="menuitem" class="menu-item danger" onClick={run(() => void deleteEntry(e))}>Delete <kbd>Del</kbd></button>
        </>
      )}
    </div>
  );
}

export function Explorer() {
  const ws = (settings.value?.workspace ?? "").replace(/[\\/]+$/, "");
  const [filter, setFilter] = useState("");
  const [menu, setMenu] = useState<Menu | null>(null);
  const tree = useRef<HTMLDivElement>(null);

  // new workspace: reset and load the root
  useEffect(() => {
    children.value = {};
    expanded.value = new Set();
    selected.value = null;
    if (ws) {
      void loadDir(ws);
      void api.fsWatch().catch((e) => toast(`File watching unavailable: ${errText(e)}`, "error"));
    }
  }, [ws]);

  // something changed on disk: reload the affected folders that are loaded
  useEffect(() => {
    if (!ws || fsTick.value === 0) return;
    for (const d of changedDirs.value) if (children.value[d] || d === k(ws)) void loadDir(d === k(ws) ? ws : d);
  }, [fsTick.value]);

  // follow the active editor tab
  useEffect(() => {
    const p = revealPath.value ?? active.value;
    if (!p || !ws || !k(p).startsWith(k(ws))) return;
    let dir = parentOf(p);
    const chain: string[] = [];
    while (k(dir).length > k(ws).length) {
      chain.unshift(dir);
      dir = parentOf(dir);
    }
    for (const d of chain) toggle(d, true);
    selected.value = p;
    revealPath.value = null;
    requestAnimationFrame(() => tree.current?.querySelector(".tree-row.sel")?.scrollIntoView({ block: "nearest" }));
  }, [active.value, revealPath.value]);

  const rows = useMemo(() => (ws ? flatten(ws, filter) : []), [ws, filter, children.value, expanded.value]);
  const touched = agentTouched.value;
  const touchedDirs = useMemo(() => {
    const s = new Set<string>();
    for (const p of touched.keys()) {
      let d = parentOf(p);
      while (d.length > k(ws).length) {
        s.add(d);
        d = parentOf(d);
      }
    }
    return s;
  }, [touched, ws]);

  const choose = async () => {
    const dir = await pickFolder({ directory: true, multiple: false, defaultPath: ws || undefined });
    if (typeof dir === "string") await saveSettings({ workspace: dir });
  };

  if (!ws) {
    return (
      <aside class="explorer" aria-label="Explorer">
        <div class="pane-head"><span class="pane-title">Explorer</span></div>
        <div class="explorer-empty">
          <p class="hint">Open a folder to browse and edit its files. The agent can only work inside this folder.</p>
          <button class="primary" onClick={() => void choose()}><Icon name="folder" size={14} /> Open folder</button>
        </div>
      </aside>
    );
  }

  const selIdx = rows.findIndex((r) => k(r.e.path) === k(selected.value ?? ""));
  const onKey = (ev: KeyboardEvent) => {
    if (editing.value) return;
    const cur = rows[selIdx];
    const go = (i: number) => {
      const r = rows[Math.max(0, Math.min(rows.length - 1, i))];
      if (r) selected.value = r.e.path;
      requestAnimationFrame(() => tree.current?.querySelector(".tree-row.sel")?.scrollIntoView({ block: "nearest" }));
    };
    switch (ev.key) {
      case "ArrowDown": go(selIdx + 1); break;
      case "ArrowUp": go(selIdx - 1); break;
      case "Home": go(0); break;
      case "End": go(rows.length - 1); break;
      case "ArrowRight":
        if (cur?.e.is_dir) expanded.value.has(k(cur.e.path)) ? go(selIdx + 1) : toggle(cur.e.path, true);
        break;
      case "ArrowLeft":
        if (cur?.e.is_dir && expanded.value.has(k(cur.e.path))) toggle(cur.e.path, false);
        else if (cur) {
          const parent = rows.findIndex((r) => k(r.e.path) === k(parentOf(cur.e.path)));
          if (parent >= 0) go(parent);
        }
        break;
      case "Enter":
      case " ":
        if (cur) cur.e.is_dir ? toggle(cur.e.path) : void openFile(cur.e.path, ev.key === "Enter");
        break;
      case "F2":
        if (cur) editing.value = { mode: "rename", path: cur.e.path };
        break;
      case "Delete":
        if (cur) void deleteEntry(cur.e);
        break;
      default:
        return;
    }
    ev.preventDefault();
  };

  const targetDir = () => {
    const cur = rows[selIdx]?.e;
    return cur ? (cur.is_dir ? cur.path : parentOf(cur.path)) : ws;
  };
  const rootList = children.value[k(ws)];
  const newAt = editing.value?.mode === "new" ? editing.value : null;

  return (
    <aside class="explorer" aria-label="Explorer">
      <div class="pane-head">
        <span class="pane-title ellipsis" title={ws}>{ws.split("\\").pop()}</span>
        <span class="spacer" />
        <button class="icon-btn sm" title="New file" aria-label="New file" onClick={() => (editing.value = { mode: "new", dir: targetDir(), isDir: false })}><Icon name="plus" size={14} /></button>
        <button class="icon-btn sm" title="New folder" aria-label="New folder" onClick={() => (editing.value = { mode: "new", dir: targetDir(), isDir: true })}><Icon name="folder" size={14} /></button>
        <button class="icon-btn sm" title="Refresh" aria-label="Refresh" onClick={() => { for (const d of Object.keys(children.value)) void loadDir(d); }}><Icon name="refresh" size={14} /></button>
        <button class="icon-btn sm" title="Collapse folders" aria-label="Collapse folders" onClick={() => (expanded.value = new Set())}><Icon name="chevron" size={14} /></button>
        <button class="icon-btn sm" title="Open another folder" aria-label="Open another folder" onClick={() => void choose()}><Icon name="search" size={14} /></button>
      </div>
      <div class="explorer-filter">
        <input placeholder="Filter files" aria-label="Filter files" value={filter} onInput={(e) => setFilter((e.target as HTMLInputElement).value)} />
      </div>
      <div
        class="tree"
        role="tree"
        aria-label="Workspace files"
        tabIndex={0}
        ref={tree}
        onKeyDown={onKey}
        onContextMenu={(ev) => {
          ev.preventDefault();
          setMenu({ x: ev.clientX, y: ev.clientY, e: null, dir: ws });
        }}
      >
        {rootList === "error" && <div class="error small">Can't read this folder.</div>}
        {rootList && rootList !== "error" && rootList.length === 0 && !newAt && <div class="hint tree-hint">This folder is empty.</div>}
        {newAt && k(newAt.dir) === k(ws) && <NewRow depth={0} isDir={newAt.isDir} />}
        {rows.map(({ e, depth }) => {
          const isOpen = e.is_dir && expanded.value.has(k(e.path));
          const b = !e.is_dir ? BADGE[touched.get(k(e.path)) ?? ""] : undefined;
          const sel = k(selected.value ?? "") === k(e.path);
          const isActive = k(active.value ?? "") === k(e.path);
          const renaming = editing.value?.mode === "rename" && k(editing.value.path) === k(e.path);
          return (
            <Fragment key={e.path}>
              <div
                role="treeitem"
                aria-level={depth + 1}
                aria-expanded={e.is_dir ? isOpen : undefined}
                aria-selected={sel}
                class={`tree-row ${sel ? "sel" : ""} ${isActive ? "active" : ""} ${e.ignored ? "ignored" : ""}`}
                style={{ paddingLeft: 8 + depth * 12 }}
                title={e.path}
                onClick={() => {
                  selected.value = e.path;
                  e.is_dir ? toggle(e.path) : void openFile(e.path);
                }}
                onDblClick={() => !e.is_dir && void openFile(e.path, true)}
                onContextMenu={(ev) => {
                  ev.preventDefault();
                  ev.stopPropagation();
                  selected.value = e.path;
                  setMenu({ x: ev.clientX, y: ev.clientY, e, dir: e.is_dir ? e.path : parentOf(e.path) });
                }}
              >
                <span class={`twisty ${e.is_dir ? (isOpen ? "open" : "") : "none"}`} aria-hidden="true" />
                {e.is_dir ? <FolderIcon open={isOpen} /> : <FileIcon name={e.name} />}
                {renaming ? (
                  <InlineInput initial={e.name} onDone={(v) => void commitEdit(v)} />
                ) : (
                  <span class="tree-name" style={b ? { color: b.c } : undefined}>{e.name}</span>
                )}
                {b && <span class="tree-badge" style={{ color: b.c }} title={b.t}>{b.l}</span>}
                {e.is_dir && !isOpen && touchedDirs.has(k(e.path)) && <span class="tree-dot" title="Contains files changed by the agent" />}
              </div>
              {newAt && e.is_dir && isOpen && k(newAt.dir) === k(e.path) && <NewRow depth={depth + 1} isDir={newAt.isDir} />}
            </Fragment>
          );
        })}
      </div>
      {menu && <ContextMenu m={menu} onClose={() => setMenu(null)} />}
    </aside>
  );
}

function NewRow({ depth, isDir }: { depth: number; isDir: boolean }) {
  return (
    <div class="tree-row sel" style={{ paddingLeft: 8 + depth * 12 }}>
      <span class="twisty none" aria-hidden="true" />
      {isDir ? <FolderIcon open={false} /> : <FileIcon name="" />}
      <InlineInput initial="" onDone={(v) => void commitEdit(v)} />
    </div>
  );
}
