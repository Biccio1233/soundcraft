#!/usr/bin/env bash
# The `cargo xtask ci` gate for macOS machines with third-party plug-ins installed: the same steps,
# with the tests listed in scripts/macos-plugin-test-skips.txt skipped (see that file for why).
# The authoritative full gate is `docker compose run --rm ci`.
set -euo pipefail
cd "$(dirname "$0")/.."

skips=()
while IFS= read -r line; do
  case "$line" in ''|'#'*) continue ;; esac
  skips+=(--skip "$line")
done < scripts/macos-plugin-test-skips.txt

step() {
  local name="$1"; shift
  if "$@"; then echo "ok    $name"; else echo "FAIL  $name"; exit 1; fi
}

step fmt cargo fmt --all -- --check
step clippy cargo clippy --workspace --all-targets -- -D warnings
step test cargo test --workspace -- "${skips[@]}"
step assets cargo xtask assets
step layers cargo xtask layers
step wasm cargo xtask wasm
