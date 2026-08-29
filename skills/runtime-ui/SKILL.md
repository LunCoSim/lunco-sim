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

The current implementation is the `luncosim` windowed UI layer in
`crates/lunco-luncosim/src/ui/`. Do not assume that `lunica` or a headless
server has this surface manifest.

## Choose the right layer

Use runtime HTML/CSS for a small authored presentation surface that should be
changed without recompiling Rust: HUDs, status cards, progress overlays,
telemetry summaries, and simple buttons.

Use `lunco-workbench`/egui for the application shell, docking, code editors,
large inspectors, rich text input, complex forms, modal dialogs, and controls
that need semantics not present in the runtime contract. Runtime UI uses the existing
egui host and dock geometry; it does not replace the workbench or create a
second hit-test/camera system.

The shared `lunco-ui::modal` host owns modal queueing, scrim, focus, Esc
dismissal, outcomes, and the typed `CloseModal` command. HUI currently has no
modal queue/outcome, checkbox/input state events, or dynamic repeated-list
contract, so a dialog requiring those capabilities belongs in that host until
the runtime surface contract grows and is tested. A compact authored button may
still open an existing egui host control when that is the established owner:
the camera-status picker binds the deterministic `active_label` projection,
uses the authored `camera.picker.toggle` intent, the measured HUI surface
rectangle as its anchor, and the same
`CameraSelectionStatus` view model as the workbench Camera menu. Keep the
selection command typed and do not add a second camera registry or a HUI
dynamic-list shim.

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

Producers must use change detection, revisions, or dirty flags. Continuous
values are coalesced to the current bounded presentation cadence (20 Hz).
`EngineExposures.revision` changes only when a value or visibility flag changes;
it is not a frame counter. Do not use JSON to detect internal changes.

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
loader rejects unknown fields, unsafe asset paths, invalid geometry, and
actions outside the host's closed semantic action set.

Binding target names must be declared template properties. `map` translates
exact rendered strings, which is useful for `true`/`false`, body ids, `display`,
or CSS colors. It does not perform arithmetic or general expressions; publish a
formatted presentation value when that is what the surface needs.

Placement modes:

- `viewport` fills the window;
- `dock_panel` uses the workbench's authoritative `PanelRects` plus an inset;
- `window` uses logical-point width/height, a corner/center anchor, and an offset.

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
semantic action string; the runtime emits a typed action event; the host
observer maps that action to an existing typed command/event. A template must
not mutate resources or call a domain API directly.

The closed action set includes `camera.picker.toggle` for the authored
camera-status trigger. That action only opens the camera picker; because HUI
has no dynamic repeated-list or payload-action contract, the existing egui host
renders the options from `CameraSelectionStatus`, anchors them to the measured
HUI surface rectangle, and emits `SetUserCamera`. Reuse that view model and
typed command path instead of adding a second camera registry or a HUI list
shim.

If a new action is truly needed, add the canonical typed command/observer and
register it in the owning domain. Do not add a legacy callback alias or a
widget-specific Rust shim merely to make one template work.

## Reload loop

On native desktop, keep one production `luncosim` process running and edit assets:

| Edit | Live effect |
|---|---|
| `assets/ui/*.html` | HUI rebuilds the affected retained surface tree. |
| `assets/ui/*.css` / imports | Flair reapplies the stylesheet. |
| `assets/ui/runtime_surfaces.json` | Surface roots and action registrations are rebuilt. |
| Rust producer/observer | Rebuild the production binary; replace the session through API `Exit`. |

`ReloadShader` reloads WGSL only. `RunScenario` hot-reloads Rhai only. Neither
is an HTML/CSS reload. The native file watcher is not present in the headless
server; web builds use bundled static assets and browser cache rules.

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

For a Rust change, build `target/debug/luncosim` in this worktree, send API
`Exit`, verify the existing process and port are gone, then launch the replacement.
Never overlap sessions or use `pkill`.

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
