# Container and Kustomize verification

This is the offline gate for the container and Kustomize artifacts: every overlay under `k8s/` composes from files that exist, the shipped overlays have no unresolved placeholder, and both Dockerfiles run as non-root with the `relay` entrypoint.

## Commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --locked -p relay --test it direct_k8s_assets::
cargo test --locked -p relay --test it deploy_cli::
cargo test --locked -p relay --features operator --test it operator::
cargo test --locked -p relay-operator
```

`operator::` is behind the `operator` feature, so its run needs `--features operator`.

## Tests

- `direct_k8s_assets::every_overlay_composes_from_existing_files`: composes the `dev`, `staging`, `prod`, `gke-test` and `template` overlays from their `resources` and `components`, asserts the expected kinds, and asserts that every patch file exists.
- `direct_k8s_assets::shipped_overlays_have_no_unresolved_placeholder`: no `REPLACE_ME` or `${` in the shipped base, components and `dev`, `staging`, `prod` overlays. The two template overlays (`gke-test`, `template`) keep their `REPLACE_ME` marker.
- `direct_k8s_assets::dockerfiles_run_non_root_with_the_relay_entrypoint`: both Dockerfiles end with `ENTRYPOINT ["/usr/local/bin/relay"]`, `Dockerfile` runs as `USER relay`, `Dockerfile.release` uses the `nonroot` distroless base, and the base StatefulSet runs `relay`.

## Last result

| Run | Passed |
|-----|--------|
| `direct_k8s_assets::` | 5 |
| `deploy_cli::` | 7 |
| `operator::` (with `--features operator`) | 7 |
| `cargo test -p relay-operator` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |

Commit: `2103a63`.

The live checks (`kubectl kustomize`, image builds and cluster deploys) run in CI and in the cluster e2e jobs, not in this gate.

The open owners #32, #33, #34 and #35 cover the topology, operator, CRD and Dockerfile-fixture claims. This page does not duplicate them.

Result: passed
