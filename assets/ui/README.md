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
- Runtime data: Rust publishes a bounded typed view model; templates do not
  read ECS state or mutate simulation state.

The desktop GUI watches these assets. Editing the template or stylesheet emits
an asset modification event and rebuilds/restyles the retained Bevy UI tree
without relaunching the simulation. The headless/server feature does not link
this UI or its file watcher.
