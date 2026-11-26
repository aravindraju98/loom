//! Keeping a conversation inside a small local context window.
//!
//! Old tool outputs are the bulk of an agent transcript, so when the prompt nears the window
//! they are replaced by one-line stubs, oldest and superseded first, without calling the model.
//! Pruning is done in one big step (down to about half the budget) and the decision is persisted
//! on the message (`ui.elided`), so later requests send identical text and the KV cache keeps
//! matching instead of being invalidated every turn.
use serde_json::{json, Value};
use std::collections::HashSet;

/// Rough tokens for text when the server hasn't told us (≈3.5 chars/token for code and English).
pub fn estimate_tokens(chars: usize) -> usize {
    chars * 2 / 7 + 1
}

fn msg_chars(m: &Value) -> usize {
    let mut n = m["content"].as_str().map(|s| s.len()).unwrap_or(0) + 16;
    if let Some(tc) = m["tool_calls"].as_array() {
        for c in tc {
            n += c["function"]["arguments"].as_str().map(|s| s.len()).unwrap_or(0) + 24;
        }
    }
    n
}

pub fn is_elided(m: &Value) -> bool {
    m["ui"]["elided"].as_bool().unwrap_or(false)
}

/// The text sent in place of an elided tool result.
pub fn stub(m: &Value) -> String {
    let name = m["name"].as_str().unwrap_or("tool");
    let n = m["content"].as_str().map(|s| s.len()).unwrap_or(0);
    let first = m["content"].as_str().and_then(|s| s.lines().next()).unwrap_or("").chars().take(120).collect::<String>();
    format!("[{name} output removed to save context: {n} chars, began \"{first}\". Run the tool again if you need it.]")
}

/// Characters this message will take when sent.
pub fn sent_chars(m: &Value) -> usize {
    if is_elided(m) {
        stub(m).len() + 16
    } else {
        msg_chars(m)
    }
}

pub struct Budget {
    /// context window in tokens
    pub ctx: usize,
    /// tokens kept free for the reply
    pub reserve: usize,
    /// tokens of system prompt + tool schemas
    pub fixed: usize,
    /// measured chars per token from the last server response (None = use the estimate)
    pub chars_per_token: Option<f64>,
}

impl Budget {
    pub fn new(ctx: usize, fixed_chars: usize) -> Self {
        let reserve = (ctx / 8).clamp(1024, 8192);
        Budget { ctx, reserve, fixed: estimate_tokens(fixed_chars), chars_per_token: None }
    }
    pub fn tokens(&self, chars: usize) -> usize {
        match self.chars_per_token {
            Some(r) if r > 0.5 => (chars as f64 / r) as usize + 1,
            _ => estimate_tokens(chars),
        }
    }
    pub fn available(&self) -> usize {
        self.ctx.saturating_sub(self.reserve + self.fixed)
    }
    pub fn used(&self, msgs: &[Value]) -> usize {
        self.tokens(msgs.iter().map(sent_chars).sum())
    }
}

/// tool_call_id -> path argument of read_file calls, and paths later changed, for "superseded" checks
fn read_paths(msgs: &[Value]) -> Vec<(usize, String)> {
    // index of the tool message, path it read
    let mut calls: Vec<(String, String)> = vec![];
    for m in msgs {
        if let Some(tc) = m["tool_calls"].as_array() {
            for c in tc {
                let name = c["function"]["name"].as_str().unwrap_or("");
                let args: Value = serde_json::from_str(c["function"]["arguments"].as_str().unwrap_or("{}")).unwrap_or(Value::Null);
                if let Some(p) = args["path"].as_str() {
                    if matches!(name, "read_file" | "edit_file" | "edit_lines" | "write_file") {
                        calls.push((c["id"].as_str().unwrap_or("").to_string(), format!("{name}\u{1}{p}")));
                    }
                }
            }
        }
    }
    msgs.iter()
        .enumerate()
        .filter(|(_, m)| m["role"] == "tool")
        .filter_map(|(i, m)| {
            let id = m["tool_call_id"].as_str()?;
            calls.iter().find(|(c, _)| c == id).map(|(_, k)| (i, k.clone()))
        })
        .collect()
}

