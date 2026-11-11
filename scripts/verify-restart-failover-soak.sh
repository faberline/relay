#!/usr/bin/env bash
# Restart, failover and soak verification for relay (issue #48).
#
# Runs the local restart and persistence test modules and the bounded
# work-queue soak as one command. A module that runs 0 tests fails the gate.
# Temp files are removed on success and failure; scripts/soak.sh stops the
# relay process it started.
#
# Environment:
#   RFS_MODULES   space-separated test modules
#                 (default: durable raft_cluster raft_persistence backup)
#   RFS_SKIP_SOAK=1  skip scripts/soak.sh (used by the offline tests)

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

require() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "!! required command not found: $1" >&2
    exit 1
  }
}

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rfs-soak.XXXXXX")"

cleanup() {
  local ec=$?
  trap - EXIT INT TERM
  rm -rf "$TMP_DIR"
  exit "$ec"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

require cargo

MODULES=(${RFS_MODULES:-durable raft_cluster raft_persistence backup})
(( ${#MODULES[@]} > 0 )) || {
  echo "!! RFS_MODULES is empty" >&2
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

if [[ "${RFS_SKIP_SOAK:-0}" != "1" ]]; then
  require curl
  require jq
  require ps
  soak_log="$TMP_DIR/soak.log"
  soak_status=0
  echo ">> soak: RELAY_SOAK_AUTOSTART=1 bash scripts/soak.sh"
  RELAY_SOAK_AUTOSTART=1 bash scripts/soak.sh >"$soak_log" 2>&1 || soak_status=$?
  cat "$soak_log"
  if (( soak_status != 0 )); then
    echo "!! soak failed (exit ${soak_status})" >&2
    exit 1
  fi
  total_ops="$(sed -n 's/^   total_ops: *\([0-9][0-9]*\)$/\1/p' "$soak_log" | tail -n 1)"
  errors="$(sed -n 's/^   errors: *\([0-9][0-9]*\)$/\1/p' "$soak_log" | tail -n 1)"
  if [[ -z "$total_ops" ]] || (( total_ops == 0 )); then
    echo "!! soak: total_ops is 0 or missing" >&2
    exit 1
  fi
  if [[ "$errors" != "0" ]]; then
    echo "!! soak: errors=${errors:-missing}" >&2
    exit 1
  fi
  grep -q '^>> Relay bounded work-queue soak PASS$' "$soak_log" || {
    echo "!! soak: PASS line missing" >&2
    exit 1
  }
  echo ">> soak: total_ops ${total_ops}, errors ${errors}"
fi

echo ">> restart-failover-soak: passed"
