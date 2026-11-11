//! Scan the models folder for GGUF files, grouping split shards and attaching mmproj files.

use super::{gguf, LocalModel};
use crate::settings::Paths;
use regex::Regex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Quant name from a file name or stem, e.g. "UD-Q4_K_XL", "IQ4_XS", "Q8_0", "BF16", "MXFP4_MOE".
pub fn quant_from_name(name: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?i)(?:^|[-_./])((?:UD-)?(?:IQ\d_[A-Z]+(?:_[A-Z]+)?|TQ\d_\d|Q\d_K(?:_[A-Z]+)*|Q\d_\d|Q\d_K|MXFP4(?:_MOE)?|BF16|F16|F32))(?:[-_.]|$)",
        )
        .unwrap()
    });
    let stem = name.strip_suffix(".gguf").or_else(|| name.strip_suffix(".GGUF")).unwrap_or(name);
    let stem = gguf::split_parts(&format!("{stem}.gguf")).map(|(p, _, _)| p).unwrap_or_else(|| stem.to_string());
    // Take the last match: the quant is normally at the end of the name.
    let mut last = None;
    let mut start = 0;
    while start < stem.len() {
        match re.captures_at(&stem, start) {
            Some(c) => {
                let m = c.get(1).unwrap();
                last = Some(m.as_str().to_uppercase());
                start = m.end();
            }
            None => break,
        }
    }
    last
}

pub fn is_mmproj(file_name: &str) -> bool {
    file_name.to_lowercase().contains("mmproj")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 8 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            walk(&p, out, depth + 1);
        } else if ft.is_file()
            && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("gguf"))
        {
            out.push(p);
        }
    }
}

