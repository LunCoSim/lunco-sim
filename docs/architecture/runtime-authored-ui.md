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

## Version and capability baseline

The committed dependency baseline is `bevy_hui 0.8.0`, `bevy_flair 0.9.0`, and
Bevy `0.20.0`. Keep the committed `Cargo.lock` as the version
evidence and re-check upstream release notes before changing this contract.

The shipped contract deliberately uses the smallest stable subset:

| Capability | Runtime contract |
|---|---|
| HUI templates | `<template>`, typed recursive `<property>` values, retained `<node>`, `<text>`, `<button>`, and authored row templates |
| HUI events | semantic `on_press` callbacks, including Rhai-authored `tag:action` values and typed `tag:*` parameters; `on_enter`, `on_exit`, `on_spawn`, and `on_change` remain available for a tested surface-specific need |
| Flair styling | authored `#id` selectors, flex/limited grid layout, overflow clipping, custom properties, transitions, animations, media queries, and bundled font imports where supported by the current Bevy style components |
| LunCo bridge | revision-gated typed exposures, Rhai-owned view models, keyed collections, retained scrolling, explicit placement, and typed command/event dispatch |

`bevy_hui_widgets 0.6.0` is a separate optional crate, not a dependency of
LunCoSim. It supplies primitive text-input, slider, and select components, but
not a browser form model, clipboard/caret editing, validation, keyboard
navigation, accessibility tree, or rich modal outcome contract. Do not add it
merely to make a HUD look like a web form. Simple authored viewport modals use
the generic `modal`/`dismiss_action` surface fields; rich source editing and
dialogs remain owned by the existing egui workbench.

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

HTML button ─► HUI callback ─► RuntimeUiAction(action, HookValue map)
          ─► Rhai policy ─► typed command/event
```

Each side owns one concern:

| Layer | Owns | Does not own |
|---|---|---|
| Engine/domain producer | authoritative values, sampling, visibility, capability names | HTML ids, CSS, layout, widget-specific code |
| `EngineExposures` | typed named snapshots and change revision | renderer or presentation policy |
| `runtime_surfaces.json` | reusable surface registration, bindings, gates, actions, modal/input policy, placement | simulation state, subject selection, and visibility policy |
| active Twin `LunCoPolicy` / Rhai | subject visibility, scalar presentation, recording selection | retained tree, simulation authority, and renderer details |
| `.html` template | retained tree shape, declared properties, callback names | ECS queries, domain commands, CSS layout |
| `.css` stylesheet | appearance, internal layout, transitions, custom-property mapping | engine state and command dispatch |
| workbench / egui | shell, editors, complex panels, docking tree | runtime surface content |

The generic exposure namespace is the contract. Do not create a special
`domain_to_view` path or a Rust producer for one particular HTML template. Ports,
telemetry, physics, scripts, and derived capabilities all publish through the
same registry.

Runtime surface facts expose the subject's retained `SimulationObjectPaused` as
`pause: #{condition, detail}`, or unit when that subject is running. This is a
typed snapshot of the existing co-simulation owner state, installed with the
ordinary runtime-properties hook and invalidated when the marker changes. The
active Twin's Rhai policy chooses failure wording from this retained condition;
it must not infer a paused state from a transient model output. Scene teardown
retires the subject and its prepared exposure lease. The Roadster overload gate
checks coupled pause and its windowed HUD consumer.

Runtime surface facts include the ordered owner/reason pairs from
`SimulationProgress`. The active Twin's Rhai properties policy chooses the
visible status wording, while the typed producer remains domain-neutral.
The vessel status shows `WAITING FOR TERRAIN` while required ground is held,
including while a download awaits user action. Changes to these holds
invalidate surface properties through the existing
exposure refresh path, so the built-in Rhai visibility policy can temporarily
show an authored surface during scene, terrain, script, or Modelica preparation
before possession and before the first physics step. Once preparation clears,
the authored `possessed` or `always` mode controls visibility again; neither
Rust exposure code nor the HUI template adds a second visibility rule.

