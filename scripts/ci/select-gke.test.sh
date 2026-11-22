#!/usr/bin/env bash
# Tests for scripts/ci/select-gke.sh: which pull requests start the GKE suite.
# Each case feeds changed paths on stdin and compares the first output line.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/select-gke.sh"
failed=0

# check <name> <EVENT_NAME> <expected first line> <paths...>
check() {
  local name="$1" event="$2" expected="$3"
  shift 3
  local out
  out="$(printf '%s\n' "$@" | EVENT_NAME="$event" bash "$script" 2>/dev/null | head -n1)"
  if [[ "$out" == "$expected" ]]; then
    echo "ok $name"
  else
    echo "FAIL $name: expected '$expected', got '$out'"
    failed=1
  fi
}

# check_empty <name> <EVENT_NAME> <expected first line>: no paths at all.
check_empty() {
  local name="$1" event="$2" expected="$3"
  local out
  out="$(EVENT_NAME="$event" bash "$script" < /dev/null 2>/dev/null | head -n1)"
  if [[ "$out" == "$expected" ]]; then
    echo "ok $name"
  else
    echo "FAIL $name: expected '$expected', got '$out'"
    failed=1
  fi
}

check push-always-full push 'gke=true' docs/a.md
check dispatch-always-full workflow_dispatch 'gke=true' docs/a.md
check_empty empty-diff-full pull_request 'gke=true'
check docs-only pull_request 'gke=false' docs/operations/runbook.md README.md
check config-only pull_request 'gke=false' deny.toml vat.toml
check scripts-only pull_request 'gke=false' scripts/release/promote.sh
check unit-test-file-only pull_request 'gke=false' crates/relay/tests/it/raft_persistence.rs
check e2e-test-file pull_request 'gke=true' crates/relay/tests/it/e2e/raft_failover.rs
check e2e-helper-file pull_request 'gke=true' crates/relay/tests/it/e2e.rs
check it-main pull_request 'gke=true' crates/relay/tests/it/main.rs
check source pull_request 'gke=true' crates/relay-queue/src/server.rs
check manifest pull_request 'gke=true' k8s/base/statefulset.yaml
check dockerfile pull_request 'gke=true' Dockerfile.test
check lockfile pull_request 'gke=true' Cargo.lock
check workflow pull_request 'gke=true' .github/workflows/ci.yml
check select-script pull_request 'gke=true' scripts/ci/select-gke.sh
check unknown-path pull_request 'gke=true' newdir/file.txt
check mixed pull_request 'gke=true' docs/a.md crates/relay/src/tls.rs

# The reason line names the first path that selects the suite.
reason="$(printf '%s\n' docs/a.md crates/relay/src/tls.rs | EVENT_NAME=pull_request bash "$script" 2>/dev/null | sed -n '2p')"
if [[ "$reason" == *crates/relay/src/tls.rs* ]]; then
  echo "ok mixed-reason"
else
  echo "FAIL mixed-reason: got '$reason'"
  failed=1
fi

# Usage error: no EVENT_NAME means exit code 2.
printf '%s\n' docs/a.md | env -u EVENT_NAME bash "$script" >/dev/null 2>&1
rc=$?
if [[ "$rc" -eq 2 ]]; then
  echo "ok usage-error"
else
  echo "FAIL usage-error: expected exit 2, got $rc"
  failed=1
fi

exit "$failed"
