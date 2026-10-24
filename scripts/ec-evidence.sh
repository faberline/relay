#!/usr/bin/env bash
# HANDWRITE-BEGIN gap="missing-generator:e2e-test:relay-fail-closed-ec-oracle" tracker="#2175" reason="Relay's EC gates need a test-owned outer oracle because Cargo and meter both exit successfully when an exact filter executes zero tests."
# @spec tech-design/logic/replace-static-only-security-ec-with-negative-evidence.md#unit-test

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/relay-ec-evidence.XXXXXX")"

cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

fail() {
  echo "relay_ec_oracle: $*" >&2
  exit 1
}

check_suite_log() {
  local label="$1"
  local minimum="$2"
  local marker="$3"
  local log="$4"
  local executed
  local marker_count

  executed="$(awk '
    /^running [0-9]+ tests?$/ { total += $2; seen = 1 }
    END { if (seen) print total; else print "missing" }
  ' "$log")"
  if [[ ! "$executed" =~ ^[0-9]+$ ]] || (( executed < minimum )); then
    echo "relay_ec_oracle: suite=${label} expected_at_least=${minimum} executed=${executed}" >&2
    return 1
  fi

  if [[ "$marker" != "-" ]]; then
    marker_count="$(awk -v marker="$marker" 'index($0, marker) { count += 1 } END { print count + 0 }' "$log")"
    if (( marker_count != 1 )); then
      echo "relay_ec_oracle: suite=${label} marker=${marker} occurrences=${marker_count} expected=1" >&2
      return 1
    fi
  fi

  echo "relay_ec_oracle: suite=${label} executed=${executed} state=passed"
}

run_suite() {
  local label="$1"
  local minimum="$2"
  local marker="$3"
  shift 3
  local log="$TMP_DIR/${label}.run.log"
  local status

  echo ">> relay EC suite: ${label}"
  set +e
  "$@" 2>&1 | tee "$log"
  status=${PIPESTATUS[0]}
  set -e
  (( status == 0 )) || fail "suite=${label} child_exit=${status}"
  check_suite_log "$label" "$minimum" "$marker" "$log" || exit 1
}

list_suite() {
  local label="$1"
  shift
  local list="$TMP_DIR/${label}.list.log"
  local status

  set +e
  "$@" >"$list" 2>&1
  status=$?
  set -e
  (( status == 0 )) || {
    cat "$list" >&2
    fail "suite=${label} list_exit=${status}"
  }
  cat "$list"
}

require_listed() {
  local label="$1"
  shift
  local list="$TMP_DIR/${label}.list.log"
  local name

  for name in "$@"; do
    awk -v expected="${name}: test" '$0 == expected { found = 1 } END { exit found ? 0 : 1 }' "$list" || {
      echo "relay_ec_oracle: suite=${label} missing_required_test=${name}" >&2
      return 1
    }
  done
}

self_test() {
  local zero="$TMP_DIR/self-zero.log"
  local missing_marker="$TMP_DIR/self-marker.log"
  local valid="$TMP_DIR/self-valid.log"
  local list="$TMP_DIR/self-list.list.log"

  printf 'running 0 tests\n\ntest result: ok. 0 passed; 0 failed\n' >"$zero"
  if check_suite_log self-zero 1 - "$zero" >/dev/null 2>&1; then
    fail "self-test accepted a zero-test false green"
  fi
  printf 'running 1 test\n\ntest result: ok. 1 passed; 0 failed\n' >"$missing_marker"
  if check_suite_log self-marker 1 relay_perf_gate "$missing_marker" >/dev/null 2>&1; then
    fail "self-test accepted a missing performance marker"
  fi
  printf 'running 1 test\nrelay_perf_gate workload=fixture\n\ntest result: ok. 1 passed; 0 failed\n' >"$valid"
  check_suite_log self-valid 1 relay_perf_gate "$valid" >/dev/null
  printf 'required_gate: test\n' >"$list"
  require_listed self-list required_gate
  if require_listed self-list missing_gate >/dev/null 2>&1; then
    fail "self-test accepted a missing required test name"
  fi
  echo "relay_ec_oracle: self_test=passed"
}

