use crate::tools::ToolError;
use tree_sitter::Node;

use super::{call_args, callee, children, html_entries, join, line_of, nodes, parent, Files, Source};

pub(super) fn collect(files: &Files<'_, '_>, out: &mut Vec<String>) -> Result<(), ToolError> {
    let mut stmt = files.ctx.conn.prepare(
        "SELECT file_path FROM file_meta WHERE status = 'indexed' AND is_vendor = 0
         AND (file_path GLOB 'electron.vite.config.*' OR file_path GLOB '*/electron.vite.config.*') ORDER BY file_path",
    )?;
    let paths = stmt.query_map([], |r| r.get::<_, String>(0))?;
    for config in paths {
        let config = config?;
        render(files, &config, out);
    }
    Ok(())
}

fn render(files: &Files<'_, '_>, config: &str, out: &mut Vec<String>) {
    let dir = parent(config);
    let Some(manifest) = join(dir, "package.json") else { return };
    let Some(manifest_source) = files.live(&manifest) else { return };
    let Ok(package) = serde_json::from_str::<serde_json::Value>(&manifest_source) else { return };
    if !["dependencies", "devDependencies"].iter().any(|group| package[group]["electron"].is_string()) {
        return;
    }
    let Some(source) = files.indexed(config) else { return };
    let Some((object, resolver)) = config_object(&source) else { return };
    let roles = ["main", "preload", "renderer"].map(|role| property(&source, object, role));
    let Some(main) = roles[0] else { return };
    let mains = inputs(&source, main, resolver.as_deref());
    if mains.is_empty() {
        return;
    }
    let mut components = Vec::new();
    let mut extra = Vec::new();
    for (i, (name, path, line)) in mains.into_iter().enumerate() {
        let Some(file) = join(dir, &path) else { continue };
        let Some(src) = files.indexed(&file) else { continue };
        let ready = electron_call(&src, "app", "whenReady").map(line_of);
        let boot = ready.map(|line| format!(" | {file}:{line} app.whenReady()")).unwrap_or_default();
        let window = window_preload(&src).map(|(line, name)| format!(" | preload :{line} {name}")).unwrap_or_default();
        let row = format!("Electron main ({name}) {file} via {config}:{line}{boot}{window}");
        if i == 0 { &mut components } else { &mut extra }.push(row);
    }
    let mut bridges = Vec::new();
    if let Some(preload) = roles[1] {
        for (i, (name, path, line)) in inputs(&source, preload, resolver.as_deref()).into_iter().enumerate() {
            let Some(file) = join(dir, &path) else { continue };
            let Some(src) = files.indexed(&file) else { continue };
            let bridge = electron_call(&src, "contextBridge", "exposeInMainWorld")
                .and_then(|call| {
                    let name = src.literal(*call_args(call).first()?)?;
                    if !name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$') || name.is_empty() {
                        return None;
                    }
                    bridges.push(name.clone());
                    Some(format!(" | {file}:{} contextBridge.exposeInMainWorld('{name}', ...)", line_of(call)))
                })
                .unwrap_or_default();
            let ipc = electron_call(&src, "ipcRenderer", "invoke")
                .map(|n| format!(", ipcRenderer.invoke :{}", line_of(n)))
                .filter(|_| !bridge.is_empty())
                .unwrap_or_default();
            let row = format!("Electron preload ({name}) {file} via {config}:{line}{bridge}{ipc}");
            if i == 0 { &mut components } else { &mut extra }.push(row);
        }
    }
    if let Some(renderer) = roles[2] {
        let web_root = match property(&source, renderer, "root") {
            Some(root) => path_value(&source, root, resolver.as_deref()).and_then(|p| join(dir, &p)),
            None => Some(dir.to_owned()).filter(|_| children(renderer).all(|child| keyed(&source, child).is_some())),
        };
        for (i, (name, path, config_line)) in inputs(&source, renderer, resolver.as_deref()).into_iter().enumerate() {
            let Some(html) = join(dir, &path).filter(|p| p.ends_with(".html")) else { continue };
            for (file, line) in html_entries(files, &html, web_root.as_deref()) {
                let Some(src) = files.indexed(&file) else { continue };
                let access = nodes(src.root()).find(|&n| {
                    n.kind() == "member_expression"
                        && n.child_by_field_name("object").is_some_and(|o| src.text(o) == "window" && unshadowed(&src, o, "window"))
                        && n.child_by_field_name("property").is_some_and(|p| bridges.iter().any(|b| b == src.text(p)))
                });
                let access = access.map(|n| format!(" | {file}:{} {}", line_of(n), src.text(n))).unwrap_or_default();
                let row = format!("Electron renderer ({name}) {html}:{line} -> {file} via {config}:{config_line}{access}");
                if i == 0 { &mut components } else { &mut extra }.push(row);
            }
        }
    }
    if components.is_empty() && extra.is_empty() {
        return;
    }
    out.extend(components);
    if let Some(main) = package["main"].as_str().filter(|p| join(dir, p).is_some()) {
        let location = super::json_property_line(&manifest_source, "main").map(|n| format!(":{n}")).unwrap_or_default();
        out.push(format!("Electron packaged main {main} | {manifest}{location}"));
    }
    out.extend(extra);
}

