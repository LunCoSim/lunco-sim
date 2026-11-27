# Runtime UI assets

These files are authored retained Bevy UI surfaces for the running `luncosim`
client. They are not browser documents: there is no DOM, JavaScript, network
fetch, or full HTML/CSS implementation. The architecture and limitations are
documented in [`docs/architecture/runtime-authored-ui.md`](../../docs/architecture/runtime-authored-ui.md).

The committed baseline is `bevy_hui 0.8.0` with `bevy_flair 0.9.0` on Bevy
`0.20.0`. HUI is the template/event layer and Flair is the retained Bevy CSS
style layer; neither is a browser DOM.

## Choose the surface owner

Use `"owner": "application"` in the application manifest for controls that span
Twin changes. The `startup-assets` entry is the shipped example.
Use the default `workspace` owner for controls that act on the active Twin or
scene. The `asset-consent` entry uses this default. Both entries use
`"overlay": true` to paint above dock panels. Keep the visible card pickable so
its measured bounds also own pointer input.
[Manifest examples](runtime_surfaces.json)

Application and Twin manifests use the same `lunco.runtime-ui-manifest.v1`
format. A Twin can add or replace workspace entries by ID.
Asset paths resolve relative to the manifest's application or Twin root. Use
`lunco://ui/runtime_fonts.css` to import the shared fonts from a Twin.
[Ownership and publisher requirements](../../docs/architecture/runtime-authored-ui.md#application-and-workspace-surface-ownership)

## Stable surface contract

Runtime templates currently rely on:

- HUI `<template>`, `<property>`, `<node>`, `<text>`, and `<button>` elements;
- standard text and number `<input>` elements with Twin-scoped form fields;
- stable `id` values and `{property}` interpolation;
- `on_press="callback_name"` for semantic actions;
- authored horizontal, vertical, or two-axis scroll areas;
- Flair `#id` selectors, flex/limited grid layout, absolute positioning,
  dimensions, spacing, borders, backgrounds, colors, text properties,
  `display`, custom properties, and `var(...)`.

Every template, including a keyed collection row, must have one `<node>` root.
Put buttons and text elements beneath that root: HUI reuses its scope entity
while rebuilding the tree, and a root leaf element can reapply its Flair type
name to retained selector metadata.

Set `tag:ui_scroll_area` on a clipped `<node>` to `vertical`, `horizontal`, or
`both` to enable pointer scrolling on the authored axes.
Horizontal-only areas also map an ordinary vertical mouse wheel to horizontal
movement; trackpads and Shift-wheel use their native horizontal delta.

For tabs that are pages of one working area, add a manifest `workspace` to one
surface. Its HTML shell provides `workspace.host`, authored tab controls, and
`tag:ui_tab_id` values matching page IDs. The selected control receives the
`is-active` CSS class. Each page keeps its own Twin HTML, stylesheet, exposure
namespace, bindings, collections, data sources, and dropdowns. Prefer one
workspace `selection_setting` containing the active page ID; Rhai owns updates
to that key. Keep per-page boolean settings only for older workspaces. The
containing surface owns one stable position and size for every page.

HUI and Flair support additional features, but a feature outside this contract
needs a real surface test before it becomes a shared interface convention.
LunCoSim supplies a narrow native form contract for text and number inputs,
including pointer focus, caret editing, character limits, numeric filtering,
tab order, and Twin-scoped values. Keyed row templates can interpolate
properties into input tags as well. This layer still has no DOM querying,
JavaScript, accessibility semantics, browser-style event propagation, slider
or select controls, automatic validation, or rich dialogs. The separate
`bevy_hui_widgets 0.6.0` crate has primitive text-input, slider, and select
components, but it does not supply the complete editor, focus, clipboard,
validation, accessibility, or rich-dialog contract required by LunCoSim. It is
not part of the runtime UI dependency set. Simple authored viewport modals may
use the runtime `modal`/`dismiss_action` contract; rich dialogs remain in the
shared `lunco-ui::modal` egui host.

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

Each exposed value is projected into its declared template property. A manifest
binding with `css: true` also projects a CSS custom property named
`--ui-<property-name>`; ordinary prose never enters the CSS token stream.
Give each bound surface property an explicit template default. HUI only
registers a surface property when its declaration contains a value.
The registry stores typed text,
boolean, f64, array, and map values, not CSS or HUI types. A manifest binding
may map exact rendered values such as `true`, `false`, or `301` into
presentation values.

HUI callbacks are semantic runtime actions. Built-in actions remain registered
by the runtime, while Twin-authored actions are forwarded as typed
`runtime.ui.action` events for Rhai policy. A dynamic control can use one
shared callback and a HUI tag property:
`on_press="runtime_ui_authored_action" tag:action="{action}"`.
Additional `tag:*` values are carried as a typed `HookValue` parameter map; the
runtime never encodes an action and its arguments into a name string. The
pressed node supplies the action value; the template does not inspect HTML ids
or call domain resources. This is the reusable path for program selection,
route editing, consent rows, and other Twin-defined tools without a Rust
callback per item.
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

When a lunar scene has a valid site anchor and the Moon orbital pin is active,
the `celestial-view` surface shows a separate Focus mission site card below the
body mode selector. The engine publishes the scene site's position as a typed
f64 vector in the Moon's inertial frame. Rhai chooses the target direction and
reads the persisted `CameraInputSettings.orbit_direction_animation_duration_s`
property (0.6 s by default). A generic camera command moves the view along its
current orbit, preserving the orbital distance and vertical offset. The mission
view is available even before a rover is possessed.

The persisted `CameraInputSettings.surface_mode_engage_altitude_m` property
sets the surface-mode engage altitude and orbital zoom floor (1,000 m by
default). `surface_mode_disengage_altitude_m` defaults to 2,000 m to preserve
hysteresis. Rhai can change both with `set_camera_surface_mode_altitudes(...)`.
Returning from Moon/Earth orbit to Surface view eases to the saved local camera
pose over 1.2 seconds by default. Rhai can tune
`CameraInputSettings.orbit_return_transition_duration_s` with `set_camera_input(...)`.

The body-mode selector and optional mission-site card stay in the root's
normal vertical flow. The map is absolutely positioned below whichever controls
are visible inside the movable surface rectangle. Its authored default is
top-centre; users can drag the window and use View ▸ HUDs ▸ Built-in HUDs ▸ Reset position to
remove the per-Twin override.

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

Use `ReadExposures` to check the namespace, visibility and bound properties
when iterating on a surface. Exercise application controls with a Twin open
and again after changing or closing the Twin. Exercise workspace controls
against their current scene and check them again after replacement.

Changing Rust producers or action observers still requires a rebuilt binary and
a controlled session replacement. `ReloadShader` and `RunScenario` reload
other systems and do not reload HTML/CSS. The headless/server feature does not
link this UI or its file watcher; web builds use the normal bundled-asset cache
workflow.

The shipped surfaces include the rover HUD, camera-status card, celestial view
switcher, terrain progress card, startup-assets card, scene asset-consent
window, and networking scenario-download card. The two asset notices consume
independent exposure namespaces and share the scoped Rhai action path.
[Asset preparation contract](../../docs/architecture/asset-processing.md#desktop-startup-assets)

The View ▸ HUDs submenu splits **Built-in HUDs** (application-owner controls
and application manifest surfaces marked `built_in_hud`) from **Twin HUDs**
(active manifest, guidance cards and authored floating perspective panels).
Every row offers Auto, Show and Hide; eligibility is reported separately. Supply a human-readable `title` separately from the stable surface
id. A checked HUD can be unavailable in the current perspective or scene.
Camera-status follows the active Twin's generic `ui.camera_status` setting
in automatic mode and defaults on when that key is absent. The HUD choice
can show or hide it for the current scene, and **Auto** restores the
Twin setting without modifying `twin.toml`. Camera status follows the interactive
main scene viewport, including Twin-authored scene perspectives. Rover, terrain, download, and tutorial HUDs retain their authored visibility
conditions but can be suppressed by the user. Modal and dock-panel surfaces remain lifecycle-controlled. Essential workflow
overlays can declare `"user_dismissible": false`; other free-standing surfaces
default to dismissible, independently of their paint layer. Camera-status has one runtime visibility row that does not edit the Twin
manifest. Celestial-view uses the native settings row. Both window placements
use `"region": "scene_viewport"` to stay inside the measured scene leaf instead
of obscuring adjacent docked panels.
Rhai owns camera selection and can
read the current camera fact through `get_exposure("camera-status", "active_name")`;
camera changes update the exposure through an event observer. Rich text editors
and UTC date editing remain workbench-owned egui panels until explicit
text-input semantics are added to this contract.
