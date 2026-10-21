//! The integration tests, linked into one test binary `it`. Each file here is
//! one module, so a test is named `<file>::<test>`: run one module with
//! `cargo test -p relay --test it <file>::`. The cluster end-to-end suites
//! are under `e2e`. `tests/raft_config.rs` stays its own binary because it
//! changes process-global env vars.

mod auth;
mod backup;
mod behavior_relay_competitor_performance_behavior;
mod behavior_relay_security_hardening_auth_and_admission_behavior;
mod bounded_log;
mod cli_contract;
mod delayed_delivery;
mod deploy_cli;
mod direct_k8s_assets;
mod durable;
mod e2e;
mod efficiency_relay_competitor_performance_measured_durable_lifecycle;
mod http2_transport;
mod measured_performance;
mod multi_shard;
mod operator;
mod perf_gate;
mod priority;
mod raft_cluster;
mod raft_core;
mod raft_peer_mtls;
mod raft_persistence;
mod reconciler;
mod relay_core;
mod security_relay_security_hardening_guard_scan;
mod security_relay_security_hardening_negative_peer_and_kubernetes_boundaries;
mod segments;
mod service_admission;
mod sparse_index;
mod spec_cli;
mod stability_relay_competitor_performance_bounded_soak;
mod stability_relay_security_hardening_rotation_and_peer_stability;
mod work_queue_api;
mod work_queue_throughput;
mod worker_loop;
