use codegraph_graph::query::find_nodes;
use codegraph_graph::{index_project, open, IndexOptions};
use rusqlite::params;

#[test]
fn scoped_imports_follow_only_exact_unique_concrete_bindings() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for (path, source) in [
        ("target.ts", "export function actual() { return 1; }\nexport type Payload = { id: string };\nfunction hidden() {}\n"),
        ("other.ts", "export function alternative() { return 2; }\n"),
        (
            "first/consumer.ts",
            "import { actual as Alias } from '../target.js';\n\
             import { actual as renamed } from '../target';\n\
             import { actual as Escaped } from '../../target';\n\
             import type { Payload as Wire } from '../target';\n\
             import { hidden as Private } from '../target';\n\
             import * as Namespace from '../target';\n\
             class Worker { Renamed() {} }\n",
        ),
        ("second/consumer.ts", "import { alternative as Alias } from '../other';\n"),
        ("directory/index.ts", "export function direct() {}\n"),
        ("directory-consumer.ts", "import { direct as DirectoryAlias } from './directory';\n"),
        ("barrel/index.ts", "export * from './one';\nexport * from './two';\n"),
        ("barrel/one.ts", "export const Shared = 1;\n"),
        ("barrel/two.ts", "export const Shared = 2;\n"),
        ("barrel-consumer.ts", "import { Shared as BarrelAlias } from './barrel';\n"),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
    let mut conn = open(root).unwrap();
    index_project(root, &mut conn, IndexOptions::default()).unwrap();

    let names = |query| find_nodes(&conn, query, 20).unwrap().into_iter().map(|node| (node.file_path, node.sym.name)).collect::<Vec<_>>();
    let alias = "first/consumer.ts:Alias";
    let target = [("target.ts".to_owned(), "actual".to_owned())];
    assert_eq!(names(alias), target);
    assert_eq!(names("first/consumer.ts:Wire"), [("target.ts".to_owned(), "Payload".to_owned())]);
    assert_eq!(names("first/consumer.ts:renamed"), target);
    assert_eq!(names("first/consumer.ts:Renamed"), [("first/consumer.ts".to_owned(), "Renamed".to_owned())]);
    assert!(names("first/consumer.ts:alias").is_empty());
    assert!(names("first/consumer.ts:Namespace").is_empty());
    assert!(names("first/consumer.ts:Private").is_empty());
    assert!(names("first/consumer.ts:Escaped").is_empty());
    assert_eq!(names("directory-consumer.ts:DirectoryAlias"), [("directory/index.ts".to_owned(), "direct".to_owned())]);
    let barrel_edge: (String, String) = conn
        .query_row("SELECT target_file, confidence FROM edge WHERE source_file = 'barrel-consumer.ts' AND relation = 'IMPORTS'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(barrel_edge, ("barrel/one.ts".to_owned(), "exact".to_owned()));
    assert!(names("barrel-consumer.ts:BarrelAlias").is_empty());
    assert!(names("consumer.ts:Alias").is_empty());
    assert!(find_nodes(&conn, alias, 0).unwrap().is_empty());

    let copy_import = |target_name: &str, confidence: &str| {
        let affected = conn
            .execute(
                "INSERT INTO edge(source_id,target_id,relation,confidence,source_file,metadata)
                 SELECT source_id, (SELECT id FROM node WHERE name = ?1), 'IMPORTS', ?2, source_file, metadata
                 FROM edge WHERE source_file = 'first/consumer.ts' AND relation = 'IMPORTS' AND json_extract(metadata, '$.local') = 'Alias'
                 LIMIT 1",
                params![target_name, confidence],
            )
            .unwrap();
        assert_eq!(affected, 1);
    };
    copy_import("actual", "exact");
    copy_import("alternative", "inferred");
    assert_eq!(names(alias), target, "duplicate edges to the same target are one binding");
    copy_import("alternative", "exact");
    assert!(names(alias).is_empty(), "competing exact targets make the binding ambiguous");
    conn.execute("UPDATE edge SET confidence = 'inferred' WHERE source_file = 'first/consumer.ts'", []).unwrap();
    assert!(names(alias).is_empty(), "inferred-only bindings do not qualify");
}
