# Release smoke check

Task T11 of [multi-output-tasks.md](multi-output-tasks.md). Run before
tagging a release, on the Touch Bar machine and, where possible, on one
without a Touch Bar. Record each run below.

## Checklist

1. **Build and tests:** `cargo test --release` (backend, Touch Bar client and
   golden images); `tools/desktop-check.sh` (desktop client against a private
   backend: updates, resync, press, refused press, hiding, backend restart).
2. **Install:** `cargo install --path . --root ~/.local`, then
   `omarchy-glance desktop on`; `omarchy-glance status` shows the row
   installed. On the Touch Bar machine also `omarchy-glance on`.
3. **Widgets:** the default `desktop.json` shows media, CPU and memory
   graphs and agents; a config with every desktop widget renders
   (`tools/desktop-shot.sh`); clicks run actions, a held `repeat` button
   repeats, hover highlights.
4. **Reload:** saving `desktop.json` changes the row within a second; an
   invalid file keeps the previous config (log) and leaves the Touch Bar
   alone.
5. **Reconnection:** `systemctl --user restart omarchy-glance` shows the
   disconnected state at most briefly, then both clients come back.
6. **Reservation:** `hyprctl monitors` reserves bar + row at the top; windows
   start below both; `omarchy-glance desktop off` releases the row's space.
7. **Focus:** clicking the row leaves keyboard focus on the active window.
8. **Theme:** switching between a dark and a light theme recolours the row
   (and the light Codex logo) without a backend restart; a transparent bar
   makes the row transparent.
9. **Scaling:** changing the monitor scale keeps the row under the bar.
10. **Monitor changes:** plugging a monitor in and out adds and removes its
    row; `"monitor"` in `desktop.json` limits the row to the named monitors.
11. **Alongside the Touch Bar:** both run on one backend (`omarchy-glance
    log` shows a `touchbar` and a `desktop` session); the Touch Bar still
    draws, takes touches and reconnects.

## Runs

### 2026-10-07, MacBook Pro 13,3 (Touch Bar), Omarchy 4.0.4

| # | Result |
| --- | --- |
| 1 | Passed: 20 tests; `desktop-check.sh` ok |
| 2 | `cargo install` into a temporary root built and installed the binary; `desktop on` and `desktop off` worked from the build (installed copy rendered, space released, fresh install enabled without a shell restart). The dev link was put back afterwards |
| 3 | Rendered headlessly with every desktop widget and live; clicks on the real widgets and hold-to-repeat (volume buttons) checked by the user |
| 4 | Checked by the user (a broken save kept the previous config; the fix applied within a second) |
| 5 | Checked by `desktop-check.sh` with a private backend; the live backend restarted during T08 with both clients reconnecting |
| 6 | Passed: 26 + 39 px reserved, windows at y 77, 26 px after disabling |
| 7 | Checked by the user (T10 plugin, placeholder widgets) |
| 8 | Checked by the user (light theme, Codex logo); transparency checked by a temporary `shell.json` change |
| 9 | Checked by the user and by polling `hyprctl layers` during 1.6 and back to 2 |
| 10 | Checked by the user: a row appeared on an external monitor when plugged in; after unplugging, the built-in display kept bar and row in order (65 px reserved). `"monitor"` not tried |
| 11 | Passed: both sessions on one backend; the Touch Bar check after T05 was the user's |

Not run on a machine without a Touch Bar yet (none at hand); that run is
still owed before calling T11 done.
