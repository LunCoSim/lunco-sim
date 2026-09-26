---
name: runtime-ui
description: >
  Author or review a reloadable Twin-facing HTML/CSS-like runtime surface in
  LunCoSim. Use this for HUDs, telemetry cards, progress overlays, view
  switchers, runtime UI bindings, HTML/CSS hot reload, HUI, Flair,
  EngineExposures, or questions about the limits of the native HTML UI. Use
  lunco-ui instead for workbench/egui panels and docking internals.
---

# Runtime-authored UI

## Read first

Before changing a runtime surface, read:

1. [`docs/architecture/runtime-authored-ui.md`](../../docs/architecture/runtime-authored-ui.md)
2. [`assets/ui/README.md`](../../assets/ui/README.md)
3. [`skills/lunco-ui/SKILL.md`](../lunco-ui/SKILL.md) when the surface overlaps
   egui, the workbench, or docking
4. [`skills/test-via-api/SKILL.md`](../test-via-api/SKILL.md) for live verification

The generic implementation is `lunco-workbench-runtime-ui`; the `luncosim`
windowed host in `crates/lunco-luncosim-ui/src/ui/` supplies app-specific gates,
capture mode, and action handling. Do not assume that `lunica` or a headless
server has this surface manifest.

The current compatible dependency baseline is `bevy_hui 0.7.0`,
`bevy_flair 0.8.1`, and Bevy `0.19.1`; verify the lockfile and upstream release
notes before changing it. HUI 0.7 is the Bevy 0.19 release. The separate
`bevy_hui_widgets 0.6.0` crate provides primitive text-input, slider, and select
components, but is intentionally not a LunCoSim dependency: it does not define
our clipboard, validation, keyboard-navigation, accessibility, or modal
semantics.

## Choose the right layer

Use runtime HTML/CSS for a small authored presentation surface that should be
changed without recompiling Rust: HUDs, status cards, progress overlays,
telemetry summaries, and simple buttons.

Use `lunco-workbench`/egui for the application shell, docking, code editors,
large inspectors, rich text input, complex forms, modal dialogs, and controls
that need semantics not present in the runtime contract. Runtime UI uses the existing
egui host and dock geometry; it does not replace the workbench or create a
second hit-test/camera system.

The standard Twin and Files navigation is the optional
`lunco-workbench-browser` feature layered on the shell; do not recreate those
panels in an authored runtime surface.

`lunco-ui::modal` remains the owner for rich dialogs requiring focus, queued
outcomes, editing, validation, or accessibility. The generic runtime surface
also supports a deliberately small authored modal contract: a viewport surface
may declare `modal: true` and an authored `dismiss_action`; visible controls
own their computed input regions and Escape emits that semantic action. Generic
keyed collection hosts provide dynamic rows, while Rhai owns the records,
labels, ordering, visibility, and action meaning. Do not grow this into a
second rich-dialog implementation without a separate typed contract and
acceptance tests.

Twin-authored actions are open-ended semantic identifiers. The reusable
`program-browser` surface publishes typed arrays of records; its HUI row
template is reconciled by the generic keyed collection host. A surface may also
declare a generic dropdown: the manifest supplies the trigger, option source,
key/label/action field names, and Rhai-owned width/max-height sources. The
compact `camera-status` surface is one consumer of that primitive, not a
camera-specific widget.
`program_editor` owns selection, editor focus, atomic source switching, and
creation for any authored program, regardless of whether its owner is a rover,
lander, route, or another model. Rich source text entry remains in the existing
Rhai editor/REPL until HUI gains tested typed input semantics.

For dynamic semantic controls, use the HUI convention
`on_press="runtime_ui_authored_action" tag:action="{action}"`. Additional
`tag:*` values become a typed `HookValue` parameter map on the generic action
event; the runtime does not concatenate names and values into a protocol
string. This permits Twin-defined actions and dynamic controls without
registering one Rust callback per item. Do not use JavaScript or encode action
payloads as JSON.