performance_behavior() {
  list_suite perf-work-queue cargo test -q -p relay --test it work_queue_throughput:: -- --list
  require_listed perf-work-queue \
    work_queue_throughput::ack_batch_skips_stale_epoch \
    work_queue_throughput::committed_watermark_tracks_contiguous_prefix \
    work_queue_throughput::concurrent_subjects_are_isolated_and_exactly_once \
    work_queue_throughput::cursor_leases_in_order \
    work_queue_throughput::lease_batch_returns_up_to_max \
    work_queue_throughput::prefers_redeliver_over_fresh
  run_suite perf-work-queue 6 - \
    cargo test -q -p relay --test it work_queue_throughput:: -- --nocapture

  list_suite perf-decision-model cargo test -q -p relay --test it perf_gate:: -- --list
  require_listed perf-decision-model \
    perf_gate::gate_fails_when_must_beat_cell_is_lost \
    perf_gate::gate_workloads_are_valid \
    perf_gate::ratchet_fails_on_regression \
    perf_gate::ratchet_holds_when_no_regression \
    perf_gate::report_only_cell_does_not_fail_when_behind
  run_suite perf-decision-model 5 - \
    cargo test -q -p relay --test it perf_gate:: -- --nocapture
}

performance_efficiency() {
  list_suite perf-measured \
    cargo test -q --release -p relay --test it measured_performance:: -- --ignored --list
  require_listed perf-measured \
    measured_performance::durable_lifecycle_report_child \
    measured_performance::measured_durable_lifecycle_gate
  run_suite perf-measured 1 'relay_perf_gate workload=' \
    cargo test -q --release -p relay --test it \
      measured_performance::measured_durable_lifecycle_gate -- --exact --ignored --nocapture

  echo ">> meter evidence: measured durable lifecycle"
  meter test -- --release -p relay --test it \
    measured_performance::measured_durable_lifecycle_gate -- --exact --ignored --nocapture
}

security_behavior() {
  list_suite security-auth cargo test -q -p relay --test it auth:: -- --list
  require_listed security-auth \
    auth::consume_side_requires_read_grant_on_subject \
    auth::error_bodies_use_shared_service_auth_shape \
    auth::off_mode_keeps_tokenless_behavior \
    auth::probes_stay_tokenless_under_required_auth \
    auth::publish_requires_write_grant_on_subject \
    auth::relay_auth_adapter_rotates_the_shared_registry_without_restart \
    auth::resolve_fails_fast_on_missing_or_bad_registry \
    auth::streaming_consume_enforces_read_grant
  run_suite security-auth 8 - cargo test -q -p relay --test it auth:: -- --nocapture

  list_suite security-admission cargo test -q -p relay --test it service_admission:: -- --list
  require_listed security-admission \
    service_admission::default_router_keeps_admission_disabled \
    service_admission::publish_uses_shared_write_admission
  run_suite security-admission 2 - \
    cargo test -q -p relay --test it service_admission:: -- --nocapture
}

