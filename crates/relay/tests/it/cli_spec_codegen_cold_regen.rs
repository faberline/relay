// HANDWRITE-BEGIN gap="missing-generator:unit-test:cli-spec-codegen-cold-regen" tracker="#41" reason="Prove offline cold client regeneration is deterministic and free of scaffold placeholders."
//! Cold client regeneration determinism and no-scaffold verification (#41).
//!
//! Drives the COMPILED `relay` binary offline across TypeScript, Python, and
//! Rust: cold regeneration into two separate fresh directories yields byte-equal
//! file trees including `.openapi-codegen.json`, non-zero client bytes,
//! standard TypeScript entrypoints matching OpenAPI paths, and no scaffold
//! placeholders.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run_relay(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_relay"))
        .args(args)
        .output()
        .expect("run relay binary")
}

fn openapi_path_to_ts_template(path: &str) -> String {
    let mut out = String::new();
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            for c in chars.by_ref() {
                if c == '}' {
                    break;
                }
                name.push(c);
            }
            out.push_str("${data.path.");
            out.push_str(&name);
            out.push('}');
        } else {
            out.push(c);
        }
    }
    out
}

fn collect_tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(current) = dirs.pop() {
        for entry in std::fs::read_dir(&current).expect("read dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.is_file() {
                let rel = path.strip_prefix(root).expect("strip prefix").to_path_buf();
                let bytes = std::fs::read(&path).expect("read file");
                files.insert(rel, bytes);
            }
        }
    }
    files
}

#[test]
fn spec_codegen_cold_regeneration_is_deterministic_and_scaffold_free() {
    // Terminal marker & JSON parsing for `relay spec --format openapi`.
    let spec_output = run_relay(&["spec", "--format", "openapi"]);
    assert!(
        spec_output.status.success(),
        "relay spec failed: {}",
        String::from_utf8_lossy(&spec_output.stderr)
    );
    let spec_stdout = String::from_utf8(spec_output.stdout).expect("utf8 stdout");
    let doc: serde_json::Value = serde_json::from_str(&spec_stdout)
        .expect("`relay spec --format openapi` stdout must parse as JSON");
    let paths_obj = doc
        .get("paths")
        .and_then(|p| p.as_object())
        .expect("OpenAPI doc missing paths object");

    // Acceptance: at least /v1/{subject}/publish and /admin/backup.
    assert!(
        paths_obj.contains_key("/v1/{subject}/publish"),
        "OpenAPI paths must contain /v1/{{subject}}/publish"
    );
    assert!(
        paths_obj.contains_key("/admin/backup"),
        "OpenAPI paths must contain /admin/backup"
    );
    let openapi_paths: Vec<String> = paths_obj.keys().cloned().collect();

    let mut total_bytes: u64 = 0;

    for lang in ["ts", "py", "rust"] {
        let dir_a = tempfile::tempdir().expect("tempdir A");
        let dir_b = tempfile::tempdir().expect("tempdir B");

        let out_a = run_relay(&[
            "spec",
            "gen",
            "--lang",
            lang,
            "--out",
            dir_a.path().to_str().unwrap(),
        ]);
        assert!(
            out_a.status.success(),
            "relay spec gen {lang} run A failed: {}",
            String::from_utf8_lossy(&out_a.stderr)
        );
        let stdout_a = String::from_utf8(out_a.stdout).expect("utf8 stdout A");
        assert!(
            stdout_a.lines().any(|line| line.trim() == "next: done"),
            "relay spec gen {lang} run A stdout missing terminal marker 'next: done'"
        );

        let out_b = run_relay(&[
            "spec",
            "gen",
            "--lang",
            lang,
            "--out",
            dir_b.path().to_str().unwrap(),
        ]);
        assert!(
            out_b.status.success(),
            "relay spec gen {lang} run B failed: {}",
            String::from_utf8_lossy(&out_b.stderr)
        );
        let stdout_b = String::from_utf8(out_b.stdout).expect("utf8 stdout B");
        assert!(
            stdout_b.lines().any(|line| line.trim() == "next: done"),
            "relay spec gen {lang} run B stdout missing terminal marker 'next: done'"
        );

        let tree_a = collect_tree(dir_a.path());
        let tree_b = collect_tree(dir_b.path());

        // Non-zero evidence: each tree has at least 1 file, every file is non-empty.
        assert!(
            !tree_a.is_empty(),
            "cold generation for {lang} emitted no files"
        );
        for (rel_path, bytes) in &tree_a {
            assert!(
                !bytes.is_empty(),
                "cold generation file {:?} for {lang} is empty",
                rel_path
            );
            total_bytes += bytes.len() as u64;
        }

        // Determinism: the two trees have the same relative file names and byte-equal contents,
        // including .openapi-codegen.json.
        assert!(
            tree_a.contains_key(Path::new(".openapi-codegen.json")),
            "cold generation for {lang} missing .openapi-codegen.json"
        );
        assert_eq!(
            tree_a.keys().collect::<Vec<_>>(),
            tree_b.keys().collect::<Vec<_>>(),
            "file set mismatch in cold regeneration for {lang}"
        );
        assert_eq!(
            tree_a, tree_b,
            "byte contents mismatch in cold regeneration for {lang}"
        );

        // No scaffold: no generated file contains TODO, unimplemented!, todo! or NotImplemented.
        for (rel_path, bytes) in &tree_a {
            let text = String::from_utf8_lossy(bytes);
            for placeholder in ["TODO", "unimplemented!", "todo!", "NotImplemented"] {
                assert!(
                    !text.contains(placeholder),
                    "file {:?} in {lang} contains scaffold placeholder {placeholder:?}",
                    rel_path
                );
            }
        }

        // Language-specific assertions for TypeScript.
        if lang == "ts" {
            for entry in ["types.ts", "client.ts", "index.ts"] {
                assert!(
                    tree_a.contains_key(Path::new(entry)),
                    "TypeScript client missing {entry}"
                );
            }
            let ts_client_text: String = tree_a
                .iter()
                .filter(|(p, _)| p.extension().and_then(|s| s.to_str()) == Some("ts"))
                .map(|(_, b)| String::from_utf8_lossy(b))
                .collect();
            for path in &openapi_paths {
                let ts_template = openapi_path_to_ts_template(path);
                assert!(
                    ts_client_text.contains(path.as_str()) || ts_client_text.contains(&ts_template),
                    "generated TypeScript client text missing operation path {path}"
                );
            }
        }
    }

    assert!(
        total_bytes > 0,
        "total generated bytes across ts, py, rust must be greater than zero"
    );
}
// HANDWRITE-END
