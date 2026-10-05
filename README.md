# Touch Bar renderer (Rust)

A Rust port of the Python renderer in `~/Work/touchbar` for the MacBook Pro T1
Touch Bar, driven through [t1bridge](https://github.com/standardagents/t1bridge)'s
Touch Bar hardware IPC (`~/Work/touchbar/t1bridge-interfaces.md`). It reads the
same `~/.config/touchbar/config.json`, has the same widgets and options, and
draws with cairo's toy text API just as the Python version does, so frames come
out the same. The config format is documented in `~/Work/touchbar/README.md`.

## Build and run

```
cargo build --release
touchbar-custom on            # point ~/.config/t1bridge/renderer here and restart
touchbar-custom restart       # after rebuilding
```

`touchbar-custom on python` switches back to the Python renderer. The built-in
`config.default.json` is compiled into the binary and used when there is no
user config.

To check a change without the hardware, draw one frame to a PNG (`fn` shows the
Fn layer):

```
target/release/touchbar --preview /tmp/bar.png [fn]
```

## Layout

| File | |
|---|---|
| `src/main.rs` | Entry point, `--preview`. |
| `src/proto.rs` | The IPC: socket, wire format, sealed memfd buffers. |
| `src/config.rs` | Config parsing and option helpers. |
| `src/renderer.rs` | Widgets, layout, drawing, input and the event loop. |
| `src/sources.rs` | Data for the graph widgets (cpu, memory, gpu, network, disk, battery, fan). |
| `src/mic.rs` | Mic mute/in-use state and the waveform's level capture. |
| `src/proc.rs` | Child processes. |
