# Runtime-authored HTML/CSS UI

> Status: Active · Audience: Twin authors, UI contributors, and engine maintainers

LunCoSim has a small runtime-authored presentation layer for HUDs, telemetry
cards, progress overlays, and other Twin-facing surfaces. It looks like HTML
and CSS, but it is not a browser: templates become retained Bevy UI entities,
and styles become Bevy `Node`/visual components through `bevy_hui` and
`bevy_flair`.

The design goal is a fast, reloadable interface boundary that can be shared by
native and web-facing presentation work without moving simulation authority into
markup. A surface may read a named engine capability and emit a semantic action;
it cannot inspect ECS state or directly mutate the simulation.

## The boundary

```text
authoritative engine state
        │  change detection + bounded presentation cadence
        ▼
EngineExposures  ────────────────► ReadExposures / telemetry / remote clients
        │
        │ named snapshot, revision-gated
        ▼
HUI template properties + Flair CSS custom properties
        │
        ▼
retained Bevy UI tree ───────────► WorkbenchEguiHost / window

HTML button ─► HUI callback ─► RuntimeUiAction ─► typed command observer
```

Each side owns one concern:

| Layer | Owns | Does not own |
|---|---|---|
| Engine/domain producer | authoritative values, sampling, visibility, capability names | HTML ids, CSS, layout, widget-specific code |
| `EngineExposures` | typed named snapshots and change revision | renderer or presentation policy |
| `runtime_surfaces.json` | reusable surface registration, bindings, gates, actions, placement | simulation state, subject selection, and visibility policy |
| active Twin `LunCoPolicy` / Rhai | subject visibility, scalar presentation, recording selection | retained tree, simulation authority, and renderer details |
| `.html` template | retained tree shape, declared properties, callback names | ECS queries, domain commands, CSS layout |
| `.css` stylesheet | appearance, internal layout, transitions, custom-property mapping | engine state and command dispatch |
| workbench / egui | shell, editors, complex panels, docking tree | runtime surface content |

The generic exposure namespace is the contract. Do not create a special
`domain_to_view` path or a Rust producer for one particular HTML template. Ports,
telemetry, physics, scripts, and derived capabilities all publish through the
same registry.

Subject-scoped surfaces are discovered from composed USD. A prim opts in with
`lunco:ui:surfaceId`, and `lunco:ui:visibilityMode` supplies the mode consumed
by the built-in Rhai policy (`possessed` or `always`). The surface ID is the
exposure namespace; Rust does not construct names from model classes, slots, or
vehicle counts. A Twin may author, override, or omit this metadata on its
scene-level opinions.

## Where to work

The shipped luncosim surfaces live in [`assets/ui/`](../../assets/ui/):

- [`runtime_surfaces.json`](../../assets/ui/runtime_surfaces.json) registers each
  surface.
- `*.html` files describe retained structure and declared properties.
- `*.css` files describe the presentation and import
  [`runtime_fonts.css`](../../assets/ui/runtime_fonts.css).
- [`assets/ui/README.md`](../../assets/ui/README.md) is the short asset-level
  reference.

The runtime bridge is in
`crates/lunco-luncosim-ui/src/ui/runtime_exposure.rs`.
Runtime snapshot projection is in
`crates/lunco-luncosim-exposures/src/lib.rs`; it is intentionally independent
of domain calculations. Modelica and Rhai own domain values and
policy, while `lunco-workbench` supplies the
egui host camera, dock rectangles, and scene-pick ownership.

## Authoring a surface

### 1. Register the surface

Every surface needs a stable `id`, template, stylesheet, namespace, and placement. The
following is a minimal interactive window surface:

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

The fields are:

| Field | Meaning |
|---|---|
| `id` | Stable manifest identity; unique within the manifest. |
| `template`, `stylesheet` | Asset paths under the normal Bevy asset root. |
| `namespace` | Exact key in `EngineExposures`; use a capability name, not a widget name. |
| `bindings` | Optional map from template property name to exposure property name. A `map` translates exact rendered values such as `true` or `301` into CSS values. |
| `actions` | Maps a unique HUI callback name to one of the host's closed semantic actions (`view.surface`, `view.body.moon`, `view.body.earth`, `overlay.terrain.dismiss`, `autopilot.toggle`, or `camera.picker.toggle`). |
| `visible_in_perspective` | Optional workbench perspective restriction. |
| `gate` | Optional named host gate. Unknown gates are closed. |
| `setting` | Optional namespaced boolean in the active Twin's `[settings]` table. The surface is hidden when the value is false. |
| `setting_default` | Value used when `setting` is absent, including when no Twin is active. This is authored per surface; Rust has no per-setting field. |
| `interactive` | Enables input ownership for authored controls carrying HUI `on_press`; only those controls' computed Bevy UI rectangles enter the existing chrome/scene pick gate. The surface root and a `viewport` placement never claim the full window. |
| `draggable` | Allows a `window` surface root to move with primary-button dragging. The user position is clamped to the live logical target, keyed by stable `id`, and reset to the authored anchor with a primary-button double click. `viewport` and `dock_panel` roots cannot opt in. The shipped `celestial-view` switcher opts in and also has a surface-specific reset in Settings ▸ HUD. |
| `placement` | The outer rectangle and its relationship to the workbench. |

