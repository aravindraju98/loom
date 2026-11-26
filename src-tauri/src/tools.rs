//! Built-in agent tools, confined to the workspace folder.
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const MAX_OUT: usize = 30_000;

pub fn schemas() -> Vec<Value> {
    fn f(name: &str, desc: &str, props: Value, req: &[&str]) -> Value {
        json!({"type":"function","function":{"name":name,"description":desc,
            "parameters":{"type":"object","properties":props,"required":req}}})
    }
    vec![
        f("read_file", "Read a text file in the workspace. Returns numbered lines.",
          json!({"path":{"type":"string","description":"File path, relative to the workspace or absolute inside it"},
                 "offset":{"type":"integer","description":"1-based first line (default 1)"},
                 "limit":{"type":"integer","description":"Max lines (default 400)"}}), &["path"]),
        f("list_dir", "List files and folders in a workspace directory.",
          json!({"path":{"type":"string","description":"Directory (default: workspace root)"}}), &[]),
        f("glob", "Find files by glob pattern, e.g. **/*.rs. Respects .gitignore.",
          json!({"pattern":{"type":"string"}}), &["pattern"]),
        f("grep", "Search file contents with a regular expression. Returns file:line: text.",
          json!({"pattern":{"type":"string","description":"Rust regex"},
                 "path":{"type":"string","description":"Directory or file to search (default: workspace)"},
                 "glob":{"type":"string","description":"Only files matching this glob"}}), &["pattern"]),
        f("write_file", "Create or overwrite a file with the given content. For a large file, write the first part, then call again with append=true for each further part (keep each call under ~150 lines).",
          json!({"path":{"type":"string"},"content":{"type":"string"},
                 "append":{"type":"boolean","description":"add to the end of the file instead of overwriting"}}), &["path", "content"]),
        f("edit_file", "Replace an exact string in a file. old_string must match exactly once unless replace_all is true.",
          json!({"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},
                 "replace_all":{"type":"boolean"}}), &["path", "old_string", "new_string"]),
        f("run_shell", "Run a PowerShell command in the workspace directory and return its output.",
          json!({"command":{"type":"string"},"timeout_secs":{"type":"integer","description":"default 60, max 600"}}), &["command"]),
    ]
}

/// Tools that change things and need the user's approval.
pub fn needs_approval(name: &str) -> bool {
    matches!(name, "write_file" | "edit_file" | "run_shell")
}

pub fn is_builtin(name: &str) -> bool {
    matches!(name, "read_file" | "list_dir" | "glob" | "grep" | "write_file" | "edit_file" | "run_shell")
}

/// Resolve `p` against the workspace and refuse anything that lands outside it (.., symlinks, other drives).
pub fn resolve(ws: &Path, p: &str) -> Result<PathBuf, String> {
    let root = ws.canonicalize().map_err(|e| format!("workspace unavailable: {e}"))?;
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
    let mut real = existing.canonicalize().map_err(|e| format!("bad path: {e}"))?;
    for t in tail.into_iter().rev() {
        real.push(t);
    }
    if real.starts_with(&root) {
        Ok(real)
    } else {
        Err(format!("path '{p}' is outside the workspace"))
    }
}

