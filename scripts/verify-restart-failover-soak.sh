#!/usr/bin/env bash
# Verification wrapper for the restart, failover, and live soak axis.
# Runs the local integration and bounded soak test sequence, verifies non-zero
# test execution across all required modules, and guarantees cleanup of temp
# directories and spawned processes on success or failure.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/relay-verify-soak.XXXXXX")"

cleanup() {
  local ec=$?
  trap - EXIT INT TERM
  local pids
  pids="$(jobs -p 2>/dev/null || true)"
  if [[ -n "$pids" ]]; then
    kill $pids 2>/dev/null || true
    wait $pids 2>/dev/null || true
  fi
  if [[ -n "${TMP_DIR:-}" && -d "$TMP_DIR" ]]; then
    rm -rf "$TMP_DIR"
  fi
  exit "$ec"
}
trap cleanup EXIT INT TERM

cd "$REPO_ROOT"

echo "=== Relay Restart, Failover, and Soak Verification ==="

# Step 1: Run local restart, failover, and persistence modules.
STEP1_LOG="$TMP_DIR/step1_modules.log"
echo ">> Step 1: cargo test --locked -p relay --test it durable:: raft_cluster:: raft_persistence:: backup::"
set +e
cargo test --locked -p relay --test it durable:: raft_cluster:: raft_persistence:: backup:: 2>&1 | tee "$STEP1_LOG"
STATUS1=${PIPESTATUS[0]}
set -e

if [[ $STATUS1 -ne 0 ]]; then
  echo "!! Step 1 failed with exit code $STATUS1" >&2
  exit "$STATUS1"
fi

EXECUTED1="$(awk '
  /^running [0-9]+ tests?$/ { total += $2; seen = 1 }
  END { if (seen) print total; else print "0" }
' "$STEP1_LOG")"

if [[ ! "$EXECUTED1" =~ ^[0-9]+$ ]] || (( EXECUTED1 == 0 )); then
  echo "!! Step 1 failed: zero tests were executed" >&2
  exit 1
fi

for mod in "durable::" "raft_cluster::" "raft_persistence::" "backup::"; do
  if ! grep -q "test ${mod}.* \.\.\. ok" "$STEP1_LOG"; then
    echo "!! Step 1 failed: module ${mod} had zero passed tests" >&2
    exit 1
  fi
done
echo ">> Step 1 PASSED: all required modules (durable, raft_cluster, raft_persistence, backup) reported passing tests ($EXECUTED1 tests executed)"

# Step 2: Run bounded soak test harness.
STEP2_LOG="$TMP_DIR/step2_soak.log"
echo ">> Step 2: cargo test --locked -p relay --test it -- --ignored stability_relay_competitor_performance_bounded_soak::"
set +e
cargo test --locked -p relay --test it -- --ignored stability_relay_competitor_performance_bounded_soak:: 2>&1 | tee "$STEP2_LOG"
STATUS2=${PIPESTATUS[0]}
set -e

if [[ $STATUS2 -ne 0 ]]; then
  echo "!! Step 2 failed with exit code $STATUS2" >&2
  exit "$STATUS2"
fi

EXECUTED2="$(awk '
  /^running [0-9]+ tests?$/ { total += $2; seen = 1 }
  END { if (seen) print total; else print "0" }
' "$STEP2_LOG")"

if [[ ! "$EXECUTED2" =~ ^[0-9]+$ ]] || (( EXECUTED2 == 0 )); then
  echo "!! Step 2 failed: zero tests were executed" >&2
  exit 1
fi

if ! grep -q "test stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak \.\.\. ok" "$STEP2_LOG"; then
  echo "!! Step 2 failed: stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak did not pass" >&2
  exit 1
fi
echo ">> Step 2 PASSED: bounded soak test executed and passed ($EXECUTED2 test executed)"

echo "=== All restart, failover, and soak verification checks PASSED ==="
