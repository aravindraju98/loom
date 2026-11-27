/**
 * Browser preview: a fake backend with realistic sample data, used only by `npm run dev` outside
 * Tauri (for UI work, screenshots and component tests). Never bundled into the app build.
 */
import { emit } from "@tauri-apps/api/event";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { Chat, LogEntry, Msg, Settings } from "../api";

const WS = "D:\\Projects\\demo-app";
const now = Date.now();

const settings: Settings = {
  model_dirs: ["D:\\models"],
  polystrata_dir: "",
  llama_server: "C:\\llama.cpp\\llama-server.exe",
  workspace: WS,
  ctx: 16384,
  max_turns: 25,
  auto_approve: false,
  temperature: 0.7,
  system_prompt: "You are a helpful assistant running locally.",
  engine_overrides: {},
  serve_on_network: false,
  api_key: "",
  poly_port: 8090,
  llama_port: 8081,
  gpu_layers: -1,
  flash_attn: "auto",
  cache_type_k: "q8_0",
  cache_type_v: "q8_0",
  parallel: 0,
  llama_extra_args: "",
  providers: [],
  hf_token: "",
  shell_allow: ["git status", "npm test"],
  shell_deny: [],
  trusted: [],
};

const diff = `--- a/src/utils/format.ts
+++ b/src/utils/format.ts
@@ -12,9 +12,11 @@ export function formatBytes(n: number): string {
   if (n < 1024) return \`\${n} B\`;
   const units = ["KB", "MB", "GB", "TB"];
-  let i = -1;
-  do { n /= 1024; i++; } while (n >= 1024);
-  return n.toFixed(1) + " " + units[i];
+  let i = 0;
+  let v = n / 1024;
+  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
+  return \`\${v.toFixed(v < 10 ? 1 : 0)} \${units[i]}\`;
 }

 export function formatDuration(ms: number): string {
`;

const messages: Msg[] = [
  { role: "user", content: "formatBytes shows \"1024.0 KB\" for 1 MiB. Can you fix it and keep the tests passing?", ui: { checkpoint_id: "cp1" } },
  {
    role: "assistant",
    content: "Let me look at the helper first.",
    tool_calls: [{ id: "t1", type: "function", function: { name: "read_file", arguments: JSON.stringify({ path: "src/utils/format.ts", outline: true }) } }],
    ui: { tok_s: 38.4, tokens: 41, prompt_tokens: 3120, cache_n: 0, prompt_ms: 1480 },
  },
  {
    role: "assistant", content: "",
    tool_calls: [{ id: "t0", type: "function", function: { name: "update_plan", arguments: "{}" } }],
  },
  {
    role: "tool", tool_call_id: "t0", name: "update_plan", content: "Plan updated",
    ui: { ok: true, ms: 0, plan: [
      { step: "Read formatBytes", status: "done" },
      { step: "Fix the unit loop", status: "done" },
      { step: "Run the format tests", status: "in_progress" },
      { step: "Summarise the change", status: "pending" },
    ] },
  },
  { role: "tool", tool_call_id: "t1", name: "read_file", content: "src/utils/format.ts · outline · 48 lines · version 3f9a2c1b\n   11\texport function formatBytes(n: number): string {\n   23\texport function formatDuration(ms: number): string {", ui: { ok: true, ms: 4 } },
  {
    role: "assistant",
    content: "The loop divides once too often when the value is exactly 1024. I'll rewrite that part.",
    reasoning_content: "The do/while compares after dividing, so values just under the next unit are reported one unit too low. Start from the first unit and compare before dividing.",
    tool_calls: [{ id: "t2", type: "function", function: { name: "edit_file", arguments: JSON.stringify({ path: "src/utils/format.ts", old_string: "  let i = -1;", new_string: "  let i = 0;" }) } }],
    ui: { tok_s: 36.9, tokens: 212, prompt_tokens: 3410, cache_n: 3120, prompt_ms: 96 },
  },
  {
    role: "tool", tool_call_id: "t2", name: "edit_file",
    content: "Edited src/utils/format.ts (+4 -3, now 49 lines) · version 9b1e04d2",
    ui: { ok: true, ms: 7, change: { path: "src/utils/format.ts", kind: "edit", added: 4, removed: 3, diff } },
  },
  {
    role: "assistant",
    content: "",
    tool_calls: [{ id: "t3", type: "function", function: { name: "run_shell", arguments: JSON.stringify({ command: "npm test -- format" }) } }],
    ui: { tok_s: 37.2, tokens: 24, prompt_tokens: 3560, cache_n: 3410, prompt_ms: 61 },
  },
  { role: "tool", tool_call_id: "t3", name: "run_shell", content: " PASS  src/utils/format.test.ts\n  ✓ formats bytes (3 ms)\n  ✓ formats durations (1 ms)\n\nTests: 2 passed, 2 total\n[exit code 0]", ui: { ok: true, ms: 2140 } },
  {
    role: "assistant",
    content: "Fixed. `formatBytes` now divides before comparing, caps at TB and drops the decimal for values ≥ 10:\n\n```ts\nformatBytes(1048576) // \"1.0 MB\"\nformatBytes(52428800) // \"50 MB\"\n```\n\nBoth tests pass.",
    ui: { tok_s: 37.8, tokens: 96, prompt_tokens: 3770, cache_n: 3560, prompt_ms: 74 },
  },
];

