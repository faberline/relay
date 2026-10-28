#!/usr/bin/env bash
# Verification wrapper for the restart, failover, and live soak axis.
# Runs the local integration and bounded soak test sequence, verifies non-zero
# test execution across all required modules, verifies raw logs and post-cleanup
# evidence, and produces an immutable provenance receipt.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REAL_SCRIPT_PATH="$SCRIPT_DIR/$(basename "${BASH_SOURCE[0]}")"

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
  python3 - "$report_file" << 'PYEOF'
import sys, re

report_file = sys.argv[1]
fields = {}
with open(report_file) as f:
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
    sys.exit(f'Soak reported non-zero errors: {fields["errors"]}')

if int(fields['total_ops']) <= 0:
    sys.exit(f'Soak reported zero operations: {fields["total_ops"]}')

print('report_valid')
PYEOF
}

generate_receipt() {
  local receipt_file="$1"
  local soak_report="$2"

  python3 - "$receipt_file" "$soak_report" << 'PYEOF'
import json
import os
import sys

receipt_file = sys.argv[1]
soak_report_path = sys.argv[2]

report_fields = {}
with open(soak_report_path) as f:
    for line in f:
        line = line.strip()
        if line and not line.startswith('#') and '=' in line:
            k, v = line.split('=', 1)
            report_fields[k.strip()] = int(v.strip())

candidate_head_sha = os.environ['CANDIDATE_HEAD_SHA']
actual_checkout_sha = os.environ['ACTUAL_CHECKOUT_SHA']
base_sha = os.environ['BASE_SHA']
merge_tested = (os.environ.get('MERGE_TESTED', 'false') == 'true')

autostart_pid_str = os.environ.get('SOAK_PID', '')
autostart_pid = int(autostart_pid_str) if autostart_pid_str.isdigit() else None

autostart_pid_stopped = (os.environ.get('AUTOSTART_PID_STOPPED', 'false') == 'true')
autostart_dir_cleaned = (os.environ.get('AUTOSTART_DIR_CLEANED', 'false') == 'true')
soak_tmp_dir_cleaned = (os.environ.get('SOAK_TMP_DIR_CLEANED', 'false') == 'true')
wrapper_tmp_cleaned = (os.environ.get('WORK_TMP_CLEANED', 'false') == 'true')

receipt = {
    'schema_version': '1.0',
    'candidate_head_sha': candidate_head_sha,
    'actual_checkout_sha': actual_checkout_sha,
    'base_sha': base_sha,
    'merge_tested': merge_tested,
    'github_workflow': os.environ.get('GITHUB_WORKFLOW', 'local'),
    'github_run_id': os.environ.get('GITHUB_RUN_ID', 'local'),
    'github_run_attempt': int(os.environ.get('GITHUB_RUN_ATTEMPT', '1')),
    'github_job': os.environ.get('GITHUB_JOB', 'gates'),
    'timestamp_start': os.environ['TIMESTAMP_START'],
    'timestamp_end': os.environ['TIMESTAMP_END'],
    'helper_identity': {
        'path': 'scripts/verify-restart-failover-soak.sh',
        'verify_script_sha256': os.environ['VERIFY_SCRIPT_SHA256'],
        'soak_script_sha256': os.environ['SOAK_SCRIPT_SHA256'],
    },
    'commands': [
        os.environ['STEP1_CMD'],
        os.environ['STEP2_CMD'],
    ],
    'raw_artifact_digests': {
        'step1_modules_log_sha256': os.environ['STEP1_LOG_SHA256'],
        'step2_soak_log_sha256': os.environ['STEP2_LOG_SHA256'],
        'relay_log_sha256': os.environ['RELAY_LOG_SHA256'],
        'latency_a_sha256': os.environ['LATENCY_A_SHA256'],
        'latency_b_sha256': os.environ['LATENCY_B_SHA256'],
        'soak_report_sha256': os.environ['SOAK_REPORT_SHA256'],
        'soak_cleanup_sha256': os.environ['SOAK_CLEANUP_SHA256'],
    },
    'modules': {
        'durable': {'passed': int(os.environ['DURABLE_COUNT']), 'failed': 0},
        'raft_cluster': {'passed': int(os.environ['RAFT_CLUSTER_COUNT']), 'failed': 0},
        'raft_persistence': {'passed': int(os.environ['RAFT_PERSIST_COUNT']), 'failed': 0},
        'backup': {'passed': int(os.environ['BACKUP_COUNT']), 'failed': 0},
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
        'p99_window_b_ms': report_fields['p99_window_b_ms'],
    },
    'cleanup': {
        'autostart_pid': autostart_pid,
        'autostart_pid_stopped': autostart_pid_stopped,
        'autostart_dir_cleaned': autostart_dir_cleaned,
        'soak_tmp_dir_cleaned': soak_tmp_dir_cleaned,
        'wrapper_tmp_cleaned': wrapper_tmp_cleaned,
        'status': 'verified',
    },
    'terminal_result': 'passed',
}

with open(receipt_file, 'w') as f:
    json.dump(receipt, f, indent=2)
PYEOF
}

