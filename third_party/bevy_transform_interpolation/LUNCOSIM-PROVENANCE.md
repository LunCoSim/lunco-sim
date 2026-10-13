# Transform interpolation source provenance

This is the published `bevy_transform_interpolation` 0.5.0 source
(MIT OR Apache-2.0), with its licences retained. Avian consumes it through the
root crates.io patch.
The original crate archive has SHA256
`ea40784f1c6a3f61333064ea1d1c7663ef677c1ce46b5ca148c46c4a62c27e62`.

Linear translation/scale, spherical rotation, and Hermite easing bound the
fixed-clock fraction to `[0, 1]` before evaluating their existing interpolation
functions. The simulation scheduler retains complete pending fixed cycles in
`Time<Fixed>::overstep` during admission holds and bounded catch-up bursts.
Those cycles are queued simulation demand; they cannot extend a render sample
beyond its two captured endpoints. A backlog presents the upper endpoint while
the fixed clock retains every pending tick. Extrapolation still constructs its
explicit predicted endpoint through the upstream velocity owner.

The production-shaped `queued_fixed_cycles_do_not_extrapolate_the_render_pose`
mechanism test in `lunco-usd-avian` checks retained time, solved/rendered position,
and unit rock rotations. The existing constant-velocity interpolation check
covers ordinary fractional easing. Source updates must preserve these bounds.

Imported documentation whitespace is normalized.
