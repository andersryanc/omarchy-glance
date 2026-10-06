# ADR 0001: Shared application core with multiple output hosts

- Status: Accepted; implementation pending
- Date: 2026-10-06

## Context

The Rust application currently combines provider polling, configuration,
widget state, layer selection, input handling, actions, Cairo drawing, and
t1bridge transport in `src/renderer.rs`. The Python renderer remains a fallback.
The hardware output has a fixed usable width of 2060 pixels and hardware-specific
touch and key handling. `--preview` produces a static image without hardware.

We want a live desktop touchbar below the existing Omarchy top bar, reserving
screen space for normal application windows. We also want to explore a true
second row inside Omarchy without reimplementing application behavior.
The existing top row should retain its contents and interactions. An embedded
row should share its theme, background, transparency, and hiding behavior.

Current features include configurable buttons and commands, monitoring graphs,
agent usage, media controls, microphone mute and waveform, and default/Fn layers.
Proposed features in `TODO.md` include sliders, gestures, media seeking,
workspaces, active-window controls, app-aware layers, notifications, development
status, timers, animations, and theme styling. These proposals remain proposals;
this decision provides extension boundaries rather than approving them all.

## Decision

Separate provider state, application behavior, actions, presentation, and output
hosting. Keep Rust as the shared backend. Retain Cairo for hardware presentation
and implement desktop controls in QML. Share those QML controls between a
standalone Quickshell panel and a future embedded Omarchy row.

Deliver the standalone panel first. Pursue a supported upstream second-row
extension separately. Do not require a replacement bar or fork of the Omarchy
package for the initial desktop implementation.

```text
Providers -> Application core -> Widget state -> Presentation -> Output host
                    ^                                |
                    +------ Semantic input ----------+
                    |
                    +------ Actions -> Platform adapters

Presentation / host combinations:
  Cairo controls -> t1bridge hardware host (also static PNG preview)
  QML controls   -> standalone Quickshell panel host
  QML controls   -> embedded Omarchy row host (future)
```

## Boundaries

| Layer | Owns | Does not own |
| --- | --- | --- |
| Providers | Samples, usage records, media state, microphone levels, future window/workspace and workflow state | Geometry, drawing, output windows |
| Application core | Config reload, stable widget identity, visibility, action availability, timers, layer rules and per-output sessions | Cairo/QML objects, pixel geometry |
| Actions | Semantic activation and value changes, repeat scheduling, platform execution and results | Widget drawing or compositor placement |
| Presentation | Layout, hit testing, visual feedback, fonts, assets, animation, theme mapping | Data collection and platform command implementations |
| Output hosts | Hardware buffers/input or desktop window lifecycle, screen selection, reservation, host environment | Duplicate widget behavior |

Widget state is structured data: graph samples and ranges, media title and
playback capabilities, agent limits, mute state, values and action availability.
State must not contain drawing surfaces or final pixel coordinates. Format
domain labels consistently in shared code where appropriate; text measurement,
truncation, placement, and visual animation stay in presentation.

Controls emit semantic requests such as activate widget, set volume, seek media,
or switch workspace. Platform adapters execute those requests. Existing shell
commands remain supported, with execution centralized in the backend. A desktop
client references configured widget actions rather than supplying arbitrary
commands. Hardware keys continue using t1bridge; desktop key injection needs a
separate capability and must report when unavailable.

Each output has a session containing its active layer, interaction lifecycle,
visibility, and capabilities. Hardware Fn state does not implicitly change every
desktop session. Touch ownership and cancel/release handling must survive config
reloads and disconnects without leaving actions repeating. Mouse alternatives
for multitouch and gestures belong to the desktop input mapping.

Providers are shared across outputs. Sessions declare demand so hidden or
disconnected outputs do not keep unnecessary polling or capture running. In
particular, the microphone waveform retains the existing conditions: another
application is recording, input is live, and at least one visible widget needs
levels. Multiple outputs share a single capture.

## Backend and desktop interface

Initially keep the core and hardware adapter in one Rust process. Add a local
Unix-domain socket for desktop clients; splitting the hardware host into another
process is optional future work, not a prerequisite.

The interface must provide:

- A version/capability handshake, stable widget/session IDs, and a complete
  initial snapshot followed by revisioned updates.
- Structured action requests with acknowledgements or errors.
- Session demand, visibility, input cancellation, and cleanup on disconnect.
- Reconnection and snapshot resynchronization after restart or missed updates.
- Bounded queues and coalescing of fast state updates so slow clients cannot
  stall hardware rendering or provider collection.