Collection hosts retain the Rhai-authored row order, clip their list, and
consume wheel input at the host boundary. They own row lifecycle only; do not
add a per-surface Rust list resource or a fixed row-count view model.

Use stable `id` attributes and `#id` selectors for authored HUI nodes. Flair
supports class selectors, but HUI 0.7 does not turn an HTML `class` attribute
into a `ClassList`; it treats that attribute as an unknown style property.

Project-owned visibility policy belongs in the active Twin manifest's generic
`[settings]` table. A surface declares `setting` plus `setting_default` in
`runtime_surfaces.json`; Rhai reads/writes the same scope with
`get_twin_setting`/`set_twin_setting`. Keep user-global diagnostics, theme,
input visualisation, and window preferences in `lunco-settings`. Missing Twin
keys use the surface's authored default, so Rust does not grow a field for
each new preference.

## Authoring workflow

### 1. Define a generic capability namespace

Add or extend an engine-side producer only when the value is not already
available. Publish authoritative, presentation-ready named values through
`EngineExposures`:

```rust
let mut ui = exposures.writer("mission-status");
ui.visible(has_mission);
ui.property("title", mission_title);
ui.property("state", state_label);
ui.property("state_color", "var(--ok-color)");
```

The namespace is a capability boundary shared by HTML, egui, API, telemetry,
and remote consumers. Do not add `domain_to_view`, `vessel_exposure`, or a
widget-specific Rust registry. Resolve source state in the engine producer;
keep markup unaware of ECS/domain types.

Engine health follows the same generic path. Read the typed
`EngineHealthSnapshot`/`PhysicsHealthSnapshot` publication and expose named
properties through the ordinary `engine-health` namespace; do not add a HUD
reader for `DiagnosticsStore`, an Avian timing query, or another source-specific
bridge. Native UI, HUI, API, telemetry, and recording consumers all read the
common publication. For scalar participant state, use the shared
`PortRegistry`; do not create a parallel port reader for a HUD.

`SimulationProgress` owner and reason facts are projected into each authored
runtime surface as typed `simulation_progress` data. Let the active Rhai policy
turn those facts into user-facing labels such as `PHYSICS LOADING` or
`TERRAIN LOADING`; the engine does not hardcode HUD wording. Progress changes
invalidate the existing surface projection, so a readiness label does not poll
the simulation. During preparation the built-in Rhai visibility policy
temporarily shows an authored surface before possession, then returns to its
authored `possessed` or `always` mode when the holds clear.

Command and one-shot REPL timing is also generic presentation data. Read the
`application-cadence` exposure for command and REPL sequence/interval/rate
values; do not attach a HUD timer to `Time<Virtual>`, count API requests as
completed script evaluations, or read the command/REPL owners directly.

Producers must use change detection, revisions, or dirty flags. Continuous
values are coalesced to the current bounded presentation cadence (20 Hz).
`EngineExposures.revision` changes only when a value or visibility flag changes;
it is not a frame counter. Do not use JSON to detect internal changes.
When one producer owns several surfaces, keep invalidation domains separate so
continuous motion does not rebuild static authored topology. Use the existing
authoritative stage revision for USD-derived membership and cache that
membership plus static authored metadata such as program source facts and
declared public-output names. Do not reread those facts at publication cadence,
rescan all prims, or add a second revision/source registry. Keep simulation
status, telemetry, outputs, and Rhai policy results live.

For camera status, Rust publishes the current camera fact and compact label
through the generic exposure namespace. The shared
`lunco-usd-bevy::camera_switch::camera_display_labels` resolver is also used by
the picker, Camera menu, USD/entity trees, and Inspector: unique leaves stand
alone, duplicate leaves gain nearest-owner context and then ancestors,
generated hexadecimal/UUID-like owner suffixes are hidden, and an unavoidable
normalized collision gets an ordinal. The full USD path remains the typed
selection value and hover/diagnostic text. Rhai owns selection policy
(`set_camera(name)`) and can read the fact with `get_exposure(...)`; HUI/CSS
owns rendering. Camera status emits `CameraSelectionStatusChanged` after its
camera/viewport lifecycle projection changes, and the exposure observer
consumes that event. The UI is revision-gated. Do not add a Rhai `on_tick`
loop, a timer poll, or a per-frame camera scan for this HUD.

