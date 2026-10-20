//! Unity: engine-invoked methods confirmed through real inheritance, and editor-only code.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use codegraph_core::extract::clean_type;
use codegraph_core::unity::{lineage, messages_of};

use crate::index::SymbolIndex;

/// Names of every ancestor of a project type: project base classes (all partial parts), their
/// declared external bases (`UnityEngine.MonoBehaviour` → `MonoBehaviour`) and Unity's own
/// lineage of those (`Button` → `MonoBehaviour`).
pub fn ancestor_names(index: &SymbolIndex, t: usize) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack = vec![t];
    let mut seen = HashSet::new();
    while let Some(x) = stack.pop() {
        if !seen.insert(x) || seen.len() > 64 {
            continue;
        }
        for p in index.parts_of(x) {
            let s = &index.syms[p].sym;
            let resolved = index.bases.get(&s.id).cloned().unwrap_or_default();
            for b in &s.bases {
                let name = clean_type(b);
                let short = name.rsplit('.').next().unwrap_or(&name).to_string();
                for l in lineage(&short) {
                    out.insert(l.to_string());
                }
                out.insert(short);
            }
            for (b, _) in resolved {
                if let Some(bi) = index.by_id.get(&b) {
                    stack.push(*bi);
                }
            }
        }
    }
    out
}

/// Methods flagged `unity-msg` at extraction that really are engine messages of one of their
/// type's ancestors → (symbol id, `unity message Base.Name`).
pub fn entrypoints(index: &SymbolIndex) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for s in &index.syms {
        if !s.sym.modifiers.iter().any(|m| m == "unity-msg") {
            continue;
        }
        let Some(owner) = s.sym.parent_id.as_deref().and_then(|p| index.by_id.get(p)).copied() else { continue };
        let names = ancestor_names(index, owner);
        if let Some(base) = names.iter().find(|b| messages_of(b).contains(&s.sym.name.as_str())) {
            out.push((s.sym.id.clone(), format!("unity message {base}.{}", s.sym.name)));
        }
    }
    out.sort();
    out
}

/// Does the type (transitively) derive from a Unity type whose instances live in scenes or
/// assets (MonoBehaviour, ScriptableObject, …)?
pub fn is_serialized_type(index: &SymbolIndex, t: usize) -> bool {
    let names = ancestor_names(index, t);
    codegraph_core::unity::SERIALIZED_ROOTS.iter().any(|r| names.contains(*r))
}

/// Directories of assembly definitions restricted to the Editor platform.
pub fn load_editor_dirs(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    collect(root, root, 0, &mut out);
    out.sort();
    out
}

fn collect(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > 12 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            if !name.starts_with('.')
                && !codegraph_core::walk::is_skipped_dir(&name)
                && !matches!(name.as_str(), "Library" | "Temp" | "Logs")
            {
                collect(root, &e.path(), depth + 1, out);
            }
        } else if name.ends_with(".asmdef") {
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) else { continue };
            let editor_only = v
                .get("includePlatforms")
                .and_then(|p| p.as_array())
                .map(|a| !a.is_empty() && a.iter().all(|x| x.as_str() == Some("Editor")))
                .unwrap_or(false);
            if editor_only {
                let rel = dir.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
                out.push(rel);
            }
        }
    }
}
