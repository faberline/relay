# Upgrade and version compatibility

This page says which versions of relay can work with each other and with the
data that an older version wrote. Read it before you upgrade a cluster.

## Policy

A minor release `relay@X.(Y+1).0` reads all data written by `relay@X.Y.Z`:
the on-disk log, the raft state and snapshot of a node, and the operator
`Relay` CR.

A change that a previous minor cannot read needs a migration note in this file
and a major version bump. Patch releases never change a data format.

Downgrade is not supported. If an upgrade goes wrong, restore from a backup
(see [runbook.md](runbook.md#restore)).

## What is compatible

| Interface | Compatibility rule |
|---|---|
| HTTP/2 API (OpenAPI served at `/openapi.json`) | `same minor`. Changes are additive only inside a major version. |
| Peer raft wire | `adjacent minor`. Nodes of two adjacent minors form one group during a rolling upgrade. |
| On-disk log | `adjacent minor`. See the next section for the format. |
| Operator CR `Relay` | `adjacent minor`. New fields are optional and have a default. |

A row that says `major bump needed` means the change breaks the rule above.
Such a change needs a migration note in this file and a new major version.

## On-disk log format today

The log is stored as core `storage-durable` framed-log segments (since
2026-11-11; the format change is described in
[log-format-break.md](log-format-break.md)).

- One file per segment. The file name is `<subject>__shard<N>__<base_seq>.frames`
  (`Log::segment_path` in `crates/relay-storage/src/log.rs`).
- One frame per entry, behind a 16-byte header (seq, length, CRC32). The
  payload is one JSON object, a `LogEntry`
  (`crates/relay-shared-kernel/src/types.rs`) with these nine fields: `seq`,
  `message_id`, `subject`, `shard`, `payload`, `headers`, `appended_at`,
  `not_before`, `priority`. The fields `headers`, `not_before` and `priority`
  have serde defaults, so older lines without them still load.
- The active segment rolls at `segment_bytes`. Retention is delete-on-ack: a
  segment is removed only after every entry in it is acknowledged.

Old `.ndjson` data is not migrated. A data directory that still holds `.ndjson`
segments fails to open with a clear error, and relay neither reads nor deletes
those files. See [log-format-break.md](log-format-break.md) for the operator
steps.

## Rolling upgrade order

1. Back up the cluster first (`relay backup`, see [runbook.md](runbook.md)).
2. Change the image in the `Relay` CR and apply it (the command is in
   [runbook.md](runbook.md), section Upgrade).
3. The StatefulSet restarts one pod at a time. Wait until the pod's `/readyz`
   answers 200 before the next pod restarts
   (`kubectl -n relay rollout status statefulset/relay`).
4. Restart the leader last when possible.
5. If a pod does not become Ready, stop. Do not restart a second pod.

## Test

The test `raft_cluster::rolling_restart_keeps_all_acked_messages` restarts all
three voters one at a time on the same data directories. It checks that 20
acked messages stay, in publish order. It runs on one build. The two-build
upgrade is checked by the GKE run of issue #137.

The tests `compat::openapi_operations_are_not_removed` and
`compat::on_disk_log_format_is_stable` (in `crates/relay/tests/it/compat.rs`)
pin the served API and the on-disk log format. They fail when an operation is
removed or a log frame field changes; added paths and fields still pass.
