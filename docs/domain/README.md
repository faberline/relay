# Domain

relay is a single-cast pull work-queue. A producer publishes a task, one
worker leases it, runs it, and acks it. The broker then reclaims the entry.
Payloads stay opaque, so relay never learns the caller's domain model. Terms
are defined in the [glossary](../glossary.md).

## What each part owns

| Part | Owns | Does not own |
|------|------|--------------|
| `broker` | The durable ordered log per `(subject, shard)`, dedupe, leases, epochs, redelivery, dead-lettering, priority, delayed visibility, and the committed offset. | Replication, the network surface, the payload schema. |
| `replication` | Agreement between replicas: every publish, lease, ack, release, heartbeat and reconcile is a committed raft command. Snapshots, the applied floor, and backups. | Queue rules. It applies the broker's transitions in the same order on every replica. |
| `transport` | The HTTP/h2c and OpenAPI contract that workers and producers use, bearer auth per subject, and metrics. | Queue state. It calls the broker directly or through replication. |
| `operator` | The `Relay` custom resource and the Kubernetes objects it renders. | The serving process. |

## Invariants

- One entry has at most one live lease. The lease names its executor node and
  epoch, and a stale ack is refused.
- An entry is never deleted before it is acked. Retention follows the
  committed offset.
- Delivery is at-least-once. A worker may finish while its result is lost, and
  lease expiry redelivers on purpose. Workers must be idempotent.
- Priority orders only entries that are already visible. It never makes a
  delayed entry visible early.

The worker-facing contract is [worker-protocol.md](../worker-protocol.md).
Replication details are in [ha.md](../ha.md).
