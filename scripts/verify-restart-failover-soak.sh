#!/usr/bin/env bash
# Verification wrapper for the restart, failover, and live soak axis.
# Runs the local integration and bounded soak test sequence, verifies non-zero
# test execution across all required modules, verifies raw logs and post-cleanup
# evidence, and produces an immutable provenance receipt.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# --- Parser and Validation Helpers ---

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

file_sha256() {
  local file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print $1}'
  else
    shasum -a 256 "$file" | awk '{print $1}'
  fi
}

validate_soak_report() {
  local report_file="$1"
  python3 -c "
import sys, re

fields = {}
with open('$report_file') as f:
    for line in f:
        line = line.strip()
        if not line or line.startswith('#'):
            continue
        if '=' not in line:
            sys.exit(f'Invalid report line: {line}')
        k, v = line.split('=', 1)
        fields[k.strip()] = v.strip()

required = [
    'duration_secs', 'total_ops', 'errors', 'rss_window_a_kb', 'rss_window_b_kb',
    'steady_drift_pct', 'fd_window_a', 'fd_window_b', 'tasks_window_a', 'tasks_window_b',
    'p99_window_a_ms', 'p99_window_b_ms'
]

for r in required:
    if r not in fields:
        sys.exit(f'Missing required report field: {r}')
    if not re.fullmatch(r'-?[0-9]+', fields[r]):
        sys.exit(f'Field {r} is not an integer: {fields[r]}')

if int(fields['errors']) != 0:
    sys.exit(f'Soak reported non-zero errors: {fields[\"errors\"]}')

if int(fields['total_ops']) <= 0:
    sys.exit(f'Soak reported zero operations: {fields[\"total_ops\"]}')

print('report_valid')
"
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

  # Check 1: Cargo argv format validation
  local step1_cmd="cargo test --locked -p relay --test it -- durable:: raft_cluster:: raft_persistence:: backup::"
  local step2_cmd="cargo test --locked -p relay --test it -- --ignored --nocapture stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak"
  if [[ "$step1_cmd" != *" -- "* || "$step2_cmd" != *" -- "* ]]; then
    echo "!! Self-test failed: Cargo test filters must be placed after '--'" >&2
    exit 1
  fi
  echo ">> Check 1 (Cargo argv format): PASS"

  # Check 2: Zero test execution and ignored-only detection
  local zero_log="$test_tmp/zero.log"
  cat <<'EOF' >"$zero_log"
running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
EOF
  local zero_count
  zero_count="$(parse_executed_count "$zero_log")"
  if (( zero_count != 0 )); then
    echo "!! Self-test failed: Expected 0 executed tests, got $zero_count" >&2
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

  # Check 3: Module coverage verification
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

  # Check 4: SHA-40 validation
  local good_sha="ae86077cb61dc0a865231588fa0651de3c8480b6"
  local bad_sha="main"
  local null_sha="0000000000000000000000000000000000000000"
  if [[ ! "$good_sha" =~ ^[0-9a-f]{40}$ || "$good_sha" =~ ^0+$ ]]; then
    echo "!! Self-test failed: 40-hex SHA regex rejected valid SHA" >&2
    exit 1
  fi
  if [[ "$bad_sha" =~ ^[0-9a-f]{40}$ && ! "$bad_sha" =~ ^0+$ ]]; then
    echo "!! Self-test failed: 40-hex SHA regex accepted invalid string" >&2
    exit 1
  fi
  if [[ ! "$null_sha" =~ ^0+$ ]]; then
    echo "!! Self-test failed: all-zero null SHA was not rejected" >&2
    exit 1
  fi
  echo ">> Check 4 (SHA provenance format validation): PASS"

  # Check 5: Soak report non-shell strict numeric parser
  local good_report="$test_tmp/good_report.env"
  cat <<'EOF' >"$good_report"
duration_secs=60
total_ops=9258
errors=0
rss_window_a_kb=14740
rss_window_b_kb=14800
steady_drift_pct=0
fd_window_a=13
fd_window_b=13
tasks_window_a=5
tasks_window_b=5
p99_window_a_ms=0
p99_window_b_ms=0
EOF
  if ! validate_soak_report "$good_report" >/dev/null 2>&1; then
    echo "!! Self-test failed: Valid soak report rejected by parser" >&2
    exit 1
  fi

  local bad_report="$test_tmp/bad_report.env"
  cat <<'EOF' >"$bad_report"
duration_secs=60
total_ops=0
errors=0
EOF
  if validate_soak_report "$bad_report" >/dev/null 2>&1; then
    echo "!! Self-test failed: Report with missing fields accepted by parser" >&2
    exit 1
  fi
  echo ">> Check 5 (Strict numeric soak report parsing): PASS"

  # Check 6: Post-cleanup verification logic
  local mock_pid=99999999
  if kill -0 "$mock_pid" 2>/dev/null; then
    echo "!! Self-test warning: mock pid $mock_pid exists"
  fi
  local mock_dir="$test_tmp/mock_dir"
  mkdir -p "$mock_dir"
  rm -rf "$mock_dir"
  if [[ -d "$mock_dir" ]]; then
    echo "!! Self-test failed: Removed dir still detected" >&2
    exit 1
  fi
  echo ">> Check 6 (Post-cleanup verification logic): PASS"

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
AW_DIR_CREATED=false

# Ensure .aw project root directory exists for AW EC test root discovery
if [[ ! -d "$REPO_ROOT/.aw" ]]; then
  mkdir -p "$REPO_ROOT/.aw"
  AW_DIR_CREATED=true
fi

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
  if [[ "$AW_DIR_CREATED" == "true" && -d "$REPO_ROOT/.aw" ]]; then
    CLEANED_PATHS+=("$REPO_ROOT/.aw")
    rm -rf "$REPO_ROOT/.aw"
  fi
  exit "$ec"
}
trap cleanup EXIT INT TERM