The manifest loader rejects unknown fields, duplicate surface IDs/namespaces or
callbacks, unsafe relative paths, empty contract names, unsupported actions, and
non-finite or non-positive window geometry before any surface is mounted.

Window surfaces may opt into movement with `draggable: true`. The manifest
remains the default and visibility authority; the workbench stores only a
validated logical top-left override in the active Twin's existing workspace
state file. Overrides are pruned when a manifest no longer authors their stable
surface id, are clamped after a resize or DPI change, and are cleared when the
Twin closes. This keeps runtime layout preferences per-Twin without adding a
second UI persistence store. A double click restores the authored anchor and
removes the override. Dragging uses Bevy picking events on the retained HUI
tree and the existing pointer propagation path; it does not add a parallel
egui hit-test or per-frame correction loop. The shipped `celestial-view` surface
uses a top-centre authored default; Settings ▸ HUD removes only its override,
leaving the shared layout store and other draggable surfaces unchanged.

### Twin settings and camera policy

Project-owned presentation policy belongs in the active Twin manifest, not in a
Rust field or the user's global settings file. The generic scalar map is
available to Rhai as `get_twin_setting(key)` and is changed through
`set_twin_setting(key, value)`, which persists through the workspace command
owner. A surface opts into that policy with `setting` and declares its omitted
value with `setting_default`.

The camera-status surface is the reference composition. Rust publishes the
full current camera fact (`active_name`) and a deterministic compact identity
projection (`active_label`) through the generic `camera-status` exposure. Rhai
owns authored camera-selection policy through `set_camera(name)` and can read
the full fact with `get_exposure(...)`; HUI/CSS owns the compact retained
presentation and binds `active_label` while its complete authored button emits
`camera.picker.toggle` from the label, camera name, and card padding.
The native picker uses the measured HUI rectangle only as its anchor; it sizes
the popup from the widest rendered option, clamps that width to egui's menu and
viewport limits, and truncates only the display projection when a bound is
reached. Full USD identities remain the selection and hover data.
The shared `lunco-usd-bevy-camera::camera_switch::camera_display_labels` policy is
used by the exposure, picker, Camera menu, USD prim tree, entity tree, and
Inspector: unique authored leaves stand alone; duplicate leaves gain the
nearest owner context and then additional ancestors; generated
hexadecimal/UUID-like owner suffixes are omitted; an unavoidable normalized
collision gets a small ordinal. The full USD path remains the selection
identity and is available through tooltips, diagnostics, and `active_name`,
never replaced by the display projection.
Because HUI has no dynamic repeated-list or payload-action contract, the
existing egui host draws the picker from the same `CameraSelectionStatus` view
model, anchors it to the measured HUI surface rectangle, and emits the typed
`SetUserCamera` command. No camera registry or selection heuristic is
duplicated in the exposure producer.

Camera and exposure updates are reactive: camera status is rebuilt after its
selection, viewport, camera-entity, or track inputs change; it emits a
`CameraSelectionStatusChanged` event; and the camera exposure observer updates
the retained snapshot from that event. The exposure registry advances only
when a value changes, and the retained UI applies only new exposure revisions
or lifecycle changes. Production Rhai does not use `on_tick` for this path.

The `celestial-view` surface applies the same boundary to the lunar map. The
engine resolves the authoritative local avatar and driven vessel, reads the
canonical `SurfacePose.geodetic`, and publishes a pure equirectangular
projection plus explicit loading, no-fix, and valid-fix properties through the
existing `celestial-view` namespace. HUI/Flair owns the map grid and marker
presentation. Twin policy owns only the boolean `[settings] ui.lunar_map`
setting; omission means hidden. The workbench's existing Twin-scoped
`RuntimeSurfaceLayouts` owns the map window override and clears it on
`TwinClosed`, so camera/world transforms cannot alter its rectangle. The map
must not derive coordinates from transforms, duplicate pose state, or keep a
stale marker across avatar, Twin, or scene lifecycle changes.

