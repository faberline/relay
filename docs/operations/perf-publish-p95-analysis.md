# Publish p95 of the durable gate: Linux numbers and the fsync path

## Question

On a Mac, `measured_durable_lifecycle_gate` shows a publish p95 of 1101884 us against the 500000 us limit (#189). Is the cause in relay, or does the limit not fit that machine?

## Linux numbers

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

Host: `Linux 6.18.44-fc-v470 x86_64`, `nproc` 4, the temp directory is on `ext4` (`/dev/vda`).

CI does not record this number: `.github/workflows/ci.yml` runs no `--ignored` test, so no `gate` or perf job log on `main` has a `relay_perf_gate` line. The Linux numbers above come from local runs only.

## The fsync path

- The gate sets `FsyncPolicy::Always` (`crates/relay/tests/it/measured_performance.rs`, line 62), so every batch pays one full sync.
- The gate publishes 2000 messages of 128 bytes in batches of 100 (20 batches). `MAX_BATCH_P95_US = 500_000` is on line 18. The p95 is the 19th of 20 sorted batch times.
- `append_many` (`crates/relay-storage/src/log.rs`) writes every frame, then calls `fsync_active()` once per batch unless the policy is `Os`. So a p95 of 1.1 s means one sync took about 1.1 s, or the machine stalled. It does not mean relay issues more than one sync per batch.
- `fsync_active` calls `writer.sync()` of `storage_durable::FramedLogWriter`. In faberline/core, `crates/storage-durable/src/framed_log/sync_plan.rs`, `FramedLogSyncPlan::sync_off_lock` calls `sync_all()` and then `sync_parent_dir`. On macOS, Rust's `File::sync_all` uses `fcntl(F_FULLFSYNC)`, which asks the drive to flush its own cache. That is much slower than Linux `fsync`, and its latency depends on the drive and on other disk load.
- `FsyncPolicy::Interval` only flushes the userspace buffer, and `Os` does nothing (`log.rs`, `append_at`). Neither pays the full-sync cost.
- The Darwin arm64 calibration of 2026-07-20 (`docs/operations/performance.md`) recorded a publish batch p95 of 18872 us with the same policy.

## Can F_FULLFSYNC explain 1.1 s?

It can explain a publish p95 much slower than Linux: `F_FULLFSYNC` waits for the drive cache and can stall under disk load. Code reading cannot prove it. The 2026-07-20 calibration shows the same code passed on a Mac once, so the Mac result has one of three causes: disk load or drive state at the time of the run, or a change in the code after 2026-07-20. Only the Mac bisect of #189 step 1 can tell which.

## Evidence that would confirm it

1. Run the same gate on the Mac with `FsyncPolicy::Always` changed to `Interval` in `measured_performance.rs` line 62, locally and not committed. If the p95 falls to Linux levels, the cost is the sync.
2. During a run, use `sudo fs_usage -w -f filesys <pid>` or `iostat -d 1` to see the `F_FULLFSYNC` time and the disk queue.
3. Run the gate three times with the Mac idle and on AC power, and compare with the load of the failing run (1-min load 2.31 in #189).
4. Check out the commit of 2026-07-20 and run the gate again, to see whether the code changed the number.

## What this report does not change

The 500000 us limit, `vat.toml` and the acceptance of #30 stay as they are. The Mac bisect stays in #189.
