//! Phaser: engine hooks confirmed through real inheritance, scenes by key (`this.scene.start`,
//! `this.scene.get`) and events (`emit('score')` → `on('score', this.onScore)`).

use std::collections::{BTreeSet, HashMap, HashSet};

use codegraph_core::extract::clean_type;
use codegraph_core::phaser::hooks_of;

use crate::index::SymbolIndex;
use crate::{file_name, relation, Confidence, Edge};

/// Phaser classes a project type derives from (short names: `Scene`, `Sprite`, …), through
/// project base classes. An external base counts when written `Phaser.X` or imported from `phaser`.
pub fn phaser_ancestors(index: &SymbolIndex, t: usize) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack = vec![t];
    let mut seen = HashSet::new();
    while let Some(x) = stack.pop() {
        if !seen.insert(x) || seen.len() > 64 {
            continue;
        }
        for p in index.parts_of(x) {
            let s = &index.syms[p];
            let resolved = index.bases.get(&s.sym.id).cloned().unwrap_or_default();
            for b in &s.sym.bases {
                let name = clean_type(b);
                let short = name.rsplit('.').next().unwrap_or(&name).to_string();
                // `Scene` / `Physics.Arcade.Sprite` with `import { Scene, Physics } from 'phaser'`.
                let head = name.split('.').next().unwrap_or(&name);
                let imported = index
                    .imports
                    .get(&s.file)
                    .map(|imps| imps.iter().any(|i| i.module == "phaser" && i.local.as_deref() == Some(head)))
                    .unwrap_or(false);
                if name.starts_with("Phaser.") || imported {
                    out.insert(short);
                }
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

/// Methods flagged `phaser-hook` at extraction that Phaser really calls (→ `phaser hook
/// Scene.create`), and the project's scenes (→ `phaser scene`).
pub fn entrypoints(index: &SymbolIndex) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cache: HashMap<usize, BTreeSet<String>> = HashMap::new();
    for s in &index.syms {
        let hook = s.sym.modifiers.iter().any(|m| m == "phaser-hook");
        let class = s.sym.kind == codegraph_core::SymbolKind::Class && !s.sym.bases.is_empty();
        if !hook && !class {
            continue;
        }
        let owner = if hook { s.sym.parent_id.as_deref().and_then(|p| index.by_id.get(p)).copied() } else { index.by_id.get(&s.sym.id).copied() };
        let Some(owner) = owner else { continue };
        let bases = cache.entry(owner).or_insert_with(|| phaser_ancestors(index, owner));
        if hook {
            if let Some(base) = bases.iter().find(|b| hooks_of(b).contains(&s.sym.name.as_str())) {
                out.push((s.sym.id.clone(), format!("phaser hook {base}.{}", s.sym.name)));
            }
        } else if bases.contains("Scene") {
            out.push((s.sym.id.clone(), "phaser scene".to_string()));
        }
    }
    out.sort();
    out
}

/// A `scene-key` / `scene-ref` / `event-emit` / `event-listen` literal.
#[derive(Clone, Debug)]
pub struct KeyRow {
    pub kind: String,
    pub value: String,
    pub file: String,
    pub enclosing: String,
    pub line: u32,
}

/// Scene key → scene class: `super('Game')` (enclosing = the class) and
/// `this.scene.add('Pause', PauseScene)` (value `Pause#PauseScene`).
pub fn scene_keys(index: &SymbolIndex, rows: &[KeyRow]) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    for r in rows.iter().filter(|r| r.kind == "scene-key") {
        match r.value.split_once('#') {
            Some((key, class)) => {
                let cands: Vec<usize> = index
                    .by_name
                    .get(class)
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|i| index.syms[*i].sym.kind == codegraph_core::SymbolKind::Class)
                    .collect();
                if let [c] = cands.as_slice() {
                    out.entry(key.to_string()).or_insert(*c);
                }
            }
            None => {
                if let Some(c) = index.by_id.get(&r.enclosing) {
                    out.entry(r.value.clone()).or_insert(*c);
                }
            }
        }
    }
    out
}

/// `this.scene.start('Game')` → the scene's first lifecycle hook (`init`, else `preload`, else
/// `create`), or the scene class.
pub fn scene_edges(index: &SymbolIndex, rows: &[KeyRow]) -> Vec<Edge> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for r in rows.iter().filter(|r| r.kind == "scene-ref") {
        let Some(&scene) = index.scene_keys.get(&r.value) else { continue };
        let target = ["init", "preload", "create"].iter().find_map(|h| index.members_named(scene, h).into_iter().next()).unwrap_or(scene);
        if !seen.insert((r.enclosing.clone(), target)) {
            continue;
        }
        let t = &index.syms[target];
        out.push(
            Edge::new(
                &r.enclosing,
                &r.file,
                Some((&t.sym.id, &t.file)),
                relation::CALLS,
                Confidence::Inferred,
                format!("scene '{}' started ({}:{})", r.value, file_name(&index.syms[scene].file), index.syms[scene].sym.start_line),
                "phaser-scene-linker",
                r.line,
            )
            .derived(),
        );
    }
    out
}

/// `emit('score')` → every handler registered with `on('score', …)` / `once`: the named method
/// / function (`score#onScore`), or the code that registered an inline handler.
pub fn event_edges(index: &SymbolIndex, rows: &[KeyRow]) -> Vec<Edge> {
    let mut listeners: HashMap<&str, Vec<&KeyRow>> = HashMap::new();
    for r in rows.iter().filter(|r| r.kind == "event-listen") {
        let key = r.value.split_once('#').map(|(k, _)| k).unwrap_or(&r.value);
        listeners.entry(key).or_default().push(r);
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for e in rows.iter().filter(|r| r.kind == "event-emit") {
        let Some(ls) = listeners.get(e.value.as_str()) else { continue };
        // A key every component listens to is a convention, not a specific flow.
        if ls.len() > 8 {
            continue;
        }
        for l in ls {
            let target = match l.value.split_once('#') {
                Some((_, handler)) => handler_symbol(index, l, handler),
                None => index.by_id.get(&l.enclosing).copied(),
            };
            let Some(t) = target else { continue };
            let ts = &index.syms[t];
            if ts.sym.id == e.enclosing || !seen.insert((e.enclosing.clone(), ts.sym.id.clone())) {
                continue;
            }
            out.push(
                Edge::new(
                    &e.enclosing,
                    &e.file,
                    Some((&ts.sym.id, &ts.file)),
                    relation::CALLS,
                    Confidence::Inferred,
                    format!("emit('{}') → on('{}') ({}:{})", e.value, e.value, file_name(&l.file), l.line),
                    "event-linker",
                    e.line,
                )
                .with_meta(serde_json::json!({ "event": e.value }))
                .derived(),
            );
        }
    }
    out
}

/// The handler a listener names: a member of the registering method's class, else a function
/// of the same file.
fn handler_symbol(index: &SymbolIndex, l: &KeyRow, handler: &str) -> Option<usize> {
    let reg = index.by_id.get(&l.enclosing).copied()?;
    let mut cur = Some(reg);
    while let Some(c) = cur {
        let s = &index.syms[c].sym;
        if s.kind.is_type() {
            if let Some(m) = index.members_named(c, handler).into_iter().next() {
                return Some(m);
            }
        }
        cur = s.parent_id.as_deref().and_then(|p| index.by_id.get(p)).copied();
    }
    index.by_file.get(&l.file)?.iter().copied().find(|i| index.syms[*i].sym.name == handler && index.syms[*i].sym.kind.is_callable())
}
