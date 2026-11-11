//! GGUF header reader (v2/v3). Parses metadata and tensor infos only; never reads tensor data.

use super::GgufSummary;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"GGUF";
/// Arrays longer than this (tokenizer vocabularies etc.) are skipped, not stored.
const MAX_STORED_ARRAY: u64 = 4096;
/// Sanity limits so a corrupt header can't make us allocate gigabytes.
const MAX_STRING: u64 = 16 * 1024 * 1024;
const MAX_COUNT: u64 = 100_000_000;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    U(u64),
    I(i64),
    F(f64),
    Bool(bool),
    Str(String),
    Arr(Vec<Value>),
    /// An array too long to keep (e.g. tokenizer tokens).
    Skipped,
}

impl Value {
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::U(v) => Some(*v),
            Value::I(v) if *v >= 0 => Some(*v as u64),
            Value::F(v) if *v >= 0.0 => Some(*v as u64),
            Value::Bool(b) => Some(*b as u64),
            _ => None,
        }
    }
    pub fn as_u32(&self) -> Option<u32> {
        self.as_u64().map(|v| v.min(u32::MAX as u64) as u32)
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TensorInfo {
    pub name: String,
    pub dims: Vec<u64>,
    pub ggml_type: u32,
}

#[derive(Debug, Clone)]
pub struct Header {
    pub kv: HashMap<String, Value>,
    pub tensors: Vec<TensorInfo>,
}

struct R<T: Read + Seek> {
    inner: T,
}

impl<T: Read + Seek> R<T> {
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let mut b = [0u8; N];
        self.inner.read_exact(&mut b).map_err(|e| format!("truncated GGUF header ({e})"))?;
        Ok(b)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.bytes::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.bytes()?))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.bytes()?))
    }
    fn count(&mut self) -> Result<u64, String> {
        let n = self.u64()?;
        if n > MAX_COUNT {
            return Err(format!("implausible count {n} in GGUF header"));
        }
        Ok(n)
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.u64()?;
        if n > MAX_STRING {
            return Err(format!("implausible string length {n} in GGUF header"));
        }
        let mut buf = vec![0u8; n as usize];
        self.inner.read_exact(&mut buf).map_err(|e| format!("truncated GGUF header ({e})"))?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }
    fn skip(&mut self, n: u64) -> Result<(), String> {
        self.inner
            .seek(std::io::SeekFrom::Current(n as i64))
            .map(|_| ())
            .map_err(|e| format!("GGUF seek failed ({e})"))
    }
    fn value(&mut self, ty: u32, depth: u32) -> Result<Value, String> {
        Ok(match ty {
            0 => Value::U(self.u8()? as u64),
            1 => Value::I(self.u8()? as i8 as i64),
            2 => Value::U(self.u16()? as u64),
            3 => Value::I(self.u16()? as i16 as i64),
            4 => Value::U(self.u32()? as u64),
            5 => Value::I(self.u32()? as i32 as i64),
            6 => Value::F(f32::from_le_bytes(self.bytes()?) as f64),
            7 => Value::Bool(self.u8()? != 0),
            8 => Value::Str(self.string()?),
            9 => {
                if depth > 8 {
                    return Err("GGUF arrays nested too deeply".into());
                }
                let elem = self.u32()?;
                let n = self.count()?;
                if n > MAX_STORED_ARRAY {
                    self.skip_array(elem, n, depth)?;
                    Value::Skipped
                } else {
                    let mut v = Vec::with_capacity(n as usize);
                    for _ in 0..n {
                        v.push(self.value(elem, depth + 1)?);
                    }
                    Value::Arr(v)
                }
            }
            10 => Value::U(self.u64()?),
            11 => Value::I(self.u64()? as i64),
            12 => Value::F(f64::from_le_bytes(self.bytes()?)),
            t => return Err(format!("unknown GGUF metadata type {t}")),
        })
    }
    fn skip_array(&mut self, elem: u32, n: u64, depth: u32) -> Result<(), String> {
        let fixed = match elem {
            0 | 1 | 7 => Some(1),
            2 | 3 => Some(2),
            4..=6 => Some(4),
            10..=12 => Some(8),
            _ => None,
        };
        if let Some(sz) = fixed {
            return self.skip(sz * n);
        }
        for _ in 0..n {
            match elem {
                8 => {
                    let len = self.u64()?;
                    if len > MAX_STRING {
                        return Err(format!("implausible string length {len} in GGUF header"));
                    }
                    self.skip(len)?;
                }
                _ => {
                    self.value(elem, depth + 1)?;
                }
            }
        }
        Ok(())
    }
}