# --- Regression Self-Test ---

run_self_test() {
  echo "=== Running verify-restart-failover-soak regression self-test ==="
  local test_tmp
  test_tmp="$(mktemp -d "${TMPDIR:-/tmp}/relay-verify-selftest.XXXXXX")"
  local fake_bin="$test_tmp/fake_bin"
  mkdir -p "$fake_bin"

  local mock_argv_log="$test_tmp/cargo_argv.log"
  local mock_leak_pids="$test_tmp/leak_pids.log"
  local mock_leak_dirs="$test_tmp/leak_dirs.log"
  touch "$mock_argv_log" "$mock_leak_pids" "$mock_leak_dirs"

  st_cleanup() {
    if [[ -f "$mock_leak_pids" ]]; then
      while read -r leak_pid; do
        if [[ -n "$leak_pid" ]] && kill -0 "$leak_pid" 2>/dev/null; then
          kill "$leak_pid" 2>/dev/null || true
          wait "$leak_pid" 2>/dev/null || true
        fi
      done < "$mock_leak_pids"
    fi
    if [[ -f "$mock_leak_dirs" ]]; then
      while read -r leak_dir; do
        if [[ -n "$leak_dir" && -d "$leak_dir" ]]; then
          rm -rf "$leak_dir"
        fi
      done < "$mock_leak_dirs"
    fi
    rm -rf "$test_tmp"
  }
  trap st_cleanup EXIT INT TERM

  # Create mock cargo test runner harness
  cat <<'EOF' > "$fake_bin/cargo"
#!/usr/bin/env bash
set -euo pipefail

echo "$*" >> "$MOCK_CARGO_ARGV_LOG"

scenario="${MOCK_CARGO_SCENARIO:-positive}"

case "$scenario" in
  positive)
    if [[ "$*" == *"durable::"* ]]; then
      cat <<'CARGO_OUT'
running 15 tests
test backup::admin_backup_returns_the_state_machine_snapshot_bytes ... ok
test backup::admin_backup_requires_cluster_mode ... ok
test backup::admin_backup_verifies_durable_header ... ok
test backup::admin_backup_records_snapshot_meta ... ok
test durable::default_config_is_durable_and_power_safe ... ok
test durable::committed_offset_survives_crash ... ok
test durable::durable_log_recovers_after_reopen ... ok
test durable::publish_batch_group_commit_and_dedupe ... ok
test durable::ack_truncation_recovers_consistently ... ok
test durable::state_machine_replays_in_order ... ok
test raft_cluster::fresh_node_catches_up_via_install_snapshot ... ok
test raft_cluster::lease_and_ack_are_committed_and_executor_fenced ... ok
test raft_cluster::three_node_group_elects_replicates_forwards_and_fails_over ... ok
test raft_persistence::acked_work_is_not_resurrected_by_cold_replay ... ok
test raft_persistence::restart_rejoins_with_applied_state_intact ... ok

test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
CARGO_OUT
      exit 0
    elif [[ "$*" == *"stability_relay_competitor_performance_bounded_soak"* ]]; then
      cat <<'CARGO_OUT'
running 1 test
test stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
CARGO_OUT
      mkdir -p "$RELAY_SOAK_KEEP_LOGS_DIR"
      echo "[mock relay server log]" > "$RELAY_SOAK_KEEP_LOGS_DIR/relay.log"
      echo "0.001" > "$RELAY_SOAK_KEEP_LOGS_DIR/latency-a.seconds"
      echo "0.002" > "$RELAY_SOAK_KEEP_LOGS_DIR/latency-b.seconds"

      cat <<'REPORT_EOF' > "$RELAY_SOAK_REPORT_FILE"
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
REPORT_EOF

      python3 -c "import time; time.sleep(0.01)" &
      cpid=$!
      wait $cpid 2>/dev/null || true

      mock_adir="$(mktemp -d "${TMPDIR:-/tmp}/mock-autostart.XXXXXX")"
      rmdir "$mock_adir"
      mock_tdir="$(mktemp -d "${TMPDIR:-/tmp}/mock-soaktmp.XXXXXX")"
      rmdir "$mock_tdir"

      cat <<CLEANUP_EOF > "$RELAY_SOAK_CLEANUP_RECEIPT"
soak_autostart_pid=$cpid
soak_autostart_dir=$mock_adir
soak_tmp_dir=$mock_tdir
CLEANUP_EOF
      exit 0
    fi
    ;;

  zero_tests)
    echo "running 0 tests"
    echo "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
    exit 0
    ;;

  ignored_only)
    if [[ "$*" == *"durable::"* ]]; then
      cat <<'CARGO_OUT'
