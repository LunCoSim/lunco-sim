# Native FMI 3 Co-Simulation

`lunco-fmi` owns native FMU archive admission, instance lifetime and FMI calls.
The common application simulation composition installs `FmiPlugin` for both
windowed and headless hosts. Native libraries execute on one serial worker;
requests and completed scalar samples cross the ECS boundary. Browser hosts
cannot load native FMU binaries and do not register `LoadFmu`.

`LoadFmu` accepts an asset identity or native file path, an instance name,
optional named Float64 parameters/initial inputs, and `communication_ticks`.
The default interval is one master physics tick. Longer intervals are positive
integer multiples of that tick; they do not create a separate wall clock.
Fixed internal FMU steps must divide the admitted interval. The command
acknowledges admission; native initialization failures become a terminal
`fmi-execution-failed` runtime fault and stop shared simulation advancement.

The adapter currently admits unclocked scalar Float64 input/output ports.
Other external port types, arrays, Scheduled Execution clocks, external
execution tools and single-instance-per-process FMUs are rejected explicitly.
Model-description interpretation uses the maintained `fmi-rs` parser; native
calls use generated `fmi-sys` FMI 3 bindings. LunCoSim owns bounded archive
admission, clock transactions, logging and native instance lifetime.

`lunco-port-core::ScalarPortContracts` owns the live scalar engineering
contract shared with Modelica. Admission resolves direct and declared-type FMI
units and limits once, retaining SI exponents, the radian exponent, scale,
affine offset and `relativeQuantity`. A unit without `BaseUnit` remains
explicitly unresolved. The common port registry publishes these facts and
validates writes; the native owner also validates initialization values and
returned samples. Contract fingerprints update only on metadata mutation, so
ordinary samples do not hash unit definitions or rebuild topology.

Each participant has the shared core runtime's integer `TickClock`.
Global pause and speed controls affect master admission, while local pause
stops that participant's child clock. Communication endpoints derive from
integer ticks. The native boundary converts those seconds into the independent
variable's declared time unit, including fixed internal intervals and returned
timestamps. Omitted time units mean seconds under FMI; unresolved or non-time
independent units are rejected. `SetFmuPaused {target, paused}` applies local
pause through the typed command/authority boundary, including from presentation
programs.
The adapter retains and validates native `lastSuccessfulTime`;
it never substitutes the requested endpoint for the FMU's returned time.
Early returns, input discontinuities and Event Mode iterations remain private
until the requested communication interval completes. Work does not advance
the master clock: `SimulationProgress` holds subsequent complete cycles until
accepted results arrive through `SimulationCompletions`.
The native headless pacer waits on the core runtime's shared
`SimulationCompletionSources`, which observes readiness without consuming a
receipt. Worker leases retire those registrations; spurious readiness cannot
retire a live worker. The common application orders Modelica response admission
before `FmiCompletionSet`, independently of which worker completes first.

A native `Fatal` status invalidates all instances sharing the model's token
and implementation identity for the host's process lifetime. FMI forbids every
later call, including `FreeInstance`; the worker enforces this across both
stepping and cleanup. A requested normal termination calls `Terminate` before
retiring the instance.

Inputs are sampled at the start of each communication interval and held over
that interval, using the existing `SimComponent` port backend and propagation
path. This is explicit Co-Simulation. In FMI Step Mode, setting inputs and
reading outputs does not implement a same-time algebraic solve. The adapter
retains ModelStructure output dependencies, including the distinction between
omitted and empty dependency lists. `fmu_can_restore_state` reports the FMU's
declared capability; it does not claim that an implicit master iteration has
been implemented. A coupled implicit algorithm requires state restoration and
repeated `doStep` transactions at the owning master boundary.

Archive and unpacked-byte limits, instance capacity, bounded queues and
event-iteration limits are owned by `FmuLimits`. Admission uses the shared
asynchronous work budget. Source bytes receive an immutable archive revision.
`SceneOwned` entities own cancellation leases; generational entity identities
fence late completions. Retired instances are freed on the worker before their
extracted resources. A disconnected worker fails outstanding transactions and
releases their progress holds.
Command-created participants use `Provenance::Authoritative` and the existing
session identity admission; their public handles and ports become discoverable
through the canonical entity registry. The command acknowledgment admits work
and does not expose a raw ECS entity as a public identity.

Run `python3 scripts/api/test_fmi_shared_clock.py --port <free-port>` against
the already-built production binary. Add `--windowed` for the same authored
gate in a visible host. The driver downloads the official Reference FMUs
0.0.39 archive, verifies its fixed SHA-256, and supplies exact external fixture
paths to `assets/scenarios/tests/fmi_shared_clock.rhai`. It checks native
continuous state, impact events, mixed communication intervals, shared physics
ticks, global pause and local pause/resume. External binaries stay in `target/`.
The Linux production gate passes 43 checks in each host, including inherited length/velocity
metadata through `ReadPorts`.
`--negative` selects an incompatible fixed internal period;
`--invalid-parameter` supplies an out-of-range native parameter. Each passes
three checks for the owner diagnostic and released preparation hold. Pure metadata lowering
checks declared millisecond time conversion and non-time rejection; the native
Reference FMUs use seconds. This focused profile does not qualify automatic
wire unit conversion, broad FMI conformance or implicit coupling. Those contracts remain required before claiming general physical
interoperability.
