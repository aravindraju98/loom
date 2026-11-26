import { signal, computed } from "@preact/signals";
import { api, onAgentEvent, onEvent, onHfProgress } from "./api";
import type { HfProgress, Chat, ChatSummary, EngineStatus, McpServer, ModelEntry, Msg, Settings, AgentEvent, Usage } from "./api";

/* ---------- notifications ---------- */

export type ToastKind = "info" | "success" | "error";
export interface Toast { id: number; kind: ToastKind; text: string }
export const toasts = signal<Toast[]>([]);
let toastSeq = 0;

export function dismissToast(id: number) {
  toasts.value = toasts.value.filter((t) => t.id !== id);
}

/** Show a short message. Errors stay until dismissed; others fade after a few seconds. */
export function toast(text: string, kind: ToastKind = "info") {
  const id = ++toastSeq;
  toasts.value = [...toasts.value.slice(-4), { id, kind, text }];
  if (kind !== "error") setTimeout(() => dismissToast(id), 4000);
}

export function errText(e: unknown): string {
  if (e instanceof Error) return e.message;
  return typeof e === "string" ? e : JSON.stringify(e);
}

export const settings = signal<Settings | null>(null);
export const models = signal<ModelEntry[]>([]);
export const modelsLoading = signal(true);
export const scanError = signal("");
export const engine = signal<EngineStatus>({
  state: "idle", engine: "", model_id: "", model_name: "", port: 0, base_url: "", error: "", uptime_secs: 0,
});
export const chats = signal<ChatSummary[]>([]);
export const chat = signal<Chat>(newChat());
export const mcp = signal<McpServer[]>([]);
export const loadError = signal("");

export const runId = signal<string | null>(null);
export const live = signal({ content: "", reasoning: "" });
export const pending = signal<{ call_id: string; name: string; args: unknown } | null>(null);
export const runError = signal("");
/** a tool call the model is currently generating (can take minutes for big files) */
export const toolProgress = signal<{ name: string; chars: number; since: number } | null>(null);
export const lastUsage = signal<Usage | null>(null);
/** a short status line for the running turn ("Retrying in 2s…", "Reading prompt…") */
export const runStatus = signal("");
/** call id of the tool being executed right now */
export const runningTool = signal<{ call_id: string; name: string; since: number } | null>(null);
export const useTools = signal(true);
export const attachments = signal<string[]>([]);
export const settingsOpen = signal(false);
export type Tab = "models" | "hub" | "server" | "files" | "mcp";
export const rightTab = signal<Tab>("models");
/** active/finished Hugging Face downloads by job id */
export const downloads = signal<Record<string, HfProgress>>({});

export const busy = computed(() => runId.value !== null);

function uid(): string {
  return crypto.randomUUID().replace(/-/g, "");
}

export function newChat(): Chat {
  return { id: uid(), title: "New chat", updated: Date.now(), messages: [] };
}

export async function refreshSettings() {
  settings.value = await api.settingsGet();
}

/** Fields of `next` that differ from `base` (shallow; arrays/objects compared by JSON). */
export function changedFields(base: Settings, next: Settings): Partial<Settings> {
  const out: Partial<Settings> = {};
  for (const k of Object.keys(next) as (keyof Settings)[]) {
    if (JSON.stringify(base[k]) !== JSON.stringify(next[k])) (out as Record<string, unknown>)[k] = next[k];
  }
  return out;
}

/**
 * Save only the given fields. The backend merges them into what is on disk, so two panels
 * saving different fields never overwrite each other. Returns false (and shows why) on failure.
 */
export async function saveSettings(patch: Partial<Settings>): Promise<boolean> {
  if (Object.keys(patch).length === 0) return true;
  try {
    settings.value = await api.settingsPatch(patch);
    return true;
  } catch (e) {
    toast(`Could not save settings: ${errText(e)}`, "error");
    return false;
  }
}

