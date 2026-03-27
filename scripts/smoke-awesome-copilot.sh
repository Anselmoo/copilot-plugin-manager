#!/usr/bin/env bash
set -euo pipefail

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

cd "$tmpdir"

echo "Creating temporary project in $tmpdir"
uv run --project "$repo_root" cpm init --name awesome-copilot-smoke >/dev/null
uv run --project "$repo_root" cpm add \
  https://github.com/github/awesome-copilot/tree/main/plugins/partners \
  --plugin

test -f cpm.toml
test -f cpm.lock
test -d .github/plugins/partners

echo "awesome-copilot smoke succeeded"
