#!/usr/bin/env bash
# Headless check of the desktop client (desktop/check.qml) against a private
# backend: snapshot, updates, a forced resync, a press, a refused press,
# hiding the row, and a backend restart (disconnected state, then a new
# session). Leaves a screenshot of each status in the printed directory.
# Usage: tools/desktop-check.sh   (build first: cargo build --release)
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
bin="${GLANCE_BIN:-$repo/target/release/omarchy-glance}"
work="$(mktemp -d)"
mkdir -p "$work/config/omarchy-glance" "$work/run" "$work/state" "$work/shots"
backend_pid=""
qs_pid=""
cleanup() { kill $backend_pid $qs_pid 2>/dev/null || true; }
trap cleanup EXIT

cat >"$work/config/omarchy-glance/desktop.json" <<EOF
{
  "version": 1,
  "left": [
    { "id": "glance.esc" },
    { "id": "touch", "type": "button", "label": "press", "exec": "touch '$work/pressed'" },
    { "id": "glance.spacer", "size": 20 },
    { "id": "hello", "type": "command", "exec": "echo hello", "interval": 1 }
  ],
  "right": [{ "id": "glance.cpu" }]
}
EOF

start_backend() {
  XDG_CONFIG_HOME="$work/config" XDG_RUNTIME_DIR="$work/run" XDG_STATE_HOME="$work/state" \
    "$bin" backend >>"$work/backend.log" 2>&1 &
  backend_pid=$!
}
# wait_for <count> <pattern>: until the qs log has that many matching lines
wait_for() {
  for _ in $(seq 100); do
    (( $(grep -c -- "$2" "$work/qs.log" || true) >= $1 )) && return 0
    sleep 0.1
  done
  echo "timed out waiting for $1× '$2'" >&2; cat "$work/qs.log" >&2; exit 1
}
fail() { echo "FAIL: $*" >&2; cat "$work/qs.log" >&2; exit 1; }

start_backend
for _ in $(seq 50); do [[ -S "$work/run/omarchy-glance/backend.sock" ]] && break; sleep 0.1; done

env -u WAYLAND_DISPLAY QT_QPA_PLATFORM=offscreen GLANCE_SOCKET="$work/run/omarchy-glance/backend.sock" \
  GLANCE_SHOTS="$work/shots" qs --no-color -p "$repo/desktop/check.qml" >"$work/qs.log" 2>&1 &
qs_pid=$!

wait_for 1 "check: status ready"
wait_for 1 "check: text default.left.3 hello"
wait_for 1 "check: failed default.left.99 unknown_widget"
wait_for 1 "check: view toggled"
sleep 0.3
kill "$backend_pid"; wait "$backend_pid" 2>/dev/null || true
wait_for 1 "check: status offline"
sleep 0.5
start_backend
wait_for 2 "check: status ready"
sleep 0.5 # the last screenshot

grep -q "default.left.0=esc(unsupported)" "$work/qs.log" || fail "Esc should be unsupported on the desktop"
grep -q "forcing a gap" "$work/qs.log" || fail "no forced gap"
(( $(grep -c "check: snapshot rev" "$work/qs.log") >= 3 )) || fail "the gap didn't resync (expected snapshots: first, resync, after restart)"
[[ -e "$work/pressed" ]] || fail "the press didn't run the button's action"
if grep "glance: backend error" "$work/qs.log" | grep -v "unknown_widget no widget default.left.99"; then fail "unexpected backend errors"; fi
sessions="$(grep "check: status ready" "$work/qs.log" | sed 's/.*session \([^ ]*\).*/\1/' | sort -u | wc -l)"
(( sessions == 2 )) || fail "expected a new session after the restart"
grep "check: " "$work/qs.log" | sed 's/^.*check: /  /'
echo "ok; screenshots in $work/shots"
