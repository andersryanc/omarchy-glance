# Performance

Numbers to compare against when the bar changes. The first set is from before
the backend split, the second from after it (task T05 in
[multi-output-tasks.md](multi-output-tasks.md)).
Measure on the same machine (MacBook Pro 13,3, i7-6920HQ) with a release build.

## Method

- **Frame time:** `cargo test --release bench_frames -- --ignored --nocapture`.
  Draws the golden-test configs with the fixed data in `tests/golden/` and
  prints the median of 500 full frames and of 500 redraws of just the
  default layer's graphs (what a graph tick costs).
- **Memory and CPU:** `tools/perf.sh [seconds]` against the running bar (default
  60 s, leave the bar alone meanwhile). RSS is each process's `VmRSS`; process
  CPU is its user+system time over the window; service CPU is a cgroup's
  `usage_usec` over the window. Since the split the client runs in
  `t1-touchbar` (with the t1bridge launcher) and the backend in
  `omarchy-glance.service` (with helpers: `pactl subscribe`, `dbus-monitor`,
  agent usage refreshes); "total" adds the two. Before the split everything
  was in `t1-touchbar`.
- To measure the built-in default, move the user config aside for the run
  (the backend reloads within a second) and put it back afterwards.
- CPU depends heavily on the config. Graphs redraw every second; a config
  without them is nearly idle. Record which config was used.

## Results

### 2026-10-06, before the backend split (T00)

Single process, commit after `c545a52` (rename to omarchy-glance).

| Measure | Built-in default config | User config (no graphs) |
| --- | --- | --- |
| Full frame | 1.68 ms | |
| Graph redraw (2 graphs) | 1.35 ms | |
| RSS | 14.4 MB | 14.3 MB |
| Renderer CPU (60 s) | 0.58% | 0.10% |
| Service CPU (60 s) | 1.83% | 2.45% |

The every-widget golden config (7 graphs) takes 2.60 ms per full frame and
2.47 ms per graph redraw.

### 2026-10-07, after the backend split (T05)

Touch Bar client and backend service, commit after `1bfb8df` (T04).

| Measure | Built-in default config | User config (no graphs) |
| --- | --- | --- |
| Full frame | 1.64 ms | |
| Graph redraw (2 graphs) | 1.30 ms | |
| RSS, client + backend | 14.6 + 5.7 MB | 14.4 + 5.7 MB |
| Client CPU (60 s) | 0.55% | 0.00% |
| Backend CPU (60 s) | 0.13% | 0.10% |
| Total service CPU (60 s) | 1.94% | 2.56% |

The every-widget config takes 2.47 ms per full frame and 2.48 ms per graph
redraw.

Compared with T00: drawing costs the same (the client draws the same frames
from the same code). Memory grows by the backend's 5.7 MB; the client stays at
the old renderer's size, which is mostly cairo and fonts. CPU of the two
processes together is within 0.1 percentage point of the single renderer
(0.68% vs 0.58% with the default, 0.10% vs 0.10% with the user config), and
service totals are within run-to-run noise: they're dominated by the helpers
and the minutely agent usage refresh, which happen in either design.

Earlier figures from the Rust port (2026-10-05: 14.5 MB, about 0.8% CPU,
2.35 ms full frame) were taken with the config in use then and without a
recorded method, so they aren't directly comparable with these.
