# Runtime UI assets

These files are authored retained Bevy UI surfaces for the running `luncosim`
client. They are not browser documents: there is no DOM, JavaScript, network
fetch, or full HTML/CSS implementation. The architecture and limitations are
documented in [`docs/architecture/runtime-authored-ui.md`](../../docs/architecture/runtime-authored-ui.md).

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
browser-style event propagation are not supplied by this layer. It also does
not provide the `lunco-ui::modal` queue/outcome contract, scrim, focus, or
typed close dispatch; dialogs that need those semantics stay in the shared
egui modal host.

## Data and actions

Engine capabilities publish named snapshots through
`lunco_core::exposure::EngineExposures`. The template never reads ECS state or
mutates simulation state. Ports, telemetry, physics, scripts, and derived
capabilities use the same exposure boundary.

Each exposed value is mirrored as a declared template property and as a CSS
custom property named `--ui-<property-name>`. The registry stores typed values,
not CSS or HUI types. A manifest binding may map exact rendered values such as
`true`, `false`, or `301` into presentation values.

HUI callbacks are semantic runtime actions. Each surface has a stable manifest
`id`, and the manifest maps each unique callback name to a closed host action
(`view.surface`, `view.body.moon`, `view.body.earth`, `overlay.terrain.dismiss`,
`autopilot.toggle`, or `camera.picker.toggle`). The adapter emits a typed action
event, and the host translates that action through the existing command/event
path. Templates
never inspect HTML ids or call domain resources. Unknown fields/actions and
unsafe asset paths are rejected before mounting.

The camera-status card binds the deterministic compact `active_label` projection;
the full `active_name` remains available to runtime consumers. The button is
intentionally only the picker open intent. HUI 0.7
does not provide a dynamic repeated-list or payload-action contract, so the
existing egui host renders the camera options from `CameraSelectionStatus` at
the measured HUI anchor and emits the typed camera command. This keeps the
camera identity list single-sourced while preserving the authored trigger.

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
the contents. `interactive: true` does not make the outer rectangle clickable:
only visible HUI controls with an authored `on_press` action register their
computed Bevy UI rectangles with the shared scene-pick gate. This keeps a
full-window HUD transparent to camera and scene input outside its buttons.
Placement is reapplied after HUI/Flair style changes through change detection,
not by a per-frame correction loop.

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
camera-status card is gated by the active Twin's generic `ui.camera_status`
setting and defaults on when that key is absent; set it to `false` in
`twin.toml` to hide it. Rhai owns camera selection and can read the current
camera fact through `get_exposure("camera-status", "active_name")`;
camera changes update the exposure through an event observer. Rich text editors
and UTC date editing remain workbench-owned egui panels until explicit
text-input semantics are added to this contract.
