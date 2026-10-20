//! Unity: engine-invoked methods confirmed through real inheritance, and editor-only code.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use codegraph_core::extract::clean_type;
use codegraph_core::unity::{lineage, messages_of};
use codegraph_core::{Language, SymbolKind};

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

/// The Unity Test Framework's runner assemblies, by name and by GUID (asmdef references use
/// either form).
const TEST_RUNNER_REFS: &[&str] = &[
    "UnityEngine.TestRunner",
    "UnityEditor.TestRunner",
    "GUID:27619889b8ba8c24980f49ee34dbb44a",
    "GUID:0acc523941302664db1f4e527237feb3",
];

/// One assembly definition (`.asmdef`).
#[derive(Clone, Debug, Default)]
pub struct AsmDef {
    pub name: String,
    pub guid: Option<String>,
    /// Restricted to the Editor platform.
    pub editor_only: bool,
    /// A Unity Test Framework test assembly.
    pub test: bool,
}

/// The assembly definitions of a Unity project and the folders they own.
#[derive(Clone, Debug, Default)]
pub struct Assemblies {
    pub defs: Vec<AsmDef>,
    /// Project-relative folder holding an `.asmdef` / `.asmref` → its assembly (None: an
    /// `.asmref` naming an assembly outside the project).
    folders: HashMap<String, Option<usize>>,
}

impl Assemblies {
    /// The assembly compiling `file`: the nearest `.asmdef` / `.asmref` up the folder tree.
    /// None for Unity's predefined assemblies (Assembly-CSharp, Assembly-CSharp-Editor, …).
    pub fn owner(&self, file: &str) -> Option<&AsmDef> {
        let mut dir = crate::index::dirname(file);
        loop {
            if let Some(a) = self.folders.get(dir) {
                return a.map(|i| &self.defs[i]);
            }
            if dir.is_empty() {
                return None;
            }
            dir = crate::index::dirname(dir);
        }
    }

    /// Folders of assemblies restricted to the Editor platform.
    pub fn editor_dirs(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .folders
            .iter()
            .filter(|(_, a)| a.map(|i| self.defs[i].editor_only).unwrap_or(false))
            .map(|(d, _)| d.clone())
            .collect();
        out.sort();
        out
    }
}

/// Is the parsed `.asmdef` a test assembly (runner references, NUnit, or the test define)?
fn is_test_assembly(v: &serde_json::Value) -> bool {
    let any = |key: &str, pred: &dyn Fn(&str) -> bool| {
        v.get(key).and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).any(pred)).unwrap_or(false)
    };
    any("references", &|r| TEST_RUNNER_REFS.iter().any(|t| t.eq_ignore_ascii_case(r)))
        || any("precompiledReferences", &|r| r.eq_ignore_ascii_case("nunit.framework.dll"))
        || any("defineConstraints", &|d| d.trim() == "UNITY_INCLUDE_TESTS")
        // Pre-2019.2 format.
        || any("optionalUnityReferences", &|r| r == "TestAssemblies")
}

/// Reads every `.asmdef` / `.asmref` of the project.
pub fn load_assemblies(root: &Path) -> Assemblies {
    let mut defs: Vec<(String, AsmDef)> = Vec::new();
    let mut refs: Vec<(String, String)> = Vec::new();
    collect(root, root, 0, &mut defs, &mut refs);
    defs.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Assemblies::default();
    for (dir, def) in defs {
        out.folders.insert(dir, Some(out.defs.len()));
        out.defs.push(def);
    }
    for (dir, reference) in refs {
        let target = match reference.strip_prefix("GUID:") {
            Some(g) => out.defs.iter().position(|d| d.guid.as_deref() == Some(g)),
            None => out.defs.iter().position(|d| d.name == reference),
        };
        out.folders.entry(dir).or_insert(target);
    }
    out
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

fn collect(root: &Path, dir: &Path, depth: usize, defs: &mut Vec<(String, AsmDef)>, refs: &mut Vec<(String, String)>) {
    if depth > 12 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let rel = || dir.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            if !name.starts_with('.')
                && !codegraph_core::walk::is_skipped_dir(&name)
                && !matches!(name.as_str(), "Library" | "Temp" | "Logs")
            {
                collect(root, &e.path(), depth + 1, defs, refs);
            }
        } else if name.ends_with(".asmdef") {
            let Some(v) = read_json(&e.path()) else { continue };
            let editor_only = v
                .get("includePlatforms")
                .and_then(|p| p.as_array())
                .map(|a| !a.is_empty() && a.iter().all(|x| x.as_str() == Some("Editor")))
                .unwrap_or(false);
            let guid = std::fs::read_to_string(dir.join(format!("{name}.meta")))
                .ok()
                .and_then(|m| m.lines().find_map(|l| l.trim().strip_prefix("guid:").map(|g| g.trim().to_string())));
            let asm_name =
                v.get("name").and_then(|n| n.as_str()).map(String::from).unwrap_or_else(|| name.trim_end_matches(".asmdef").into());
            defs.push((rel(), AsmDef { name: asm_name, guid, editor_only, test: is_test_assembly(&v) }));
        } else if name.ends_with(".asmref") {
            if let Some(r) = read_json(&e.path()).and_then(|v| v.get("reference").and_then(|r| r.as_str()).map(String::from)) {
                refs.push((rel(), r));
            }
        }
    }
}

/// Unity test code, following the Test Framework: C# files compiled into a test assembly, and
/// files of other assemblies (Assembly-CSharp included) that declare NUnit tests. Folder and
/// file names do not count. Sets the module flags of `index` and returns the modules whose
/// flag changed.
pub fn apply_test_files(index: &mut SymbolIndex, asm: &Assemblies) -> Vec<(String, bool)> {
    let is_cs = |index: &SymbolIndex, f: &str| index.file_lang.get(f) == Some(&Language::CSharp);
    let mut tests = HashSet::new();
    for (file, members) in &index.by_file {
        if !is_cs(index, file) {
            continue;
        }
        let declares_tests = members.iter().any(|i| index.syms[*i].sym.is_test && index.syms[*i].sym.kind != SymbolKind::Module);
        if declares_tests || asm.owner(file).map(|a| a.test).unwrap_or(false) {
            tests.insert(file.clone());
        }
    }
    let changed: Vec<(usize, bool)> = index
        .module_of_file
        .iter()
        .filter(|(f, _)| is_cs(index, f))
        .map(|(f, m)| (*m, tests.contains(f)))
        .filter(|(m, t)| index.syms[*m].sym.is_test != *t)
        .collect();
    let mut out = Vec::new();
    for (m, t) in changed {
        index.syms[m].sym.is_test = t;
        out.push((index.syms[m].sym.id.clone(), t));
    }
    out.sort();
    index.unity_test_files = Some(tests);
    out
}