fn electron_call<'a>(source: &'a Source, export: &str, method: &str) -> Option<Node<'a>> {
    let imports = import_names(source, "electron", export);
    nodes(source.root()).find(|&node| {
        let Some(member) = callee(node) else { return false };
        let Some(object) = member.child_by_field_name("object") else { return false };
        object.kind() == "identifier"
            && member.child_by_field_name("property").is_some_and(|p| source.text(p) == method)
            && imports.iter().any(|name| source.text(object) == name && unshadowed(source, object, name))
    })
}

fn unshadowed(source: &Source, use_site: Node<'_>, name: &str) -> bool {
    let mut upper = use_site.parent();
    while let Some(scope) = upper {
        let binding = match scope.kind() {
            "function_declaration" | "function_expression" | "arrow_function" | "method_definition" => {
                scope.child_by_field_name("parameters").or_else(|| scope.child_by_field_name("parameter"))
            }
            "catch_clause" => scope.child_by_field_name("parameter"),
            _ => None,
        };
        if binding.is_some_and(|p| nodes(p).any(|n| n.kind() == "identifier" && source.text(n) == name)) {
            return false;
        }
        if matches!(scope.kind(), "statement_block" | "program") && children(scope).any(|child| shadows(source, child, name)) {
            return false;
        }
        upper = scope.parent();
    }
    true
}

fn shadows(source: &Source, node: Node<'_>, name: &str) -> bool {
    match node.kind() {
        "lexical_declaration" | "variable_declaration" => children(node).any(|child| {
            child.child_by_field_name("name").is_some_and(|binding| {
                nodes(binding)
                    .any(|part| matches!(part.kind(), "identifier" | "shorthand_property_identifier_pattern") && source.text(part) == name)
            })
        }),
        "class_declaration" | "function_declaration" => node.child_by_field_name("name").is_some_and(|n| source.text(n) == name),
        _ => false,
    }
}

fn window_preload(source: &Source) -> Option<(u32, String)> {
    let imports = import_names(source, "electron", "BrowserWindow");
    let joins: Vec<_> = ["node:path", "path"].into_iter().flat_map(|module| import_names(source, module, "join")).collect();
    let namespaces: Vec<_> = ["node:path", "path"].into_iter().flat_map(|module| import_objects(source, module)).collect();
    nodes(source.root()).find_map(|node| {
        if node.kind() != "new_expression" {
            return None;
        }
        let constructor = node.child_by_field_name("constructor")?;
        if !imports.iter().any(|name| source.text(constructor) == name && unshadowed(source, constructor, name)) {
            return None;
        }
        let args = call_args(node);
        let [config] = args.as_slice() else { return None };
        let preload = property(source, property(source, *config, "webPreferences")?, "preload")?;
        let path = source.literal(preload).or_else(|| {
            let callee = callee(preload)?;
            let args = call_args(preload);
            let [base, relative] = args.as_slice() else { return None };
            let bound_join =
                callee.kind() == "identifier" && joins.iter().any(|name| source.text(callee) == name && unshadowed(source, callee, name));
            let member_join = callee.child_by_field_name("object").is_some_and(|object| {
                callee.child_by_field_name("property").is_some_and(|p| source.text(p) == "join")
                    && namespaces.iter().any(|name| source.text(object) == name && unshadowed(source, object, name))
            });
            ((bound_join || member_join) && source.text(*base) == "__dirname" && unshadowed(source, *base, "__dirname"))
                .then(|| source.literal(*relative))
                .flatten()
        })?;
        Some((line_of(preload), codegraph_core::extract::truncate(&path, 100)))
    })
}

