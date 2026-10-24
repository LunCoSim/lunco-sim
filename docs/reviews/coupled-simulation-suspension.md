# Coupled simulation suspension

Status: coupled suspension and authored contact-speed validity detection
qualified on 2026-10-24. The three complete Twin physics suites remain open.

`lunco-cosim` owns `PauseSimulationObject`; backend execution consumes the
`lunco-core::SimulationObjectPaused` contract. This extends the existing physics
disable operation because Avian cannot suspend Modelica, FMI or authored script
execution. Engineering limits, damage equations and mission reactions remain in
Modelica, authored data and Rhai. The current contract is documented in
[the co-simulation architecture](../architecture/22-domain-cosim.md#coupled-object-suspension).

The existing event bus, command bridge, body-disable owner, backend clocks and
script driver are the real consumers. Scope resolution runs on a pause request,
not every frame. It uses current mechanical ownership and live port wiring.
Supervisory access declarations are permissions, not ownership edges: a mission
observer remains able to inspect the suspended system and request recovery.

Validation used the regular `target/debug/luncosim`, built with `tracy`, `-j 4`
and sccache. The production `physics_object_pause` gate passes 12 checks,
including the retained warning severity. Current receipts and captures are under
`.cache/codex-evidence/performance-20261023/instability-latest/` and remain
untracked. The unchanged suspension mechanism also has the retained
`paused_wheel_effort` 16-check qualification from 2026-10-22: physical and raycast
wheels, an externally connected continuous model, stopped object controllers,
and independent systems that continue running.

The contact validity gate uses the current native Avian `9097ce9` pin.
`contact_impact_pause` passes ten checks: a native 30.13 m/s contact exceeds
the authored 5 m/s test envelope, both joined bodies and their Modelica observer
stop, one sourced warning retains the measured limit, and a lower-speed
independent contact and model keep running. The impacted body also carries a
continuous model, qualifying independent native port ownership through startup
and suspension. The graphical host's eight-check observation passes its
authored verdict and shows the warning in the status strip. Expanding the
history through synthetic pointer input was not qualified in that run.
These gates do not qualify complete vehicle driving or impact behavior.
FMI compilation is covered by the production build;
the new suspension behavior has no external-FMU runtime qualification yet.
General numerical failure routing, calibrated absorption and damage,
scoped restart, live topology replacement and model/solver state handoff remain
separate work. Busy worker rejection preserves the existing complete receipt
transaction instead of discarding an in-flight result.