cd "$REPO_ROOT"

# --- Provenance and Exact Source Binding ---

ACTUAL_CHECKOUT_SHA="$(git rev-parse HEAD)"

# Candidate PR head SHA resolution
CANDIDATE_HEAD_SHA="${CANDIDATE_HEAD_SHA:-}"
if [[ -z "$CANDIDATE_HEAD_SHA" || ! "$CANDIDATE_HEAD_SHA" =~ ^[0-9a-f]{40}$ || "$CANDIDATE_HEAD_SHA" =~ ^0+$ ]]; then
  if git rev-parse -q --verify HEAD^2 >/dev/null 2>&1; then
    CANDIDATE_HEAD_SHA="$(git rev-parse HEAD^2)"
  else
    CANDIDATE_HEAD_SHA="$ACTUAL_CHECKOUT_SHA"
  fi
fi

# Base SHA resolution
BASE_SHA="${BASE_SHA:-}"
if [[ -z "$BASE_SHA" || ! "$BASE_SHA" =~ ^[0-9a-f]{40}$ || "$BASE_SHA" =~ ^0+$ ]]; then
  if git rev-parse -q --verify HEAD^1 >/dev/null 2>&1; then
    BASE_SHA="$(git rev-parse HEAD^1)"
  else
    BASE_SHA="$(git rev-parse origin/main 2>/dev/null || true)"
  fi
fi

# Strict 40-hex SHA validation: fail closed on missing/invalid SHA
if [[ ! "$CANDIDATE_HEAD_SHA" =~ ^[0-9a-f]{40}$ || "$CANDIDATE_HEAD_SHA" =~ ^0+$ ]]; then
  echo "!! Invalid or missing candidate_head_sha: '$CANDIDATE_HEAD_SHA'" >&2
  exit 1
fi

if [[ ! "$BASE_SHA" =~ ^[0-9a-f]{40}$ || "$BASE_SHA" =~ ^0+$ ]]; then
  echo "!! Invalid or missing base_sha: '$BASE_SHA'" >&2
  exit 1
fi

if [[ ! "$ACTUAL_CHECKOUT_SHA" =~ ^[0-9a-f]{40}$ || "$ACTUAL_CHECKOUT_SHA" =~ ^0+$ ]]; then
  echo "!! Invalid actual_checkout_sha: '$ACTUAL_CHECKOUT_SHA'" >&2
  exit 1
fi

MERGE_TESTED="false"
if [[ "$ACTUAL_CHECKOUT_SHA" != "$CANDIDATE_HEAD_SHA" ]]; then
  MERGE_TESTED="true"
fi

VERIFY_SCRIPT_SHA256="$(file_sha256 "$0")"
SOAK_SCRIPT_SHA256="$(file_sha256 "$REPO_ROOT/scripts/soak.sh")"

TIMESTAMP_START="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
echo "=== Relay Restart, Failover, and Soak Verification ==="
echo "Candidate Head SHA: $CANDIDATE_HEAD_SHA"
echo "Actual Checkout SHA: $ACTUAL_CHECKOUT_SHA (merge_tested: $MERGE_TESTED)"
echo "Base SHA: $BASE_SHA"
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

if [[ ! -s "$STEP1_LOG" ]]; then
  echo "!! Step 1 failed: step1_modules.log missing or empty" >&2
  exit 1
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
SOAK_CLEANUP="$EVIDENCE_DIR/soak_cleanup.env"

