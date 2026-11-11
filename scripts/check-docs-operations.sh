#!/usr/bin/env bash
# Checks the operator docs (install and runbook) and their links.
# Run from the repository root: bash scripts/check-docs-operations.sh
set -euo pipefail

NEW_DOCS=(docs/operations/install.md docs/operations/runbook.md)
LINKED_FILES=(docs/operations/install.md docs/operations/runbook.md docs/operations/README.md README.md)
RUNBOOK=docs/operations/runbook.md
ALLOWED_VERBS="llm upgrade issue k8s dockerfile spec backup"
failures=0

fail() {
  echo "FAIL: $*" >&2
  failures=$((failures + 1))
}

# 1. The two new documents exist.
for f in "${NEW_DOCS[@]}"; do
  if [ -f "$f" ]; then
    echo "ok: $f exists"
  else
    fail "$f does not exist"
  fi
done

# 2. The runbook has the seven sections, each with the four bullets.
if [ -f "$RUNBOOK" ]; then
  for h in "Node down" "Leader failover" "Disk full" "Backup" "Restore" "Drain" "Upgrade"; do
    if ! grep -qx "## $h" "$RUNBOOK"; then
      fail "$RUNBOOK lacks the heading '## $h'"
      continue
    fi
    block=$(awk -v h="## $h" '
      $0 == h { on = 1; next }
      /^## / { on = 0 }
      on { print }' "$RUNBOOK")
    for b in Symptom Check Command Expected; do
      if ! grep -q "^- $b:" <<<"$block"; then
        fail "section '## $h' lacks the bullet '- $b:'"
      fi
    done
  done
fi

# 3. The two index pages link the new documents.
if [ -f docs/operations/README.md ]; then
  grep -q "(install.md)" docs/operations/README.md || fail "docs/operations/README.md lacks (install.md)"
  grep -q "(runbook.md)" docs/operations/README.md || fail "docs/operations/README.md lacks (runbook.md)"
fi
if [ -f README.md ]; then
  grep -q "docs/operations/install.md" README.md || fail "README.md lacks the link docs/operations/install.md"
fi

# 4. Relative markdown links resolve, relative to the file that holds them.
for file in "${LINKED_FILES[@]}"; do
  [ -f "$file" ] || continue
  dir=$(dirname "$file")
  while IFS= read -r target; do
    case "$target" in
      http*|'#'*|mailto:*|'') continue ;;
    esac
    path=${target%%#*}
    if [ ! -e "$dir/$path" ]; then
      fail "$file: link '$target' does not resolve to $dir/$path"
    fi
  done < <(grep -o '\]([^)]*)' "$file" | sed 's/^](//; s/)$//' || true)
done

# 5. Repository paths written in backticks in the new documents exist.
for file in "${NEW_DOCS[@]}"; do
  [ -f "$file" ] || continue
  while IFS= read -r p; do
    if [ ! -e "$p" ]; then
      fail "$file: path '$p' does not exist"
    fi
  done < <(grep -o '`[^`]*`' "$file" | tr -d '`' \
            | grep -E '^(k8s|docs|scripts|crates|\.github)/[A-Za-z0-9_./-]+$' | sort -u || true)
done

# 6. Every `relay <verb>` in the new documents is a verb of relay --help.
for file in "${NEW_DOCS[@]}"; do
  [ -f "$file" ] || continue
  while IFS= read -r verb; do
    case " $ALLOWED_VERBS " in
      *" $verb "*) ;;
      *) fail "$file: 'relay $verb' is not a verb of relay --help" ;;
    esac
  done < <( { grep -o '`relay [^`]*`' "$file" | tr -d '`'; grep -E '^[[:space:]]*(\$ )?relay ' "$file"; } \
            | awk '{ for (i = 1; i < NF; i++) if ($i == "relay") { v = $(i + 1); if (v !~ /^-/) print v; break } }' \
            | sort -u || true)
done

if [ "$failures" -ne 0 ]; then
  echo "check-docs-operations: $failures failure(s)" >&2
  exit 1
fi
echo "check-docs-operations: all checks pass"