### Ownership and visibility

Moving every overlay into the Twin would mix persistent project policy with
transient session state. The correct split for the shipped surfaces is:

| Surface/state | Owner |
|---|---|
| reusable template, style, binding, action, and placement | `runtime_surfaces.json` |
| subject identity and surface membership | composed USD `lunco:ui:surfaceId` |
| `possessed` / `always` visibility | built-in Rhai policy, selected by authored `lunco:ui:visibilityMode` |
| model-specific presentation and telemetry selection | active Twin/model Rhai policy and authored USD/Modelica outputs |
| recording surface selection | active Twin `runtime.ui.recording` policy |
| schema, terrain, tutorial, notification, and performance state | their existing authoritative lifecycle/resources |
| user preferences and window overrides | existing settings/workspace owners |

The workbench's **Settings ▸ HUD** submenu remains a presentation over existing
settings/workspace owners. It does not become a second visibility registry.

Surface policy is authored in Twin Rhai where it is project-specific. Do not
persist live progress, current selection, or network state in the Twin merely
because the pixels are rendered by HUI.

Modelica values remain authoritative at their authored network boundaries.
Rhai presentation policy may select and label those typed telemetry values for
a surface, while the generic exposure registry preserves authored units and
availability states.

The same policy boundary applies to simulation participants: Rust exposes
typed participant facts and authored public outputs, while Twin Rhai chooses
which values become a presentation property. Missing, compiling, paused, and
failed states remain explicit instead of being hidden until a sample exists.

Subject-scoped publication is lifecycle-scoped. It accepts only prims below the
current primary `SceneMountState::active_root`; preview, additive, and outgoing
roots cannot keep publishing after a replacement begins. Duplicate composed
projections and duplicate authored surface IDs are rejected visibly so one
retained namespace cannot represent two subjects. Scene-derived snapshots are
withdrawn synchronously at teardown, then rebuilt from the next active scene.

Offline recording is Twin-owned. While capture is active, the
`runtime.ui.recording` policy receives typed lists of stable manifest IDs and
currently visible IDs, and returns the IDs required in the shot. The runtime
validates that selection and acknowledges actual retained/rendered surfaces
through the status bus. Unknown, hidden, duplicate, or malformed selections
produce an explicit readiness error; capture selection is not stored in the
reusable manifest.

Bindings are deliberately explicit. A target property must first be declared
by the template; otherwise the bridge ignores it. If `bindings` is omitted, the
bridge tries every exposure property against a template property with the same
name. This is convenient for small surfaces, but explicit bindings are easier
to review when a capability is shared by several interfaces.

### 2. Declare properties and structure

The stable runtime contract uses a deliberately small HUI vocabulary:

```html
<template>
  <property name="title">Status</property>
  <property name="state">offline</property>

  <node id="status-root">
    <text id="status-title">{title}</text>
    <text id="status-state">{state}</text>
    <button id="status-pause" on_press="runtime_status_pause">
      <text>Pause</text>
    </button>
  </node>
</template>
```

Use `<template>`, `<property>`, `<node>`, `<text>`, and `<button>` with stable
`id` values, property interpolation (`{title}`), and `on_press` callbacks. A
button's visible caption must be a nested `<text>` node; raw text directly under
`<button>` is not part of the retained HUI contract and can render as an empty
control.
HUI supports more template features, but features outside this contract need a
real surface test before they become a shared interface convention. A surface
callback receives only the pressed entity. The manifest turns it into a
`RuntimeUiAction`; Rust then maps that semantic action to the existing typed
command/event path.

There is no DOM query, JavaScript execution, direct resource mutation, or
widget-specific Rust callback in an authored template. Inputs, forms, text
editing, lists with virtualisation, modal queue/outcome integration, and
accessibility semantics are not part of the current surface contract; use
egui/workbench panels or add an explicit engine capability before assuming
those features exist. The shared `lunco-ui::modal` host remains the owner of
scrim, focus, Esc dismissal, queued outcomes, and typed `CloseModal` dispatch.

### 3. Style the contents

Flair is CSS-like and maps properties to Bevy UI components. The practical
surface subset includes:

- `display`, flex layout, supported grid layout, absolute positioning,
  dimensions, `margin`, `padding`, and gaps;
