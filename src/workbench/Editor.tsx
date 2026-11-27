import { useEffect, useRef, useState } from "preact/hooks";
import { Compartment, EditorState } from "@codemirror/state";
import type { Extension } from "@codemirror/state";
import {
  EditorView, crosshairCursor, drawSelection, dropCursor, highlightActiveLine, highlightActiveLineGutter, highlightSpecialChars,
  keymap, lineNumbers, rectangularSelection,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
import { bracketMatching, foldGutter, foldKeymap, indentOnInput, HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { closeBrackets, closeBracketsKeymap } from "@codemirror/autocomplete";
import { unifiedMergeView } from "@codemirror/merge";
import { tags as t } from "@lezer/highlight";
import { Icon } from "../icons";
import { attachments, toast } from "../state";
import { FileIcon, fileKind } from "./fileIcons";
import {
  active, closeFile, dirty, files, keepAgentChange, reloadFile, revertAgentChange, saveFile, setDirty, tabs,
} from "./state";
import { baseName, tabLabels } from "./tabs";
import { pinTab } from "./tabs";

/* ---------- look ---------- */

const theme = EditorView.theme({
  "&": { height: "100%", fontSize: "13px", backgroundColor: "var(--bg)", color: "var(--text)" },
  ".cm-scroller": { fontFamily: "var(--mono)", lineHeight: "1.55", fontVariantLigatures: "none" },
  ".cm-content": { caretColor: "var(--accent)", padding: "6px 0" },
  ".cm-cursor": { borderLeftColor: "var(--accent)", borderLeftWidth: "2px" },
  ".cm-gutters": { backgroundColor: "var(--bg)", color: "var(--text-3)", border: "none", paddingLeft: "6px" },
  ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--text)" },
  ".cm-activeLine": { backgroundColor: "color-mix(in srgb, var(--surface-3) 45%, transparent)" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": { backgroundColor: "color-mix(in srgb, var(--accent) 28%, transparent) !important" },
  ".cm-selectionMatch": { backgroundColor: "color-mix(in srgb, var(--accent) 14%, transparent)" },
  ".cm-matchingBracket": { backgroundColor: "color-mix(in srgb, var(--accent) 22%, transparent)", outline: "none" },
  ".cm-foldGutter span": { color: "var(--text-3)" },
  ".cm-panels": { backgroundColor: "var(--surface)", color: "var(--text)", borderColor: "var(--border)" },
  ".cm-panels input, .cm-panels button": { fontSize: "12px" },
  ".cm-searchMatch": { backgroundColor: "color-mix(in srgb, var(--warn) 30%, transparent)" },
  ".cm-searchMatch-selected": { backgroundColor: "color-mix(in srgb, var(--warn) 55%, transparent)" },
  ".cm-tooltip": { backgroundColor: "var(--surface-2)", border: "1px solid var(--border)", color: "var(--text)" },
  // merge (agent diff) view
  ".cm-changedLine": { backgroundColor: "var(--diff-add) !important" },
  ".cm-deletedChunk": { backgroundColor: "var(--diff-del)", paddingLeft: "4px" },
  ".cm-deletedChunk del, .cm-deletedText": { textDecoration: "none", backgroundColor: "color-mix(in srgb, var(--bad) 20%, transparent)" },
  ".cm-changedText, ins.cm-insertedLine": { backgroundColor: "color-mix(in srgb, var(--ok) 22%, transparent)", textDecoration: "none" },
  ".cm-changeGutter": { width: "3px", paddingLeft: "0" },
  ".cm-changedLineGutter": { backgroundColor: "var(--ok)" },
  ".cm-deletedLineGutter": { backgroundColor: "var(--bad)" },
});

const highlight = HighlightStyle.define([
  { tag: [t.keyword, t.modifier, t.controlKeyword, t.operatorKeyword], color: "var(--syn-kw)" },
  { tag: [t.string, t.special(t.string), t.regexp, t.inserted], color: "var(--syn-str)" },
  { tag: [t.number, t.bool, t.null, t.atom], color: "var(--syn-num)" },
  { tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName], color: "var(--syn-fn)" },
  { tag: [t.typeName, t.className, t.namespace, t.tagName], color: "var(--syn-type)" },
  { tag: [t.comment, t.lineComment, t.blockComment, t.docComment], color: "var(--syn-com)", fontStyle: "italic" },
  { tag: [t.propertyName, t.attributeName, t.labelName], color: "var(--syn-attr)" },
  { tag: [t.heading], color: "var(--syn-fn)", fontWeight: "600" },
  { tag: [t.link, t.url], color: "var(--accent)", textDecoration: "underline" },
  { tag: [t.emphasis], fontStyle: "italic" },
  { tag: [t.strong], fontWeight: "600" },
  { tag: [t.invalid, t.deleted], color: "var(--bad)" },
]);

/* ---------- languages (loaded on demand) ---------- */

async function language(lang: string | undefined): Promise<Extension> {
  switch (lang) {
    case "ts": return (await import("@codemirror/lang-javascript")).javascript({ typescript: true });
    case "tsx": return (await import("@codemirror/lang-javascript")).javascript({ typescript: true, jsx: true });
    case "js": return (await import("@codemirror/lang-javascript")).javascript();
    case "jsx": return (await import("@codemirror/lang-javascript")).javascript({ jsx: true });
    case "json": return (await import("@codemirror/lang-json")).json();
    case "rust": return (await import("@codemirror/lang-rust")).rust();
    case "python": return (await import("@codemirror/lang-python")).python();
    case "markdown": return (await import("@codemirror/lang-markdown")).markdown();
    case "html": return (await import("@codemirror/lang-html")).html();
    case "css": return (await import("@codemirror/lang-css")).css();
    case "yaml": return (await import("@codemirror/lang-yaml")).yaml();
    default: return [];
  }
}

/* ---------- one editor view, one state per open file ---------- */

const states = new Map<string, { state: EditorState; rev: number; agentBase?: string }>();

function makeState(path: string, text: string, readonly: boolean, agentBase: string | undefined, lang: Compartment, onSave: () => void): EditorState {
  return EditorState.create({
    doc: text,
    extensions: [
      lineNumbers(),
      highlightActiveLineGutter(),
      highlightSpecialChars(),
      history(),
      foldGutter(),
      drawSelection(),
      dropCursor(),
      EditorState.allowMultipleSelections.of(true),
      indentOnInput(),
      syntaxHighlighting(highlight),
      bracketMatching(),
      closeBrackets(),
      rectangularSelection(),
      crosshairCursor(),
      highlightActiveLine(),
      highlightSelectionMatches(),
      search({ top: true }),
      keymap.of([
        { key: "Mod-s", preventDefault: true, run: () => (onSave(), true) },
        ...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, ...foldKeymap, indentWithTab,
      ]),
      lang.of([]),
      theme,
      EditorState.readOnly.of(readonly),
      EditorView.editable.of(!readonly),
      EditorView.updateListener.of((u) => {
        if (!u.docChanged) return;
        const saved = files.value[path]?.saved ?? "";
        setDirty(path, u.state.doc.toString() !== saved);
      }),
      agentBase !== undefined
        ? unifiedMergeView({ original: agentBase, mergeControls: false, highlightChanges: true, gutter: true, syntaxHighlightDeletions: true })
        : [],
    ],
  });
}

function CodeView({ path }: { path: string }) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const lang = useRef(new Compartment());
  const meta = files.value[path];

  const save = () => {
    const v = view.current;
    if (v) void saveFile(path, v.state.doc.toString());
  };

  // (re)build the document when the file, its on-disk content or the agent diff changes
  useEffect(() => {
    if (!host.current || !meta || meta.error) return;
    const agentBase = meta.agent?.base;
    let entry = states.get(path);
    if (!entry || entry.rev !== meta.rev || entry.agentBase !== agentBase) {
      // keep unsaved text when only the diff overlay changes
      const text = entry && entry.rev === meta.rev ? entry.state.doc.toString() : meta.saved;
      entry = { state: makeState(path, text, meta.readonly, agentBase, lang.current, save), rev: meta.rev, agentBase };
      states.set(path, entry);
    }
    if (!view.current) view.current = new EditorView({ state: entry.state, parent: host.current });
    else view.current.setState(entry.state);
    const kind = fileKind(baseName(path));
    let alive = true;
    void language(kind.lang).then((ext) => {
      if (alive && view.current) view.current.dispatch({ effects: lang.current.reconfigure(ext) });
    });
    return () => {
      alive = false;
      if (view.current) states.set(path, { ...states.get(path)!, state: view.current.state });
    };
  }, [path, meta?.rev, meta?.agent?.base, meta?.error]);

  useEffect(() => () => {
    view.current?.destroy();
    view.current = null;
  }, []);

  if (!meta) return <div class="editor-msg hint">Opening…</div>;
  if (meta.error) {
    return (
      <div class="editor-msg">
        <div class="hint">{baseName(path)}: {meta.error}</div>
      </div>
    );
  }
  return (
    <div class="code-wrap">
      {meta.conflict && (
        <div class="editor-bar warn" role="alert">
          <span class="grow">This file changed on disk while you were editing it.</span>
          <button onClick={() => void reloadFile(path)}>Reload from disk</button>
          <button onClick={() => { const v = view.current; if (v) void saveFile(path, v.state.doc.toString(), true); }}>Keep mine (overwrite)</button>
        </div>
      )}
      {meta.agent && (
        <div class="editor-bar agent">
          <Icon name="wrench" size={13} />
          <span class="grow">Changes made by the agent are highlighted.</span>
          <button onClick={() => keepAgentChange(path)}>Keep</button>
          <button class="danger" onClick={() => void revertAgentChange(path)}>Revert file</button>
        </div>
      )}
      {meta.readonly && <div class="editor-bar"><span class="grow hint">Read-only{meta.encoding !== "utf8" ? ` (${meta.encoding})` : ""}</span></div>}
      <div class="code-host" ref={host} />
    </div>
  );
}

