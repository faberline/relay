# CI lifecycle handoff

`handoff.py` is vendored unchanged from faberline/aw commit `84e9b72b4a63cc30b47449a2e321981525fa9c34`,
`plugins/faberline/skills/issue-ci/scripts/handoff.py`. SHA-256: `1abb5570032eaf294a50c5fa0bfb9ae7edd1db7e3f0a98a1f56601720916caf1`.

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

`qualification_blockers` deliberately prevents **all lifecycle mutations** until a
reviewed affected unit/functional/e2e/performance contract and docs-only applicability
policy is approved. The current `relay-ci` runs format/lint, workspace unit/integration
tests and release-contract checks. GKE `raft_failover` is path filtered and only one e2e
suite. It does not prove all functional/e2e or performance qualification; no automatic
measured performance gate exists. A wiring-only PR does not trigger this GKE workflow.
No missing coverage is treated as passing. No existing test triggers are expanded.

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
