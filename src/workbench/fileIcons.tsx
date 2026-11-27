/** VS Code-style file-type glyphs: a short label or shape in the type's usual colour. */

export interface FileKind {
  /** 1–3 character badge, or "" for a shape icon */
  label: string;
  color: string;
  /** CodeMirror language id (see Editor.tsx) */
  lang?: string;
}

const K = (label: string, color: string, lang?: string): FileKind => ({ label, color, lang });

const BY_EXT: Record<string, FileKind> = {
  ts: K("TS", "#3178c6", "ts"),
  mts: K("TS", "#3178c6", "ts"),
  cts: K("TS", "#3178c6", "ts"),
  tsx: K("TSX", "#3178c6", "tsx"),
  js: K("JS", "#d6b500", "js"),
  mjs: K("JS", "#d6b500", "js"),
  cjs: K("JS", "#d6b500", "js"),
  jsx: K("JSX", "#d6b500", "jsx"),
  json: K("{}", "#c9a227", "json"),
  jsonl: K("{}", "#c9a227"),
  rs: K("RS", "#ce7a4c", "rust"),
  py: K("PY", "#3a75b0", "python"),
  pyi: K("PY", "#3a75b0", "python"),
  ipynb: K("NB", "#e46e2e", "json"),
  md: K("MD", "#4a8fd4", "markdown"),
  mdx: K("MD", "#4a8fd4", "markdown"),
  html: K("<>", "#e44d26", "html"),
  htm: K("<>", "#e44d26", "html"),
  vue: K("V", "#41b883", "html"),
  svelte: K("S", "#ff3e00", "html"),
  css: K("#", "#2f7bd9", "css"),
  scss: K("#", "#c6538c", "css"),
  less: K("#", "#2a4d80", "css"),
  yml: K("YML", "#cb171e", "yaml"),
  yaml: K("YML", "#cb171e", "yaml"),
  toml: K("TML", "#9c4121"),
  ini: K("INI", "#8a8f98"),
  env: K("ENV", "#ecd53f"),
  go: K("GO", "#00add8"),
  java: K("JV", "#e76f00"),
  kt: K("KT", "#a97bff"),
  c: K("C", "#5c6bc0"),
  h: K("H", "#7986cb"),
  cpp: K("C++", "#00599c"),
  cc: K("C++", "#00599c"),
  hpp: K("H++", "#00599c"),
  cs: K("C#", "#68217a"),
  swift: K("SW", "#f05138"),
  rb: K("RB", "#cc342d"),
  php: K("PHP", "#777bb3"),
  lua: K("LUA", "#000080"),
  sh: K("$", "#89c35c"),
  bash: K("$", "#89c35c"),
  ps1: K("PS", "#2671be"),
  bat: K("BAT", "#c1f12e"),
  cmd: K("BAT", "#c1f12e"),
  sql: K("SQL", "#e38c00"),
  xml: K("XML", "#e37933"),
  svg: K("SVG", "#ffb13b"),
  txt: K("TXT", "#8a8f98"),
  log: K("LOG", "#8a8f98"),
  csv: K("CSV", "#3a9b52"),
  lock: K("", "#8a8f98"),
  gguf: K("AI", "#a855f7"),
  safetensors: K("AI", "#a855f7"),
  png: K("", "#26a69a"),
  jpg: K("", "#26a69a"),
  jpeg: K("", "#26a69a"),
  gif: K("", "#26a69a"),
  webp: K("", "#26a69a"),
  ico: K("", "#26a69a"),
  pdf: K("PDF", "#e53935"),
  zip: K("ZIP", "#afb42b"),
  wasm: K("WA", "#654ff0"),
};

const BY_NAME: Record<string, FileKind> = {
  "package.json": K("npm", "#cb3837", "json"),
  "package-lock.json": K("", "#8a8f98", "json"),
  "cargo.toml": K("RS", "#ce7a4c"),
  "cargo.lock": K("", "#8a8f98"),
  dockerfile: K("DK", "#2496ed"),
  ".gitignore": K("GIT", "#f05032"),
  ".gitattributes": K("GIT", "#f05032"),
  "readme.md": K("i", "#4a8fd4", "markdown"),
  "license": K("§", "#d4a13a"),
  "agents.md": K("AI", "#36b3c4", "markdown"),
  "claude.md": K("AI", "#36b3c4", "markdown"),
  "tsconfig.json": K("TS", "#3178c6", "json"),
  "vite.config.ts": K("VT", "#9a6cff", "ts"),
};

const DEFAULT: FileKind = K("", "#8a8f98");

export function fileKind(name: string): FileKind {
  const n = name.toLowerCase();
  if (BY_NAME[n]) return BY_NAME[n];
  if (n.startsWith(".env")) return BY_EXT.env;
  const dot = n.lastIndexOf(".");
  return (dot > 0 || (dot === 0 && n.length > 1) ? BY_EXT[n.slice(dot + 1)] : undefined) ?? DEFAULT;
}

const IMAGE = new Set(["png", "jpg", "jpeg", "gif", "webp", "ico"]);

export function FileIcon({ name, size = 16 }: { name: string; size?: number }) {
  const k = fileKind(name);
  const ext = name.toLowerCase().split(".").pop() ?? "";
  if (!k.label) {
    // shape icons: image, lock, generic document
    return (
      <svg class="ficon" width={size} height={size} viewBox="0 0 16 16" aria-hidden="true">
        {IMAGE.has(ext) ? (
          <>
            <rect x="2" y="3" width="12" height="10" rx="1.5" fill="none" stroke={k.color} stroke-width="1.3" />
            <circle cx="6" cy="6.5" r="1.2" fill={k.color} />
            <path d="M3 12l3.5-3.5 2.5 2.5 2-2 2.5 3" fill="none" stroke={k.color} stroke-width="1.3" />
          </>
        ) : ext === "lock" || name.toLowerCase().endsWith(".lock") || name.toLowerCase().endsWith("-lock.json") ? (
          <>
            <rect x="3.5" y="7" width="9" height="7" rx="1.3" fill={k.color} />
            <path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2" fill="none" stroke={k.color} stroke-width="1.4" />
          </>
        ) : (
          <path d="M4 1.5h5l3.5 3.5v9.5H4z M9 1.5V5h3.5" fill="none" stroke={k.color} stroke-width="1.2" stroke-linejoin="round" />
        )}
      </svg>
    );
  }
  const fs = k.label.length >= 3 ? 5.6 : k.label.length === 2 ? 7 : 9;
  return (
    <svg class="ficon" width={size} height={size} viewBox="0 0 16 16" aria-hidden="true">
      <text x="8" y="11.2" text-anchor="middle" font-size={fs} font-weight="700" font-family="var(--font)" fill={k.color} letter-spacing="-0.2">
        {k.label}
      </text>
    </svg>
  );
}

export function FolderIcon({ open, size = 16 }: { open: boolean; size?: number }) {
  return (
    <svg class="ficon" width={size} height={size} viewBox="0 0 16 16" aria-hidden="true">
      {open ? (
        <path d="M1.5 4.5a1 1 0 0 1 1-1h3.3l1.4 1.4h5.8a1 1 0 0 1 1 1V6H4.2L2.3 12.5H1.5z M4.2 6h11l-2 6.5H2.3z" fill="#c09553" />
      ) : (
        <path d="M1.5 4a1 1 0 0 1 1-1h3.3l1.4 1.4h6.3a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1H2.5a1 1 0 0 1-1-1z" fill="#c09553" />
      )}
    </svg>
  );
}
