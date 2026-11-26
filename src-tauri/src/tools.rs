//! Built-in agent tools, confined to the workspace folder.
//!
//! Tuned for local models: small outputs (they cost context and prefill time), forgiving edit
//! matching with precise hints when a match fails, and edits that return a short diff instead of
//! asking the model to regenerate whole files.
use crate::checkpoint::Recorder;
use crate::textfile::{self, TextFile};
use crate::util::LockExt;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// How much the loaded model can take in; sets read windows and output caps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tier {
    /// local model with a small window (≤16k)
    Small,
    /// local model with a mid-size window
    Medium,
    /// remote API or big window
    Large,
}

impl Tier {
    pub fn from_ctx(ctx: u32, local: bool) -> Tier {
        match (local, ctx) {
            (false, _) => Tier::Large,
            (true, c) if c > 0 && c <= 16_384 => Tier::Small,
            (true, c) if c > 65_536 => Tier::Large,
            _ => Tier::Medium,
        }
    }
    fn read_lines(self) -> usize {
        match self {
            Tier::Small => 150,
            Tier::Medium => 250,
            Tier::Large => 400,
        }
    }
    /// characters per tool result (~4 chars per token)
    pub fn max_out(self) -> usize {
        match self {
            Tier::Small => 12_000,
            Tier::Medium => 20_000,
            Tier::Large => 30_000,
        }
    }
}

/// Per-run tool state.
pub struct ToolCtx {
    pub ws: PathBuf,
    pub tier: Tier,
    /// file -> content version the model has seen (by reading or writing it)
    seen: Mutex<HashMap<PathBuf, String>>,
    /// (file, offset, limit) -> version, for answering repeat reads without resending content
    reads: Mutex<HashMap<(PathBuf, usize, usize), String>>,
    pub recorder: Option<Recorder>,
}

impl ToolCtx {
    pub fn new(ws: PathBuf, tier: Tier, recorder: Option<Recorder>) -> Self {
        ToolCtx { ws, tier, seen: Mutex::new(HashMap::new()), reads: Mutex::new(HashMap::new()), recorder }
    }
    fn saw(&self, p: &Path, ver: &str) {
        self.seen.lk().insert(p.to_path_buf(), ver.to_string());
        self.reads.lk().retain(|(f, _, _), v| f != p || v == ver);
    }
    fn has_seen(&self, p: &Path) -> Option<String> {
        self.seen.lk().get(p).cloned()
    }
    /// forget repeat-read dedup (after context pruning the model no longer has earlier reads)
    pub fn forget_reads(&self) {
        self.reads.lk().clear();
    }
    fn rel(&self, p: &Path) -> String {
        let root = crate::util::canon(&self.ws).unwrap_or_else(|_| self.ws.clone());
        p.strip_prefix(&root).unwrap_or(p).to_string_lossy().replace('\\', "/")
    }
}

/// Result of a tool: text for the model, optional extra data for the UI (e.g. a full diff).
#[derive(Debug, Default)]
pub struct ToolOut {
    pub text: String,
    pub ui: Option<Value>,
}

impl From<String> for ToolOut {
    fn from(text: String) -> Self {
        ToolOut { text, ui: None }
    }
}

