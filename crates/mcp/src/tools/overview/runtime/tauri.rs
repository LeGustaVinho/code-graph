use codegraph_core::extract::truncate;
use tree_sitter::Node;

use super::{call_args, callee, children, html_entries, join, line_of, nodes, parent, Files, Source};
use crate::tools::ToolError;

pub(super) fn collect(files: &Files<'_, '_>, out: &mut Vec<String>) -> Result<(), ToolError> {
    let mut stmt = files.ctx.conn.prepare(
        "SELECT DISTINCT n.file_path FROM node n JOIN file_meta f ON f.file_path = n.file_path
         WHERE n.is_entrypoint = 1 AND n.name = 'main' AND n.language = 'rust' AND n.is_test = 0
           AND n.kind = 'function' AND f.status = 'indexed' AND f.is_vendor = 0 ORDER BY n.file_path",
    )?;
    let paths = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut checked = std::collections::BTreeSet::new();
    for path in paths {
        let path = path?;
        if !crate::tools::discovery::matches_scope(&path, files.scope) {
            continue;
        }
        let Some(dir) = manifest_dir(files, &path) else { continue };
        if checked.insert(dir.clone()) {
            render(files, &dir, out);
        }
    }
    Ok(())
}

fn manifest(files: &Files<'_, '_>, dir: &str) -> Option<toml::Table> {
    files.live(&join(dir, "Cargo.toml")?)?.parse().ok()
}

fn manifest_dir(files: &Files<'_, '_>, file: &str) -> Option<String> {
    let mut dir = parent(file);
    loop {
        if manifest(files, dir).is_some() {
            return Some(dir.to_owned());
        }
        if dir.is_empty() {
            return None;
        }
        dir = parent(dir);
    }
}

fn dependency<'a>(table: &'a toml::Table, name: &str) -> Option<&'a toml::Value> {
    table.get("dependencies")?.get(name)
}

