# Runtime UI assets

These files are a small authored UI surface for the running Bevy client. They
are not a browser document and do not support JavaScript, a DOM, or the full
HTML/CSS standards.

The supported HUD contract is intentionally narrow:

- HUI template nodes: `<template>`, `<property>`, `<node>`, `<text>`, `id`, and
  `{property}` interpolation.
- Flair styling: `#id` selectors, `:root` variables, `var(...)`, flex layout,
  absolute positioning, dimensions, spacing, borders, backgrounds, colors,
  text size/weight/alignment, and `display`/visibility.
- Runtime data: engine capabilities publish named snapshots through
  `lunco_core::exposure::EngineExposures`; templates do not read ECS state or
  mutate simulation state. Ports, telemetry, physics, scripts, and derived
  capabilities use the same exposure boundary.
- HUI callbacks are semantic runtime actions. The adapter emits a typed action
  event; the host translates it into the existing command/event path. Authored
  templates never query or mutate domain resources.

Each exposed value is mirrored by the HTML adapter as a template property and
as a CSS custom property named `--ui-<property-name>`. CSS variables are a
presentation binding rule; the engine registry itself stores no CSS or HUI
types.

The exposure registry is reactive: identical values do not advance its revision,
and producers coalesce continuous changes to a bounded presentation cadence. HUI
and Flair only apply a changed snapshot, asset reload, or surface-geometry change;
they do not parse HTML/CSS on every render frame.

Runtime surfaces use the stable `WorkbenchEguiHost`/`PrimaryEguiContext` camera.
Full-window View surfaces occupy the window. Docked surfaces use the workbench's
authoritative `PanelRects` rectangle and existing egui scene-pick ownership;
they do not duplicate dock widths, reconstruct egui hit regions, or spawn a
second UI camera.

Runtime styles import `runtime_fonts.css`, which selects the bundled Fira Sans
asset as the deterministic UI font. Bevy's minimal `default_font` is ASCII-only;
runtime-authored telemetry must not depend on a host-installed font or render
Unicode symbols as tofu.

The shipped runtime surfaces include the rover HUD, celestial view switcher,
terrain-generation progress card, and networking scenario-download progress
card. Rich text editors and the UTC date field remain workbench-owned egui
panels until the minimal runtime contract grows explicit text-input semantics.

Theme colors are authored CSS custom-property defaults for HTML surfaces. The
engine still owns semantic theme selection for non-HTML consumers, but it does
not overwrite an HTML stylesheet's color variables on every update.

The desktop GUI watches these assets. Editing the template or stylesheet emits
an asset modification event and rebuilds/restyles the retained Bevy UI tree
without relaunching the simulation. The headless/server feature does not link
this UI or its file watcher.
