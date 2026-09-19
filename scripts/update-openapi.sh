#!/usr/bin/env sh
# Copyright 2026 Blackboard Studios
# SPDX-License-Identifier: Apache-2.0

set -eu

mode=${1:---check}
case "$mode" in
  --check | --update) ;;
  *)
    echo "usage: scripts/update-openapi.sh [--check|--update]" >&2
    exit 2
    ;;
esac

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
snapshot="$repository_root/openapi/hamstik-v1.json"
live_url="https://hamstik.com/api/v1/openapi.json"
download=$(mktemp "${TMPDIR:-/tmp}/hamstik-openapi.XXXXXX")
trap 'rm -f -- "$download"' EXIT HUP INT TERM

curl --fail --silent --show-error --location "$live_url" --output "$download"

if command -v jq >/dev/null 2>&1; then
  jq -e '.openapi and .paths and .components.schemas' "$download" >/dev/null
else
  echo "jq is required to validate the downloaded OpenAPI document" >&2
  exit 1
fi

if cmp -s "$download" "$snapshot"; then
  echo "OpenAPI snapshot matches $live_url"
  exit 0
fi

if [ "$mode" = "--check" ]; then
  echo "OpenAPI drift detected: $snapshot differs from $live_url" >&2
  echo "Run scripts/update-openapi.sh --update, then update API support and openapi/api-parity.json." >&2
  exit 1
fi

install -m 0644 "$download" "$snapshot"
echo "Updated $snapshot from $live_url"
echo "Run the parity test and classify every operation in openapi/api-parity.json."