running 4 tests
test backup::admin_backup_returns_the_state_machine_snapshot_bytes ... ok
test durable::default_config_is_durable_and_power_safe ... ok
test raft_cluster::fresh_node_catches_up_via_install_snapshot ... ok
test raft_persistence::acked_work_is_not_resurrected_by_cold_replay ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
CARGO_OUT
      exit 0
    else
      echo "running 1 test"
      echo "test stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak ... ignored"
      echo "test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out"
      exit 0
    fi
    ;;

  missing_module)
    if [[ "$*" == *"durable::"* ]]; then
      cat <<'CARGO_OUT'
running 3 tests
test durable::default_config_is_durable_and_power_safe ... ok
test raft_cluster::fresh_node_catches_up_via_install_snapshot ... ok
test raft_persistence::acked_work_is_not_resurrected_by_cold_replay ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
CARGO_OUT
      exit 0
    fi
    ;;

  missing_raw_log)
    if [[ "$*" == *"durable::"* ]]; then
      cat <<'CARGO_OUT'
running 4 tests
test backup::admin_backup_returns_the_state_machine_snapshot_bytes ... ok
test durable::default_config_is_durable_and_power_safe ... ok
test raft_cluster::fresh_node_catches_up_via_install_snapshot ... ok
test raft_persistence::acked_work_is_not_resurrected_by_cold_replay ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
CARGO_OUT
      exit 0
    else
      cat <<'CARGO_OUT'
running 1 test
test stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
CARGO_OUT
      mkdir -p "$RELAY_SOAK_KEEP_LOGS_DIR"
      # Omit relay.log
      echo "0.001" > "$RELAY_SOAK_KEEP_LOGS_DIR/latency-a.seconds"
      echo "0.002" > "$RELAY_SOAK_KEEP_LOGS_DIR/latency-b.seconds"
      cat <<'REPORT_EOF' > "$RELAY_SOAK_REPORT_FILE"
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
REPORT_EOF
      python3 -c "import time; time.sleep(0.01)" &
      cpid=$!
      wait $cpid 2>/dev/null || true
      mock_adir="$(mktemp -d "${TMPDIR:-/tmp}/mock-autostart.XXXXXX")"
      rmdir "$mock_adir"
      mock_tdir="$(mktemp -d "${TMPDIR:-/tmp}/mock-soaktmp.XXXXXX")"
      rmdir "$mock_tdir"
      cat <<CLEANUP_EOF > "$RELAY_SOAK_CLEANUP_RECEIPT"
soak_autostart_pid=$cpid
soak_autostart_dir=$mock_adir
soak_tmp_dir=$mock_tdir
CLEANUP_EOF
      exit 0
    fi
    ;;

  missing_latency_log)
    if [[ "$*" == *"durable::"* ]]; then
      cat <<'CARGO_OUT'
running 4 tests
test backup::admin_backup_returns_the_state_machine_snapshot_bytes ... ok
test durable::default_config_is_durable_and_power_safe ... ok
test raft_cluster::fresh_node_catches_up_via_install_snapshot ... ok
test raft_persistence::acked_work_is_not_resurrected_by_cold_replay ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
CARGO_OUT
      exit 0
    else
      cat <<'CARGO_OUT'
running 1 test
test stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
CARGO_OUT
      mkdir -p "$RELAY_SOAK_KEEP_LOGS_DIR"
      echo "[mock relay server log]" > "$RELAY_SOAK_KEEP_LOGS_DIR/relay.log"
      # Omit latency-a.seconds
      echo "0.002" > "$RELAY_SOAK_KEEP_LOGS_DIR/latency-b.seconds"
      cat <<'REPORT_EOF' > "$RELAY_SOAK_REPORT_FILE"
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
REPORT_EOF
      python3 -c "import time; time.sleep(0.01)" &
      cpid=$!
      wait $cpid 2>/dev/null || true
      mock_adir="$(mktemp -d "${TMPDIR:-/tmp}/mock-autostart.XXXXXX")"
      rmdir "$mock_adir"
      mock_tdir="$(mktemp -d "${TMPDIR:-/tmp}/mock-soaktmp.XXXXXX")"
      rmdir "$mock_tdir"
      cat <<CLEANUP_EOF > "$RELAY_SOAK_CLEANUP_RECEIPT"
soak_autostart_pid=$cpid
soak_autostart_dir=$mock_adir
soak_tmp_dir=$mock_tdir
CLEANUP_EOF
      exit 0
    fi
    ;;

  malformed_metrics)
    if [[ "$*" == *"durable::"* ]]; then
      cat <<'CARGO_OUT'
running 4 tests
test backup::admin_backup_returns_the_state_machine_snapshot_bytes ... ok
test durable::default_config_is_durable_and_power_safe ... ok
test raft_cluster::fresh_node_catches_up_via_install_snapshot ... ok
test raft_persistence::acked_work_is_not_resurrected_by_cold_replay ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
CARGO_OUT
      exit 0
    else
      cat <<'CARGO_OUT'
