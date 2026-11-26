//! Making tool calls from small local models usable:
//! - recover calls a model wrote as text instead of the OpenAI `tool_calls` field
//! - repair slightly broken JSON arguments
//! - map common argument-name aliases (`file_path` -> `path`) and coerce types to the schema
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::OnceLock;

/// Best-effort JSON parse: strict first, then progressively more lenient repairs.
pub fn repair_json(raw: &str) -> Option<Value> {
    let s = raw.trim();
    if s.is_empty() {
        return Some(Value::Object(Map::new()));
    }
    if let Ok(v) = serde_json::from_str::<Value>(s) {
        return Some(v);
    }
    let s = strip_fence(s);
    if let Ok(v) = serde_json::from_str::<Value>(s) {
        return Some(v);
    }
    // models sometimes wrap the object in prose; take the outermost {...}
    let s = match (s.find('{'), s.rfind('}')) {
        (Some(a), Some(b)) if b > a => &s[a..=b],
        (Some(a), None) => &s[a..],
        _ => s,
    };
    let mut fixed = escape_raw_control_chars(s);
    if !fixed.contains('"') && fixed.contains('\'') {
        fixed = fixed.replace('\'', "\"");
    }
    fixed = remove_trailing_commas(&fixed);
    fixed = close_unbalanced(&fixed);
    serde_json::from_str::<Value>(&fixed).ok()
}

fn strip_fence(s: &str) -> &str {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```") {
        let rest = rest.trim_start_matches(|c: char| c.is_ascii_alphanumeric());
        return rest.trim().trim_end_matches("```").trim();
    }
    t
}

/// Raw newlines/tabs inside string literals are invalid JSON but very common from models
/// writing file content.
fn escape_raw_control_chars(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    let (mut in_str, mut esc) = (false, false);
    for c in s.chars() {
        if in_str {
            if esc {
                esc = false;
                out.push(c);
                continue;
            }
            match c {
                '\\' => {
                    esc = true;
                    out.push(c);
                }
                '"' => {
                    in_str = false;
                    out.push(c);
                }
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        } else {
            if c == '"' {
                in_str = true;
            }
            out.push(c);
        }
    }
    out
}

fn remove_trailing_commas(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r",(\s*[}\]])").unwrap());
    re.replace_all(s, "$1").into_owned()
}

