import { render } from "preact";
import { invoke } from "@tauri-apps/api/core";
import { App } from "./App";
import { initState } from "./state";
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/600.css";
import "./styles.css";

// links in rendered markdown open in the system browser instead of replacing the app
document.addEventListener("click", (ev) => {
  const t = ev.target as HTMLElement;
  const a = t.closest("a") as HTMLAnchorElement | null;
  if (a && /^https?:\/\//i.test(a.href)) {
    ev.preventDefault();
    void invoke("open_url", { url: a.href });
    return;
  }
  const btn = t.closest("button.copy") as HTMLButtonElement | null;
  if (btn) {
    const code = btn.parentElement?.querySelector("code")?.textContent ?? "";
    void navigator.clipboard.writeText(code).then(() => {
      btn.textContent = "Copied";
      setTimeout(() => (btn.textContent = "Copy"), 1200);
    });
  }
});

async function boot() {
  // `npm run dev` in a normal browser: no Tauri backend, so use a mock one with sample data
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
    const { installMock } = await import("./dev/mock");
    installMock();
  }
  render(<App />, document.getElementById("app")!);
  await initState();
}

void boot();
