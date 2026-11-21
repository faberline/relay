#!/usr/bin/env bash
# Usage: check-acceptance-count.sh <evidence dir>
# Fails unless the gke-test Job succeeded and a test log shows at least one
# passed test. A run that selected 0 tests is not a pass.
set -euo pipefail

die() { echo "::error::$*" >&2; exit 1; }

dir="${1:?usage: check-acceptance-count.sh <evidence dir>}"

status="$dir/test-job-status.yaml"
[[ -f "$status" ]] || die "no test-job-status.yaml in $dir"
succeeded="$(grep -E '^[[:space:]]*succeeded:' "$status" | head -n1 \
  | sed -E 's/.*succeeded:[[:space:]]*//' || true)"
[[ "$succeeded" == "1" ]] || die "test Job did not succeed (succeeded: ${succeeded:-none})"

# Every "test result: ok. <n> passed" line of every Job log; keep the largest n.
counts="$(cat "$dir"/logs-*.txt 2>/dev/null \
  | grep -oE 'test result: ok\. [0-9]+ passed' \
  | sed -E 's/.*ok\. ([0-9]+) passed/\1/' || true)"
max=0
while read -r n; do
  if [[ -n "$n" && "$n" -gt "$max" ]]; then
    max="$n"
  fi
done <<< "$counts"

(( max >= 1 )) || die "no test log shows 'test result: ok. <n> passed' with n >= 1"
echo "acceptance: $max test(s) passed"