The application dataset surface uses the existing `assets.provision` hook,
declared by `lunco-core-session` and installed by application/Twin policy.
Its typed input map contains scope identity, prompt preferences, and registry
dataset states, including unsigned byte counters and processing/failure detail.
Its closed output map contains visibility, title, description, scoped rows,
and semantic actions. `lunco-luncosim-ui` validates and publishes that view;
Rhai owns labels, progress formatting, retry/cancel actions, and completion.
The hook is optional at the host boundary: absence or an invalid result warns
and publishes no consent view, while the native resource panel and registry
remain available. It does not authorize network activity; user actions dispatch
the typed dataset commands.

An accepted download stays visible through transfer and preparation. The user
can dismiss the surface to continue in the background; completion remains
explicit until dismissal. Suppression persists through the Twin manifest.
`TwinClosed` retires the scope and queued presentation. The production Rhai
gates are `test_download_policies.rhai` for policy states and
`dataset_provisioning_surface.rhai` for live scope/action and dismissal behavior.

The same rule applies to engine health. The core runtime publishes the latest
frame and physics facts once as typed `EngineHealthSnapshot` and
`PhysicsHealthSnapshot` resources. The engine snapshot also retains a bounded
raw frame-time history for app-level performance presentation. The
read-only `EnginePerformance` query exposes that same bounded history and its
process-scoped sample counter for render-cadence measurements while paused.
Simulation-clock telemetry retains its own sampling cadence. The
renderer-independent exposure layer projects the latest facts into the ordinary
`engine-health` namespace. Native egui, authored HUI, API readers, telemetry
adapters, and recorders consume those publications; none of them scans
`DiagnosticsStore` or the physics schedule for their own copy. A new HUD value
therefore requires a producer-side exposure property and a normal authored
binding, not a HUD-specific reader.

Application interaction has two distinct wall-clock cadences. Typed commands
advance the application `command` clock when the shared `CommandOccurred` fact
is published. One-shot Rhai/REPL work advances the independent `repl` clock
when the queued script is actually evaluated by `drain_world_scripts`. A queued
`RunRhai` request therefore appears once in each relevant phase, with no
simulation-time or UI-time substitution. Their sequence, elapsed time, last
interval, and rate are published through the generic `application-cadence`
namespace for status, HUD, telemetry, and recording consumers.

This is a set of owned publication boundaries, not a catch-all event bus:

| Fact class | One publication owner | Readers |
|---|---|---|
| Scalar participant inputs/outputs | `PortRegistry` | co-simulation, commands, inspector, telemetry, scripts, API |
| Presentation-ready engine/domain facts | `EngineExposures` | HUI, egui adapters, API, telemetry/recording views, remote clients |
| Lifecycle and discrete occurrences | typed events and revisioned resources | the owning schedule's observers |
| Application command/REPL cadence | `ApplicationCadence` → `EngineExposures` | status, HUI, telemetry, recording |

Readers must use the boundary for their fact class and must not recreate a
parallel cache or direct source query for a presentation surface.

Subject-scoped surfaces are discovered from composed USD. A prim opts in with
`lunco:ui:surfaceId`, and `lunco:ui:visibilityMode` supplies the mode consumed
by the built-in Rhai policy (`possessed` or `always`). The surface ID is the
exposure namespace; Rust does not construct names from model classes, slots, or
vehicle counts. A Twin may author, override, or omit this metadata on its
scene-level opinions. Keep these fields on a prim with `LunCoUiSchemaAPI`.
Live edits invalidate authored-surface discovery at the USD stage-revision
boundary; they do not recreate the prim subtree or reset simulation state below
the same scene scope.

## Where to work

The shipped luncosim surfaces live in [`assets/ui/`](../../assets/ui/):