export async function refreshModels() {
  modelsLoading.value = true;
  scanError.value = "";
  try {
    models.value = await api.modelsScan();
  } catch (e) {
    scanError.value = String(e);
  } finally {
    modelsLoading.value = false;
  }
}

export async function refreshEngine() {
  engine.value = await api.engineStatus();
}

export async function refreshChats() {
  chats.value = await api.chatList();
}

export async function refreshMcp() {
  mcp.value = await api.mcpStatus();
}

export async function loadModel(id: string, eng?: string) {
  loadError.value = "";
  try {
    await api.engineLoad(id, eng, settings.value?.ctx);
  } catch (e) {
    loadError.value = String(e);
  } finally {
    await refreshEngine();
  }
}

export async function unloadModel() {
  try {
    await api.engineUnload();
  } catch (e) {
    toast(`Could not unload the model: ${errText(e)}`, "error");
  }
  await refreshEngine();
}

export async function openChat(id: string) {
  if (busy.value) return;
  try {
    chat.value = await api.chatLoad(id);
    runError.value = "";
  } catch (e) {
    toast(errText(e), "error");
  }
}

export function startNewChat() {
  if (busy.value) return;
  chat.value = newChat();
  attachments.value = [];
  runError.value = "";
}

export async function removeChat(id: string) {
  try {
    await api.chatDelete(id);
  } catch (e) {
    toast(errText(e), "error");
    return;
  }
  if (chat.value.id === id) chat.value = newChat();
  await refreshChats();
}

async function persist() {
  const c = chat.value;
  if (c.messages.length === 0) return;
  try {
    await api.chatSave(c.id, c);
    await refreshChats();
  } catch (e) {
    toast(`Chat not saved: ${errText(e)}`, "error");
  }
}

function pushMsg(m: Msg) {
  chat.value = { ...chat.value, messages: [...chat.value.messages, m], updated: Date.now() };
}

export async function send(text: string) {
  const s = settings.value;
  if (!s || busy.value || !text.trim()) return;
  let body = text.trim();
  const atts = attachments.value;
  if (atts.length) {
    const parts: string[] = [];
    for (const p of atts) {
      try {
        const pv = await api.fsRead(p);
        if (pv.binary) toast(`Skipped ${p}: binary file`, "info");
        else parts.push(`File: ${p}\n\`\`\`\n${pv.text}${pv.truncated ? "\n...[truncated]" : ""}\n\`\`\``);
      } catch (e) {
        toast(`Could not attach ${p}: ${errText(e)}`, "error");
      }
    }
    if (parts.length) body = parts.join("\n\n") + "\n\n" + body;
  }
  const first = chat.value.messages.length === 0;
  pushMsg({ role: "user", content: body, ui: atts.length ? { attachments: atts } : undefined });
  if (first) chat.value = { ...chat.value, title: text.trim().slice(0, 48) };
  attachments.value = [];
  await persist();

  const id = uid();
  runId.value = id;
  live.value = { content: "", reasoning: "" };
  runError.value = "";
  runStatus.value = "Reading prompt…";
  pending.value = null;
  try {
    await api.agentRun({
      run_id: id,
      chat_id: chat.value.id,
      messages: chat.value.messages,
      workspace: s.workspace,
      system_prompt: s.system_prompt,
      temperature: s.temperature,
      max_turns: s.max_turns,
      auto_approve: s.auto_approve,
      use_tools: useTools.value,
    });
  } catch (e) {
    runError.value = errText(e);
    runId.value = null;
    runStatus.value = "";
  }
}

export async function stop() {
  if (runId.value) await api.agentCancel(runId.value);
}

export async function answerPermission(allow: boolean, always: boolean) {
  const p = pending.value;
  if (!p) return;
  pending.value = null;
  try {
    await api.permissionReply(p.call_id, chat.value.id, p.name, allow, always);
  } catch (e) {
    toast(errText(e), "error");
  }
}

