#!/usr/bin/env bash
# Screenshot the desktop row headlessly for a desktop.json, against a private
# backend that reads the real usage records and btop theme.
# Usage: tools/desktop-shot.sh desktop.json out.png [width] [height] [scale]
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
bin="${GLANCE_BIN:-$repo/target/release/omarchy-glance}"
work="$(mktemp -d)"
mkdir -p "$work/config/omarchy-glance" "$work/run"
cp "$1" "$work/config/omarchy-glance/desktop.json"
XDG_CONFIG_HOME="$work/config" XDG_RUNTIME_DIR="$work/run" "$bin" backend >"$work/backend.log" 2>&1 &
backend=$!
trap 'kill $backend 2>/dev/null || true; rm -rf "$work"' EXIT
for _ in $(seq 50); do [[ -S "$work/run/omarchy-glance/backend.sock" ]] && break; sleep 0.1; done
env -u WAYLAND_DISPLAY QT_QPA_PLATFORM=offscreen GLANCE_SOCKET="$work/run/omarchy-glance/backend.sock" \
  GLANCE_OUT="$(realpath -m "$2")" GLANCE_WIDTH="${3:-1440}" GLANCE_HEIGHT="${4:-52}" GLANCE_SCALE="${5:-1}" \
  qs --no-color -p "$repo/desktop/shot.qml" 2>&1 | grep -E "shot:|ERROR|WARN qml|TypeError|ReferenceError" | grep -v "window masks" || true
grep -i "not supported\|unknown widget" "$work/backend.log" || true