const chat: Chat = { id: "c1", title: "Fix formatBytes rounding", updated: now, messages };

const logs: LogEntry[] = [
  { seq: 1, ts: now - 90_000, level: "info", source: "app", msg: "Harness 0.1.0 started" },
  { seq: 2, ts: now - 89_000, level: "info", source: "engine", msg: "loading model…" },
  { seq: 3, ts: now - 88_500, level: "info", source: "engine", msg: "llama_model_loader: loaded meta data with 36 key-value pairs from qwen2.5-coder-7b-instruct-q4_k_m.gguf" },
  { seq: 4, ts: now - 80_000, level: "info", source: "engine", msg: "srv  load_model: context size 16384, n_slots 1" },
  { seq: 5, ts: now - 79_800, level: "info", source: "engine", msg: "engine ready" },
  { seq: 6, ts: now - 79_000, level: "warn", source: "mcp:github", msg: "GITHUB_TOKEN not set; only public repositories are available" },
  { seq: 7, ts: now - 78_500, level: "info", source: "mcp:github", msg: "connected, 26 tools" },
  { seq: 8, ts: now - 60_000, level: "info", source: "agent", msg: "run started", run_id: "r1" },
  { seq: 9, ts: now - 52_000, level: "warn", source: "agent", msg: "edit_file failed: old_string not found exactly. Closest text is at lines 14-16 (2 of 3 lines match).", run_id: "r1" },
  { seq: 10, ts: now - 40_000, level: "info", source: "agent", msg: "run finished: done", run_id: "r1" },
  { seq: 11, ts: now - 20_000, level: "error", source: "mcp:sqlite", msg: "failed to connect: could not start uvx: program not found" },
];

const run = {
  summary: {
    run_id: "r1", chat_id: "c1", started: now - 60_000, ms: 19_850, reason: "done", error: null,
    model: "qwen2.5-coder-7b-instruct-q4_k_m", engine: "llama", ctx: 16384, tier: "small",
    turns: 4, tool_calls: 3, tool_errors: 0, prompt_tokens: 13_860, cached_tokens: 10_090, completion_tokens: 373, prompt_ms: 1711,
  },
  events: [
    { kind: "turn", t: 2600, secs: 2.6, ttft_ms: 1490, prompt_tokens: 3120, cache_n: 0, prompt_ms: 1480, tokens: 41, tok_s: 38.4 },
    { kind: "tool", t: 2610, name: "read_file", ok: true, ms: 4 },
    { kind: "turn", t: 8400, secs: 5.8, ttft_ms: 110, prompt_tokens: 3410, cache_n: 3120, prompt_ms: 96, tokens: 212, tok_s: 36.9 },
    { kind: "tool", t: 8410, name: "edit_file", ok: true, ms: 7, change: { path: "src/utils/format.ts", added: 4, removed: 3 } },
    { kind: "turn", t: 9200, secs: 0.8, ttft_ms: 70, prompt_tokens: 3560, cache_n: 3410, prompt_ms: 61, tokens: 24, tok_s: 37.2 },
    { kind: "tool", t: 11_350, name: "run_shell", ok: true, ms: 2140 },
    { kind: "turn", t: 14_000, secs: 2.6, ttft_ms: 85, prompt_tokens: 3770, cache_n: 3560, prompt_ms: 74, tokens: 96, tok_s: 37.8 },
  ],
};

const models = [
  { id: "m1", name: "qwen2.5-coder-7b-instruct", arch: "qwen2", quant: "Q4_K_M", size_bytes: 4.68e9, ctx_train: 32768, moe: false, parts: [], mmproj: null, engine: "llama", auto_engine: "llama", profile: null },
  { id: "m2", name: "Qwen3-30B-A3B-Instruct", arch: "qwen3moe", quant: "Q4_K_XL", size_bytes: 17.7e9, ctx_train: 262144, moe: true, parts: [], mmproj: null, engine: "llama", auto_engine: "llama", profile: null },
  { id: "m3", name: "gemma-3-12b-it", arch: "gemma3", quant: "Q5_K_M", size_bytes: 8.4e9, ctx_train: 131072, moe: false, parts: [], mmproj: "x", engine: "llama", auto_engine: "llama", profile: null },
];

