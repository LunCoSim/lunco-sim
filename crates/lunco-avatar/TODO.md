# lunco-avatar — TODO

## Camera smoothing

Baseline (shipped): frame-rate-stable exponential-decay follow with per-camera
`damping` — see `spring_arm_system` / `orbit_system` in `src/lib.rs`. Open work
below builds on that.

### Follow-ups

- [ ] **Pluggable smoothing functions.** Today the only curve is exponential
      decay. Add a choice of easing/smoothing functions (exp decay,
      critically-damped spring / SmoothDamp, ease-in-out, etc.) so collision
      pull-in and zoom glide can have a nicer feel than pure exp.
- [ ] **Tunable smoothing time.** Expose a smoothing *time constant* (or
      half-life in seconds) instead of the raw `rate` Hz number — more
      intuitive to dial for feel. Play with values to find the good range.
- [ ] **Make all of the above camera properties.** Smoothing function +
      time/rate + damping should be per-camera fields (on `SpringArmCamera`,
      `OrbitCamera`, `ChaseCamera`), falling back to `CameraDefaults`, the same
      way `damping` already does. A cinematic orbit can then differ from a
      snappy chase cam.

### Before building our own: check existing Bevy facilities

Smoothing is extremely common — likely don't need to hand-roll the math.

- **Bevy core already has it.** `bevy::math::StableInterpolate` provides
  `smooth_nudge(&mut self, target, decay_rate, delta)` — exactly the
  `1 - exp(-decay_rate · dt)` form we wrote by hand, frame-rate independent.
  The official [Smooth Follow example](https://bevy.org/examples/math/smooth-follow/)
  uses it. We could replace our hand-rolled exp lines with `smooth_nudge`.
- [`smooth-bevy-cameras`](https://crates.io/crates/smooth-bevy-cameras) —
  camera controllers with exponential smoothing baked in.
- [`bevy_dolly`](https://lib.rs/crates/bevy_dolly) — "dolly rig" abstraction;
  `Smooth::new_position()` / `Smooth::new_rotation()` driver components.
- [`bevy_easings`](https://crates.io/crates/bevy_easings) — easing-function
  plugin (the curve library, if we want named easings).
- [`bevy_map_camera`](https://crates.io/crates/bevy_map_camera) — 3D camera
  controller with easing/tweening, as a reference design.

Decision pending: adopt `StableInterpolate::smooth_nudge` for the math (cheap,
in-tree) and keep our spring-arm/collision logic, vs. lean on a full controller
crate. Our floating-origin `Grid` + `CellCoord` positioning means a drop-in
camera crate probably won't fit without adaptation — but the smoothing *math*
(`smooth_nudge`) and easing *curves* (`bevy_easings`) are reusable as-is.
