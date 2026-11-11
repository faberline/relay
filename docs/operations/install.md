# Install and quickstart

This page installs relay, runs three replicas on Kubernetes, and shows a first
publish and consume. For day-to-day operator steps, see the
[runbook](runbook.md).

## Install

### Container image

The release image is `ghcr.io/faberline/relay:<version>`, the name that
`.github/workflows/relay-release.yml` pushes. Replace `<version>` with the
release you want. The first tagged image comes with issue #137; until then,
use the source build below.

### From source

```bash
./build.sh release
~/.cargo/bin/relay --version
```

`./build.sh release` builds with `--release --locked` and installs the binary
to `~/.cargo/bin/relay`. It also pins the image tag in
`k8s/base/statefulset.yaml`, `k8s/operator/deployment.yaml` and
`Dockerfile.release` to the version in `Cargo.toml`. Those are file changes:
review them before you commit.

For a debug build, run `./build.sh debug`.

## Quickstart: three replicas on Kubernetes

The operator path is the production topology. `docs/ha.md` names the operator
custom resource (CR) as the authoritative HA layout. Run the steps in order,
with `kubectl` pointed at the target cluster.

1. Install the CRD:

   ```bash
   relay k8s crd render | kubectl apply -f -
   ```

2. Install the operator into its own namespace:

   ```bash
   relay k8s operator render --namespace relay-system | kubectl apply -f -
   ```

3. Create the app namespace and the token Secret. With auth on, the broker
   reads its bearer tokens from this Secret. The key is `token-registry.json`,
   the same name that `k8s/overlays/prod/kustomization.yaml` uses. The file
   holds JSON of the form `{token: {subject, roles}}`.

   ```bash
   kubectl create namespace relay
   kubectl -n relay create secret generic relay-token-registry \
     --from-file=token-registry.json=<path to token-registry.json>
   ```

4. Declare the instance. The `prod` profile is the three-replica raft-HA CR
   with auth on. Its default image is `registry.example.com/relay:0.4.13`, a
   placeholder, so pass the image you pushed with `--image`. Keep the default
   name `relay`: HA instances must use it.

   ```bash
   relay k8s instance render --profile prod --namespace relay \
     --image ghcr.io/faberline/relay:<version> | kubectl apply -f -
   ```

5. Check the pods. Expect three pods, `relay-0`, `relay-1` and `relay-2`, to
   reach `Ready`:

   ```bash
   kubectl -n relay get pods -l app.kubernetes.io/instance=relay
   ```

   The operator labels its pods `app.kubernetes.io/instance=relay`. The
   kustomize manifests in `k8s/base` use `app=relay` instead.

6. Port-forward the service and check readiness. Expect HTTP `200`. The probes
   stay tokenless.

   ```bash
   kubectl -n relay port-forward svc/relay 7000:7000
   ```

   In a second shell:

   ```bash
   curl -s -o /dev/null -w '%{http_code}\n' localhost:7000/readyz
   ```

The public listener is plain h2c. TLS on the public API does not exist yet
(#136), so keep the service inside the cluster or behind a TLS-terminating
proxy.

For a single pod with no HA, `kubectl apply -k k8s/overlays/dev` runs the
dev overlay. Use it only to try relay out.

## First publish and consume

This example runs a node locally with auth off, so no token is needed. Start
it with a fresh data directory:

```bash
relay --bind 127.0.0.1:7000 --data-dir "$(mktemp -d)"
```

Publish one message. `message_id` is the idempotency key: a repeat returns
`"deduped":true` instead of a second entry.

```bash
curl -s -X POST localhost:7000/v1/jobs/publish \
  -H 'content-type: application/json' \
  -d '{"message_id":"m-1","payload":{"n":"m-1"}}'
```

Expect HTTP `200` and `{"seq":0,"deduped":false}` on a fresh data directory.

Lease the next entry. `/lease` is the polling form and is still served; the
current worker path is the streaming `/consume` endpoint. See
[worker-protocol.md](../worker-protocol.md) for it and the rest of the
contract.

```bash
curl -s -X POST localhost:7000/v1/jobs/lease \
  -H 'content-type: application/json' \
  -d '{"consumer_id":"worker-1"}'
```

Expect HTTP `200` with a `lease` object and an `entry` object. Keep
`lease.lease_id` and `lease.epoch` for the ack.

Ack the lease. Pass the `lease_id` and `epoch` from the lease:

```bash
curl -s -X POST localhost:7000/v1/jobs/ack \
  -H 'content-type: application/json' \
  -d '{"lease_id":"<lease_id>","epoch":<epoch>}'
```

Expect `{"acked":true,"committed_seq":0}`. Ack the same lease again and expect
`"acked":false`.

With auth on, add `-H 'authorization: Bearer <token>'` to each request.
