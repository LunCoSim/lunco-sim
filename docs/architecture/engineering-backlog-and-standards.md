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

**Scope:** small in code, but it lands as a WIRE change and must ship in
lockstep. The reply and `BakeReplyHeader` gain a `base_key`; the worker folds it
per stage; the OPFS-hit path folds it inside its async task. The trap is that
the key must be **bit-identical** to `oracle.rs`'s `grid_key` (FNV-1a over
`res`, `half_extent.to_bits()` and each height's `to_bits()`) or native and web
keys silently occupy different domains — and `oracle` folds with
`lunco_precompute::Fnv1a` while bake uses `lunco_hash::Fnv1a`, so one of them
has to move. Because the header layout changes, the wasm worker bundle must be
rebuilt with the main bundle or `LUNCO_WIRE_BUILD_ID` skew presents as a stale
worker rather than a version error.

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

### USDA nesting depth is unguarded in the fork

**What:** Bound recursion depth in the fork's USDA text parser.

**Why:** The USDC/USDZ readers were hardened — allocations grow with the bytes
actually delivered, decompression is bounded by LZ4's maximum expansion, table
indices resolve through checked accessors, and dictionary decode carries a depth
guard. The USDA text parser did not get its guard: `parse_block` is re-entered by
every nested construct, so a deeply nested file recurses until the stack
overflows. A stack overflow ABORTS — it is not an `Err` any caller can catch — so
no error handling above it can contain the failure.

An attempt was reverted rather than shipped: a limit of 256 still overflowed a
2 MiB test thread before the guard could fire, which means the ceiling has to be
chosen against the smallest stack the library runs on (test threads and wasm,
not the 8 MiB main thread), and the test has to run somewhere with a known stack
to prove the guard rather than the platform.

**Scope:** small, in `../openusd` (`src/usda/parser.rs`). The single choke point
is `parse_block`; a depth counter there covers the whole mutually-recursive
family. Low urgency while only local, authored files are parsed — it matters when
untrusted `.usda` can reach the parser.

### Time-sampled values are not unit-converted

**What:** Apply the canonical→stage conversion to `UsdOp::SetTimeSample` as
`SetAttribute` already does.

**Why:** The write-back inversion landed for `SetTranslate`, `SetRotate` and
`SetAttribute` — the last of those dispatching on the attribute's USD type role
and, for scalars, on the schema's declared linear unit. `SetTimeSample` carries
the same `type_name`/`value` pair and goes through the same authoring boundary,
but was not wired, so an **animated** position authored onto a Z-up/cm stage is
still written in canonical values while its static counterpart is written
correctly. A file where the static and sampled forms of one attribute disagree
is worse than one that is uniformly wrong.

**Scope:** small. The conversion helpers exist and are tested; this is the same
two call sites (author + inverse) in the `SetTimeSample` arm of
`UsdDocument::apply`.

### `!resetXformStack!` detaches to the stage root, not to the world

**What:** Let a `!resetXformStack!` prim ignore the stage root's own authored
xform, as strict USD does.

**Why:** The detachment itself landed — such a prim now reparents onto the
topmost prim of its own stage instead of hanging under its authored parent. It
anchors to the stage-root ENTITY rather than to nothing, deliberately: detaching
to nothing would drop the twin's mount placement and teleport the prim out of
the scene it belongs to, and a prim that is never grid-direct is an anchoring
contract avian depends on. The residual is that if the stage root itself authors
an xform, that one transform still applies where USD would drop it.

**Scope:** medium, and it is not really a USD problem — it needs the twin's
mount placement separated from the root prim's authored xform, so that
"detached" can mean the mount without meaning the authored value.

## 6. Architecture debt

### Core purity: the `SUBSYSTEMS` allowlist

**What:** Replace the hardcoded `SUBSYSTEMS` allowlist in `lunco-core` with
dynamic registration.

**Why:** The standing rule is that nothing domain-specific enters `lunco-core`
([`38-domains-as-packages.md`](38-domains-as-packages.md)). The drive kernels
half of this has landed — `DriveInputs`/`DriveMix`/`ControlKernelRegistry` now
live in `lunco-mobility` with the systems that consume them, and core no longer
names a vehicle concept. The static allowlist is the same disease in registry
form: adding a subsystem should be registration, not a core edit.

**Scope:** small–medium; a registration API plus the sites that read the
allowlist.

### `UsdDocument` stores `sdf::Data`, so every edit round-trips through text

**What:** Stop serializing the whole document to USDA and reparsing it on every
edit.

**Why:** `UsdDocument` holds `sdf::Data` layers, and `apply()` authors by calling
`open_doc_stage`, which does `data_to_usda(data)` → parse → `Stage` → author →
`extract_root_layer_data`. That is a full serialize-and-reparse of the ENTIRE
document per edit — dragging a gizmo pays it once per frame. It is also the last
place `sdf::Data` is load-bearing: the read path already retired its flattened
reader in favour of the composed `StageView`, because a flattened layer sees no
PCP composition.

**Scope:** two very different options, and the difference matters.

*(a) Wrap the existing data, no text.* `sdf::Layer::new(identifier, Box<dyn
AbstractData>)` already exists in the fork — it is what `new_in_memory` calls —
but is `pub(crate)`. Exposing it, plus a `StageBuilder` entry that accepts a
prepared root layer, lets `open_doc_stage` wrap the document's `sdf::Data`
directly. Two small fork additions, no concurrency change, no API churn here,
and it removes the dominant per-edit cost. **This is the recommended step.**

*(b) Hold a live `Stage` in the document.* Blocked, and not by us: `Stage` is
`!Send + !Sync` — `pcp/layer_graph.rs` holds an `Rc` and `StageInner` is
`RefCell`-based throughout — so it cannot live in a Bevy resource. Unblocking it
means converting the fork to `Arc`/`RwLock` across `StageInner` and `pcp`: a
permanent divergence from the tracked `mxpv/openusd` upstream, and atomics on
every composition operation. That is a product decision about the fork, not a
refactor, and should be taken explicitly rather than arrived at.

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

### Shaders are never compiled by CI

**What:** Validate the WGSL at build or test time, not only when a scene renders.

**Why:** `cargo check` does not compile shaders — naga_oil does, at startup, when
a material is first bound. So the entire shader tree can be broken by a rename
and every check stays green. The import graph is real code: `lunco::noise`,
`lunco::pbr_lit`, `lunco::horizon` and `lunco::transfer` are shared modules with
call sites across 20 files, and a symbol both `#import`ed and defined locally is
a hard naga_oil error. A static import/redefinition check catches most of it
cheaply; a headless run that binds one material of each family catches the rest.

**Scope:** small for the static check; small–medium for a headless render smoke
test.

### Per-crate builds are never exercised

**What:** Build each workspace member on its own (or `cargo hack --each-feature`)
in CI.

**Why:** `cargo check --workspace` unifies features across the graph, so a crate
whose code depends on a feature it does not itself enable still compiles as long
as SOME member turns that feature on. `lunco-workbench` was exactly this: a use
of `screenshot::` while `pub mod screenshot` sits behind `#[cfg(feature =
"api")]`. The workspace check was green; `cargo test -p lunco-workbench` was not.
Nothing rules out more of these, and the failure only appears to someone
depending on a single crate.

**Scope:** small — a CI matrix over members, or one `cargo hack` invocation.

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
