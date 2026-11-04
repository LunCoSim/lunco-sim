# Simulation core audit: second pass

> Status: Active · Audience: simulation, physics, Modelica, API and networking maintainers

Reviewed commit `a3d425c4fcad0036522186efb461e603d91fa83b` on `tutorials`.
The preceding fix and its dependent Twin wire changes were committed first:
LunCoSim `a3d425c4f`, lunar-base-model `9209431`, Summer Space School `702f777`.
This pass changes no implementation or authored scene files. The findings below
remain unfixed.

## Scope and evidence

Three independent source reviews covered physics/USD, Modelica lifecycle, and
API/control/networking. The coordinator checked their claims against current
owners, callers, maintained dependencies, relevant existing review reports and
the preceding audit's receipts. Existing broad determinism limitations are not
counted as new defects; concrete failing paths are listed separately.

The earlier executable and `target/` directory were absent. Fresh runtime probes
used the production `luncosim` binary rebuilt in this checkout with:

```sh
RUSTC_WRAPPER=sccache cargo build --locked -p lunco-luncosim --bin luncosim \
  --no-default-features --features api-transport,transport-http,sysml -j 4
```

This is the numeric application with HTTP and SysML, without the windowed or
networking features. Source/PID/executable/cwd ownership was checked. Sessions
used ports **4893** and **4895**. All ports are released; healthy sessions exited
through API `Exit`, and the intentional crash reproduction exited **101**.
Edits were applied through typed document commands to unsaved documents; input
scene files remained unchanged. No dependency source was edited.

Evidence directory: `.cache/codex-evidence/simulation-core-review-20261104/`.
It contains source inspections, typed operation receipts, native physics
snapshots, command responses, build output and the crash backtrace. No new Cargo
test suite, graphical interaction, macOS run or multiplayer run was performed.
The preceding 76 focused Rust tests and 12 production gates describe the
preceding fix; they do not cover the new triggers below.

## Findings

| ID | Priority | Defect | Evidence |
|---|---|---|---|
| SC11 | P1 | Full-body mass admission can panic or silently discard a malformed body | Runtime |
| SC12 | Live mass-unit changes bypass initial admission validation | Runtime |
| SC13 | Deleting a held input leaves a stale hold that faults the whole exchange | Runtime |
| SC14 | Z-up force actuators use a direction in the wrong frame | Runtime |
| SC15 | Compound collider composition still narrows intermediate translations | Runtime |
| SC16 | Implicit USD density uses 1 instead of 1000 kg/m³ | Runtime and USD contract |
| SC17 | Explicit child-collider mass loses precedence to density | Runtime and USD contract |
| SC18 | Live wheel mass edits do not update the folded carrier | Runtime |
| SC19 | Unknown global entity IDs can address local entity bits | Runtime |
| SC20 | Finite API numbers narrow to infinity and enter authored Modelica | Runtime |
| SC21 | Compile reuse ignores changed sibling sources | Source and caller trace |
| SC22 | Run reuse ignores an explicitly requested class | Source and caller trace |
| SC23 | Invalid Integer/protected parameter edits mutate before checked rejection | Source and compiler contract |
| SC24 | Expression-valued parameter defaults disappear from the editing inventory | Source and data-flow trace |
| SC25 | Network buffering/replay competes with current persistent holds | Source; networking feature |
| SC26 | Named input release is absent from the replicated command protocol | Source; networking feature |

### SC11 — full-body admission does not share the reduced-body safety boundary

Owners: `crates/lunco-usd-avian/src/lib.rs:2182`, `:2281`, `:1433`;
checked native constructor: `crates/lunco-physics/src/body_mass.rs`.

On the existing free unit cube, changing standard `physics:mass` to `1e-45`
returned an applied document receipt and then panicked in Avian's mass-property
observer when constructing computed angular inertia. The process exited 101.
The backtrace identifies `ComputedAngularInertia` construction and
`update_mass_properties` during rigid-body insertion. This is user-authored
data reaching an unchecked native construction boundary, not a Rumoca failure.

The full-body reader accepts positive finite f64 mass before casting to f32;
it does not validate the narrowed value and native reciprocal as the reduced
mass constructor does. A separate negative-mass probe removed the body from
native physics but left readiness true, diagnostics empty and ticks advancing.
The projector logged the malformed mass and marked the prim processed.

