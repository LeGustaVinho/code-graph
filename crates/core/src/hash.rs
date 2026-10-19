//! Blake3 hashing helpers (content hashes, stable symbol ids).

use rayon::prelude::*;
use std::path::{Path, PathBuf};

pub fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Stable symbol id derived from file, kind, qualified name and an overload discriminator.
pub fn symbol_id(file: &str, kind: &str, qualified_name: &str, discriminator: u32) -> String {
    let mut h = blake3::Hasher::new();
    h.update(file.as_bytes());
    h.update(&[0]);
    h.update(kind.as_bytes());
    h.update(&[0]);
    h.update(qualified_name.as_bytes());
    h.update(&[0]);
    h.update(&discriminator.to_le_bytes());
    let hex = h.finalize().to_hex();
    format!("sym_{}", &hex[..16])
}

pub fn short_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex()[..16].to_string()
}

/// Hashes many files in parallel (Rayon work-stealing, Blake3 SIMD per file).
pub fn hash_files_parallel(paths: &[PathBuf]) -> Vec<(PathBuf, std::io::Result<(String, Vec<u8>)>)> {
    paths
        .par_iter()
        .map(|p| {
            let res = std::fs::read(p).map(|bytes| (content_hash(&bytes), bytes));
            (p.clone(), res)
        })
        .collect()
}

pub fn hash_file(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(content_hash(&bytes))
}
