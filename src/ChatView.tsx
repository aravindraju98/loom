import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { Icon } from "./icons";
import type { FileChange, Msg, PlanStep } from "./api";
import { md } from "./markdown";
import {
  answerPermission, attachments, busy, chat, contextUse, currentPlan, draft, engine, live, pending, regenerate, restoreReq, runError,
  runStatus, runningTool, send, settings, stop, stripAttachments, thinking, toast, toolProgress, touched, useTools,
} from "./state";

function PlanList({ steps }: { steps: PlanStep[] }) {
  return (
    <ol class="plan">
      {steps.map((s, i) => (
        <li key={i} class={`plan-${s.status}`}>
          <span class="plan-box" aria-hidden="true">{s.status === "done" ? "✓" : ""}</span>
          <span>{s.step}</span>
          <span class="sr-only">{s.status === "done" ? " (done)" : s.status === "in_progress" ? " (in progress)" : ""}</span>
        </li>
      ))}
    </ol>
  );
}

/** The model's current checklist, pinned above the composer while it works. */
function PlanCard() {
  const p = currentPlan.value;
  const [open, setOpen] = useState(true);
  if (!p || (!busy.value && p.every((s) => s.status === "done"))) return null;
  const done = p.filter((s) => s.status === "done").length;
  return (
    <div class="plan-card">
      <button class="plan-head" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span class="grow">Plan · {done}/{p.length} done</span>
        <span class="plan-bar" aria-hidden="true"><i style={{ width: `${(done / p.length) * 100}%` }} /></span>
      </button>
      {open && <PlanList steps={p} />}
    </div>
  );
}

function k(n: number) {
  return n >= 1000 ? `${(n / 1000).toFixed(n >= 10_000 ? 0 : 1)}k` : String(n);
}

function ContextMeter() {
  const c = contextUse.value;
  if (!c) return null;
  const level = c.pct >= 85 ? "bad" : c.pct >= 70 ? "warn" : "";
  return (
    <span class={`ctx-meter ${level}`} title={`Context: ${c.used.toLocaleString()} of ${c.ctx.toLocaleString()} tokens. Old tool output is trimmed automatically near the limit.`}>
      <span class="ctx-bar" aria-hidden="true"><i style={{ width: `${c.pct}%` }} /></span>
      {k(c.used)} / {k(c.ctx)}
    </span>
  );
}
import { Diff, DiffStat } from "./ui/Diff";