fn trunc(mut s: String) -> String {
    if s.len() > MAX_OUT {
        let mut cut = MAX_OUT;
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

pub async fn run(name: &str, args: &Value, ws: &Path) -> Result<String, String> {
    match name {
        "read_file" => {
            let p = resolve(ws, arg_str(args, "path")?)?;
            let bytes = tokio::fs::read(&p).await.map_err(|e| e.to_string())?;
            if bytes.iter().take(8000).any(|b| *b == 0) {
                return Err("binary file".into());
            }
            let text = String::from_utf8_lossy(&bytes);
            let off = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
            let lim = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(400) as usize;
            let mut out = String::new();
            for (i, l) in text.lines().enumerate().skip(off - 1).take(lim) {
                out.push_str(&format!("{:>5}\t{}\n", i + 1, l));
            }
            if out.is_empty() {
                out = "(empty or past end of file)".into();
            }
            Ok(trunc(out))
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
            Ok(trunc(
                rows.into_iter()
                    .map(|(f, n, s)| if f { format!("{n}  ({s} bytes)") } else { format!("{n}/") })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ))
        }
        "glob" => {
            let pat = arg_str(args, "pattern")?.to_string();
            let ws = ws.to_path_buf();
            tokio::task::spawn_blocking(move || {
                let g = globset::Glob::new(&pat).map_err(|e| e.to_string())?.compile_matcher();
                let mut out = vec![];
                for e in ignore::WalkBuilder::new(&ws).build().flatten() {
                    if let Ok(rel) = e.path().strip_prefix(&ws) {
                        if e.path().is_file() && g.is_match(rel) {
                            out.push(rel.to_string_lossy().replace('\\', "/"));
                            if out.len() >= 500 {
                                break;
                            }
                        }
                    }
                }
                Ok(if out.is_empty() { "(no matches)".into() } else { trunc(out.join("\n")) })
            })
            .await
            .map_err(|e| e.to_string())?
        }
        "grep" => {
            let pat = arg_str(args, "pattern")?.to_string();
            let base = resolve(ws, args.get("path").and_then(|v| v.as_str()).unwrap_or("."))?;
            let glob = args.get("glob").and_then(|v| v.as_str()).map(String::from);
            let ws = ws.canonicalize().map_err(|e| e.to_string())?;
            tokio::task::spawn_blocking(move || {
                let re = regex::Regex::new(&pat).map_err(|e| format!("bad regex: {e}"))?;
                let gm = match glob {
                    Some(g) => Some(globset::Glob::new(&g).map_err(|e| e.to_string())?.compile_matcher()),
                    None => None,
                };
                let mut out = vec![];
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
                    if bytes.len() > 2_000_000 || bytes.iter().take(4000).any(|b| *b == 0) {
                        continue;
                    }
                    let text = String::from_utf8_lossy(&bytes);
                    for (i, l) in text.lines().enumerate() {
                        if re.is_match(l) {
                            let l = if l.len() > 300 { &l[..l.floor_char_boundary(300)] } else { l };
                            out.push(format!("{}:{}: {}", rel.to_string_lossy().replace('\\', "/"), i + 1, l.trim()));
                            if out.len() >= 200 {
                                break 'files;
                            }
                        }
                    }
                }
                Ok(if out.is_empty() { "(no matches)".into() } else { trunc(out.join("\n")) })
            })
            .await
            .map_err(|e| e.to_string())?
        }
        "write_file" => {
            let p = resolve(ws, arg_str(args, "path")?)?;
            let content = arg_str(args, "content")?;
            if let Some(d) = p.parent() {
                tokio::fs::create_dir_all(d).await.map_err(|e| e.to_string())?;
            }
            let append = args.get("append").and_then(|v| v.as_bool()).unwrap_or(false);
            if append {
                use tokio::io::AsyncWriteExt;
                let mut f = tokio::fs::OpenOptions::new().create(true).append(true).open(&p).await.map_err(|e| e.to_string())?;
                f.write_all(content.as_bytes()).await.map_err(|e| e.to_string())?;
                return Ok(format!("appended {} bytes to {}", content.len(), p.display()));
            }
            tokio::fs::write(&p, content).await.map_err(|e| e.to_string())?;
            Ok(format!("wrote {} bytes to {}", content.len(), p.display()))
        }
        "edit_file" => {
            let p = resolve(ws, arg_str(args, "path")?)?;
            let old = arg_str(args, "old_string")?;
            let new = arg_str(args, "new_string")?;
            let all = args.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
            if old.is_empty() {
                return Err("old_string is empty".into());
            }
            let text = tokio::fs::read_to_string(&p).await.map_err(|e| e.to_string())?;
            let n = text.matches(old).count();
            if n == 0 {
                return Err("old_string not found".into());
            }
            if n > 1 && !all {
                return Err(format!("old_string matches {n} times; add context or set replace_all"));
            }
            let out = if all { text.replace(old, new) } else { text.replacen(old, new, 1) };
            tokio::fs::write(&p, out).await.map_err(|e| e.to_string())?;
            Ok(format!("edited {} ({n} replacement{})", p.display(), if n == 1 { "" } else { "s" }))
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
            {
                        c.creation_flags(0x0800_0000);
            }
            let mut child = c.spawn().map_err(|e| e.to_string())?;
            let mut so = child.stdout.take().unwrap();
            let mut se = child.stderr.take().unwrap();
            let fut = async {
                let mut a = String::new();
                let mut b = String::new();
                let (_, _) = tokio::join!(so.read_to_string(&mut a), se.read_to_string(&mut b));
                let st = child.wait().await;
                (a, b, st)
            };
            match tokio::time::timeout(Duration::from_secs(secs), fut).await {
                Ok((a, b, st)) => {
                    let code = st.ok().and_then(|s| s.code()).unwrap_or(-1);
                    let mut out = a;
                    if !b.is_empty() {
                        out.push_str("\n[stderr]\n");
                        out.push_str(&b);
                    }
                    Ok(trunc(format!("{out}\n[exit code {code}]")))
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
    async fn write_read_edit_grep() {
        let ws = tmp("rw");
        run("write_file", &json!({"path":"d/hello.txt","content":"one\ntwo\nthree\n"}), &ws).await.unwrap();
        let r = run("read_file", &json!({"path":"d/hello.txt"}), &ws).await.unwrap();
        assert!(r.contains("two"));
        // append builds a file in pieces; a plain write still overwrites
        run("write_file", &json!({"path":"d/chunks.txt","content":"A\n"}), &ws).await.unwrap();
        run("write_file", &json!({"path":"d/chunks.txt","content":"B\n","append":true}), &ws).await.unwrap();
        assert_eq!(std::fs::read_to_string(ws.join("d/chunks.txt")).unwrap(), "A\nB\n");
        run("edit_file", &json!({"path":"d/hello.txt","old_string":"two","new_string":"2"}), &ws).await.unwrap();
        let g = run("grep", &json!({"pattern":"^2$"}), &ws).await.unwrap();
        assert!(g.contains("hello.txt:2"), "{g}");
        assert!(run("edit_file", &json!({"path":"d/hello.txt","old_string":"zzz","new_string":"y"}), &ws).await.is_err());
        let o = run("run_shell", &json!({"command":"Write-Output hi"}), &ws).await.unwrap();
        assert!(o.contains("hi") && o.contains("exit code 0"), "{o}");
        assert!(run("read_file", &json!({"path":"..\\evil"}), &ws).await.is_err());
    }
}