Failed camera actions are logged and published through the shared warning toast.
Keep their messages out of the Camera menu.
Register open egui dropdown bounds with `ScenePickGate` so option clicks do not
fall through to the 3D scene.

### 2. Add the template and stylesheet

Place files under `assets/ui/`. The stable contract is:

- HUI `<template>`, `<property>`, `<node>`, `<text>`, and `<button>`;
- stable `id` values and `{property}` interpolation;
- `on_press="callback_name"` for semantic actions;
- Flair CSS-like layout and visual properties, custom properties, and
  `var(...)`.

Declare every property that the bridge should write:

```html
<template>
  <property name="title">Status</property>
  <property name="state">offline</property>

  <node id="status-root">
    <text id="status-title">{title}</text>
    <text id="status-state">{state}</text>
  </node>
</template>
```

Each projected value is also available as `--ui-<property-name>` in CSS. Keep
the manifest-owned outer rectangle separate from CSS-owned internal layout:

```css
@import "ui/runtime_fonts.css";

#status-root {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 12px;
  background-color: var(--panel-background);
}

#status-state {
  color: var(--ui-state-color);
}
```

Use the bundled font import for telemetry/status glyphs. Do not rely on Bevy's
minimal `default_font` or a host-installed fallback.

### 3. Register bindings, gates, actions, and placement

Add a surface entry to `assets/ui/runtime_surfaces.json`:

```json
{
  "id": "mission-status",
  "template": "ui/status.html",
  "stylesheet": "ui/status.css",
  "namespace": "mission-status",
  "bindings": {
    "title": { "source": "title" },
    "state": { "source": "state" }
  },
  "actions": [
    { "callback": "runtime_status_focus_moon", "action": "view.body.moon" }
  ],
  "visible_in_perspective": "sandbox_view",
  "interactive": true,
  "placement": {
    "mode": "window",
    "anchor": "top_right",
    "offset": [-16.0, 16.0],
    "width": 260.0,
    "height": 84.0
  }
}
```

Surface `id` values and callback names must be unique within the manifest. The
loader rejects unknown fields, unsafe asset paths, and invalid geometry.
Authored semantic actions are accepted and delivered to Rhai; do not add a Rust
enum arm for each Twin, route, rover, or lander.

Binding target names must be declared template properties. `map` translates
exact rendered strings, which is useful for `true`/`false`, body ids, `display`,
or CSS colors. It does not perform arithmetic or general expressions; publish a
formatted presentation value when that is what the surface needs.

Placement modes:

- `viewport` fills the window;
- `dock_panel` uses the workbench's authoritative `PanelRects` plus an inset;
- `window` uses logical-point width/height, a corner/center anchor, and an offset.

Only a `window` surface may set `draggable: true`. The primary-button drag
stores a finite logical top-left override by stable surface id, clamps it to
the live target after resize/DPI changes, and persists it in the active Twin's
existing workbench workspace state. A primary-button double click removes the
override and restores the authored anchor. The shipped `celestial-view`
switcher is a draggable window with a top-centre authored default; Settings ▸
HUD also exposes a surface-specific reset that removes its per-Twin override.
Manifest reconciliation prunes unknown surface ids, and `TwinClosed` clears the
in-memory layout scope.

`interactive: true` enables input ownership for visible HUI controls that carry
an authored `on_press` action. The runtime feeds each control's computed Bevy UI
rectangle into the existing `ScenePickGate`; it never registers the surface root
or a full-window `viewport` rectangle. This keeps HUDs transparent to camera
dragging and scene clicks outside their explicit controls. Do not add a parallel
pointer/interception system. Do not add per-frame position correction; placement
is applied after HUI/Flair style work with change detection, and the startup
resolver ignores a zero-sized target in favor of the live primary window
dimensions.

