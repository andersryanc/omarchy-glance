# Performance

Numbers to compare against when the renderer changes, in particular after the
backend split (task T05 in [multi-output-tasks.md](multi-output-tasks.md)).
Measure on the same machine (MacBook Pro 13,3, i7-6920HQ) with a release build.

## Method

- **Frame time:** `cargo test --release bench_frames -- --ignored --nocapture`.
  Draws the golden-test configs with the fixed data in `tests/golden/` and
  prints the median of 500 full frames and of 500 redraws of just the
  default layer's graphs (what a graph tick costs).
- **Memory and CPU:** `tools/perf.sh [seconds]` against the running bar (default
  60 s, leave the bar alone meanwhile). RSS is the renderer's `VmRSS`; renderer
  CPU is its user+system time over the window; service CPU is the
  `t1-touchbar` cgroup's `usage_usec` over the window, which also counts the
  t1bridge launcher and helpers (`pactl subscribe`, `dbus-monitor`, agent
  usage refreshes).
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

Earlier figures from the Rust port (2026-10-05: 14.5 MB, about 0.8% CPU,
2.35 ms full frame) were taken with the config in use then and without a
recorded method, so they aren't directly comparable with these.