- [`runtime_surfaces.json`](../../assets/ui/runtime_surfaces.json) registers each
  surface.
- `*.html` files describe retained structure and declared properties.
- `*.css` files describe the presentation and import
  [`runtime_fonts.css`](../../assets/ui/runtime_fonts.css).
- [`assets/ui/README.md`](../../assets/ui/README.md) is the short asset-level
  reference.

The reusable runtime bridge is in
`crates/lunco-workbench-runtime-ui/src/lib.rs`; the application host in
`lunco-luncosim-ui` supplies capture mode and app-specific action handling.
Runtime snapshot projection is in
`crates/lunco-luncosim-exposures/src/lib.rs`; it is intentionally independent
of domain calculations. Modelica and Rhai own domain values and
policy, while `lunco-workbench` supplies the
egui host camera, dock rectangles, and scene-pick ownership.

## Twin-owned presentation assets

The shared `JsonAssetScopeChanged` lifecycle admits one typed
`lunco.runtime-ui-manifest.v1` document per application or open Twin scope.
The application manifest supplies shared surfaces; the active Twin's manifest
replaces matching surface IDs and adds new ones. Duplicate IDs/namespaces within
a composed manifest, duplicate manifests in one scope, and invalid contracts
produce a structured runtime fault. No generated copy of application assets is
needed. Unrelated JSON kinds retain their own owners.

Template, stylesheet and collection-template paths in a manifest are relative
to that scope's asset root. The owner qualifies them with the exact `lunco://`
or `twin://` authority before publishing the retained tree. Explicit logical
URIs are also accepted; a Twin stylesheet can import shared fonts with
`@import "lunco://ui/runtime_fonts.css";`. The native asset readers own subsequent
HTML/CSS dependencies and reloads.

The manifest cache belongs to open source scopes. Scene teardown retires the
resolved presentation and republishes it even while paused; exact `TwinClosed`
identity removes that Twin's source. Late snapshots from a retired Twin are
rejected. Replacing the active Twin rebuilds the retained surface roots from the
current composition, restoring application surfaces when no override exists.
Rhai still owns exposure contents, visibility and action meaning. Rust owns only
source admission, validated composition and retained-tree lifecycle.

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
| `bindings` | Optional map from template property name to exposure property name. A `map` translates exact rendered values. `css: true` explicitly projects a binding into a CSS custom property; prose stays in template text. |
| `actions` | Maps a unique HUI callback name to a built-in semantic action or a Twin-authored action identifier. Built-ins are handled by the host; other identifiers become the typed `runtime.ui.action` event for Rhai policy. |
| `visible_in_perspective` | Optional workbench perspective id or array of 1–32 distinct nonempty ids. The same filter gates interactive rendering and capture readiness; omission means unrestricted. |
| `gate` | Optional named host gate. Unknown gates are closed. |
| `setting` | Optional namespaced boolean in the active Twin's `[settings]` table. The surface is hidden when the value is false. |
| `setting_default` | Value used when `setting` is absent, including when no Twin is active. This is authored per surface; Rust has no per-setting field. |
| `interactive` | Enables input ownership for authored controls carrying HUI `on_press`; only those controls' computed Bevy UI rectangles enter the existing chrome/scene pick gate. The surface root and a `viewport` placement never claim the full window. |
| `modal` | Requires an interactive viewport surface and makes its visible root claim the full-window scene-pick gate. This is the generic simple-modal primitive, not a rich dialog queue. |
| `dismiss_action` | Optional authored semantic action emitted by Escape while a modal surface is visible. Rhai owns what dismissal means. |
| `draggable` | Allows a `window` surface root to move with primary-button dragging. The user position is clamped to the live logical target, keyed by stable `id`, and reset to the authored anchor with a primary-button double click. `viewport` and `dock_panel` roots cannot opt in. The shipped `celestial-view` switcher opts in and also has a surface-specific reset in Settings ▸ HUD. |
| `placement` | The outer rectangle and its relationship to the workbench. |

