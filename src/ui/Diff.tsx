import { useMemo } from "preact/hooks";

interface Row {
  kind: "add" | "del" | "ctx" | "hunk";
  old?: number;
  new?: number;
  text: string;
}

/** Parse a unified diff into numbered rows. Header lines (---/+++) are dropped. */
export function parseUnified(diff: string): Row[] {
  const rows: Row[] = [];
  let o = 0;
  let n = 0;
  for (const line of diff.split("\n")) {
    if (line.startsWith("---") || line.startsWith("+++")) continue;
    const h = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
    if (h) {
      o = Number(h[1]);
      n = Number(h[2]);
      rows.push({ kind: "hunk", text: line });
    } else if (line.startsWith("+")) {
      rows.push({ kind: "add", new: n++, text: line.slice(1) });
    } else if (line.startsWith("-")) {
      rows.push({ kind: "del", old: o++, text: line.slice(1) });
    } else if (line.startsWith(" ")) {
      rows.push({ kind: "ctx", old: o++, new: n++, text: line.slice(1) });
    } else if (line.startsWith("\\")) {
      // "\ No newline at end of file"
    } else if (line !== "") {
      rows.push({ kind: "ctx", text: line });
    }
  }
  return rows;
}

/** Line-numbered, coloured unified diff. Long diffs are capped with a "show all" toggle by the caller. */
export function Diff({ diff, max = 400 }: { diff: string; max?: number }) {
  const rows = useMemo(() => parseUnified(diff), [diff]);
  if (rows.length === 0) return <div class="hint">No changes.</div>;
  const shown = rows.slice(0, max);
  return (
    <div class="diff" role="table" aria-label="File changes">
      {shown.map((r, i) =>
        r.kind === "hunk" ? (
          <div key={i} class="diff-hunk" role="row">{r.text}</div>
        ) : (
          <div key={i} class={`diff-row diff-${r.kind}`} role="row">
            <span class="diff-ln" aria-hidden="true">{r.old ?? ""}</span>
            <span class="diff-ln" aria-hidden="true">{r.new ?? ""}</span>
            <span class="diff-sign" aria-hidden="true">{r.kind === "add" ? "+" : r.kind === "del" ? "−" : " "}</span>
            <span class="diff-text">{r.text || " "}</span>
          </div>
        ),
      )}
      {rows.length > max && <div class="diff-hunk">… {rows.length - max} more lines</div>}
    </div>
  );
}

/** "+12 −3" */
export function DiffStat({ added, removed }: { added?: number; removed?: number }) {
  return (
    <span class="diffstat">
      {added ? <span class="diff-plus">+{added}</span> : null}
      {removed ? <span class="diff-minus">−{removed}</span> : null}
    </span>
  );
}