const evalScenarios = [
  { id: "fix_bug", name: "Fix a one-line bug", prompt: "" },
  { id: "rename", name: "Rename across two files", prompt: "" },
  { id: "insert_large", name: "Add a function to a 300-line file", prompt: "" },
  { id: "json_multi", name: "Change two settings in JSON", prompt: "" },
  { id: "find_value", name: "Find a value in a large file", prompt: "" },
  { id: "create_file", name: "Create a new file", prompt: "" },
];

function evalBatch(model: string, started: number, pass: boolean[]) {
  const results = evalScenarios.map((s, i) => ({
    scenario: s.id, name: s.name, passed: pass[i], detail: pass[i] ? "" : "original line changed or lost", ms: 6000 + i * 2100, end: "done",
    turns: 3 + (i % 3), tool_calls: 3 + i, tool_errors: pass[i] ? 0 : 2, edit_failures: pass[i] ? 0 : 2, rewrites: pass[i] ? 0 : 1, recovered: i === 1 ? 1 : 0, completion_tokens: 300,
  }));
  return { id: `b${started}`, started, ms: 60000, model, engine: "llama", ctx: 16384, passed: pass.filter(Boolean).length, total: pass.length, cancelled: false, results };
}

const FORMAT_BEFORE = [
  "export function formatBytes(n: number): string {",
  "  if (n < 1024) return `${n} B`;",
  '  const units = ["KB", "MB", "GB", "TB"];',
  "  let i = -1;",
  "  do { n /= 1024; i++; } while (n >= 1024);",
  '  return n.toFixed(1) + " " + units[i];',
  "}",
  "",
  "export function formatDuration(ms: number): string {",
  "  return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`;",
  "}",
  "",
].join("\n");

const FORMAT_AFTER = FORMAT_BEFORE.replace(
  ["  let i = -1;", "  do { n /= 1024; i++; } while (n >= 1024);", '  return n.toFixed(1) + " " + units[i];'].join("\n"),
  ["  let i = 0;", "  let v = n / 1024;", "  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }", "  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;"].join("\n"),
);

/** a small fake workspace: relative path -> content */
const TREE: Record<string, string> = {
  "src\\utils\\format.ts": FORMAT_AFTER,
  "src\\utils\\format.test.ts": 'import { formatBytes } from "./format";\n\ntest("formats bytes", () => {\n  expect(formatBytes(1048576)).toBe("1.0 MB");\n});\n',
  "src\\main.tsx": 'import { render } from "preact";\nimport { App } from "./App";\n\nrender(<App />, document.getElementById("app")!);\n',
  "src\\App.tsx": "export function App() {\n  return <h1>demo</h1>;\n}\n",
  "src\\styles.css": "body {\n  margin: 0;\n  font: 14px system-ui;\n}\n",
  "Cargo.toml": '[package]\nname = "demo"\nversion = "0.1.0"\n',
  "package.json": '{\n  "name": "demo-app",\n  "version": "1.0.0",\n  "scripts": { "test": "vitest" }\n}\n',
  "README.md": "# demo-app\n\nA small demo.\n",
  ".gitignore": "dist\nnode_modules\n",
  "dist\\index.js": "console.log(1);\n",
};
const FILES: Record<string, string> = Object.fromEntries(Object.entries(TREE).map(([r, t]) => [`${WS}\\${r}`.toLowerCase(), t]));

function listDir(dir: string) {
  const rel = dir.length > WS.length ? dir.slice(WS.length + 1) : "";
  const prefix = rel ? `${rel}\\` : "";
  const seen = new Map<string, boolean>();
  for (const r of Object.keys(TREE)) {
    if (!r.startsWith(prefix)) continue;
    const rest = r.slice(prefix.length).split("\\");
    seen.set(rest[0], rest.length > 1 || seen.get(rest[0]) === true);
  }
  return [...seen.entries()]
    .map(([name, isDir]) => ({ name, path: `${dir}\\${name}`, is_dir: isDir, size: isDir ? 0 : (TREE[prefix + name]?.length ?? 0), ignored: name === "dist" }))
    .sort((x, y) => Number(y.is_dir) - Number(x.is_dir) || x.name.localeCompare(y.name));
}

