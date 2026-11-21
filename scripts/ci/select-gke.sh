#!/usr/bin/env bash
# Decides whether a pull request starts the GKE suite (raft_failover).
#
# Input:  changed paths on stdin, one per line. Environment: EVENT_NAME.
# Output: two lines on stdout: `gke=true|false` and `reason=<text>`.
# Exit:   0 on a decision; 2 on a usage error (no EVENT_NAME).
#
# Rules, in order. An unknown path selects the full set.
#   1. Any event other than pull_request runs the full set.
#   2. No changed path runs the full set.
#   3. A path that no GKE suite uses is "no GKE"; any other path is "GKE".
#      One "GKE" path makes gke=true. All "no GKE" makes gke=false.
set -uo pipefail

if [[ -z "${EVENT_NAME:-}" ]]; then
  echo "select-gke: EVENT_NAME is not set" >&2
  exit 2
fi

# no_gke <path>: succeeds when no GKE suite uses the path.
# The GKE exceptions come first: `*` in a case pattern also matches `/`.
no_gke() {
  case "$1" in
    # The test-client binary that the GKE Job runs is built from these.
    crates/relay/tests/it/e2e.rs | crates/relay/tests/it/e2e/* | crates/relay/tests/it/main.rs)
      return 1 ;;
    # The selection rule itself and its test.
    scripts/ci/*)
      return 1 ;;
    docs/* | tech-design/* | external-contracts/* | clients/* | scripts/* \
      | .claude/* | .codex/* | *.md \
      | LICENSE | .gitignore | clippy.toml | deny.toml | vat.toml \
      | meter-relay-performance.toml | ddd.toml \
      | crates/*/tests/*)
      return 0 ;;
    *)
      return 1 ;;
  esac
}

paths=()
while IFS= read -r path; do
  [[ -n "$path" ]] && paths+=("$path")
done

if [[ "$EVENT_NAME" != "pull_request" ]]; then
  echo "gke=true"
  echo "reason=event $EVENT_NAME runs the full set"
  exit 0
fi

if [[ "${#paths[@]}" -eq 0 ]]; then
  echo "gke=true"
  echo "reason=no changed paths, full set"
  exit 0
fi

for path in "${paths[@]}"; do
  if ! no_gke "$path"; then
    echo "gke=true"
    echo "reason=$path can affect the GKE suite"
    exit 0
  fi
done

echo "gke=false"
echo "reason=only paths that no GKE suite uses: ${#paths[@]} files"
exit 0