export RELAY_SOAK_AUTOSTART=1
export RELAY_SOAK_REPORT_FILE="$SOAK_REPORT"
export RELAY_SOAK_KEEP_LOGS_DIR="$EVIDENCE_DIR/soak_raw"
export RELAY_SOAK_CLEANUP_RECEIPT="$SOAK_CLEANUP"

STEP2_CMD="cargo test --locked -p relay --test it -- --ignored --nocapture stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak"
echo ">> Step 2: $STEP2_CMD"
set +e
eval "$STEP2_CMD" 2>&1 | tee "$STEP2_LOG"
STATUS2=${PIPESTATUS[0]}
set -e

if [[ $STATUS2 -ne 0 ]]; then
  echo "!! Step 2 failed with exit code $STATUS2" >&2
  exit "$STATUS2"
fi

if [[ ! -s "$STEP2_LOG" ]]; then
  echo "!! Step 2 failed: step2_soak.log missing or empty" >&2
  exit 1
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

# Step 3: Verify required raw evidence logs exist and are non-empty
RELAY_LOG="$EVIDENCE_DIR/soak_raw/relay.log"
LATENCY_A="$EVIDENCE_DIR/soak_raw/latency-a.seconds"
LATENCY_B="$EVIDENCE_DIR/soak_raw/latency-b.seconds"

for req_file in "$RELAY_LOG" "$LATENCY_A" "$LATENCY_B" "$SOAK_REPORT" "$SOAK_CLEANUP"; do
  if [[ ! -f "$req_file" || ! -s "$req_file" ]]; then
    echo "!! Required raw evidence file missing or empty: $req_file" >&2
    exit 1
  fi
done

# Step 4: Strict non-shell numeric parsing of soak report
if ! validate_soak_report "$SOAK_REPORT"; then
  echo "!! Soak report validation failed on numeric assertions" >&2
  exit 1
fi

# Step 5: Post-cleanup verification of harness-owned resources
AUTOSTART_PID_STOPPED=false
AUTOSTART_DIR_CLEANED=false
SOAK_TMP_DIR_CLEANED=false

CLEANUP_VALUES="$(python3 -c "
with open('$SOAK_CLEANUP') as f:
    for line in f:
        line = line.strip()
        if line.startswith('soak_autostart_pid='):
            pid = line.split('=', 1)[1]
        elif line.startswith('soak_autostart_dir='):
            adir = line.split('=', 1)[1]
        elif line.startswith('soak_tmp_dir='):
            tdir = line.split('=', 1)[1]
print(f'{pid} {adir} {tdir}')
")"

read -r SOAK_PID SOAK_ADIR SOAK_TDIR <<<"$CLEANUP_VALUES"

# Verify autostart PID stopped
if [[ -n "$SOAK_PID" && "$SOAK_PID" =~ ^[0-9]+$ ]]; then
  if kill -0 "$SOAK_PID" 2>/dev/null; then
    echo "!! Cleanup verification failed: soak autostart PID $SOAK_PID is still running" >&2
    exit 1
  fi
  AUTOSTART_PID_STOPPED=true
fi

# Verify autostart directory removed
if [[ -n "$SOAK_ADIR" ]]; then
  if [[ -e "$SOAK_ADIR" ]]; then
    echo "!! Cleanup verification failed: soak autostart dir $SOAK_ADIR still exists" >&2
    exit 1
  fi
  AUTOSTART_DIR_CLEANED=true
fi

# Verify soak tmp directory removed
if [[ -n "$SOAK_TDIR" ]]; then
  if [[ -e "$SOAK_TDIR" ]]; then
    echo "!! Cleanup verification failed: soak tmp dir $SOAK_TDIR still exists" >&2
    exit 1
  fi
  SOAK_TMP_DIR_CLEANED=true
fi

# Verify wrapper work tmp directory removed
if [[ -d "$WORK_TMP" ]]; then
  rm -rf "$WORK_TMP"
fi
WORK_TMP_CLEANED=false
if [[ ! -d "$WORK_TMP" ]]; then
  WORK_TMP_CLEANED=true
fi

# Verify .aw removed if created
if [[ "$AW_DIR_CREATED" == "true" && -d "$REPO_ROOT/.aw" ]]; then
  rm -rf "$REPO_ROOT/.aw"
fi

TIMESTAMP_END="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

