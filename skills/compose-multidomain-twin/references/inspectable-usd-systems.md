# Usable USD system models

Read this when authoring or restructuring a multi-domain model that people need
to inspect, simulate, and maintain. Use the existing component-authoring cycle;
this reference adds the system-structure and diagram acceptance criteria.

## Choose boundaries people can inspect

Describe the intended operating modes and required domains before choosing
files. For each subsystem, identify its physical parts, continuous equations,
external ports, installing assembly, controls, and verification case. Distinguish
modeled behavior, render geometry, study assumptions, and missing source data.
A visible tank, cable, ramp, or radiator does not establish its simulation model.

Use a reusable USD assembly when its internal physical connections need to be
visible in the system graph. Give valves, reservoirs, chambers, loads, and other
independently inspectable parts explicit prim identities and program facets.
Reference the reusable Modelica classes for their equations and state. Keep an
opaque Modelica composite when its internals are intentionally outside the
required inspection boundary; do not split every equation into a USD prim.

For example, a reusable bipropellant engine can contain two valve program prims
and one chamber program prim. Its USD owns valve-to-chamber connections; the
installing vehicle owns tank feeds, command/health bindings, parameters, and
actuator placement. All mutually coupled parts join the enclosing propulsion
collection. A reusable assembly does not need a separate solver: file boundaries,
prim hierarchy, and numerical coupling are different decisions.

Keep one owner for each physical state and conserved quantity. Render-only tank
stations do not create additional reservoir mass. A joint or actuator uses the
native physics boundary; Modelica does not duplicate rigid-body integration.
Rhai sequences operations and applies explicit native commands where required.

## Author and verify the installed graph

Use the existing document owner and typed USD operations. Before connecting a
new reference, install the reference and wait for the exact document generation
to compose. Inspect its inherited ports, types, and schemas, then apply external
wiring. Author numeric values as `double` where the owning schema permits;
retain their requirement provenance and units. Pass raw strings to typed string
attribute operations, rather than embedding USDA quote characters. Read values
back to catch literal quotes and unintended numeric conversions.

Use `inputs:`, `outputs:`, `connectors:`, standard relationships, and the existing
registered schemas. Hierarchy does not imply a connection or attachment. Do not
infer a wire from proximity, similar names, geometry, or a desired diagram.
Do not add a custom field when the standard owner already expresses the fact.

Check every required endpoint against the composed source, including referenced
children. Validate complete source-to-consumer paths and membership in the one
owning network before returning a successful assembly plan. Report a missing
part, port, source datum, or unsupported operation at that owner. Do not build a
plausible diagram around an incomplete model or silently invent defaults.

## Give the model readable identities

Keep canonical prim paths and Modelica identifiers stable. Use `ui:displayName`
for concise human names and short local port captions in presentation. Repeated
parts need their parent or station in the caption, such as `Engine03 fuel valve`.
Keep exact paths, full port names, units, and peers available in inspection.
Truncating or renaming persisted identifiers to fit labels is not a UI fix.

Classify physical connectors from declared Modelica type facts, with their
source-resolution limits explicit. Do not infer a domain from an instance name.
An acausal connection is reversible; an authored display role does not prescribe
its runtime flow direction. Keep causal commands and telemetry distinguishable
from physical connector networks and native joints.

## Deliver views alongside the assembly

Use **Systems** / Connections for the composed graph across USD, Modelica,
physics, and scripts. Use generated Modelica diagrams for the equation networks.
Neither alone proves that the other domains are connected.

For a large model, save useful `.lunco-view.toml` artifacts through the typed
view commands. A practical set is a subsystem overview, the full expanded graph,
and focused views for dense circuits or repeated assemblies. Choose the set for
the actual model; four particular view files or one vehicle's groups are not a
universal template. Save stable source URIs, independent scopes, readable group
labels, and deliberate placement. Include simple instructions for reopening.

Group by engineering responsibility. Keep a model and its driven visual helpers
together when separating them would flood the overview with render bindings.
Use geometry counts honestly: group prim counts are not model-component counts.
Keep unassigned new prims visible. A collapsed overview must preserve original
edge and endpoint identities; expanding a group must expose the actual members.
Dense boundary captions may require expansion or inspection, but must remain
recoverable. A focused view must state its scope: an engine's internal view does
not show external tank feeds unless those endpoints are within that view.

Use the existing renderer geometry and theme. Prevent overlapping captions,
clipped titles, excessive empty margins, and feedback routes outside Fit. Review
the saved result at normal window size, including one expanded repeated part.
Do not call a diagram usable merely because the source parses or every link fits
in a screenshot.

Scripted force application and mission actions are runtime relationships. Show
and document their owning controller or bridge; do not fabricate static USD
connections for them. In the live Connections scene browser, enable **Scenario
access** to inspect separately styled declared reads and writes. Expand a group
to see its controller and target identities. The overlay and `ScriptInspect`'s
`access_plan` come from the scenario's admitted dependency plan; they describe
authorization, not observed port transfers or a validated mechanical coupling.
Use native state/force telemetry and an authored behavioral gate for that claim.
A document preview has no live authorization overlay. Collapsed-group access
links aggregate by displayed pair and direction; their counts are separate from
static connection counts. A static exit ramp does not imply hydraulic actuation.
List genuinely missing domains separately from hidden or unexpanded detail.

Keep constitutive-law calibration separate from measured state. If a declared
study limits a coefficient's input range, retain the actual state and expose
whether limiting occurred; an assumed cold reference hold does not calibrate
colder hardware. Reject unsupported hot operation at the model owner. Thermal
acceptance permits heating or cooling while independently checking conservation.
For switched loads, qualify battery-voltage rebound as well as a prescribed
voltage trace; event-driven hysteresis must retain its admitted mode.

## Acceptance evidence

Use an owned production session and the existing Rhai/API test facilities.

| Claim | Evidence |
| --- | --- |
| The installed assembly is complete | Composed references, program sources, required ports, network membership, and consumer bindings |
| The graph is complete | Exact edge identities before/after grouping; zero unresolved links; every required physical path checked |
| The views are reusable | Reopen every delivered file against its stable source URI; expand, trace a real port, and inspect a repeated component |
| The simulation works | Ready with no hold/fault, clean connection diagnostics, nominal behavior and a relevant off-nominal authored verdict |
| A wiring refactor preserves behavior | Compare the same stimulus and outputs before/after; verify conservation and failed/disabled parts |
| The mission still works | A bounded production integration case that exercises the affected subsystem and its real consumers |

Keep assertions in authored Rhai when the public API can observe the contract.
Capture actual exit codes and verdicts. A successful compile, static diagram,
or isolated component test does not establish a complete mission result.
Retain source/revision and test scope with the evidence; distinguish assumptions
and untested operating modes from observed results.