/** Turn backend error strings into something a person can act on. */
export function friendlyError(msg: string): string {
  if (msg.startsWith("context_overflow:")) return msg.slice("context_overflow:".length).trim();
  if (/could not reach the engine/i.test(msg)) return "The model server is not responding. Check the Models tab; it may have crashed or still be loading.";
  return msg;
}

let usageForTurn: Usage | null = null;

function onAgent(e: AgentEvent) {
  if (e.run_id !== runId.value) return;
  switch (e.type) {
    case "delta":
      runStatus.value = "";
      live.value =
        e.kind === "content"
          ? { ...live.value, content: live.value.content + e.text }
          : { ...live.value, reasoning: live.value.reasoning + e.text };
      break;
    case "first_token":
      runStatus.value = "";
      break;
    case "tool_progress": {
      runStatus.value = "";
      const prev = toolProgress.value;
      toolProgress.value = { name: e.name, chars: e.chars, since: prev && prev.name === e.name ? prev.since : Date.now() };
      break;
    }
    case "retry":
      runStatus.value = `Engine busy or unreachable. Retrying in ${Math.round(e.wait_ms / 1000)}s (${e.attempt}/${e.max})…`;
      break;
    case "notice":
      toast(e.text, "info");
      break;
    case "message": {
      toolProgress.value = null;
      const m = e.message;
      if (m.role === "assistant" && usageForTurn) {
        const u = usageForTurn;
        m.ui = {
          ...(m.ui ?? {}),
          tok_s: u.tok_s,
          tokens: u.tokens,
          prompt_tokens: u.prompt_tokens ?? undefined,
          cache_n: u.cache_n ?? undefined,
          prompt_ms: u.prompt_ms ?? undefined,
        };
        usageForTurn = null;
      }
      if (m.role === "tool") runningTool.value = null;
      pushMsg(m);
      live.value = { content: "", reasoning: "" };
      if (m.role === "tool") runStatus.value = "Reading tool result…";
      break;
    }
    case "usage": {
      const u: Usage = { tokens: e.tokens, tok_s: e.tok_s, prompt_tokens: e.prompt_tokens, prompt_n: e.prompt_n, prompt_ms: e.prompt_ms, cache_n: e.cache_n, ctx: e.ctx };
      lastUsage.value = u;
      usageForTurn = u;
      break;
    }
    case "tool_start":
      runStatus.value = "";
      runningTool.value = { call_id: e.call_id, name: e.name, since: Date.now() };
      break;
    case "permission":
      pending.value = { call_id: e.call_id, name: e.name, args: e.args };
      break;
    case "tool_result":
      if (pending.value?.call_id === e.call_id) pending.value = null;
      break;
    case "error":
      runError.value = friendlyError(e.message);
      finish();
      break;
    case "done":
      if (e.reason === "max_turns") runError.value = "Stopped: reached the maximum number of tool turns. Send a message to continue.";
      else if (e.reason === "length") runError.value = "The reply hit the output length limit and was cut off.";
      finish();
      break;
  }
}

function finish() {
  runId.value = null;
  toolProgress.value = null;
  runningTool.value = null;
  runStatus.value = "";
  pending.value = null;
  usageForTurn = null;
  live.value = { content: "", reasoning: "" };
  void persist();
}

export async function initState() {
  try {
    await refreshSettings();
  } catch (e) {
    toast(`Could not read settings: ${errText(e)}`, "error");
  }
  const results = await Promise.allSettled([refreshModels(), refreshEngine(), refreshChats(), refreshMcp()]);
  for (const r of results) if (r.status === "rejected") toast(errText(r.reason), "error");
  await onAgentEvent(onAgent);
  await onHfProgress((p) => {
    downloads.value = { ...downloads.value, [p.id]: p };
    if (p.state === "done") void refreshModels();
  });
  await onEvent("engine-changed", () => void refreshEngine());
  await onEvent("mcp-changed", () => void refreshMcp());
  // engine state changes while loading are pushed; poll as a safety net
  setInterval(() => void refreshEngine(), 5000);
}