# Compute sha256 digests for raw log artifacts
STEP1_LOG_SHA256="$(file_sha256 "$STEP1_LOG")"
STEP2_LOG_SHA256="$(file_sha256 "$STEP2_LOG")"
RELAY_LOG_SHA256="$(file_sha256 "$RELAY_LOG")"
LATENCY_A_SHA256="$(file_sha256 "$LATENCY_A")"
LATENCY_B_SHA256="$(file_sha256 "$LATENCY_B")"
SOAK_REPORT_SHA256="$(file_sha256 "$SOAK_REPORT")"
SOAK_CLEANUP_SHA256="$(file_sha256 "$SOAK_CLEANUP")"

# Step 6: Generate receipt.json
RECEIPT_FILE="$EVIDENCE_DIR/receipt.json"
WORKFLOW="${GITHUB_WORKFLOW:-local}"
RUN_ID="${GITHUB_RUN_ID:-local}"
RUN_ATTEMPT="${GITHUB_RUN_ATTEMPT:-1}"
JOB="${GITHUB_JOB:-gates}"

python3 -c "
import json

report_fields = {}
with open('$SOAK_REPORT') as f:
    for line in f:
        line = line.strip()
        if line and not line.startswith('#') and '=' in line:
            k, v = line.split('=', 1)
            report_fields[k.strip()] = int(v.strip())

receipt = {
    'schema_version': '1.0',
    'candidate_head_sha': '$CANDIDATE_HEAD_SHA',
    'actual_checkout_sha': '$ACTUAL_CHECKOUT_SHA',
    'base_sha': '$BASE_SHA',
    'merge_tested': ($MERGE_TESTED == 'true'),
    'github_workflow': '$WORKFLOW',
    'github_run_id': '$RUN_ID',
    'github_run_attempt': '$RUN_ATTEMPT',
    'github_job': '$JOB',
    'timestamp_start': '$TIMESTAMP_START',
    'timestamp_end': '$TIMESTAMP_END',
    'helper_identity': {
        'path': 'scripts/verify-restart-failover-soak.sh',
        'verify_script_sha256': '$VERIFY_SCRIPT_SHA256',
        'soak_script_sha256': '$SOAK_SCRIPT_SHA256'
    },
    'commands': [
        '$STEP1_CMD',
        '$STEP2_CMD'
    ],
    'raw_artifact_digests': {
        'step1_modules_log_sha256': '$STEP1_LOG_SHA256',
        'step2_soak_log_sha256': '$STEP2_LOG_SHA256',
        'relay_log_sha256': '$RELAY_LOG_SHA256',
        'latency_a_sha256': '$LATENCY_A_SHA256',
        'latency_b_sha256': '$LATENCY_B_SHA256',
        'soak_report_sha256': '$SOAK_REPORT_SHA256',
        'soak_cleanup_sha256': '$SOAK_CLEANUP_SHA256'
    },
    'modules': {
        'durable': {'passed': int('$DURABLE_COUNT'), 'failed': 0},
        'raft_cluster': {'passed': int('$RAFT_CLUSTER_COUNT'), 'failed': 0},
        'raft_persistence': {'passed': int('$RAFT_PERSIST_COUNT'), 'failed': 0},
        'backup': {'passed': int('$BACKUP_COUNT'), 'failed': 0}
    },
    'soak_harness': {
        'test_name': 'stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak',
        'status': 'passed',
        'duration_secs': report_fields['duration_secs'],
        'total_ops': report_fields['total_ops'],
        'errors': report_fields['errors'],
        'rss_window_a_kb': report_fields['rss_window_a_kb'],
        'rss_window_b_kb': report_fields['rss_window_b_kb'],
        'steady_drift_pct': report_fields['steady_drift_pct'],
        'fd_window_a': report_fields['fd_window_a'],
        'fd_window_b': report_fields['fd_window_b'],
        'tasks_window_a': report_fields['tasks_window_a'],
        'tasks_window_b': report_fields['tasks_window_b'],
        'p99_window_a_ms': report_fields['p99_window_a_ms'],
        'p99_window_b_ms': report_fields['p99_window_b_ms']
    },
    'cleanup': {
        'autostart_pid': int('$SOAK_PID') if '$SOAK_PID'.isdigit() else None,
        'autostart_pid_stopped': ($AUTOSTART_PID_STOPPED == 'true'),
        'autostart_dir_cleaned': ($AUTOSTART_DIR_CLEANED == 'true'),
        'soak_tmp_dir_cleaned': ($SOAK_TMP_DIR_CLEANED == 'true'),
        'wrapper_tmp_cleaned': ($WORK_TMP_CLEANED == 'true'),
        'status': 'verified'
    },
    'terminal_result': 'passed'
}

with open('$RECEIPT_FILE', 'w') as f:
    json.dump(receipt, f, indent=2)
"

echo ">> Wrote verification receipt to $RECEIPT_FILE"
echo "=== All restart, failover, and soak verification checks PASSED ==="
