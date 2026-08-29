# lunco-avatar — TODO

## Camera smoothing

Baseline (shipped): frame-rate-stable exponential-decay follow with per-camera
`damping` — see `spring_arm_system` / `orbit_system` in `src/lib.rs`. Quaternion
follow reuses Bevy's `StableInterpolate`; the position path keeps the same decay
law in f64 so BigSpace coordinates are not rounded through f32.

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
      `OrbitCamera`), falling back to `CameraDefaults`, the same
      way `damping` already does. A cinematic orbit can then differ from a
      snappy full-attitude chase.

### Existing facilities considered

Smoothing is extremely common — likely don't need to hand-roll the math.

- **Bevy core owns the quaternion primitive.** `bevy::math::StableInterpolate` provides
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

Decision: use `StableInterpolate::smooth_nudge` for f32 quaternion follow and
keep the spring-arm/collision and BigSpace f64 position logic in the camera
owner. A full controller crate would still need to own these grid/frame
transactions, so adopting one would add a second pose owner rather than remove
code.
