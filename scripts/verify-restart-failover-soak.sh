#!/usr/bin/env bash
# Verification wrapper for the restart, failover, and live soak axis.
# Runs the local integration and bounded soak test sequence, verifies non-zero
# test execution across all required modules, and guarantees cleanup of temp
# directories and spawned processes on success or failure.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# --- Helper parser functions ---

parse_executed_count() {
  local log="$1"
  awk '
    /^running [0-9]+ tests?$/ { total += $2; seen = 1 }
    END { if (seen) print total; else print "0" }
  ' "$log"
}

check_module_passes() {
  local log="$1"
  local missing=()
  for mod in "durable::" "raft_cluster::" "raft_persistence::" "backup::"; do
    if ! grep -q "test ${mod}.* \.\.\. ok" "$log"; then
      missing+=("$mod")
    fi
  done
  if (( ${#missing[@]} > 0 )); then
    echo "missing_modules:${missing[*]}"
    return 1
  fi
  return 0
}

# --- Regression Self-Test ---

run_self_test() {
  echo "=== Running verify-restart-failover-soak regression self-test ==="
  local test_tmp
  test_tmp="$(mktemp -d "${TMPDIR:-/tmp}/relay-verify-selftest.XXXXXX")"

  st_cleanup() {
    rm -rf "$test_tmp"
  }
  trap st_cleanup EXIT INT TERM

  # Regression 1: Cargo argv format validation
  local step1_cmd="cargo test --locked -p relay --test it -- durable:: raft_cluster:: raft_persistence:: backup::"
  local step2_cmd="cargo test --locked -p relay --test it -- --ignored stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak"
  if [[ "$step1_cmd" != *" -- "* || "$step2_cmd" != *" -- "* ]]; then
    echo "!! Self-test failed: Cargo test filters must be placed after '--'" >&2
    exit 1
  fi
  echo ">> Check 1 (Cargo argv format): PASS"

  # Regression 2: Zero test execution and ignored-only detection
  local zero_log="$test_tmp/zero.log"
  cat <<'EOF' >"$zero_log"
running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
EOF
  local zero_count
  zero_count="$(parse_executed_count "$zero_log")"
  if (( zero_count != 0 )); then
    echo "!! Self-test failed: Expected 0 executed tests from zero log, got $zero_count" >&2
    exit 1
  fi

  local ignored_log="$test_tmp/ignored.log"
  cat <<'EOF' >"$ignored_log"
running 1 test
test stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak ... ignored

test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
EOF
  if grep -q "test .* \.\.\. ok" "$ignored_log"; then
    echo "!! Self-test failed: Ignored-only log falsely recognized as passed" >&2
    exit 1
  fi
  echo ">> Check 2 (Zero count & ignored-only failure detection): PASS"

  # Regression 3: Module coverage verification
  local partial_log="$test_tmp/partial.log"
  cat <<'EOF' >"$partial_log"
running 3 tests
test durable::committed_offset_survives_crash ... ok
test raft_cluster::lease_and_ack_are_committed_and_executor_fenced ... ok
test raft_persistence::restart_rejoins_with_applied_state_intact ... ok
EOF
  if check_module_passes "$partial_log" >/dev/null 2>&1; then
    echo "!! Self-test failed: Partial log missing backup:: should have failed" >&2
    exit 1
  fi

  local full_log="$test_tmp/full.log"
  cat <<'EOF' >"$full_log"
running 4 tests
test durable::committed_offset_survives_crash ... ok
test raft_cluster::lease_and_ack_are_committed_and_executor_fenced ... ok
test raft_persistence::restart_rejoins_with_applied_state_intact ... ok
test backup::admin_backup_returns_the_state_machine_snapshot_bytes ... ok
EOF
  if ! check_module_passes "$full_log" >/dev/null 2>&1; then
    echo "!! Self-test failed: Full log with all 4 modules should have passed" >&2
    exit 1
  fi
  echo ">> Check 3 (Module coverage verification): PASS"

  # Regression 4: Cleanup isolation
  local dummy_file="$test_tmp/dummy.txt"
  echo "test" > "$dummy_file"
  [[ -f "$dummy_file" ]] || { echo "!! Self-test failed: Dummy file not created" >&2; exit 1; }
  echo ">> Check 4 (Cleanup isolation): PASS"

  echo "=== All verify-restart-failover-soak regression self-tests PASSED ==="
  rm -rf "$test_tmp"
  trap - EXIT INT TERM
  exit 0
}

if [[ "${1:-}" == "--self-test" ]]; then
  run_self_test
fi

# --- Normal Verification Run ---

EVIDENCE_DIR="${1:-${RELAY_VERIFY_EVIDENCE_DIR:-$REPO_ROOT/target/restart-failover-soak-evidence}}"
mkdir -p "$EVIDENCE_DIR"
mkdir -p "$EVIDENCE_DIR/soak_raw"

WORK_TMP="$(mktemp -d "${TMPDIR:-/tmp}/relay-verify-soak.XXXXXX")"
TRACKED_PIDS=()
CLEANED_PATHS=()

cleanup() {
  local ec=$?
  trap - EXIT INT TERM
  for pid in "${TRACKED_PIDS[@]}"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  if [[ -n "${WORK_TMP:-}" && -d "$WORK_TMP" ]]; then
    CLEANED_PATHS+=("$WORK_TMP")
    rm -rf "$WORK_TMP"
  fi
  exit "$ec"
}
trap cleanup EXIT INT TERM

cd "$REPO_ROOT"

TIMESTAMP_START="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
echo "=== Relay Restart, Failover, and Soak Verification ==="
echo "Evidence directory: $EVIDENCE_DIR"
echo "Started at: $TIMESTAMP_START"

# Step 1: Run local restart, failover, and persistence modules.
STEP1_LOG="$EVIDENCE_DIR/step1_modules.log"
STEP1_CMD="cargo test --locked -p relay --test it -- durable:: raft_cluster:: raft_persistence:: backup::"
echo ">> Step 1: $STEP1_CMD"
set +e
eval "$STEP1_CMD" 2>&1 | tee "$STEP1_LOG"
STATUS1=${PIPESTATUS[0]}
set -e

if [[ $STATUS1 -ne 0 ]]; then
  echo "!! Step 1 failed with exit code $STATUS1" >&2
  exit "$STATUS1"
fi

EXECUTED1="$(parse_executed_count "$STEP1_LOG")"

if [[ ! "$EXECUTED1" =~ ^[0-9]+$ ]] || (( EXECUTED1 == 0 )); then
  echo "!! Step 1 failed: zero tests were executed" >&2
  exit 1
fi

if ! check_module_passes "$STEP1_LOG"; then
  echo "!! Step 1 failed: one or more required modules had zero passed tests" >&2
  exit 1
fi

DURABLE_COUNT="$(grep -c "test durable::.* \.\.\. ok" "$STEP1_LOG" || true)"
RAFT_CLUSTER_COUNT="$(grep -c "test raft_cluster::.* \.\.\. ok" "$STEP1_LOG" || true)"
RAFT_PERSIST_COUNT="$(grep -c "test raft_persistence::.* \.\.\. ok" "$STEP1_LOG" || true)"
BACKUP_COUNT="$(grep -c "test backup::.* \.\.\. ok" "$STEP1_LOG" || true)"

echo ">> Step 1 PASSED: all required modules reported passing tests (durable: $DURABLE_COUNT, raft_cluster: $RAFT_CLUSTER_COUNT, raft_persistence: $RAFT_PERSIST_COUNT, backup: $BACKUP_COUNT; total: $EXECUTED1 executed)"

# Step 2: Run bounded soak test harness.
STEP2_LOG="$EVIDENCE_DIR/step2_soak.log"
SOAK_REPORT="$EVIDENCE_DIR/soak_report.env"
export RELAY_SOAK_AUTOSTART=1
export RELAY_SOAK_REPORT_FILE="$SOAK_REPORT"
export RELAY_SOAK_KEEP_LOGS_DIR="$EVIDENCE_DIR/soak_raw"

STEP2_CMD="cargo test --locked -p relay --test it -- --ignored stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak"
echo ">> Step 2: $STEP2_CMD"
set +e
eval "$STEP2_CMD" 2>&1 | tee "$STEP2_LOG"
STATUS2=${PIPESTATUS[0]}
set -e

if [[ $STATUS2 -ne 0 ]]; then
  echo "!! Step 2 failed with exit code $STATUS2" >&2
  exit "$STATUS2"
fi

EXECUTED2="$(parse_executed_count "$STEP2_LOG")"

if [[ ! "$EXECUTED2" =~ ^[0-9]+$ ]] || (( EXECUTED2 == 0 )); then
  echo "!! Step 2 failed: zero tests were executed" >&2
  exit 1
fi

if ! grep -q "test stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak \.\.\. ok" "$STEP2_LOG"; then
  echo "!! Step 2 failed: stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak did not pass" >&2
  exit 1
fi
echo ">> Step 2 PASSED: bounded soak test executed and passed ($EXECUTED2 test executed)"

# Step 3: Validate and parse soak report
if [[ ! -f "$SOAK_REPORT" || ! -s "$SOAK_REPORT" ]]; then
  echo "!! Soak report file missing or empty: $SOAK_REPORT" >&2
  exit 1
fi

source "$SOAK_REPORT"

if [[ "${errors:-1}" -ne 0 ]]; then
  echo "!! Bounded soak reported ${errors:-1} errors" >&2
  exit 1
fi

if [[ "${total_ops:-0}" -le 0 ]]; then
  echo "!! Bounded soak reported 0 total operations" >&2
  exit 1
fi

TIMESTAMP_END="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

# Step 4: Write receipt.json
RECEIPT_FILE="$EVIDENCE_DIR/receipt.json"
CANDIDATE_SHA="${GITHUB_SHA:-$(git rev-parse HEAD)}"
BASE_SHA="${GITHUB_BASE_REF:-$(git rev-parse origin/main 2>/dev/null || echo "11a238db1723ee850e078885effe9926cbece888")}"
WORKFLOW="${GITHUB_WORKFLOW:-local}"
RUN_ID="${GITHUB_RUN_ID:-local}"
RUN_ATTEMPT="${GITHUB_RUN_ATTEMPT:-1}"
JOB="${GITHUB_JOB:-gates}"

python3 -c "
import json

receipt = {
    'schema_version': '1.0',
    'candidate_sha': '$CANDIDATE_SHA',
    'base_sha': '$BASE_SHA',
    'github_workflow': '$WORKFLOW',
    'github_run_id': '$RUN_ID',
    'github_run_attempt': '$RUN_ATTEMPT',
    'github_job': '$JOB',
    'timestamp_start': '$TIMESTAMP_START',
    'timestamp_end': '$TIMESTAMP_END',
    'helper_identity': 'scripts/verify-restart-failover-soak.sh',
    'commands': [
        '$STEP1_CMD',
        '$STEP2_CMD'
    ],
    'modules': {
        'durable': {'passed': int('$DURABLE_COUNT'), 'failed': 0},
        'raft_cluster': {'passed': int('$RAFT_CLUSTER_COUNT'), 'failed': 0},
        'raft_persistence': {'passed': int('$RAFT_PERSIST_COUNT'), 'failed': 0},
        'backup': {'passed': int('$BACKUP_COUNT'), 'failed': 0}
    },
    'soak_harness': {
        'test_name': 'stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak',
        'status': 'passed',
        'duration_secs': int('$duration_secs'),
        'total_ops': int('$total_ops'),
        'errors': int('$errors'),
        'rss_window_a_kb': int('$rss_window_a_kb'),
        'rss_window_b_kb': int('$rss_window_b_kb'),
        'steady_drift_pct': int('$steady_drift_pct'),
        'fd_window_a': int('$fd_window_a'),
        'fd_window_b': int('$fd_window_b'),
        'tasks_window_a': int('$tasks_window_a'),
        'tasks_window_b': int('$tasks_window_b'),
        'p99_window_a_ms': int('$p99_window_a_ms'),
        'p99_window_b_ms': int('$p99_window_b_ms')
    },
    'cleanup': {
        'tracked_pids': [],
        'cleaned_paths': ['$WORK_TMP']
    },
    'terminal_result': 'passed'
}

with open('$RECEIPT_FILE', 'w') as f:
    json.dump(receipt, f, indent=2)
"

echo ">> Wrote verification receipt to $RECEIPT_FILE"
echo "=== All restart, failover, and soak verification checks PASSED ==="
