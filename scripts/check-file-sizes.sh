#!/usr/bin/env bash
# Fail when a Rust source file outgrows its line budget.
#
# Default limit: 1000 lines. Files that predate the limit carry a ratchet
# budget in scripts/file-size-budgets.txt pinned to their current size, so
# they can only shrink. Once a file drops to the default limit, its entry
# must be removed.
set -euo pipefail
cd "$(dirname "$0")/.."

limit=1000
budgets=scripts/file-size-budgets.txt
status=0

while IFS= read -r file; do
  [ -f "$file" ] || continue
  lines=$(wc -l <"$file")
  budget=$(awk -v f="$file" '$1 == f { print $2 }' "$budgets")

  if [ -n "$budget" ]; then
    if [ "$budget" -le "$limit" ]; then
      echo "stale budget: $file is budgeted at $budget (<= $limit); remove its entry from $budgets"
      status=1
    elif [ "$lines" -gt "$budget" ]; then
      echo "too large: $file has $lines lines (ratchet budget $budget); split it instead of growing it"
      status=1
    elif [ "$lines" -lt "$budget" ]; then
      echo "stale budget: $file shrank to $lines lines; lower its budget in $budgets (or remove the entry at <= $limit)"
      status=1
    fi
  elif [ "$lines" -gt "$limit" ]; then
    echo "too large: $file has $lines lines (limit $limit); split it, don't add a budget"
    status=1
  fi
done < <(git ls-files '*.rs')

# Every budgeted path must still exist.
while read -r file _; do
  case "$file" in ''|\#*) continue ;; esac
  if [ ! -f "$file" ]; then
    echo "stale budget: $file no longer exists; remove its entry from $budgets"
    status=1
  fi
done <"$budgets"

if [ "$status" -eq 0 ]; then
  echo "All file sizes within budget."
fi
exit "$status"
