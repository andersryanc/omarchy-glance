# Multiple-output implementation tasks

Architecture: [ADR 0001](adr/0001-multiple-output-architecture.md).
Unchecked items are planned work, not implemented behavior. Complete each stage
with a working hardware renderer. Proposed features in `TODO.md` are separate
scope unless explicitly included below.

## Stage 1: Extract shared behavior

### T01 — Define state, identity, and session contracts

- [ ] Inventory current widget state, provider dependencies, actions, and
  hardware-specific behavior in `renderer.rs`.
- [ ] Introduce drawing-independent widget state, stable instance identity,
  semantic input/actions, output capabilities, and session state.
- [ ] Specify identity across reloads, legacy duplicate widget IDs, layer
  selection, cancellation, and supported/unsupported actions.

Acceptance: contracts cover every existing widget and default/Fn behavior;
domain types do not depend on Cairo or QML. Existing config remains accepted.
Dependencies: none.

### T02 — Extract providers and demand management

- [ ] Move graph history, agent usage, commands, media observation, and
  microphone state out of drawing code, reusing existing provider modules.
- [ ] Share collection between sessions and track demand for costly sources.
- [ ] Preserve usage-based widget hiding and microphone capture conditions.

Acceptance: hardware state remains equivalent; multiple consumers do not
duplicate capture; removing the last consumer releases resources. Validate
provider visibility and mic lifecycle with focused tests.
Dependencies: T01.

### T03 — Extract application rules and config lifecycle

- [ ] Move visibility, config reload, action availability, and layer rules into
  the core, with per-output session state.
- [ ] Preserve the current hardware Fn/contact layer latch and invalid-config
  fallback behavior.
- [ ] Cancel interactions safely when widgets disappear or sessions close.

Acceptance: valid/invalid reloads and independent sessions behave predictably;
no stale widget ownership or repeating actions after removal.
Dependencies: T01, T02.

### T04 — Centralize actions and adapt hardware presentation

- [ ] Route shell execution, key requests, repeat scheduling, and errors through
  action adapters.
- [ ] Make Cairo layout/drawing consume core state and translate physical
  contacts into semantic input.
- [ ] Keep t1bridge buffers, submission, physical dimensions, and PNG preview
  in the hardware/presentation adapters.

Acceptance: existing buttons, commands, media, graphs, agents, microphone,
repeats, Fn layer, config reload, and preview still work. Run existing relevant
tests, compare representative previews, and check real touch interactions.
Dependencies: T03.

## Stage 2: Backend interface and lifecycle

### T05 — Specify the desktop protocol

- [ ] Document framing, version negotiation, snapshots/deltas, revision and
  widget IDs, session demand, actions, acknowledgements/errors, and reconnect.
- [ ] Define queue limits/coalescing and user-private socket access.
- [ ] Specify unavailable providers and unsupported output actions.

Acceptance: worked examples cover connect, update, action, disconnect, and
resync; clients cannot submit arbitrary commands outside configured actions.
Dependencies: T01, T04.

### T06 — Implement backend service and startup ownership

- [ ] Add socket transport and session cleanup without blocking hardware work.
- [ ] Support desktop-only startup without hardware and simultaneous outputs.
- [ ] Integrate existing `touchbar-custom`/service ownership; document restart,
  fallback, logging, and prevention of duplicate backends.
- [ ] Handle hardware loss, client disconnect, slow clients, and backend restart.

Acceptance: two desktop clients share providers; a client failure leaves other
outputs running; hardware absence permits desktop use. Protocol tests cover
resync, malformed requests, bounded updates, and resource cleanup.
Dependencies: T05.

## Stage 3: Shared desktop presentation

### T07 — Build the QML client and host contract

- [ ] Implement connection, state updates, action results, and reconnection.
- [ ] Define host-injected theme, scale, dimensions, orientation, visibility,
  and transparency without constructing a panel window in shared controls.
- [ ] Add a small development host for exercising controls without Omarchy.

Acceptance: controls can mount in a generic container; backend restart restores
state and reports disconnected/unavailable status coherently.
Dependencies: T06.

### T08 — Implement existing desktop widgets and input mapping

- [ ] Implement buttons/commands, graphs, agent meters/logos, media, and
  microphone controls from shared state.