The manifest loader rejects unknown fields, duplicate surface IDs/namespaces or
callbacks, unsafe relative paths, empty contract names, and non-finite or
non-positive window geometry before any surface is mounted. A `modal` surface
must be interactive and use viewport placement; its optional `dismiss_action`
is an authored semantic action. Authored semantic actions are intentionally
open-ended; their meaning belongs to the active Twin Rhai program, not to the
runtime UI crate.

The retained UI picker uses Bevy's marker filter because its camera renders
after the scene camera and otherwise every visible UI node blocks the scene,
including decorative or transparent layout nodes. The runtime marks only
visible `on_press` controls on interactive surfaces and visible HUI nodes in
draggable window surfaces, including their roots, as `Pickable`; those are the
retained nodes that own pointer input. The active presentation camera is marked
for UI picking when a runtime surface is mounted. This keeps ordinary HUI
controls and surface dragging on the native pointer path while allowing
unmarked UI layout to pass scene hits through.

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

The lunar mission-site action is a distinct HUI card, separate from the body
mode selector, while both remain children of the movable `celestial-view`
surface. Its visibility follows the typed `mission_view_available` exposure.
Its animation duration and surface-mode engage/release altitudes are persisted
user camera settings in `CameraInputSettings`; Rhai reads the animation duration
through `get_setting`, and `SetCameraInput` updates all three values.

The camera-status surface is the reference composition. Rust publishes the
full current camera fact (`active_name`) and a deterministic compact identity
projection (`active_label`) through the generic `camera-status` exposure. Rhai
adds a typed option array and authored dimensions to that snapshot. The
manifest declares only the option field names and the trigger action; the
authored HUI button is the stable trigger, and the shared egui renderer reads
the generic records. It forwards the selected authored action as a typed
`runtime.ui.action` event. Rhai handles that event through the normal typed
command bridge, so the runtime UI crate does not know camera names, camera
commands, or camera selection rules. HUI 0.7 has no native select/accessibility
tree; the generic egui primitive is therefore the low-level dropdown option,
not a camera widget.

The same contract is usable for any option list: the exposure value is an
array of maps, the manifest identifies the key/label/action fields plus
`width_source` and `max_height_source`, and Rhai supplies both records and
dimensions. HTML/CSS can bind those dimensions for the authored trigger while
the egui popup uses the same values for its bounded content area. Missing or
malformed dimensions are an authored UI error and do not silently fall back to
a Rust size. While the popup is open, its bounds are registered with the shared
scene-pick gate so option clicks stay in the popup instead of reaching the 3D
scene underneath.
The shared `lunco-usd-bevy-camera::camera_switch::camera_display_labels` policy
is used by the exposure, Camera menu, USD prim tree, entity tree, and
Inspector: unique authored leaves stand alone; duplicate leaves gain the
nearest owner context and then additional ancestors; generated
hexadecimal/UUID-like owner suffixes are omitted; an unavoidable normalized
collision gets a small ordinal. The full USD path remains the selection
identity and is available through tooltips, diagnostics, and `active_name`,
never replaced by the display projection.

Rejected camera actions are shared runtime errors, which the application
presents through its warning toast. The camera owner keeps one canonical
contract finding set and projects it to `RuntimeDiagnostics` and Recent Events.
An unresolved director track is an error while director control is active, and
a warning while a valid operator-selected camera owns the viewport. Recent
Events presents camera findings as warnings and never routes them through the
runtime-error toast path. Standalone-framing failures use this same contract
projection instead of publishing a second camera diagnostic.

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