function fmtMs(ms: number): string {
  return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)} s`;
}

/** "31.2 tok/s · 412 tokens · prompt 6.1k (92% cached) in 0.4 s" */
function turnMeta(u: NonNullable<Msg["ui"]>): string {
  const parts: string[] = [];
  if (u.tok_s) parts.push(`${u.tok_s.toFixed(1)} tok/s`);
  if (u.tokens) parts.push(`${u.tokens} tokens`);
  if (u.prompt_tokens) {
    const k = u.prompt_tokens >= 1000 ? `${(u.prompt_tokens / 1000).toFixed(1)}k` : String(u.prompt_tokens);
    let p = `prompt ${k}`;
    if (u.cache_n != null && u.prompt_tokens > 0) p += ` (${Math.round((u.cache_n / u.prompt_tokens) * 100)}% cached)`;
    if (u.prompt_ms) p += ` in ${fmtMs(Math.round(u.prompt_ms))}`;
    parts.push(p);
  }
  return parts.join(" · ");
}

function pretty(args: unknown): string {
  if (typeof args === "string") {
    try {
      return JSON.stringify(JSON.parse(args), null, 2);
    } catch {
      return args;
    }
  }
  return JSON.stringify(args, null, 2);
}

/** Markdown is parsed once per distinct text, not on every render. */
function Markdown({ text }: { text: string }) {
  const html = useMemo(() => md(text), [text]);
  return <div class="md" dangerouslySetInnerHTML={{ __html: html }} />;
}

/** Throttled markdown for the streaming message: at most one parse per animation frame. */
function LiveMarkdown({ text }: { text: string }) {
  const [shown, setShown] = useState(text);
  const frame = useRef(0);
  useEffect(() => {
    if (frame.current) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = 0;
      setShown(text);
    });
  }, [text]);
  useEffect(() => () => cancelAnimationFrame(frame.current), []);
  return <Markdown text={shown} />;
}

function Reasoning({ text, open }: { text: string; open?: boolean }) {
  if (!text) return null;
  return (
    <details class="reasoning" open={open}>
      <summary>Thinking</summary>
      <div class="reasoning-body">{text}</div>
    </details>
  );
}

const FILE_TOOLS = new Set(["edit_file", "edit_lines", "write_file", "move_file", "delete_file"]);

function argPath(args: string): string {
  try {
    const a = JSON.parse(args) as Record<string, unknown>;
    return String(a.path ?? a.from ?? a.command ?? a.pattern ?? "");
  } catch {
    return "";
  }
}

function ChangeView({ c }: { c: FileChange }) {
  return c.diff ? <Diff diff={c.diff} /> : <div class="hint">{c.kind === "delete" ? "File deleted." : c.kind === "move" ? `Moved ${c.path}` : "No diff."}</div>;
}

function ToolCard({ name, args, result, callId }: { name: string; args: string; result?: Msg; callId: string }) {
  const waiting = pending.value?.call_id === callId;
  const running = busy.value && !result && !waiting;
  const cancelled = result?.ui?.cancelled;
  const change = result?.ui?.change;
  const status = waiting
    ? "awaiting approval"
    : running
      ? runningTool.value?.call_id === callId ? "running…" : "queued"
      : !result
        ? "not run"
        : cancelled
          ? "cancelled"
          : `${result.ui?.ok === false ? "failed" : "done"}${result.ui?.ms != null ? ` · ${fmtMs(result.ui.ms)}` : ""}`;
  const cls = waiting ? "wait" : running ? "run" : cancelled || !result ? "skip" : result.ui?.ok === false ? "bad" : "ok";
  const target = change?.path ?? argPath(args);
  if (result?.ui?.plan) {
    const p = result.ui.plan;
    return (
      <details class="tool ok">
        <summary>
          <span class="tool-name">plan</span>
          <span class="tool-target">{p.filter((s) => s.status === "done").length}/{p.length} done</span>
        </summary>
        <div class="tool-plan"><PlanList steps={p} /></div>
      </details>
    );
  }
  return (
    <details class={`tool ${cls}`} open={waiting}>
      <summary>
        <span class="tool-name">{name}</span>
        {target && <span class="tool-target ellipsis" title={target}>{target}</span>}
        {change && <DiffStat added={change.added} removed={change.removed} />}
        <span class="tool-status">{status}</span>
      </summary>
      {waiting ? (
        <PermissionBar name={name} args={args} />
      ) : (
        <>
          {change ? <ChangeView c={change} /> : <pre class="tool-args">{pretty(args)}</pre>}
          {result && !change && (
            <pre class="tool-out">{result.content}</pre>
          )}
          {result && change && result.ui?.ok === false && <pre class="tool-out">{result.content}</pre>}
          {result?.ui?.elided && <div class="hint small">This output was removed from the model's context to save space.</div>}
        </>
      )}
    </details>
  );
}

function PermissionBar({ name, args }: { name: string; args: string }) {
  const p = pending.value;
  const preview = p?.preview;
  const shell = preview && preview.kind === "shell" ? preview : null;
  const file = preview && preview.kind !== "shell" ? preview : null;
  const [more, setMore] = useState(false);
  const what = FILE_TOOLS.has(name) ? "file changes" : name === "run_shell" ? "commands" : name;
  return (
    <div class="perm">
      {file ? (
        <>
          <div class="perm-head">
            <span>{file.kind === "create" ? "Create" : file.kind === "delete" ? "Delete" : file.kind === "move" ? "Move" : "Change"} <code>{file.path}</code></span>
            <DiffStat added={file.added} removed={file.removed} />
          </div>
          <ChangeView c={file} />
        </>
      ) : shell ? (
        <>
          {shell.dangerous && (
            <div class="perm-warn" role="alert">This command can change things outside the workspace or can't be undone. Read it carefully.</div>
          )}
          <pre class="tool-args shell-cmd">{shell.command}</pre>
        </>
      ) : (
        <pre class="tool-args">{pretty(args)}</pre>
      )}
      <div class="perm-actions">
        <button class="primary" data-autofocus onClick={() => void answerPermission(true, "once")}>Allow once</button>
        {!shell?.dangerous && (
          <div class="split">
            <button onClick={() => void answerPermission(true, "chat")}>Allow {what} in this chat</button>
            <button class="split-more" aria-label="More options" aria-expanded={more} onClick={() => setMore(!more)}><Icon name="chevron" size={12} /></button>
            {more && (
              <div class="menu-pop" role="menu">
                {shell ? (
                  <button role="menuitem" class="menu-item" onClick={() => void answerPermission(true, "prefix")}>
                    Always allow <code>{shell.prefix}</code> commands
                  </button>
                ) : (
                  <button role="menuitem" class="menu-item" onClick={() => void answerPermission(true, "workspace")}>
                    Always allow {what} in this workspace
                  </button>
                )}
              </div>
            )}
          </div>
        )}
        <span class="spacer" />
        <button class="danger" onClick={() => void answerPermission(false)}>Deny</button>
      </div>
    </div>
  );
}

function UserActions({ index, m }: { index: number; m: Msg }) {
  const files = m.ui?.checkpoint_id ? touched.value[m.ui.checkpoint_id] : undefined;
  const dis = busy.value;
  return (
    <div class="msg-actions">
      {files ? (
        <button class="chip-btn" disabled={dis} title="Put files and the conversation back to how they were before this message" onClick={() => (restoreReq.value = { index })}>
          <Icon name="refresh" size={12} /> Restore checkpoint · {files} file{files === 1 ? "" : "s"}
        </button>
      ) : (
        <button class="chip-btn" disabled={dis} title="Go back to before this message" onClick={() => (restoreReq.value = { index })}>
          <Icon name="refresh" size={12} /> Restore
        </button>
      )}
      <button class="chip-btn" disabled={dis} title="Edit this message and resend" onClick={() => (restoreReq.value = { index, edit: true })}>Edit</button>
      <button
        class="chip-btn"
        title="Copy text"
        onClick={() => void navigator.clipboard.writeText(stripAttachments(m.content ?? "")).then(() => toast("Copied", "info"))}
      >
        Copy
      </button>
    </div>
  );
}

function MessageView({ m, all, index }: { m: Msg; all: Msg[]; index: number }) {
  if (m.role === "tool") return null;
  if (m.role === "user") {
    return (
      <div class="msg user">
        {m.ui?.attachments?.length ? (
          <div class="chips">{m.ui.attachments.map((p) => <span class="chip" key={p}>{p.split("\\").pop()}</span>)}</div>
        ) : null}
        <div class="bubble"><Markdown text={stripAttachments(m.content ?? "")} /></div>
        <UserActions index={index} m={m} />
      </div>
    );
  }
  return (
    <div class="msg assistant">
      <Reasoning text={m.reasoning_content ?? ""} />
      {m.content ? <Markdown text={m.content} /> : null}
      {m.tool_calls?.map((tc) => (
        <ToolCard
          key={tc.id}
          callId={tc.id}
          name={tc.function.name}
          args={tc.function.arguments}
          result={all.find((x) => x.role === "tool" && x.tool_call_id === tc.id)}
        />
      ))}
      {(m.ui?.cancelled || m.ui?.tok_s || m.content) && (
        <div class="meta">
          {m.ui?.cancelled && <span class="badge">Stopped</span>}
          {m.ui?.tok_s ? <span>{turnMeta(m.ui)}</span> : null}
          {isLastAnswer(all, index) && (
            <span class="meta-actions">
              {m.content && (
                <button class="chip-btn" onClick={() => void navigator.clipboard.writeText(m.content ?? "").then(() => toast("Copied", "info"))}>Copy</button>
              )}
              <button class="chip-btn" disabled={busy.value} title="Ask again (files are left as they are)" onClick={() => void regenerate()}>
                <Icon name="refresh" size={12} /> Retry
              </button>
            </span>
          )}
        </div>
      )}
    </div>
  );
}

/** the final assistant message of the latest request (where Retry makes sense) */
function isLastAnswer(all: Msg[], index: number): boolean {
  for (let i = all.length - 1; i >= 0; i--) {
    if (all[i].role === "assistant") return i === index;
    if (all[i].role === "user") return false;
  }
  return false;
}

function Composer() {
  const text = draft.value;
  const setText = (v: string) => (draft.value = v);
  const ready = engine.value.state === "ready";
  const ta = useRef<HTMLTextAreaElement>(null);
  // focus the composer when a message is put back for editing
  useEffect(() => {
    if (text && document.activeElement !== ta.current) ta.current?.focus();
  }, [text === ""]);
  const submit = () => {
    if (!ready || busy.value || !text.trim()) return;
    const t = text;
    setText("");
    void send(t);
  };
  return (
    <div class="composer">
      {attachments.value.length > 0 && (
        <div class="chips">
          {attachments.value.map((p) => (
            <span class="chip" key={p}>
              {p.split("\\").pop()}
              <button class="x" aria-label="Remove attachment" onClick={() => (attachments.value = attachments.value.filter((x) => x !== p))}>×</button>
            </span>
          ))}
        </div>
      )}
      <textarea
        ref={ta}
        value={text}
        aria-label="Message"
        placeholder={ready ? "Message the model…  (Enter to send, Shift+Enter for newline)" : "Load a model from the Models tab to start"}
        onInput={(e) => setText((e.target as HTMLTextAreaElement).value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            submit();
          }
        }}
        rows={3}
      />
      <div class="composer-row">
        <button
          class={`icon-btn tool-toggle ${useTools.value ? "on" : ""}`}
          aria-pressed={useTools.value}
          aria-label="Tools"
          title={useTools.value ? (settings.value?.workspace ? "Tools on: file access and MCP (click to turn off)" : "Tools on: MCP only. Pick a workspace in the Files tab for file access") : "Tools off (click to turn on)"}
          onClick={() => (useTools.value = !useTools.value)}
        >
          <Icon name="wrench" />
          {useTools.value && !settings.value?.workspace && <i class="warn-dot" />}
        </button>
        {engine.value.engine !== "openai" && (
          <select
            class="mini think"
            aria-label="Thinking"
            title="For reasoning models (Qwen3 and similar). Off answers faster and saves context."
            value={thinking.value}
            onChange={(e) => (thinking.value = (e.target as HTMLSelectElement).value as typeof thinking.value)}
          >
            <option value="auto">Thinking: default</option>
            <option value="on">Thinking: on</option>
            <option value="off">Thinking: off</option>
          </select>
        )}
        <span class="spacer" />
        <ContextMeter />
        {busy.value ? (
          <button class="danger" onClick={() => void stop()}>Stop</button>
        ) : (
          <button class="primary send" aria-label="Send" disabled={!ready || !text.trim()} onClick={submit}>
            Send
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M5 12h14M13 6l6 6-6 6" /></svg>
          </button>
        )}
      </div>
    </div>
  );
}

export function ChatView() {
  const scroller = useRef<HTMLDivElement>(null);
  const msgs = chat.value.messages;
  const l = live.value;
  useEffect(() => {
    const el = scroller.current;
    if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 200) el.scrollTop = el.scrollHeight;
  }, [msgs.length, l.content, l.reasoning, pending.value]);
  return (
    <div class="chatview">
      <div class="messages" ref={scroller}>
        {msgs.length === 0 && !busy.value && (
          <div class="empty">
            <div class="empty-mark" aria-hidden="true"><Icon name="terminal" size={20} /></div>
            <h2>What are we building?</h2>
            <p>
              {engine.value.state === "ready"
                ? "Your model is ready. Ask anything, or let it work on files in your workspace."
                : "Load a local model or connect an endpoint from the Models tab to begin."}
            </p>
            <div class="suggest">
              {["Explain this project's structure", "Find and fix bugs in my workspace", "Write a README for this folder", "Summarize the largest files here"].map((t) => (
                <button key={t} class="suggestion" disabled={engine.value.state !== "ready"} onClick={() => (draft.value = t)}>{t}</button>
              ))}
            </div>
          </div>
        )}
        {msgs.map((m, i) => <MessageView key={`${chat.value.id}:${i}`} m={m} all={msgs} index={i} />)}
        {busy.value && (l.content || l.reasoning) && (
          <div class="msg assistant" aria-live="polite" aria-busy="true">
            <Reasoning text={l.reasoning} open />
            {l.content && <LiveMarkdown text={l.content} />}
          </div>
        )}
        {busy.value && toolProgress.value && (
          <div class="msg assistant">
            <div class="tool run">
              <div class="tool-gen">
                <span class="typing" aria-hidden="true"><i /><i /><i /></span>
                <span class="tool-name">{toolProgress.value.name || "tool call"}</span>
                <span class="tool-status">writing… {toolProgress.value.chars.toLocaleString()} chars</span>
              </div>
            </div>
          </div>
        )}
        {busy.value && !toolProgress.value && !l.content && !l.reasoning && !pending.value && (
          <div class="msg assistant dim" role="status">
            <span class="typing" aria-hidden="true"><i /><i /><i /></span>
            {runStatus.value && <span class="run-status">{runStatus.value}</span>}
          </div>
        )}
        {runError.value && <div class="error" role="alert">{runError.value}</div>}
      </div>
      <PlanCard />
      <Composer />
    </div>
  );
}
