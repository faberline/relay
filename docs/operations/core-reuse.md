# Core reuse (S4)

On 2026-11-14 this records the S4 core-reuse check of relay (standard `docs/standards/core-reuse.md` in faberline/aw). It compares the core pin `v0.4.14` in `Cargo.toml` with the current core release `v0.5.0`. `aw standards check` is named by the standard but is not in the Rust `aw` 0.7.33, so the rules CR0 to CR8 were checked by hand with the commands below.

| Check | Result | Evidence |
|-------|--------|----------|
| CR0 | pass | `grep -n 'core_reuse' aw.toml` printed nothing (no exceptions). |
| CR1 | pass | The 13 core crates in `Cargo.toml` (lines 32 to 44) pin only `tag = "v0.4.14"`, with no `branch`, `rev` or `path`. The `path =` entries name only crates in this repository. |
| CR2 | pass | All 13 core pins use `v0.4.14`. |
| CR3 | fail | The pin is `v0.4.14`. The current release is `v0.5.0`, the highest `vX.Y.Z` tag of faberline/core (`v0.5.0-rc.1` is a pre-release). Read with `gh api "repos/faberline/core/tags?per_page=30"`. |
| CR4 | pass | `grep -n 'faberline/core' crates/*/Cargo.toml` matched only a comment (`crates/relay/Cargo.toml:35`). Members use `<crate>.workspace = true`. |
| CR5 | pass | `crates/relay/Cargo.toml` has `[[bin]] name = "relay"` and `cli-std.workspace = true`. |
| CR6 | pass | `crates/relay/build.rs` calls `build_stamp::stamp("RELAY")`. `build-stamp.workspace = true` is under `[build-dependencies]`. `grep -n 'rustc-env' crates/relay/build.rs` printed nothing. |
| CR7 | pass | `grep -nE '^(prometheus\|openraft\|raft) *=' Cargo.toml crates/*/Cargo.toml` printed nothing. |
| CR8 | n/a | Relay is not `core`. |

## Findings

| Finding | File | Core function to use instead |
|---------|------|------------------------------|
| CR3: the core pin `v0.4.14` is behind the current release `v0.5.0` | `Cargo.toml` lines 32 to 44 | None. The pin move to `v0.5.0` is out of scope for this issue (see #137). |

Duplicate-code scan (step 4):

- `fsync_active` in `crates/relay-storage/src/log.rs:480` only calls `FramedLogWriter::sync()` from `storage-durable`. It is a thin call, not a finding.
- `encode_frame` in `crates/relay-queue/src/wire.rs:163` and `crates/relay-queue/src/consume.rs:50` writes length-prefixed wire frames (CBOR and JSON). Core's `storage-durable` frame writer is private and writes a different on-disk format (seq and CRC header). It does not do the same job, so it is not a finding.
- No hit for `Metrics`, `Counter`, `Histogram` or `Gauge` structs, for `shutdown_signal` or `wait_for_sigterm`, or for `peer_tls`, `load_pem` or `client_config`.

Result: one finding (CR3).

## Re-run

```bash
aw standards check --core ~/faberlines/core --repo ~/faberlines/relay
```

Use this command in place of the hand check once `aw` has the `standards` subcommand.
