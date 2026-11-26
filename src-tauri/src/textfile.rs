//! Text files as the agent sees them: decoded to a `\n`-only string for matching and editing,
//! then written back with the original encoding, BOM and line endings.
use sha2::{Digest, Sha256};
use similar::{ChangeTag, TextDiff};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}

#[derive(Debug, Clone)]
pub struct TextFile {
    /// content with `\n` line endings
    pub text: String,
    pub encoding: Encoding,
    pub bom: bool,
    pub crlf: bool,
    /// invalid UTF-8 was replaced; writing it back would change bytes
    pub lossy: bool,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Short content version shown to the model (`version 3f9a2c1b`).
pub fn version_of(bytes: &[u8]) -> String {
    sha256_hex(bytes)[..8].to_string()
}

pub fn looks_binary(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return false; // UTF-16 has NULs but is text
    }
    bytes.iter().take(8000).any(|b| *b == 0)
}

pub fn decode(bytes: &[u8]) -> TextFile {
    let (encoding, bom, body): (Encoding, bool, &[u8]) = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        (Encoding::Utf8, true, &bytes[3..])
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        (Encoding::Utf16Le, true, &bytes[2..])
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        (Encoding::Utf16Be, true, &bytes[2..])
    } else {
        (Encoding::Utf8, false, bytes)
    };
    let (raw, lossy) = match encoding {
        Encoding::Utf8 => match std::str::from_utf8(body) {
            Ok(s) => (s.to_string(), false),
            Err(_) => (String::from_utf8_lossy(body).into_owned(), true),
        },
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let units: Vec<u16> = body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| if encoding == Encoding::Utf16Le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
                .collect();
            (String::from_utf16_lossy(&units), false)
        }
    };
    let crlf_count = raw.matches("\r\n").count();
    let lf_count = raw.matches('\n').count();
    let crlf = crlf_count > 0 && crlf_count * 2 >= lf_count;
    TextFile { text: raw.replace("\r\n", "\n"), encoding, bom, crlf, lossy }
}

/// Encode `text` (with `\n` endings) using the same conventions as `like`.
pub fn encode(text: &str, like: &TextFile) -> Vec<u8> {
    let body = if like.crlf { text.replace("\r\n", "\n").replace('\n', "\r\n") } else { text.to_string() };
    let mut out = vec![];
    match like.encoding {
        Encoding::Utf8 => {
            if like.bom {
                out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            }
            out.extend_from_slice(body.as_bytes());
        }
        Encoding::Utf16Le | Encoding::Utf16Be => {
            out.extend_from_slice(if like.encoding == Encoding::Utf16Le { &[0xFF, 0xFE] } else { &[0xFE, 0xFF] });
            for u in body.encode_utf16() {
                out.extend_from_slice(&if like.encoding == Encoding::Utf16Le { u.to_le_bytes() } else { u.to_be_bytes() });
            }
        }
    }
    out
}

/// Defaults for a file that doesn't exist yet: UTF-8, no BOM, CRLF only if the text already has it.
pub fn new_file_like(text: &str) -> TextFile {
    TextFile { text: String::new(), encoding: Encoding::Utf8, bom: false, crlf: text.contains("\r\n"), lossy: false }
}

pub fn read(path: &Path) -> Result<(TextFile, Vec<u8>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if looks_binary(&bytes) {
        return Err(format!("{} is a binary file", path.display()));
    }
    Ok((decode(&bytes), bytes))
}

/// Line counts of a change, for summaries.
pub fn diff_stat(old: &str, new: &str) -> (usize, usize) {
    let d = TextDiff::from_lines(old, new);
    let (mut add, mut del) = (0, 0);
    for c in d.iter_all_changes() {
        match c.tag() {
            ChangeTag::Insert => add += 1,
            ChangeTag::Delete => del += 1,
            ChangeTag::Equal => {}
        }
    }
    (add, del)
}

/// Unified diff with `ctx` lines of context. Empty when nothing changed.
pub fn unified(old: &str, new: &str, path: &str, ctx: usize) -> String {
    if old == new {
        return String::new();
    }
    TextDiff::from_lines(old, new).unified_diff().context_radius(ctx).header(&format!("a/{path}"), &format!("b/{path}")).to_string()
}

/// A diff small enough to hand back to a local model: hunks only, capped at `max_lines`.
pub fn compact_diff(old: &str, new: &str, max_lines: usize) -> String {
    let full = TextDiff::from_lines(old, new).unified_diff().context_radius(2).to_string();
    let lines: Vec<&str> = full.lines().filter(|l| !l.starts_with("---") && !l.starts_with("+++")).collect();
    if lines.len() <= max_lines {
        return lines.join("\n");
    }
    let mut out = lines[..max_lines].join("\n");
    out.push_str(&format!("\n… ({} more diff lines not shown)", lines.len() - max_lines));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_crlf_bom_and_utf16() {
        for bytes in [b"a\r\nb\r\n".to_vec(), b"\xEF\xBB\xBFx\ny\n".to_vec(), b"no newline at end".to_vec(), {
            let mut v = vec![0xFF, 0xFE];
            for u in "h\r\ni".encode_utf16() {
                v.extend_from_slice(&u.to_le_bytes());
            }
            v
        }] {
            let f = decode(&bytes);
            assert!(!f.text.contains('\r'));
            assert_eq!(encode(&f.text, &f), bytes);
        }
    }

    #[test]
    fn edits_keep_line_endings() {
        let f = decode(b"one\r\ntwo\r\n");
        let out = encode(&f.text.replace("two", "2\n3"), &f);
        assert_eq!(out, b"one\r\n2\r\n3\r\n");
    }

    #[test]
    fn diffs_and_versions() {
        assert_eq!(diff_stat("a\nb\n", "a\nc\nd\n"), (2, 1));
        let d = compact_diff("a\nb\nc\n", "a\nB\nc\n", 40);
        assert!(d.contains("-b") && d.contains("+B"), "{d}");
        assert_eq!(version_of(b"x"), version_of(b"x"));
        assert_ne!(version_of(b"x"), version_of(b"y"));
        assert!(looks_binary(b"\0\x01"));
        assert!(!looks_binary(&[0xFF, 0xFE, b'a', 0]));
    }
}