running 1 test
test stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
CARGO_OUT
      mkdir -p "$RELAY_SOAK_KEEP_LOGS_DIR"
      echo "[mock relay server log]" > "$RELAY_SOAK_KEEP_LOGS_DIR/relay.log"
      echo "0.001" > "$RELAY_SOAK_KEEP_LOGS_DIR/latency-a.seconds"
      echo "0.002" > "$RELAY_SOAK_KEEP_LOGS_DIR/latency-b.seconds"
      cat <<'REPORT_EOF' > "$RELAY_SOAK_REPORT_FILE"
duration_secs=60
total_ops=0
errors=5
rss_window_a_kb=14740
rss_window_b_kb=14800
steady_drift_pct=0
fd_window_a=13
fd_window_b=13
tasks_window_a=5
tasks_window_b=5
p99_window_a_ms=0
p99_window_b_ms=0
REPORT_EOF
      python3 -c "import time; time.sleep(0.01)" &
      cpid=$!
      wait $cpid 2>/dev/null || true
      mock_adir="$(mktemp -d "${TMPDIR:-/tmp}/mock-autostart.XXXXXX")"
      rmdir "$mock_adir"
      mock_tdir="$(mktemp -d "${TMPDIR:-/tmp}/mock-soaktmp.XXXXXX")"
      rmdir "$mock_tdir"
      cat <<CLEANUP_EOF > "$RELAY_SOAK_CLEANUP_RECEIPT"
soak_autostart_pid=$cpid
soak_autostart_dir=$mock_adir
soak_tmp_dir=$mock_tdir
CLEANUP_EOF
      exit 0
    fi
    ;;

  cleanup_failure_pid)
    if [[ "$*" == *"durable::"* ]]; then
      cat <<'CARGO_OUT'
running 4 tests
test backup::admin_backup_returns_the_state_machine_snapshot_bytes ... ok
test durable::default_config_is_durable_and_power_safe ... ok
test raft_cluster::fresh_node_catches_up_via_install_snapshot ... ok
test raft_persistence::acked_work_is_not_resurrected_by_cold_replay ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
CARGO_OUT
      exit 0
    else
      cat <<'CARGO_OUT'
running 1 test
test stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
CARGO_OUT
      mkdir -p "$RELAY_SOAK_KEEP_LOGS_DIR"
      echo "[mock relay server log]" > "$RELAY_SOAK_KEEP_LOGS_DIR/relay.log"
      echo "0.001" > "$RELAY_SOAK_KEEP_LOGS_DIR/latency-a.seconds"
      echo "0.002" > "$RELAY_SOAK_KEEP_LOGS_DIR/latency-b.seconds"
      cat <<'REPORT_EOF' > "$RELAY_SOAK_REPORT_FILE"
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
REPORT_EOF
      mock_adir="$(mktemp -d "${TMPDIR:-/tmp}/mock-autostart.XXXXXX")"
      rmdir "$mock_adir"
      mock_tdir="$(mktemp -d "${TMPDIR:-/tmp}/mock-soaktmp.XXXXXX")"
      rmdir "$mock_tdir"
      cat <<CLEANUP_EOF > "$RELAY_SOAK_CLEANUP_RECEIPT"
soak_autostart_pid=${MOCK_ALIVE_PID:-1}
soak_autostart_dir=$mock_adir
soak_tmp_dir=$mock_tdir
CLEANUP_EOF
      exit 0
    fi
    ;;

  cleanup_failure_dir)
    if [[ "$*" == *"durable::"* ]]; then
      cat <<'CARGO_OUT'
running 4 tests
test backup::admin_backup_returns_the_state_machine_snapshot_bytes ... ok
test durable::default_config_is_durable_and_power_safe ... ok
test raft_cluster::fresh_node_catches_up_via_install_snapshot ... ok
test raft_persistence::acked_work_is_not_resurrected_by_cold_replay ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
CARGO_OUT
      exit 0
    else
      cat <<'CARGO_OUT'
running 1 test
test stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
CARGO_OUT
      mkdir -p "$RELAY_SOAK_KEEP_LOGS_DIR"
      echo "[mock relay server log]" > "$RELAY_SOAK_KEEP_LOGS_DIR/relay.log"
      echo "0.001" > "$RELAY_SOAK_KEEP_LOGS_DIR/latency-a.seconds"
      echo "0.002" > "$RELAY_SOAK_KEEP_LOGS_DIR/latency-b.seconds"
      cat <<'REPORT_EOF' > "$RELAY_SOAK_REPORT_FILE"
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
REPORT_EOF
      python3 -c "import time; time.sleep(0.01)" &
      cpid=$!
      wait $cpid 2>/dev/null || true
      mock_adir="$(mktemp -d "${TMPDIR:-/tmp}/mock-autostart.XXXXXX")"
      echo "$mock_adir" >> "$MOCK_LEAK_DIRS_FILE"
      mock_tdir="$(mktemp -d "${TMPDIR:-/tmp}/mock-soaktmp.XXXXXX")"
      rmdir "$mock_tdir"
      cat <<CLEANUP_EOF > "$RELAY_SOAK_CLEANUP_RECEIPT"
