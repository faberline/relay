# Configuration reference

The server is the bare `relay` command, with no subcommand. Each flag reads its
environment variable when the flag is not given, and a flag overrides the variable.

## Flags and their variables

| Flag | Environment variable | Default | Meaning |
|------|----------------------|---------|---------|
| `--bind` | `RELAY_BIND` | `0.0.0.0:7000` | h2c listen address for this shard. |
| `--data-dir` | `RELAY_DATA_DIR` | none | Durable log directory. Defaults to the core config's data dir. |
| `--grace-secs` | `RELAY_GRACE_SECS` | `10` | Graceful-drain window in seconds after SIGTERM, while `/readyz` reports 503. |
| `--log-format` | `RELAY_LOG_FORMAT` | `pretty` | Log output format: `pretty` or `json`. Kubernetes uses `json`. |
| `--otlp-endpoint` | `RELAY_OTLP_ENDPOINT` | none | OTLP/gRPC trace collector endpoint. Needs a build with `--features otel`. |
| `--auth` | `RELAY_AUTH` | `off` | Request-auth mode for the `/v1` data plane: `off` or `required`. Probes stay tokenless. |
| `--token-registry-file` | `RELAY_TOKEN_REGISTRY_FILE` | none | Bearer-token registry file (JSON). Required and validated at startup with `--auth required`. |
| `--peer-service` | `RELAY_PEER_SERVICE` | `relay` | Headless Service name for raft peer DNS in replica mode. |
| `--raft-port` | `RELAY_RAFT_PORT` | `7001` | Port of the dedicated authenticated Raft listener when `RELAY_PEER_MTLS=on`. |
| `--body-limit-bytes` | `RELAY_BODY_LIMIT_BYTES` | `8388608` | Request body size limit in bytes. Larger requests get 413. |

## Environment-only variables

| Variable | Default | Meaning |
|----------|---------|---------|
| `RELAY_TLS_CERT`, `RELAY_TLS_KEY` | none | PEM file paths. Both set turns TLS on for the public listener. Exactly one set is a startup error. Source: `crates/relay/src/tls/public.rs`. |
| `RELAY_PEER_MTLS` | off | `on` requires client certificates on the raft peer listener. Source: `crates/relay/src/tls.rs`. |
| `RELAY_TOKENS` | none | Legacy token list as JSON, loaded through the same registry loader as `RELAY_TOKEN_REGISTRY_FILE`. Source: `crates/relay-access/src/auth.rs` (`LEGACY_TOKENS_ENV`). |
| `RELAY_ADMISSION_READ_CAPACITY`, `RELAY_ADMISSION_WRITE_CAPACITY`, `RELAY_ADMISSION_ADMIN_CAPACITY` | none | Positive integers, the requests allowed per key in one refill window for each route class. Any one of them enables admission. |
| `RELAY_ADMISSION_REFILL_SECS` | `60` | Length of the refill window in seconds. An error unless a capacity is set. |
| `RELAY_ADMISSION_MAX_KEYS` | `1024` | Most keys that admission tracks at once. An error unless a capacity is set. |

The admission variables are parsed by `AdmissionConfig::from_lookup` in
faberline/core, in the `service-http` crate, file `admission_config.rs`, at the
tag pinned in the root `Cargo.toml`.

## HA topology variables

Setting `REPLICAS_PER_SHARD` greater than `1` turns on replica mode. The other
variables come with it, from the StatefulSet's downward API or from the operator.
Source: `crates/relay/src/bin/relay/llm.rs`, the text near `REPLICAS_PER_SHARD`.

| Variable | Meaning |
|----------|---------|
| `REPLICAS_PER_SHARD` | Replicas in the shard. Greater than `1` turns on replica mode. |
| `POD_NAME` | The pod's name, from the downward API. |
| `SHARD_COUNT` | Number of shards. Set to `1`. |
| `VOTER_COUNT` | Number of raft voters. |

## Example

```bash
relay --bind 0.0.0.0:7000 --data-dir /var/lib/relay --auth required --token-registry-file /etc/relay/tokens.json
```