pub fn schemas(tier: Tier) -> Vec<Value> {
    fn f(name: &str, desc: &str, props: Value, req: &[&str]) -> Value {
        json!({"type":"function","function":{"name":name,"description":desc,
            "parameters":{"type":"object","properties":props,"required":req}}})
    }
    let mut v = vec![
        f("read_file", "Read a text file. Output lines look like `   12\tcode`: the number and tab are NOT part of the file. Use outline=true first on big files.",
          json!({"path":{"type":"string"},
                 "offset":{"type":"integer","description":"first line, 1-based"},
                 "limit":{"type":"integer","description":"max lines"},
                 "outline":{"type":"boolean","description":"list only headings/functions/classes with line numbers"}}), &["path"]),
        f("list_dir", "List a directory.", json!({"path":{"type":"string","description":"default: workspace root"}}), &[]),
        f("glob", "Find files by pattern, e.g. **/*.rs (respects .gitignore).", json!({"pattern":{"type":"string"}}), &["pattern"]),
        f("grep", "Search file contents with a regex. Returns path:line: text.",
          json!({"pattern":{"type":"string"},"path":{"type":"string"},"glob":{"type":"string","description":"only files matching, e.g. *.ts"}}), &["pattern"]),
        f("edit_file", "Change part of a file: replace old_string (copied exactly from the file, without line numbers, with enough lines to be unique) by new_string. Preferred way to change existing files.",
          json!({"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},
                 "replace_all":{"type":"boolean","description":"replace every occurrence"},
                 "edits":{"type":"array","description":"several replacements in one call, applied in order",
                          "items":{"type":"object","properties":{"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["old_string","new_string"]}}}),
          &["path"]),
        f("edit_lines", "Replace lines start..end (inclusive, numbers from your latest read_file) with new_text. new_text \"\" deletes them. To insert before line N without replacing, use start=N, end=N-1.",
          json!({"path":{"type":"string"},"start":{"type":"integer"},"end":{"type":"integer"},"new_text":{"type":"string"}}), &["path", "start", "end", "new_text"]),
        f("write_file", "Create a new file, or replace a whole file you have read (set overwrite=true). For big new files write the first ~150 lines, then call again with append=true.",
          json!({"path":{"type":"string"},"content":{"type":"string"},
                 "append":{"type":"boolean","description":"add to the end"},
                 "overwrite":{"type":"boolean","description":"replace an existing file entirely"}}), &["path", "content"]),
        f("run_shell", "Run a PowerShell command in the workspace and return its output.",
          json!({"command":{"type":"string"},"timeout_secs":{"type":"integer","description":"default 60, max 600"}}), &["command"]),
        f("update_plan", "Keep a short checklist for multi-step tasks. Send the whole list each time; mark one step in_progress at a time.",
          json!({"steps":{"type":"array","items":{"type":"object","properties":{
                    "step":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","done"]}},"required":["step","status"]}}}), &["steps"]),
    ];
    if tier != Tier::Small {
        v.push(f("move_file", "Move or rename a file.", json!({"from":{"type":"string"},"to":{"type":"string"}}), &["from", "to"]));
        v.push(f("delete_file", "Delete a file.", json!({"path":{"type":"string"}}), &["path"]));
    }
    v
}

const MUTATING: &[&str] = &["write_file", "edit_file", "edit_lines", "move_file", "delete_file"];

/// Tools that change things and need the user's approval.
pub fn needs_approval(name: &str) -> bool {
    MUTATING.contains(&name) || name == "run_shell"
}

pub fn is_builtin(name: &str) -> bool {
    matches!(name, "read_file" | "list_dir" | "glob" | "grep" | "run_shell" | "update_plan") || MUTATING.contains(&name)
}

/// Validate a plan from `update_plan` and summarise it.
fn plan_tool(args: &Value) -> Result<ToolOut, String> {
    let steps = args.get("steps").and_then(|s| s.as_array()).ok_or("give steps: [{step, status}]")?;
    if steps.is_empty() || steps.len() > 30 {
        return Err("a plan needs 1 to 30 steps".into());
    }
    let mut clean = vec![];
    for s in steps {
        let text = s.get("step").and_then(|x| x.as_str()).map(str::trim).filter(|t| !t.is_empty()).ok_or("every step needs text")?;
        let status = match s.get("status").and_then(|x| x.as_str()).unwrap_or("pending") {
            "done" | "completed" | "complete" => "done",
            "in_progress" | "in progress" | "active" | "doing" => "in_progress",
            _ => "pending",
        };
        clean.push(json!({"step": text.chars().take(200).collect::<String>(), "status": status}));
    }
    let done = clean.iter().filter(|s| s["status"] == "done").count();
    let next = clean.iter().find(|s| s["status"] != "done").and_then(|s| s["step"].as_str()).unwrap_or("");
    let text =
        if next.is_empty() { format!("Plan updated: all {done} steps done.") } else { format!("Plan updated: {done}/{} done. Next: {next}", clean.len()) };
    Ok(ToolOut { text, ui: Some(json!({"plan": clean})) })
}

/// Resolve `p` against the workspace and refuse anything that lands outside it (.., symlinks, other drives).
pub fn resolve(ws: &Path, p: &str) -> Result<PathBuf, String> {
    let root = crate::util::canon(ws).map_err(|e| format!("workspace unavailable: {e}"))?;
    let p = p.trim().trim_matches('"').trim_matches('`');
    let raw = Path::new(p);
    let joined = if raw.is_absolute() { raw.to_path_buf() } else { root.join(raw) };
    // normalise lexically first so `..` cannot hide behind a non-existent directory
    let mut norm = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                norm.pop();
            }
            Component::CurDir => {}
            other => norm.push(other.as_os_str()),
        }
    }
    // canonicalise the deepest existing ancestor to resolve symlinks
    let mut existing = norm.clone();
    let mut tail: Vec<std::ffi::OsString> = vec![];
    while !existing.exists() {
        match existing.file_name() {
            Some(n) => tail.push(n.to_os_string()),
            None => break,
        }
        if !existing.pop() {
            break;
        }
    }
    let mut real = crate::util::canon(&existing).map_err(|e| format!("bad path: {e}"))?;
    for t in tail.into_iter().rev() {
        real.push(t);
    }
    if real.starts_with(&root) {
        Ok(real)
    } else {
        Err(format!("path '{p}' is outside the workspace"))
    }
}

fn trunc(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n...[output truncated]");
    }
    s
}

fn arg_str<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    a.get(k).and_then(|v| v.as_str()).ok_or_else(|| format!("missing string argument '{k}'"))
}

fn arg_u(a: &Value, k: &str) -> Option<usize> {
    a.get(k).and_then(|v| v.as_u64().or_else(|| v.as_i64().map(|i| i.max(0) as u64))).map(|n| n as usize)
}

fn arg_bool(a: &Value, k: &str) -> bool {
    a.get(k).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Split into lines without the trailing empty element a final newline would create.
fn split_lines(t: &str) -> Vec<&str> {
    if t.is_empty() {
        return vec![];
    }
    let mut v: Vec<&str> = t.split('\n').collect();
    if t.ends_with('\n') {
        v.pop();
    }
    v
}

fn gutter(n: usize, line: &str) -> String {
    const MAX_LINE: usize = 400;
    if line.len() > MAX_LINE {
        let cut = line.floor_char_boundary(MAX_LINE);
        format!("{n:>5}\t{}…[line truncated, {} chars]\n", &line[..cut], line.len())
    } else {
        format!("{n:>5}\t{line}\n")
    }
}

/// Models often paste read_file output (with `   12\t` prefixes) into edits; strip it when every line has it.
fn strip_gutter(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^\s*\d+\t").unwrap());
    let lines: Vec<&str> = s.split('\n').collect();
    let nonempty: Vec<&&str> = lines.iter().filter(|l| !l.is_empty()).collect();
    if nonempty.is_empty() || !nonempty.iter().all(|l| re.is_match(l)) {
        return s.to_string();
    }
    lines.iter().map(|l| re.replace(l, "").into_owned()).collect::<Vec<_>>().join("\n")
}

fn outline(text: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"^\s*(#{1,6}\s|(pub(\([a-z]+\))?\s+)?(async\s+)?(fn|struct|enum|trait|impl|mod|macro_rules!)\b|(export\s+)?(default\s+)?(async\s+)?(function|class|interface|type|enum|const\s+\w+\s*=\s*(async\s*)?\()|(async\s+)?def\s|class\s|func\s|(public|private|protected|internal|static)\s+[\w<>\[\], ]+\s+\w+\s*\()",
        )
        .unwrap()
    });
    let mut out = String::new();
    for (i, l) in split_lines(text).iter().enumerate() {
        if re.is_match(l) {
            out.push_str(&gutter(i + 1, l.trim_end()));
        }
    }
    if out.is_empty() {
        "(no headings, functions or classes recognised; use offset/limit)".into()
    } else {
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Edit matching

#[derive(Debug, PartialEq)]
enum Found {
    /// byte ranges in the text
    Ranges(Vec<(usize, usize)>),
    /// line window [start, end) matched loosely; replacement may need re-indenting
    Lines { start: usize, end: usize, reindent: Option<(String, String)> },
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut v = vec![0];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            v.push(i + 1);
        }
    }
    v
}

fn line_of(starts: &[usize], byte: usize) -> usize {
    match starts.binary_search(&byte) {
        Ok(i) => i + 1,
        Err(i) => i,
    }
}

fn indent_of(l: &str) -> &str {
    &l[..l.len() - l.trim_start().len()]
}

fn trim_blank_edges<'a>(v: &[&'a str]) -> Vec<&'a str> {
    let a = v.iter().position(|l| !l.trim().is_empty()).unwrap_or(v.len());
    let b = v.iter().rposition(|l| !l.trim().is_empty()).map(|x| x + 1).unwrap_or(a);
    v[a..b].to_vec()
}

/// Find `old` in `text`: exactly, then ignoring trailing whitespace, then ignoring indentation.
fn find(text: &str, old: &str) -> Result<Found, String> {
    let exact: Vec<(usize, usize)> = text.match_indices(old).map(|(i, m)| (i, i + m.len())).collect();
    if !exact.is_empty() {
        return Ok(Found::Ranges(exact));
    }
    let tl = split_lines(text);
    let ol = trim_blank_edges(&split_lines(old));
    if ol.is_empty() {
        return Err("old_string is empty or only whitespace".into());
    }
    for (pass, eq) in [(1, &(|a: &str, b: &str| a.trim_end() == b.trim_end()) as &dyn Fn(&str, &str) -> bool), (2, &|a: &str, b: &str| a.trim() == b.trim())] {
        let hits: Vec<usize> = (0..tl.len().saturating_sub(ol.len() - 1)).filter(|&i| ol.iter().enumerate().all(|(j, o)| eq(tl[i + j], o))).collect();
        match hits.len() {
            0 => continue,
            1 => {
                let i = hits[0];
                let reindent = if pass == 2 {
                    let first = (0..ol.len()).find(|&j| !ol[j].trim().is_empty()).unwrap_or(0);
                    let (have, want) = (indent_of(ol[first]).to_string(), indent_of(tl[i + first]).to_string());
                    (have != want).then_some((have, want))
                } else {
                    None
                };
                return Ok(Found::Lines { start: i, end: i + ol.len(), reindent });
            }
            n => {
                let at: Vec<String> = hits.iter().take(5).map(|i| (i + 1).to_string()).collect();
                return Err(format!("old_string matches {n} places (lines {}); include more surrounding lines to make it unique", at.join(", ")));
            }
        }
    }
    Err(not_found_hint(&tl, &ol))
}

/// Point the model at the most similar region so it can retry with the exact text.
fn not_found_hint(tl: &[&str], ol: &[&str]) -> String {
    let n = ol.len();
    let mut best = (0usize, 0usize);
    if tl.len() >= n {
        for i in 0..=tl.len() - n {
            let score = (0..n).filter(|&j| tl[i + j].trim() == ol[j].trim()).count();
            if score > best.1 {
                best = (i, score);
            }
        }
    }
    if best.1 == 0 {
        // try a single distinctive line
        if let Some(probe) = ol.iter().map(|l| l.trim()).filter(|l| l.len() > 8).max_by_key(|l| l.len()) {
            if let Some(i) = tl.iter().position(|l| l.contains(probe)) {
                best = (i.saturating_sub(1), 1);
            }
        }
    }
    if best.1 == 0 {
        return "old_string not found. read_file the region again and copy the text exactly, or use edit_lines with line numbers.".into();
    }
    let (a, b) = (best.0, (best.0 + n.max(3) + 1).min(tl.len()));
    let mut s = format!("old_string not found exactly. Closest text is at lines {}-{} ({} of {} lines match). Current text there:\n", a + 1, b, best.1, n);
    for (k, l) in tl[a..b].iter().enumerate() {
        s.push_str(&gutter(a + k + 1, l));
    }
    s.push_str("Copy it exactly (without line numbers) or use edit_lines.");
    s
}

fn reindent(new: &str, have: &str, want: &str) -> String {
    new.split('\n')
        .map(|l| {
            if l.trim().is_empty() {
                l.to_string()
            } else if let Some(rest) = l.strip_prefix(have) {
                format!("{want}{rest}")
            } else if have.is_empty() {
                format!("{want}{l}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Apply one replacement; returns the new text and a note about how it matched.
fn apply_edit(text: &str, old: &str, new: &str, all: bool) -> Result<(String, &'static str), String> {
    let old = strip_gutter(&old.replace("\r\n", "\n"));
    let new = strip_gutter(&new.replace("\r\n", "\n"));
    if old.is_empty() {
        return Err("old_string is empty; to create a file use write_file, to insert use edit_lines".into());
    }
    if old == new {
        return Err("old_string and new_string are identical; nothing to change".into());
    }
    match find(text, &old)? {
        Found::Ranges(r) if r.len() == 1 || all => Ok((if all { text.replace(&old, &new) } else { text.replacen(&old, &new, 1) }, "")),
        Found::Ranges(r) => {
            let starts = line_starts(text);
            let at: Vec<String> = r.iter().take(5).map(|(b, _)| line_of(&starts, *b).to_string()).collect();
            Err(format!(
                "old_string matches {} places (lines {}); include more surrounding lines to make it unique, or set replace_all",
                r.len(),
                at.join(", ")
            ))
        }
        Found::Lines { start, end, reindent: ri } => {
            let starts = line_starts(text);
            let a = starts[start];
            let b = if end < starts.len() { starts[end] - 1 } else { text.len() };
            let b = b.max(a);
            let body = new.strip_suffix('\n').unwrap_or(&new);
            let body = match &ri {
                Some((have, want)) => reindent(body, have, want),
                None => body.to_string(),
            };
            let mut out = String::with_capacity(text.len() + body.len());
            out.push_str(&text[..a]);
            out.push_str(&body);
            out.push_str(&text[b..]);
            Ok((out, if ri.is_some() { " (matched ignoring indentation; re-indented)" } else { " (matched ignoring trailing spaces)" }))
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Planned file changes: computed first (for the approval diff), written after approval.

pub struct Change {
    path: PathBuf,
    rel: String,
    /// existing file, if any
    before: Option<TextFile>,
    new_text: String,
    kind: &'static str,
    note: String,
    /// moves/deletes: (from, to)
    mv: Option<(PathBuf, Option<PathBuf>)>,
}

impl Change {
    fn old_text(&self) -> &str {
        self.before.as_ref().map(|t| t.text.as_str()).unwrap_or("")
    }
    pub fn preview(&self) -> Value {
        let (added, removed) = textfile::diff_stat(self.old_text(), &self.new_text);
        json!({
            "path": self.rel, "kind": self.kind, "added": added, "removed": removed,
            "diff": textfile::unified(self.old_text(), &self.new_text, &self.rel, 3),
        })
    }
}

fn load_existing(p: &Path) -> Result<Option<TextFile>, String> {
    if !p.exists() {
        return Ok(None);
    }
    if p.is_dir() {
        return Err(format!("{} is a directory", p.display()));
    }
    let (tf, _) = textfile::read(p)?;
    if tf.lossy {
        return Err("file is not valid UTF-8 text; refusing to edit it (it would be corrupted)".into());
    }
    Ok(Some(tf))
}

fn current_version(p: &Path) -> Option<String> {
    std::fs::read(p).ok().map(|b| textfile::version_of(&b))
}

/// Work out what a mutating tool would do, without touching disk. Errors here go straight back
/// to the model, without bothering the user for approval.
pub fn plan(name: &str, args: &Value, ctx: &ToolCtx) -> Result<Change, String> {
    match name {
        "edit_file" => {
            let p = resolve(&ctx.ws, arg_str(args, "path")?)?;
            let rel = ctx.rel(&p);
            let tf = load_existing(&p)?.ok_or_else(|| format!("{rel} does not exist; use write_file to create it"))?;
            let mut edits: Vec<(String, String, bool)> = vec![];
            if let Some(arr) = args.get("edits").and_then(|e| e.as_array()) {
                for e in arr {
                    edits.push((arg_str(e, "old_string")?.to_string(), arg_str(e, "new_string")?.to_string(), arg_bool(e, "replace_all")));
                }
            }
            if let (Some(o), Some(n)) = (args.get("old_string").and_then(|v| v.as_str()), args.get("new_string").and_then(|v| v.as_str())) {
                edits.insert(0, (o.to_string(), n.to_string(), arg_bool(args, "replace_all")));
            }
            if edits.is_empty() {
                return Err("give old_string and new_string (or an edits list)".into());
            }
            let mut text = tf.text.clone();
            let mut notes = vec![];
            let multi = edits.len() > 1;
            for (i, (o, n, all)) in edits.iter().enumerate() {
                let (t, note) = apply_edit(&text, o, n, *all).map_err(|e| if multi { format!("edit {}: {e}. No edits were applied.", i + 1) } else { e })?;
                if !note.is_empty() {
                    notes.push(note.trim().to_string());
                }
                text = t;
            }
            Ok(Change { path: p, rel, before: Some(tf), new_text: text, kind: "edit", note: notes.join("; "), mv: None })
        }
        "edit_lines" => {
            let p = resolve(&ctx.ws, arg_str(args, "path")?)?;
            let rel = ctx.rel(&p);
            let tf = load_existing(&p)?.ok_or_else(|| format!("{rel} does not exist; use write_file to create it"))?;
            let cur = current_version(&p).unwrap_or_default();
            if ctx.has_seen(&p).as_deref() != Some(cur.as_str()) {
                return Err(format!("read_file {rel} first: line numbers must come from the current version of the file"));
            }
            let lines = split_lines(&tf.text);
            let n = lines.len();
            let start = arg_u(args, "start").ok_or("missing integer 'start'")?;
            let end = arg_u(args, "end").ok_or("missing integer 'end'")?;
            if start < 1 || start > n + 1 || end + 1 < start || end > n {
                return Err(format!("bad range {start}-{end}: the file has {n} lines (to insert before line N use start=N, end=N-1)"));
            }
            let new_text = arg_str(args, "new_text")?.replace("\r\n", "\n");
            let new_text = strip_gutter(&new_text);
            let body = new_text.strip_suffix('\n').unwrap_or(&new_text);
            let mut out: Vec<String> = lines[..start - 1].iter().map(|s| s.to_string()).collect();
            if !body.is_empty() {
                out.extend(body.split('\n').map(String::from));
            }
            out.extend(lines[end..].iter().map(|s| s.to_string()));
            let mut text = out.join("\n");
            if tf.text.ends_with('\n') || (n == 0 && !text.is_empty()) {
                text.push('\n');
            }
            let replaced = end + 1 - start;
            let inserted = if body.is_empty() { 0 } else { body.split('\n').count() };
            let delta = inserted as i64 - replaced as i64;
            let note = if delta != 0 && end < n { format!("lines after {end} moved by {delta:+}") } else { String::new() };
            Ok(Change { path: p, rel, before: Some(tf), new_text: text, kind: "edit", note, mv: None })
        }
        "write_file" => {
            let p = resolve(&ctx.ws, arg_str(args, "path")?)?;
            let rel = ctx.rel(&p);
            let content = arg_str(args, "content")?.replace("\r\n", "\n");
            let before = load_existing(&p)?;
            let append = arg_bool(args, "append");
            if let Some(tf) = &before {
                let unread = ctx.has_seen(&p).is_none();
                if !append && !tf.text.is_empty() && !arg_bool(args, "overwrite") && unread {
                    return Err(format!(
                        "{rel} already exists ({} lines). Use edit_file to change part of it, or read it and call write_file with overwrite=true to replace it all.",
                        split_lines(&tf.text).len()
                    ));
                }
            }
            let (new_text, kind) = match (&before, append) {
                (Some(tf), true) => {
                    let mut t = tf.text.clone();
                    if !t.is_empty() && !t.ends_with('\n') && !content.starts_with('\n') {
                        t.push('\n');
                    }
                    t.push_str(&content);
                    (t, "append")
                }
                (Some(_), false) => (content, "overwrite"),
                (None, _) => (content, "create"),
            };
            Ok(Change { path: p, rel, before, new_text, kind, note: String::new(), mv: None })
        }
        "move_file" => {
            let from = resolve(&ctx.ws, arg_str(args, "from")?)?;
            let to = resolve(&ctx.ws, arg_str(args, "to")?)?;
            if !from.is_file() {
                return Err(format!("{} is not a file", ctx.rel(&from)));
            }
            if to.exists() {
                return Err(format!("{} already exists", ctx.rel(&to)));
            }
            let rel = format!("{} → {}", ctx.rel(&from), ctx.rel(&to));
            Ok(Change { path: from.clone(), rel, before: None, new_text: String::new(), kind: "move", note: String::new(), mv: Some((from, Some(to))) })
        }
        "delete_file" => {
            let p = resolve(&ctx.ws, arg_str(args, "path")?)?;
            if !p.is_file() {
                return Err(format!("{} is not a file", ctx.rel(&p)));
            }
            let before = textfile::read(&p).ok().map(|(t, _)| t);
            Ok(Change { rel: ctx.rel(&p), path: p.clone(), before, new_text: String::new(), kind: "delete", note: String::new(), mv: Some((p, None)) })
        }
        _ => Err(format!("{name} does not change files")),
    }
}

/// Write a planned change to disk (with checkpointing) and describe it for the model.
pub fn commit(c: Change, ctx: &ToolCtx) -> Result<ToolOut, String> {
    let rec = |p: &Path, before: bool| {
        if let Some(r) = &ctx.recorder {
            if before {
                r.before(p)
            } else {
                r.after(p)
            }
        }
    };
    if let Some((from, to)) = &c.mv {
        rec(from, true);
        if let Some(to) = to {
            rec(to, true);
            if let Some(d) = to.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            std::fs::rename(from, to).map_err(|e| format!("move failed: {e}"))?;
            rec(to, false);
        } else {
            std::fs::remove_file(from).map_err(|e| format!("delete failed: {e}"))?;
        }
        rec(from, false);
        let verb = if to.is_some() { "Moved" } else { "Deleted" };
        return Ok(ToolOut { text: format!("{verb} {}", c.rel), ui: Some(json!({"path": c.rel, "kind": c.kind})) });
    }
    let like = c.before.clone().unwrap_or_else(|| textfile::new_file_like(&c.new_text));
    let bytes = textfile::encode(&c.new_text, &like);
    if let Some(d) = c.path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("cannot create folder: {e}"))?;
    }
    rec(&c.path, true);
    crate::util::write_atomic(&c.path, &bytes).map_err(|e| format!("write failed: {e}"))?;
    rec(&c.path, false);
    let ver = textfile::version_of(&bytes);
    ctx.saw(&c.path, &ver);

    let old = c.old_text();
    let (added, removed) = textfile::diff_stat(old, &c.new_text);
    let lines = split_lines(&c.new_text).len();
    let mut text = match c.kind {
        "create" => format!("Created {} ({lines} lines) · version {ver}", c.rel),
        "append" => format!("Appended to {} (now {lines} lines) · version {ver}", c.rel),
        _ => format!("Edited {} (+{added} -{removed}, now {lines} lines) · version {ver}", c.rel),
    };
    if !c.note.is_empty() {
        text.push_str(&format!("\nNote: {}", c.note));
    }
    // show the model what changed, briefly; it doesn't need its own new file echoed back
    if c.kind == "edit" || c.kind == "overwrite" {
        let max = if ctx.tier == Tier::Small { 30 } else { 50 };
        let d = textfile::compact_diff(old, &c.new_text, max);
        if !d.is_empty() {
            text.push('\n');
            text.push_str(&d);
        }
    }
    let ui = json!({
        "path": c.rel, "kind": c.kind, "added": added, "removed": removed,
        "diff": textfile::unified(old, &c.new_text, &c.rel, 3),
    });
    Ok(ToolOut { text, ui: Some(ui) })
}

pub async fn run(name: &str, args: &Value, ctx: &ToolCtx) -> Result<ToolOut, String> {
    let ws = ctx.ws.as_path();
    let max = ctx.tier.max_out();
    match name {
        n if MUTATING.contains(&n) => {
            let c = plan(name, args, ctx)?;
            commit(c, ctx)
        }
        "update_plan" => plan_tool(args),
        "read_file" => {
            let p = resolve(ws, arg_str(args, "path")?)?;
            let rel = ctx.rel(&p);
            if p.is_dir() {
                return Err(format!("{rel} is a directory; use list_dir"));
            }
            let (tf, bytes) = textfile::read(&p)?;
            let ver = textfile::version_of(&bytes);
            let lines = split_lines(&tf.text);
            let total = lines.len();
            if arg_bool(args, "outline") {
                ctx.saw(&p, &ver);
                return Ok(format!("{rel} · outline · {total} lines · version {ver}\n{}", trunc(outline(&tf.text), max)).into());
            }
            let off = arg_u(args, "offset").unwrap_or(1).max(1);
            let lim = arg_u(args, "limit").unwrap_or(ctx.tier.read_lines()).clamp(1, 2000);
            if total == 0 {
                ctx.saw(&p, &ver);
                return Ok(format!("{rel} is empty · version {ver}").into());
            }
            if off > total {
                return Err(format!("offset {off} is past the end: {rel} has {total} lines"));
            }
            let key = (p.clone(), off, lim);
            if ctx.reads.lk().get(&key) == Some(&ver) {
                return Ok(format!("{rel} lines {off}+ are unchanged since your previous read (version {ver}); use that output.").into());
            }
            let mut body = String::new();
            let mut last = off - 1;
            for (i, l) in lines.iter().enumerate().skip(off - 1).take(lim) {
                let g = gutter(i + 1, l);
                if body.len() + g.len() > max {
                    break;
                }
                body.push_str(&g);
                last = i + 1;
            }
            let mut head = format!("{rel} · lines {off}-{last} of {total} · version {ver}");
            if tf.lossy {
                head.push_str(" · not valid UTF-8 (shown lossily)");
            }
            let more = if last < total { format!("\n… {} more lines: read_file with offset={}", total - last, last + 1) } else { String::new() };
            ctx.saw(&p, &ver);
            ctx.reads.lk().insert(key, ver);
            Ok(format!("{head}\n{body}{more}").into())
        }
        "list_dir" => {
            let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            let p = resolve(ws, rel)?;
            let mut rd = tokio::fs::read_dir(&p).await.map_err(|e| e.to_string())?;
            let mut rows = vec![];
            while let Ok(Some(e)) = rd.next_entry().await {
                let md = e.metadata().await.ok();
                let dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                rows.push((!dir, e.file_name().to_string_lossy().into_owned(), md.map(|m| m.len()).unwrap_or(0)));
            }
            rows.sort();
            if rows.is_empty() {
                return Ok("(empty directory)".to_string().into());
            }
            Ok(trunc(rows.into_iter().map(|(f, n, s)| if f { format!("{n}  ({s} bytes)") } else { format!("{n}/") }).collect::<Vec<_>>().join("\n"), max)
                .into())
        }
        "glob" => {
            let pat = arg_str(args, "pattern")?.to_string();
            let ws = ws.to_path_buf();
            tokio::task::spawn_blocking(move || {
                let g = globset::Glob::new(&pat).map_err(|e| e.to_string())?.compile_matcher();
                let mut out = vec![];
                for e in ignore::WalkBuilder::new(&ws).build().flatten() {
                    if let Ok(rel) = e.path().strip_prefix(&ws) {
                        if e.path().is_file() && (g.is_match(rel) || g.is_match(e.path().file_name().unwrap_or_default())) {
                            out.push(rel.to_string_lossy().replace('\\', "/"));
                            if out.len() >= 300 {
                                out.push("… (more matches; narrow the pattern)".into());
                                break;
                            }
                        }
                    }
                }
                Ok(if out.is_empty() { "(no matches)".to_string() } else { trunc(out.join("\n"), max) }.into())
            })
            .await
            .map_err(|e| e.to_string())?
        }
        "grep" => {
            let pat = arg_str(args, "pattern")?.to_string();
            let base = resolve(ws, args.get("path").and_then(|v| v.as_str()).unwrap_or("."))?;
            let glob = args.get("glob").and_then(|v| v.as_str()).map(String::from);
            let ws = crate::util::canon(ws).map_err(|e| e.to_string())?;
            tokio::task::spawn_blocking(move || {
                let re = regex::Regex::new(&pat).or_else(|_| regex::Regex::new(&regex::escape(&pat))).map_err(|e| format!("bad regex: {e}"))?;
                let gm = match glob {
                    Some(g) => Some(globset::Glob::new(&g).map_err(|e| e.to_string())?.compile_matcher()),
                    None => None,
                };
                let mut out = vec![];
                let mut size = 0;
                'files: for e in ignore::WalkBuilder::new(&base).build().flatten() {
                    let p = e.path();
                    if !p.is_file() {
                        continue;
                    }
                    let rel = p.strip_prefix(&ws).unwrap_or(p);
                    if let Some(g) = &gm {
                        if !g.is_match(rel) && !g.is_match(p.file_name().unwrap_or_default()) {
                            continue;
                        }
                    }
                    let Ok(bytes) = std::fs::read(p) else { continue };
                    if bytes.len() > 2_000_000 || textfile::looks_binary(&bytes) {
                        continue;
                    }
                    let text = textfile::decode(&bytes).text;
                    for (i, l) in text.lines().enumerate() {
                        if re.is_match(l) {
                            let l = if l.len() > 200 { &l[..l.floor_char_boundary(200)] } else { l };
                            let row = format!("{}:{}: {}", rel.to_string_lossy().replace('\\', "/"), i + 1, l.trim());
                            size += row.len() + 1;
                            out.push(row);
                            if out.len() >= 150 || size > max {
                                out.push("… (more matches; narrow the pattern or path)".into());
                                break 'files;
                            }
                        }
                    }
                }
                Ok(if out.is_empty() { "(no matches)".to_string() } else { out.join("\n") }.into())
            })
            .await
            .map_err(|e| e.to_string())?
        }
        "run_shell" => {
            let cmd = arg_str(args, "command")?.to_string();
            let secs = args.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(60).clamp(1, 600);
            let mut c = Command::new("powershell");
            c.args(["-NoProfile", "-NonInteractive", "-Command", &cmd])
                .current_dir(ws)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            c.creation_flags(0x0800_0000);
            let mut child = c.spawn().map_err(|e| e.to_string())?;
            let mut so = child.stdout.take().ok_or("no stdout")?;
            let mut se = child.stderr.take().ok_or("no stderr")?;
            let fut = async {
                let mut a = vec![];
                let mut b = vec![];
                let (_, _) = tokio::join!(so.read_to_end(&mut a), se.read_to_end(&mut b));
                let st = child.wait().await;
                (a, b, st)
            };
            match tokio::time::timeout(Duration::from_secs(secs), fut).await {
                Ok((a, b, st)) => {
                    let code = st.ok().and_then(|s| s.code()).unwrap_or(-1);
                    let mut out = String::from_utf8_lossy(&a).into_owned();
                    let err = String::from_utf8_lossy(&b);
                    if !err.trim().is_empty() {
                        out.push_str("\n[stderr]\n");
                        out.push_str(&err);
                    }
                    // keep the end of long output: errors and summaries are usually last
                    if out.len() > max {
                        let cut = out.len() - max;
                        let cut = (cut..out.len()).find(|i| out.is_char_boundary(*i)).unwrap_or(cut);
                        out = format!("…[{} earlier characters cut]\n{}", cut, &out[cut..]);
                    }
                    Ok(format!("{}\n[exit code {code}]", out.trim_end()).into())
                }
                Err(_) => Err(format!("timed out after {secs}s")),
            }
        }
        _ => Err(format!("unknown tool {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("harness_ws_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn ctx(ws: &Path) -> ToolCtx {
        ToolCtx::new(ws.to_path_buf(), Tier::Medium, None)
    }

    async fn ok(name: &str, a: Value, c: &ToolCtx) -> String {
        run(name, &a, c).await.unwrap_or_else(|e| panic!("{name} failed: {e}")).text
    }

    #[test]
    fn confines_paths() {
        let ws = tmp("conf");
        assert!(resolve(&ws, "a/b.txt").is_ok());
        assert!(resolve(&ws, "new_dir/new.txt").is_ok());
        assert!(resolve(&ws, "../x").is_err());
        assert!(resolve(&ws, "a/../../x").is_err());
        assert!(resolve(&ws, "C:\\Windows\\win.ini").is_err());
        assert!(resolve(&ws, ws.join("ok.txt").to_str().unwrap()).is_ok());
    }

    #[tokio::test]
    async fn write_read_edit_grep_shell() {
        let ws = tmp("rw");
        let c = ctx(&ws);
        ok("write_file", json!({"path":"d/hello.txt","content":"one\ntwo\nthree\n"}), &c).await;
        let r = ok("read_file", json!({"path":"d/hello.txt"}), &c).await;
        assert!(r.contains("lines 1-3 of 3") && r.contains("    2\ttwo"), "{r}");
        // a repeat read of the same unchanged range doesn't resend the content
        assert!(ok("read_file", json!({"path":"d/hello.txt"}), &c).await.contains("unchanged"));
        ok("write_file", json!({"path":"d/chunks.txt","content":"A\n"}), &c).await;
        ok("write_file", json!({"path":"d/chunks.txt","content":"B\n","append":true}), &c).await;
        assert_eq!(std::fs::read_to_string(ws.join("d/chunks.txt")).unwrap(), "A\nB\n");
        let e = ok("edit_file", json!({"path":"d/hello.txt","old_string":"two","new_string":"2"}), &c).await;
        assert!(e.contains("+1 -1") && e.contains("+2"), "{e}");
        let g = ok("grep", json!({"pattern":"^2$"}), &c).await;
        assert!(g.contains("hello.txt:2"), "{g}");
        assert!(run("edit_file", &json!({"path":"d/hello.txt","old_string":"zzz","new_string":"y"}), &c).await.is_err());
        let o = ok("run_shell", json!({"command":"Write-Output hi"}), &c).await;
        assert!(o.contains("hi") && o.contains("exit code 0"), "{o}");
        assert!(run("read_file", &json!({"path":"..\\evil"}), &c).await.is_err());
    }

    #[tokio::test]
    async fn write_guard_protects_unread_files() {
        let ws = tmp("guard");
        std::fs::write(ws.join("big.rs"), "fn main() {}\n").unwrap();
        let c = ctx(&ws);
        let e = run("write_file", &json!({"path":"big.rs","content":"x"}), &c).await.unwrap_err();
        assert!(e.contains("already exists"), "{e}");
        ok("read_file", json!({"path":"big.rs"}), &c).await;
        ok("write_file", json!({"path":"big.rs","content":"x\n","overwrite":true}), &c).await;
        assert_eq!(std::fs::read_to_string(ws.join("big.rs")).unwrap(), "x\n");
    }

    #[tokio::test]
    async fn edit_tolerates_gutters_whitespace_and_indent() {
        let ws = tmp("fuzzy");
        let f = ws.join("a.py");
        std::fs::write(&f, "def f():\r\n    x = 1  \r\n    return x\r\n").unwrap();
        let c = ctx(&ws);
        // copied from read_file output, with line numbers
        ok("edit_file", json!({"path":"a.py","old_string":"    2\t    x = 1  ","new_string":"    2\t    x = 2  "}), &c).await;
        // trailing spaces missing in old_string
        ok("edit_file", json!({"path":"a.py","old_string":"    x = 2\n    return x","new_string":"    x = 3\n    return x"}), &c).await;
        // indentation lost by the model: matched and re-indented
        let out = ok("edit_file", json!({"path":"a.py","old_string":"return x","new_string":"return x + 1"}), &c).await;
        assert!(!out.contains("re-indented"), "exact substring should match first: {out}");
        let out = ok("edit_file", json!({"path":"a.py","old_string":"x = 3\nreturn x + 1","new_string":"y = 3\nreturn y"}), &c).await;
        assert!(out.contains("re-indented"), "{out}");
        // CRLF preserved throughout
        assert_eq!(std::fs::read(&f).unwrap(), b"def f():\r\n    y = 3\r\n    return y\r\n");
    }

    #[tokio::test]
    async fn edit_failures_point_at_the_right_place() {
        let ws = tmp("hint");
        std::fs::write(ws.join("m.rs"), "fn a() {\n    let v = compute(1, 2);\n    v\n}\nfn b() {}\nfn b() {}\n").unwrap();
        let c = ctx(&ws);
        let e = run("edit_file", &json!({"path":"m.rs","old_string":"let v = compute(1, 3);\n    v","new_string":"z"}), &c).await.unwrap_err();
        assert!(e.contains("Closest text is at lines") && e.contains("compute(1, 2)"), "{e}");
        let e = run("edit_file", &json!({"path":"m.rs","old_string":"fn b() {}","new_string":"fn c() {}"}), &c).await.unwrap_err();
        assert!(e.contains("matches 2 places (lines 5, 6)"), "{e}");
        // multi-edit is all-or-nothing
        let before = std::fs::read_to_string(ws.join("m.rs")).unwrap();
        let e = run("edit_file", &json!({"path":"m.rs","edits":[{"old_string":"fn a()","new_string":"fn a2()"},{"old_string":"nope","new_string":"x"}]}), &c)
            .await
            .unwrap_err();
        assert!(e.starts_with("edit 2:") && e.contains("No edits were applied"), "{e}");
        assert_eq!(std::fs::read_to_string(ws.join("m.rs")).unwrap(), before);
        ok(
            "edit_file",
            json!({"path":"m.rs","edits":[{"old_string":"fn a()","new_string":"fn a2()"},{"old_string":"compute(1, 2)","new_string":"compute(3, 4)"}]}),
            &c,
        )
        .await;
        let t = std::fs::read_to_string(ws.join("m.rs")).unwrap();
        assert!(t.contains("fn a2()") && t.contains("compute(3, 4)"));
    }

    #[tokio::test]
    async fn edit_lines_needs_fresh_read_and_reports_shift() {
        let ws = tmp("lines");
        std::fs::write(ws.join("l.txt"), "1\n2\n3\n4\n").unwrap();
        let c = ctx(&ws);
        assert!(run("edit_lines", &json!({"path":"l.txt","start":2,"end":2,"new_text":"x"}), &c).await.unwrap_err().contains("read_file"));
        ok("read_file", json!({"path":"l.txt"}), &c).await;
        let o = ok("edit_lines", json!({"path":"l.txt","start":2,"end":3,"new_text":"a\nb\nc\n"}), &c).await;
        assert!(o.contains("moved by +1"), "{o}");
        assert_eq!(std::fs::read_to_string(ws.join("l.txt")).unwrap(), "1\na\nb\nc\n4\n");
        // insert before line 1, then delete the last line; own edits keep the file "seen"
        ok("edit_lines", json!({"path":"l.txt","start":1,"end":0,"new_text":"top"}), &c).await;
        ok("edit_lines", json!({"path":"l.txt","start":6,"end":6,"new_text":""}), &c).await;
        assert_eq!(std::fs::read_to_string(ws.join("l.txt")).unwrap(), "top\n1\na\nb\nc\n");
        // an outside change invalidates line numbers
        std::fs::write(ws.join("l.txt"), "changed\n").unwrap();
        assert!(run("edit_lines", &json!({"path":"l.txt","start":1,"end":1,"new_text":"y"}), &c).await.is_err());
        assert!(run("edit_lines", &json!({"path":"l.txt","start":5,"end":9,"new_text":"y"}), &c).await.is_err());
    }

    #[tokio::test]
    async fn outline_move_delete_and_preview() {
        let ws = tmp("misc");
        std::fs::write(ws.join("s.rs"), "use x;\npub fn alpha() {}\nstruct B;\nimpl B {\n    fn c(&self) {}\n}\n").unwrap();
        let c = ToolCtx::new(ws.clone(), Tier::Large, None);
        let o = ok("read_file", json!({"path":"s.rs","outline":true}), &c).await;
        assert!(o.contains("2\tpub fn alpha") && o.contains("5\t    fn c") && !o.contains("use x"), "{o}");
        let p = plan("edit_file", &json!({"path":"s.rs","old_string":"alpha","new_string":"beta"}), &c).unwrap().preview();
        assert_eq!((p["added"].as_u64(), p["removed"].as_u64()), (Some(1), Some(1)));
        assert!(p["diff"].as_str().unwrap().contains("+pub fn beta"));
        ok("move_file", json!({"from":"s.rs","to":"sub/t.rs"}), &c).await;
        assert!(ws.join("sub/t.rs").exists() && !ws.join("s.rs").exists());
        ok("delete_file", json!({"path":"sub/t.rs"}), &c).await;
        assert!(!ws.join("sub/t.rs").exists());
        assert!(schemas(Tier::Small).iter().all(|s| s["function"]["name"] != "move_file"));
    }

    #[tokio::test]
    async fn plan_tool_normalises_statuses() {
        let ws = tmp("plan");
        let c = ctx(&ws);
        let o = run(
            "update_plan",
            &json!({"steps":[{"step":"read","status":"completed"},{"step":"fix","status":"in progress"},{"step":"test","status":"todo"}]}),
            &c,
        )
        .await
        .unwrap();
        assert_eq!(o.text, "Plan updated: 1/3 done. Next: fix");
        assert_eq!(o.ui.unwrap()["plan"][1]["status"], "in_progress");
        assert!(run("update_plan", &json!({"steps":[]}), &c).await.is_err());
    }
}