/// Relative path with forward slashes.
pub fn rel_id(models: &Path, p: &Path) -> String {
    p.strip_prefix(models)
        .unwrap_or(p)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn file_size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// Prefer F16, then BF16, then Q8, then anything (sorted).
fn pick_mmproj(cands: &[PathBuf]) -> Option<PathBuf> {
    let name = |p: &PathBuf| p.file_name().unwrap_or_default().to_string_lossy().to_uppercase();
    for want in ["-F16", "-BF16", "Q8"] {
        if let Some(p) = cands.iter().find(|p| name(p).contains(want)) {
            return Some(p.clone());
        }
    }
    cands.first().cloned()
}

pub fn scan(paths: &Paths, with_meta: bool) -> Vec<LocalModel> {
    scan_dir(&paths.models, with_meta)
}

pub fn scan_dir(models: &Path, with_meta: bool) -> Vec<LocalModel> {
    let mut files = Vec::new();
    walk(models, &mut files, 0);
    files.sort();

    // mmproj files by folder.
    let mut mmprojs: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    // Model groups: key = (folder, prefix-or-filename) → shard files.
    let mut groups: BTreeMap<(PathBuf, String), Vec<PathBuf>> = BTreeMap::new();
    for f in files {
        let name = f.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let dir = f.parent().unwrap_or(models).to_path_buf();
        if is_mmproj(&name) {
            mmprojs.entry(dir).or_default().push(f);
            continue;
        }
        if name.to_lowercase().contains("imatrix") {
            continue;
        }
        let key = match gguf::split_parts(&name) {
            Some((prefix, _, total)) => format!("{prefix}-of-{total:05}"),
            None => name.clone(),
        };
        groups.entry((dir, key)).or_default().push(f);
    }

    let mut out = Vec::new();
    for ((dir, _), mut shards) in groups {
        shards.sort();
        let primary = shards[0].clone();
        let file_name = primary.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let stem = match gguf::split_parts(&file_name) {
            Some((prefix, _, _)) => prefix,
            None => file_name.trim_end_matches(".gguf").trim_end_matches(".GGUF").to_string(),
        };
        let rel_dirs: Vec<String> = dir
            .strip_prefix(models)
            .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        let (org, repo_id, repo_root) = if rel_dirs.len() >= 2 {
            (
                Some(rel_dirs[0].clone()),
                Some(format!("{}/{}", rel_dirs[0], rel_dirs[1])),
                Some(models.join(&rel_dirs[0]).join(&rel_dirs[1])),
            )
        } else {
            (None, None, None)
        };
        let mmproj = mmprojs
            .get(&dir)
            .and_then(|c| pick_mmproj(c))
            .or_else(|| repo_root.as_ref().and_then(|r| mmprojs.get(r)).and_then(|c| pick_mmproj(c)));
        let meta = if with_meta { gguf::read_summary(&primary).ok() } else { None };
        let quant = quant_from_name(&file_name)
            .or_else(|| dir.file_name().and_then(|d| quant_from_name(&d.to_string_lossy())))
            .or_else(|| meta.as_ref().and_then(|m| m.file_type.clone()));
        out.push(LocalModel {
            id: rel_id(models, &primary),
            org,
            name: stem,
            primary_path: primary.to_string_lossy().into_owned(),
            size_bytes: shards.iter().map(|p| file_size(p)).sum(),
            files: shards.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
            mmproj_path: mmproj.map(|p| p.to_string_lossy().into_owned()),
            quant,
            source_known: repo_id.is_some(),
            repo_id,
            meta,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::gguf::tests::{tmp_dir, write_gguf, W};

    #[test]
    fn quant_names() {
        let q = |s: &str| quant_from_name(s);
        assert_eq!(q("Qwen3.6-35B-A3B-UD-Q4_K_M.gguf").as_deref(), Some("UD-Q4_K_M"));
        assert_eq!(q("Qwen3.6-35B-A3B-UD-Q5_K_XL.gguf").as_deref(), Some("UD-Q5_K_XL"));
        assert_eq!(q("Qwen3.6-35B-A3B-UD-IQ4_NL_XL.gguf").as_deref(), Some("UD-IQ4_NL_XL"));
        assert_eq!(q("model-IQ4_XS.gguf").as_deref(), Some("IQ4_XS"));
        assert_eq!(q("model.Q8_0.gguf").as_deref(), Some("Q8_0"));
        assert_eq!(q("llama-3-8b-instruct-q4_k_m.gguf").as_deref(), Some("Q4_K_M"));
        assert_eq!(q("Qwen3.6-35B-A3B-BF16-00001-of-00002.gguf").as_deref(), Some("BF16"));
        assert_eq!(q("Qwen3.6-35B-A3B-MXFP4_MOE.gguf").as_deref(), Some("MXFP4_MOE"));
        assert_eq!(q("gemma-f16.gguf").as_deref(), Some("F16"));
        assert_eq!(q("plain-model.gguf"), None);
    }

    #[test]
    fn scans_layout() {
        let root = tmp_dir("scan");
        let repo = root.join("unsloth").join("Qwen3.6-35B-A3B-GGUF");
        let kv = || vec![("general.architecture", W::Str("llama")), ("general.file_type", W::U32(7))];
        write_gguf(&repo.join("Qwen3.6-35B-A3B-UD-Q4_K_M.gguf"), &kv(), &[]);
        write_gguf(&repo.join("mmproj-BF16.gguf"), &kv(), &[]);
        write_gguf(&repo.join("mmproj-F16.gguf"), &kv(), &[]);
        write_gguf(&repo.join("BF16").join("Qwen3.6-35B-A3B-BF16-00001-of-00002.gguf"), &kv(), &[]);
        write_gguf(&repo.join("BF16").join("Qwen3.6-35B-A3B-BF16-00002-of-00002.gguf"), &kv(), &[]);
        std::fs::write(repo.join("partial.gguf.part"), b"xx").unwrap();
        write_gguf(&root.join("flat-model.gguf"), &kv(), &[]);

        let models = scan_dir(&root, true);
        assert_eq!(models.len(), 3, "{models:#?}");
        let split = models.iter().find(|m| m.quant.as_deref() == Some("BF16")).unwrap();
        assert_eq!(split.files.len(), 2);
        assert_eq!(split.id, "unsloth/Qwen3.6-35B-A3B-GGUF/BF16/Qwen3.6-35B-A3B-BF16-00001-of-00002.gguf");
        assert_eq!(
            split.size_bytes,
            split.files.iter().map(|f| std::fs::metadata(f).unwrap().len()).sum::<u64>()
        );
        assert!(split.mmproj_path.as_deref().unwrap().ends_with("mmproj-F16.gguf"));
        let q4 = models.iter().find(|m| m.quant.as_deref() == Some("UD-Q4_K_M")).unwrap();
        assert_eq!(q4.repo_id.as_deref(), Some("unsloth/Qwen3.6-35B-A3B-GGUF"));
        assert_eq!(q4.org.as_deref(), Some("unsloth"));
        assert!(q4.source_known);
        assert_eq!(q4.name, "Qwen3.6-35B-A3B-UD-Q4_K_M");
        assert!(q4.mmproj_path.as_deref().unwrap().ends_with("mmproj-F16.gguf"));
        let flat = models.iter().find(|m| m.id == "flat-model.gguf").unwrap();
        assert!(!flat.source_known);
        assert_eq!(flat.repo_id, None);
        assert_eq!(flat.quant.as_deref(), Some("Q8_0")); // metadata fallback
        assert_eq!(flat.meta.as_ref().unwrap().architecture, "llama");
        std::fs::remove_dir_all(root).ok();
    }
}