The same surface shows a separate Focus mission site card only while the Moon
orbital pin is active and the scene has a valid lunar site anchor. It publishes
the site's position as a typed f64 vector in the Moon inertial frame,
independently of rover possession. Rhai computes the target direction, reads
the persisted camera animation-duration setting, and sends the generic
`AnimateOrbitCameraDirection` command. The camera moves along its existing
orbit while preserving its current distance and vertical offset.

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
The built-in properties policy counts statuses through indexed participant
access and passes only those counts to its formatting helper. Rhai array
iteration and by-value helper arguments can copy nested input/output maps;
presentation helpers should receive only the fields they consume. Custom
policies retain the complete typed participant facts at the existing hook
boundary.

Subject-scoped publication is lifecycle-scoped. It accepts only prims below the
current primary `SceneMountState::active_root`; preview, additive, and outgoing
roots cannot keep publishing after a replacement begins. Duplicate composed
projections and duplicate authored surface IDs are rejected visibly so one
retained namespace cannot represent two subjects. Scene-derived snapshots are
withdrawn synchronously at teardown, then rebuilt from the next active scene.

Offline recording is Twin-owned. While capture is active, the
`runtime.ui.recording` policy receives typed lists of stable manifest IDs and
currently visible IDs, and returns the IDs required in the shot. Candidate
visibility uses the same exposure, perspective, named gate and Twin setting
predicate as mounting and rendering. A perspective or gate change refreshes
the contract even when exposure values stay unchanged. Mounting and render
acknowledgement are subsequent readiness checks, not candidate filters.
The runtime
validates that selection and acknowledges actual retained/rendered surfaces
through the status bus. Unknown, hidden, duplicate, or malformed selections
produce an explicit readiness error; capture selection is not stored in the
reusable manifest.

Initial recording readiness requires a mounted tree that has consumed a real
exposure snapshot, camera targeting, nonzero layout and render extraction.
Later global exposure revisions do not invalidate that initial projection:
live producers may publish after the UI update phase. Property updates keep
using the revision fast path independently of the presentation lifecycle.

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
Every template, including a keyed collection row, must use `<node>` as its
single root and put `<button>` and `<text>` elements beneath it. HUI reuses the
template scope entity during reload, while Flair retains selector metadata on
that entity; a typed leaf element at the root can therefore panic when its type
is applied again. Leaf controls below the root are recreated with the subtree.
HUI supports more template features, but features outside this contract need a
real surface test before they become a shared interface convention. A surface
callback receives only the pressed entity. The manifest turns it into a
`RuntimeUiAction`; built-ins remain in the host, while a Twin-authored action
is forwarded as a typed `runtime.ui.action` event. Rhai owns program/path
selection, editor focus, source switching, and USD transactions; the template
never writes simulation state.

For dynamic controls, HUI supports the project convention
`on_press="runtime_ui_authored_action" tag:action="{action}"`. The
`action` tag value is a dynamic scalar property supplied by the active
exposure/Rhai policy, and the typed event resolves it from the pressed HUI
node. Other `tag:*` values form a typed `HookValue` map on the event. The
runtime transports this map without concatenating it into the action name;
Rhai owns the interpretation. This supports Twin-defined actions without
JavaScript or a JSON payload.

There is no DOM query, JavaScript execution, direct resource mutation, or
widget-specific Rust callback in an authored template. Inputs, forms, text
editing, rich modal queue/outcome integration, and accessibility semantics are
not part of the current surface contract; use egui/workbench panels or add an
explicit engine capability before assuming those features exist. A visible
authored modal blocks the shared scene-pick gate and Escape emits its declared
semantic dismiss action. The shared `lunco-ui::modal` host remains the owner of
focus, queued rich-dialog outcomes, and typed `CloseModal` dispatch.

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
its explicitly selected `--ui-<property-name>` custom property (`"css": true`
on the manifest binding):

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

The bridge projects bound values to HUI template properties. A binding with
`css: true` additionally writes a CSS variable named `--ui-<property-name>`
(underscores become hyphens). Text-only bindings and implicit same-name
bindings never enter the CSS token stream. Mapping
values can themselves be CSS values, for example `var(--accent-color)`.