Reuse complete native mass/inertia validation before publishing components.
Malformed data must produce a structured owning diagnostic and hold/reject the
affected operation without a process panic or silent body omission.
Evidence: `tiny-mass-crash.json`, `tiny-mass-backtrace.log`,
`full-body-negative-mass.json`.

### SC12 — live convention changes can remove the scene while time keeps running

Owners: `crates/lunco-usd-bevy-runtime-core/src/twin_projection.rs:4050`;
`crates/lunco-usd-bevy/src/lib.rs:680`.

`SetStageMassUnits {edit_target:"@runtime@", kilograms_per_unit:0.001}` on a
settled document returned an applied receipt. The live rebuild installed a
canonical stage without passing the initial prepared-plan convention gate.
Projection then refused its unsupported metadata. `/World/Target` disappeared,
but `GetReadiness` reported ready with no fault, `RuntimeDiagnostics` had zero
findings, and the tick advanced from 14 to 77. Errors appeared only in logs.

The initial-loader SC8 fix is effective; this is a distinct live rebuild path.
The stage owner must apply the same convention admission before replacing live
state, and retain a structured terminal failure/hold for an unsupported active
stage. Evidence: `live-mass-units.json` and its process log.

### SC13 — endpoint retirement does not retire its holds

Owners: `crates/lunco-cosim/src/systems/propagate.rs:238`;
`crates/lunco-usd-bevy-runtime-core/src/scene.rs:928`.

After admitting a persistent force on `/World/ForceBody`, removing that prim
through `ApplyUsdOps` left its `PortHolds` entry alive. The next wiring rebuild
could not find a stable identity for the dead entity and raised
`cosim-held-input-identity-invalid`, stopping the entire exchange. Full scene
teardown clears holds, but individual deletion and live stage replacement do
not pass through that teardown boundary.

The co-simulation lifetime owner must retire or explicitly remap endpoint
holds when their actual entity lease ends. Keep strict identity validation for
live endpoints; do not turn this into a silent invalid-ID fallback.
Evidence: `held-input-delete.json`.

### SC14 — actuator direction and mount rotation use different frames

Owners: `crates/lunco-usd-actuation/src/lib.rs:59` and `:76`;
schema: `crates/lunco-usd-authoring/schema/schema.usda:1651`.

The reader rotates a raw USD direction by an already canonical body-relative
rotation. It omits the stage convention's direction conversion. In the existing
Z-up fixture, authored direction `(0,1,0)` became native `(0,1,0)`; it should
be `(0,0,-1)`. Both force application and geometry-derived allocation consume
this helper, so the force and moment allocation share the error.

Convert the prim-frame direction at the existing stage-convention boundary
before applying the canonical mount rotation. Evidence: `force-axis-z.json`.

### SC15 — compound colliders bypass the corrected physical mount path

Owner: `crates/lunco-usd-avian-reader/src/collider.rs:584`.

The recursive collider collector obtains f32 `Transform`s for each hierarchy
segment and multiplies them before native collider publication. A group at
X=100000000 with a child cube at X=-99999999 has body-relative X=1 in authored
double precision. The observed collider translation and computed COM were both
X=0. The final one-metre offset is representable; it was lost prematurely.

Compose the body-relative stack in f64 and perform one validated conversion at
the native boundary. Reuse the shared transform owner rather than maintaining
a second lower-precision geometry walk. Evidence: `compound-transform.json`.

### SC16–SC17 — standard implicit mass and child precedence remain incomplete

Owners: `crates/lunco-usd-avian/src/lib.rs:2108`, `:2142`, `:1465`.

**SC16:** removing the mass opinion from a one-metre cube produced **1 kg**.
The USD density reader leaves an omitted density to Avian's default of
1 kg/m³. USD specifies **1000 kg/m³** when neither hierarchy nor material
provides density. Resolve that standard default at the USD owner before native
publication. Evidence: `implicit-density.json`.

**SC17:** a rigid body with no root mass and one unit-cube child with explicit
mass **7 kg** and density **100 kg/m³** produced **100 kg**, rather than 7 kg.
The child collider path transfers geometry, material and density but ignores
its explicit MassAPI properties. This trigger is independent of SC16.
Complete per-source mass properties must participate in the maintained native
reduction, while preserving explicit root-mass precedence.
Evidence: `child-mass-precedence.json`.

