//! The network surface: the axum router over the shared service shell, the
//! wire models, the streaming consume endpoint, OpenAPI, and request metrics.
//! Bearer auth comes from `relay-access`.

pub mod consume;
pub mod metrics;
pub mod openapi;
pub mod server;
pub mod server_config;
pub mod wire;
