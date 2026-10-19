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

/// Largest content-addressed row id (2^53 - 1: stays exact in JSON / JavaScript numbers).
pub const MAX_ROWID: i64 = (1 << 53) - 1;

/// Content-addressed SQLite row id in `1..=MAX_ROWID`. Rows keyed this way are stored (and
/// scanned, and tie-broken) in the same order however and in whatever order they were inserted.
pub fn stable_rowid(parts: &[&[u8]]) -> i64 {
    let mut h = blake3::Hasher::new();
    for p in parts {
        h.update(&(p.len() as u64).to_le_bytes());
        h.update(p);
    }
    let b = h.finalize();
    let v = i64::from_le_bytes(b.as_bytes()[..8].try_into().unwrap()) & MAX_ROWID;
    v.max(1)
}

/// Next probe slot after a (vanishingly rare) row id collision.
pub fn next_rowid(id: i64) -> i64 {
    if id >= MAX_ROWID {
        1
    } else {
        id + 1
    }
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
