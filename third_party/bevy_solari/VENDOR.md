# Bevy Solari source identity

User-approved vendoring scope: this crate and a public path-tracer ordering hook,
23 October 2026. No dependency patch was published.

- Source repository: https://github.com/LunCoSim/bevy
- Revision: `a12e8366cdd9c0ee8dabd3ada7bb7e8f02f0b338`
- Source directory: `crates/bevy_solari`
- Version: 0.20.0; original MIT and Apache-2.0 licences are included.
- Runtime source change: `src/pathtracer/mod.rs` exports
  `PathtracingSystems::Trace` and assigns the existing path-tracing system to it.
  The integrator, shaders, lighting and accumulation implementation are unchanged.
- Packaging changes: Cargo dependencies use their declared registry versions
  instead of repository-relative paths; standalone packaging removes inheritance
  of Bevy's workspace lint table. The application's existing `bevy_shader` patch
  continues to select its maintained WESL implementation.

The exact source patch is
[`docs/reviews/pathtracer-forward-composition.patch`](../../docs/reviews/pathtracer-forward-composition.patch).
The application uses the public set to order Bevy's native transparent pass
after the path-traced opaque image and before tonemapping. Transparent effects
are forward-rendered; this does not add ray-traced volumes or local-light support.
