# Formal Claude review and PR completion

The default branch now declares the formal read-only Claude producer and one CI/outcome writer.
The writer replaces the previous issue-ci workflow. Each wake reconciles both queues.

- Complete trusted qualification is required before paid final review.
- A current-head PASS plus all required evidence enables native squash auto-merge.
- GitHub closes the one Closes #N issue only after merging to the default branch.
- A valid fail/block returns the linked open issue to doing and preserves unrelated labels.
- Stale, missing, skipped or unknown evidence cannot approve merge.

The existing required CI jobs, qualification mode and coverage blockers are unchanged.
The exact previous CI policy is retained for handoff journal continuity.
Current coverage holds still prevent final review and automatic merge.

Shared source: faberline/aw@28637704225692e5a1766b9c6b9778ef2b52e155 (AW #92).
Formal workflow ID: 399790575.
Unified writer ID: 394215454.
Native protection ruleset: 26069041.

Retained blockers:

- No automatically enforced affected performance PR qualification gate exists.
- The existing path-filtered GKE suite is partial e2e coverage; affected functional/e2e selection and docs-only applicability require a reviewed policy. Missing or skipped GKE runs cannot qualify done.
