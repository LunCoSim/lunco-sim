# lunco-celestial-spatial-core

Reusable ECS-facing contracts for celestial spatial state.

This crate owns the semantic frame index, canonical surface pose query, ENU
surface-frame helpers, scene body declarations, orbital-view state, cached
local-gravity facts, and render-independent connectivity state shared by
cameras, avatars, networking, scripting, telemetry, and UI. It also publishes
solar-tracking and Wi-Fi endpoint contracts consumed by USD projection and the
runtime adapter. It does not install the celestial runtime or own terrain,
globe, link solving, imagery, cadence, or asset integration.

Use `lunco-celestial-spatial` when the application needs to run the celestial
scene projection and its runtime systems.
