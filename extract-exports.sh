#!/usr/bin/env bash
# extract-exports.sh — dump all public items from a crate's dependencies
set -euo pipefail

CRATE="${1:-}"
if [[ -z "$CRATE" ]]; then
  echo "usage: $0 <crate-name>" >&2
  exit 1
fi

# Requires nightly for JSON output
cargo +nightly rustdoc -p "$CRATE" --lib -- -Z unstable-options --output-format json >/dev/null 2>&1

JSON="target/doc/${CRATE//-/_}.json"

jq -r '
  .index | to_entries[] | .value |
  select(.visibility == "public") |
  select(.inner | keys[0] | . as $k | ["struct","trait","enum","function","constant","static","type_alias","macro","union"] | index($k)) |
  (.inner | keys[0]) as $kind |
  "\($kind)\t\(.name)"
' "$JSON" | sort -u