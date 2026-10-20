//! Containment hierarchy for the Graph Explorer: namespace (or folder) → file →
//! type → member, plus roll-up of relations to whatever level is currently visible.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::Connection;
use serde::Serialize;
use serde_json::{json, Value};

/// One element of the hierarchy (a synthetic container or an indexed symbol).
#[derive(Clone, Debug, Serialize)]
pub struct TreeItem {
    pub id: String,
    pub parent: Option<String>,
    pub kind: String,
    pub label: String,
    pub title: String,
    pub file: String,
    pub start: u32,
    pub end: u32,
    pub coords: String,
    pub is_entrypoint: bool,
    pub signature: String,
    /// Third-party code (a folder / namespace is vendor when everything in it is).
    pub vendor: bool,
}

pub struct Tree {
    pub items: HashMap<String, TreeItem>,
    pub children: HashMap<String, Vec<String>>,
    pub roots: Vec<String>,
}

/// Sort order inside a container: folders/namespaces, files, types, members.
fn rank(kind: &str) -> u8 {
    match kind {
        "namespace" | "directory" => 0,
        "module" => 1,
        "class" | "interface" | "struct" | "enum" | "type_alias" | "data_model" => 2,
        "test" => 4,
        _ => 3,
    }
}

impl Tree {
    pub fn load(conn: &Connection) -> rusqlite::Result<Tree> {
        let mut items: HashMap<String, TreeItem> = HashMap::new();
        let mut file_ns: HashMap<String, BTreeMap<String, usize>> = HashMap::new();
        let mut module_of_file: HashMap<String, String> = HashMap::new();
        let vendor_files: HashSet<String> = {
            let mut st = conn.prepare("SELECT file_path FROM file_meta WHERE is_vendor = 1")?;
            let rows: HashSet<String> = st.query_map([], |r| r.get::<_, String>(0))?.flatten().collect();
            rows
        };
        {
            let mut st = conn.prepare(
                "SELECT id, parent_id, kind, name, qualified_name, file_path, namespace, start_line, end_line, is_entrypoint, signature \
                 FROM node WHERE derived = 0",
            )?;
            let rows = st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, u32>(7)?,
                    r.get::<_, u32>(8)?,
                    r.get::<_, bool>(9)?,
                    r.get::<_, Option<String>>(10)?.unwrap_or_default(),
                ))
            })?;
            for (id, parent, kind, name, qname, file, ns, start, end, entry, sig) in rows.flatten() {
                if let Some(ns) = ns.filter(|n| !n.is_empty()) {
                    *file_ns.entry(file.clone()).or_default().entry(ns).or_default() += 1;
                }
                let label = if kind == "module" { file.rsplit('/').next().unwrap_or(&file).to_string() } else { name.clone() };
                if kind == "module" {
                    module_of_file.insert(file.clone(), id.clone());
                }
                let coords = if end > start { format!("{file}:{start}-{end}") } else { format!("{file}:{start}") };
                let vendor = vendor_files.contains(&file);
                items.insert(
                    id.clone(),
                    TreeItem {
                        id,
                        parent,
                        kind,
                        label,
                        title: if qname.is_empty() { name } else { qname },
                        file,
                        start,
                        end,
                        coords,
                        is_entrypoint: entry,
                        signature: sig,
                        vendor,
                    },
                );
            }
        }

        // Containers: the dominant namespace of a file (C#) or its directory (other languages).
        let mut containers: HashMap<String, TreeItem> = HashMap::new();
        let ensure = |key: &str, containers: &mut HashMap<String, TreeItem>| -> String {
            let (kind, path, sep) = match key.split_once(':') {
                Some(("ns", p)) => ("namespace", p, '.'),
                Some((_, p)) => ("directory", p, '/'),
                None => ("directory", key, '/'),
            };
            let prefix = if kind == "namespace" { "ns:" } else { "dir:" };
            let segs: Vec<&str> = path.split(sep).filter(|s| !s.is_empty()).collect();
            let mut parent: Option<String> = None;
            let mut acc = String::new();
            for seg in segs {
                if !acc.is_empty() {
                    acc.push(sep);
                }
                acc.push_str(seg);
                let id = format!("{prefix}{acc}");
                containers.entry(id.clone()).or_insert_with(|| TreeItem {
                    id: id.clone(),
                    parent: parent.clone(),
                    kind: kind.into(),
                    label: seg.to_string(),
                    title: acc.clone(),
                    file: String::new(),
                    start: 0,
                    end: 0,
                    coords: String::new(),
                    is_entrypoint: false,
                    signature: String::new(),
                    vendor: false,
                });
                parent = Some(id);
            }
            parent.unwrap_or_else(|| prefix.to_string())
        };
        for (file, module_id) in &module_of_file {
            let key = match file_ns.get(file).and_then(|m| m.iter().max_by_key(|(_, c)| **c).map(|(n, _)| n.clone())) {
                Some(ns) => format!("ns:{ns}"),
                None => {
                    let dir = file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                    if dir.is_empty() {
                        "dir:(root)".to_string()
                    } else {
                        format!("dir:{dir}")
                    }
                }
            };
            let cid = ensure(&key, &mut containers);
            if let Some(m) = items.get_mut(module_id) {
                m.parent = Some(cid);
            }
        }
        items.extend(containers);

        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        let mut roots = Vec::new();
        for it in items.values() {
            match &it.parent {
                Some(p) if items.contains_key(p) => children.entry(p.clone()).or_default().push(it.id.clone()),
                _ => roots.push(it.id.clone()),
            }
        }
        // Roots carry no parent (a dangling `parent_id` would confuse clients).
        for r in &roots {
            if let Some(it) = items.get_mut(r) {
                it.parent = None;
            }
        }
        let mut tree = Tree { items, children, roots };
        tree.compact();
        tree.mark_vendor_containers();
        let sort_key = |t: &Tree, id: &String| {
            let i = &t.items[id];
            (rank(&i.kind), i.start, i.label.to_ascii_lowercase())
        };
        let mut sorted: HashMap<String, Vec<String>> = HashMap::new();
        for (p, kids) in &tree.children {
            let mut k = kids.clone();
            k.sort_by_key(|id| sort_key(&tree, id));
            sorted.insert(p.clone(), k);
        }
        tree.children = sorted;
        let mut roots = tree.roots.clone();
        roots.sort_by_key(|id| sort_key(&tree, id));
        tree.roots = roots;
        Ok(tree)
    }

    /// Merges container chains with a single sub-container and no files (`Assets` → `Assets/Game/App`).
    fn compact(&mut self) {
        loop {
            let candidate = self.items.values().find_map(|it| {
                if !matches!(it.kind.as_str(), "namespace" | "directory") {
                    return None;
                }
                let kids = self.children.get(&it.id)?;
                if kids.len() != 1 {
                    return None;
                }
                let only = &self.items[&kids[0]];
                (only.kind == it.kind).then(|| (it.id.clone(), only.id.clone()))
            });
            let Some((outer, inner)) = candidate else { break };
            let sep = if self.items[&outer].kind == "namespace" { "." } else { "/" };
            let outer_item = self.items.remove(&outer).unwrap();
            let inner_item = self.items.get_mut(&inner).unwrap();
            inner_item.label = format!("{}{sep}{}", outer_item.label, inner_item.label);
            inner_item.parent = outer_item.parent.clone();
            self.children.remove(&outer);
            match &outer_item.parent {
                Some(p) => {
                    if let Some(kids) = self.children.get_mut(p) {
                        for k in kids.iter_mut() {
                            if *k == outer {
                                *k = inner.clone();
                            }
                        }
                    }
                }
                None => {
                    for r in self.roots.iter_mut() {
                        if *r == outer {
                            *r = inner.clone();
                        }
                    }
                }
            }
        }
    }

    pub fn parent(&self, id: &str) -> Option<&str> {
        self.items.get(id).and_then(|i| i.parent.as_deref()).filter(|p| self.items.contains_key(*p))
    }

    /// Ancestors from the nearest container up to the root.
    pub fn ancestors(&self, id: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = self.parent(id);
        while let Some(p) = cur {
            if out.len() > 64 {
                break;
            }
            out.push(p.to_string());
            cur = self.parent(p);
        }
        out
    }

    pub fn item_json(&self, id: &str) -> Option<Value> {
        let it = self.items.get(id)?;
        let count = self.children.get(id).map(|c| c.len()).unwrap_or(0);
        Some(json!({
            "id": it.id,
            "parent": it.parent,
            "kind": it.kind,
            "label": it.label,
            "title": it.title,
            "file": it.file,
            "start": it.start,
            "end": it.end,
            "coords": it.coords,
            "is_entrypoint": it.is_entrypoint,
            "signature": it.signature,
            "child_count": count,
            "vendor": it.vendor,
        }))
    }

    /// Children of `id` (roots when empty); third-party items only with `include_vendor`.
    pub fn children_json(&self, id: Option<&str>, include_vendor: bool) -> Vec<Value> {
        let ids = match id {
            None | Some("") => &self.roots,
            Some(p) => match self.children.get(p) {
                Some(c) => c,
                None => return Vec::new(),
            },
        };
        ids.iter()
            .filter(|c| include_vendor || !self.items.get(*c).map(|i| i.vendor).unwrap_or(false))
            .filter_map(|c| self.item_json(c))
            .collect()
    }

    /// Containers are vendor when every item inside them is.
    fn mark_vendor_containers(&mut self) {
        fn visit(t: &mut Tree, id: &str, guard: u32) -> bool {
            let kids = t.children.get(id).cloned().unwrap_or_default();
            let kind = t.items.get(id).map(|i| i.kind.clone()).unwrap_or_default();
            if guard > 64 || !matches!(kind.as_str(), "namespace" | "directory") {
                return t.items.get(id).map(|i| i.vendor).unwrap_or(false);
            }
            let mut all = !kids.is_empty();
            for k in &kids {
                all &= visit(t, k, guard + 1);
            }
            if let Some(i) = t.items.get_mut(id) {
                i.vendor = all;
            }
            all
        }
        for r in self.roots.clone() {
            visit(self, &r, 0);
        }
    }

    /// Aggregates relations onto the visible items (each endpoint climbs to its nearest
    /// visible ancestor). Internal relations and parent ↔ child links are dropped.
    pub fn rollup(
        &self,
        conn: &Connection,
        visible: &HashSet<String>,
        relations: &[String],
        confidences: &[String],
    ) -> rusqlite::Result<Vec<Value>> {
        let mut memo: HashMap<String, Option<String>> = HashMap::new();
        let map = |id: &str, memo: &mut HashMap<String, Option<String>>| -> Option<String> {
            if let Some(m) = memo.get(id) {
                return m.clone();
            }
            let mut cur = Some(id.to_string());
            let mut found = None;
            let mut guard = 0;
            while let Some(c) = cur {
                if visible.contains(&c) {
                    found = Some(c);
                    break;
                }
                cur = self.parent(&c).map(|s| s.to_string());
                guard += 1;
                if guard > 64 {
                    break;
                }
            }
            memo.insert(id.to_string(), found.clone());
            found
        };
        let rel_ph = relations.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let conf_ph = confidences.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        if relations.is_empty() || confidences.is_empty() {
            return Ok(Vec::new());
        }
        let sql = format!(
            "SELECT source_id, target_id, relation, confidence FROM edge WHERE target_id IS NOT NULL AND relation IN ({rel_ph}) AND confidence IN ({conf_ph})"
        );
        let mut st = conn.prepare(&sql)?;
        let args: Vec<&String> = relations.iter().chain(confidences.iter()).collect();
        #[derive(Default)]
        struct Agg {
            count: usize,
            relations: BTreeMap<String, usize>,
            confidences: BTreeMap<String, usize>,
        }
        let mut agg: BTreeMap<(String, String), Agg> = BTreeMap::new();
        let rows = st.query_map(rusqlite::params_from_iter(args), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })?;
        for (s, t, rel, conf) in rows.flatten() {
            let (Some(a), Some(b)) = (map(&s, &mut memo), map(&t, &mut memo)) else { continue };
            if a == b {
                continue;
            }
            // Edges between an item and its own container are containment, not dependency.
            if self.ancestors(&a).contains(&b) || self.ancestors(&b).contains(&a) {
                continue;
            }
            let e = agg.entry((a, b)).or_default();
            e.count += 1;
            *e.relations.entry(rel).or_default() += 1;
            *e.confidences.entry(conf).or_default() += 1;
        }
        Ok(agg
            .into_iter()
            .enumerate()
            .map(|(i, ((s, t), a))| {
                // The strongest evidence present styles the aggregated edge.
                let conf = ["exact", "inferred", "unresolved"].into_iter().find(|c| a.confidences.contains_key(*c)).unwrap_or("exact");
                let main = a.relations.iter().max_by_key(|(_, c)| **c).map(|(r, _)| r.clone()).unwrap_or_default();
                json!({
                    "id": format!("r{i}"),
                    "source": s,
                    "target": t,
                    "count": a.count,
                    "relation": main,
                    "confidence": conf,
                    "relations": a.relations,
                    "confidences": a.confidences,
                    "rollup": true,
                })
            })
            .collect())
    }
}

/// Cached tree, rebuilt when the index changes.
type Cached = Option<(String, Arc<Tree>)>;
static CACHE: OnceLock<Mutex<Cached>> = OnceLock::new();

pub fn get(conn: &Connection) -> rusqlite::Result<Arc<Tree>> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM node", [], |r| r.get(0))?;
    // Keyed by database too: the explorer can switch projects at runtime.
    let stamp = format!(
        "{}#{}#{count}",
        conn.path().unwrap_or_default(),
        codegraph_graph::store::get_meta(conn, "last_indexed_at").unwrap_or_default()
    );
    let cell = CACHE.get_or_init(|| Mutex::new(None));
    if let Some((s, t)) = cell.lock().unwrap().as_ref() {
        if *s == stamp {
            return Ok(t.clone());
        }
    }
    let tree = Arc::new(Tree::load(conn)?);
    *cell.lock().unwrap() = Some((stamp, tree.clone()));
    Ok(tree)
}