/// Parse the header (metadata + tensor infos) of one GGUF file.
pub fn read_header(path: &Path) -> Result<Header, String> {
    let f = File::open(path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    let mut r = R { inner: BufReader::with_capacity(1 << 16, f) };
    let magic = r.bytes::<4>()?;
    if &magic != MAGIC {
        return Err(format!("{} is not a GGUF file", path.display()));
    }
    let version = r.u32()?;
    if !(2..=3).contains(&version) {
        return Err(format!("Unsupported GGUF version {version} (only v2 and v3 are supported)"));
    }
    let tensor_count = r.count()?;
    let kv_count = r.count()?;
    let mut kv = HashMap::new();
    for _ in 0..kv_count {
        let key = r.string()?;
        let ty = r.u32()?;
        let v = r.value(ty, 0)?;
        kv.insert(key, v);
    }
    let mut tensors = Vec::with_capacity(tensor_count.min(65536) as usize);
    for _ in 0..tensor_count {
        let name = r.string()?;
        let n_dims = r.u32()?;
        if n_dims > 8 {
            return Err(format!("tensor {name} has {n_dims} dimensions"));
        }
        let mut dims = Vec::with_capacity(n_dims as usize);
        for _ in 0..n_dims {
            dims.push(r.u64()?);
        }
        let ggml_type = r.u32()?;
        let _data_offset = r.u64()?;
        tensors.push(TensorInfo { name, dims, ggml_type });
    }
    Ok(Header { kv, tensors })
}

/// (block size in elements, bytes per block) for a ggml tensor type.
pub fn ggml_block(ty: u32) -> Option<(u64, u64)> {
    Some(match ty {
        0 => (1, 4),      // F32
        1 => (1, 2),      // F16
        2 => (32, 18),    // Q4_0
        3 => (32, 20),    // Q4_1
        6 => (32, 22),    // Q5_0
        7 => (32, 24),    // Q5_1
        8 => (32, 34),    // Q8_0
        9 => (32, 36),    // Q8_1
        10 => (256, 84),  // Q2_K
        11 => (256, 110), // Q3_K
        12 => (256, 144), // Q4_K
        13 => (256, 176), // Q5_K
        14 => (256, 210), // Q6_K
        15 => (256, 292), // Q8_K
        16 => (256, 66),  // IQ2_XXS
        17 => (256, 74),  // IQ2_XS
        18 => (256, 98),  // IQ3_XXS
        19 => (256, 50),  // IQ1_S
        20 => (32, 18),   // IQ4_NL
        21 => (256, 110), // IQ3_S
        22 => (256, 82),  // IQ2_S
        23 => (256, 136), // IQ4_XS
        24 => (1, 1),     // I8
        25 => (1, 2),     // I16
        26 => (1, 4),     // I32
        27 => (1, 8),     // I64
        28 => (1, 8),     // F64
        29 => (256, 56),  // IQ1_M
        30 => (1, 2),     // BF16
        34 => (256, 54),  // TQ1_0
        35 => (256, 66),  // TQ2_0
        39 => (32, 17),   // MXFP4
        _ => return None,
    })
}

pub fn tensor_bytes(t: &TensorInfo) -> Option<u64> {
    let (blk, sz) = ggml_block(t.ggml_type)?;
    let n: u64 = t.dims.iter().product();
    Some(n.div_ceil(blk) * sz)
}

/// llama_ftype (general.file_type) → name.
pub fn file_type_name(ft: u64) -> Option<&'static str> {
    Some(match ft {
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
        36 => "TQ1_0",
        37 => "TQ2_0",
        38 => "MXFP4_MOE",
        _ => return None,
    })
}

