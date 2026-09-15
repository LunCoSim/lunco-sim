# Mission and engineering quality gates

Use this reference when a Twin represents a mission, flight system, surface
system, payload, habitat, or operations product. It is a tailored LunCoSim
workflow informed by NASA and ECSS practice; it is not a claim that a Twin is
flight certified. Tailor the depth of each gate to the model's purpose and
fidelity, but do not omit ownership, provenance, interfaces, verification, or
configuration evidence.

## 1. Start with mission intent and operations

Write a small Concept of Operations (ConOps) before detailed geometry or
controller work. State the mission objective, system boundary, actors and
external systems, mission phases, operating modes, command/telemetry path,
time and resource constraints, and the nominal and off-nominal scenarios that
the model must represent. Include safe, degraded, recovery, maintenance, and
disposal behaviour when it is in scope. A visual-only model still needs a
declared intended use: presentation, design study, dynamics analysis, or
operations rehearsal.

Use a lightweight, evidence-based review sequence rather than ceremonial
names. The useful checkpoints are:

| Checkpoint | Entry evidence | Exit evidence |
|---|---|---|
| Mission concept | objective, boundary, ConOps scenarios | agreed use and success measures |
| Requirements baseline | source-backed, measurable requirements | every requirement has owner, status, and verification method |
| Architecture/interface baseline | product tree, datums, ports, sockets, modes | interfaces are named, typed, dimensioned, and allocated |
| Build-to baseline | component contracts and approved assumptions | detached assets can be verified in isolation |
| Integration/test readiness | verified components, fixture, procedure, deterministic configuration | assembly and scenario tests have explicit entry/exit criteria |
| Operational readiness | nominal and contingency rehearsals, command/telemetry checks | known discrepancies are closed, waived, or visibly open |

Do not advance a checkpoint with an unresolved TBD hidden in a default. Record
the owner, impact, disposition date, or a failing gate.

## 2. Make requirements and evidence traceable

Keep one requirement in one Twin-owned SysML source. For each requirement record
the stable ID and qualified name, subject, parent goal, source URI/title and
access date, extracted fact, rationale, confidence (`reference`, `derived`, or
`study-assumption`), units and frame, operating condition, acceptance criterion,
verification method, and status. Derived values include the equation and any
margin or uncertainty. A design choice belongs in the requirement only when it
is itself mandatory.

The Rhai observer loads the SysML source and reads the composed USD/Modelica
evidence; it must not copy thresholds or silently fill missing facts. Keep
verification (does the product conform to the requirement?) separate from
validation (does the verified product serve the intended ConOps in a realistic
scenario?). A simulation result supports a claim only at the fidelity and
conditions it actually exercised.

## 3. Control interfaces before detail

Treat every physical, data, and operational interface as an explicit contract.
Record both endpoints, owner, direction, type, units, coordinate frame,
positive/sign convention, reference datum, update rate/latency, initialization,
invalid-data behaviour, limits, and the verification evidence. Author named
USD sockets, frames, ports, relationships, and native connections; use
Modelica connectors for continuous domains and Rhai commands/events for
scenario policy. The assembly owns placement and cross-component wiring; the
detached component owns its local geometry, mass/collision envelope, and named
attachment frames. An interface integration test must fail on a missing,
ambiguous, wrong-type, wrong-unit, or wrong-frame endpoint.

## 4. Choose model fidelity deliberately

Declare the purpose and fidelity of every model layer before implementing it:

- presentation geometry proves silhouette, proportions, materials, and camera
  readability;
- mechanical geometry proves datums, envelopes, mass properties, collision,
  joints, limits, and clearances;
- Modelica proves continuous electrical, thermal, propulsion, structural, or
  control equations;
- the operations model proves modes, sequencing, commandability, telemetry,
  timing, resources, and contingency response.

For each simplification, record what is omitted, why it is acceptable for the
declared use, and the error or uncertainty it introduces. Do not use a detailed
mesh to imply validated mass/inertia, and do not use a visual proxy as a
substitute for collision or joint evidence. Add fidelity only when it changes
a requirement, interface, physical envelope, or decision.

## 5. Engineer faults and degraded operations

