import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { Icon } from "./icons";
import type { FileChange, Msg } from "./api";
import { md } from "./markdown";
import {
  answerPermission, attachments, busy, chat, draft, engine, live, pending, restoreReq, runError, runStatus, runningTool,
  send, settings, stop, stripAttachments, toast, toolProgress, touched, useTools,
} from "./state";
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
  return (
    <div class="perm">
      {preview ? (
        <>
          <div class="perm-head">
            <span>{preview.kind === "create" ? "Create" : preview.kind === "delete" ? "Delete" : preview.kind === "move" ? "Move" : "Change"} <code>{preview.path}</code></span>
            <DiffStat added={preview.added} removed={preview.removed} />
          </div>
          <ChangeView c={preview} />
        </>
      ) : name === "run_shell" ? (
        <pre class="tool-args">{(() => { try { return String((JSON.parse(args) as { command?: string }).command ?? args); } catch { return args; } })()}</pre>
      ) : (
        <pre class="tool-args">{pretty(args)}</pre>
      )}
      <div class="perm-actions">
        <button class="primary" data-autofocus onClick={() => answerPermission(true, false)}>Allow once</button>
        <button onClick={() => answerPermission(true, true)}>Allow {FILE_TOOLS.has(name) ? "edits" : name === "run_shell" ? "commands" : "this tool"} for this chat</button>
        <span class="spacer" />
        <button class="danger" onClick={() => answerPermission(false, false)}>Deny</button>
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
      {(m.ui?.cancelled || m.ui?.tok_s) && (
        <div class="meta">
          {m.ui?.cancelled && <span class="badge">Stopped</span>}
          {m.ui?.tok_s ? turnMeta(m.ui) : null}
        </div>
      )}
    </div>
  );
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
        <span class="spacer" />
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
      <Composer />
    </div>
  );
}