/// Architectures that mix full-attention layers with recurrent (SSM / linear-attention) layers.
const HYBRID_ARCHS: &[&str] = &[
    "qwen3next",
    "qwen35",
    "qwen35moe",
    "jamba",
    "falcon-h1",
    "granitehybrid",
    "nemotron_h",
    "nemotron_h_moe",
    "lfm2",
    "lfm2moe",
    "plamo2",
    "kimi_linear",
];
/// Pure recurrent architectures: no KV cache at all.
const RECURRENT_ARCHS: &[&str] = &["mamba", "mamba2", "rwkv6", "rwkv6qwen2", "rwkv7", "arwkv7"];

/// Split-file naming: `<prefix>-00001-of-00003.gguf`.
pub fn split_parts(file_name: &str) -> Option<(String, u32, u32)> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"(?i)^(.*)-(\d{5})-of-(\d{5})\.gguf$").unwrap());
    let c = re.captures(file_name)?;
    Some((c[1].to_string(), c[2].parse().ok()?, c[3].parse().ok()?))
}

/// All shard paths for a split model (just `path` for a single file).
pub fn shard_paths(path: &Path) -> Vec<PathBuf> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match split_parts(&name) {
        Some((prefix, _, total)) => (1..=total)
            .map(|i| path.with_file_name(format!("{prefix}-{i:05}-of-{total:05}.gguf")))
            .collect(),
        None => vec![path.to_path_buf()],
    }
}

/// Read the GGUF header and summarize what the fit calculator needs. For split models
/// (`-00001-of-0000N`), the tensor infos of every shard present on disk are included.
pub fn read_summary(path: &Path) -> Result<GgufSummary, String> {
    let header = read_header(path)?;
    let mut tensors = header.tensors.clone();
    let shards = shard_paths(path);
    if shards.len() > 1 {
        tensors.clear();
        for p in &shards {
            if p.exists() {
                tensors.extend(read_header(p)?.tensors);
            }
        }
    }
    Ok(summarize(&header.kv, &tensors))
}

pub fn summarize(kv: &HashMap<String, Value>, tensors: &[TensorInfo]) -> GgufSummary {
    let arch = kv.get("general.architecture").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let k = |suffix: &str| kv.get(&format!("{arch}.{suffix}"));
    let ku32 = |suffix: &str| k(suffix).and_then(scalar_or_max);

    let block_count = ku32("block_count").unwrap_or(0);
    let embedding_length = ku32("embedding_length");
    let head_count = ku32("attention.head_count");
    let head_count_kv = ku32("attention.head_count_kv").or(head_count);
    let per_head = match (embedding_length, head_count) {
        (Some(e), Some(h)) if h > 0 => Some(e / h),
        _ => None,
    };
    let expert_count = ku32("expert_count").filter(|&n| n > 0);
    let expert_used_count = ku32("expert_used_count").filter(|&n| n > 0);

    // KV layers.
    let (kv_layers, kv_source) = if let Some(Value::Arr(a)) = k("attention.head_count_kv") {
        let n = a.iter().filter(|v| v.as_u64().unwrap_or(0) > 0).count() as u32;
        (Some(n), "exact")
    } else if let Some(interval) = ku32("full_attention_interval").filter(|&i| i > 0) {
        (Some(block_count / interval), "exact")
    } else if RECURRENT_ARCHS.contains(&arch.as_str()) {
        (Some(0), "exact")
    } else if HYBRID_ARCHS.contains(&arch.as_str()) {
        (Some(block_count), "assumed")
    } else {
        (Some(block_count), "exact")
    };

    // Recurrent state (per sequence, f32), from ssm.* keys.
    let recurrent_state_bytes = {
        let conv = ku32("ssm.conv_kernel").map(u64::from);
        let state = ku32("ssm.state_size").map(u64::from);
        let inner = ku32("ssm.inner_size").map(u64::from);
        let groups = ku32("ssm.group_count").map(u64::from).unwrap_or(0);
        // ssm.time_step_rank (number of value heads) doesn't change the total: the
        // per-head state is state_size x (inner_size / heads), summed over heads.
        match (conv, state, inner) {
            (Some(c), Some(s), Some(i)) => {
                let per_layer = c.saturating_sub(1) * (i + 2 * groups * s) + s * i;
                let rec_layers = (block_count as u64).saturating_sub(kv_layers.unwrap_or(0) as u64);
                let layers = if rec_layers == 0 && kv_source == "assumed" { block_count as u64 } else { rec_layers };
                Some(per_layer * layers * 4)
            }
            _ => None,
        }
    };

    let mut total: Option<u64> = Some(0);
    let mut experts: u64 = 0;
    for t in tensors {
        match tensor_bytes(t) {
            Some(b) => {
                total = total.map(|x| x + b);
                if t.name.contains("_exps") {
                    experts += b;
                }
            }
            None => total = None,
        }
    }
    if tensors.is_empty() {
        total = None;
    }
    let is_moe = expert_count.is_some_and(|n| n > 1) || experts > 0;

    GgufSummary {
        name: kv.get("general.name").and_then(|v| v.as_str()).map(str::to_string),
        block_count,
        context_length: ku32("context_length"),
        embedding_length,
        head_count,
        head_count_kv,
        key_length: ku32("attention.key_length").or(per_head),
        value_length: ku32("attention.value_length").or(per_head),
        expert_count,
        expert_used_count,
        file_type: kv
            .get("general.file_type")
            .and_then(|v| v.as_u64())
            .map(|ft| file_type_name(ft).map(str::to_string).unwrap_or_else(|| format!("type {ft}"))),
        kv_layers,
        kv_layers_source: kv_source.into(),
        recurrent_state_bytes,
        is_moe,
        expert_tensor_bytes: total.map(|_| experts),
        total_tensor_bytes: total,
        architecture: arch,
    }
}