For every mission-critical function, identify the failure stimulus, effect,
detection/observability, isolation or diagnosis, mitigation, recovery or safe
state, timing deadline, and mission impact. Exercise representative faults in
Rhai fixtures: missing references, disconnected ports, sensor-invalid data,
joint-limit violations, loss of a repeated element, power/thermal margins, and
controller or communication outages as applicable. A fault must produce a
bounded, structured failure or an explicitly specified safe/degraded mode;
never hide it behind a guessed value, retry loop, or silent fallback.

Keep a small hazard/risk record for safety-critical or mission-critical
functions: hazard, initiating cause, severity/likelihood rationale, detection,
control, residual risk, and the requirement or test that closes it. Carry
uncertainty and engineering margins into the requirement and report when a
margin is consumed; do not treat an unquantified margin as evidence of safety.

## 6. Verify progressively and replay deterministically

Use the smallest test that proves each claim, then compose upward:

1. parse, resolve, schema, namespace, and source-set lint;
2. detached component geometry, dimensions, frames, physics, and visual gate;
3. subassembly interface, symmetry, clearances, joints, and references;
4. integrated multi-domain behaviour and conservation/limit checks;
5. ConOps validation with nominal, off-nominal, and recovery scenarios;
6. regression and deterministic replay from the same authored baseline.

Every test has an explicit initial state, stimulus, sampling window, pass
criterion, verdict channel, configuration/source revision, clock policy,
solver settings, thread count, random seed, and evidence path. Capture both
positive and negative cases. Compare replay traces (not just the final frame)
when checking determinism. Keep Editor screenshots tied to the same document,
generation, camera, and configuration as the typed readback; images are
supporting evidence, never the sole proof.

## 7. Baseline changes and operations data

Treat the Twin manifest, SysML source set, USD references, Modelica sources,
Rhai tools/tests, and runtime configuration as one configuration item. Record
the exact source revision and dependencies for each evidence run. A changed
interface, parameter, frame, solver setting, or source invalidates the affected
evidence and requires an impact review; do not overwrite a baseline in place.
Keep command definitions, telemetry/health fields, modes, procedures, and
scenario timelines close to the owning Twin so an operations rehearsal uses
the same identities as the model. Use the repository's typed Editor and
runtime journal for USD changes, and commit only after the relevant gates pass.

## Sources and tailoring notes

These are the primary references used to shape the gates above (accessed
2026-09-15):

- [NASA NPR 7123.1D — Systems Engineering Processes and Requirements](https://nodis3.gsfc.nasa.gov/displayDir.cfm?c=7123&s=1D&t=NPR): a systematic, quantifiable, repeatable lifecycle with requirements, interface, risk, configuration, verification, validation, and transition processes.
- [NASA Systems Engineering Handbook](https://www.nasa.gov/reference/system-engineering-handbook/): requirements quality, product-tree decomposition, verification versus validation, technical reviews, and operational transition.
- [NASA stakeholder expectations and ConOps guidance](https://www.nasa.gov/reference/4-1-stakeholder-expectations-definition/): mission phases, nominal/off-nominal scenarios, human/system allocation, fault response, and command/data concepts.
- [NASA Fault Management Handbook (NASA-HDBK-1002)](https://www.nasa.gov/wp-content/uploads/2015/04/636372main_NASA-HDBK-1002_Draft.pdf): detect, isolate, diagnose, respond, and test fault-management behaviour.
- [ECSS-E-ST-10-24C Rev.1 — Interface management](https://ecss.nl/standard/ecss-e-st-10-24c-rev-1-interface-management-15-november-2024/): identify, specify, approve, control, implement, verify, and validate interfaces through the product tree.
- [ECSS-E-ST-10-02C Rev.1 — Verification](https://ecss.nl/standard/ecss-e-st-10-02c-rev-1-verification-1-february-2018/) and [ECSS-E-ST-10-03C Rev.1 — Testing](https://ecss.nl/standard/ecss-e-st-10-03c-rev-1-testing-31-may-2022/): tailored verification methods, test planning, and evidence.
- [ECSS-E-ST-70C — Ground systems and operations](https://ecss.nl/standard/ecss-e-st-70c-ground-systems-and-operations/): mission-operations engineering, preparation, execution, evaluation, and post-operational work.
- [ECSS-E-ST-70-11C Rev.1 — Space-segment operability](https://ecss.nl/standard/ecss-e-st-70-11c-rev-1-space-segment-operability-15-october-2025/): commandability and predefined contingency operation for unmanned space segments.
