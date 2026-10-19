//! Cluster behavior: the raft state machine for auto-mode HA, the lease
//! reconciler, and (feature `backup`) snapshot shipping.

#[cfg(feature = "backup")]
pub mod backup;
pub mod raft;
pub mod reconciler;