fn bin_sources(table: &toml::Table, dir: &str) -> Vec<String> {
    let bins = table.get("bin").and_then(toml::Value::as_array);
    let mut paths: Vec<_> = bins.into_iter().flatten().filter_map(|bin| join(dir, bin.get("path")?.as_str()?)).collect();
    if table.get("package").is_some() && table.get("package").and_then(|p| p.get("autobins")).and_then(toml::Value::as_bool) != Some(false)
    {
        if let Some(main) = join(dir, "src/main.rs") {
            paths.push(main);
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

fn render(files: &Files<'_, '_>, dir: &str, out: &mut Vec<String>) {
    let Some(package) = manifest(files, dir) else { return };
    if dependency(&package, "tauri").is_none() {
        return;
    }
    let Some(config_path) = join(dir, "tauri.conf.json") else { return };
    let Some(config_src) = files.live(&config_path) else { return };
    let Ok(config) = serde_json::from_str::<serde_json::Value>(&config_src) else { return };
    if !config.is_object() {
        return;
    }

    for file in bin_sources(&package, dir) {
        let Some(src) = files.indexed(&file) else { continue };
        let Some(main) = rust_function(&src, "main") else { continue };
        let Some(body) = main.child_by_field_name("body") else { continue };
        let Some(builder) = call(&src, body, "tauri::Builder::default") else { continue };
        let mut desktop = format!("Tauri desktop {file}:{} fn main | tauri::Builder :{}", line_of(main), line_of(builder));
        if let Some(handler) = builder_method(&src, builder, "invoke_handler") {
            if let Some(token) = call_args(handler).into_iter().find(|n| n.kind() == "macro_invocation") {
                if token.child_by_field_name("macro").is_some_and(|n| src.text(n) == "tauri::generate_handler") {
                    desktop.push_str(&format!(" | commands :{} {}", line_of(token), truncate(src.text(token), 160)));
                }
            }
        }
        out.push(desktop);
        let setup_name = builder_method(&src, builder, "setup").and_then(|call| {
            let args = call_args(call);
            let [name] = args.as_slice() else { return None };
            (name.kind() == "identifier").then(|| src.text(*name))
        });
        if let Some(setup) = setup_name.and_then(|name| rust_function(&src, name)).and_then(|n| n.child_by_field_name("body")) {
            linked_server(files, dir, &package, &file, &src, setup, out);
        }
    }
}

fn rust_function<'a>(src: &'a Source, name: &str) -> Option<Node<'a>> {
    children(src.root())
        .find(|&n| n.kind() == "function_item" && n.child_by_field_name("name").is_some_and(|n| src.text(n) == name) && !n.has_error())
}

fn call<'a>(src: &'a Source, root: Node<'a>, name: &str) -> Option<Node<'a>> {
    nodes(root).find(|&n| callee(n).is_some_and(|f| src.text(f) == name))
}

fn builder_method<'a>(src: &'a Source, mut receiver: Node<'a>, name: &str) -> Option<Node<'a>> {
    loop {
        let field = receiver.parent().filter(|n| n.kind() == "field_expression" && n.child_by_field_name("value") == Some(receiver))?;
        let method = field.parent().filter(|n| callee(*n) == Some(field))?;
        if field.child_by_field_name("field").is_some_and(|n| src.text(n) == name) {
            return Some(method);
        }
        receiver = method;
    }
}

fn dep_dir(files: &Files<'_, '_>, dir: &str, root: &toml::Table, alias: &str) -> Option<String> {
    let dep = dependency(root, alias)?;
    if let Some(path) = dep.get("path").and_then(toml::Value::as_str) {
        return join(dir, path);
    }
    if dep.get("workspace").and_then(toml::Value::as_bool) != Some(true) {
        return None;
    }
    let mut ancestor = parent(dir);
    loop {
        if let Some(workspace) = manifest(files, ancestor).and_then(|t| t.get("workspace").cloned()) {
            let path = workspace.get("dependencies")?.get(alias)?.get("path")?.as_str()?;
            return join(ancestor, path);
        }
        if ancestor.is_empty() {
            return None;
        }
        ancestor = parent(ancestor);
    }
}

fn linked_server(
    files: &Files<'_, '_>,
    dir: &str,
    package: &toml::Table,
    desktop_file: &str,
    desktop: &Source,
    setup: Node<'_>,
    out: &mut Vec<String>,
) {
    let starts: Vec<_> = nodes(setup)
        .filter_map(|node| {
            let function = desktop.text(callee(node)?);
            let alias = function.strip_suffix("::start")?;
            if !alias.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return None;
            }
            let key = alias.replace('_', "-");
            let server = dep_dir(files, dir, package, &key).or_else(|| dep_dir(files, dir, package, alias))?;
            Some((server, alias, node))
        })
        .collect();
    let [(server, alias, desktop_start)] = starts.as_slice() else { return };
    let Some(server_package) = manifest(files, server) else { return };
    let Some(lib) =
        join(server, server_package.get("lib").and_then(|v| v.get("path")).and_then(toml::Value::as_str).unwrap_or("src/lib.rs"))
    else {
        return;
    };
    let Some(lib_src) = files.indexed(&lib) else { return };
    let Some((start_file, start_src)) = exported_start(files, &lib, &lib_src) else { return };
    let Some(start) = rust_function(&start_src, "start") else { return };
    let Some(serve) = start.child_by_field_name("body").and_then(|body| {
        ["axum::serve", "hyper::Server::bind", "HttpServer::new"].into_iter().find_map(|name| call(&start_src, body, name))
    }) else {
        return;
    };
    let mut row = format!(
        "Tauri setup {desktop_file}:{} {alias}::start | server {start_file}:{} {}",
        line_of(*desktop_start),
        line_of(serve),
        start_src.text(callee(serve).unwrap())
    );
    let external = call(desktop, setup, "WebviewUrl::External").or_else(|| {
        nodes(setup).find_map(|n| {
            let function = callee(n).filter(|n| n.kind() == "identifier")?;
            let helper = rust_function(desktop, desktop.text(function))?.child_by_field_name("body")?;
            call(desktop, helper, "WebviewUrl::External")
        })
    });
    if let Some(url) = external {
        row.push_str(&format!(" | External webview :{}", line_of(url)));
    }
    if let Some(url) = call(desktop, setup, "WebviewUrl::App") {
        row.push_str(&format!(" | bundled App webview :{}", line_of(url)));
    }
    out.push(row);

    for bin in bin_sources(&server_package, server) {
        let Some(src) = files.indexed(&bin) else { continue };
        let Some(main) = rust_function(&src, "main") else { continue };
        let direct = format!("{alias}::start");
        let imported = children(src.root()).any(|n| {
            n.kind() == "use_declaration"
                && nodes(n).any(|item| {
                    item.kind() == "scoped_use_list"
                        && item.child_by_field_name("path").is_some_and(|p| src.text(p) == *alias)
                        && item.child_by_field_name("list").is_some_and(|list| children(list).any(|child| src.text(child) == "start"))
                })
        });
        if let Some(boot) = super::first_call(&src, &direct).or_else(|| imported.then(|| super::first_call(&src, "start")).flatten()) {
            out.push(format!("Server binary {bin}:{} fn main | {alias}::start :{}", line_of(main), line_of(boot)));
        }
    }
    served_web(files, server, &lib, &lib_src, out);
}

