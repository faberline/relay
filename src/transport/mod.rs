//! The network surface: the axum router over the shared service shell, the
//! wire models, the streaming consume endpoint, OpenAPI, bearer auth, and
//! request metrics.

pub mod auth;
pub mod consume;
pub mod metrics;
pub mod openapi;
pub mod server;
pub mod server_config;
pub mod wire;
