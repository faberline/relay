//! The in-process broker core: config, the durable per-shard log, the
//! single-cast work queue, and the [`engine::Relay`] facade over them.
//! Transport- and consensus-agnostic.

pub mod config;
pub mod engine;
pub mod log;
pub mod shard;
pub mod types;
pub mod workqueue;
