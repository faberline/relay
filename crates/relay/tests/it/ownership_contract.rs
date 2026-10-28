// HANDWRITE-BEGIN gap="missing-generator:unit-test:relay-capability-shared-ownership" tracker="#40" reason="Capability contracts and shared ownership verification contract: context crate dependency edges and single fencing owner."
//! Executable capability contract and shared ownership verification:
//! - Dependency edges: bounded contexts depend only on contexts permitted by ddd.toml plus relay-shared-kernel.
//! - Single fencing owner: work-queue fencing is implemented in relay-broker (and raft in relay-replication),
//!   never duplicated in the relay assembly crate.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

struct DddContext {
    name: String,
    depends_on: Vec<String>,
}

fn parse_ddd_contexts(ddd_toml_content: &str) -> Vec<DddContext> {
    let mut contexts = Vec::new();
    let mut in_context = false;
    let mut current_name = None;
    let mut current_deps = Vec::new();

    for line in ddd_toml_content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        if trimmed == "[[contexts]]" {
            if in_context {
                if let Some(name) = current_name.take() {
                    contexts.push(DddContext {
                        name,
                        depends_on: std::mem::take(&mut current_deps),
                    });
                }
            }
            in_context = true;
            current_name = None;
            current_deps.clear();
            continue;
        }

        if in_context && trimmed.starts_with('[') && !trimmed.starts_with("[contexts") {
            if let Some(name) = current_name.take() {
                contexts.push(DddContext {
                    name,
                    depends_on: std::mem::take(&mut current_deps),
                });
            }
            in_context = false;
            continue;
        }

        if in_context {
            if let Some(rest) = trimmed.strip_prefix("name =") {
                let name = rest.trim().trim_matches('"').trim_matches('\'').to_string();
                current_name = Some(name);
            } else if let Some(rest) = trimmed.strip_prefix("depends_on =") {
                let deps_str = rest.trim();
                if let (Some(start), Some(end)) = (deps_str.find('['), deps_str.rfind(']')) {
                    let inner = &deps_str[start + 1..end];
                    for item in inner.split(',') {
                        let item = item.trim().trim_matches('"').trim_matches('\'').trim();
                        if !item.is_empty() {
                            current_deps.push(item.to_string());
                        }
                    }
                }
            }
        }
    }

    if in_context {
        if let Some(name) = current_name.take() {
            contexts.push(DddContext {
                name,
                depends_on: current_deps,
            });
        }
    }

    contexts
}

fn parse_cargo_relay_deps(cargo_toml: &str) -> BTreeSet<String> {
    let mut deps = BTreeSet::new();
    let mut current_section = String::new();

    for line in cargo_toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            current_section = trimmed[1..trimmed.len() - 1].trim().to_string();
            if let Some(crate_name) = current_section
                .strip_prefix("dependencies.")
                .or_else(|| current_section.strip_prefix("dev-dependencies."))
                .or_else(|| current_section.strip_prefix("build-dependencies."))
            {
                let crate_name = crate_name.trim();
                if crate_name.starts_with("relay-") {
                    deps.insert(crate_name.to_string());
                }
            }
            continue;
        }

        if current_section == "package" || current_section.starts_with("package.") {
            continue;
        }

        let is_dep_section = current_section == "dependencies"
            || current_section == "dev-dependencies"
            || current_section == "build-dependencies"
            || current_section.ends_with(".dependencies")
            || current_section.ends_with(".dev-dependencies")
            || current_section.ends_with(".build-dependencies");

        if is_dep_section {
            let key = if let Some((k, _)) = trimmed.split_once('=') {
                k.trim()
            } else {
                trimmed
            };
            let key = if let Some((k, _)) = key.split_once('.') {
                k.trim()
            } else {
                key
            };
            if key.starts_with("relay-") {
                deps.insert(key.to_string());
            }
        }

        if current_section == "features" {
            if let Some((_, val)) = trimmed.split_once('=') {
                for part in val.split(',') {
                    let cleaned = part
                        .trim()
                        .trim_matches('[')
                        .trim_matches(']')
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'')
                        .trim();
                    let dep = if let Some(d) = cleaned.strip_prefix("dep:") {
                        d
                    } else if let Some((d, _)) = cleaned.split_once('/') {
                        d
                    } else {
                        cleaned
                    };
                    if dep.starts_with("relay-") {
                        deps.insert(dep.to_string());
                    }
                }
            }
        }
    }

    deps
}

