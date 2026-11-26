import { marked } from "marked";
import { markedHighlight } from "marked-highlight";
import markedKatex from "marked-katex-extension";
import hljs from "highlight.js/lib/common";
import DOMPurify from "dompurify";
import "katex/dist/katex.min.css";
import "highlight.js/styles/github-dark.css";

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

export function md(src: string): string {
  const html = marked.parse(normalizeMath(src ?? ""), { async: false }) as string;
  const clean = DOMPurify.sanitize(html, { ADD_ATTR: ["target"] });
  // copy button on every code block (wired by a delegated click handler in main.tsx)
  return clean
    .replace(/<pre><code/g, '<div class="codewrap"><button type="button" class="copy">Copy</button><pre><code')
    .replace(/<\/code><\/pre>/g, "</code></pre></div>");
}
