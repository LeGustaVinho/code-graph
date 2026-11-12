//! Cross-process regression: both `query` commands initialize the same cold C# graph.

use std::path::Path;
use std::process::{Command, Output, Stdio};

fn create_csharp_fixture(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("Service.cs"), "namespace Demo { public class Service { public void Run() { new Helper().DoWork(); } } }")
        .unwrap();
    std::fs::write(root.join("Helper.cs"), "namespace Demo { public class Helper { public void DoWork() {} } }").unwrap();
    // Keep the initial indexing window open long enough for both processes to compete.
    for i in 0..60 {
        std::fs::write(
            root.join(format!("Other{i}.cs")),
            format!("namespace Demo {{ public class Other{i} {{ public string Get() {{ return \"value{i}\"; }} }} }}"),
        )
        .unwrap();
    }
}

fn query_pair(root: &Path) -> (Output, Output) {
    let launch = || {
        Command::new(env!("CARGO_BIN_EXE_codegraph"))
            .arg("query")
            .arg("get_file_skeleton")
            .arg("--root")
            .arg(root)
            .arg("--args")
            .arg(r#"{"file_path":"Service.cs","format":"json"}"#)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let first = launch();
    let second = launch();
    (first.wait_with_output().unwrap(), second.wait_with_output().unwrap())
}

#[test]
fn concurrent_queries_succeed_on_cold_and_warm_csharp_graphs() {
    for _cold_window in 0..2 {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        create_csharp_fixture(&root);
        for phase in ["cold", "warm"] {
            let (first, second) = query_pair(&root);
            for (i, output) in [first, second].into_iter().enumerate() {
                assert!(output.status.success(), "{phase} query #{i}: {}", String::from_utf8_lossy(&output.stderr));
                let stdout = String::from_utf8_lossy(&output.stdout);
                assert!(stdout.contains("Service"), "{phase} query #{i}: {stdout}");
            }
        }
    }
}