security_boundaries() {
  list_suite security-peer-tls cargo test -q -p relay --lib tls::peer::tests:: -- --list
  require_listed security-peer-tls \
    tls::peer::tests::builds_rustls_peer_configs_from_pem_material \
    tls::peer::tests::empty_ca_file_returns_err \
    tls::peer::tests::from_env_errors_on_partial_config \
    tls::peer::tests::from_env_loads_when_all_set \
    tls::peer::tests::from_env_returns_none_when_nothing_set \
    tls::peer::tests::mis_pointed_ca_path_fails_fast_naming_the_path \
    tls::peer::tests::mis_pointed_cert_path_fails_fast_naming_the_path \
    tls::peer::tests::mis_pointed_key_path_fails_fast_naming_the_path \
    tls::peer::tests::mismatched_key_returns_err_from_rustls_server_config \
    tls::peer::tests::non_pem_ca_file_fails_rustls_configs \
    tls::peer::tests::non_pem_cert_file_fails_rustls_configs \
    tls::peer::tests::non_pem_key_file_fails_rustls_configs
  run_suite security-peer-tls 12 - \
    cargo test -q -p relay --lib tls::peer::tests:: -- --nocapture

  list_suite security-peer cargo test -q -p relay --test it raft_peer_mtls:: -- --list
  require_listed security-peer \
    raft_peer_mtls::trusted_relay_peers_replicate_messages_over_mtls \
    raft_peer_mtls::untrusted_relay_peer_certificate_is_rejected
  run_suite security-peer 2 - \
    cargo test -q -p relay --test it raft_peer_mtls:: -- --nocapture

  list_suite security-k8s cargo test -q -p relay --test it direct_k8s_assets:: -- --list
  require_listed security-k8s \
    direct_k8s_assets::direct_base_is_a_restricted_durable_singleton \
    direct_k8s_assets::prod_profile_uses_security_components_without_voter_hpa
  run_suite security-k8s 2 - \
    cargo test -q -p relay --test it direct_k8s_assets:: -- --nocapture
}

security_reload() {
  list_suite security-reload \
    bash scripts/faberline-core-test.sh service-auth -q --lib reload::tests -- --list
  require_listed security-reload \
    reload::tests::authorization_events_are_typed_and_credential_free \
    reload::tests::failed_file_reload_emits_read_failure_without_losing_registry \
    reload::tests::file_watcher_adopts_a_valid_replacement_without_restart \
    reload::tests::invalid_replacements_preserve_last_known_good_snapshot \
    reload::tests::valid_rotation_is_immediately_visible_and_advances_revision
  run_suite security-reload 5 - \
    bash scripts/faberline-core-test.sh service-auth -q --lib reload::tests -- --nocapture
}

security_stability() {
  security_reload

  list_suite security-relay-rotation cargo test -q -p relay --test it auth:: -- --list
  require_listed security-relay-rotation \
    auth::relay_auth_adapter_rotates_the_shared_registry_without_restart
  run_suite security-relay-rotation 1 - \
    cargo test -q -p relay --test it \
      auth::relay_auth_adapter_rotates_the_shared_registry_without_restart -- --exact --nocapture

  list_suite security-trusted-peer cargo test -q -p relay --test it raft_peer_mtls:: -- --list
  require_listed security-trusted-peer raft_peer_mtls::trusted_relay_peers_replicate_messages_over_mtls
  run_suite security-trusted-peer 1 - \
    cargo test -q -p relay --test it \
      raft_peer_mtls::trusted_relay_peers_replicate_messages_over_mtls -- --exact --nocapture
}

security_guard() {
  security_behavior
  security_boundaries
  security_reload

  echo ">> meter evidence: Relay security binaries"
  meter test -- -p relay \
    --test it \
    -- auth:: service_admission:: raft_peer_mtls:: direct_k8s_assets:: --nocapture
  echo ">> meter evidence: service-auth reload"
  meter test -- \
    --manifest-path "$(bash scripts/faberline-core-test.sh --root)/Cargo.toml" \
    --target-dir "${CARGO_TARGET_DIR:-target}" \
    -p service-auth --lib reload::tests -- --nocapture
}

cd "$REPO_ROOT"
mode="${1:-}"
self_test
case "$mode" in
  performance-behavior) performance_behavior ;;
  performance-efficiency) performance_efficiency ;;
  security-behavior) security_behavior ;;
  security-boundaries) security_boundaries ;;
  security-stability) security_stability ;;
  security-guard) security_guard ;;
  self-test) ;;
  *) fail "usage: $0 performance-behavior|performance-efficiency|security-behavior|security-boundaries|security-stability|security-guard|self-test" ;;
esac
echo "RELAY_EC_ORACLE mode=${mode} state=passed"

# HANDWRITE-END
