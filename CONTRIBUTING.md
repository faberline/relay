# Contributing to relay

## Brief

How to change `relay`. What it promises and the work roots it owns live in
[README.md](README.md); repository-wide authoring and verification rules live
in [AW standards](https://github.com/faberline/aw/tree/main/docs/standards).

Issues are delivered with the `aw-worker` skill from
[AW](https://github.com/faberline/aw/tree/main/plugins/faberline/skills/aw-worker).
One issue carries one `stage:` label at a time: `stage:spec` (write the
spec), `stage:tdd` (tests first, then code; `aw` opens the pull request),
`stage:review` (review a green pull request). `aw report passed` in review
squash-merges the pull request and moves the issue to `stage:done`.

CI runs the required checks. The default branch requires the check `gate`
and a pull request cannot merge before it is green on the exact head. Local
work runs the affected unit and static checks listed under Verification.
Keep all feature matrices, test selection checks and numeric limits.

## Verification

| Gate | Command |
|---|---|
| format | `cargo fmt --all -- --check` |
| lint | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| dependency audit | `cargo deny check advisories bans licenses sources` |
| tests (workspace) | `cargo test --locked --workspace` |
| lint, all features | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` |
| tests, all features | `cargo test --locked --workspace --all-features` |
| release contract | `uv run --python 3.13 --no-project scripts/release/verify-release-contract.py --app relay`; `uv run --python 3.13 --no-project scripts/release/verify-release-contract.py --self-test`; `uv run --python 3.13 --no-project scripts/release/make-gke-release-receipt.py --self-test` |