The runtime retains HUI's compiler style context outside its active style
system. Property updates restore that context for the public HUI compiler,
flush dynamic text/tags/actions, and retire it before style application.
Flair remains the only rendered style owner; collection controls therefore
update their semantic action with their label, including Cancel and Retry.
The context is an entity-owned component and retires with its presentation tree.

The Update-phase style handoff follows HUI Build, exposure compilation, surface
interactions and collection reconciliation, then precedes HUI Style. It assigns
selector names and caches/removes runtime `HtmlStyle` before HUI can apply its
inline defaults to retained nodes. Flair prepares and applies CSS in PostUpdate;
an Update ordering edge to a Flair set does not order that later schedule.
Non-runtime HUI templates retain their inline styles. The low-level
`hui_rebuild_preserves_retained_runtime_styles_only` regression exercises the
real HUI style system and the shared production handoff configuration.

Authored HUI nodes use stable `id` attributes and `#id` selectors. Flair also
supports class selectors, but HUI 0.7 does not create a `ClassList` from an
HTML `class` attribute; it parses that attribute as an unknown style property
and rejects the template.

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
20 Hz); discrete lifecycle facts should use an event observer. The preparation
owner compares the complete typed facts, telemetry, derived vessel data and
caller identity before admitting work. Equal snapshots retain their published
view; advancing the caller's time sample or sequence alone does not rerun a
data projection. Clocked animation belongs to the presentation owner. Policy
generation, source lifetime, caller route/phase/clock/producer and changed
inputs still admit a fresh projection. The immutable admitted snapshot is
shared with its worker and is retired with its surface or scene.
Camera status uses event observers. `EngineExposures.revision` advances only when a complete surface
changes (including its preparation state) or is retired; it is not a frame counter.

`lunco-luncosim-exposures` captures immutable facts and typed caller context,
then evaluates pure visibility and property policies through shared bounded
async admission. Headless and windowed consumers use this same path. The owner
retains one job plus the latest pending input per surface, and atomically commits
validated results. Policy generation, active source revision and subject identity
fence stale completions; closing a Twin retires its surfaces immediately.
Existing ready samples remain available during continuous refresh. New surfaces
or replaced identities start `pending`, hidden with no properties. Invalid
policy results become `failed`, with an error and no partial view. Such failures
remain terminal until the source or identity changes. Native synchronous domain
producers publish `ready` views through the same registry.

Only owned values and pure hook registrations leave the application thread.
The native host uses workers; wasm currently schedules the same kernel as a
cooperative local task. Neither path changes simulation clocks or permits UI
completion to choose an authoritative simulation transition.

The registry and authored metadata caches follow the shared
[scene resource lifetime contract](61-scene-lifecycle-and-teardown.md).
Namespaces default to scene lifetime, with application retention declared at
the producer through `retain_across_scenes`.

The runtime-surface producer keeps authored USD metadata behind its existing
active-root and `UsdStageRevision` cache. That includes surface membership,
program source metadata, and per-participant declared public output names.
Simulation status, telemetry, participant outputs, and Rhai policy results
remain live at the bounded publication cadence. Do not reread composed USD to
rebuild those static facts on each publication or add a second revision owner.
Public output projection resolves that authored name set through
`PortMap::slots_for_names` and reads the selected live samples in map order.
Missing or unsampled names produce no value. The complete output map remains
available to policy alongside this subset; scalar values retain their exact
`f64` representation. Selection needs no retained handles or second index.

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

The response contains `revision` and each surface's `state` (`pending`, `ready`,
or `failed`), `error` (a diagnostic on failure, otherwise unit), `visible`, and
typed properties. Consumers wait for `ready` before reading prepared properties,
and can retain their view while the revision is unchanged. Inspect `failed`
rather than repeatedly waiting for an invalid policy.

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
| `ReloadShader` | Queues reload for active WESL assets only; bare engine paths resolve the canonical `lunco://` identity, explicit sources stay exact, and an empty path covers every loaded WESL asset. A missing active target fails visibly. | No, but unrelated |
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
  propagation contract beyond the authored HUI callbacks. The optional
  `bevy_hui_widgets` crate is not this contract; its primitive widgets need an
  explicit LunCo owner and acceptance tests before they can replace egui
  editors or dialogs.

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

