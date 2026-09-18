# Runtime UI assets

These files are authored retained Bevy UI surfaces for the running `luncosim`
client. They are not browser documents: there is no DOM, JavaScript, network
fetch, or full HTML/CSS implementation. The architecture and limitations are
documented in [`docs/architecture/runtime-authored-ui.md`](../../docs/architecture/runtime-authored-ui.md).

The pinned 2026-09 baseline is `bevy_hui 0.7.0` with `bevy_flair 0.8.1` on
Bevy `0.19.1`. These are the current compatible releases in the committed
lockfile. HUI is the template/event layer and Flair is the retained Bevy CSS
style layer; neither is a browser DOM.

## Stable surface contract

Runtime templates currently rely on:

- HUI `<template>`, `<property>`, `<node>`, `<text>`, and `<button>` elements;
- stable `id` values and `{property}` interpolation;
- `on_press="callback_name"` for semantic actions;
- Flair `#id` selectors, flex/limited grid layout, absolute positioning,
  dimensions, spacing, borders, backgrounds, colors, text properties,
  `display`, custom properties, and `var(...)`.

HUI and Flair support additional features, but a feature outside this contract
needs a real surface test before it becomes a shared interface convention.
Forms, text inputs, DOM querying, JavaScript, accessibility semantics, and
browser-style event propagation are not supplied by this layer. The separate
`bevy_hui_widgets 0.6.0` crate has primitive text-input, slider, and select
components, but it does not supply the complete editor, focus, clipboard,
validation, accessibility, or modal contract required by LunCoSim. It is not
part of the runtime UI dependency set. The `lunco-ui::modal` queue/outcome
contract, scrim, focus, and typed close dispatch therefore remain in the shared
egui modal host.

## Data and actions

Engine capabilities publish named snapshots through
`lunco_exposure_core::EngineExposures`. The template never reads ECS state or
mutates simulation state. Ports, telemetry, physics, scripts, and derived
capabilities use the same exposure boundary.

Subject-scoped surfaces are selected by composed USD metadata, not by a Rust
list of models or slots. Author `lunco:ui:surfaceId` on the scene/model prim;
that value is the exposure namespace and must match a manifest namespace.
`lunco:ui:visibilityMode` is consumed by the built-in Rhai policy and supports
`possessed` and `always`. Twin policy may replace visibility and presentation
without changing Rust.

Offline recording is Twin-owned. The `runtime.ui.recording` policy returns the
stable surface IDs required for the current capture, and the recorder validates
the selected IDs against visible retained surfaces before capture.

Each exposed value is mirrored as a declared template property and as a CSS
custom property named `--ui-<property-name>`. The registry stores typed values,
not CSS or HUI types. A manifest binding may map exact rendered values such as
`true`, `false`, or `301` into presentation values.

HUI callbacks are semantic runtime actions. Built-in actions remain registered
by the runtime, while Twin-authored actions are forwarded as typed
`runtime.ui.action` events for Rhai policy. A dynamic control can use one
shared callback and a HUI tag property:
`on_press="runtime_ui_authored_action" tag:action="{action}"`.
The pressed node supplies the action value; the template does not inspect HTML
ids or call domain resources. This is the reusable path for program selection,
route editing, and other Twin-defined tools without a Rust callback per item.
Unknown fields/actions and unsafe asset paths are rejected before mounting.

Use stable `id` attributes for authored HUI nodes and `#id` selectors in their
stylesheets. Flair supports class selectors, but HUI 0.7 does not turn an HTML
`class` attribute into a `ClassList`; the loader treats that attribute as an
unknown style property and rejects the template.

The generic `program-browser` surface demonstrates this contract. A Twin or
Rhai policy enables it on a USD scope, while the runtime exposes only the
direct `LunCoProgramAPI` children and Rhai owns adding, selecting, editing, or
switching a program source. Its `programs` array is rendered by the generic
keyed collection host from the authored `program_browser_row.html` template;
there is no fixed row count or program-specific Rust view model. Collection
hosts retain authored row order and own wheel scrolling inside their clipped
list; Rhai still owns the records, ordering, and actions.

The camera-status card binds the deterministic compact `active_label` projection;
the full `active_name` remains available to runtime consumers. Its authored HUI
button is a generic dropdown trigger. The manifest names the typed option-array
fields (`camera_items`, `key`, `label`, and `action`) and the two Rhai-owned
dimension properties (`dropdown_width` and `dropdown_max_height`). Rust only
renders that generic record shape and forwards the selected action; the camera
policy in Rhai turns those actions into typed camera commands. HUI 0.7 has no
native select/accessibility tree, so the shared egui dropdown is the low-level
renderer and is not camera-aware.

