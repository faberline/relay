# Relay runbook

Operator steps for a running relay. The commands use the namespace `relay` and
the instance `relay` from the [install guide](install.md).

The node serves `GET /healthz`, `GET /readyz` (503 once draining) and
`GET /metrics` on port 7000. The request counters are
`relay_publish_requests_total`, `relay_lease_requests_total`,
`relay_ack_requests_total`, `relay_consume_requests_total`,
`relay_publish_batch_requests_total` and `relay_other_requests_total`, each with
its `_latency_ms_count` and `_latency_ms_sum` pair. There is no disk or leader
metric today: use `kubectl` and `/readyz` for those.

## Node down

- Symptom: a pod is not `Ready`, or `kubectl -n relay get pods` shows it restarting.
- Check: `kubectl -n relay describe pod relay-0` shows the events, and
  `kubectl -n relay logs relay-0 --previous` shows the last run. The data
  directory `/data` is a PVC from `volumeClaimTemplates`, so state survives a
  restart. `k8s/base/pdb.yaml` limits voluntary disruptions.
- Command: `kubectl -n relay get pods -l app.kubernetes.io/instance=relay`. The
  StatefulSet restarts the pod on its own; do not delete the PVC.
- Expected: the pod returns to `Ready`, and the other two replicas keep serving.

## Leader failover

- Symptom: writes fail for a short time after a pod is lost. A leader change
  is normal; it does not lose committed entries.
- Check: publish through the service and expect HTTP `200`. Use a fresh
  `message_id`, because a repeat is deduped. Run it with the port-forward from
  [install.md](install.md) and add `-H 'authorization: Bearer <token>'` when
  auth is required:

  ```bash
  curl -s -o /dev/null -w '%{http_code}\n' -X POST localhost:7000/v1/jobs/publish \
    -H 'content-type: application/json' \
    -d '{"message_id":"check-1","payload":{"n":1}}'
  ```

- Command: `kubectl -n relay delete pod relay-0`. There is no leader metric
  today, so to test failover delete a pod and run the check above.
- Expected: the StatefulSet recreates the pod, the group elects a leader, and
  the check returns `200`. Committed entries are kept (`docs/ha.md`; the e2e
  test is `crates/relay/tests/it/e2e/raft_failover.rs`).

## Disk full

- Symptom: writes fail, and `kubectl -n relay get pvc` shows the data volume
  near its capacity.
- Check: `kubectl -n relay get pvc` and `kubectl -n relay logs relay-0 --tail=100`.
  There is no disk alert or disk metric today; watch the PVC by hand.
- Command: if the storage class allows expansion,
  `kubectl -n relay patch pvc data-relay-0 -p '{"spec":{"resources":{"requests":{"storage":"<new size>"}}}}'`.
  The prod overlay sets `100Gi` in `k8s/overlays/prod/kustomization.yaml`, and
  the prod CR sets `storage: 100Gi`. If the storage class does not allow
  expansion, there is no in-place fix today; see [Restore](#restore).
- Expected: the PVC capacity grows and writes succeed again. Repeat the
  publish check from [Leader failover](#leader-failover).

## Backup

- Symptom: no new backup object appears under the destination, or the operator's
  backup CronJob failed.
- Check: `kubectl -n relay get cronjob` and `kubectl -n relay get jobs` show the
  `<name>-backup` CronJob and its runs (`docs/ha.md`). The CronJob uses the token
  in `spec.backup.adminTokenSecret`.
- Command: `relay backup --url http://localhost:7000 --dest file:///path/to/backups --token "$RELAY_BACKUP_TOKEN"`.
  Use the port-forward from [install.md](install.md) for `--url`, or the service
  DNS name from inside the cluster. `--retention-secs N` drops older objects.
  The binary needs a build with `--features backup`; the default build does not
  include the command. The token needs role `admin` on `*`. Confirm the flags
  with `cargo run -q -p relay --features backup -- backup --help`.
- Expected: the artifact is written under `--dest`.

## Restore

- Symptom: a node lost its data, or the group must go back to an earlier backup.
- Check: confirm the artifact from [Backup](#backup) is the one you need. There
  is no restore verb in the binary today.
- Command: none today. The artifact holds the raft-snapshot bytes. Restore loads
  them with `load_live`, which replaces the state exactly (`docs/ha.md`; the
  tested path is `crates/relay/tests/it/backup.rs`). A scripted restore drill on
  GKE is tracked by #137.
- Expected: no restore runs from this page today. Stop, and follow `docs/ha.md`
  with a person who owns the cluster before you replace any state.

## Drain

- Symptom: a pod is deleted or restarted, and `/readyz` returns 503 for a while
  before the pod stops serving.
- Check: `/readyz` on the draining pod returns 503. The window is
  `RELAY_GRACE_SECS`, which is `30` in `k8s/base/configmap.yaml`. Check the
  pod itself with `kubectl -n relay port-forward pod/relay-2 7000:7000`, not the
  service, because the service can route to another pod.

  ```bash
  curl -s -o /dev/null -w '%{http_code}\n' localhost:7000/readyz
  ```

- Command: `kubectl -n relay delete pod relay-2`, or
  `kubectl -n relay rollout restart statefulset/relay` to drain all three pods
  one at a time.
- Expected: on SIGTERM the node holds `RELAY_GRACE_SECS` with `/readyz` at 503,
  then the listener closes. The new pod returns to `Ready`.

## Upgrade

- Symptom: a newer release is out, or the cluster runs an older image tag.
- Check: `relay upgrade --check` reports the current and latest binary version
  without changing anything. For Kubernetes, read the image the CR asks for:
  `kubectl -n relay get relay relay -o jsonpath='{.spec.image}'`.
- Command: re-render the CR with the new image and apply it:

  ```bash
  relay k8s instance render --profile prod --namespace relay \
    --image ghcr.io/faberline/relay:<version> | kubectl apply -f -
  kubectl -n relay rollout status statefulset/relay
  ```

- Expected: `rollout status` reports a complete rollout, one pod at a time, and
  each pod is `Ready` before the next one restarts. The version compatibility policy is in [upgrade.md](upgrade.md); the rolling-restart test is `raft_cluster::rolling_restart_keeps_all_acked_messages`.