- [ ] Support responsive left/center/right layout, text truncation, and explicit
  overflow behavior at narrow widths and large display scales.
- [ ] Map mouse press/release/hold and layer selection into semantic input;
  report unavailable keyboard actions.
- [ ] Add compatible output overrides with documented precedence and examples.

Acceptance: existing configured widgets work on desktop without duplicating
providers/actions; hidden agent widgets reclaim space; interactions preserve
the active application. Hardware config and appearance remain compatible.
Dependencies: T07.

## Stage 4: Standalone panel release

### T09 — Implement layer-shell placement and reservation

- [ ] Add the standalone Quickshell host with monitor selection and height.
- [ ] Place below the existing top bar and reserve additional space without
  overlap or double-counting. Select and document fullscreen behavior.
- [ ] Handle monitor removal, scaling, hide/show, and space release on exit.

Acceptance: normal tiled/maximized windows remain below both rows on the
selected monitor; other monitors are unaffected; no permanent reservation
remains after exit. Verify fullscreen and focus behavior on the actual system.
Dependencies: T08.

### T10 — Add theme and optional bar-state synchronization

- [ ] Resolve live theme palette/font updates for the standalone host.
- [ ] Investigate supported access to Omarchy transparency, foreground contrast,
  hiding, position, and monitor state; implement supported synchronization.
- [ ] Document gaps and behavior when the system bar moves or is absent.

Acceptance: theme changes update controls without backend restart; supported
transparency changes affect the panel background. Do not claim exact native
parity where no supported state interface exists.
Dependencies: T09.

### T11 — Package and validate the standalone mode

- [ ] Provide installation, launch/autostart, configuration, troubleshooting,
  and simultaneous hardware/desktop usage instructions.
- [ ] Record runtime dependencies and Python fallback support boundaries.
- [ ] Perform a release smoke check covering widgets, reload, reconnection,
  reservation, focus, theme, multiple outputs, scaling, and monitor changes.

Acceptance: documented commands reproduce a usable panel and existing hardware
workflow; remaining limitations are explicit. No system-bar fork is required.
Dependencies: T10.

## Stage 5: Native Omarchy row exploration

### T12 — Design a supported upstream extension contract

- [ ] Draft a proposal for mounting an extra row while preserving the original
  row's contents, dimensions, gestures, and widget/popup behavior.
- [ ] Specify row sizing, combined reservation, host environment, visibility,
  orientation policy, input boundaries, popup anchoring, and lifecycle.
- [ ] Check service capability needs, especially notifications/media, against
  Omarchy's plugin facades; distinguish required API from optional features.

Acceptance: reviewable proposal and local prototype establish feasibility;
upstream submission is a separate explicitly authorized action. No packaged
Omarchy files are modified as an installation strategy.
Dependencies: ADR; can be researched alongside T01–T11.

### T13 — Implement the embedded host when the contract is available

- [ ] Mount the shared QML controls through the accepted extension API.
- [ ] Delegate background, transparency, hiding, theme, and reservation to
  Omarchy; reuse the existing backend/client protocol.
- [ ] Package the community plugin and document supported host versions.

Acceptance: original top row behavior is preserved; double-click transparency
and theme changes apply to both rows; existing popups still anchor correctly;
the plugin contains no copied implementation of the system bar.
Dependencies: T08 and an available, validated T12 extension contract.

## Follow-on feature work

After the core and desktop mode are stable, refine the proposals in `TODO.md`
into individual feature tasks. Use the following ownership boundaries:

| Feature family | Main work |
| --- | --- |
| Volume/brightness sliders and media seeking | Provider capabilities, value actions, Cairo/QML controls |
| Workspaces, active window, app-aware layers | Hyprland provider, action adapters, core layer rules |
| Notifications | Supported notification-service integration and transient core state |
| Claude sessions, Git/CI, timers | Providers and core state, then both presentations |
| Gestures and additional layers | Semantic input, session policy, per-output mappings |
| Animations and theme polish | Presentation and host environment |
| Hardware brightness/ambient light | Hardware capability/adapter, contingent on available IPC/sensors |

New features should specify behavior for unsupported outputs and which visible
sessions require provider activity. They do not require redesigning output hosts.
