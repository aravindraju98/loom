import { useEffect, useState } from "preact/hooks";
import { signal } from "@preact/signals";
import { api, fmtBytes } from "./api";
import type { HfGroup, HfList, HfModel } from "./api";
import { Icon } from "./icons";
import { downloads, saveSettings, settings } from "./state";

/** "3d ago", "2mo ago" */
function ago(iso?: string): string {
  if (!iso) return "";
  const d = (Date.now() - Date.parse(iso)) / 86_400_000;
  if (!Number.isFinite(d)) return "";
  if (d < 1) return "today";
  if (d < 30) return `${Math.floor(d)}d ago`;
  if (d < 365) return `${Math.floor(d / 30)}mo ago`;
  return `${Math.floor(d / 365)}y ago`;
}

const LISTS: { id: HfList; label: string; hint: string }[] = [
  { id: "recommended", label: "Recommended", hint: "Trending chat models from well-known GGUF publishers (unsloth, bartowski, lmstudio-community, ggml-org…)" },
  { id: "trending", label: "Trending", hint: "What's popular on Hugging Face right now" },
  { id: "downloads", label: "Most downloaded", hint: "All-time downloads" },
  { id: "likes", label: "Most liked", hint: "All-time likes" },
];

/** fetched lists, kept while the app is open (switching tabs doesn't refetch) */
const browseCache = signal<Partial<Record<HfList, HfModel[]>>>({});
const browseList = signal<HfList>("recommended");

function Browse({ dest }: { dest: string }) {
  const list = browseList.value;
  const items = browseCache.value[list];
  const [err, setErr] = useState("");
  const load = (force = false) => {
    if (!force && browseCache.value[list]) return;
    setErr("");
    api
      .hfBrowse(list)
      .then((r) => (browseCache.value = { ...browseCache.value, [list]: r }))
      .catch((e) => setErr(String(e)));
  };
  useEffect(() => load(), [list]);
  return (
    <>
      <div class="segmented hub-lists" role="tablist" aria-label="Model lists">
        {LISTS.map((l) => (
          <button key={l.id} role="tab" aria-selected={list === l.id} class={list === l.id ? "on" : ""} title={l.hint} onClick={() => (browseList.value = l.id)}>
            {l.label}
          </button>
        ))}
      </div>
      {err && (
        <div class="error small">
          {err} <button class="chip-btn" onClick={() => load(true)}>Retry</button>
        </div>
      )}
      {!items && !err && (
        <div class="skeleton-list" aria-label="Loading">
          {Array.from({ length: 6 }, (_, i) => <div key={i} class="skeleton" />)}
        </div>
      )}
      {items?.length === 0 && <div class="hint">Nothing here right now.</div>}
      {items?.map((m) => <Repo key={m.id} m={m} dest={dest} />)}
      {items && items.length > 0 && <div class="hint small">From huggingface.co · GGUF chat models only</div>}
    </>
  );
}

const fmtN = (n: number) => (n >= 1e6 ? (n / 1e6).toFixed(1) + "M" : n >= 1e3 ? (n / 1e3).toFixed(1) + "k" : String(n));

function Group({ repo, g, dest }: { repo: string; g: HfGroup; dest: string }) {
  const id = `${repo}::${g.files[0].path}`;
  const d = downloads.value[id];
  const active = d?.state === "downloading";
  const [err, setErr] = useState("");
  const start = async () => {
    setErr("");
    try {
      await api.hfDownload(repo, g.files, dest);
    } catch (e) {
      setErr(String(e));
    }
  };
  const pct = d?.total ? Math.min(100, Math.round(((d.done ?? 0) / d.total) * 100)) : 0;
  return (
    <div class="hf-file">
      <div class="row">
        <div class="grow">
          <div class="menu-name ellipsis" title={g.label}>{g.label}</div>
          <div class="menu-sub">
            {fmtBytes(g.total)}{g.files.length > 1 ? ` · ${g.files.length} parts` : ""}{g.kind === "mmproj" ? " · vision projector" : ""}
          </div>
        </div>
        {d?.state === "done" ? (
          <span class="pill ready" title="Downloaded: it now appears in your models"><Icon name="check" size={12} /> saved</span>
        ) : active ? (
          <button class="icon-btn danger" title="Cancel download" aria-label="Cancel download" onClick={() => void api.hfCancel(id)}><Icon name="x" /></button>
        ) : (
          <button class="icon-btn primary" title={d?.state === "cancelled" || d?.state === "error" ? "Resume download" : "Download"} aria-label="Download" disabled={!dest} onClick={() => void start()}><Icon name="download" /></button>
        )}
      </div>
      {active && (
        <>
          <div class="bar" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}><i style={{ width: `${pct}%` }} /></div>
          <div class="menu-sub">{pct}% · {fmtBytes(d.done ?? 0)} / {fmtBytes(d.total ?? 0)}{(d.count ?? 1) > 1 ? ` · file ${d.idx}/${d.count}` : ""}</div>
        </>
      )}
      {d?.state === "error" && <div class="error small">{d.error}</div>}
      {d?.state === "cancelled" && <div class="menu-sub">Cancelled. Download again to resume.</div>}
      {err && <div class="error small">{err}</div>}
    </div>
  );
}

