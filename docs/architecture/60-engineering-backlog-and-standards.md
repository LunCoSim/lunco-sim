# Engineering Backlog & Adopted-Standards Roadmap

**Status:** backlog. This doc is the deliberate exception to "a doc here
describes what IS" — it records what is *not yet built*, why each item matters,
and its rough scope, so the reasoning survives until someone picks it up.
Each entry carries **what**, **why** (the motivating problem), and **scope**.

Security and multiplayer-hardening deferrals are **not** here — they live in
[`../../DEFERRED-2026-07-19.md`](../../DEFERRED-2026-07-19.md) §A, annotated
with `TODO(multiplayer)` at the exact code sites.

Sourced from the 2026-07-19 static review (`REVIEW-2026-07-19.md` §8) and its
deferred-work companion. When an item lands, delete it here and let the
subsystem doc describe the result.

---

## 1. Ephemerides & orbit propagation

### Adopt ANISE

**What:** Replace the hand-cached ephemeris CSV pipeline with
[ANISE](https://github.com/nyx-space/anise) — a pure-Rust SPICE implementation
reading real `.bsp` kernels.

**Why:** Three problems at once. The CSV cache is a bespoke format that must be
regenerated out-of-band, and a missing CSV **fails the wasm build** (the
build-script regen path only works into the workspace-parent cache). ANISE
gives MOON_ME↔MOON_PA frame transforms for free — rasters now declare their
frame (MOON_ME, ~875 m from PA on the surface), but nothing can *convert*
between the two. And `.bsp` is the format every upstream ephemeris source
actually ships.

**Scope:** medium. Swap the ephemeris source behind `lunco-celestial`'s
existing sampling interface; delete the CSV cache + regen build script; verify
wasm (ANISE is no_std-friendly and pure Rust). The IAU/WGCCRE rotation model
in [`43-orbital-view.md`](43-orbital-view.md) is a consumer, not a casualty.

### Kepler solver → lox-space

**What:** Replace the hand-rolled Newton iteration in
`crates/lunco-celestial/src/kepler.rs` with
[lox-space](https://github.com/lox-space/lox)'s vetted propagation.

**Why:** Hand-rolled root-finders for Kepler's equation are a classic source of
silent divergence at high eccentricity. The interim hardening (eccentricity
clamp to `e < 1` plus a residual warning) makes failure *visible*; it does not
make the solver *good*. lox-space is maintained by people whose whole job is
this maths.

**Scope:** small. One solver behind one function; the clamp+warning becomes a
regression test against the library.

## 2. Model exchange — FMI 3.0 FMU import

**What:** An FMU import backend via the [`fmi` crate](https://crates.io/crates/fmi),
registered in `BackendRegistry`.

**Why:** `BackendRegistry` ([`14-simulation-layers.md`](14-simulation-layers.md))
was designed for pluggable backends precisely so rumoca would not be the only
way to get equations into a run. FMI 3.0 is the industry model-exchange
standard (Dymola, OpenModelica, Ansys export it), so an FMU backend both opens
the door to external models and **hedges rumoca regressions** — when the
compiler misbehaves, a reference FMU of the same model isolates whether the
bug is ours.

**Scope:** medium. New backend crate implementing `Backend`/`Participant`
against the co-sim master loop in [`22-domain-cosim.md`](22-domain-cosim.md)
(the macro-step contract already speaks FMI-CS vocabulary); no changes to the
registry itself.

## 3. Robotics interop

### ROS 2 bridge

**What:** A ROS 2 bridge using `zenoh-bridge-ros2dds` (avoids linking DDS
directly).

**Why:** The ontology ([`01-ontology.md`](01-ontology.md)) already declares a
1:1 Port↔topic mapping — the design work is done. ROS 2 is table stakes for
the 2026 space-robotics ecosystem (VIPER, Space ROS, LunarSim all lead with
it); without a bridge, LunCo is a destination, not a participant.

**Scope:** medium. Sidecar process + a thin Port-to-zenoh adapter; no core
changes.

### Newton USD converter load test

**What:** Test that output from Newton's `urdf-usd-converter` /
`mujoco-usd-converter` loads in LunCo.

**Why:** The UsdPhysics migration ([`39-usd-native-migration-plan.md`](39-usd-native-migration-plan.md))
means robot import via USD should be nearly free — but nobody has run the
converters' actual output through the loader. Gaps found become doc-39
Phase 3 items rather than user-facing surprises.

**Scope:** small. Acquire sample outputs, load, catalogue what breaks.

## 4. Wire robustness

### Derive the wire version from the wire-type layout

**What:** Generalize `LUNCO_WIRE_BUILD_ID` — derive the wire version from a
build-time hash of the wire-type layout, instead of a hand-bumped constant.

**Why:** The wire is positional bincode with no handshake beyond the version
constant; the hand-bumped constant has already bitten once (stale web worker
after a rebuild). Two same-version builds with different envelope shapes still
corrupt silently. This is failure pattern #3 from the review: hand-maintained
constants that must agree with something else — derive, don't duplicate.

**Scope:** small–medium. Build-script (or macro) hash over the wire-type
definitions feeding the existing version check; applies to both the worker
transport and the network wire.

### Content key in the terrain worker reply

**What:** Add a content key to the terrain worker reply protocol.

**Why:** Today the worker's Full-swap reply carries no key, so the receiving
side hashes the **whole raster once on the wasm main thread** to identify the
content. On wasm there are no compute threads to hide that in
(`AsyncComputeTaskPool` runs on main), so it is a guaranteed hitch. The worker
already knows what it computed; it should say so.

**Scope:** small. One field on the reply message (wire-version bump — see
above), delete the main-thread hash.

## 5. USD conformance

### AOUSD Core Spec 1.0 compliance suite

**What:** Run the openusd fork against the AOUSD Core Materials/Spec 1.0
compliance suite (shipped Dec 2025).

**Why:** Tier-1 authority — the document of record — rides a **non-reference
USD implementation** (our fork). A conformance suite is the only systematic
answer to "does our composition match Pixar's"; everything today is
spot-checked. Findings are fixed in the fork
(`../openusd`, pull first), never worked around in LunCo.

**Scope:** medium; mostly harness work, then a burn-down of findings.

### Units write-back inversion

**What:** Invert `StageMetrics`/`ConventionTransform` when authoring onto
non-canonical stages.

**Why:** [`41-axes-and-units.md`](41-axes-and-units.md) is "convert once, at
the importer" — reads are correct. But **authoring onto a Z-up/cm stage
currently writes canonical (Y-up/m) values**, corrupting the stage for any
other tool. Round-tripping someone else's stage is exactly the interop story
USD is for.

**Scope:** small–medium. Apply the inverse convention transform in the
authoring path (`ApplyUsdOp` boundary); tests pin a Z-up/cm round trip.

### Schema registry `(schema, property)` keying

**What:** Key the schema registry by `(schema, property)` instead of property
name alone.

**Why:** Two applied schemas can legitimately declare the same property name
with different types/defaults; a flat key makes one silently shadow the other.
Warn-on-conflict shipped as the interim — collisions are now *visible*, but
still wrong.

**Scope:** small. Registry key change + lookup-site sweep in `lunco-usd`.

### `!resetXformStack!` ECS detachment

**What:** Project `!resetXformStack!` as ECS re-parenting (detach from parent
transform), not just composed-value correctness.

**Why:** Composition handles the op correctly — the *values* are right — but
the ECS projection still parents the entity under its prim parent, so anything
that walks the entity hierarchy (frames, follow cameras, physics attachment)
sees a lie.

**Scope:** small–medium in the StageSink projection; the subtlety is the
interaction with `big_space` grid parenting.

## 6. Architecture debt

### Core purity: `DriveInputs`/`DriveMix` and the `SUBSYSTEMS` allowlist

**What:** Move `DriveInputs`/`DriveMix` out of `lunco-core` into the vehicle
domain; replace the hardcoded `SUBSYSTEMS` allowlist with dynamic
registration.

**Why:** The standing rule is that nothing domain-specific enters
`lunco-core` ([`38-domains-as-packages.md`](38-domains-as-packages.md)) —
these two types are vehicle-domain vocabulary that leaked into the substrate,
and every leak makes the next one look normal. The static allowlist is the
same disease in registry form: adding a subsystem should be registration, not
a core edit.

**Scope:** medium; mechanical move + a registration API, but the import sweep
touches many crates.

### Modelica compile-core split

**What:** Split a compile-core crate out of `lunco-modelica` so
`lunco-usd-sim` depends only on what it uses.

**Why:** `lunco-usd-sim → lunco-modelica` drags modelica's full heavy closure
(parol/rumoca and friends) into consumers that never compile a model. Build
time is a tax on every iteration loop.

**Scope:** medium. Crate split along the existing compile/runtime seam;
`cargo tree` before/after is the acceptance test.

### Re-home active-document clearing

**What:** Move the generic active-document clearing out of `lunco-modelica`'s
`CloseDocument` handler into a `lunco-workspace` observer.

**Why:** Clearing "the active document" is workspace policy, not a Modelica
concern; a domain crate owning it means every other domain must remember to
replicate it (fixed-in-one-place-only, review failure pattern #2). **Caveat
that shapes the design:** the current handler also touches the
`editor_buffer`, and that coupling makes a naive split into two independent
observers racy — the ordering between buffer teardown and active-doc clearing
must stay explicit.

**Scope:** small, but only after the ordering question is answered; not a
mechanical move.

### `lunco-scripting` persistence through Storage

**What:** Route timeline and tool-lib persistence in `lunco-scripting`
through the `Storage` handle.

**Why:** Storage is the I/O perimeter ([`40-asset-io.md`](40-asset-io.md));
every `std::fs` bypass is a hole in wasm support and in any future
confinement. Atomicity is already solved (temp+rename is in place) — what
remains is the Storage-API migration, which needs a dependency line from
`lunco-scripting` to the storage crate.

**Scope:** small.

## 7. Dependencies & supply chain

- **pyo3 0.23 (SUP-3)** — carries two unpatched RustSec advisories, and it is
  on the **live Python-cosim path**, not a dev-dependency. Upgrade when the
  cosim backend is next touched; until then this is accepted, known risk.
- **lightyear duplication** — the version + feature lists are duplicated
  across the two target-cfg blocks in `crates/lunco-networking/Cargo.toml`.
  Hoist the version to workspace deps when the file is next touched; skew
  between the two blocks would be a confusing wasm-vs-native failure.
- **cargo-machete pass** — likely-unused dependencies were flagged but not
  verified (see also
  [`../../DEFERRED-2026-07-19.md`](../../DEFERRED-2026-07-19.md) §C on why
  pruning by references alone is not sufficient).

## 8. Testing debt

- **Zero-test crates:** `lunco-worker-transport` (the wire handshake — the
  exact place silent corruption lives, see §4), `lunco-sandbox-server`, and
  the `luncosim` binary itself. The review's meta-finding: the bimodal
  coverage map matches the bug map.
- **`lunco-command-macro` is structurally untestable** — the logic lives
  inline in the `proc_macro` entry points, which cannot be called from unit
  tests. Two exits: a `trybuild` dev-dependency (test through expansion), or
  refactor the logic into `proc_macro2`-typed helper functions the entry
  points delegate to. The refactor is the better end state; trybuild is the
  cheaper first step.

## 9. Known parked bugs

Parked, not forgotten — each has a reason it wasn't fixed in the sweep:

- **`globe_lod.rs:133` "globe not rendering"** — parked as a code comment;
  the orbital-view globe path is not on the current milestone.
- **The two divergent key-state widgets** — two implementations of the same
  keyboard-state display have drifted; unification is merge-conflict bait
  while the theme branch is open.
- **`dem_process_crops_non_square_to_square`** — asserts a `metadata.yaml`
  the pipeline stopped writing; the test is stale, not the pipeline. Delete
  the assertion or the test.

## 10. Measure before optimizing

Do these **before** further performance fixes — every perf claim in the
2026-07-19 review is inference from reading code, not from a profile:

1. **Tracy / `bevy_diagnostic` capture** of the representative scenes
   (moonbase, terrain streaming poses). Turns "this probably hitches" into a
   frame graph.
2. **Per-feature-combination build checks** — the feature matrix
   (`--no-ui`, wasm, server) is only ever built in the combinations CI
   happens to hit.
3. **Two-peer float-determinism harness** — the wire mandates f64 and stable
   ordering; nothing measures whether two peers actually stay bit-identical.

## 11. Watch list

Not adopted; re-evaluate when the trigger fires:

| Standard | Trigger to act |
|---|---|
| SSP 2.0 scenario container | before the RON scenario format ossifies |
| XTCE export of the command registry | first mission-control-facing integration (Yamcs; VIPER precedent) |
| SimReady electrical/thermal schemas | before finalizing `LunCoElectricalAPI` (doc 54) |
| MaterialX / OpenPBR | keep UsdShade authoring compatible; do **not** implement a runtime |
| GPU-driven LOD (CBT/UDLOD) | the wasm main-thread LOD cost, if terrain streaming targets web seriously |
| International Lunar Reference Frame | standardization lands; ANISE (§1) positions us to adopt cheaply |
| lightyear releases | each release; we track 0.27 |

## 12. Validated non-adoptions

Evaluated and **rejected** — recorded so future readers don't re-litigate:

- **Full rollback netcode** — state-sync + visual render-lead matches genre
  norms for this vehicle-sim class; rollback's cost (replicate every predicted
  body) was paid once for wheels and is not worth generalizing.
- **Protobuf wire migration** — until external (non-LunCo) clients exist,
  schema evolution machinery buys nothing over versioned bincode (§4 makes
  the version honest).
- **DCP** — no demand.
- **Codeful USD schemas** — codeless remains the recommended route
  (`luncoSchema` registry works; codeful buys build coupling).
- **bevy_terrain replacing CDLOD** — the physics-coherent height oracle
  ([`terrain-substrate.md`](terrain-substrate.md)) is load-bearing; a
  render-only terrain crate cannot serve physics, spawn, and visuals from one
  source.
- **SMP2** — cite for ESA-facing credibility only; do not implement.
