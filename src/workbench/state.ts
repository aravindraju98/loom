import { effect, signal } from "@preact/signals";
import { api } from "../api";
import { chat, checkpointIdsFrom, errText, settings, toast } from "../state";
import { closeTab, openTab, pinTab, renameTabs } from "./tabs";
import type { Tab } from "./tabs";

/* ---------- layout ---------- */

export interface Layout {
  explorer: number;
  chat: number;
  explorerOpen: boolean;
  chatOpen: boolean;
}

const LAYOUT_KEY = "harness.layout";
function loadLayout(): Layout {
  const d: Layout = { explorer: 260, chat: 440, explorerOpen: true, chatOpen: true };
  try {
    return { ...d, ...(JSON.parse(localStorage.getItem(LAYOUT_KEY) ?? "{}") as Partial<Layout>) };
  } catch {
    return d;
  }
}
export const layout = signal<Layout>(loadLayout());
export function setLayout(p: Partial<Layout>) {
  layout.value = { ...layout.value, ...p };
  try {
    localStorage.setItem(LAYOUT_KEY, JSON.stringify(layout.value));
  } catch {
    /* private mode */
  }
}

/* ---------- open files ---------- */

export interface FileMeta {
  /** content on disk when last loaded/saved */
  saved: string;
  version: string;
  readonly: boolean;
  crlf: boolean;
  encoding: string;
  /** could not open (binary, too large…) */
  error?: string;
  /** changed on disk while the tab had unsaved edits */
  conflict?: boolean;
  /** an agent change to review: text before the agent's first change in the turn(s) */
  agent?: { base: string; ids: string[] };
  /** bumped whenever `saved` is replaced from disk, so the editor reloads its document */
  rev: number;
}

export const tabs = signal<Tab[]>([]);
export const active = signal<string | null>(null);
export const files = signal<Record<string, FileMeta>>({});
export const dirty = signal<Set<string>>(new Set());
/** bumped on any change on disk; the explorer refreshes the folders it shows */
export const fsTick = signal(0);
export const changedDirs = signal<Set<string>>(new Set());
/** reveal a path in the explorer (expand parents, select) */
export const revealPath = signal<string | null>(null);

const k = (p: string) => p.toLowerCase();
const findOpen = (p: string) => tabs.value.find((t) => k(t.path) === k(p))?.path ?? null;

export function setDirty(path: string, isDirty: boolean) {
  const has = dirty.value.has(path);
  if (has === isDirty) return;
  const s = new Set(dirty.value);
  if (isDirty) {
    s.add(path);
    tabs.value = pinTab(tabs.value, path);
  } else s.delete(path);
  dirty.value = s;
}

function patchMeta(path: string, p: Partial<FileMeta>) {
  const cur = files.value[path];
  if (cur) files.value = { ...files.value, [path]: { ...cur, ...p } };
}

async function load(path: string): Promise<void> {
  try {
    const f = await api.fsOpen(path);
    const prev = files.value[path];
    files.value = {
      ...files.value,
      [path]: { saved: f.text, version: f.version, readonly: f.readonly, crlf: f.crlf, encoding: f.encoding, rev: (prev?.rev ?? 0) + 1, agent: prev?.agent },
    };
  } catch (e) {
    files.value = { ...files.value, [path]: { saved: "", version: "", readonly: true, crlf: false, encoding: "", error: errText(e), rev: 1 } };
  }
}

/** Open a file in the editor. Single click = preview tab; `pin` keeps it open. */
export async function openFile(path: string, pin = false) {
  const existing = findOpen(path);
  const p = existing ?? path;
  tabs.value = openTab(tabs.value, p, !pin, (x) => dirty.value.has(x));
  // drop metadata of tabs replaced by the preview slot
  const keep = new Set(tabs.value.map((t) => t.path));
  const next: Record<string, FileMeta> = {};
  for (const [kk, v] of Object.entries(files.value)) if (keep.has(kk)) next[kk] = v;
  files.value = next;
  active.value = p;
  if (!files.value[p]) await load(p);
}

export function closeFile(path: string) {
  const r = closeTab(tabs.value, path, active.value);
  tabs.value = r.tabs;
  active.value = r.active;
  const { [path]: _gone, ...rest } = files.value;
  files.value = rest;
  setDirty(path, false);
}

