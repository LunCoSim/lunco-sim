# Coupled simulation suspension

Status: production mechanism qualified on 2026-10-22; engineering failure
detection and the three complete Twin physics suites remain open.

`lunco-cosim` owns `PauseSimulationObject`; backend execution consumes the
`lunco-core::SimulationObjectPaused` contract. This extends the existing physics
disable operation because Avian cannot suspend Modelica, FMI or authored script
execution. Engineering limits, damage equations and mission reactions remain in
Modelica, authored data and Rhai. There is no second detector, motor model or
solver in this change. The current contract is documented in
[the co-simulation architecture](../architecture/22-domain-cosim.md#coupled-object-suspension).

The existing event bus, command bridge, body-disable owner, backend clocks and
script driver are the real consumers. Scope resolution runs on a pause request,
not every frame. It uses current mechanical ownership and live port wiring.
Supervisory access declarations are permissions, not ownership edges: a mission
observer remains able to inspect the suspended system and request recovery.

Validation used the regular `target/debug/luncosim`, built with `tracy`, `-j 4`
and sccache. The production `physics_object_pause` gate passes 11 checks;
`paused_wheel_effort` passes 16 checks after normal pre-pause integration. It
includes physical and raycast wheels, a continuous model connected from outside
the rover hierarchy, two stopped object controllers, and independent physics,
continuous state and controller execution. Both processes exit successfully.
Evidence is under `.cache/codex-evidence/physics-followup-20261021/` in
`native-e4-analytic-wheel-runtime`.

These runs use the retained native Avian `e4f073b` pin and ordinary authored
wheel hinges. They qualify suspension; they do not qualify complete vehicle
driving or impact behavior. The dense candidate pin and native failure bridge
remain outside the application. FMI compilation is covered by the production build;
the new suspension behavior has no external-FMU runtime qualification yet.
Automatic physical/numerical failure routing, calibrated absorption and damage,
scoped restart, live topology replacement and model/solver state handoff remain
separate work. Busy worker rejection preserves the existing complete receipt
transaction instead of discarding an in-flight result.
