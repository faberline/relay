#!/usr/bin/env bash
# Tests for scripts/ci/check-acceptance-count.sh: the gke-acceptance run fails
# unless the Job succeeded and the test log shows at least one passed test.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/check-acceptance-count.sh"
failed=0
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# make_evidence <dir> <succeeded> <log line>
make_evidence() {
  local dir="$1" succeeded="$2" line="$3"
  mkdir -p "$dir"
  printf 'status:\n  succeeded: %s\n' "$succeeded" > "$dir/test-job-status.yaml"
  printf '%s\n' "$line" > "$dir/logs-relay-client-abc.txt"
}

# check <name> <expected exit: 0 or nonzero> <dir>
check() {
  local name="$1" expect="$2" dir="$3"
  bash "$script" "$dir" >/dev/null 2>&1
  local code=$?
  if [[ "$expect" == "0" && "$code" -eq 0 ]]; then
    echo "ok $name"
  elif [[ "$expect" == "nonzero" && "$code" -ne 0 ]]; then
    echo "ok $name"
  else
    echo "FAIL $name: expected $expect, got exit $code"
    failed=1
  fi
}

make_evidence "$tmp/a" 1 'test result: ok. 3 passed; 0 failed; 0 ignored'
check passed-tests-and-job-succeeded 0 "$tmp/a"

make_evidence "$tmp/b" 1 'test result: ok. 0 passed; 0 failed; 0 ignored'
check zero-tests-selected-fails nonzero "$tmp/b"

make_evidence "$tmp/c" 0 'test result: ok. 3 passed; 0 failed; 0 ignored'
check job-not-succeeded-fails nonzero "$tmp/c"

exit "$failed"