/// Close strings/brackets a model stopped generating before finishing.
fn close_unbalanced(s: &str) -> String {
    let mut stack = vec![];
    let (mut in_str, mut esc) = (false, false);
    for c in s.chars() {
        if in_str {
            match (esc, c) {
                (true, _) => esc = false,
                (false, '\\') => esc = true,
                (false, '"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                stack.pop();
            }
            _ => {}
        }
    }
    let mut out = s.to_string();
    if in_str {
        out.push('"');
    }
    while let Some(c) = stack.pop() {
        out.push(c);
    }
    out
}

/// A tool call recovered from free text.
#[derive(Debug, Clone, PartialEq)]
pub struct TextCall {
    pub name: String,
    pub args: Value,
}

fn call_from_obj(v: &Value, known: &dyn Fn(&str) -> bool) -> Option<TextCall> {
    let o = v.as_object()?;
    let name = o.get("name").or_else(|| o.get("function").and_then(|f| f.get("name"))).or_else(|| o.get("tool")).and_then(|n| n.as_str())?.to_string();
    if !known(&name) {
        return None;
    }
    let raw = o
        .get("arguments")
        .or_else(|| o.get("parameters"))
        .or_else(|| o.get("args"))
        .or_else(|| o.get("function").and_then(|f| f.get("arguments")))
        .cloned()
        .unwrap_or(Value::Object(Map::new()));
    let args = match raw {
        Value::String(s) => repair_json(&s).unwrap_or(Value::Null),
        v => v,
    };
    Some(TextCall { name, args })
}

/// Find tool calls written into the message text. Returns the calls and the text with them removed.
/// `known` limits matches to real tool names so ordinary JSON in an answer isn't executed.
pub fn extract_text_calls(content: &str, known: &dyn Fn(&str) -> bool) -> (Vec<TextCall>, String) {
    static TAG: OnceLock<Regex> = OnceLock::new();
    static FUNC: OnceLock<Regex> = OnceLock::new();
    static MISTRAL: OnceLock<Regex> = OnceLock::new();
    static FENCE: OnceLock<Regex> = OnceLock::new();
    let tag = TAG.get_or_init(|| Regex::new(r"(?s)<tool_call>\s*(.*?)\s*(?:</tool_call>|$)").unwrap());
    let func = FUNC.get_or_init(|| Regex::new(r"(?s)<function=([A-Za-z0-9_\-]+)>\s*(.*?)\s*</function>").unwrap());
    let mistral = MISTRAL.get_or_init(|| Regex::new(r"(?s)\[TOOL_CALLS\]\s*(\[.*\]|\{.*\})").unwrap());
    let fence = FENCE.get_or_init(|| Regex::new(r"(?s)```(?:json|tool_call)?\s*(\{.*?\})\s*```").unwrap());

    let mut calls = vec![];
    let mut spans: Vec<(usize, usize)> = vec![];

    for c in tag.captures_iter(content) {
        if let Some(v) = repair_json(&c[1]) {
            let items = if let Value::Array(a) = v { a } else { vec![v] };
            for it in items {
                if let Some(tc) = call_from_obj(&it, known) {
                    calls.push(tc);
                    let m = c.get(0).unwrap();
                    spans.push((m.start(), m.end()));
                }
            }
        }
    }
    if calls.is_empty() {
        for c in func.captures_iter(content) {
            let name = c[1].to_string();
            if !known(&name) {
                continue;
            }
            let args = repair_json(&c[2]).unwrap_or(Value::Null);
            calls.push(TextCall { name, args });
            let m = c.get(0).unwrap();
            spans.push((m.start(), m.end()));
        }
    }
    if calls.is_empty() {
        if let Some(c) = mistral.captures(content) {
            if let Some(v) = repair_json(&c[1]) {
                let items = if let Value::Array(a) = v { a } else { vec![v] };
                for it in items {
                    if let Some(tc) = call_from_obj(&it, known) {
                        calls.push(tc);
                    }
                }
                if !calls.is_empty() {
                    let m = c.get(0).unwrap();
                    spans.push((m.start(), m.end()));
                }
            }
        }
    }
    if calls.is_empty() {
        for c in fence.captures_iter(content) {
            if let Some(v) = repair_json(&c[1]) {
                if let Some(tc) = call_from_obj(&v, known) {
                    calls.push(tc);
                    let m = c.get(0).unwrap();
                    spans.push((m.start(), m.end()));
                }
            }
        }
    }
    if calls.is_empty() {
        // the whole message is a bare JSON call
        let t = content.trim();
        if t.starts_with('{') && t.ends_with('}') {
            if let Some(v) = repair_json(t) {
                if let Some(tc) = call_from_obj(&v, known) {
                    return (vec![tc], String::new());
                }
            }
        }
        return (vec![], content.to_string());
    }
    spans.sort();
    let mut rest = String::new();
    let mut at = 0;
    for (a, b) in spans {
        if a >= at {
            rest.push_str(&content[at..a]);
            at = b;
        }
    }
    rest.push_str(&content[at..]);
    (calls, rest.trim().to_string())
}

/// Aliases models use for our argument names, keyed by canonical name.
const ALIASES: &[(&str, &[&str])] = &[
    ("path", &["file_path", "filepath", "filename", "file", "file_name", "dir", "directory", "target_file"]),
    ("content", &["contents", "text", "data", "file_content", "code"]),
    ("old_string", &["old", "old_str", "old_text", "search", "find", "original"]),
    ("new_string", &["new", "new_str", "new_text", "replace", "replacement", "updated"]),
    ("command", &["cmd", "shell", "script"]),
    ("pattern", &["regex", "query", "glob_pattern"]),
    ("new_text", &["text", "content", "replacement", "code"]),
];

/// Rename aliased keys to the names in `schema_props` and coerce scalar types to match.
pub fn normalize_args(args: &mut Value, schema_props: &Map<String, Value>) {
    let Some(o) = args.as_object_mut() else { return };
    for (canon, aliases) in ALIASES {
        if !schema_props.contains_key(*canon) || o.contains_key(*canon) {
            continue;
        }
        if let Some(a) = aliases.iter().find(|a| !schema_props.contains_key(**a) && o.contains_key(**a)) {
            let v = o.remove(*a).unwrap();
            o.insert(canon.to_string(), v);
        }
    }
    for (k, spec) in schema_props {
        let Some(v) = o.get_mut(k) else { continue };
        match (spec.get("type").and_then(|t| t.as_str()), &*v) {
            (Some("integer"), Value::String(s)) => {
                if let Ok(n) = s.trim().parse::<i64>() {
                    *v = Value::from(n);
                }
            }
            (Some("integer"), Value::Number(n)) if n.is_f64() => {
                *v = Value::from(n.as_f64().unwrap_or(0.0) as i64);
            }
            (Some("boolean"), Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "1" => *v = Value::Bool(true),
                "false" | "no" | "0" => *v = Value::Bool(false),
                _ => {}
            },
            (Some("string"), Value::Number(n)) => *v = Value::String(n.to_string()),
            (Some("string"), Value::Bool(b)) => *v = Value::String(b.to_string()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn known(n: &str) -> bool {
        matches!(n, "read_file" | "edit_file" | "list_dir")
    }

    #[test]
    fn repairs_common_json_breakage() {
        assert_eq!(repair_json(r#"{"a":1,}"#).unwrap(), json!({"a":1}));
        assert_eq!(repair_json("{'a': 'b'}").unwrap(), json!({"a":"b"}));
        assert_eq!(repair_json("{\"c\":\"line1\nline2\"}").unwrap(), json!({"c":"line1\nline2"}));
        assert_eq!(repair_json(r#"{"a":{"b":"x"#).unwrap(), json!({"a":{"b":"x"}}));
        assert_eq!(repair_json("```json\n{\"a\":2}\n```").unwrap(), json!({"a":2}));
        assert_eq!(repair_json("Sure: {\"a\":3} done").unwrap(), json!({"a":3}));
        assert_eq!(repair_json("").unwrap(), json!({}));
    }

    #[test]
    fn extracts_hermes_qwen_calls() {
        let t = "I'll read it.\n<tool_call>\n{\"name\": \"read_file\", \"arguments\": {\"path\": \"a.rs\"}}\n</tool_call>";
        let (c, rest) = extract_text_calls(t, &known);
        assert_eq!(c, vec![TextCall { name: "read_file".into(), args: json!({"path":"a.rs"}) }]);
        assert_eq!(rest, "I'll read it.");
    }

    #[test]
    fn extracts_function_tag_mistral_and_fenced() {
        let (c, _) = extract_text_calls("<function=list_dir>{\"path\":\".\"}</function>", &known);
        assert_eq!(c[0].name, "list_dir");
        let (c, _) = extract_text_calls("[TOOL_CALLS][{\"name\":\"read_file\",\"arguments\":{\"path\":\"x\"}}]", &known);
        assert_eq!(c[0].args, json!({"path":"x"}));
        let (c, _) = extract_text_calls("```json\n{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"y\\\"}\"}\n```", &known);
        assert_eq!(c[0].args, json!({"path":"y"}));
        let (c, rest) = extract_text_calls("{\"name\":\"list_dir\",\"arguments\":{}}", &known);
        assert_eq!((c.len(), rest.as_str()), (1, ""));
    }

    #[test]
    fn ignores_unknown_names_and_plain_json() {
        let t = "Here is config:\n```json\n{\"name\":\"my-app\",\"version\":\"1.0\"}\n```";
        let (c, rest) = extract_text_calls(t, &known);
        assert!(c.is_empty());
        assert_eq!(rest, t);
    }

    #[test]
    fn normalizes_aliases_and_types() {
        let props = json!({"path":{"type":"string"},"offset":{"type":"integer"},"replace_all":{"type":"boolean"},
                           "old_string":{"type":"string"},"new_string":{"type":"string"}});
        let mut a = json!({"file_path":"a.rs","offset":"10","replace_all":"true","old":"x","new":"y"});
        normalize_args(&mut a, props.as_object().unwrap());
        assert_eq!(a, json!({"path":"a.rs","offset":10,"replace_all":true,"old_string":"x","new_string":"y"}));
        // an alias that is itself a real parameter is left alone
        let props2 = json!({"path":{"type":"string"},"content":{"type":"string"},"text":{"type":"string"}});
        let mut b = json!({"path":"a","text":"t"});
        normalize_args(&mut b, props2.as_object().unwrap());
        assert_eq!(b, json!({"path":"a","text":"t"}));
    }
}
