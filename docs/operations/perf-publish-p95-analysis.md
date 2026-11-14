# Publish p95 of the durable gate: Linux numbers and the fsync path

## Question

On a Mac, `measured_durable_lifecycle_gate` showed a publish p95 of 1101884 us against the 500000 us limit (#189). Is the cause in relay, or in the machine?

## Answer

The cause is in relay, not in the machine. The Mac bisect in #189 (comment of 2026-11-14T12:14Z) names `a051153` (#166, the log built on core `storage_durable::FramedLogWriter`) as the first bad commit: its publish p95 was 614713 to 629021 us, while its parent `44495fe` gave 13523 to 18606 us on the same Mac. Before #198, core synced every frame under `FsyncPolicy::Always`. #198 changed the mapping so that one batch costs one sync, and the desk's Mac run of #198 gives a publish p95 of 12818 us (comment of 2026-11-14T18:30Z on #189).

## Linux numbers

### Before #198 (per-frame sync), measured on the base of PR #199 earlier in this branch

Ten runs, one after another, of `cargo test --release -p relay --test it measured_performance::measured_durable_lifecycle_gate -- --exact --ignored --nocapture`. Each line is the `relay_perf_gate` output of the run. All ten runs passed: `errors=0`, `samples=20/20`.

| Run | Publish msg/s | Lease+ack msg/s | Publish p95 (us) | Lease+ack p95 (us) |
|-----|---------------|-----------------|------------------|--------------------|
| 1 | 5660.3 | 197569.9 | 21461 | 675 |
| 2 | 4898.4 | 184246.9 | 24245 | 733 |
| 3 | 4892.6 | 226065.3 | 24619 | 715 |
| 4 | 5739.0 | 211126.4 | 20168 | 684 |
| 5 | 4381.7 | 171541.3 | 35072 | 880 |
| 6 | 5750.0 | 227092.1 | 19301 | 602 |
| 7 | 5910.1 | 228649.8 | 21384 | 781 |
| 8 | 4901.9 | 217556.8 | 22899 | 719 |
| 9 | 4988.3 | 191534.2 | 23629 | 913 |
| 10 | 5499.0 | 194269.1 | 22863 | 797 |

Publish p95 over the ten runs: min 19301 us, median 22881 us, max 35072 us.

### After #198 (one sync per batch)

Ten runs of the same command on `main` after #198 (the branch merges `main`, so `CoreFsync::Interval` is in `core_policy`). All ten passed: `errors=0`, `samples=20/20`.

| Run | Publish msg/s | Lease+ack msg/s | Publish p95 (us) | Lease+ack p95 (us) |
|-----|---------------|-----------------|------------------|--------------------|
| 1 | 174993.4 | 220726.2 | 1508 | 589 |
| 2 | 193311.4 | 224971.9 | 1054 | 728 |
| 3 | 139314.6 | 177367.9 | 1372 | 850 |
| 4 | 164921.3 | 223438.7 | 1183 | 578 |
| 5 | 215146.3 | 250783.7 | 1078 | 641 |
| 6 | 69185.0 | 196521.6 | 15670 | 781 |
| 7 | 194950.8 | 166819.6 | 1072 | 1064 |
| 8 | 177242.1 | 225886.6 | 1164 | 669 |
| 9 | 136082.2 | 156030.6 | 1463 | 884 |
| 10 | 188803.9 | 245579.6 | 1417 | 632 |

Publish p95 over the ten runs: min 1054 us, median 1277.5 us, max 15670 us. Run 6 is one outlier (publish p95 15670 us, 69185 msg/s); the cause was not investigated, and the run still passed the gate.

Host: `Linux 6.18.44-fc-v470 x86_64`, `nproc` 4, the temp directory is on `ext4` (`/dev/vda`).

Linux `fsync` is cheap enough that the wall-time gate passes even with a sync per frame (before #198 the publish p95 was about 19 to 35 ms, far below the 500000 us limit), which is why the gate did not show the regression on Linux.

CI does not record this number: `.github/workflows/ci.yml` runs no `--ignored` test, so no `gate` or perf job log on `main` has a `relay_perf_gate` line. The Linux numbers above come from local runs only.

## The fsync path

- The gate sets `FsyncPolicy::Always` (`crates/relay/tests/it/measured_performance.rs`, line 62). It publishes 2000 messages of 128 bytes in 20 batches of 100. `MAX_BATCH_P95_US = 500_000` is on line 18. The p95 is the 19th of 20 sorted batch times.
- Before #198: `core_policy` in `crates/relay-storage/src/log.rs` mapped `Always` to core `Always`. In faberline/core, `should_sync_immediately` (`crates/storage-durable/src/fsync.rs`) is true for `Always` alone, and `FramedLogWriter::append` then calls `sync()` (flush, `sync_all`, `sync_parent_dir`) after every frame. `Log::write_frame` calls `append` once per entry. So one 100-message `append_many` did about 100 full syncs, plus the one `fsync_active()`: about 101.
- Since #198: `core_policy` maps `Always` to core `Interval`, so core never syncs inside `append` (unit test `core_policy_never_syncs_inside_append` in `log.rs`). `append_at` under `Always` syncs once before it returns, and `append_many` calls `fsync_active()` once per batch. The integration tests in `crates/relay/tests/it/durable.rs` assert the count: `always_batch_of_100_does_one_sync`, `always_single_append_syncs_once_per_entry` and `interval_batch_does_one_sync_and_os_batch_none`. No entry is acknowledged before its sync.
- On macOS, Rust's `File::sync_all` uses `fcntl(F_FULLFSYNC)`, which asks the drive to flush its own cache. Linux `fsync` is much cheaper.

## Can F_FULLFSYNC explain 1.1 s?

Yes, in the sense that the number of full syncs per batch explains the Mac result. With about 100 `F_FULLFSYNC` calls per batch, at about 6 to 11 ms each on that Mac, the publish p95 comes to about 0.6 to 1.1 s. The bisect on #189 shows 614713 to 629021 us at `a051153`, and 1101884 us on the PR 90 head (`14ddf9b`). With one sync per batch, as at `44495fe` (before #166) and on the #198 head, the publish p95 is about 13 to 19 ms (`44495fe`) and 12818 us (the #198 Mac run). The same Mac passes once the sync count per batch is one. So the cause is the count of syncs, not a stalled drive or disk load.

## Evidence

1. Done: the Mac bisect of #189 (comment of 2026-11-14T12:14Z). The first bad commit is `a051153`; its parent `44495fe` passes with 13523 to 18606 us.
2. Done: the Mac run of the #198 head `625bc145` (comment of 2026-11-14T18:30Z on #189): publish p95 12818 us, the gate passes.
3. Done in code: the sync count per batch is asserted by `always_batch_of_100_does_one_sync`, so the regression cannot return unseen on Linux.

## What this report does not change

The 500000 us limit, `vat.toml` and the acceptance of #30 stay as they are. The Mac bisect was done in #189.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