/// Decide which tool results to elide so the history fits. Returns indices into `msgs`
/// (newly elided only). `keep_recent` most recent tool results are never touched.
pub fn plan_prune(msgs: &[Value], b: &Budget, keep_recent: usize) -> Vec<usize> {
    let avail = b.available();
    if avail == 0 || b.used(msgs) <= avail * 85 / 100 {
        return vec![];
    }
    let target = avail / 2;
    let tool_idx: Vec<usize> = msgs.iter().enumerate().filter(|(_, m)| m["role"] == "tool" && !is_elided(m)).map(|(i, _)| i).collect();
    let protected: HashSet<usize> = tool_idx.iter().rev().take(keep_recent).copied().collect();

    // 1) reads of a file that was read or changed again later: the old content is stale anyway
    let mut order: Vec<usize> = vec![];
    let rp = read_paths(msgs);
    for (k, (i, key)) in rp.iter().enumerate() {
        let (name, path) = key.split_once('\u{1}').unwrap_or(("", ""));
        if name != "read_file" || protected.contains(i) || is_elided(&msgs[*i]) {
            continue;
        }
        if rp[k + 1..].iter().any(|(_, later)| later.split_once('\u{1}').map(|(_, p)| p) == Some(path)) {
            order.push(*i);
        }
    }
    // 2) then everything else, oldest first, biggest savings among equals
    for i in &tool_idx {
        if !protected.contains(i) && !order.contains(i) {
            order.push(*i);
        }
    }

    let mut used = b.used(msgs);
    let mut out = vec![];
    for i in order {
        if used <= target {
            break;
        }
        let m = &msgs[i];
        let saved = b.tokens(msg_chars(m)).saturating_sub(b.tokens(stub(m).len() + 16));
        if saved < 40 {
            continue; // not worth breaking the cache for
        }
        used = used.saturating_sub(saved);
        out.push(i);
    }
    out
}

/// Mark messages elided (persisted by the UI via the `pruned` event).
pub fn apply(msgs: &mut [Value], idx: &[usize]) {
    for &i in idx {
        if let Some(m) = msgs.get_mut(i) {
            if !m["ui"].is_object() {
                m["ui"] = json!({});
            }
            m["ui"]["elided"] = json!(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, name: &str, path: &str) -> Value {
        json!({"role":"assistant","content":"","tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":json!({"path":path}).to_string()}}]})
    }
    fn result(id: &str, name: &str, size: usize) -> Value {
        json!({"role":"tool","tool_call_id":id,"name":name,"content":"x".repeat(size)})
    }

    #[test]
    fn under_budget_prunes_nothing() {
        let msgs = vec![json!({"role":"user","content":"hi"}), call("a", "read_file", "f"), result("a", "read_file", 1000)];
        assert!(plan_prune(&msgs, &Budget::new(16_384, 2000), 2).is_empty());
    }

    #[test]
    fn prunes_superseded_then_oldest_and_keeps_recent() {
        let mut msgs = vec![json!({"role":"user","content":"go"})];
        // f read twice: the first read is superseded and goes first even though "g" is older
        for (id, name, path) in [("g", "read_file", "g"), ("a", "read_file", "f"), ("b", "list_dir", "."), ("c", "read_file", "f"), ("d", "read_file", "h")] {
            msgs.push(call(id, name, path));
            msgs.push(result(id, name, 12_000));
        }
        let b = Budget::new(16_384, 2000);
        let p = plan_prune(&msgs, &b, 2);
        assert!(!p.is_empty());
        assert_eq!(p[0], 4, "superseded read of f first");
        // the two most recent results are protected
        assert!(!p.contains(&8) && !p.contains(&10));
        let before = b.used(&msgs);
        apply(&mut msgs, &p);
        // back under the trigger; the protected recent results alone are ~half the window here
        assert!(b.used(&msgs) < b.available() * 85 / 100, "{} -> {}", before, b.used(&msgs));
        assert_eq!(p.len(), 3);
        assert!(stub(&msgs[4]).contains("read_file output removed"));
        // already-elided messages are stable: a second pass with the same history does nothing new
        assert!(plan_prune(&msgs, &b, 2).is_empty());
    }
}
