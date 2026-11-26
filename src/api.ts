import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface Settings {
  model_dirs: string[];
  polystrata_dir: string;
  llama_server: string;
  workspace: string;
  ctx: number;
  max_turns: number;
  auto_approve: boolean;
  temperature: number;
  system_prompt: string;
  engine_overrides: Record<string, string>;
  serve_on_network: boolean;
  api_key: string;
  poly_port: number;
  llama_port: number;
  gpu_layers: number;
  flash_attn: "auto" | "on" | "off";
  cache_type_k: string;
  cache_type_v: string;
  parallel: number;
  llama_extra_args: string;
  providers: Provider[];
  hf_token: string;
}

export interface Provider {
  id: string;
  name: string;
  base_url: string;
  api_key: string;
  model: string;
}

export interface HfModel { id: string; downloads: number; likes: number }
export interface HfPart { path: string; size: number }
export interface HfGroup { label: string; kind: "model" | "mmproj"; files: HfPart[]; total: number }
export interface HfProgress {
  id: string;
  state: "downloading" | "done" | "error" | "cancelled";
  file?: string; idx?: number; count?: number; done?: number; total?: number; error?: string; dir?: string;
}

export interface ModelEntry {
  id: string;
  name: string;
  arch: string;
  quant: string;
  size_bytes: number;
  ctx_train: number;
  moe: boolean;
  parts: string[];
  mmproj: string | null;
  engine: "polystrata" | "llama";
  auto_engine: "polystrata" | "llama";
  profile: string | null;
}

export interface EngineStatus {
  state: "idle" | "loading" | "ready" | "error";
  engine: string;
  model_id: string;
  model_name: string;
  port: number;
  base_url: string;
  error: string;
  uptime_secs: number;
}

export interface ToolCall {
  id: string;
  type: "function";
  function: { name: string; arguments: string };
}

export interface Msg {
  role: "user" | "assistant" | "tool";
  content: string | null;
  reasoning_content?: string;
  tool_calls?: ToolCall[];
  tool_call_id?: string;
  name?: string;
  ui?: {
    ok?: boolean;
    tok_s?: number;
    tokens?: number;
    attachments?: string[];
    /** stopped by the user (partial answer, or a tool that never ran) */
    cancelled?: boolean;
    /** tool run time */
    ms?: number;
    /** prompt size and KV-cache reuse for the turn that produced this message */
    prompt_tokens?: number;
    cache_n?: number;
    prompt_ms?: number;
    /** user messages: restore point for file changes made while answering it */
    checkpoint_id?: string;
    /** tool messages: the file change this tool made */
    change?: FileChange;
    /** tool messages: output replaced by a stub in prompts to save context */
    elided?: boolean;
  };
}

export interface FileChange {
  path: string;
  kind: "edit" | "create" | "overwrite" | "append" | "move" | "delete";
  added?: number;
  removed?: number;
  diff?: string;
}

export interface Branch {
  /** index of the first message that was replaced */
  at: number;
  created: number;
  messages: Msg[];
}

export interface Chat {
  id: string;
  title: string;
  updated: number;
  messages: Msg[];
  /** conversation tails replaced by restoring or editing an earlier message */
  branches?: Branch[];
}

export interface PlanItem {
  path: string;
  action: "restore" | "delete" | "recreate" | "none" | "skip";
  conflict: boolean;
  added: number;
  removed: number;
}

export interface ChatSummary {
  id: string;
  title: string | null;
  updated: number;
}

export interface FsEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
}

export interface Preview {
  text: string;
  truncated: boolean;
  binary: boolean;
  size: number;
}

export interface McpTool {
  name: string;
  description: string;
}
export interface McpServer {
  name: string;
  transport: string;
  state: "connecting" | "ready" | "error" | "disabled";
  error: string;
  tools: McpTool[];
}

export interface RunArgs {
  run_id: string;
  chat_id: string;
  messages: Msg[];
  workspace: string;
  system_prompt: string;
  temperature: number;
  max_turns: number;
  auto_approve: boolean;
  use_tools: boolean;
  extra?: Record<string, unknown>;
  checkpoint_id?: string;
}

export type AgentEvent = { run_id: string } & (
  | { type: "delta"; kind: "content" | "reasoning"; text: string }
  | { type: "message"; message: Msg }
  | { type: "tool_progress"; name: string; chars: number }
  | { type: "tool_start"; call_id: string; name: string; args: unknown }
  | { type: "permission"; call_id: string; name: string; args: unknown; preview?: FileChange | null }
  | { type: "tool_result"; call_id: string; ok: boolean; output: string; ms?: number; change?: FileChange | null }
  | { type: "pruned"; call_ids: string[] }
  | {
      type: "usage";
      tokens: number;
      secs: number;
      tok_s: number;
      prompt_tokens?: number | null;
      prompt_n?: number | null;
      prompt_ms?: number | null;
      cache_n?: number | null;
      ctx?: number;
    }
  | { type: "first_token"; ms: number }
  | { type: "retry"; attempt: number; max: number; wait_ms: number; reason: string }
  | { type: "notice"; text: string }
  | { type: "done"; reason: "done" | "cancelled" | "max_turns" | "length" | string }
  | { type: "error"; message: string }
);

