#!/usr/bin/env bash
# Measure the running Touch Bar renderer: resident memory, and CPU over a
# window for the renderer process alone and for the whole t1-touchbar service
# (launcher, renderer and its helpers such as pactl and dbus-monitor).
# Usage: tools/perf.sh [seconds]   (default 60; leave the bar alone meanwhile)
set -euo pipefail

seconds="${1:-60}"
cgroup="/sys/fs/cgroup$(systemctl --user show -P ControlGroup t1-touchbar)"
pid="$(systemctl --user show -P MainPID t1-touchbar)"
renderer="$(pgrep -P "$pid" | head -n1)"
[[ -n "$renderer" ]] || { echo "no renderer process under t1-touchbar" >&2; exit 1; }

ticks() { awk '{ print $14 + $15 }' "/proc/$renderer/stat"; }      # utime + stime
usage() { awk '/^usage_usec/ { print $2 }' "$cgroup/cpu.stat"; }   # whole service

t0="$(ticks)"; u0="$(usage)"; s0="$(date +%s.%N)"
sleep "$seconds"
t1="$(ticks)"; u1="$(usage)"; s1="$(date +%s.%N)"

hz="$(getconf CLK_TCK)"
echo "renderer: $(readlink "/proc/$renderer/exe")"
awk '/^VmRSS/ { printf "RSS: %.1f MB\n", $2 / 1024 }' "/proc/$renderer/status"
awk -v t="$((t1 - t0))" -v hz="$hz" -v s0="$s0" -v s1="$s1" \
  'BEGIN { printf "renderer CPU: %.2f%% over %.0f s\n", 100 * t / hz / (s1 - s0), s1 - s0 }'
awk -v u="$((u1 - u0))" -v s0="$s0" -v s1="$s1" \
  'BEGIN { printf "service CPU: %.2f%%\n", 100 * u / 1e6 / (s1 - s0) }'
