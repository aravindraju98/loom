import { marked } from "marked";
import { markedHighlight } from "marked-highlight";
import markedKatex from "marked-katex-extension";
import hljs from "highlight.js/lib/common";
import DOMPurify from "dompurify";
import "katex/dist/katex.min.css";

marked.use(
  markedHighlight({
    langPrefix: "hljs language-",
    highlight(code, lang) {
      try {
        return lang && hljs.getLanguage(lang) ? hljs.highlight(code, { language: lang }).value : hljs.highlightAuto(code).value;
      } catch {
        return code;
      }
    },
  }),
);
// output "html" keeps KaTeX to plain spans, which survive the sanitizer
marked.use(markedKatex({ throwOnError: false, output: "html", nonStandard: true }));
marked.setOptions({ gfm: true, breaks: true });

/** Models write math as \( \) and \[ \]; the KaTeX extension wants $ and $$. Leave code alone. */
function normalizeMath(src: string): string {
  return src
    .split(/(```[\s\S]*?(?:```|$)|`[^`\n]*`)/g)
    .map((part, i) =>
      i % 2 === 1
        ? part
        : part
            .replace(/\\\[([\s\S]+?)\\\]/g, (_m, x) => `\n$$\n${x.trim()}\n$$\n`)
            .replace(/\\\(([\s\S]+?)\\\)/g, (_m, x) => `$${x.trim()}$`),
    )
    .join("");
}

// Model output can be steered by file contents or tool results (prompt injection). A remote image
// would make the app fetch an attacker URL that can carry data in its query string, so remote
// images become plain links; only inline data: images are shown.
DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node.tagName === "IMG") {
    const src = node.getAttribute("src") ?? "";
    if (!/^data:image\/(png|jpe?g|gif|webp);/i.test(src)) {
      const a = document.createElement("a");
      a.textContent = `[image: ${node.getAttribute("alt") || src}]`;
      if (/^https?:\/\//i.test(src)) a.setAttribute("href", src);
      node.replaceWith(a);
    }
  }
  if (node.tagName === "A") node.setAttribute("rel", "noreferrer noopener");
});

export function md(src: string): string {
  const html = marked.parse(normalizeMath(src ?? ""), { async: false }) as string;
  const clean = DOMPurify.sanitize(html, { ADD_ATTR: ["target"], FORBID_TAGS: ["style", "form", "input", "button", "iframe", "object", "embed"] });
  // copy button on every code block (wired by a delegated click handler in main.tsx)
  return clean
    .replace(/<pre><code/g, '<div class="codewrap"><button type="button" class="copy">Copy</button><pre><code')
    .replace(/<\/code><\/pre>/g, "</code></pre></div>");
}