export interface Usage {
  tokens: number;
  tok_s: number;
  prompt_tokens?: number | null;
  prompt_n?: number | null;
  prompt_ms?: number | null;
  cache_n?: number | null;
  ctx?: number;
}

export const api = {
  settingsGet: () => invoke<Settings>("settings_get"),
  settingsSet: (settings: Settings) => invoke<void>("settings_set", { settings }),
  /** merge only these fields into the saved settings; returns the result */
  settingsPatch: (patch: Partial<Settings>) => invoke<Settings>("settings_patch", { patch }),
  chatList: () => invoke<ChatSummary[]>("chat_list"),
  chatLoad: (id: string) => invoke<Chat>("chat_load", { id }),
  chatSave: (id: string, chat: Chat) => invoke<void>("chat_save", { id, chat }),
  chatDelete: (id: string) => invoke<void>("chat_delete", { id }),
  modelsScan: () => invoke<ModelEntry[]>("models_scan"),
  engineStatus: () => invoke<EngineStatus>("engine_status"),
  engineLog: (lines = 150) => invoke<string[]>("engine_log", { lines }),
  engineLoad: (modelId: string, engine?: string, ctx?: number) =>
    invoke<void>("engine_load", { modelId, engine, ctx }),
  engineUnload: () => invoke<void>("engine_unload"),
  hfSearch: (query: string) => invoke<HfModel[]>("hf_search", { query }),
  hfFiles: (repo: string) => invoke<HfGroup[]>("hf_files", { repo }),
  hfDownload: (repo: string, files: HfPart[], destDir: string) => invoke<string>("hf_download", { repo, files, destDir }),
  hfCancel: (id: string) => invoke<void>("hf_cancel", { id }),
  providerModels: (baseUrl: string, apiKey: string) => invoke<string[]>("provider_models", { baseUrl, apiKey }),
  agentRun: (args: RunArgs) => invoke<void>("agent_run", { args }),
  agentCancel: (runId: string) => invoke<void>("agent_cancel", { runId }),
  permissionReply: (callId: string, chatId: string, tool: string, allow: boolean, always: boolean) =>
    invoke<void>("permission_reply", { callId, chatId, tool, allow, always }),
  checkpointPlan: (chatId: string, ids: string[]) => invoke<PlanItem[]>("checkpoint_plan", { chatId, ids }),
  checkpointRestore: (chatId: string, ids: string[], force: boolean) =>
    invoke<{ undo_id: string; restored: string[] }>("checkpoint_restore", { chatId, ids, force }),
  checkpointUndo: (chatId: string, undoId: string) => invoke<string[]>("checkpoint_undo", { chatId, undoId }),
  checkpointDiff: (chatId: string, ids: string[], path: string) => invoke<string>("checkpoint_diff", { chatId, ids, path }),
  checkpointTouched: (chatId: string, ids: string[]) => invoke<Record<string, number>>("checkpoint_touched", { chatId, ids }),
  mcpStatus: () => invoke<McpServer[]>("mcp_status"),
  mcpGetConfig: () => invoke<string>("mcp_get_config"),
  mcpSaveConfig: (text: string) => invoke<void>("mcp_save_config", { text }),
  mcpReload: () => invoke<void>("mcp_reload"),
  fsList: (path: string) => invoke<FsEntry[]>("fs_list", { path }),
  fsRead: (path: string) => invoke<Preview>("fs_read", { path }),
  fsReveal: (path: string) => invoke<void>("fs_reveal", { path }),
};

export const onAgentEvent = (cb: (e: AgentEvent) => void) => listen<AgentEvent>("agent-event", (e) => cb(e.payload));
export const onHfProgress = (cb: (e: HfProgress) => void) => listen<HfProgress>("hf-progress", (e) => cb(e.payload));
export const onEvent = (name: string, cb: () => void) => listen(name, () => cb());

export function fmtBytes(n: number): string {
  if (n >= 1 << 30) return (n / 2 ** 30).toFixed(1) + " GB";
  if (n >= 1 << 20) return (n / 2 ** 20).toFixed(0) + " MB";
  return (n / 1024).toFixed(0) + " KB";
}
