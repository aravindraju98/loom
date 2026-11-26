import { useEffect, useRef, useState } from "preact/hooks";
import { fmtBytes } from "./api";
import { Icon } from "./icons";
import { engine, loadModel, models, rightTab, settings, unloadModel } from "./state";

const BADGE: Record<string, string> = { polystrata: "PolyStrata", llama: "llama.cpp", openai: "Remote" };

/** Top-bar dropdown: current model + one-click switching between local models and remote endpoints. */
export function ModelPicker() {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const e = engine.value;
  const providers = settings.value?.providers ?? [];

  useEffect(() => {
    if (!open) return;
    const away = (ev: MouseEvent) => !box.current?.contains(ev.target as Node) && setOpen(false);
    const esc = (ev: KeyboardEvent) => ev.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("mousedown", away);
      document.removeEventListener("keydown", esc);
    };
  }, [open]);

  const pick = (id: string, eng?: string) => {
    setOpen(false);
    void loadModel(id, eng);
  };
  const label = e.state === "idle" || e.state === "error" ? "Select a model" : e.model_name || "…";

  return (
    <div class="picker" ref={box}>
      <button class="picker-btn" aria-haspopup="listbox" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span class={`dot ${e.state}`} />
        <span class="picker-label ellipsis">{label}</span>
        {e.state !== "idle" && e.state !== "error" && <span class="badge">{BADGE[e.engine] ?? e.engine}</span>}
        {e.state === "loading" && <span class="spin" aria-label="loading" />}
        <Icon name="chevron" size={14} />
      </button>
      {open && (
        <div class="picker-menu" role="listbox">
          {models.value.length > 0 && <div class="menu-title">Local models</div>}
          {models.value.map((m) => {
            const active = e.model_id === m.id && e.state !== "idle";
            return (
              <button key={m.id} class={`menu-item ${active ? "sel" : ""}`} role="option" aria-selected={active} onClick={() => (active ? setOpen(false) : pick(m.id, m.engine))}>
                <span class="grow">
                  <span class="menu-name ellipsis">{m.name}</span>
                  <span class="menu-sub">{m.quant} · {fmtBytes(m.size_bytes)}{m.moe ? " · MoE" : ""}{m.mmproj ? " · vision" : ""}</span>
                </span>
                <span class="badge">{BADGE[m.engine]}</span>
                {active && <Icon name="check" size={14} />}
              </button>
            );
          })}
          {providers.length > 0 && <div class="menu-title">Remote endpoints</div>}
          {providers.map((p) => {
            const active = e.model_id === `remote:${p.id}`;
            return (
              <button key={p.id} class={`menu-item ${active ? "sel" : ""}`} role="option" aria-selected={active} onClick={() => pick(`remote:${p.id}`)}>
                <span class="grow">
                  <span class="menu-name ellipsis">{p.name}</span>
                  <span class="menu-sub">{p.model || "no model chosen"}</span>
                </span>
                <Icon name="cloud" size={14} />
                {active && <Icon name="check" size={14} />}
              </button>
            );
          })}
          {models.value.length === 0 && providers.length === 0 && <div class="menu-empty">No models yet. Download one from the Hub tab or add a remote endpoint.</div>}
          <div class="menu-foot">
            <button onClick={() => { setOpen(false); rightTab.value = "hub"; }}><Icon name="download" size={14} /> Get models</button>
            <button onClick={() => { setOpen(false); rightTab.value = "models"; }}><Icon name="cube" size={14} /> Manage</button>
            {(e.state === "ready" || e.state === "loading") && <button class="danger" onClick={() => { setOpen(false); void unloadModel(); }}><Icon name="stop" size={14} /> Unload</button>}
          </div>
        </div>
      )}
    </div>
  );
}
