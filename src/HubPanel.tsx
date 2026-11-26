import { useState } from "preact/hooks";
import { api, fmtBytes } from "./api";
import type { HfGroup, HfModel } from "./api";
import { Icon } from "./icons";
import { downloads, saveSettings, settings } from "./state";

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
          {m.downloads > 0 && <span class="menu-sub">↓ {fmtN(m.downloads)} · ♥ {fmtN(m.likes)}</span>}
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
        <input placeholder="Search GGUF models, or paste owner/name or a link" value={q} onInput={(e) => setQ((e.target as HTMLInputElement).value)} aria-label="Search Hugging Face" />
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
      {res === null && !err && <div class="hint">Try “qwen3”, “gemma”, or paste a repo like unsloth/Qwen3-8B-GGUF. Large files can take a while; you can leave this tab and downloads continue.</div>}
      {res?.map((m) => <Repo key={m.id} m={m} dest={dest} />)}
      <details class="fold">
        <summary>Hugging Face token (for gated models)</summary>
        <input
          type="password" placeholder="hf_…" value={s?.hf_token ?? ""}
          onChange={(e) => s && void saveSettings({ ...s, hf_token: (e.target as HTMLInputElement).value.trim() })}
          aria-label="Hugging Face token"
        />
        <div class="hint">Stored in plain text in settings.json. Only sent to huggingface.co.</div>
      </details>
    </div>
  );
}
