// SPEC-MANAGED: tech-design/logic/core-durable-log-single-multi-broadcast-delivery-model.md#logic
// HANDWRITE-BEGIN gap="missing-generator:logic:cb6414bd" tracker="pending-tracker" reason="Crate root: module wiring and public re-exports for the in-process core."
//! # relay core
//!
//! The broker server core: a single-cast **work-queue** over a durable ordered
//! log per `(subject, shard)`. Each message is leased to exactly one competing
//! consumer, acked, then reclaimed (delete-on-ack), with lease-expiry
//! redelivery, a dead-letter path, priority, and a committed offset (see
//! [`WorkQueue`]). Pure pull broker — no broadcast/replay (that is `tape`) and no
//! push/ETA dispatch (that is `defer`).
//!
//! The core is payload-agnostic (epic #120: the broker "knows nothing about
//! workflows"): a message body is an opaque [`Payload`] (JSON) plus a
//! caller-supplied [`MessageId`] for sequencing and dedupe. A producer
//! serializes whatever message type it uses into the payload. relay owns the
//! log, sequencing, dedupe, and leases, and depends on no other faberline project.
//!
//! ```
//! use std::collections::BTreeMap;
//! use chrono::Utc;
//! use relay::{Relay, RelayCoreConfig};
//!
//! let relay = Relay::new(RelayCoreConfig::in_memory());
//! let now = Utc::now();
//! let body = serde_json::json!({ "task": "greet", "args": ["hi"] });
//!
//! // Publish a task; a worker leases it exactly once and acks it.
//! let out = relay.publish("tasks", "m-1", body, BTreeMap::new(), now).unwrap();
//! assert_eq!(out.seq, 0);
//!
//! let lease = relay.lease("tasks", "worker-a", now).unwrap().unwrap();
//! assert_eq!(lease.seq, 0);
//! assert!(relay.ack("tasks", &lease.lease_id, Some(lease.epoch)).unwrap());
//! ```
//!
//! The crate is the assembly over the bounded-context crates, re-exported
//! here by name: [`storage`] is the durable log, [`broker`] is the in-process
//! work-queue core, [`replication`] is raft HA and lease reconciliation,
//! [`access`] is bearer auth, [`queue`] is the HTTP/h2c + OpenAPI surface, and
//! [`shared_kernel`] holds the shared data model and config. [`tls`] holds the
//! rustls provider and peer-mTLS adapter.

pub use relay_access as access;
pub use relay_broker as broker;
#[cfg(feature = "operator")]
pub use relay_operator as operator;
pub use relay_queue as queue;
pub use relay_replication as replication;
pub use relay_shared_kernel as shared_kernel;
pub use relay_storage as storage;

pub mod perf_gate;
pub mod tls;

pub use relay_broker::engine::{Relay, SubjectLive};
pub use relay_broker::shard::shard_for;
pub use relay_broker::workqueue::WorkQueue;
pub use relay_queue::server::{router, AppState};
pub use relay_queue::server_config::RelayServerConfig;
pub use relay_replication::raft::{
    load_snapshot_bytes, snapshot_bytes, EngineSnapshot, PubCommand, RelayCommand, RelayOutcome,
    RelayRaft, RelayStateMachine,
};
pub use relay_replication::reconciler::{
    spawn_reconciler, spawn_replicated_reconciler, ReconcilerHandle,
};
pub use relay_shared_kernel::config::{
    DedupeConfig, FsyncPolicy, RelayCoreConfig, RetentionConfig, WorkQueueConfig,
};
pub use relay_shared_kernel::types::{
    AppendOutcome, CommittedOffset, Lease, LogEntry, MessageId, Payload, Seq, ShardId, Subject,
    DEFAULT_PRIORITY,
};
pub use relay_storage::log::Log;
// HANDWRITE-END
