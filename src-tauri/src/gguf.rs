//! Minimal GGUF header reader: pulls a few metadata keys without loading tensors.
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct GgufInfo {
    pub arch: String,
    pub name: String,
    pub file_type: Option<u32>,
    pub ctx_train: Option<u64>,
    pub expert_count: Option<u64>,
}

fn rd<const N: usize, R: Read>(r: &mut R) -> std::io::Result<[u8; N]> {
    let mut b = [0u8; N];
    r.read_exact(&mut b)?;
    Ok(b)
}
fn u32_<R: Read>(r: &mut R) -> std::io::Result<u32> {
    Ok(u32::from_le_bytes(rd(r)?))
}
fn u64_<R: Read>(r: &mut R) -> std::io::Result<u64> {
    Ok(u64::from_le_bytes(rd(r)?))
}
fn string<R: Read>(r: &mut R) -> std::io::Result<String> {
    let n = u64_(r)? as usize;
    if n > 1 << 24 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "string too long"));
    }
    let mut v = vec![0u8; n];
    r.read_exact(&mut v)?;
    Ok(String::from_utf8_lossy(&v).into_owned())
}

#[derive(Debug)]
enum Val {
    U(u64),
    S(String),
    Other,
}

fn scalar_size(t: u32) -> Option<i64> {
    match t {
        0 | 1 | 7 => Some(1),
        2 | 3 => Some(2),
        4 | 5 | 6 => Some(4),
        10 | 11 | 12 => Some(8),
        _ => None,
    }
}

fn read_val<R: Read + Seek>(r: &mut R, t: u32) -> std::io::Result<Val> {
    Ok(match t {
        0 => Val::U(rd::<1, _>(r)?[0] as u64),
        2 => Val::U(u16::from_le_bytes(rd(r)?) as u64),
        4 => Val::U(u32_(r)? as u64),
        10 => Val::U(u64_(r)?),
        1 | 3 | 5 | 6 | 7 | 11 | 12 => {
            r.seek(SeekFrom::Current(scalar_size(t).unwrap()))?;
            Val::Other
        }
        8 => Val::S(string(r)?),
        9 => {
            let et = u32_(r)?;
            let n = u64_(r)?;
            if let Some(sz) = scalar_size(et) {
                r.seek(SeekFrom::Current(sz * n as i64))?;
            } else if et == 8 {
                for _ in 0..n {
                    let l = u64_(r)?;
                    r.seek(SeekFrom::Current(l as i64))?;
                }
            } else {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "nested array"));
            }
            Val::Other
        }
        _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad type")),
    })
}

pub fn read_info(path: &Path) -> std::io::Result<GgufInfo> {
    let mut r = BufReader::with_capacity(1 << 16, File::open(path)?);
    if &rd::<4, _>(&mut r)? != b"GGUF" {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "not a GGUF file"));
    }
    let _ver = u32_(&mut r)?;
    let _tensors = u64_(&mut r)?;
    let kvs = u64_(&mut r)?;
    let mut info = GgufInfo::default();
    let mut ctx_key = String::new();
    let mut exp_key = String::new();
    for _ in 0..kvs {
        let key = string(&mut r)?;
        // the tokenizer arrays (100k+ strings) come after every key we need; skipping them is what made scans slow
        if key.starts_with("tokenizer.") {
            break;
        }
        let t = u32_(&mut r)?;
        let v = read_val(&mut r, t)?;
        match (key.as_str(), v) {
            ("general.architecture", Val::S(s)) => {
                ctx_key = format!("{s}.context_length");
                exp_key = format!("{s}.expert_count");
                info.arch = s;
            }
            ("general.name", Val::S(s)) => info.name = s,
            ("general.file_type", Val::U(n)) => info.file_type = Some(n as u32),
            (k, Val::U(n)) if !ctx_key.is_empty() && k == ctx_key => info.ctx_train = Some(n),
            (k, Val::U(n)) if !exp_key.is_empty() && k == exp_key => info.expert_count = Some(n),
            _ => {}
        }
    }
    Ok(info)
}

pub fn file_type_name(t: u32) -> Option<&'static str> {
    Some(match t {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        20 => "IQ2_XS",
        21 => "Q2_K_S",
        22 => "IQ3_XS",
        23 => "IQ3_XXS",
        24 => "IQ1_S",
        25 => "IQ4_NL",
        26 => "IQ3_S",
        27 => "IQ3_M",
        28 => "IQ2_S",
        29 => "IQ2_M",
        30 => "IQ4_XS",
        31 => "IQ1_M",
        32 => "BF16",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_real_qwen36() {
        let p = Path::new(r"E:\PolyStrata-data\models\Qwen3.6-35B-A3B\Qwen3.6-35B-A3B-UD-Q4_K_M.gguf");
        if !p.exists() {
            eprintln!("skipped: model file not present");
            return;
        }
        let i = read_info(p).unwrap();
        println!("{i:?}");
        assert!(!i.arch.is_empty());
        assert!(i.ctx_train.unwrap_or(0) > 1000);
    }

    #[test]
    fn rejects_non_gguf() {
        let d = std::env::temp_dir().join("harness_not_gguf.bin");
        std::fs::write(&d, b"hello world, not gguf").unwrap();
        assert!(read_info(&d).is_err());
    }
}