## Script-driven HUD cards

`lunco-workbench-guided-ui::hud` owns Rhai hint/objective/action cards separately
from coach tours and spotlights. The host's `GuidedHudVisibility` supplies the
default perspective restriction. LunCoSim chooses View (`sandbox_view`). Authored
Rhai chooses exceptions with `hud_perspectives(ids)`; `[]` means all perspectives.
Unknown perspective ids reject before changing state. One rule covers all card
content and pointer input. Hiding a card preserves state; visible cards stack
by stable owner identity and measured height.

Implicit cards belong to the executing script's stable identity. Unscripted
API/REPL calls use a distinct command-context card; there is no fabricated
script identity for that context. Explicit `hud`
names identify scene-owned cards shared by several producers; Twin identity
fences both kinds. `hint(hud, text)`, `objectives_hud(hud, records)`, and
`guided_hud_actions(hud, tool, hook, actions)` address the same named card.
Single-producer scripts use the overloads without `hud`. Named scene cards are
appropriate when a scenario and a callback tool update the same presentation.
There is no tutorial classification, current-writer selection, or asset-name
policy in the engine.

Action sets are validated and stored with a required `ToolHookBinding` entity
lease. Replacing or clearing actions despawns the previous lease. Clicks carry
that exact entity to `RunRhaiToolHook.binding`, alongside typed action data.
The shared tool queue checks the lease, immutable endpoint, registered tool
scope, active Twin, and scene generation before admitting and executing work.
All queued tool calls pin their admitted scene transaction, including menu
callbacks. A callback queued before a reset cannot execute against the next
instance of the same scene or Twin.

The action row's egui identity includes its binding entity. Button identity is
stable within that immutable action set, including its authored order.
Replacing the action set therefore retires pointer/keyboard interaction identity
as well as queued execution; a release cannot activate a replacement endpoint.

Script component removal immediately retires its implicit card, even while the
script driver is paused or preparing. The language-neutral script lifecycle
also emits `ScriptInstanceRetired` after stop
or executable invalidation. The HUD owner clears that script's implicit card
and despawns its bindings. `SceneTransitionStarted` clears all cards and leases;
`TwinClosed` clears that Twin's cards and leases. During script teardown,
idempotent content/action clears may update an existing card; they never create
a card. New content, scope changes, and callback bindings reject, so stop hooks
cannot recreate retired presentation.
`ClearGuidedOverlay` clears the caller's implicit card plus coach presentation;
other cards retain their state. `InspectGuidedHud` exposes the same per-card
visibility/input decision used by rendering for runtime verification.

Perspective policy stays in authored Rhai. Rust owns identity, membership checks,
validated callback transport, and teardown. The production windowed gate is
`assets/scripting/tests/test_guided_hud.rhai`; generic lifecycle and queued-lease
seams have Rust tests. A headless acknowledgement does not verify presentation.

Semantic HUI actions, dropdowns, Escape dismissal, and native menu actions carry
`RuntimeUiActionScope`, captured at input emission. The host validates the
explicitly captured Twin and scene before interpreting even a built-in action.
The tool queue then pins that same admitted scene for deferred execution. This
fences both event delivery and callback execution; stale input never acquires
the replacement Twin's identity from handler timing.