- borders, radii, backgrounds, colors, opacity, z-order, and transforms;
- text size, weight, color, alignment, and inherited font properties;
- `#id`, type, class, descendant/child, state selectors such as `:hover` and
  `:active`, `@import`, custom properties, `var(...)`, transitions, and
  keyframe animations.

Keep the outer rectangle in the manifest and use CSS for the contents of that
rectangle. For a value that changes at runtime, expose a property and consume
its mirrored `--ui-<property-name>` custom property:

```css
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

The bridge projects each bound value both to the HUI template property and to a
CSS variable named `--ui-<property-name>` (underscores become hyphens). Mapping
values can themselves be CSS values, for example `var(--accent-color)`.

### 4. Publish capabilities

An engine producer resolves authoritative state and writes generic values:

```rust
let mut ui = exposures.writer("mission-status");
ui.visible(has_mission);
ui.property("title", mission_title);
ui.property("state", state_label);
ui.property("state_color", "var(--ok-color)");
```

The actual producer should be scheduled with the engine and protected by
Bevy change detection or a producer-owned fingerprint. Continuous inputs are
coalesced to the bounded exposure cadence (`EXPOSURE_UPDATE_HZ`, currently
20 Hz); discrete lifecycle facts should use an event observer. Camera status
is the latter. `EngineExposures.revision` advances only when a visibility flag
or value actually changes; it is not a frame counter.

The render-world readiness acknowledgement keeps its derived set of visible
required namespaces at that same revision boundary. Stable frames therefore
reuse the set while still querying live `RuntimeUiSurface` roots and extracted
UI nodes; a changed exposure revision rebuilds the visibility set before the
next acknowledgement.

Use `ReadExposures` when inspecting a running session or building another
consumer:

```sh
curl -s -X POST http://127.0.0.1:4101/api/commands \
  -H 'content-type: application/json' \
  -d '{"type":"ExecuteCommand","command":"ReadExposures","params":{"surface":"mission-status"}}'