fn exported_start(files: &Files<'_, '_>, lib: &str, src: &Source) -> Option<(String, Source)> {
    if rust_function(src, "start").is_some_and(|n| children(n).any(|c| src.text(c) == "pub")) {
        return Some((lib.to_owned(), files.indexed(lib)?));
    }
    for uses in children(src.root()).filter(|n| n.kind() == "use_declaration" && children(*n).any(|c| src.text(c) == "pub")) {
        for list in nodes(uses).filter(|n| n.kind() == "scoped_use_list") {
            let Some(module) = list.child_by_field_name("path").filter(|n| n.kind() == "identifier") else { continue };
            if !list.child_by_field_name("list").is_some_and(|n| children(n).any(|c| src.text(c) == "start")) {
                continue;
            }
            let module = src.text(module);
            let declared = children(src.root())
                .any(|n| n.kind() == "mod_item" && n.child_by_field_name("name").is_some_and(|name| src.text(name) == module));
            if declared {
                let file = join(parent(lib), &format!("{module}.rs"))?;
                return Some((file.clone(), files.indexed(&file)?));
            }
        }
    }
    None
}

fn served_web(files: &Files<'_, '_>, dir: &str, lib: &str, src: &Source, out: &mut Vec<String>) {
    let Some(serve) = super::first_call(src, "assets::serve").or_else(|| {
        nodes(src.root()).find(|&n| {
            n.kind() == "scoped_identifier" && src.text(n) == "assets::serve" && n.parent().is_some_and(|p| p.kind() == "arguments")
        })
    }) else {
        return;
    };
    let Some(asset_file) = join(parent(lib), "assets.rs") else { return };
    let Some(asset) = files.indexed(&asset_file) else { return };
    let Some(serve_impl) = rust_function(&asset, "serve") else { return };
    let Some(body) = serve_impl.child_by_field_name("body") else { return };
    for attr in nodes(asset.root()).filter(|n| n.kind() == "attribute_item") {
        let Some(decl) =
            std::iter::successors(attr.next_named_sibling(), |n| n.next_named_sibling()).find(|n| n.kind() != "attribute_item")
        else {
            continue;
        };
        if decl.kind() != "struct_item" {
            continue;
        }
        let Some(name) = decl.child_by_field_name("name") else { continue };
        let getter = format!("{}::get", asset.text(name));
        if call(&asset, body, &getter).is_none() {
            continue;
        }
        let derives_embed = std::iter::successors(decl.prev_named_sibling(), |n| n.prev_named_sibling())
            .take_while(|n| n.kind() == "attribute_item")
            .any(|n| {
                nodes(n).any(|child| {
                    child.kind() == "token_tree"
                        && children(child).any(|token| matches!(asset.text(token), "RustEmbed" | "Embed"))
                        && asset.text(n).starts_with("#[derive")
                })
            });
        if !derives_embed {
            continue;
        }
        let Some(value) = children(attr)
            .find(|n| n.kind() == "attribute")
            .and_then(|n| {
                (n.named_child(0).is_some_and(|c| asset.text(c) == "folder"))
                    .then(|| n.child_by_field_name("value").or_else(|| children(n).nth(1)))
                    .flatten()
            })
            .and_then(|n| asset.literal(n))
        else {
            continue;
        };
        let Some(folder) = join(dir, &value) else { continue };
        let Some(web_root) = folder.strip_suffix("/dist") else { continue };
        if !vite_default_output(files, web_root) {
            continue;
        }
        let Some(html) = join(web_root, "index.html") else { continue };
        for (file, html_line) in html_entries(files, &html, Some(web_root)) {
            let Some(frontend) = files.indexed(&file) else { continue };
            let mount = super::call_line(&frontend, "createRoot").map(|n| format!(":{n} createRoot()")).unwrap_or_default();
            out.push(format!(
                "Server web {lib}:{} assets::serve | {asset_file}:{} embeds {folder} | Vite source {html}:{html_line} -> {file}{mount}",
                line_of(serve),
                line_of(attr)
            ));
        }
    }
}