// Behavior evidence for shared fencing and lease semantics:
// - raft_cluster::lease_and_ack_are_committed_and_executor_fenced
// - reconciler::dead_worker_redelivered_and_late_ack_fenced
// - work_queue_api
#[test]
fn capability_contracts_and_shared_ownership() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .join("../..")
        .canonicalize()
        .expect("canonicalize repo root");

    // 1. Dependency edge verification against ddd.toml
    let ddd_path = repo_root.join("ddd.toml");
    let ddd_content = fs::read_to_string(&ddd_path)
        .unwrap_or_else(|e| panic!("failed to read ddd.toml at {:?}: {}", ddd_path, e));
    let contexts = parse_ddd_contexts(&ddd_content);

    let expected_contexts = [
        "storage",
        "broker",
        "replication",
        "access",
        "queue",
        "operator",
    ];
    assert_eq!(
        contexts.len(),
        expected_contexts.len(),
        "expected {} contexts in ddd.toml, found {}",
        expected_contexts.len(),
        contexts.len()
    );

    let mut total_relay_edges = 0;
    let mut checked_context_names = BTreeSet::new();

    for ctx in &contexts {
        assert!(
            expected_contexts.contains(&ctx.name.as_str()),
            "unexpected context name in ddd.toml: {}",
            ctx.name
        );
        checked_context_names.insert(ctx.name.clone());

        let mut allowed = BTreeSet::new();
        allowed.insert("relay-shared-kernel".to_string());
        for dep in &ctx.depends_on {
            allowed.insert(format!("relay-{dep}"));
        }

        let cargo_toml_path = repo_root
            .join("crates")
            .join(format!("relay-{}", ctx.name))
            .join("Cargo.toml");
        let cargo_content = fs::read_to_string(&cargo_toml_path).unwrap_or_else(|e| {
            panic!(
                "failed to read Cargo.toml for context {} at {:?}: {}",
                ctx.name, cargo_toml_path, e
            )
        });

        let observed_deps = parse_cargo_relay_deps(&cargo_content);
        for dep in &observed_deps {
            assert!(
                allowed.contains(dep),
                "context '{}' in {:?} depends on '{}' which is not in ddd.toml depends_on ({:?}) plus relay-shared-kernel",
                ctx.name,
                cargo_toml_path,
                dep,
                ctx.depends_on
            );
        }

        total_relay_edges += observed_deps.len();
    }

    assert_eq!(
        checked_context_names.len(),
        6,
        "all 6 context crates must be checked"
    );
    assert!(
        total_relay_edges > 0,
        "expected at least one relay-* dependency edge across contexts, found none"
    );

    // 2. Fencing ownership verification
    let broker_workqueue_path = repo_root.join("crates/relay-broker/src/workqueue.rs");
    let broker_workqueue_content = fs::read_to_string(&broker_workqueue_path)
        .unwrap_or_else(|e| panic!("failed to read {:?}: {}", broker_workqueue_path, e));
    assert!(
        broker_workqueue_content.contains("FenceToken"),
        "crates/relay-broker/src/workqueue.rs must reference FenceToken"
    );
    assert!(
        broker_workqueue_content.contains("FencedAssignment"),
        "crates/relay-broker/src/workqueue.rs must reference FencedAssignment"
    );

    let assembly_src = repo_root.join("crates/relay/src");
    let mut checked_assembly_files = 0;
    let mut dirs_to_visit = vec![assembly_src];

    while let Some(dir) = dirs_to_visit.pop() {
        for entry in
            fs::read_dir(&dir).unwrap_or_else(|e| panic!("failed to read dir {:?}: {}", dir, e))
        {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "bin") {
                    continue;
                }
                dirs_to_visit.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let file_content = fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("failed to read assembly source {:?}: {}", path, e));
                assert!(
                    !file_content.contains("FenceToken"),
                    "File {:?} in crates/relay/src/ contains FenceToken; assembly crate must not implement fencing",
                    path
                );
                assert!(
                    !file_content.contains("FencedAssignment"),
                    "File {:?} in crates/relay/src/ contains FencedAssignment; assembly crate must not implement fencing",
                    path
                );
                checked_assembly_files += 1;
            }
        }
    }

    assert!(
        checked_assembly_files > 0,
        "expected to inspect assembly crate source files in crates/relay/src (excluding bin/), found none"
    );
}
// HANDWRITE-END
