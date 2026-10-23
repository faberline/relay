# CI lifecycle handoff

`handoff.py` is vendored unchanged from faberline/aw commit `67af8d020df3ca2ec4b064289408cfb0c2f8b299`,
`plugins/faberline/skills/issue-ci/scripts/handoff.py`. SHA-256: `c67533b39d45ad62d407af3748856be396941647c349b63464424fa6aff1c198`.

The privileged workflow checks out only the trusted default branch with no persisted
credentials. It reads the trusted manifest and GitHub metadata, never PR code or
artifacts. One repository concurrency group serializes all reconciliations.
AGY owns only todo → doing; after submission CI alone may set done/todo. A fresh
review checks the same current head after qualification; no automatic merge.

## Staged, blocked rollout

The manifest inventories every existing ordinary PR CI job and all four exact GKE
job identities (including nested reusable-workflow names). GKE names were verified
from a historical Actions run, not inferred from green checks. All configured
workflows/jobs must succeed for the current PR head and latest run attempt. Missing,
skipped, cancelled and pending gates never pass. Failures retain exact run/job links;
the controller journals/deduplicates handoffs and preserves other issue labels.

The explicit `lifecycle_mode: failure-only` permits a genuine failure of any
configured current-head gate to return an open `doing` issue to `todo`, with
concise failure and exact run/job links. Other labels and existing `done` or
unclaimed `todo` issues are preserved. Success, missing, pending, cancelled or
skipped evidence produces no issue mutation. All-green evidence still cannot
produce `done`, even if someone accidentally clears blockers. The policy guard
imports the shared `load_config` and `evaluate_policy` APIs, refuses old controllers
and full activation, and requires an explicit nonempty blockers list.

The nonempty `qualification_blockers` retain the incomplete qualification decision.
The current `relay-ci` runs format/lint, workspace unit/integration tests and
release-contract checks. Path-filtered GKE covers only one e2e suite; it does not
prove all affected functional/e2e or performance qualification. No automatic
measured performance gate exists. Wiring-only PRs do not trigger GKE. No missing
coverage is treated as passing and no existing test triggers are expanded.

Resolving blockers requires a separately reviewed policy/coverage decision; do not
clear them merely because the inventoried jobs are green. Product releases still
need their complete declared functional/performance qualification. No release or
manual dispatch is part of this change.

Managed PRs must use `issue/N` or `agy/issue-N` and an exact `Closes #N` line.
This installation PR uses a maintenance branch and is deliberately unmanaged.
The default-branch workflow only becomes available after review and merge; this
PR does not assert that deployed lifecycle handoff already ran.

Run only affected offline tests locally:

```sh
python3 -B -m unittest discover -s scripts/issue-ci -p 'test_*.py'
```
