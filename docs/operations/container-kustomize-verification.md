# Container and Kustomize verification

This is the offline gate for the container and Kustomize artifacts: every overlay under `k8s/` composes from files that exist, the shipped files carry no unresolved placeholder, and both Dockerfiles run the `relay` entrypoint as a non-root user.

## Commands

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked -p relay --test it direct_k8s_assets::
cargo test --locked -p relay --test it deploy_cli::
cargo test --locked -p relay --features operator --test it operator::
cargo test --locked -p relay-operator
```

The `operator::` module is compiled only with `--features operator`.

## Tests

- `direct_k8s_assets::every_overlay_composes_from_existing_files` composes `dev`, `staging`, `prod`, `gke-test` and `template`. It checks the expected kinds for each overlay and that every patch file exists.
- `direct_k8s_assets::shipped_overlays_have_no_unresolved_placeholder` checks that the base, the components and the `dev`, `staging` and `prod` overlays contain no `REPLACE_ME` or `${`. It also checks that the `gke-test` and `template` markers stay in place.
- `direct_k8s_assets::dockerfiles_run_non_root_with_the_relay_entrypoint` checks the `ENTRYPOINT`, the non-root user and the distroless runtime of `Dockerfile` and `Dockerfile.release`, and the StatefulSet command and image name.

## Result

Commit under test: `4371298` (the tree before this change).

| Filter | Result |
|---|---|
| `direct_k8s_assets::` | 5 passed |
| `deploy_cli::` | 7 passed |
| `operator::` (with `--features operator`) | 7 passed |
| `cargo test -p relay-operator` | exit 0; the crate has no tests of its own |

The live checks (`kubectl kustomize`, the image build and the cluster deploy) run in CI and in the cluster e2e jobs, not in this gate.

The open issues #32, #33, #34 and #35 own the operator, topology, CRD and Dockerfile-fixture claims. This page does not duplicate them.

Result: passed
