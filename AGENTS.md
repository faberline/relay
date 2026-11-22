# relay repository instructions

relay is the online single-cast pull work-queue broker of the faberline stack (publish, lease, ack, dead-letter, raft-backed HA).

## Output style

- Write in the user's language. Keep paths, commands and exact output intact.
- Use short active sentences, one idea in each.
- Explain a technical term the first time you use it.
- Report what changed, what passed, what is unknown, and the next action.
- Offer at most two choices, and recommend one.

## Scope

- `README.md` is the promise of `relay` and the work roots it owns. Read it before a change. A change keeps the behavior that `README.md` promises.
- This repository is public. Never write a secret, token, private host name, e-mail address or personal path in a file, a commit, an issue or a pull request.
- A GKE run costs money. Never start one, and never add a node pool, without the owner's approval (see `CONTRIBUTING.md`).

## Issue work

- Work on a claimed issue only through the `aw` CLI (`aw claim`, the work, `aw report`). `aw` does the branch, push, pull request, merge and signal comment.
- Edit labels only in the plan step of the `aw-worker` skill.
- Use the outcome that `aw-worker` names: `split` for an issue that is too large, `respec` for an unclear spec in tdd, and `blocked` only for a decision that a person must make.

## Verify

Run these commands before you report a change. Each one is a `run:` command of `.github/workflows/ci.yml`.

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo deny check advisories bans licenses sources`
- `cargo test --locked --workspace`
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
- `cargo test --locked --workspace --all-features`
- `bash scripts/ci/select-gke.test.sh`

- Do not delete, skip or weaken a test, a threshold or a coverage ratchet.
- The default branch requires the check `gate`.
- `CONTRIBUTING.md` holds the full table of checks.