```

The response contains `revision`, `visible`, and typed properties. A remote
consumer can retain its own view and skip rebuilding while the revision is
unchanged.

## Placement and workbench coexistence

Runtime surfaces use the existing `WorkbenchEguiHost` / `PrimaryEguiContext`
camera. They do not spawn a second UI camera and do not duplicate egui hit
testing.

The three placement modes are:

| Mode | Use |
|---|---|
| `viewport` | Full-window overlay, useful for HUDs that frame the 3D scene. |
| `dock_panel` | Surface aligned to a workbench panel, using its authoritative `PanelRects` rectangle and an optional logical-point `inset`. |
| `window` | Fixed logical-point rectangle anchored at a window corner or center, with an offset and explicit width/height. |

Set `interactive: true` only when the surface contains controls that should own
pointer input. A control becomes an input owner by declaring HUI
`on_press="callback"`; the runtime uses that explicit marker plus Bevy's
computed layout rectangle and registers it with the same scene/chrome press
latch as egui dock cards. The surface root is never registered: a full-window
HUD therefore remains transparent to scene clicks and camera drags everywhere
except on its authored buttons.

The manifest owns the outer rectangle; CSS owns the internal layout. HUI
replaces a template root's `Node` while building, so placement is applied after
HUI and Flair have finished their authoritative style work. The runtime pins
the root's minimum and maximum size to the resolved rectangle and clips child
overflow at that boundary; authored profiles must fit their contents within
the registered size. The correction is change-detected for the initial build,
an actual resize, or a style rebuild — not a per-frame placement loop. During
startup, a zero-sized target is ignored until the primary window has valid
dimensions; this prevents top-center surfaces from appearing at a temporary
negative position.

Use the workbench for the editor shell, docking tree, rich text/code editors,
large inspectors, and controls that need semantics not provided by this small
runtime layer. Use runtime HTML/CSS for authored Twin-facing presentation that
should be changed without recompiling Rust.

## Reloading without relaunching

On a native desktop session, the Bevy asset watcher reloads runtime UI assets:

| Edit | What reloads | Relaunch? |
|---|---|---|
| `assets/ui/*.html` | HUI rebuilds the affected retained surface tree. | No |
| `assets/ui/*.css` or an imported stylesheet | Flair reparses and reapplies the affected stylesheet. | No |
| `assets/ui/runtime_surfaces.json` | The manifest bridge replaces the registered surface roots and re-registers actions. | No |
| `assets/ui/runtime_fonts.css` | The stylesheet is reapplied; verify the font asset is available. | No, normally |
| Rust exposure producer or action observer | Requires a rebuild of the production binary and a controlled session replacement. | Yes |
| `ReloadShader` | Reloads WGSL materials only; it is not an HTML/CSS reload. | No, but unrelated |
| `RunScenario` | Hot-reloads Rhai policy only; it is not an HTML/CSS reload. | No, but unrelated |

Keep one production `luncosim` process while editing assets. For a rebuilt
binary, send the API `Exit`, verify the old process and port are gone, then
launch the new `target/debug/luncosim` with an explicit `--api PORT`. The
headless/server composition does not link the runtime UI or its file watcher.
On web, the normal static asset bundling/cache workflow applies; native file
watching is the fast authoring loop.

HUI's upstream limitation is relevant when templates are nested: reloading a
component template can temporarily break a higher-level template. Reload the
top-level surface template again if that occurs. Avoid recursive imports, keep
one root per template component, and do not manually write Bevy styling
components from Rust under a runtime surface; HUI/Flair will overwrite them.

## Performance rules

The runtime layer is designed to be cheap, but CSS does not make a large UI
free:

1. Keep templates small and retained. Prefer a few surfaces with stable ids to
   thousands of per-value nodes.
2. Publish only values a surface can read. Use `Changed<T>`, resource change
   ticks, revisions, and dirty flags before resolving a view model.
3. Let the exposure cadence coalesce continuous telemetry. Do not publish a
   string every render frame unless the presentation genuinely needs it.
4. Let identical values remain identical. The registry revision gate is the
   retained UI's invalidation boundary; do not introduce JSON hashing or a
   per-frame HTML rebuild.
5. Mount optional surfaces lazily through visibility and gates. A hidden
   surface has no HUI presentation tree; when exposure returns, the runtime
   creates a fresh root from the manifest so template-owned build state cannot
   survive a hidden transition. The bridge also destroys any retained root
   whose exposure or presentation gates turn off even if its lifecycle marker
   says it is currently unmounted; deferred HUI rebuilds must not leave a stale
   overlay visible after the authoritative exposure is false.
6. Prefer CSS custom properties and text replacement over rebuilding the
   hierarchy. Use transitions sparingly and measure large animations.
7. Profile the complete frame: runtime UI, egui, physics, terrain, and the
   renderer share the same budget. A fast HTML tree cannot compensate for a
   GPU-bound scene.

## CSS and font limitations

This is a minimum native UI language, not full CSS. Current Flair limitations
include:

- no browser DOM, JavaScript, networking, or browser layout engine;
- no full CSS standards guarantee; unsupported properties are reported or have
  no effect rather than being polyfilled;
- one stylesheet per styled subtree; use `@import` for shared rules;
- no global stylesheet, and `!important` is detected but ignored;
- custom-property fallback values are not supported;
- `calc()` is limited by Bevy's `Val` types; `calc(100% - 20px)` is not a
  safe assumption, while calculations using compatible variables are useful;
- limited font support: one explicit `@font-face`, with no browser-like local
  or fallback font chain;
- unsupported web units and features such as `em` and `text-decoration` are
  not available just because they exist in browser CSS;
- no automatic form controls, focus model, accessibility tree, or DOM event
  propagation contract beyond the authored HUI callbacks.

Use the bundled font through `@import "ui/runtime_fonts.css"`. Bevy's minimal
`default_font` does not cover the full Unicode range needed by telemetry and
status glyphs; relying on it produces tofu on some platforms. If a surface
needs a new glyph set, bundle and explicitly author the font asset rather than
assuming a host-installed font.

## Debugging checklist

- Surface absent: check the namespace, `visible`, perspective, gate, and a
  valid placement rectangle.
- Blank value: check that the target `<property>` is declared and that the
  binding source name matches exactly. Mapped values are exact strings.
- Button does nothing: check the same callback spelling in HTML and manifest,
  then confirm a host observer handles the semantic action.
- Tofu: inspect `runtime_fonts.css` and the bundled font path.
- Surface jumps on startup or resize: keep placement in the manifest and
  preserve the change-detected post-style placement boundary; do not add a
  per-frame compensation system.
- Style keeps reverting: remove Rust writes to `Node`, `BackgroundColor`,
  `TextColor`, or HUI style components beneath the surface and let Flair own
  presentation.
- Need to inspect the engine side: query `ReadExposures` and compare its
  revision before looking at rendering code.
