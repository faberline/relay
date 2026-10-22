//! The in-process broker core: the single-cast work queue over the durable
//! log, shard routing, and the [`engine::Relay`] facade over them.
//! Transport- and consensus-agnostic.

pub mod engine;
pub mod shard;
pub mod workqueue;