/// A scalar value, or the max of a per-layer array.
fn scalar_or_max(v: &Value) -> Option<u32> {
    match v {
        Value::Arr(a) => a.iter().filter_map(|x| x.as_u32()).max(),
        other => other.as_u32(),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Minimal GGUF writer for tests.
    pub enum W {
        U32(u32),
        U64(u64),
        Str(&'static str),
        ArrI32(Vec<i32>),
        ArrStr(Vec<String>),
        ArrArrU8(Vec<Vec<u8>>),
        F32(f32),
        Bool(bool),
    }

    fn put_str(b: &mut Vec<u8>, s: &str) {
        b.extend((s.len() as u64).to_le_bytes());
        b.extend(s.as_bytes());
    }

    pub fn write_gguf(path: &Path, kv: &[(&str, W)], tensors: &[(&str, Vec<u64>, u32)]) {
        let mut b = Vec::new();
        b.extend(b"GGUF");
        b.extend(3u32.to_le_bytes());
        b.extend((tensors.len() as u64).to_le_bytes());
        b.extend((kv.len() as u64).to_le_bytes());
        for (k, v) in kv {
            put_str(&mut b, k);
            match v {
                W::U32(x) => {
                    b.extend(4u32.to_le_bytes());
                    b.extend(x.to_le_bytes());
                }
                W::U64(x) => {
                    b.extend(10u32.to_le_bytes());
                    b.extend(x.to_le_bytes());
                }
                W::F32(x) => {
                    b.extend(6u32.to_le_bytes());
                    b.extend(x.to_le_bytes());
                }
                W::Bool(x) => {
                    b.extend(7u32.to_le_bytes());
                    b.push(*x as u8);
                }
                W::Str(s) => {
                    b.extend(8u32.to_le_bytes());
                    put_str(&mut b, s);
                }
                W::ArrI32(a) => {
                    b.extend(9u32.to_le_bytes());
                    b.extend(5u32.to_le_bytes());
                    b.extend((a.len() as u64).to_le_bytes());
                    for x in a {
                        b.extend(x.to_le_bytes());
                    }
                }
                W::ArrStr(a) => {
                    b.extend(9u32.to_le_bytes());
                    b.extend(8u32.to_le_bytes());
                    b.extend((a.len() as u64).to_le_bytes());
                    for s in a {
                        put_str(&mut b, s);
                    }
                }
                W::ArrArrU8(a) => {
                    b.extend(9u32.to_le_bytes());
                    b.extend(9u32.to_le_bytes());
                    b.extend((a.len() as u64).to_le_bytes());
                    for inner in a {
                        b.extend(0u32.to_le_bytes());
                        b.extend((inner.len() as u64).to_le_bytes());
                        b.extend(inner);
                    }
                }
            }
        }
        let mut off = 0u64;
        for (name, dims, ty) in tensors {
            put_str(&mut b, name);
            b.extend((dims.len() as u32).to_le_bytes());
            for d in dims {
                b.extend(d.to_le_bytes());
            }
            b.extend(ty.to_le_bytes());
            b.extend(off.to_le_bytes());
            off += 4096;
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b).unwrap();
    }

    pub fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nl-{tag}-{}", crate::settings::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn dense_transformer_summary() {
        let dir = tmp_dir("gguf-dense");
        let p = dir.join("m.gguf");
        let vocab: Vec<String> = (0..5000).map(|i| format!("tok{i}")).collect();
        write_gguf(
            &p,
            &[
                ("general.architecture", W::Str("llama")),
                ("general.name", W::Str("Test Llama")),
                ("general.file_type", W::U32(15)),
                ("llama.block_count", W::U32(32)),
                ("llama.context_length", W::U32(131072)),
                ("llama.embedding_length", W::U32(4096)),
                ("llama.attention.head_count", W::U32(32)),
                ("llama.attention.head_count_kv", W::U32(8)),
                ("llama.rope.freq_base", W::F32(500000.0)),
                ("llama.use_parallel_residual", W::Bool(true)),
                ("tokenizer.ggml.tokens", W::ArrStr(vocab)),
                ("tokenizer.ggml.merges_nested", W::ArrArrU8(vec![vec![1, 2], vec![3]])),
                ("general.size", W::U64(1)),
            ],
            &[
                ("token_embd.weight", vec![4096, 1000], 14), // Q6_K
                ("blk.0.attn_q.weight", vec![4096, 4096], 12), // Q4_K
                ("output_norm.weight", vec![4096], 0),       // F32
                ("blk.0.x.weight", vec![256, 4], 30),        // BF16
                ("blk.0.y.weight", vec![256, 4], 39),        // MXFP4
                ("blk.0.z.weight", vec![256, 4], 23),        // IQ4_XS
            ],
        );
        let s = read_summary(&p).unwrap();
        assert_eq!(s.architecture, "llama");
        assert_eq!(s.name.as_deref(), Some("Test Llama"));
        assert_eq!(s.file_type.as_deref(), Some("Q4_K_M"));
        assert_eq!(s.block_count, 32);
        assert_eq!(s.context_length, Some(131072));
        assert_eq!(s.head_count_kv, Some(8));
        assert_eq!(s.key_length, Some(128));
        assert_eq!(s.value_length, Some(128));
        assert_eq!(s.kv_layers, Some(32));
        assert_eq!(s.kv_layers_source, "exact");
        assert!(!s.is_moe);
        let expect = 4096 * 1000 / 256 * 210 + 4096 * 4096 / 256 * 144 + 4096 * 4 + 1024 * 2 + 1024 / 32 * 17 + 1024 / 256 * 136;
        assert_eq!(s.total_tensor_bytes, Some(expect));
        assert_eq!(s.expert_tensor_bytes, Some(0));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn hybrid_per_layer_array_and_split_moe() {
        let dir = tmp_dir("gguf-hybrid");
        // Per-layer head_count_kv: 0 = recurrent layer.
        let p1 = dir.join("Qwen3.6-35B-A3B-UD-Q4_K_M-00001-of-00002.gguf");
        let p2 = dir.join("Qwen3.6-35B-A3B-UD-Q4_K_M-00002-of-00002.gguf");
        let per_layer: Vec<i32> = (0..40).map(|i| if (i + 1) % 4 == 0 { 2 } else { 0 }).collect();
        write_gguf(
            &p1,
            &[
                ("general.architecture", W::Str("qwen35moe")),
                ("general.file_type", W::U32(38)),
                ("qwen35moe.block_count", W::U32(40)),
                ("qwen35moe.context_length", W::U32(262144)),
                ("qwen35moe.embedding_length", W::U32(2048)),
                ("qwen35moe.attention.head_count", W::U32(16)),
                ("qwen35moe.attention.head_count_kv", W::ArrI32(per_layer)),
                ("qwen35moe.attention.key_length", W::U32(256)),
                ("qwen35moe.attention.value_length", W::U32(256)),
                ("qwen35moe.expert_count", W::U32(256)),
                ("qwen35moe.expert_used_count", W::U32(8)),
                ("qwen35moe.ssm.conv_kernel", W::U32(4)),
                ("qwen35moe.ssm.state_size", W::U32(128)),
                ("qwen35moe.ssm.group_count", W::U32(16)),
                ("qwen35moe.ssm.inner_size", W::U32(4096)),
                ("qwen35moe.ssm.time_step_rank", W::U32(32)),
            ],
            &[("blk.0.ffn_up_exps.weight", vec![2048, 512, 256], 8), ("blk.0.attn_norm.weight", vec![2048], 0)],
        );
        write_gguf(
            &p2,
            &[("split.no", W::U32(1))],
            &[("blk.1.ffn_down_exps.weight", vec![512, 2048, 256], 39)],
        );
        let s = read_summary(&p1).unwrap();
        assert_eq!(s.file_type.as_deref(), Some("MXFP4_MOE"));
        assert!(s.is_moe);
        assert_eq!(s.head_count_kv, Some(2));
        assert_eq!(s.kv_layers, Some(10));
        assert_eq!(s.kv_layers_source, "exact");
        assert_eq!(s.key_length, Some(256));
        let n: u64 = 2048 * 512 * 256;
        let exps = n / 32 * 34 + n / 32 * 17;
        assert_eq!(s.expert_tensor_bytes, Some(exps));
        assert_eq!(s.total_tensor_bytes, Some(exps + 2048 * 4));
        // 30 recurrent layers * ((4-1)*(4096 + 2*16*128) + 128*4096) * 4 bytes
        assert_eq!(s.recurrent_state_bytes, Some(30 * (3 * (4096 + 4096) + 128 * 4096) * 4));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn full_attention_interval_and_assumed_hybrid() {
        let dir = tmp_dir("gguf-interval");
        let p = dir.join("a.gguf");
        write_gguf(
            &p,
            &[
                ("general.architecture", W::Str("qwen3next")),
                ("qwen3next.block_count", W::U32(48)),
                ("qwen3next.full_attention_interval", W::U32(4)),
                ("qwen3next.attention.head_count", W::U32(16)),
                ("qwen3next.attention.head_count_kv", W::U32(2)),
            ],
            &[],
        );
        let s = read_summary(&p).unwrap();
        assert_eq!((s.kv_layers, s.kv_layers_source.as_str()), (Some(12), "exact"));
        assert_eq!(s.total_tensor_bytes, None);

        let q = dir.join("b.gguf");
        write_gguf(
            &q,
            &[("general.architecture", W::Str("jamba")), ("jamba.block_count", W::U32(32))],
            &[],
        );
        let s = read_summary(&q).unwrap();
        assert_eq!((s.kv_layers, s.kv_layers_source.as_str()), (Some(32), "assumed"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn rejects_bad_files() {
        let dir = tmp_dir("gguf-bad");
        let p = dir.join("x.gguf");
        std::fs::write(&p, b"NOPE").unwrap();
        assert!(read_summary(&p).is_err());
        std::fs::write(&p, b"GGUF\x03\x00\x00\x00").unwrap();
        assert!(read_summary(&p).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn split_names() {
        assert_eq!(split_parts("Foo-Q4_K_M-00002-of-00003.gguf"), Some(("Foo-Q4_K_M".into(), 2, 3)));
        assert_eq!(split_parts("foo.gguf"), None);
        let shards = shard_paths(Path::new("/m/Foo-00001-of-00002.gguf"));
        assert_eq!(shards[1], Path::new("/m/Foo-00002-of-00002.gguf"));
    }
}
