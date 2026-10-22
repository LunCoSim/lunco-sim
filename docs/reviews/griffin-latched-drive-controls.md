# Native joint drive controls

Griffin's ramp investigation needs live motor-effort control without replacing
its native hinge, changing its target, or reloading the scene. The existing
`lunco-physics` dimensional drive setter and snapshot are the authoritative
owner. They now cover native revolute and prismatic coordinates through the
same interface. `max_effort` means force in N or torque in N m; angular gains
and targets use radians. USD degree conversion remains in the USD reader.

The consumer is authored Rhai. The World bridge retains its existing entity
capability and mutation fences. Invalid coefficients reject before mutation;
zero effort retains the native constraint and its target. Positive infinity
retains the native unlimited-cap semantic. A changed drive wakes its sleeping
dynamic endpoints through the same owner used by cosimulation target writes.
The Avian f32 conversion is explicit and range checked.

Hook review: the physics owner applies validated coefficients supplied by the
caller. It does not select latch identities, deployment phases or drive laws.
Those lifecycle decisions belong to Twin-scoped Rhai, with entity declarations
covering the affected hinges. No new routing hook or Twin-specific Rust policy
is needed. Failure to obtain a native force-law snapshot or apply a requested
law must stop that authored action visibly. No scene-wide or name-only state
is retained by this mechanism.

The production `native_force_drives` Rhai observer checks coordinate units,
angular effort suppression, target retention, exact restoration, and invalid
law rejection. Its analytic force/torque checks remain enabled. Griffin's
mission consumes the interface through `griffin_surface_ramp_ops`: it retains
native profiles in the scene's mission state, suppresses latched effort and
restores each released pair on its existing command event. The integrated
mission passed all 16 checks in its recorded pre-cleanup source cohort,
including sampled hinge settlement, without changing drive gains, torque caps
or physical acceptance limits. The latest Griffin model completes deployment
but fails a later surface-route stage; this is not current full-mission acceptance.

The force-drive scene passes 24 checks. Its stop-reaction cancellation envelope
is relative to the opposing dimensional actuator load, rather than comparing
newtons to a unitless epsilon: the measured residual is 0.0000771 N against
80 N, within 16 native binary32 epsilons (0.0001526 N). Analytic dynamics and
conservation checks remain independent. Landing stability and high-energy
engine-off impact still require separate qualification.