- User-private socket access and validation of requested widget actions.

Use an XDG runtime location with restrictive permissions. Document framing,
ordering, errors, and version compatibility before relying on the interface.
Prefer straightforward structured messages initially; measure waveform/graph
traffic before introducing shared-memory transport.

The current hardware service lets t1bridge fall back when the renderer exits.
Desktop-only operation must also work without t1bridge. Establish backend
ownership and startup behavior so enabling both outputs does not start duplicate
providers. A desktop disconnect must not stop the hardware output, and hardware
unavailability must not unnecessarily stop desktop clients.

## Configuration and host environment

Preserve existing version-1 configuration behavior and hardware appearance
during extraction. Add documented output-specific presentation and session
settings without requiring existing users to migrate immediately. Provider and
action definitions are shared; dimensions, styling, monitor choice, and layer
selection can vary by output. Define override precedence explicitly.

The desktop controls accept a host environment: theme palette, font, scale,
orientation, available dimensions, transparency, and visibility. They contain no
assumption about being in a standalone window or in the Omarchy bar.

The standalone host uses a layer-shell `PanelWindow` anchored across the top of
the selected monitor and reserves space below the existing bar. Validate actual
compositor stacking and exclusion rather than assuming a fixed top offset.
Normal tiled windows must remain below both panels. Fullscreen behavior is a
separate policy to select and test. Clicking controls should preserve the active
application wherever possible.

The embedded host relies on a future supported Omarchy row API for mounting,
height, visibility, orientation, theme, transparency, input boundaries, and
popup placement. Omarchy should own combined space reservation and background.
The current installed bar has no supported second-row configuration option.

Standalone theme synchronization is possible, but exact parity with the bar's
transparency, contrast adjustment, hiding, movement, and monitor behavior is not
assumed. Record supported synchronization and gaps. An upstream row API would
provide these directly.

## Alternatives

| Alternative | Reason not selected as the default |
| --- | --- |
| Mirror Cairo frames into a desktop window | Reuses drawing, but retains hardware-oriented sizing and styling and makes native desktop controls harder |
| Implement each output independently | Duplicates providers, actions, layer rules, and resource management |
| Replacement Omarchy bar plugin | Supported in principle, but copies bar maintenance and has different service capabilities; existing popup parity requires validation |
| Fork the complete system package | Larger maintenance scope than needed; copied code does not inherit package updates automatically |
| Ordinary floating application window | Does not provide the panel reservation contract we need |

## Consequences and limitations

We maintain two presentation implementations, Cairo and QML, while sharing
application behavior. They need semantic parity, not identical pixels.
IPC and session lifecycle introduce complexity, so migrate in small working
steps. No framework or generic plugin SDK is required for the core.

Native embedding depends on upstream acceptance and API design. A replacement
bar remains a fallback exploration, not the selected deployment architecture.
Reusing installed internal bar code could follow system updates but is fragile
without a supported extension contract.

Media seeking requires position/duration support beyond the existing media
status command. Notifications require an explicit service integration; the
touchbar should not start a competing notification daemon. Fn observation,
desktop key injection, active-window actions, and non-Hyprland support need
capability checks. Orientation, fractional scaling, multiple monitors, popup
placement, and fullscreen behavior require host-specific validation.

The Python fallback remains operational but is not automatically a second
implementation of the new backend protocol. Decide its longer-term support
separately; do not silently remove it during this migration.

## Validation and delivery

Use focused tests for visibility, layer/session transitions, action lifecycle,
provider demand, config reload, and protocol reconnection. Preserve hardware
preview and validate real hardware interaction where required. Desktop smoke
checks cover reservation, focus, theme changes, scaling, monitor changes, and
backend restarts. Avoid tests that merely duplicate drawing implementation.

Implementation tasks and their acceptance criteria are in
[`../multi-output-tasks.md`](../multi-output-tasks.md).

## References

- [Current features and configuration](../../README.md)
- [Feature proposals](../../TODO.md)
- [Quickshell PanelWindow](https://quickshell.org/docs/types/Quickshell/PanelWindow)
- [Layer-shell protocol](https://github.com/swaywm/wlr-protocols/blob/master/unstable/wlr-layer-shell-unstable-v1.xml)
- Installed Omarchy sources inspected for this decision:
  `/usr/share/omarchy/shell/plugins/bar/Bar.qml`,
  `/usr/share/omarchy/shell/Ui/PluginBarApi.qml`, and
  `/usr/share/omarchy/shell/README.md`.
