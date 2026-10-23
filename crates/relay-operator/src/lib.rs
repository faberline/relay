// HANDWRITE-BEGIN gap="missing-generator:logic:5849f96b" tracker="pending-tracker" reason="Feature-gated operator module root: crd/render/reconcile submodules, re-exports (Relay, RelaySpec, RelayStatus, run), crd::crd_yaml() = serde_json(Relay::crd()) -> normalize_kubernetes_schema_formats (recursively rewrite format: uint32/uint64 to plain integer + minimum: 0 — Kubernetes structural-schema rules) -> serde_yaml string (keep's pattern)."
//! K8s operator for relay: a `Relay` custom resource ([`crd`]) plus a
//! reconcile loop ([`reconcile`]) that renders ([`render`]) relay's HA
//! topology — ServiceAccount, headless + client Services, PodDisruptionBudget,
//! and the downward-API StatefulSet raft-runtime consumes. Behind the `operator`
//! feature so the serving image never links kube-rs.
//!
//! ```text
//! Relay (relay.dev/v1alpha1)  --reconcile-->  ServiceAccount, StatefulSet,
//!                                             headless + client Service,
//!                                             PodDisruptionBudget
//! ```

pub mod crd;
pub mod reconcile;
pub mod render;

pub use crd::{crd_yaml, Relay, RelaySpec, RelayStatus};
pub use reconcile::run;
// HANDWRITE-END