soak_autostart_pid=$cpid
soak_autostart_dir=$mock_adir
soak_tmp_dir=$mock_tdir
CLEANUP_EOF
      exit 0
    fi
    ;;

  *)
    echo "Unknown mock scenario: $scenario" >&2
    exit 1
    ;;
esac
EOF
  chmod +x "$fake_bin/cargo"

  # --- Positive Self-Test: execute REAL wrapper with mock cargo ---
  export MOCK_CARGO_SCENARIO=positive
  export MOCK_CARGO_ARGV_LOG="$mock_argv_log"
  export MOCK_LEAK_PIDS_FILE="$mock_leak_pids"
  export MOCK_LEAK_DIRS_FILE="$mock_leak_dirs"

  local pos_ev_dir="$test_tmp/ev_pos"
  PATH="$fake_bin:$PATH" \
  CANDIDATE_HEAD_SHA="0123456789abcdef0123456789abcdef01234567" \
  BASE_SHA="11a238db1723ee850e078885effe9926cbece888" \
  GITHUB_WORKFLOW="relay-ci-test" \
  GITHUB_RUN_ID="999999" \
  GITHUB_RUN_ATTEMPT="1" \
  GITHUB_JOB="gates-test" \
  bash "$REAL_SCRIPT_PATH" "$pos_ev_dir"

  # Validate Step 1 and Step 2 invoked argv
  local step1_argv step2_argv
  step1_argv="$(sed -n '1p' "$mock_argv_log")"
  step2_argv="$(sed -n '2p' "$mock_argv_log")"

  if [[ "$step1_argv" != *" -- durable:: raft_cluster:: raft_persistence:: backup::"* ]]; then
    echo "!! Self-test failed: Step 1 did not invoke cargo with filters after '--': '$step1_argv'" >&2
    exit 1
  fi
  if [[ "$step2_argv" != *" -- --ignored --nocapture stability_relay_competitor_performance_bounded_soak::relay_competitor_performance_bounded_soak"* ]]; then
    echo "!! Self-test failed: Step 2 did not invoke cargo with --ignored after '--': '$step2_argv'" >&2
    exit 1
  fi
  echo ">> Check 1 (Production Cargo argv format): PASS"

  # Validate produced receipt.json schema, types, exact SHAs, and booleans
  python3 - "$pos_ev_dir/receipt.json" "$pos_ev_dir" << 'PYEOF'
import sys, json, os

receipt_path = sys.argv[1]
ev_dir = sys.argv[2]

with open(receipt_path) as f:
    r = json.load(f)

# Assert schema version
assert r["schema_version"] == "1.0", f"Wrong schema version: {r['schema_version']}"

# Assert SHA-40 format
for sha_key in ["candidate_head_sha", "actual_checkout_sha", "base_sha"]:
    val = r.get(sha_key, "")
    assert len(val) == 40 and all(c in "0123456789abcdef" for c in val) and val != "0"*40, f"Invalid {sha_key}: {val}"

assert r["candidate_head_sha"] == "0123456789abcdef0123456789abcdef01234567"
assert r["base_sha"] == "11a238db1723ee850e078885effe9926cbece888"

# Assert typed booleans
assert isinstance(r["merge_tested"], bool), "merge_tested must be a boolean"
assert r["merge_tested"] is True, "merge_tested expected to be True when candidate != checkout"

cleanup = r.get("cleanup", {})
assert isinstance(cleanup.get("autostart_pid_stopped"), bool), "autostart_pid_stopped must be bool"
assert cleanup["autostart_pid_stopped"] is True, "autostart_pid_stopped must be True"
assert isinstance(cleanup.get("autostart_dir_cleaned"), bool), "autostart_dir_cleaned must be bool"
assert cleanup["autostart_dir_cleaned"] is True, "autostart_dir_cleaned must be True"
assert isinstance(cleanup.get("soak_tmp_dir_cleaned"), bool), "soak_tmp_dir_cleaned must be bool"
assert cleanup["soak_tmp_dir_cleaned"] is True, "soak_tmp_dir_cleaned must be True"
assert isinstance(cleanup.get("wrapper_tmp_cleaned"), bool), "wrapper_tmp_cleaned must be bool"
assert cleanup["wrapper_tmp_cleaned"] is True, "wrapper_tmp_cleaned must be True"
assert cleanup.get("status") == "verified", "cleanup status must be verified"

# Assert module counts
modules = r.get("modules", {})
assert modules["durable"]["passed"] == 6
assert modules["raft_cluster"]["passed"] == 3
assert modules["raft_persistence"]["passed"] == 2
assert modules["backup"]["passed"] == 4