/* ---------- tabs + welcome ---------- */

function TabBar() {
  const list = tabs.value;
  const labels = tabLabels(list.map((x) => x.path));
  const close = (p: string) => {
    if (dirty.value.has(p) && !confirm(`${baseName(p)} has unsaved changes. Close without saving?`)) return;
    states.delete(p);
    closeFile(p);
  };
  return (
    <div class="tabbar" role="tablist" aria-label="Open files">
      {list.map((tb) => {
        const on = tb.path === active.value;
        const isDirty = dirty.value.has(tb.path);
        const m = files.value[tb.path];
        return (
          <div
            key={tb.path}
            role="tab"
            aria-selected={on}
            tabIndex={on ? 0 : -1}
            class={`ftab ${on ? "on" : ""} ${tb.preview ? "preview" : ""}`}
            title={tb.path}
            onClick={() => (active.value = tb.path)}
            onDblClick={() => (tabs.value = pinTab(tabs.value, tb.path))}
            onAuxClick={(e) => e.button === 1 && close(tb.path)}
          >
            <FileIcon name={baseName(tb.path)} size={14} />
            <span class="ftab-name">{labels.get(tb.path)}</span>
            {m?.agent && <span class="ftab-agent" title="Agent changes to review" />}
            <button class={`ftab-x ${isDirty ? "dirty" : ""}`} aria-label={`Close ${baseName(tb.path)}`} onClick={(e) => { e.stopPropagation(); close(tb.path); }}>
              <span class="dot" aria-hidden="true" />
              <span class="x" aria-hidden="true">×</span>
            </button>
          </div>
        );
      })}
    </div>
  );
}

function Breadcrumb({ path }: { path: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div class="crumbs">
      <span class="ellipsis">{path}</span>
      <button class="chip-btn" title="Add this file to the chat as context" onClick={() => { if (!attachments.value.includes(path)) attachments.value = [...attachments.value, path]; toast("Added to chat", "info"); }}>
        Add to chat
      </button>
      <button class="chip-btn" onClick={() => void navigator.clipboard.writeText(path).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1000); })}>
        {copied ? "Copied" : "Copy path"}
      </button>
    </div>
  );
}

export function EditorArea({ welcome }: { welcome: preact.ComponentChildren }) {
  const p = active.value;
  return (
    <section class="editor" aria-label="Editor">
      {tabs.value.length > 0 && <TabBar />}
      {p ? (
        <>
          <Breadcrumb path={p} />
          <CodeView key="code" path={p} />
        </>
      ) : (
        welcome
      )}
    </section>
  );
}