### 4. Map actions through the existing command path

The HTML callback name is only an authored binding. The manifest maps it to a
semantic action string; the runtime emits a typed action event; the owning
Rhai program maps that action to a typed command/event. A template must not
mutate resources or call a domain API directly.

The generic dropdown mechanic owns only popup lifecycle, typed record
projection, scrolling, and authored dimensions. It does not parse action
prefixes or construct options. HUI 0.7 has no native select/accessibility
tree, so this shared egui primitive is the low-level option. Camera, program,
route, and future domain policies remain authored in Rhai.

If a new domain action is needed, author its semantic identifier and handle it
in the owning Rhai program through the existing typed command/query/event
surface. Do not add a legacy callback alias or a widget-specific Rust shim
merely to make one template work.

## Reload loop

On native desktop, keep one production `luncosim` process running and edit assets:

| Edit | Live effect |
|---|---|
| `assets/ui/*.html` | HUI rebuilds the affected retained surface tree. |
| `assets/ui/*.css` / imports | Flair reapplies the stylesheet. |
| `assets/ui/runtime_surfaces.json` | Surface roots and action registrations are rebuilt. |
| Rust producer/observer | Rebuild the production binary; replace the session through API `Exit`. |

`ReloadShader` reloads WGSL only. A bare engine path such as
`shaders/foo.wgsl` resolves whichever active asset identity the renderer holds
(default source or `lunco://`); explicit `lunco://…` and `twin://…` paths are
exact. An empty path queues every currently loaded WGSL asset, and an inactive
target fails visibly instead of being reported as a successful no-op.
`SetShaderSource` uses the same identity resolution for direct in-memory WGSL
edits; when a bare target is not loaded yet, it seeds the canonical
`lunco://` asset for journal replay. `RunScenario` hot-reloads Rhai only. None
of these commands reloads HTML/CSS. The native file watcher is not present in
the headless server; web builds use bundled static assets and browser cache
rules.

HUI caveats: one root per template component, no recursive imports, and a
nested component template reload may require reloading the top-level template
again. Never manually write Bevy styling components under the surface from Rust;
HUI/Flair owns those components.

Lifecycle invariant: when an exposure or presentation gate turns off, the
bridge removes the retained root whenever any HUI state remains, even if its
local mounted marker is stale after a deferred rebuild. A hidden surface must
not leave a stale progress card in the render tree.

## Verification

For a markup/style-only change:

1. Start the already-built production binary with an explicit free API port.
2. Wait for `/api/ready` to report `ready:true`, `world_hold:false`, and
   `pending_count:0` when scene readiness is relevant.
3. Edit the asset and observe the live window; do not rebuild or relaunch just
   for HTML/CSS.
4. Query the capability side with `ReadExposures` and capture a screenshot with
   `CaptureScreenshot` when the visual result matters.
5. Check logs for HUI/Flair parse or asset errors.

For a Rust change, build the production binary in this worktree and set
`LUNCOSIM_BIN` to it. Each agent owns a distinct free API port and launches from
the same checkout and working directory as its terminal. Before replacing your
own session, send API `Exit` and verify its process and port are gone. Do not
control another agent's session or use `pkill`.

Useful diagnosis order:

- missing surface → namespace, exposure visibility, perspective, gate,
  placement, asset paths;
- blank value → declared `<property>`, exact binding source, exact `map` key;
- dead button → callback spelling, manifest action, host observer;
- tofu → explicit bundled font import and asset path;
- startup jump → manifest placement and change-detected post-style boundary;
- slow frame → exposure revision/cadence, tree size, egui, physics, and GPU
  measurements separately. Do not infer that HTML is the bottleneck from FPS
  alone.

## Current non-goals

Do not assume support for browser DOM APIs, JavaScript, forms, text editing,
virtualised lists, full accessibility, arbitrary web CSS, `!important`, global
stylesheets, font fallback chains, or reliable mixed-unit `calc()`. Add a
deliberate engine/runtime contract and tests before expanding the surface
language.
