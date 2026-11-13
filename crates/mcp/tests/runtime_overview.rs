use std::path::Path;

use codegraph_mcp::{call_tool, Engine};
use serde_json::{json, Value};

fn fixture(root: &Path, files: &[(&str, &str)]) {
    for &(path, source) in files {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
}

fn overview(engine: &Engine, args: Value) -> String {
    let (text, error) = call_tool(engine, "get_project_overview", args);
    assert!(!error, "{text}");
    text
}

#[test]
fn electron_overview_uses_declared_inputs_and_module_scripts_in_scope() {
    let temp = tempfile::tempdir().unwrap();
    fixture(
        temp.path(),
        &[
            ("apps/desktop/package.json", r#"{"build":{"main":"wrong"},"main":"out/main/index.js","devDependencies":{"electron":"1"}}"#),
            (
                "apps/desktop/electron.vite.config.ts",
                r#"
import { defineConfig } from 'electron-vite';
import { resolve } from 'node:path';
export default defineConfig({
    main: { build: { rollupOptions: { input: resolve(__dirname, 'src/main.ts') } } },
    preload: { build: { rollupOptions: { input: 'src/preload.ts' } } },
    renderer: {
        root: resolve(__dirname, 'src/web'),
        build: { rollupOptions: { input: 'src/web/index.html' } }
    }
});
"#,
            ),
            (
                "apps/desktop/src/main.ts",
                "import { app, BrowserWindow } from 'electron';\n\
                 import path from 'node:path';\n\
                 app.whenReady();\n\
                 const fakePath = { join() { return 'wrong'; } };\n\
                 new BrowserWindow({webPreferences:{preload:fakePath.join(__dirname, 'wrong')}});\n\
                 new BrowserWindow({webPreferences:{preload:path.join(__dirname, '../preload/index.js')}});\n",
            ),
            (
                "apps/desktop/src/preload.ts",
                "import { contextBridge as bridge, ipcRenderer } from 'electron';\n\
                 function decoy(bridge: any) { bridge.exposeInMainWorld('fake', {}); }\n\
                 bridge.exposeInMainWorld('api', { load: () => ipcRenderer.invoke('load') });\n",
            ),
            (
                "apps/desktop/src/web/index.html",
                "<!-- <script type='module' src='./decoy.ts'></script> -->\n\
                 <template><script type='module' src='./decoy.ts'></script></template>\n\
                 <script data-note='a > b' src='/app.ts' type='module'></script>\n\
                 <script type='module' src='../../../../outside.ts'></script>\n",
            ),
            ("apps/desktop/src/web/app.ts", "const bridge = window.api;\n"),
            ("apps/desktop/src/web/decoy.ts", "export const ignored = true;\n"),
            ("outside.ts", "export const outside = 42;\n"),
            ("apps/other/main.ts", "export const unrelated = 0;\n"),
        ],
    );
    let engine = Engine::open(temp.path()).unwrap();
    let result = overview(&engine, json!({"runtime":true,"path":"apps/desktop"}));
    for expected in [
        "Electron main (input) apps/desktop/src/main.ts",
        "apps/desktop/src/main.ts:3 app.whenReady() | preload :6 ../preload/index.js",
        "apps/desktop/src/preload.ts:3 contextBridge.exposeInMainWorld('api', ...), ipcRenderer.invoke :3",
        "apps/desktop/src/web/index.html:3 -> apps/desktop/src/web/app.ts",
        "apps/desktop/src/web/app.ts:1 window.api",
        "Electron packaged main out/main/index.js | apps/desktop/package.json:1",
    ] {
        assert!(result.contains(expected), "missing {expected}: {result}");
    }
    for excluded in ["decoy.ts", "outside.ts", "fake", "apps/other", "wrong"] {
        assert!(!result.contains(excluded), "{result}");
    }

    let narrow = overview(&engine, json!({"runtime":true,"path":"apps/desktop/src"}));
    assert!(narrow.contains("No supported desktop entry evidence"), "{narrow}");
    let limited = overview(&engine, json!({"runtime":true,"path":"apps/desktop","limit":1}));
    assert!(limited.contains("[MORE: showing 1 of 4 results"), "{limited}");
    assert_eq!(overview(&engine, json!({})), overview(&engine, json!({"runtime":false})));

    std::fs::write(temp.path().join("apps/desktop/.gitignore"), "src/web/index.html\n").unwrap();
    let ignored = overview(&engine, json!({"runtime":true,"path":"apps/desktop"}));
    assert!(ignored.contains("Electron main") && !ignored.contains("Electron renderer"), "{ignored}");
}

#[test]
fn tauri_overview_links_cargo_bin_and_local_server_boot_without_using_frontend_dist() {
    let temp = tempfile::tempdir().unwrap();
    fixture(
        temp.path(),
        &[
            ("desktop/Cargo.toml", "[package]\nname='desktop'\n[dependencies]\ntauri='2'\nlocal-server={path='../server'}\n"),
            ("desktop/tauri.conf.json", r#"{"build":{"frontendDist":"fallback"}}"#),
            (
                "desktop/src/main.rs",
                r#"fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![save, refresh])
        .setup(setup);
}
fn setup() {
    let server = local_server::start();
    WebviewUrl::App(fallback);
    show(server);
}
fn unrelated() { WebviewUrl::External(wrong) }
fn show(server: Server) { WebviewUrl::External(server.url()) }
"#,
            ),
            ("desktop/fallback/index.html", "<script type='module' src='./wrong.ts'></script>\n"),
            ("desktop/fallback/wrong.ts", "export const wrong = true;\n"),
            ("server/Cargo.toml", "[package]\nname='local-server'\n"),
            (
                "server/src/lib.rs",
                "mod assets;\nmod server;\npub use server::{start};\nfn router() { Router::new().fallback(assets::serve); }\n",
            ),
            ("server/src/server.rs", "pub async fn start() { axum::serve(listener, router()).await; }\n"),
            ("server/src/main.rs", "use local_server::{start};\nfn main() { start(); }\n"),
            (
                "server/src/assets.rs",
                "use rust_embed::RustEmbed;\n#[derive(RustEmbed)]\n#[folder = \"../web/dist\"]\nstruct Assets;\npub fn serve() { Assets::get(\"index.html\"); }\n",
            ),
            ("web/package.json", r#"{"scripts":{"build":"vite build"}}"#),
            ("web/vite.config.ts", "import { defineConfig } from 'vite';\nexport default defineConfig({});\n"),
            ("web/index.html", "<script type='module' src='/main.tsx'></script>\n"),
            ("web/main.tsx", "import { createRoot } from 'react-dom/client';\ncreateRoot(document.getElementById('root')!);\n"),
        ],
    );
    let engine = Engine::open(temp.path()).unwrap();
    let result = overview(&engine, json!({"runtime":true}));
    for expected in [
        "Tauri desktop desktop/src/main.rs:1 fn main | tauri::Builder :2 | commands :3",
        "tauri::generate_handler![save, refresh]",
        "desktop/src/main.rs:7 local_server::start | server server/src/server.rs:1 axum::serve",
        "External webview :12 | bundled App webview :8",
        "Server binary server/src/main.rs:2 fn main",
        "server/src/assets.rs:3 embeds web/dist | Vite source web/index.html:1 -> web/main.tsx:2 createRoot()",
    ] {
        assert!(result.contains(expected), "missing {expected}: {result}");
    }
    assert!(!result.contains("wrong") && !result.contains("fallback/index.html"), "{result}");
    let narrow = overview(&engine, json!({"runtime":true,"path":"desktop"}));
    assert!(narrow.contains("Tauri desktop") && !narrow.contains("Server binary"), "{narrow}");
}
