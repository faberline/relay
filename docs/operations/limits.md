# Limits

Each value below is the one in the code or the manifest named in its Source
column. The configuration variables are in [configuration.md](configuration.md).

| Limit | Value | Source |
|-------|-------|--------|
| Request body size | 8 MiB default (`8388608` bytes), set by `--body-limit-bytes`. Larger requests get 413. | `crates/relay/src/bin/relay/serve.rs` |
| Consumer lease retry backoff | From 50 ms up to 1000 ms. | `crates/relay-queue/src/consume.rs` (`MIN_LEASE_BACKOFF`, `MAX_LEASE_BACKOFF`) |
| Raft snapshot interval | Every 1024 applied entries. | `crates/relay-replication/src/raft.rs` (`SNAPSHOT_EVERY`) |
| Priority bands | 256 bands, priorities 0 to 255. | `crates/relay-broker/src/workqueue.rs` (`PRIORITY_BANDS`) |
| Log frame header | 16 bytes. | `crates/relay-storage/src/log.rs` (`FRAME_HEADER_BYTES`) |
| Replicas per shard | 1 in the base manifest. The three-voter HA example uses 3 voters. | `k8s/base/statefulset.yaml`, `docs/ha.md` |
| Data volume | 10Gi requested in the base manifest. | `k8s/base/statefulset.yaml` |
| Request rates | Unlimited unless `RELAY_ADMISSION_*` is set. When set, the capacity is per key per refill window (default 60 s), with at most 1024 keys. | `crates/relay/src/bin/relay/serve.rs`; faberline/core `service-http` `admission_config.rs` (see configuration.md) |

## Not limited today

- Message count: no limit in the code. It is bounded by the data volume.
- Total data size: no limit in the code. It is bounded by the data volume.