export function installMock() {
  mockWindows("main");
  mockIPC(
    (cmd, args) => {
      const a = (args ?? {}) as Record<string, unknown>;
      switch (cmd) {
        case "settings_get": return settings;
        case "settings_patch": Object.assign(settings, a.patch); return settings;
        case "chat_list": return [{ id: "c1", title: chat.title, updated: now }, { id: "c2", title: "Add dark mode to settings page", updated: now - 3.6e6 }, { id: "c3", title: "Explain the build pipeline", updated: now - 8.64e7 }];
        case "chat_load": return chat;
        case "chat_save": case "chat_delete": return null;
        case "models_scan": return models;
        case "engine_status": return { state: "ready", engine: "llama", model_id: "m1", model_name: "qwen2.5-coder-7b-instruct", port: 8081, base_url: "http://127.0.0.1:8081", error: "", uptime_secs: 420 };
        case "engine_log": return logs.filter((l) => l.source === "engine").map((l) => l.msg);
        case "mcp_status": return [
          { name: "github", transport: "stdio", state: "ready", error: "", tools: [{ name: "search_repositories", description: "Search for GitHub repositories" }] },
          { name: "sqlite", transport: "stdio", state: "error", error: "could not start uvx: program not found", tools: [] },
        ];
        case "mcp_get_config": return '{\n  "mcpServers": {}\n}';
        case "checkpoint_touched": return { cp1: 1 };
        case "checkpoint_plan": return [{ path: `${WS}\\src\\utils\\format.ts`, action: "restore", conflict: false, added: 3, removed: 4 }];
        case "checkpoint_restore": return { undo_id: "undo_1", restored: [] };
        case "logs_tail": return logs;
        case "trace_list": return [run.summary, { ...run.summary, run_id: "r0", started: now - 7.2e6, ms: 4200, turns: 1, tool_calls: 0, reason: "error", error: "context_overflow: the conversation no longer fits", prompt_tokens: 0, cached_tokens: 0 }];
        case "trace_get": return run;
        case "eval_list": return evalScenarios;
        case "hf_browse": return [
          { id: "unsloth/Qwen3.6-35B-A3B-GGUF", downloads: 2_480_000, likes: 1450, pipeline: "image-text-to-text", updated: new Date(now - 3 * 8.64e7).toISOString() },
          { id: "bartowski/gemma-4-26B-A4B-it-GGUF", downloads: 912_000, likes: 610, pipeline: "image-text-to-text", updated: new Date(now - 9 * 8.64e7).toISOString() },
          { id: "unsloth/Qwen3.8-27B-GGUF", downloads: 8_180_000, likes: 7128, updated: new Date(now - 20 * 8.64e7).toISOString() },
          { id: "ggml-org/gpt-oss-20b-GGUF", downloads: 640_000, likes: 402, pipeline: "text-generation", updated: new Date(now - 70 * 8.64e7).toISOString() },
          { id: "lmstudio-community/Mistral-Small-4-24B-Instruct-GGUF", downloads: 210_000, likes: 188, pipeline: "text-generation", updated: new Date(now - 40 * 8.64e7).toISOString() },
        ];
        case "eval_results": return [evalBatch("qwen2.5-coder-7b-instruct-q4_k_m", now - 3.6e6, [true, true, false, true, true, true]), evalBatch("gemma-3-12b-it-Q5_K_M", now - 8.64e7, [true, false, false, true, true, true])];
        case "fs_list": return listDir(String(a.path));
        case "fs_open": {
          const p = String(a.path);
          const t = FILES[p.toLowerCase()];
          if (t === undefined) throw "file not found";
          return { path: p, text: t, version: `v${t.length}`, size: t.length, crlf: false, bom: false, encoding: "utf8", readonly: false };
        }
        case "fs_save": FILES[String(a.path).toLowerCase()] = String(a.text); return { version: `v${String(a.text).length}` };
        case "fs_watch": case "fs_create": case "fs_rename": case "fs_delete": case "fs_reveal": return null;
        case "checkpoint_file_base": return FORMAT_BEFORE;
        case "fs_read": return { text: "# demo-app\n\nA small demo.", truncated: false, binary: false, size: 30 };
        default: return null;
      }
    },
    { shouldMockEvents: true },
  );
  // open the sample chat once the app has loaded
  setTimeout(() => {
    void import("../state").then((s) => {
      // the app's own module instance, for scripted UI checks
      (window as unknown as { __state: typeof s }).__state = s;
      void s.openChat("c1");
    });
  }, 50);
  (window as unknown as { __mockEmit: typeof emit }).__mockEmit = emit;
}
