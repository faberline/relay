// HANDWRITE-BEGIN gap="missing-generator:logic:dc1ec053" tracker="pending-tracker" reason="impl ManagedService for Relay: MANAGER relay-operator (SSA field manager + leader-election Lease name); render() -> render::render; readiness_targets = [StatefulSet {name}]; status_patch = Pending|Reconciling|Ready from readyReplicas vs desiredReplicas (= replicasPerShard, shard pinned 1) + observedGeneration + message; pub async fn run() = service_k8s::run::<Relay>()."
//! relay's operator wiring onto the shared `core/service-k8s` controller.
//!
//! The reconcile loop + leader-election lease live in `core/service-k8s`
//! (`service_k8s::run` drives the watch + leader-gated server-side apply over
//! kube-rs). relay supplies only its [`ManagedService`] impl — what to render,
//! which workload to poll for readiness, and the `Relay` status subresource to
//! write.

use std::future::Future;

use kube::{Client, ResourceExt};
use serde_json::json;
use service_k8s::service::ReconcilePlan;
use service_k8s::{ManagedService, ReadinessTarget, ReadyFacts};

use super::crd::Relay;
use super::render;

impl ManagedService for Relay {
    /// Server-side-apply field manager + leader-election Lease name.
    const MANAGER: &'static str = "relay-operator";

    fn render(&self) -> Vec<serde_json::Value> {
        render::render(self)
    }

    /// Rejects a spec that the render cannot express (see `render::validate`)
    /// before anything is rendered or applied.
    fn reconcile_plan(
        &self,
        _client: Client,
    ) -> impl Future<Output = anyhow::Result<ReconcilePlan>> + Send {
        let plan = match render::validate(self) {
            Ok(()) => Ok(ReconcilePlan::new(self.render(), serde_json::Value::Null)),
            Err(reason) => Err(anyhow::anyhow!("invalid Relay spec: {reason}")),
        };
        async move { plan }
    }

    fn readiness_targets(&self) -> Vec<ReadinessTarget> {
        // relay is always a StatefulSet (durable log + raft state on a PVC);
        // poll it for `.status.readyReplicas`.
        vec![ReadinessTarget::new("StatefulSet", self.name_any())]
    }

    fn status_patch(&self, ready: &ReadyFacts) -> serde_json::Value {
        let name = self.name_any();
        let ready_replicas = ready.get(&name) as i32;
        // relay is a single raft group: shardCount is pinned to 1 by the
        // render, so replicasPerShard is the desired broker count.
        let desired = self.spec.cluster.replicas_per_shard as i32;
        let phase = if desired > 0 && ready_replicas >= desired {
            "Ready"
        } else if ready_replicas > 0 {
            "Reconciling"
        } else {
            "Pending"
        };
        json!({ "status": {
            "phase": phase,
            "observedGeneration": self.metadata.generation.unwrap_or(0),
            "readyReplicas": ready_replicas,
            "desiredReplicas": desired,
            "message": format!("{ready_replicas}/{desired} broker pods ready"),
        }})
    }
}

/// `relay k8s operator run` — run the reconcile controller on the shared
/// `core/service-k8s` host (leader-gated; safe at `replicas > 1`).
pub async fn run() -> anyhow::Result<()> {
    service_k8s::run::<Relay>().await
}
// HANDWRITE-END
