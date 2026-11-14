#!/usr/bin/env bash
# Local competitor semantics and efficiency verification for relay (issue #50).
#
# Runs the local in-process competitor-performance test modules and the
# release-mode measured durable lifecycle gate as one command. A module that
# runs 0 tests fails the gate. Temp files are removed on success and failure.
# Peer comparison (RabbitMQ, NATS, Redis, Dragonfly), live, GKE and cloud runs
# are not part of this script; they are a manual pre-release check.
#
# Environment:
#   CE_MODULES       space-separated test modules
#                    (default: work_queue_throughput perf_gate measured_performance)
#   CE_SKIP_MEASURED=1  skip the measured gate (used by the offline tests)

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

require() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "!! required command not found: $1" >&2
    exit 1
  }
}

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ce-verify.XXXXXX")"

cleanup() {
  local ec=$?
  trap - EXIT INT TERM
  rm -rf "$TMP_DIR"
  exit "$ec"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

require cargo

# ${CE_MODULES-...} (no colon) keeps an explicitly empty value empty.
MODULES=(${CE_MODULES-work_queue_throughput perf_gate measured_performance})
(( ${#MODULES[@]} > 0 )) || {
  echo "!! CE_MODULES is empty" >&2
  exit 1
}

run_module() {
  local m="$1"
  local log="$TMP_DIR/${m}.log"
  local status=0
  cargo test --locked -p relay --test it -- "${m}::" >"$log" 2>&1 || status=$?
  if (( status != 0 )); then
    tail -n 40 "$log" >&2
    echo "!! ${m}: cargo test failed (exit ${status})" >&2
    exit 1
  fi
  local n
  n="$(sed -n 's/^test result: ok\. \([0-9][0-9]*\) passed.*/\1/p' "$log" | tail -n 1)"
  if [[ -z "$n" || "$n" == "0" ]]; then
    echo "!! ${m}: 0 tests ran" >&2
    exit 1
  fi
  echo ">> ${m}: ${n} passed"
}

for m in "${MODULES[@]}"; do
  run_module "$m"
done

if [[ "${CE_SKIP_MEASURED:-0}" != "1" ]]; then
  gate_log="$TMP_DIR/measured.log"
  gate_status=0
  cargo test -q --release --locked -p relay --test it \
    measured_performance::measured_durable_lifecycle_gate -- \
    --exact --ignored --nocapture >"$gate_log" 2>&1 || gate_status=$?
  if (( gate_status != 0 )); then
    tail -n 40 "$gate_log" >&2
    echo "!! measured gate failed (exit ${gate_status})" >&2
    exit 1
  fi
  marker_count="$(grep -c 'relay_perf_gate workload=' "$gate_log" || true)"
  marker="$(grep 'relay_perf_gate workload=' "$gate_log" | head -n 1 || true)"
  if [[ "$marker_count" != "1" || "$marker" != *"errors=0"* ]]; then
    echo "!! measured gate: marker missing or errors != 0" >&2
    exit 1
  fi
  grep -q '^running 1 test' "$gate_log" || {
    echo "!! measured gate: 0 tests ran" >&2
    exit 1
  }
  echo ">> measured: ${marker}"
fi

echo ">> competitor-efficiency: passed"
