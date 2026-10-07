# Desktop hosts: standalone Quickshell vs Omarchy panel plugin

Task T06 of [multi-output-tasks.md](multi-output-tasks.md), for the host
question in [ADR 0001](adr/0001-multiple-output-architecture.md#desktop-host).
Both spikes are in `tools/spikes/desktop-host/`: the same hardcoded 30 px row
(text, a click counter, theme colours) as a standalone Quickshell `PanelWindow`
(`standalone/shell.qml`, run with `qs -p tools/spikes/desktop-host/standalone`)
and as a third-party `panel` plugin with `keepLoaded: true` (`plugin/`, linked
into `~/.config/omarchy/plugins/` and enabled with
`omarchy-shell shell setPluginEnabled glance.host-spike true`). Both use the
`top` layer, `ExclusionMode.Auto` and no keyboard focus, like the Omarchy bar.

Tested 2026-10-07 on Omarchy 4.0.4 (Hyprland 0.56.2, Quickshell 0.3.1), over
a Sunshine virtual display (2560×1600 at scale 2) with the bar on top (26 px).
Positions come from `hyprctl layers` and reserved space from `hyprctl
monitors`, plus screenshots.

## Results

| Check | Standalone | Plugin |
| --- | --- | --- |
| Stacks below the bar | Yes when started after the shell (row at y 26, 56 px reserved) | Yes |
| After `omarchy-restart-shell` | **No**: the bar maps after the row, so the row moves to y 0 and the bar to y 30. Fixed in the spike by remapping the row when Hyprland reports `openlayer>>omarchy-bar` (windows below shift twice, about 100 ms apart) | Yes, 3 of 3 restarts: the row restarts with the shell and maps after the bar |
| Bar hidden (`omarchy-toggle-bar`) | Row moves to the top, back under the bar when shown | Same; `shell.bar.barHidden` updates live |
| Bar moved to the bottom and back | Row stays on top; back under the bar afterwards | Same; `shell.bar.position` updates live |
| Fullscreen window | Covers bar and row | Same |
| Maximised window | Below both rows | Same |
| Space released on exit | Yes (reserved back to 26 px) | Yes on disable |
| Theme switch | Follows, by watching `~/.local/state/omarchy/current/theme.name` and re-reading `colors.toml` (the theme directory is replaced, so it can't be watched itself) | Follows through the shell's `Color` singleton (`import qs.Commons` works for third-party plugins), with no file handling |
| Theme data available | `colors.toml` palette; shell surface roles and sizing only by parsing `shell.toml` and `~/.config/omarchy/shell.toml` ourselves | `Color` (palette and surface roles such as `Color.bar.*`), `Style` (type scale, spacing, bar sizes), live |
| Bar state | Inferred: `hyprctl layers` / `shell.json` / the `bar-off` toggle file | `PluginBarStateApi`: `barHidden`, `barSize`, `fontFamily`, `position` |
| Transparency | Readable from `shell.json` (`bar.transparent`) | Not in the facade; readable from `shell.json` the same way |
| Clicks keep focus on the active window | Not tested (no way to click remotely); both set `keyboardFocus: None`, which Hyprland honours for layer surfaces | Same |
| Fractional scaling | Not tested (changing the scale of the streamed display would disrupt the session) | Not tested |
| Monitor hotplug | Not tested; both create one window per `Quickshell.screens` entry through `Variants` | Not tested |
| Autostart and install | Ours to provide (a user service or Hyprland autostart) | The shell mounts it at startup once enabled; installs with `omarchy plugin add`; reloads on file save while developing |
| Isolation | Own process: a bug in the row can't stall the bar, notifications or the lock screen | Runs unsandboxed in `omarchy-shell`: a hang or heavy work in the row stalls the whole desktop shell, including the lock screen |
| API stability | Only Quickshell and Hyprland | The third-party plugin facade, `keepLoaded` panels and `qs.Commons`, all new in Omarchy 4 |

## Recommendation

Approved by the user on 2026-10-07; [desktop-client.md](desktop-client.md)
builds on it.

Use the **Omarchy panel plugin as the primary host**. The project is
Omarchy-only, and the plugin gets the things the standalone host has to
rebuild: correct stacking across shell restarts without a remap hack, the live
theme with surface roles and the type scale, bar state, and autostart and
installation through Omarchy's own plugin tooling.

Keep the **standalone host for development only** (the small development host
T07 asks for), not as a shipped option. The shared controls stay
host-agnostic, so a standalone host remains possible if the plugin API changes
or isolation turns out to matter. Its findings are recorded here: it needs the
remap on `openlayer>>omarchy-bar` and its own theme and bar-state handling.

The plugin's main cost is isolation: keep the QML light (no heavy JavaScript,
an asynchronous socket, the backend doing all work), so a misbehaving row
can't stall the shell. Transparency isn't in the facade; read
`bar.transparent` from `shell.json`, or ask upstream to add it to
`PluginBarStateApi`.

## Still to check by hand

Clicking the row while another window has focus (focus must stay put),
fractional scaling (e.g. 1.25 and 1.6 on the built-in display), and plugging a
monitor in and out. T10 also needs these, on the chosen host.
