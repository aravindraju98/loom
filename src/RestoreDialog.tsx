import { useEffect, useState } from "preact/hooks";
import type { PlanItem } from "./api";
import { DiffStat } from "./ui/Diff";
import { Modal } from "./ui/Modal";
import { chat, errText, restorePlan, restoreReq, restoreTo, settings, stripAttachments } from "./state";

const ACTION: Record<PlanItem["action"], string> = {
  restore: "restore",
  delete: "delete (created by the agent)",
  recreate: "bring back (deleted by the agent)",
  none: "unchanged",
  skip: "too large, can't restore",
};

function shortPath(p: string): string {
  const ws = settings.value?.workspace;
  const rel = ws && p.toLowerCase().startsWith(ws.toLowerCase()) ? p.slice(ws.length).replace(/^[\\/]/, "") : p;
  return rel.replace(/\\/g, "/");
}

/**
 * "Go back to before this message": shows which files the agent changed since then and lets
 * the user restore files, the conversation, or both.
 */
export function RestoreDialog() {
  const req = restoreReq.value;
  const [plan, setPlan] = useState<PlanItem[] | null>(null);
  const [err, setErr] = useState("");
  const [force, setForce] = useState(false);
  const [working, setWorking] = useState(false);

  useEffect(() => {
    if (!req) return;
    setPlan(null);
    setErr("");
    restorePlan(req.index).then(setPlan).catch((e) => setErr(errText(e)));
  }, [req?.index]);

  if (!req) return null;
  const close = () => (restoreReq.value = null);
  const msg = chat.value.messages[req.index];
  const changes = (plan ?? []).filter((p) => p.action !== "none");
  const conflicts = changes.filter((p) => p.conflict);
  const hasFiles = changes.some((p) => p.action !== "skip");
  // what the user sees as messages: their prompts and the assistant's replies, not tool results
  const later = chat.value.messages.slice(req.index).filter((m) => m.role !== "tool" && (m.role === "user" || m.content)).length;
  const text = stripAttachments(msg?.content ?? "").replace(/\s+/g, " ").trim();
  const quote = text.length > 80 ? `${text.slice(0, 78).trimEnd()}…` : text;

  const go = async (files: boolean, conversation: boolean) => {
    setWorking(true);
    await restoreTo(req.index, { files, conversation, force });
    setWorking(false);
    close();
  };

  const blocked = conflicts.length > 0 && !force;
  return (
    <Modal
      title={req.edit ? "Edit message" : "Restore checkpoint"}
      onClose={close}
      wide
      footer={
        <>
          <button onClick={close}>Cancel</button>
          <span class="spacer" />
          {!req.edit && hasFiles && (
            <button disabled={working || blocked} onClick={() => void go(true, false)}>Files only</button>
          )}
          {!req.edit && (
            <button disabled={working} onClick={() => void go(false, true)}>Conversation only</button>
          )}
          <button class="primary" data-autofocus disabled={working || (hasFiles && blocked)} onClick={() => void go(hasFiles, true)}>
            {req.edit ? (hasFiles ? "Undo changes and edit" : "Edit") : hasFiles ? "Restore both" : "Restore conversation"}
          </button>
        </>
      }
    >
      <p class="dlg-lead">
        Go back to before <q class="quote">{quote}</q>.{" "}
        {later > 0 && <>The {later} message{later === 1 ? "" : "s"} from there on will be moved to history (not deleted){req.edit ? " and the message put back in the composer" : ""}.</>}
      </p>
      {err && <div class="error">{err}</div>}
      {!plan && !err && <div class="hint">Checking files…</div>}
      {plan && changes.length === 0 && <div class="hint">The agent didn't change any files after this message.</div>}
      {changes.length > 0 && (
        <>
          <div class="section-title">Files the agent changed</div>
          <ul class="filelist">
            {changes.map((p) => (
              <li key={p.path} class={p.conflict ? "conflict" : ""}>
                <span class="ellipsis grow" title={p.path}>{shortPath(p.path)}</span>
                <span class="hint">{ACTION[p.action]}</span>
                <DiffStat added={p.added} removed={p.removed} />
                {p.conflict && <span class="badge warn" title="Edited outside the agent since its last change">edited since</span>}
              </li>
            ))}
          </ul>
          {conflicts.length > 0 && (
            <label class="toggle">
              <input type="checkbox" checked={force} onChange={(e) => setForce((e.target as HTMLInputElement).checked)} />
              {conflicts.length === 1 ? "1 file was" : `${conflicts.length} files were`} edited outside the agent afterwards. Overwrite those edits too.
            </label>
          )}
          <div class="hint">Changes made by shell commands are not tracked. A restore can be undone from the notification that follows.</div>
        </>
      )}
    </Modal>
  );
}
