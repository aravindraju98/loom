import { afterEach, describe, expect, it, vi } from "vitest";
import { render } from "preact";
import { act } from "preact/test-utils";
import { md } from "../markdown";
import { Diff } from "../ui/Diff";
import { Modal } from "../ui/Modal";

let host: HTMLDivElement;
function mount(node: preact.ComponentChild) {
  host = document.createElement("div");
  document.body.appendChild(host);
  act(() => render(node, host));
  return host;
}
afterEach(() => {
  if (host) {
    render(null, host);
    host.remove();
  }
});

describe("markdown sanitising", () => {
  it("turns remote images into links so they are never fetched", () => {
    const html = md("![x](https://evil.example/p.png?secret=1)");
    expect(html).not.toContain("<img");
    expect(html).toContain("[image: x]");
  });
  it("keeps inline data images", () => {
    expect(md("![d](data:image/png;base64,iVBORw0KGgo=)")).toContain("<img");
  });
  it("removes scripts, iframes and handlers", () => {
    const html = md('<script>alert(1)</script><iframe src="x"></iframe><a href="https://a.b" onclick="x()">l</a>');
    expect(html).not.toMatch(/<script|<iframe|onclick/);
    expect(html).toContain('rel="noreferrer noopener"');
  });
  it("wraps code blocks with a copy button", () => {
    expect(md("```js\nlet a = 1\n```")).toContain('class="codewrap"');
  });
});

describe("Diff", () => {
  it("renders numbered added and removed rows", () => {
    const el = mount(<Diff diff={"@@ -1,2 +1,2 @@\n a\n-b\n+c\n"} />);
    expect(el.querySelectorAll(".diff-add").length).toBe(1);
    expect(el.querySelectorAll(".diff-del").length).toBe(1);
    expect(el.querySelector(".diff-del .diff-text")?.textContent).toBe("b");
  });
  it("shows an empty state", () => {
    expect(mount(<Diff diff="" />).textContent).toContain("No changes");
  });
});

describe("Modal", () => {
  it("is a labelled dialog that closes on Escape and restores focus", () => {
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    opener.focus();
    const onClose = vi.fn();
    const el = mount(
      <Modal title="Hello" onClose={onClose}>
        <button>first</button>
      </Modal>,
    );
    const dlg = el.querySelector('[role="dialog"]')!;
    expect(dlg.getAttribute("aria-modal")).toBe("true");
    expect(document.getElementById(dlg.getAttribute("aria-labelledby")!)?.textContent).toBe("Hello");
    act(() => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(onClose).toHaveBeenCalled();
    act(() => render(null, host));
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });
});
