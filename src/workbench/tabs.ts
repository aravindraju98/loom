/** Editor tab rules (VS Code-like), as pure functions so they're easy to test. */

export interface Tab {
  path: string;
  /** single-clicked from the explorer: replaced by the next preview; pinned on edit or double-click */
  preview: boolean;
}

/** Open `path`. A preview open reuses the current preview slot unless that tab has unsaved edits. */
export function openTab(tabs: Tab[], path: string, preview: boolean, isDirty: (p: string) => boolean): Tab[] {
  const i = tabs.findIndex((t) => t.path === path);
  if (i >= 0) {
    if (!preview && tabs[i].preview) return tabs.map((t, j) => (j === i ? { ...t, preview: false } : t));
    return tabs;
  }
  const slot = preview ? tabs.findIndex((t) => t.preview && !isDirty(t.path)) : -1;
  if (slot >= 0) return tabs.map((t, j) => (j === slot ? { path, preview: true } : t));
  return [...tabs, { path, preview }];
}

export function pinTab(tabs: Tab[], path: string): Tab[] {
  return tabs.some((t) => t.path === path && t.preview) ? tabs.map((t) => (t.path === path ? { ...t, preview: false } : t)) : tabs;
}

/** Close `path`; if it was active, the neighbour to the right (else left) becomes active. */
export function closeTab(tabs: Tab[], path: string, active: string | null): { tabs: Tab[]; active: string | null } {
  const i = tabs.findIndex((t) => t.path === path);
  if (i < 0) return { tabs, active };
  const next = tabs.filter((t) => t.path !== path);
  if (active !== path) return { tabs: next, active };
  const n = next[i] ?? next[i - 1] ?? null;
  return { tabs: next, active: n ? n.path : null };
}

/** After a rename or move on disk, keep tabs pointing at the right files. */
export function renameTabs(tabs: Tab[], from: string, to: string): Tab[] {
  const f = from.toLowerCase();
  return tabs.map((t) => {
    const p = t.path.toLowerCase();
    if (p === f) return { ...t, path: to };
    if (p.startsWith(f + "\\")) return { ...t, path: to + t.path.slice(from.length) };
    return t;
  });
}

export function baseName(p: string): string {
  return p.split(/[\\/]/).filter(Boolean).pop() ?? p;
}

/** Shortest distinguishing label: "index.ts" or "index.ts · src/a" when names collide. */
export function tabLabels(paths: string[]): Map<string, string> {
  const out = new Map<string, string>();
  const byName = new Map<string, string[]>();
  for (const p of paths) {
    const n = baseName(p);
    byName.set(n, [...(byName.get(n) ?? []), p]);
  }
  for (const [n, ps] of byName) {
    if (ps.length === 1) {
      out.set(ps[0], n);
      continue;
    }
    for (const p of ps) {
      const parts = p.split(/[\\/]/).filter(Boolean);
      out.set(p, `${n} · ${parts.slice(-3, -1).join("/")}`);
    }
  }
  return out;
}