function Repo({ m, dest }: { m: HfModel; dest: string }) {
  const [open, setOpen] = useState(false);
  const [groups, setGroups] = useState<HfGroup[] | null>(null);
  const [err, setErr] = useState("");
  const toggle = async () => {
    setOpen(!open);
    if (!open && groups === null) {
      try {
        setGroups(await api.hfFiles(m.id));
      } catch (e) {
        setErr(String(e));
        setGroups([]);
      }
    }
  };
  return (
    <div class="hf-repo">
      <button class="hf-head" aria-expanded={open} onClick={() => void toggle()}>
        <span class="grow">
          <span class="menu-name ellipsis">{m.id}</span>
          {m.downloads > 0 && (
            <span class="menu-sub">
              ↓ {fmtN(m.downloads)} · ♥ {fmtN(m.likes)}
              {m.pipeline === "image-text-to-text" ? " · vision" : ""}
              {m.updated ? ` · ${ago(m.updated)}` : ""}
            </span>
          )}
        </span>
        <span style={{ transform: open ? "rotate(180deg)" : "none", transition: "transform 160ms", display: "inline-flex" }}><Icon name="chevron" size={14} /></span>
      </button>
      {open && (
        <div class="hf-files">
          {groups === null && <div class="hint">Loading files…</div>}
          {err && <div class="error small">{err}</div>}
          {groups?.length === 0 && !err && <div class="hint">No .gguf files in this repo.</div>}
          {groups?.map((g) => <Group key={g.label} repo={m.id} g={g} dest={dest} />)}
        </div>
      )}
    </div>
  );
}

export function HubPanel() {
  const s = settings.value;
  const [q, setQ] = useState("");
  const [res, setRes] = useState<HfModel[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [destIdx, setDestIdx] = useState(0);
  const dirs = s?.model_dirs ?? [];
  const dest = dirs[destIdx] ?? dirs[0] ?? "";

  const search = async () => {
    if (!q.trim()) return;
    setBusy(true);
    setErr("");
    try {
      setRes(await api.hfSearch(q));
    } catch (e) {
      setErr(String(e));
      setRes(null);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="panel-body">
      <form class="searchbar" onSubmit={(e) => { e.preventDefault(); void search(); }}>
        <Icon name="search" size={15} />
        <input placeholder="Search GGUF models, or paste owner/name or a link" value={q} onInput={(e) => {
            const v = (e.target as HTMLInputElement).value;
            setQ(v);
            if (!v.trim()) {
              setRes(null); // back to the lists
              setErr("");
            }
          }}
          aria-label="Search Hugging Face" />
        <button class="primary" type="submit" disabled={busy || !q.trim()}>{busy ? "…" : "Search"}</button>
      </form>
      <div class="row">
        <label class="inline grow" title="Where downloaded models are saved">
          Save to
          <select value={destIdx} onChange={(e) => setDestIdx(Number((e.target as HTMLSelectElement).value))}>
            {dirs.map((d, i) => <option value={i} key={d}>{d}</option>)}
          </select>
        </label>
      </div>
      {err && <div class="error">{err}</div>}
      {res?.length === 0 && <div class="hint">No GGUF models found for “{q}”.</div>}
      {res === null && !err && <Browse dest={dest} />}
      {res?.map((m) => <Repo key={m.id} m={m} dest={dest} />)}
      <details class="fold">
        <summary>Hugging Face token (for gated models)</summary>
        <input
          type="password" placeholder="hf_…" value={s?.hf_token ?? ""}
          onChange={(e) => void saveSettings({ hf_token: (e.target as HTMLInputElement).value.trim() })}
          aria-label="Hugging Face token"
        />
        <div class="hint">Stored in Windows Credential Manager. Only sent to huggingface.co.</div>
      </details>
    </div>
  );
}
