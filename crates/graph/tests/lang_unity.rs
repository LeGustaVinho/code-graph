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
    // `GetComponent<Button>().onClick.AddListener(…)` is an engine call (Button is external).
    assert!(edge(c, "Player.Start", "LifecycleObserver.AddListener", "CALLS").is_none(), "{:?}", targets(c, "Player.Start", "CALLS"));
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

// ---------------------------------------------------------------- phase 5: discovery & vendor

#[test]
fn engine_folders_skipped_nested_build_folders_kept() {
    let mut f = fixture();
    let c = &f.conn;
    assert_eq!(count(c, "SELECT COUNT(*) FROM file_meta WHERE file_path LIKE 'Library/%'"), 0, "Library/PackageCache");
    assert_eq!(count(c, "SELECT COUNT(*) FROM file_meta WHERE file_path = 'Assets/Editor/Build/PostBuild.cs'"), 1);
    assert!(entry(&f, "PostBuild.OnPostprocessBuild").contains("PostProcessBuild"));
    // Incremental re-index (watcher / freshness path) applies the same rules.
    let rel = "Library/PackageCache/com.unity.foo/Cached.cs".to_string();
    codegraph_graph::index_paths(&f.root.clone(), &mut f.conn, &[rel]).unwrap();
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM file_meta WHERE file_path LIKE 'Library/%'"), 0);
}

#[test]
fn third_party_code_is_flagged_and_left_out_of_reports() {
    let f = fixture();
    let c = &f.conn;
    let vendor = |p: &str| count(c, &format!("SELECT COALESCE(MAX(is_vendor), -1) FROM file_meta WHERE file_path = '{p}'"));
    assert_eq!(vendor("Assets/Plugins/Vendor/VendorLib.cs"), 1);
    assert_eq!(vendor("Packages/com.vendor.tool/Editor/EditorUtil.cs"), 1);
    assert_eq!(vendor("Assets/Scripts/Player.cs"), 0);
    let engine = codegraph_mcp::Engine::open(&f.root).unwrap();
    let (default, _) = codegraph_mcp::call_tool(&engine, "find_orphaned_symbols", json!({ "limit": 200 }));
    assert!(!default.contains("VendorLib"), "{default}");
    let (all, _) = codegraph_mcp::call_tool(&engine, "find_orphaned_symbols", json!({ "limit": 200, "include_vendor": true }));
    assert!(all.contains("VendorLib"), "{all}");
}

// ---------------------------------------------------------------- Unity Test Framework

fn module_is_test(c: &rusqlite::Connection, path: &str) -> bool {
    count(c, &format!("SELECT COALESCE(MAX(is_test), -1) FROM node WHERE kind = 'module' AND file_path = '{path}'")) == 1
}

#[test]
fn tests_follow_test_assemblies_not_folder_names() {
    let f = fixture();
    let c = &f.conn;
    // Test assemblies: runner references by name (EditMode) or by GUID (PlayMode, in a folder
    // whose name says nothing about tests); helpers without attributes included.
    assert!(module_is_test(c, "Assets/Tests/EditMode/ScoreTests.cs"));
    assert!(module_is_test(c, "Assets/Tests/EditMode/ScoreBuilder.cs"));
    assert!(module_is_test(c, "Assets/Verification/PlayMode/SceneSmoke.cs"));
    // Assembly-CSharp declaring NUnit tests.
    assert!(module_is_test(c, "Assets/Scripts/InlineChecks.cs"));
    // Production code in a `Test/` folder, and a support assembly under `Tests/`.
    assert!(!module_is_test(c, "Assets/Scripts/Test/MockData.cs"));
    assert!(!module_is_test(c, "Assets/Tests/Support/FakeClock.cs"));
    assert!(!module_is_test(c, "Assets/Scripts/Scoring/Score.cs"));
    for t in ["ScoreTests", "ScoreTests.AddAccumulates", "ScoreTests.AddOnce", "SceneSmoke.Boots", "InlineChecks.CoinsStartPositive"] {
        assert!(sym(c, t).is_test, "{t}");
    }
    assert!(!sym(c, "MockData").is_test);
    assert!(!sym(c, "MockData.StartingCoins").is_test);
}

#[test]
fn tests_link_to_the_runtime_code_they_call() {
    let f = fixture();
    let c = &f.conn;
    assert!(edge(c, "ScoreTests.AddAccumulates", "Score.Add", "TESTS").is_some(), "{:?}", targets(c, "ScoreTests.AddAccumulates", "TESTS"));
    assert!(edge(c, "ScoreTests.AddOnce", "Score.Add", "TESTS").is_some());
    assert!(edge(c, "InlineChecks.CoinsStartPositive", "MockData.StartingCoins", "TESTS").is_some());
}

#[test]
fn runner_hooks_are_entry_points() {
    let f = fixture();
    assert!(entry(&f, "ScoreTests.Init").contains("[SetUp]"));
    assert!(entry(&f, "ScoreTests.Done").contains("[OneTimeTearDown]"));
    assert!(entry(&f, "SceneSmoke.Prepare").contains("[UnitySetUp]"));
    assert_eq!(entry(&f, "SceneSmoke.Setup"), "unity message IPrebuildSetup.Setup");
    assert_eq!(entry(&f, "SceneSmoke.Cleanup"), "unity message IPostBuildCleanup.Cleanup");
    assert!(!sym(&f.conn, "SmokeSettings.Setup").is_entrypoint);
}

#[test]
fn test_flags_follow_asmdef_changes_on_reindex() {
    let mut f = fixture();
    // The support assembly starts referencing the runner: its files become tests.
    let asmdef = f.root.join("Assets/Tests/Support/Game.TestSupport.asmdef");
    std::fs::write(&asmdef, r#"{ "name": "Game.TestSupport", "defineConstraints": ["UNITY_INCLUDE_TESTS"] }"#).unwrap();
    let rel = "Assets/Tests/Support/FakeClock.cs".to_string();
    std::fs::write(f.root.join(&rel), "namespace Game.TestSupport { public class FakeClock { public float Now => 1f; } }").unwrap();
    codegraph_graph::index_paths(&f.root.clone(), &mut f.conn, &[rel.clone()]).unwrap();
    assert!(module_is_test(&f.conn, &rel));
}