fn vite_default_output(files: &Files<'_, '_>, root: &str) -> bool {
    let config = ["vite.config.ts", "vite.config.js", "vite.config.mts", "vite.config.mjs"]
        .into_iter()
        .filter_map(|name| join(root, name))
        .find_map(|file| files.indexed(&file));
    let Some(config) = config else { return false };
    // Only report the conventional dist mapping for a declared Vite build
    // with no explicit output/root override in its indexed configuration.
    let Some(export) = children(config.root()).find(|n| n.kind() == "export_statement").and_then(|n| n.child_by_field_name("value")) else {
        return false;
    };
    let Some(callee) = callee(export).filter(|n| config.text(*n) == "defineConfig") else { return false };
    if !children(config.root()).any(|n| {
        n.kind() == "import_statement"
            && n.child_by_field_name("source")
                .and_then(|n| config.literal(n))
                .is_some_and(|module| matches!(module.as_str(), "vite" | "vitest/config"))
            && nodes(n).any(|n| {
                n.kind() == "import_specifier"
                    && n.child_by_field_name("alias").is_none()
                    && n.child_by_field_name("name").is_some_and(|name| config.text(name) == config.text(callee))
            })
    }) {
        return false;
    }
    let args = call_args(export);
    let [options] = args.as_slice() else { return false };
    if options.kind() != "object" || !vite_options(&config, *options, &["root", "configFile"], true) {
        return false;
    }
    let Some(manifest) = join(root, "package.json").and_then(|p| files.live(&p)) else { return false };
    let Ok(package) = serde_json::from_str::<serde_json::Value>(&manifest) else { return false };
    let Some(script) = package["scripts"]["build"].as_str() else { return false };
    script == "vite build" || script == "tsc -b && vite build"
}

fn vite_options(source: &Source, object: Node<'_>, excluded: &[&str], check_build: bool) -> bool {
    children(object).all(|field| {
        let (Some(key), Some(value)) = (field.child_by_field_name("key"), field.child_by_field_name("value")) else { return false };
        let name = match key.kind() {
            "property_identifier" => source.text(key).to_owned(),
            "string" => match source.literal(key) {
                Some(key) => key,
                None => return false,
            },
            _ => return false,
        };
        !excluded.contains(&name.as_str())
            && (!check_build
                || name != "build"
                || value.kind() == "object" && vite_options(source, value, &["outDir", "rollupOptions", "rolldownOptions"], false))
    })
}
