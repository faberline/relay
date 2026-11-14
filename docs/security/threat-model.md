# Threat model

This page lists what relay protects, where data crosses a trust boundary, the
threats at each boundary, and the control or test for each one. The configuration
values are in [configuration.md](../operations/configuration.md), and the limits
are in [limits.md](../operations/limits.md).

## Assets

- Queued messages on disk, in the data directory set by `RELAY_DATA_DIR`.
- The token registry, which maps each bearer token to a subject and its roles.
- The TLS private keys for the public listener and for the peer listener.

## Trust boundaries

- Client to data plane: clients reach the `/v1` API on port 7000.
- Peer to peer: raft replicas talk to each other on port 7001.
- Operator to Kubernetes API: the operator applies manifests and reads Secrets.

## Threats

| Threat | Boundary | Control | Evidence |
|--------|----------|---------|----------|
| Unauthenticated data-plane access | Client to data plane | `--auth required` (`RELAY_AUTH`) with bearer tokens from `RELAY_TOKEN_REGISTRY_FILE`. Probes stay tokenless. | `crates/relay-access/src/auth.rs`, `crates/relay/tests/it/auth.rs` |
| Plain-text peer traffic | Peer to peer | `RELAY_PEER_MTLS=on` and the Raft listener on `--raft-port` (default 7001). | `crates/relay/src/tls.rs` |
| Plain-text client traffic | Client to data plane | `RELAY_TLS_CERT` and `RELAY_TLS_KEY`, both set or neither. | `crates/relay/src/tls/public.rs` |
| Oversized request body (memory exhaustion) | Client to data plane | `--body-limit-bytes` (default 8 MiB), answered with 413. | `crates/relay/src/bin/relay/serve.rs` |
| Request flooding | Client to data plane | `RELAY_ADMISSION_*` token buckets per key. | `crates/relay/src/bin/relay/serve.rs` |
| Stolen or leaked token | Operator to Kubernetes API | The token registry is mounted from the Secret `relay-token-registry`. Rotate by replacing the file. | `k8s/overlays/prod/kustomization.yaml` |
| Data loss on a node | Peer to peer | Raft replication and `relay backup`. | `docs/ha.md`, `docs/operations/runbook.md` |

## Out of scope

The model does not cover the cluster network, the node operating system, the
container runtime, or the Kubernetes control plane. It also does not cover
the client's own handling of its tokens.
