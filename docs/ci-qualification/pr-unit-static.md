# Pull request unit and static checks

The candidate workflow inventories the complete Cargo workspace from the exact
pull request head and base. It runs every enabled library and binary unit target
for each declared feature profile. It compares each nonempty execution roster
with the harness discovery lists before and after the run. A release run for
ignored tests must also include every debug test name and match its own lists.
The planner checks both checkouts for tracked and non-ignored untracked changes
before and after metadata generation and product commands. A detected change
fails the job. This does not prove that files cannot change and be restored
during a run.
Optional dependency feature activation selects the corresponding required
binary; a weak dependency feature alone does not. It also runs workspace
formatting and all-target Clippy.

These are ordinary read-only PR checks. They do not approve their own policy or
replace existing Relay CI, GKE, functional, E2E or performance evidence. The
failure-only policy and its two existing blockers remain in force. Missing
product test content and product acceptance tickets belong to the issue planner.

The candidate pins the independently reviewed AW #112 lifecycle controller.
Review fail or block returns the linked open issue to todo. Automatic closure
after merge still requires a real provider verification.
