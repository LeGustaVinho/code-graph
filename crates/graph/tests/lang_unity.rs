//! Unity edge cases (fixtures/edge-cases/unity): engine messages through real inheritance,
//! attribute hooks, calls by name, editor isolation, storage / network and reachability.

mod common;

use common::*;
use serde_json::json;

fn fixture() -> Fixture {
    index_fixture("edge-cases/unity")
}

fn entry(f: &Fixture, q: &str) -> String {
    let s = sym(&f.conn, q);
    assert!(s.is_entrypoint, "{q} should be an entry point: {s:?}");
    s.entry_kind
}

#[test]
fn messages_follow_real_inheritance() {
    let f = fixture();
    // Indirect MonoBehaviour, in a file without `using UnityEngine`.
    assert_eq!(entry(&f, "Player.Update"), "unity message MonoBehaviour.Update");
    assert_eq!(entry(&f, "Mover.Awake"), "unity message MonoBehaviour.Awake", "fully qualified base");
    assert!(entry(&f, "Clicker.OnPointerClick").contains("IPointerClickHandler"));
    assert!(entry(&f, "FancyButton.Awake").contains("MonoBehaviour"), "Button derives from MonoBehaviour");
    assert!(entry(&f, "PlayerEditor.OnInspectorGUI").contains("Editor"));
    assert!(entry(&f, "RangeDrawer.GetPropertyHeight").contains("PropertyDrawer"));
    // Same name, but an IDisposable is not a Unity object.
    assert!(!sym(&f.conn, "Disposer.Reset").is_entrypoint);
}

#[test]
fn attribute_hooks() {
    let f = fixture();
    let c = &f.conn;
    assert!(entry(&f, "Boot.Boot").contains("InitializeOnLoad"), "static constructor");
    assert!(entry(&f, "Game.GameConfig").contains("CreateAssetMenu"));
    assert!(entry(&f, "PlayerEditor").contains("CustomEditor"));
    assert!(edge(c, "PlayerEditor", "Game.Player", "USES_TYPE").is_some(), "[CustomEditor(typeof(Player))]");
    assert!(edge(c, "Mover", "Rigid", "USES_TYPE").is_some(), "[RequireComponent(typeof(Rigid))]");
    assert!(edge(c, "Player.health", "Player.ResetHealth", "CALLS").is_some(), "[ContextMenuItem(…, \"ResetHealth\")]");
}

#[test]
fn calls_by_name_and_generic_lookups() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(edge(c, "Player.Start", "Player.Spawn", "CALLS").as_deref(), Some("exact"), "StartCoroutine(nameof(Spawn))");
    assert_eq!(edge(c, "Player.Start", "Player.Fire", "CALLS").as_deref(), Some("exact"), "Invoke(\"Fire\")");
    assert!(edge(c, "Player.Start", "Rigid", "USES_TYPE").is_some(), "GetComponent<Rigid>()");
    assert_eq!(edge(c, "Player.Start", "Rigid.Push", "CALLS").as_deref(), Some("exact"), "var rb = GetComponent<Rigid>()");
}

#[test]
fn every_platform_branch_is_linked() {
    let f = fixture();
    assert!(edge(&f.conn, "Player.Review", "Player.AskIos", "CALLS").is_some());
    assert!(edge(&f.conn, "Player.Review", "Player.AskAndroid", "CALLS").is_some());
}

#[test]
fn runtime_code_never_links_into_editor_code() {
    let f = fixture();
    let c = &f.conn;
    assert!(edge(c, "Player.Start", "EditorUtil.SetTarget", "CALLS").is_none(), "Editor folder");
    assert!(edge(c, "Player.Start", "ToolHelper.Warp", "CALLS").is_none(), "Editor-only asmdef");
}

#[test]
fn player_prefs_and_web_requests() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(count(c, "SELECT COUNT(*) FROM state_mutation WHERE target_state = 'PlayerPrefs:coins'"), 1, "const key resolved");
    assert_eq!(count(c, "SELECT COUNT(*) FROM config_dependency WHERE config_key = 'level' AND source_type = 'PlayerPrefs'"), 1);
    assert_eq!(count(c, "SELECT COUNT(*) FROM http_call WHERE url = 'https://cdn.example.com/a.png'"), 1, "UnityWebRequestTexture");
    assert_eq!(count(c, "SELECT COUNT(*) FROM http_call WHERE url LIKE '<dynamic:%url%'"), 1, "non-literal URL");
}

#[test]
fn reachability_without_scenes() {
    let f = fixture();
    let engine = codegraph_mcp::Engine::open(&f.root).unwrap();
    let (text, is_err) = codegraph_mcp::call_tool(&engine, "find_orphaned_symbols", json!({ "limit": 200 }));
    assert!(!is_err, "{text}");
    assert!(text.contains("NeverUsed"), "private unused methods are still reported:\n{text}");
    for reachable in ["OnButtonClicked", "class Player ", "GameConfig", "Rigid", "Clicker"] {
        assert!(!text.contains(reachable), "{reachable} is reachable through scenes / assets:\n{text}");
    }
}
