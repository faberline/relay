# Performance

The recorded numbers for relay: the gate thresholds, measured throughput and
latency, resource use, the soak, and the SBOM. This page copies results that
are already recorded. It runs no gate. A new run replaces these numbers only
after it is recorded with its date, host and run id.

## Gate thresholds

The production gate fails when a phase is below the minimum throughput or above
the maximum batch p95. The floor detects catastrophic regressions; it is not a
performance claim. Neither value is lowered anywhere.

| Phase | Minimum throughput | Maximum batch p95 |
|---|---:|---:|
| durable publish | 500 messages/s | 500,000 us |
| durable lease + ack | 500 messages/s | 500,000 us |

Source: `docs/perf-gate.md`, section "Required local production envelope".

## Measured throughput and latency

### Gate calibration (2026-07-20, Darwin arm64, 2,000 x 128-byte messages, batch 100, fsync always)

| Phase | Throughput | Batch p95 |
|---|---:|---:|
| publish | 12,156.4 messages/s | 18,872 us |
| lease + ack | 15,912.3 messages/s | 9,816 us |

Source: `docs/perf-gate.md`, paragraph "The 2026-07-20 Darwin arm64 release calibration". Each phase has 20 samples, with zero errors.

### Release-local calibration (2026-07-17, Darwin arm64, 100,000 x 128-byte messages, batch 1,000, concurrency 10)

| Backend | Publish msg/s | Publish p50 / p95 / p99 | Lease+ack msg/s | Lease+ack p50 / p95 / p99 | Full lifecycle msg/s |
|---|---:|---:|---:|---:|---:|
| Relay | 141,985 | 69,956 / 77,057 / 77,329 us | 97,617 | 99,319 / 123,332 / 138,562 us | 57,847 |
| RabbitMQ quorum | 67,947 | 127,139 / 171,234 / 334,211 us | 48,486 | 175,983 / 203,625 / 291,586 us | 28,295 |
| NATS JetStream | 64,402 | 148,944 / 215,615 / 219,289 us | 75,642 | 100,330 / 108,878 / 108,931 us | 34,785 |

Source: `docs/perf-gate.md`, section "2026-07-17 release-local calibration".

At batch size 100, Relay did not beat either broker in the same calibration.

## Resource use

| Backend | CPU (client+server) | Client max RSS | Server RSS move | Retained durable bytes |
|---|---:|---:|---:|---:|
| Relay | 3.22 s | 30,605,312 bytes | 203,712->282,784 KiB | 33,280,000 bytes |
| RabbitMQ quorum | 15.11 s | 126,042,112 bytes | 341,092->481,944 KiB | 48,553,984 bytes |
| NATS JetStream | 7.43 s | 10,141,696 bytes | 85,792->109,616 KiB | 1,146,880 bytes |

Source: `docs/perf-gate.md`, paragraph "Resource evidence was captured around the same run". Retained bytes are footprint amplification after ack, not device-write amplification.

## Soak

Soak report (`scripts/soak.sh`, two 30 s steady windows):

| Field | Value |
|---|---|
| total_ops | 10404 |
| errors | 0 |
| rss_window_a | 14724 KB |
| rss_window_b | 14736 KB |
| steady_drift | 0% (window A to window B) |
| p99_window | 1 ms to 1 ms |

Source: `docs/operations/restart-failover-soak.md`, section "Output summary".

## SBOM

Each release candidate builds one SPDX JSON SBOM per platform with `anchore/sbom-action`, in the steps `Generate SPDX SBOM for linux/amd64` and `Generate SPDX SBOM for linux/arm64` of `.github/workflows/relay-release-candidate.yml`. Both files are attested with `actions/attest` and recorded with their SHA-256 in the candidate manifest (`sboms`). The step `Create exact GitHub Release with candidate bytes` in `.github/workflows/relay-release.yml` attaches both files to the GitHub Release.

## How to reproduce

```bash
vat run meter-perf
RELAY_SOAK_AUTOSTART=1 bash scripts/soak.sh
```

`vat run meter-perf` is the throughput ratchet in the root README.
