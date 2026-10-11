# Numeric experiments

Offline experiments resolve their solver through the capability registry and
prepare a checked Rumoca 0.10.2 FMI component. BDF is the implicit default;
RK45 is available for non-stiff systems. Tolerance defaults to `1e-6` and remains
editable per run. Output cadence is independent of the solver's internal steps.

Invalid bounds, unsupported solver requests, malformed models, cancellation,
and retained-output budget exhaustion produce terminal run results. Diagnose
failures from the current compiler or solver report before changing tolerance.
A repeated failure time alone does not establish a compiler or solver defect.

Parameter sweeps reuse the compiled DAE and apply values through the supported
simulation options. Native and browser runs share `drive_run`, result admission,
and source identity. Live prediction uses its separate fixed-step policy.

See [Rumoca runtime contracts](../architecture/29-rumoca-runtime-contracts.md),
[experiments](../architecture/25-experiments.md), and
[real-time Modelica coupling](../architecture/28-modelica-realtime-physics.md).
