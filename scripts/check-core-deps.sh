#!/usr/bin/env bash
# Fails if any crate under core/ depends (directly or transitively) on Tauri or a webview stack.
# core/ must stay UI-agnostic so it can move to a separate process or a CLI. Run in CI and pre-commit.
set -euo pipefail
cd "$(dirname "$0")/.."

forbidden='^(tauri|tauri-[a-z-]+|wry|tao|webview2-com)$'
status=0
for manifest in core/*/Cargo.toml; do
  crate=$(sed -n 's/^name = "\(.*\)"/\1/p' "$manifest" | head -1)
  hits=$(cargo tree -p "$crate" --edges normal,build --prefix none --format '{p}' 2>/dev/null \
    | awk '{print $1}' | sort -u | grep -E "$forbidden" || true)
  if [[ -n "$hits" ]]; then
    echo "FAIL: $crate depends on: $(echo "$hits" | tr '\n' ' ')"
    status=1
  else
    echo "ok:   $crate"
  fi
done
exit $status