# Assert soak metrics
soak = r.get("soak_harness", {})
assert soak["status"] == "passed"
assert soak["total_ops"] == 9258
assert soak["errors"] == 0
assert soak["steady_drift_pct"] == 0

# Assert raw digests exist and are 64 hex
digests = r.get("raw_artifact_digests", {})
assert len(digests) == 7, f"Expected 7 digests, got {len(digests)}"
for k, d in digests.items():
    assert len(d) == 64 and all(c in "0123456789abcdef" for c in d), f"Invalid sha256 digest for {k}: {d}"

# Assert terminal result
assert r.get("terminal_result") == "passed", f"Wrong terminal result: {r.get('terminal_result')}"

print("receipt_json_validation_pass")
PYEOF
  echo ">> Check 2 (Real receipt.json serializer & typed booleans): PASS"

  # --- Negative Self-Tests against the REAL wrapper ---

  # Negative 1: Zero tests in Step 1
  if PATH="$fake_bin:$PATH" MOCK_CARGO_SCENARIO=zero_tests \
     CANDIDATE_HEAD_SHA="0123456789abcdef0123456789abcdef01234567" \
     BASE_SHA="11a238db1723ee850e078885effe9926cbece888" \
     bash "$REAL_SCRIPT_PATH" "$test_tmp/ev_neg_zero" >"$test_tmp/out_zero" 2>"$test_tmp/err_zero"; then
    echo "!! Self-test failed: zero tests scenario should have failed" >&2
    exit 1
  fi
  if ! grep -q "zero tests were executed" "$test_tmp/err_zero"; then
    echo "!! Self-test failed: missing 'zero tests were executed' in error output" >&2
    exit 1
  fi
  echo ">> Check 3 (Zero test execution detection): PASS"

  # Negative 2: Ignored only in Step 2
  if PATH="$fake_bin:$PATH" MOCK_CARGO_SCENARIO=ignored_only \
     CANDIDATE_HEAD_SHA="0123456789abcdef0123456789abcdef01234567" \
     BASE_SHA="11a238db1723ee850e078885effe9926cbece888" \
     bash "$REAL_SCRIPT_PATH" "$test_tmp/ev_neg_ignored" >"$test_tmp/out_ignored" 2>"$test_tmp/err_ignored"; then
    echo "!! Self-test failed: ignored-only scenario should have failed" >&2
    exit 1
  fi
  if ! grep -q "did not pass" "$test_tmp/err_ignored"; then
    echo "!! Self-test failed: missing 'did not pass' in error output" >&2
    exit 1
  fi
  echo ">> Check 4 (Ignored-only detection): PASS"

  # Negative 3: Missing module (backup::) in Step 1
  if PATH="$fake_bin:$PATH" MOCK_CARGO_SCENARIO=missing_module \
     CANDIDATE_HEAD_SHA="0123456789abcdef0123456789abcdef01234567" \
     BASE_SHA="11a238db1723ee850e078885effe9926cbece888" \
     bash "$REAL_SCRIPT_PATH" "$test_tmp/ev_neg_missing_mod" >"$test_tmp/out_missing_mod" 2>"$test_tmp/err_missing_mod"; then
    echo "!! Self-test failed: missing module scenario should have failed" >&2
    exit 1
  fi
  if ! grep -q "required modules had zero passed tests" "$test_tmp/err_missing_mod"; then
    echo "!! Self-test failed: missing expected error for missing module" >&2
    exit 1
  fi
  echo ">> Check 5 (Missing module detection): PASS"

  # Negative 4: Missing raw server log in Step 3
  if PATH="$fake_bin:$PATH" MOCK_CARGO_SCENARIO=missing_raw_log \
     CANDIDATE_HEAD_SHA="0123456789abcdef0123456789abcdef01234567" \
     BASE_SHA="11a238db1723ee850e078885effe9926cbece888" \
     bash "$REAL_SCRIPT_PATH" "$test_tmp/ev_neg_missing_raw" >"$test_tmp/out_missing_raw" 2>"$test_tmp/err_missing_raw"; then
    echo "!! Self-test failed: missing raw log scenario should have failed" >&2
    exit 1
  fi
  if ! grep -q "Required raw evidence file missing or empty" "$test_tmp/err_missing_raw"; then
    echo "!! Self-test failed: missing expected error for missing raw log" >&2
    exit 1
  fi
  echo ">> Check 6 (Missing raw server log detection): PASS"

  # Negative 5: Missing latency log in Step 3
  if PATH="$fake_bin:$PATH" MOCK_CARGO_SCENARIO=missing_latency_log \
     CANDIDATE_HEAD_SHA="0123456789abcdef0123456789abcdef01234567" \
     BASE_SHA="11a238db1723ee850e078885effe9926cbece888" \
     bash "$REAL_SCRIPT_PATH" "$test_tmp/ev_neg_missing_lat" >"$test_tmp/out_missing_lat" 2>"$test_tmp/err_missing_lat"; then
    echo "!! Self-test failed: missing latency log scenario should have failed" >&2
    exit 1
  fi
  if ! grep -q "Required raw evidence file missing or empty" "$test_tmp/err_missing_lat"; then
    echo "!! Self-test failed: missing expected error for missing latency log" >&2
    exit 1
  fi
  echo ">> Check 7 (Missing latency log detection): PASS"

  # Negative 6: Malformed metrics in Step 4
  if PATH="$fake_bin:$PATH" MOCK_CARGO_SCENARIO=malformed_metrics \
     CANDIDATE_HEAD_SHA="0123456789abcdef0123456789abcdef01234567" \
     BASE_SHA="11a238db1723ee850e078885effe9926cbece888" \
     bash "$REAL_SCRIPT_PATH" "$test_tmp/ev_neg_malformed" >"$test_tmp/out_malformed" 2>"$test_tmp/err_malformed"; then
    echo "!! Self-test failed: malformed metrics scenario should have failed" >&2
    exit 1
  fi
  if ! grep -q "Soak reported non-zero errors" "$test_tmp/err_malformed"; then
    echo "!! Self-test failed: missing expected error for malformed metrics" >&2
    exit 1
  fi
  echo ">> Check 8 (Malformed metrics detection): PASS"

  # Negative 7: Invalid candidate SHA
  if PATH="$fake_bin:$PATH" MOCK_CARGO_SCENARIO=positive \
     CANDIDATE_HEAD_SHA="not-a-sha" \
     BASE_SHA="11a238db1723ee850e078885effe9926cbece888" \
     bash "$REAL_SCRIPT_PATH" "$test_tmp/ev_neg_sha" >"$test_tmp/out_sha" 2>"$test_tmp/err_sha"; then
    echo "!! Self-test failed: invalid SHA scenario should have failed" >&2
    exit 1
  fi
  if ! grep -q "Invalid or missing candidate_head_sha" "$test_tmp/err_sha"; then
    echo "!! Self-test failed: missing expected error for invalid SHA" >&2
    exit 1
  fi
  echo ">> Check 9 (Invalid/missing SHA-40 fail-closed): PASS"

  # Negative 8: Cleanup failure (PID still alive)
  python3 -c "import time; time.sleep(30)" &
  local leak_test_pid=$!
  echo "$leak_test_pid" >> "$mock_leak_pids"

  if PATH="$fake_bin:$PATH" MOCK_CARGO_SCENARIO=cleanup_failure_pid \
     MOCK_ALIVE_PID="$leak_test_pid" \
     CANDIDATE_HEAD_SHA="0123456789abcdef0123456789abcdef01234567" \
     BASE_SHA="11a238db1723ee850e078885effe9926cbece888" \
     bash "$REAL_SCRIPT_PATH" "$test_tmp/ev_neg_cleanup_pid" >"$test_tmp/out_cleanup_pid" 2>"$test_tmp/err_cleanup_pid"; then
    kill "$leak_test_pid" 2>/dev/null || true
    echo "!! Self-test failed: cleanup failure (PID) scenario should have failed" >&2
    exit 1
  fi
  kill "$leak_test_pid" 2>/dev/null || true
  if ! grep -q "soak autostart PID .* is still running" "$test_tmp/err_cleanup_pid"; then
    echo "!! Self-test failed: missing expected error for running PID" >&2
    exit 1
  fi
  echo ">> Check 10 (Post-cleanup unstopped PID detection): PASS"

  # Negative 9: Cleanup failure (directory still exists)
  if PATH="$fake_bin:$PATH" MOCK_CARGO_SCENARIO=cleanup_failure_dir \
     CANDIDATE_HEAD_SHA="0123456789abcdef0123456789abcdef01234567" \
     BASE_SHA="11a238db1723ee850e078885effe9926cbece888" \
     bash "$REAL_SCRIPT_PATH" "$test_tmp/ev_neg_cleanup_dir" >"$test_tmp/out_cleanup_dir" 2>"$test_tmp/err_cleanup_dir"; then
    echo "!! Self-test failed: cleanup failure (dir) scenario should have failed" >&2
    exit 1
  fi
  if ! grep -q "soak autostart dir .* still exists" "$test_tmp/err_cleanup_dir"; then
    echo "!! Self-test failed: missing expected error for existing dir" >&2
    exit 1
  fi
  echo ">> Check 11 (Post-cleanup leaked directory detection): PASS"

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
  if (( ${#TRACKED_PIDS[@]} > 0 )); then
    for pid in "${TRACKED_PIDS[@]}"; do
      if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
      fi
    done
  fi
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

# Candidate PR head SHA resolution: fallback only when empty
CANDIDATE_HEAD_SHA="${CANDIDATE_HEAD_SHA:-}"
if [[ -z "$CANDIDATE_HEAD_SHA" ]]; then
  if git rev-parse -q --verify HEAD^2 >/dev/null 2>&1; then
    CANDIDATE_HEAD_SHA="$(git rev-parse HEAD^2)"
  else
    CANDIDATE_HEAD_SHA="$ACTUAL_CHECKOUT_SHA"
  fi
fi

# Base SHA resolution: fallback only when empty
BASE_SHA="${BASE_SHA:-}"
if [[ -z "$BASE_SHA" ]]; then
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

VERIFY_SCRIPT_SHA256="$(file_sha256 "$REAL_SCRIPT_PATH")"
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
CLEANUP_VALUES="$(python3 - "$SOAK_CLEANUP" << 'PYEOF'
import sys

pid, adir, tdir = '', '', ''
with open(sys.argv[1]) as f:
    for line in f:
        line = line.strip()
        if line.startswith('soak_autostart_pid='):
            pid = line.split('=', 1)[1].strip()
        elif line.startswith('soak_autostart_dir='):
            adir = line.split('=', 1)[1].strip()
        elif line.startswith('soak_tmp_dir='):
            tdir = line.split('=', 1)[1].strip()
print(f"{pid} {adir} {tdir}")
PYEOF
)"

read -r SOAK_PID SOAK_ADIR SOAK_TDIR <<<"$CLEANUP_VALUES"

if [[ -z "$SOAK_PID" || ! "$SOAK_PID" =~ ^[0-9]+$ ]] || (( SOAK_PID <= 0 )); then
  echo "!! Cleanup verification failed: soak_autostart_pid missing or non-positive: '$SOAK_PID'" >&2
  exit 1
fi

if [[ -z "$SOAK_ADIR" ]]; then
  echo "!! Cleanup verification failed: soak_autostart_dir missing in cleanup receipt" >&2
  exit 1
fi

if [[ -z "$SOAK_TDIR" ]]; then
  echo "!! Cleanup verification failed: soak_tmp_dir missing in cleanup receipt" >&2
  exit 1
fi

AUTOSTART_PID_STOPPED=false
if kill -0 "$SOAK_PID" 2>/dev/null; then
  echo "!! Cleanup verification failed: soak autostart PID $SOAK_PID is still running" >&2
  exit 1
fi
AUTOSTART_PID_STOPPED=true

AUTOSTART_DIR_CLEANED=false
if [[ -e "$SOAK_ADIR" ]]; then
  echo "!! Cleanup verification failed: soak autostart dir $SOAK_ADIR still exists" >&2
  exit 1
fi
AUTOSTART_DIR_CLEANED=true

SOAK_TMP_DIR_CLEANED=false
if [[ -e "$SOAK_TDIR" ]]; then
  echo "!! Cleanup verification failed: soak tmp dir $SOAK_TDIR still exists" >&2
  exit 1
fi
SOAK_TMP_DIR_CLEANED=true

# Verify wrapper work tmp directory removed
if [[ -d "$WORK_TMP" ]]; then
  rm -rf "$WORK_TMP"
fi
WORK_TMP_CLEANED=false
if [[ ! -d "$WORK_TMP" ]]; then
  WORK_TMP_CLEANED=true
fi

if [[ "$AUTOSTART_PID_STOPPED" != "true" || "$AUTOSTART_DIR_CLEANED" != "true" || "$SOAK_TMP_DIR_CLEANED" != "true" || "$WORK_TMP_CLEANED" != "true" ]]; then
  echo "!! Cleanup verification incomplete" >&2
  exit 1
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

export CANDIDATE_HEAD_SHA ACTUAL_CHECKOUT_SHA BASE_SHA MERGE_TESTED
export TIMESTAMP_START TIMESTAMP_END VERIFY_SCRIPT_SHA256 SOAK_SCRIPT_SHA256
export STEP1_CMD STEP2_CMD
export STEP1_LOG_SHA256 STEP2_LOG_SHA256 RELAY_LOG_SHA256 LATENCY_A_SHA256 LATENCY_B_SHA256 SOAK_REPORT_SHA256 SOAK_CLEANUP_SHA256
export DURABLE_COUNT RAFT_CLUSTER_COUNT RAFT_PERSIST_COUNT BACKUP_COUNT
export SOAK_PID AUTOSTART_PID_STOPPED AUTOSTART_DIR_CLEANED SOAK_TMP_DIR_CLEANED WORK_TMP_CLEANED
export GITHUB_WORKFLOW="${GITHUB_WORKFLOW:-local}"
export GITHUB_RUN_ID="${GITHUB_RUN_ID:-local}"
export GITHUB_RUN_ATTEMPT="${GITHUB_RUN_ATTEMPT:-1}"
export GITHUB_JOB="${GITHUB_JOB:-gates}"

generate_receipt "$RECEIPT_FILE" "$SOAK_REPORT"

echo ">> Wrote verification receipt to $RECEIPT_FILE"
echo "=== All restart, failover, and soak verification checks PASSED ==="