The normative default and precedence are specified by
[OpenUSD body mass properties](https://openusd.org/dev/api/usd_physics_page_front.html#body-mass-properties)
and [MassAPI](https://openusd.org/dev/api/class_usd_physics_mass_a_p_i.html).

### SC18 — in-place wheel resync leaves the carrier at its old mass

Owners: `crates/lunco-usd-sim/src/wheel_runtime.rs:98`, `:290`;
`crates/lunco-mobility/src/lib.rs:788`.

Changing one raycast wheel from **25 to 35 kg** updated `WheelRaycast.mass`
but left its carrier at **1120 kg** instead of 1130 kg. The edit is claimed
by the wheel's in-place resync owner, bypassing generic subtree reprojection.
The carrier remains excluded by its permanent `RaycastWheelMassFolded` marker.
Its COM and tensor have the same stale ownership problem.

Maintain an independent baseline and recompute the complete reduced candidate
when source contributions change. Merely clearing the marker would risk adding
new contributions to an already folded baseline. This finding is deliberately
limited to in-place wheel edits: an ordinary reduced-child mass edit rebuilt
the carrier correctly in the separate control probe.
Evidence: `live-wheel-mass.json` and `live-reduced-mass.json`.

### SC19 — unresolved global identity is accepted as native identity

Owner: `crates/lunco-api/src/executor.rs:699`.

Failed global-ID resolution leaves the numeric wire value unchanged. The
concrete Bevy `Entity` deserializer then accepts it as local entity bits.
The probe obtained raw bits **4294966004** from USD topology inspection,
verified those bits were absent from the global entity list, and submitted a
`ReleasePort` targeting that number. The command admitted a release for actual
global ID **405168063241357**. Its nonexistent port name made the probe
non-destructive. A caller can reach the same wrong entity through destructive
or pose-changing commands.

Reject unresolved global IDs at the authoritative adapter; preserve only
explicitly local placeholder contracts. Symmetric globalization must not emit
unregistered raw entity bits as if they were global IDs. This is an identity
integrity finding, not a claim that the trusted local API enforces RBAC.
Evidence: `unknown-gid.json`.

### SC20 — valid f64 wire values can become invalid f32 authored values

Owner: `crates/lunco-api-core/src/value.rs:415`; consumers include
`crates/lunco-modelica-api/src/edit/component.rs:12` and `edit/mod.rs:334`.

Both finite-value validation and command decoding admit `1e300` before it
narrows to f32 infinity. `AddModelicaComponent` returned success and authored
`extent={{inf,-10},{inf,10}}`. An internally tagged `ApiGraphic::Text` request
also committed `fontSize=inf`. Both advanced their document generation.

Reject out-of-range narrowing through the typed admission boundary before
any command mutates state. Cover ordinary f32 fields and internally tagged
serde payloads; the latter may deserialize through serde's buffered content
rather than the direct f32 visitor. Evidence: `float-overflow.json`.

### SC21–SC24 — Modelica source, selection and scalar admission gaps

These four findings are source-proven and were not exercised through a
windowed host in this pass.

**SC21 — source closure freshness.**
`crates/lunco-modelica-worker/src/worker/compile.rs:275` acknowledges a cached
compile using only the primary document's generation, before gathering siblings
at line434. Compile Main using Helper=4, edit the separate Helper document to 8,
then compile unchanged Main without `force`: the old session is reused.
`CompileStatus` at `lunco-modelica-api/src/lib.rs:613` likewise labels it fresh.
Reuse and stale reporting must validate participating source identities and
leases, not just the primary document. Retaining admitted sibling bytes during
parameter reconstruction does not establish that those bytes are still current.

**SC22 — requested class ignored by Run.**
`crates/lunco-modelica-ui/src/ui/commands/compile.rs:928` unpauses an existing
fresh participant without comparing the class captured at line896. Compile
`P.First`, then run the same unchanged document explicitly requesting `P.Second`:
the fast path runs First. Resolve and compare selected class identity before
reusing a session, as the compilation path already does.

**SC23 — predictable scalar rejection occurs after mutation.**
`crates/lunco-modelica-core/src/parameter_commands.rs:139` admits a checked
`parameter Integer n=2` with requested value 1.5. It edits source at line239
and changes session/parameters at line277 before the compiler rejects that
fractional Integer. Protected declarations have an analogous gap: core checks
direct `is_final`, while the compiler requires public, non-final numeric
declarations (`lunco-modelica-compiler/src/interface.rs:27`). Consume that
existing checked declaration/scalar admission before source or session mutation.

**SC24 — expression defaults disappear from parameter inventory.**
`crates/lunco-modelica-worker/src/worker/compile.rs:418` seeds values using
`binding.parse::<f64>()`; `bridge.rs:1529` only retains those entries and adds
explicit overrides. A valid `parameter Real gain=2*3` is therefore omitted
unless an override already supplies it. The inspector has no row, and the
typed edit falsely fails core's map-membership check. Seed values from checked
parameter roles and initialized observations, rather than literal source syntax.

### SC25–SC26 — opt-in networking does not share the effective input contract

These findings require the **networking feature**, absent from default desktop
builds and this pass's runtime executable. No multiplayer reproduction was run.

**SC25 — buffered/replayed frames are overwritten by current holds.**
`crates/lunco-networking-core/src/prediction.rs:1080` writes buffered frames
directly to ports. Remote command arrival meanwhile triggers `SetPorts` via
`crates/lunco-networking-sync/src/sync.rs:1117`, installing the latest hold.
If frames `(seq1, throttle=1)` and `(seq2, throttle=0)` arrive together, the
fixed consumer selects seq1, but propagation at `cosim/src/systems/propagate.rs:831`
reapplies the current hold of zero. The acknowledgement can describe seq1 while
physics integrates seq2's value. Historical replay repeats the bypass at
`prediction.rs:1363`; rollback additionally requires `LUNCO_ROLLBACK=1`.
Route admitted frames through the hold-aware atomic input owner and prevent
arrival-time writes from competing with fixed-frame admission.

**SC26 — named release is not replicated.**
`crates/lunco-networking/src/shared.rs:230` declares `SetPorts`,
`SetPortsBatch` and `ReleaseControl`, but no `ReleasePort` channel.
`networking-sync/src/sync.rs:949` also omits it from control ordering, and the
host rejects undeclared command names. A client can establish a host-held force
level, release its local hold and leave the host level active. Add the existing
typed command to the reliable protocol and control ordering; verify host/client
release and replay together with SC25.

## Confirmed boundaries and unpromoted leads

- Initial reduced mass uses full tensors, COM and principal frames; shared
  physical mounts retain f64 subtraction; initial mass-unit admission rejects
  unsupported units. These preceding fixes remain present.
- Generic reduced-child mass **1→2 kg** correctly rebuilt carrier **6→7 kg**.
  The broader claim that all live contribution edits were stale was withdrawn.
- Parameter replacement retains admitted siblings/overrides/inputs in native
  and wasm paths; its completion no longer waits for later steps to become idle.
- Immediate control/program priority, atomic preflight and persistent unwired
  levels share the corrected owner; strict command and tagged-payload fields
  remain installed.
- Reset after an uncompiled draft correctly reports stale source. More complex
  failed-update/reset sequences remain a lead, not a proven corruption finding.
- Recompiling a document into a different class leaves the actor's old `Name`;
  camera input settings can partially apply a mixed valid/invalid request.
  These lower-priority presentation/control leads were not runtime-tested.

## Remediation order

1. **Admission and retirement:** SC11–SC13, SC19–SC20. Reuse checked native
   construction, one stage admission gate, strict identity/numeric conversion
   and endpoint lifetime cleanup. Add negative authored production verdicts
   proving terminal diagnostics, atomic rejection and non-crashing behavior.
2. **Physical interpretation:** SC14–SC18. Complete frame/precision conversion
   and standard per-source mass ownership, then recompute reduced candidates
   from independent baselines. Cover Y/Z axes, hierarchy cancellation, density,
   child precedence and live wheel edits through native observations.
3. **Modelica contracts:** SC21–SC24. Share source-closure freshness, selected
   class identity and checked scalar/declaration facts across compile/run/edit.
   Cover sibling edits, class switches, nonliteral defaults and invalid typed
   values in windowed production command tests, with native/wasm parity.
4. **Network input authority:** SC25–SC26. Reconcile buffering, persistent holds
   and replay through one admitted input path; cover two queued frames and
   named release in a networking-enabled host/client gate.

Each repair belongs at its authoritative owner and must remove the competing
path. No compatibility decoder, guessed identity, fabricated physical default,
or silent fallback should hide an invalid contract. Third-party modifications
would require the separate explicit authorization in `AGENTS.md`.
