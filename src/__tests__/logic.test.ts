import { describe, expect, it } from "vitest";
import { parseUnified } from "../ui/Diff";
import { changedFields, chatMarkdown, friendlyError, stripAttachments } from "../state";
import type { Chat, Settings } from "../api";

describe("parseUnified", () => {
  it("numbers old and new lines across a hunk", () => {
    const rows = parseUnified("--- a/x\n+++ b/x\n@@ -10,3 +10,3 @@\n keep\n-old\n+new\n tail\n");
    expect(rows.map((r) => r.kind)).toEqual(["hunk", "ctx", "del", "add", "ctx"]);
    expect(rows[1]).toMatchObject({ old: 10, new: 10, text: "keep" });
    expect(rows[2]).toMatchObject({ old: 11, text: "old" });
    expect(rows[3]).toMatchObject({ new: 11, text: "new" });
    expect(rows[4]).toMatchObject({ old: 12, new: 12 });
  });
  it("ignores no-newline markers", () => {
    expect(parseUnified("@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+b\n").length).toBe(3);
  });
});

describe("settings patches", () => {
  it("only includes fields that changed", () => {
    const base = { temperature: 0.7, model_dirs: ["a"], workspace: "w" } as unknown as Settings;
    const next = { temperature: 0.2, model_dirs: ["a"], workspace: "w" } as unknown as Settings;
    expect(changedFields(base, next)).toEqual({ temperature: 0.2 });
    expect(changedFields(base, { ...base, model_dirs: ["a", "b"] } as Settings)).toEqual({ model_dirs: ["a", "b"] });
  });
});

describe("chat helpers", () => {
  it("strips inlined attachments from a prompt", () => {
    expect(stripAttachments("File: a.txt\n```\nhello\n```\n\nwhat is this?")).toBe("what is this?");
    expect(stripAttachments("plain")).toBe("plain");
  });

  it("exports a chat as markdown with tool summaries", () => {
    const c: Chat = {
      id: "c", title: "T", updated: 0,
      messages: [
        { role: "user", content: "fix it" },
        { role: "assistant", content: "done", tool_calls: [{ id: "1", type: "function", function: { name: "edit_file", arguments: "{}" } }] },
        { role: "tool", tool_call_id: "1", content: "ok", ui: { ok: true, change: { path: "a.ts", kind: "edit", added: 2, removed: 1 } } },
      ],
    };
    const md = chatMarkdown(c);
    expect(md).toContain("# T");
    expect(md).toContain("## You\n\nfix it");
    expect(md).toContain("`edit_file` a.ts (+2 −1)");
  });

  it("turns backend errors into actionable text", () => {
    expect(friendlyError("context_overflow: Start a new chat.")).toBe("Start a new chat.");
    expect(friendlyError("could not reach the engine: refused")).toMatch(/not responding/);
    expect(friendlyError("other")).toBe("other");
  });
});