Script-opened scene context menus retain this same scope while open, validate it
before painting or dispatch, and clear on scene transition or owning Twin close.
Each popup owns a fresh ECS root and child `ToolHookBinding` leases. Its root
scopes egui widget identity, so replacing a popup also retires any held gesture.
Dismissal arms after the opening pointer transaction ends and uses egui's
layer-aware outside-click contract.
Replacement and dismissal despawn all unselected endpoints. Selection transfers
only its endpoint to the shared tool queue with `ToolHookBindingRetention::Invocation`;
the queue retires it after execution or rejection. HUD rows use `Surface`
retention and keep their binding until their owner replaces or clears it.
Scene transitions retire queued tool work immediately, including while the
interpreter is preparing; application REPL work retains its workspace lifetime.
Stop hooks cannot open menus.

Installed workbench menus use provider-owned ECS roots and child endpoint
leases with `ToolHookBindingLifetime::Provider`. Each installation scopes its
widget tree to that fresh root identity and the current input Twin/scene.
Input is disabled while a scene transition is pending. Replacement atomically
publishes a validated tree and despawns the outgoing root; `TwinClosed` retires the owning
Twin's contribution. Application contributions remain installed across scene
resets. Selecting an item creates a child `Invocation` lease pinned to the
input's Twin and committed scene generation. Provider replacement revokes that
child even before command admission; scene replacement rejects it at admission
or retires it from the queue. The provider endpoint stays available for a fresh
selection in the next scene. No menu callback dispatches by name alone.

Deferred Rhai work retains its caller's typed origin, actor, authority and
client-local restrictions. Its phase and clock are independent of which queue
executes it; a nested `RunRhai` or tool call from `on_stop` remains a stop call
and cannot publish new presentation. Twin-route snippets also retain the
caller's scene generation and retire on scene transition. Idempotent cleanup
stays available while that owning scene remains current.

Run `scripts/api/test_guided_ui_callbacks.py --port PORT` against the production
binary for held-press popup replacement, fresh selection, outside dismissal,
scene teardown, installed menu replacement/reuse, deferred stop hooks and Twin
replacement, plus document pointer-subscription registration and cleanup.
The wrapper drives window input; the
isolated Twin's `hud_binding_probe.rhai` library supplies the runtime verdicts.
The wrapper creates temporary application Rhai/JSON sources for native menu
replacement and removes them after the owned session exits. The maintained USD
fixture supplies the lifecycle clock for the stop-hook checks.
Injected input acknowledgements mean queued input, not completed callbacks.
The driver waits for a rendered input frame, then completes an authored no-op
hook on the same FIFO interaction queue before running its verdict. A REPL
query alone does not establish that the preceding UI callback has executed.

The API dispatcher scopes each deferred request's correlation around its typed
handler and uses registered command type identity to select deferral. Admission
failures answer immediately; admitted work completes through its owner. This
keeps concurrent requests distinct and prevents a rejected callback from leaving
its transport waiting.

Subject surface facts include `coordinates` from `SurfacePoseQuery`: IAU/WGCCRE
body-fixed latitude (north positive), longitude (east positive), and height above
the body's mean sphere, all f64. Compact and detailed vessel HUDs share the
coordinate-formatting policy, including degrees, hemispheres and height datum.
The detailed HUD binds its frame label and numeric height from that policy;
it does not replace an unavailable coordinate with a native world-Y value.
It must not label these coordinates
`MOON_ME` or `MOON_PA`: those require an explicit frame transform. In a scene
without a site anchor the coordinate mode is explicitly world metres; an
unresolvable or ambiguous site frame publishes coordinates as unavailable.

### Mission card layout and camera windows

Named guided HUD cards use `SetGuidedHudLayout` for bounded width, text scale
and dragging. Their native context menu submits this same command; layout and
content are scene owned and inspectable through `InspectGuidedHud`. Group
selection and persisted Twin preferences remain authored Rhai policy. See
`assets/scenarios/tests/guided_hud_layout.rhai` for production admission and
negative cases. Independent camera image windows are owned by
[`lunco-capture`](render-decoupling.md#live-camera-observations), with exact
camera selection and per-window render targets; they are not HUI image nodes.