/** Save `text`; on a disk conflict the tab shows a resolve bar instead. Returns success. */
export async function saveFile(path: string, text: string, force = false): Promise<boolean> {
  const m = files.value[path];
  if (!m || m.readonly) return false;
  try {
    const r = await api.fsSave(path, text, force ? undefined : m.version);
    patchMeta(path, { saved: text, version: r.version, conflict: false });
    setDirty(path, false);
    return true;
  } catch (e) {
    const msg = errText(e);
    if (msg.startsWith("conflict")) patchMeta(path, { conflict: true });
    else toast(`Save failed: ${msg}`, "error");
    return false;
  }
}

/** Discard edits and load what's on disk. */
export async function reloadFile(path: string) {
  setDirty(path, false);
  patchMeta(path, { conflict: false });
  await load(path);
}

/** Something changed on disk (agent, shell, another editor). */
export async function onDiskChange(paths: string[]) {
  fsTick.value++;
  const dirs = new Set<string>();
  for (const p of paths) dirs.add(k(p.replace(/[\\/][^\\/]*$/, "")));
  changedDirs.value = dirs;
  for (const p of paths) {
    const open = findOpen(p);
    if (!open) continue;
    const m = files.value[open];
    if (!m || m.error) {
      await load(open);
      continue;
    }
    let disk;
    try {
      disk = await api.fsOpen(open);
    } catch {
      continue; // deleted or unreadable: leave the tab as it is
    }
    if (disk.version === m.version) continue;
    if (dirty.value.has(open)) patchMeta(open, { conflict: true });
    else await load(open);
  }
}

/** Handle renames from the explorer. */
export function onRenamed(from: string, to: string) {
  tabs.value = renameTabs(tabs.value, from, to);
  if (active.value && k(active.value).startsWith(k(from))) active.value = to + active.value.slice(from.length);
  const next: Record<string, FileMeta> = {};
  for (const [p, v] of Object.entries(files.value)) next[k(p).startsWith(k(from)) ? to + p.slice(from.length) : p] = v;
  files.value = next;
}

/* ---------- agent changes ---------- */

function absPath(rel: string): string {
  if (/^[a-zA-Z]:[\\/]/.test(rel)) return rel;
  const ws = (settings.value?.workspace ?? "").replace(/[\\/]+$/, "");
  return `${ws}\\${rel.replace(/\//g, "\\")}`;
}

/** Show the agent's change to `rel` as a diff against how the file was before this request. */
export async function reviewAgentChange(rel: string) {
  const abs = absPath(rel);
  const ms = chat.value.messages;
  let lastUser = 0;
  for (let i = ms.length - 1; i >= 0; i--) if (ms[i].role === "user") { lastUser = i; break; }
  // find the request that changed this file most recently
  for (let i = ms.length - 1; i >= 0; i--) {
    const c = ms[i].role === "tool" ? ms[i].ui?.change : undefined;
    if (c && k(absPath(c.path.split(" → ").pop()!)) === k(abs)) {
      for (let j = i; j >= 0; j--) if (ms[j].role === "user") { lastUser = j; break; }
      break;
    }
  }
  const ids = checkpointIdsFrom(lastUser);
  await openFile(abs, true);
  const p = findOpen(abs) ?? abs;
  try {
    const base = await api.checkpointFileBase(chat.value.id, ids, p);
    patchMeta(p, { agent: { base: base ?? "", ids } });
  } catch {
    // no snapshot (e.g. changed by a shell command): plain file view
  }
}

export function keepAgentChange(path: string) {
  patchMeta(path, { agent: undefined });
}

export async function revertAgentChange(path: string) {
  const m = files.value[path];
  if (!m?.agent) return;
  try {
    await api.checkpointRevertFile(chat.value.id, m.agent.ids, path);
    patchMeta(path, { agent: undefined });
    await reloadFile(path);
    toast("Change reverted", "info");
  } catch (e) {
    toast(`Revert failed: ${errText(e)}`, "error");
  }
}

/** Open agent edits as they happen (only new ones, not when switching chats). */
let seen = { chat: "", count: 0 };
export function watchAgentChanges() {
  return effect(() => {
    const c = chat.value;
    if (c.id !== seen.chat) {
      seen = { chat: c.id, count: c.messages.length };
      return;
    }
    const fresh = c.messages.slice(seen.count);
    seen.count = c.messages.length;
    for (const m of fresh) {
      const ch = m.role === "tool" && m.ui?.ok !== false ? m.ui?.change : undefined;
      if (ch && ch.kind !== "delete" && ch.kind !== "move") void reviewAgentChange(ch.path);
    }
  });
}
