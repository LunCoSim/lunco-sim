# Rigid-body acceleration

`lunco-physics::kinematics` owns one `SolvedBodyAcceleration` per native
`RigidBody`. `PhysicsGatePlugin` installs it without UI, Modelica, or telemetry
dependencies. Sensors, the co-simulation port adapter and telemetry consume
the same completed measurement.

The native physics clock defines the interval. The producer captures its
baseline before integration and evaluates after the solver, including native
contacts and joints. A consecutive interval reuses the previous completed
velocity boundary, so an impulse applied between steps is included. Body
admission/removal, clock discontinuities and `ActivePhysicsFrame` changes seed
a fresh baseline; velocities in different frames are never differenced.
Repeated presentation frames do not generate measurements. Scene-owned body
components retire with their bodies, and removing readiness invalidates them.

The measured vectors are all `f64`:

- Net centre-of-mass acceleration: `a = Δv/Δt` in the physics frame.
- Angular acceleration: `Δω/Δt` in that frame.
- Specific force: `f = a - g`, in both physics and completed body frames.
- The graph's single accelerometer curve: `|f|`, in m/s².

`LocalGravity` is a physics-owned field contract computed by the environment.
Its f64 vector is read through `acceleration()`. Field samples also carry
`GravityChangeOrigin`: a changed source retains native wake notifications,
while resampling at a moving consumer refreshes the exact vector without
reporting a new external force. The environment retains its provider-pose
cache and chooses that provenance; acceleration measurements consume the same
vector in either case.
The producer uses the same local vector projected into Avian, plus Avian's
global gravity with its native `GravityScale`. It does not assume Earth,
subtract a scalar 9.81 from a magnitude, or classify every constant acceleration
as gravity. Missing gravity or an invalid attitude leaves the affected
specific-force result unavailable. Before the first complete finite interval,
`valid` is false; graphs do not record an invented zero. The existing raw IMU
inputs retain their declared zero initialization alongside `acceleration_valid`.

A supported body has approximately zero net acceleration and one local g of
specific force. Ideal free fall has the reverse. A body rotating in space
changes the components, not the magnitude. The native f32 attitude is checked
at its native precision, then widened and normalized in f64 before the body-frame
rotation. A native unit-tolerance check alone does not establish a unit f64
quaternion. This conversion preserves the orientation and the vector norm.
A real sensor displaced from the
centre of mass also needs `α × r + ω × (ω × r)` and any sensor error/filter
model; these belong to its mounted sensor model, not every graph widget.
Measurements average a native fixed interval: impact peaks depend on sampling
resolution, and are not presented as instantaneous sensor peaks.

The native port adapter exposes `acceleration_magnitude`,
`specific_force_magnitude`, `specific_force_body_x/y/z`, and validity. The
telemetry recorder publishes `linear_acceleration` and `specific_force`
magnitudes, with optional vector components, under the exact owning body.
It has no second velocity-difference cursor. Plotting selects a real signal;
widgets do not recalculate physics or perform per-axis gravity guesses.

This is a numerical observation mechanism, not changeable scenario policy;
there is no Rhai policy hook. Rhai owns test inputs and acceptance thresholds.
The production gate `assets/scenarios/tests/body_acceleration.rhai` exercises
support, applied force with changing attitude, free fall, and unavailable
initial measurements. Its fixture `assets/scenes/tests/body_acceleration.usda`
was authored through the typed USD document owner. Run it through the
production scene runner with verdict channel `BODY_ACCELERATION`.
