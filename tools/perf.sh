#!/usr/bin/env bash
# Measure the running bar: resident memory and CPU over a window for the
# Touch Bar client and the backend processes alone, and for their whole
# services (t1-touchbar: the t1bridge launcher and the client;
# omarchy-glance: the backend and its helpers such as pactl and dbus-monitor).
# Usage: tools/perf.sh [seconds]   (default 60; leave the bar alone meanwhile)
set -euo pipefail

seconds="${1:-60}"
cgroup() { echo "/sys/fs/cgroup$(systemctl --user show -P ControlGroup "$1")"; }
bar_cg="$(cgroup t1-touchbar)"
backend_cg="$(cgroup omarchy-glance.service)"
client="$(pgrep -P "$(systemctl --user show -P MainPID t1-touchbar)" | head -n1)"
backend="$(systemctl --user show -P MainPID omarchy-glance.service)"
[[ -n "$client" ]] || { echo "no client process under t1-touchbar" >&2; exit 1; }
[[ "$backend" != 0 ]] || { echo "the omarchy-glance backend isn't running" >&2; exit 1; }

ticks() { awk '{ print $14 + $15 }' "/proc/$1/stat"; }           # utime + stime
usage() { awk '/^usage_usec/ { print $2 }' "$1/cpu.stat"; }      # whole service

c0="$(ticks "$client")"; b0="$(ticks "$backend")"; u0="$(usage "$bar_cg")"; v0="$(usage "$backend_cg")"
s0="$(date +%s.%N)"
sleep "$seconds"
c1="$(ticks "$client")"; b1="$(ticks "$backend")"; u1="$(usage "$bar_cg")"; v1="$(usage "$backend_cg")"
s1="$(date +%s.%N)"

hz="$(getconf CLK_TCK)"
pct() { awk -v n="$1" -v d="$2" -v s0="$s0" -v s1="$s1" 'BEGIN { printf "%.2f%%", 100 * n / d / (s1 - s0) }'; }
rss() { awk '/^VmRSS/ { printf "%.1f MB", $2 / 1024 }' "/proc/$1/status"; }
echo "binary: $(readlink "/proc/$client/exe"), window $(awk -v a="$s0" -v b="$s1" 'BEGIN { printf "%.0f", b - a }') s"
echo "client:  RSS $(rss "$client"), CPU $(pct "$((c1 - c0))" "$hz")"
echo "backend: RSS $(rss "$backend"), CPU $(pct "$((b1 - b0))" "$hz")"
echo "t1-touchbar service CPU: $(pct "$((u1 - u0))" 1e6)"
echo "backend service CPU:     $(pct "$((v1 - v0))" 1e6)"
echo "total service CPU:       $(pct "$((u1 - u0 + v1 - v0))" 1e6)"