fn config_object(source: &Source) -> Option<(Node<'_>, Option<String>)> {
    let mut config_names = import_names(source, "electron-vite", "defineConfig");
    let mut path_names = import_names(source, "node:path", "resolve");
    path_names.extend(import_names(source, "path", "resolve"));
    config_names.sort();
    config_names.dedup();
    path_names.sort();
    path_names.dedup();
    let resolver = (path_names.len() == 1).then(|| path_names.remove(0));
    let exported = children(source.root()).find(|&n| n.kind() == "export_statement" && n.child_by_field_name("value").is_some())?;
    let value = exported.child_by_field_name("value")?;
    if value.kind() == "object" {
        return Some((value, resolver));
    }
    let name = source.text(callee(value)?);
    if !config_names.iter().any(|n| n == name) || !unshadowed(source, value, name) {
        return None;
    }
    let args = call_args(value);
    let [object] = args.as_slice() else { return None };
    (object.kind() == "object").then_some((*object, resolver))
}

fn import_objects(source: &Source, module: &str) -> Vec<String> {
    children(source.root())
        .filter(|n| {
            n.kind() == "import_statement" && n.child_by_field_name("source").and_then(|n| source.literal(n)).as_deref() == Some(module)
        })
        .filter_map(|n| children(n).find(|n| n.kind() == "import_clause"))
        .flat_map(children)
        .filter_map(|n| match n.kind() {
            "identifier" => Some(n),
            "namespace_import" => children(n).find(|child| child.kind() == "identifier"),
            _ => None,
        })
        .map(|n| source.text(n).to_owned())
        .collect()
}

fn import_names(source: &Source, module: &str, export: &str) -> Vec<String> {
    children(source.root())
        .filter(|n| {
            n.kind() == "import_statement" && n.child_by_field_name("source").and_then(|n| source.literal(n)).as_deref() == Some(module)
        })
        .flat_map(nodes)
        .filter_map(|n| {
            if n.kind() != "import_specifier" || source.text(n.child_by_field_name("name")?) != export {
                return None;
            }
            Some(source.text(n.child_by_field_name("alias").or_else(|| n.child_by_field_name("name"))?).to_owned())
        })
        .collect()
}

fn keyed<'a, 's>(source: &'s Source, pair: Node<'a>) -> Option<(&'s str, Node<'a>)> {
    if pair.kind() != "pair" {
        return None;
    }
    let key = pair.child_by_field_name("key")?;
    if !matches!(key.kind(), "property_identifier" | "string") || source.text(key).contains('\\') {
        return None;
    }
    Some((source.text(key).trim_matches(['\'', '"']), pair.child_by_field_name("value")?))
}

fn property<'a>(source: &Source, object: Node<'a>, name: &str) -> Option<Node<'a>> {
    if object.kind() != "object" {
        return None;
    }
    for child in children(object).rev() {
        match keyed(source, child) {
            Some((key, value)) if key == name => return Some(value),
            Some(_) => {}
            None => return None, // A spread, method or computed key can override the field.
        }
    }
    None
}

fn path_value(source: &Source, node: Node<'_>, resolver: Option<&str>) -> Option<String> {
    if let Some(path) = source.literal(node) {
        return Some(path);
    }
    let callee = callee(node)?;
    if Some(source.text(callee)) != resolver || !unshadowed(source, callee, source.text(callee)) {
        return None;
    }
    let args = call_args(node);
    let [root, path] = args.as_slice() else { return None };
    (source.text(*root) == "__dirname" && unshadowed(source, *root, "__dirname")).then(|| source.literal(*path)).flatten()
}

fn inputs(source: &Source, role: Node<'_>, resolver: Option<&str>) -> Vec<(String, String, u32)> {
    let Some(input) =
        property(source, role, "build").and_then(|n| property(source, n, "rollupOptions")).and_then(|n| property(source, n, "input"))
    else {
        return Vec::new();
    };
    if let Some(path) = path_value(source, input, resolver) {
        return vec![("input".to_owned(), path, line_of(input))];
    }
    if input.kind() != "object" {
        return Vec::new();
    }
    let mut names = std::collections::BTreeSet::new();
    let mut inputs = Vec::new();
    for child in children(input).rev() {
        let Some((name, value)) = keyed(source, child) else { break };
        if names.insert(name) {
            if let Some(path) = path_value(source, value, resolver) {
                inputs.push((name.to_owned(), path, line_of(value)));
            }
        }
    }
    inputs.reverse();
    // Reserve space for the explicitly named index input before auxiliary inputs.
    inputs.sort_by_key(|(name, _, _)| name != "index");
    inputs
}
