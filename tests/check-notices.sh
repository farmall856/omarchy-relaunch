#!/usr/bin/env bash
# THIRD-PARTY-NOTICES.md must list exactly the workspace's direct third-party
# dependencies, at their locked version and declared license.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NOTICES="$ROOT/THIRD-PARTY-NOTICES.md"

expected="$(cd "$ROOT/core" && cargo metadata --format-version 1 --locked --offline | jq -r '
  (.workspace_members) as $ws
  | (.packages | map({key: .id, value: .}) | from_entries) as $pk
  | (.resolve.nodes | map({key: .id, value: .}) | from_entries) as $node
  | [$ws[] | $node[.].deps[] | .pkg | select($pk[.].source != null) | $pk[.]]
  | unique_by([.name, .version])[] | "\(.name) \(.version) \(.license)"' | sort)"
# shellcheck disable=SC2016 # the backticks are the table's literal code spans
listed="$(sed -nE 's/^\| `([^`]+)` \| ([^ |]+) \| ([^|]+[^ |]) +\|.*/\1 \2 \3/p' "$NOTICES" | sort)"

if [[ "$expected" != "$listed" ]]; then
  printf 'THIRD-PARTY-NOTICES.md is out of date (< expected from cargo metadata, > listed):\n' >&2
  diff <(printf '%s\n' "$expected") <(printf '%s\n' "$listed") >&2 || true
  exit 1
fi
[[ -s "$ROOT/LICENSES/freedesktop-desktop-entry/LICENSE" ]] ||
  { printf 'missing LICENSES/freedesktop-desktop-entry/LICENSE (MPL-2.0 text)\n' >&2; exit 1; }
printf 'notices: %s direct dependencies listed\n' "$(printf '%s\n' "$expected" | wc -l)"
