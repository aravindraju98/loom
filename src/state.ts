import { signal, computed } from "@preact/signals";
import { api, onAgentEvent, onEvent, onHfProgress } from "./api";
import type { HfProgress, Chat, ChatSummary, EngineStatus, McpServer, ModelEntry, Msg, Settings, AgentEvent } from "./api";

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
export const lastUsage = signal<{ tokens: number; tok_s: number } | null>(null);
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

export async function saveSettings(s: Settings) {
  await api.settingsSet(s);
  settings.value = s;
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
  await api.engineUnload();
  await refreshEngine();
}

export async function openChat(id: string) {
  if (busy.value) return;
  chat.value = await api.chatLoad(id);
}

export function startNewChat() {
  if (busy.value) return;
  chat.value = newChat();
  attachments.value = [];
}

export async function removeChat(id: string) {
  await api.chatDelete(id).catch(() => {});
  if (chat.value.id === id) chat.value = newChat();
  await refreshChats();
}

async function persist() {
  const c = chat.value;
  if (c.messages.length === 0) return;
  await api.chatSave(c.id, c);
  await refreshChats();
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
        if (!pv.binary) parts.push(`File: ${p}\n\`\`\`\n${pv.text}${pv.truncated ? "\n...[truncated]" : ""}\n\`\`\``);
      } catch {
        /* unreadable: skip */
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
    runError.value = String(e);
    runId.value = null;
  }
}

export async function stop() {
  if (runId.value) await api.agentCancel(runId.value);
}

export async function answerPermission(allow: boolean, always: boolean) {
  const p = pending.value;
  if (!p) return;
  pending.value = null;
  await api.permissionReply(p.call_id, chat.value.id, p.name, allow, always);
}

function onAgent(e: AgentEvent) {
  if (e.run_id !== runId.value) return;
  switch (e.type) {
    case "delta":
      live.value =
        e.kind === "content"
          ? { ...live.value, content: live.value.content + e.text }
          : { ...live.value, reasoning: live.value.reasoning + e.text };
      break;
    case "tool_progress": {
      const prev = toolProgress.value;
      toolProgress.value = { name: e.name, chars: e.chars, since: prev && prev.name === e.name ? prev.since : Date.now() };
      break;
    }
    case "message": {
      toolProgress.value = null;
      const m = e.message;
      if (m.role === "assistant" && lastUsage.value) {
        m.ui = { ...(m.ui ?? {}), tok_s: lastUsage.value.tok_s, tokens: lastUsage.value.tokens };
      }
      pushMsg(m);
      live.value = { content: "", reasoning: "" };
      break;
    }
    case "usage":
      lastUsage.value = { tokens: e.tokens, tok_s: e.tok_s };
      break;
    case "permission":
      pending.value = { call_id: e.call_id, name: e.name, args: e.args };
      break;
    case "tool_result":
      if (pending.value?.call_id === e.call_id) pending.value = null;
      break;
    case "error":
      runError.value = e.message;
      finish();
      break;
    case "done":
      if (e.reason === "max_turns") runError.value = "Stopped: reached the maximum number of tool turns.";
      finish();
      break;
  }
}

function finish() {
  runId.value = null;
  toolProgress.value = null;
  pending.value = null;
  live.value = { content: "", reasoning: "" };
  void persist();
}

export async function initState() {
  await refreshSettings();
  await Promise.all([refreshModels(), refreshEngine(), refreshChats(), refreshMcp()]);
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