Other surfaces can reuse the same dropdown contract for programs, routes,
telemetry channels, or any other typed option records. The exposure registry
keeps scalar, array, and map values typed across the Rust/Rhai boundary; HTML
and CSS can bind the same authored size properties for the trigger while Rhai
controls the popup dimensions.

The `celestial-view` surface also owns the authored lunar map. Rust resolves the
local avatar's driven target through `TheLocalEmbodiment` and `ControlLink`,
projects that vessel's canonical `SurfacePose.geodetic` into the map's
equirectangular marker coordinates, and publishes only typed status and marker
properties. The HUI/Flair template owns the map, grid, marker, and no-fix
states; it does not reconstruct coordinates or retain a second location model.
The map is opt-in per Twin through the boolean `[settings] ui.lunar_map` key;
an absent key is the hidden default. The existing `RuntimeSurfaceLayouts`
workspace state owns the draggable window override per Twin and clears it on
`TwinClosed`, so camera/world movement cannot move the map. When there is no
complete lunar surface pose, the exposure deliberately hides the marker and
reports the authored no-fix state.

The view switcher keeps its button card in the root's normal vertical flow and
anchors the map below it. The map is absolutely positioned inside the movable
surface rectangle, so centering both children would let the later map panel
paint over the switcher controls. Its authored default is top-centre; users can
drag the window and use Settings ▸ HUD ▸ Reset position to remove the per-Twin
override.

## Performance and placement

The exposure registry is reactive: identical values do not advance its revision,
and producers coalesce continuous changes to a bounded presentation cadence
(currently 20 Hz). HUI/Flair only apply changed snapshots, asset reloads, or
change-detected geometry; they do not parse HTML/CSS on every render frame.
Optional surfaces mount lazily when their exposure, perspective, gate, and
placement are valid.

Runtime surfaces use the existing `WorkbenchEguiHost`/`PrimaryEguiContext`
camera. Full-window surfaces occupy the window. Docked surfaces use the
workbench's authoritative `PanelRects` rectangle and existing scene-pick
ownership; they do not duplicate dock widths, reconstruct egui hit regions, or
spawn a second UI camera. The manifest owns the outer rectangle and CSS owns
the contents; the runtime bounds the retained root to that rectangle and clips
overflow. `interactive: true` does not make the outer rectangle clickable:
only visible HUI controls with an authored `on_press` action register their
computed Bevy UI rectangles with the shared scene-pick gate. This keeps a
full-window HUD transparent to camera and scene input outside its buttons.
Placement is reapplied after HUI/Flair style changes through change detection,
not by a per-frame correction loop. A `window` surface may additionally declare
`"draggable": true`; its primary-button drag is clamped to the live target and
persisted per Twin through the workbench workspace state. A primary-button
double click removes that override and restores the authored anchor. Viewport
and dock-panel roots cannot be draggable, and stale layout ids are dropped when
the manifest is reconciled.

## Fonts and theme

Runtime styles should import `runtime_fonts.css`, which selects the bundled Fira
Sans asset. Bevy's minimal `default_font` does not cover the Unicode glyphs
needed by many telemetry/status surfaces; do not depend on a host-installed font.

HTML surface colors, spacing, and rounding are authored CSS custom-property
defaults. `lunco-theme` remains the semantic theme source for egui/workbench
consumers, but it does not overwrite HTML stylesheet variables every frame.

## Reloading

On native desktop, the asset watcher handles the following without relaunching:

| Edit | Result |
|---|---|
| `*.html` | Rebuilds the affected retained surface tree. |
| `*.css` or an imported stylesheet | Reapplies Flair styles. |
| `runtime_surfaces.json` | Replaces registered roots and action mappings. |
| `runtime_fonts.css` | Reapplies the font stylesheet; verify the font asset exists. |

Changing Rust producers or action observers still requires a rebuilt binary and
a controlled session replacement. `ReloadShader` and `RunScenario` reload
other systems and do not reload HTML/CSS. The headless/server feature does not
link this UI or its file watcher; web builds use the normal bundled-asset cache
workflow.

The shipped surfaces are the rover HUD, camera-status card, celestial view
switcher, terrain progress card, and networking scenario-download card. The
Settings ▸ HUD submenu is the user-facing view over the existing global HUD
owners and the active Twin's camera-status setting; it does not add a second
visibility registry. Camera-status is gated by the active Twin's generic
`ui.camera_status` setting and defaults on when that key is absent; set it to
`false` in `twin.toml` to hide it. Rover, terrain, download, tutorial, and
notification surfaces remain automatic because possession, authored scene, or
runtime lifecycle owns their visibility. Rhai owns camera selection and can
read the current camera fact through `get_exposure("camera-status", "active_name")`;
camera changes update the exposure through an event observer. Rich text editors
and UTC date editing remain workbench-owned egui panels until explicit
text-input semantics are added to this contract.
