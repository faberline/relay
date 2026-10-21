# Glossary

The terms relay's code, API and docs use.

| Term | Meaning |
|------|---------|
| **subject** | A named queue. Producers publish to a subject and workers lease from it. |
| **shard** | One ordered log of a subject. `shard_for` maps a message to its shard. |
| **seq** | The monotonic, gap-free position of an entry inside one `(subject, shard)`. |
| **message id** | The caller-supplied id of a message. A retry with the same id is deduped to the same entry. |
| **payload** | The opaque JSON body of a message. relay stores it verbatim and never reads it. |
| **log entry** | One durable record in the ordered log: seq, message id, payload, headers, priority, and `not_before`. |
| **publish** | Append one entry to a subject. The result is an `AppendOutcome`: the seq and whether it was deduped. |
| **lease** | A grant of one entry to exactly one worker until it acks or the lease expires. |
| **lease id** | The unique id of one lease. A worker needs it to ack. |
| **epoch** | The fencing token of one entry. It goes up on each (re)lease. An ack or heartbeat with a stale epoch does nothing. |
| **executor node** | The replica that the committed assignment allows to serve a lease. Standalone mode uses node 0. |
| **heartbeat** | A worker extends its lease before it expires. |
| **ack** | A worker reports that it finished an entry. The entry is then reclaimed. |
| **nack / release** | A worker gives an entry back before the lease expires, so it can be redelivered. |
| **redelivery** | A leased entry that expired or was released becomes leasable again, with backoff. |
| **attempt** | The 1-based delivery count of an entry. It drives retry and dead-lettering. |
| **dead letter (DLQ)** | After `max_attempts` an entry moves to the sibling subject `{subject}.dlq`. |
| **priority** | An unsigned byte. `0` is lowest, `255` is highest, the default is `10`. Higher leases first among visible entries. Shared with defer. |
| **delayed visibility** | `not_before`: the entry is durable at once but not leasable until that time. Long-horizon ETA belongs to defer. |
| **committed offset** | The highest seq such that every entry up to it is acked. |
| **delete-on-ack** | A log segment is reclaimed once the committed offset passes it. Storage tracks backlog, not throughput. |
| **auto-mode** | A single node serves without raft. Raft HA turns on when `REPLICAS_PER_SHARD > 1`. |
| **applied floor** | The fsynced raft applied index. Replay skips entries at or below it, so acked work does not come back. |
| **reconciler** | The loop that reclaims expired leases through a committed command. |
| **h2c** | HTTP/2 over cleartext TCP: relay's worker transport. |
| **perf gate** | The ratchet that fails the build when relay loses ground against its competitors. See [operations](operations/README.md). |
