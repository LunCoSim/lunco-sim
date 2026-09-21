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

### Physical materials and engineering properties

For every component whose structural, pressure, thermal, electrical, mass, or
contact behaviour depends on material, record a typed material selection and
only the properties needed by the declared analysis. A reusable material record
should distinguish alloy/product form/temper or composite layup and direction
(for example AA6061-T6 and AA7075-T73 are different catalogue records, not one
generic "aluminum" entry);
include units, temperature/environment range, source/revision, applicability,
and whether each value is reference data, a derived value, or a study
assumption. Suitable properties may include density, elastic and shear
moduli, Poisson ratio, yield/ultimate allowables, thermal expansion,
conductivity, heat capacity, electrical resistivity, and optical surface
properties. Composite overwraps require directional properties and layup or
winding information; an isotropic generic "carbon fiber" value is not enough
for structural sizing.

Use one reusable typed material catalogue and typed component references to
it. The intended engineering source is one SysML material package in the
indexed source set; a component refers to the material definition by a typed
SysML usage, not by a string name. Do not repeat a material's numeric
properties in each component or maintain a second hand-edited table in
Modelica. Modelica and other physics consumers should receive the same source
values through a typed adapter, with units and applicability preserved until
the domain boundary. Keep visual appearance in `UsdShade`, contact friction and
restitution in `UsdPhysicsMaterialAPI`, and engineering constitutive properties
in the material record; one must not be inferred from another. USD mass,
collision, and model equations remain explicit owners of their respective
runtime facts. If a shared cross-Twin SysML library or typed resolver is not
available, keep one catalog source in the Twin and record the library/binding
gap; do not invent an external-path resolver.

Treat a material definition, its assignment to a component role, and the
component's geometry/model parameters as separate typed facts. A catalogue
entry should capture canonical grade and product form/temper (or composite
layup and orientation), and give every property its unit, applicable
temperature/environment, source identifier/revision, and status as sourced,
derived, or assumed. Cite properties at the entry/property level; a general
material citation is not evidence that a specific strength allowable applies
to this product form, direction, process, temperature, or fluid exposure. Keep
unselected candidates distinct from the selected material and never treat an
assumption as a qualified design allowable. Modelica generation should map
only the properties required by its constitutive equations from this source;
unsupported property types or missing unit-preserving projections are explicit
tool gaps, not invitations to copy constants into `.mo` files.

For pressure vessels, model the external envelope separately from the actual
pressure boundary. A COPV contract identifies liner and overwrap materials,
MEOP and operating conditions, geometry, wall/layup definition, interfaces, and
the qualification/design basis. Standards govern selection, analysis,
qualification, and verification—not one universal alloy, operating pressure,
or wall thickness. Source assumptions from an applicable spacecraft materials
standard, pressure-vessel standard, material allowables, and compatible
propellant/environment data; mark a study analog as such. Do not assign a
flight material, proof/burst pressure, or liner/overwrap thickness when those
inputs and qualified analysis are absent.

Current LunCoSim material handling is not yet a cross-domain engineering
catalogue: `lunco-materials` is render/shader appearance intent, while
`UsdPhysicsMaterialAPI` supplies contact behaviour; the component authoring
contract explicitly notes that `physics:density` is not consumed. SysML can
project typed literals and quantities, but a generic material-record resolver
and shared SysML-to-Modelica/USD/physics binding are not established. Confirm
the current API before use; if that binding is missing, record it as one generic
feature gap rather than duplicating material data per subsystem.

Treat a surface finish or coating as a separate typed selection layered on a
substrate, not as a replacement material identity. One substrate may have
different finishes on distinct exposed faces or regions, and one finish may
be applied over different substrates. Record coating stack/order, thickness
or areal mass, process, and environment limits when the design or analysis
depends on them. Keep the finish's measured optical/thermal properties and
their sources in engineering data. A renderer mapping may select a LunCo
shader family and appearance parameters for that typed finish, but shader
color, metallic, and roughness values do not establish solar absorptance,
infrared emittance, coating thickness, or thermal performance. The typed
finish-to-`UsdShade` mapping is a generic tooling gap until a supported
resolver and scene projection are present.

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
- [NASA-STD-6016C w/Change 1](https://standards.nasa.gov/node/259): spacecraft materials and processes control; it is a material-selection/qualification basis, not a universal numeric property table.
- [NASA MAPTIS](https://maptis.nasa.gov/): NASA's materials properties information system; use a source record with material condition and environmental applicability, where access permits.
- [ANSI/AIAA S-081B-2018 COPV summary](https://www.nasa.gov/centers-and-facilities/white-sands/space-systems-composite-overwrapped-pressure-vessels-ansi-aiaa-s-081b-2018/): design, analysis, fabrication, test, inspection, operation, and maintenance of metal-lined carbon-fiber/polymer COPVs.
- [ECSS-E-ST-10-24C Rev.1 — Interface management](https://ecss.nl/standard/ecss-e-st-10-24c-rev-1-interface-management-15-november-2024/): identify, specify, approve, control, implement, verify, and validate interfaces through the product tree.
- [ECSS-E-ST-10-02C Rev.1 — Verification](https://ecss.nl/standard/ecss-e-st-10-02c-rev-1-verification-1-february-2018/) and [ECSS-E-ST-10-03C Rev.1 — Testing](https://ecss.nl/standard/ecss-e-st-10-03c-rev-1-testing-31-may-2022/): tailored verification methods, test planning, and evidence.
- [ECSS-E-ST-70C — Ground systems and operations](https://ecss.nl/standard/ecss-e-st-70c-ground-systems-and-operations/): mission-operations engineering, preparation, execution, evaluation, and post-operational work.
- [ECSS-E-ST-70-11C Rev.1 — Space-segment operability](https://ecss.nl/standard/ecss-e-st-70-11c-rev-1-space-segment-operability-15-october-2025/): commandability and predefined contingency operation for unmanned space segments.
