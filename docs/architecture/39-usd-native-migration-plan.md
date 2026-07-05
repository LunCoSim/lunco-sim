# 39 — Migration Plan: USD-Native Core

Status: **plan** (2026-07-03). Consolidates the actionable work from docs **36** (comms), **37** (model
synthesis), **38** (domains-as-packages / standards / naming). Constraint: **no legacy to preserve** —
every step is a clean cutover-with-verify, one canonical form, no back-compat shims.

> **Thesis (doc 38 §12):** `core = openusd (structure, mostly already there) + projection (USD→ECS) +
> runtime (solvers + PortRegistry value plane)`. This plan deletes the bespoke *middle* (parallel
> wiring/port/identity conventions) and moves the structure plane onto USD-native mechanisms the
> `openusd` crate already implements. The 60 Hz co-sim exchange loop is **never** in a diff.

## How to read this

Phases are **dependency-ordered**; within a phase, PRs are independent unless noted. Each PR lists
**goal · change · files · risk · verify · unblocks**. Source-of-truth sections in docs 36/37/38 are cited.
Legend: 🟢 low risk / no behavior change · 🟡 behavior change, verifiable · 🔴 needs an `openusd`
contribution.

---

## Networking-branch interlock (read before scheduling)

A separate **`networking` worktree** (branch `networking`, 9 commits ahead of `usd`) is doing a **USD
scene-representation move that partly *is* this migration's end-state** — plan around it, don't collide.

- **The journal/sync/hooks/RBAC/ports substrate this plan builds on is already in the `usd` base**, not
  incoming networking work: `journal_plane::domain_ops_after` (domain-scoped replay), scripted merge-policy
  hook, machine-unique `AuthorTag` authors, `ApplyUsdOp`→journal `EntryKind::Op{domain:Usd, op:UsdOp}`,
  `SetPorts`, `lunco-hash`. Phases 2/5 rely on it; it exists today.
- **P1.1 is already delivered on the networking branch — and over-delivered.** `flatten_stage`
  (`compose.rs`) now emits attribute **`connectionPaths`** via `attr.connections()`; *and* networking adds
  **`CanonicalStage` + a live `UsdRead`** (`canonical.rs`/`read.rs`/`view.rs`) that reads the live
  `openusd::Stage` directly and **removes the flattened-document representation**. Its `UsdRead` already
  folds `connectionPaths` + `targetPaths` into `read_rel_target`. → **When networking merges, the
  flattened path is deprecated in favor of the live Stage.** This plan should target the **live-Stage
  reader**, converging with networking, rather than the soon-to-be-deleted flatten path.
- **No conflict on the wiring code:** `SimConnection`, `PortRegistry`, `PortType`, and the
  `lunco:simWires`/wire-prim/`epsBus` parsing are **untouched** by networking's delta — P1's deletions and
  rework don't collide. Networking also **deletes** large autopilot/behavior/sandbox policy code (moving it
  to USD-authored projection), which *shrinks* the non-USD surface Phase 2 must reconcile — a tailwind.
- **The one real new gap: there is no typed `UsdOp::SetConnection`.** `UsdOp` has `SetRelationship` +
  `SetAttribute` but nothing that authors `connectionPaths`, so a connection edit replicates only via the
  coarse `ReplaceSource` fallback. **Op-selection (`domain_ops_after`, keyed by `DomainKind::Usd`, not by
  attribute name) needs no change** — but the plan must **add `UsdOp::SetConnection`** (see P1.0) for
  fine-grained wiring replication.

**Scheduling consequence:** ideally **land the networking merge first**, then build P1 on top of
`CanonicalStage`/`UsdRead`. If P1 must proceed first, keep connection-reading working on *both* readers
(both already surface `connectionPaths`) and expect the flatten path to retire on merge.

### Conflict-safe start set (do now, in files the networking delta does NOT touch)

Verified via `git diff 48f20e4d..04c924f4 --stat`. Networking is rewriting the USD **reader/compose** path
(`lunco-usd-bevy/{lib.rs,compose.rs,camera.rs,light.rs}` + new `canonical.rs`/`read.rs`/`view.rs`),
`lunco-usd-avian`, the **rhai bridge** (`lunco-scripting/{bridge_core,world_bridge}.rs`), `avian.rs`, and
deletes autopilot/behavior/sandbox-policy. **Avoid all of those.** These items are in untouched files and
additive:

| do now | where (untouched) | why safe / valuable |
|---|---|---|
| **P1.0 — `UsdOp::SetConnection`** | `lunco-usd/{document.rs,commands.rs}` (confirmed untouched) | additive enum variant + handler; **required regardless**; on the critical path |
| **P5.1 — electrical `connect()` spike** | `lunco-modelica` / rumoca (untouched) | standalone `.mo` + `compile_str` test; de-risks the biggest unknown (doc 37 §6) with zero USD-scene overlap |
| **P6.1 — `lunco-connectivity` crate** | **new crate** on `lunco-celestial` (untouched) | brand-new files = zero merge surface; builds the comms geometry substrate independently of wiring |
| **P0.3 — author `kind`** | `assets/**/*.usda` (additive) | asset-only; `openusd` already reads `kind`; no reader change |

**Defer until after networking merges** (files it is actively reworking): the P1.1–P1.5 connection cutover
(compose/read rework), **`PortType` deletion** (touches `avian.rs`), lights/camera/physics-sensor schemas
(`light.rs`/`camera.rs`/`lunco-usd-avian`), the USD-graph editor and any new rhai verbs (`read.rs` +
`world_bridge.rs`), and any attribute-read change (the `lib.rs` +465 reader restructure).

**Principle:** *safe = new crates/files + additive `UsdOp` variants + asset authoring + the Modelica
compile path. Risky = the USD scene-representation reader/compose/policy code and the rhai bridge —
exactly what networking is rewriting.*

---

## Phase 0 — Warm-ups (independent, parallel, ~no behavior change) 🟢

Prove the "adopt USD-standard spelling" muscle on the safest items. All tier-1 promotions (doc 38 §14.7).

- **P0.1 — `lunco:name`/`description` → `displayName` + `UsdUISceneGraphPrimAPI`.** `openusd` already has
  `SceneGraphPrimAPI` (`ui:displayName`/`ui:displayGroup`) + prim `displayName` metadata. *Verify:* names
  still render in browser/inspector. *Unblocks:* nothing; pure win.
- **P0.2 — `lunco:layer`/render-selection → `UsdCollectionAPI` + `UsdGeomImageable.purpose`/`visibility`.**
  `openusd` has `usd/collection.rs`. *Verify:* grouping/visibility behavior unchanged.
- **P0.3 — Author `kind`.** Add `kind="component"` to reusable part assets, `kind="assembly"` to composed
  vehicles/robots (doc 38 §8.4/§A6). *Verify:* `is_model`/`is_group` reflect it; no runtime change.
- **P0.4 — Rename `lunco:scale` → `lunco:factor`** (SSP term; doc 38 §14.1). Pure rename across assets +
  the reader; keep `lunco:offset`. *Verify:* wiring math identical. *Unblocks:* Phase 1 naming.

**Phase 0 done when:** four PRs merged, no behavior delta, team has seen a standard-schema adoption land.

---

## Phase 1 — The connections cutover (the crux; doc 38 §13) 🔴🟡

**POST-MERGE REDESIGN (2026-07-05): networking merged; USD is the main representation.** Three hard
requirements now shape P1: **(1) full migration, no legacy** — `connectionPaths` becomes the *only* wiring
encoding; **(2) the migrated code must be fast** — the hot loop stays by-slot and the projection must be
change-driven, never a rescan; **(3) every sim change is journaled + distributed** — the *only* way a
connection changes is a `UsdOp` that rides the journal to all peers. These three collapse into one pipeline
rather than a bolt-on projector.

**The one pipeline (nothing else may mutate wiring):**

```
edit ─► UsdOp::SetConnection            ◄─ the ONLY authoring path (no ECS-side wiring)
     ─► journal EntryKind::Op{Usd} ─► networking domain_ops_after ─► every peer applies
     ─► CanonicalStage openusd sink ─► RawStageChange { resynced, info_changed }
     ─► project_stage_changes (live_consume.rs) drains ─► reconcile:
            • structural (resynced)      : spawn/despawn prim entities        [exists]
            • connections (info_changed) : re-derive SimConnection edges       [NEW — P1.3]
     ─► RebuildOnChange<SimConnection, CompiledWiring> ─► ResolvedPort slots  [unchanged]
     ─► propagate_connections — by-slot hot loop                              [unchanged]
```

**Representation invariant:** USD `connectionPaths` is the **sole authored truth**. `SimConnection` and
`CompiledWiring` become **pure derived caches**, rebuilt incrementally from the change stream — *never*
authored directly. That is "USD is the main representation" made real for wiring, and it is what makes the
system deterministic across peers: replaying the same journaled op on any peer re-fires the same sink →
same reconcile → same `SimConnection` → same `CompiledWiring`. The current design (an ECS marker-query that
spawns `SimConnection` directly from parsed attrs at load) **bypasses the journal — that is exactly the
legacy to delete**, not merely the string encodings.

**What the merge already gave us (behind us):** read-source migration is done — both producers are
`UsdRead`-generic over the live `StageView`; `read.rs` already folds relationship targets and `.connect`
connections into `rel_target()`. So P1 is now *only* the encoding switch + moving derivation onto the
reconcile.

- **P1.0 ✅ DONE (committed `b0666d66`) — `UsdOp::SetConnection`, the sole authoring op.** Variant
  `SetConnection { edit_target, path, name, type_name, sources: Vec<String> }` in `lunco-usd/document.rs`;
  apply = `require_prim_anywhere` → parse `sources`→`SdfPath` → `stage.create_attribute(path.name, type_name)`
  → `set_connections(...)` (explicit `connectionPaths` list-op; **empty `sources` = clear**).
  `ApplyUsdOp{op:UsdOp}` is generic → auto-dispatches via API/MCP/rhai, records as `EntryKind::Op{domain:Usd}`
  → **journaled and distributed for free**. This is requirement (3) already satisfied at the authoring end.
- **P1.1 ✅ DONE (merged) — Connections survive composition + read off the live stage.** `StageView::rel_target`
  folds relationship targets and `.connect` uniformly (`read.rs:160`); both `UsdRead` impls surface connections.
- **P1.1b 🟡 NEW — `UsdRead::connections(prim, name) -> Vec<String>` (all sources).** `rel_target()` returns
  only the first target; the derivation needs **every** source on a fan-in `inputs:` attr. One trait method +
  both impls (`StageView`: `prim.attribute(name).connections()`; `sdf::Data`: the `connectionPaths` `PathListOp`
  items). Small, additive, in `read.rs`. *Unblocks:* P1.3.
- **P1.2 🟡 — Ports become `inputs:`/`outputs:` attributes** on component prims. Attr base name = the
  `PortRegistry`/Modelica-output/avian-input name (one name, two planes) — already true for `netForce`,
  `force_y`. *Verify:* `PortRegistry` resolves the same names.
- **P1.2b 🟡 — Transform metadata (SSP `LinearTransformation`).** `connectionPaths` carries the edge, not its
  factor/offset. Author `lunco:factor` (default 1.0) + `lunco:offset` (default 0.0) on the **consuming**
  `inputs:` attribute (sink owns its scaling); carry over from today's wire-prim `lunco:scale`/`lunco:offset`,
  completing the `lunco:scale`→`lunco:factor` rename. *Verify:* a scaled edge propagates `src*factor+offset`.
- **P1.3 🔴 REDESIGNED KEYSTONE — connection derivation *on the reconcile*, not a load-time scan.** Delete the
  `Without<UsdSourcedWire>` marker-scan model entirely; it can't see edits (re-authoring `connectionPaths` on
  an already-examined prim never re-fires) and it rescans at load. Instead, hook derivation into the **single
  change-driven reconcile** (`project_stage_changes` → `reconcile_structural_live`, `lunco-usd/live_consume.rs`)
  that already drains `RawStageChange { resynced, info_changed }` and owns the prim↔entity map (`find_live_entity`):
  - For each changed prim (`info_changed` for a `connectionPaths` edit; `resynced` for prim add/remove), **re-derive
    its edges**: despawn the `SimConnection`s whose sink is this prim, then enumerate its `inputs:*` attrs and for
    each call `reader.connections(prim, "inputs:<port>")`, spawning one fresh `SimConnection { start_element:
    by_path[src_prim], start_connector: <src leaf minus `outputs:`>, end_element: by_path[this], end_connector:
    <sink leaf minus `inputs:`>, scale: factor, offset }` per source (fan-in → multiple rows; `propagate` sums).
  - **Self-loop (`A==B`) and cross-entity (`A≠B`) fall out of the same rule** — the two old flavors unify.
  - Cost: **empty drain = zero work**; a real edit touches only the changed prim's edges — no rescan, no staleness.
  This is requirement (2) at the projection end; the hot loop (`RebuildOnChange` → `CompiledWiring` →
  `propagate_connections` by-slot) and `SimConnection`'s shape are **untouched** — they are now *derived caches*.
  *Verify (key gate):* author a connection via `SetConnection` on a host → (a) it cosims identically to the old
  `sun_tracker`/rover wiring, and (b) a late-joining networked client converges to the same `SimConnection` set
  purely from journal replay (requirements 1+2+3 in one test).
- **P1.4 🟡 — Migrate all assets to `connectionPaths` (via `SetConnection`, so the migration itself is journaled),
  then delete ALL legacy.** Remove: the `lunco:simWires` parse in `process_usd_cosim_prim_read` + `parse_wire`;
  `process_usd_cosim_wire_read` (whole system) + `lunco:wireFrom`/`wireTo`/`fromPort`/`toPort`; the
  `any_unprocessed_usd_cosim_wires` gate + the `UsdSourcedWire` marker; `rel lunco:epsBus`; **and any code path
  that spawns `SimConnection` outside the reconcile** (the bypass of requirement 3). One canonical form, no shim.
- **P1.5 🟢 — Delete `PortType` + `classify`** (dead; doc 38 §A3). Residual typing → attr `typeName` (later
  `ConnectableAPIBehavior`, P4.3).

**Phase 1 done when:** `connectionPaths` is the *only* wiring encoding; a connection changes **only** via
`UsdOp::SetConnection` → journal → networking → reconcile (no ECS-side authoring survives); `SimConnection` and
`CompiledWiring` are derived-only, rebuilt incrementally; `propagate.rs`/`CompiledWiring`/`ResolvedPort` and the
by-slot hot loop are untouched; two peers converge on identical wiring from replayed ops.

---

## Phase 2 — Identity becomes real & standard (doc 38 §A5, §14.2, §14.7) 🔴🟡

- **P2.1 🔴 — Promote `LunCo*API` to real codeless applied schemas.** Collapse the four dead role-APIs
  (`LunCoPowerComponentAPI`/`PowerDistributionAPI`/`ActuatorAPI`/`MobilityComponentAPI`) into **domain**
  schemas `LunCoElectricalAPI`/`ThermalAPI`/`CommsAPI` (**multi-apply**), with component **role** as an
  attribute/`kind`, not a schema-per-role (doc 38 §14.7). *Risk:* `openusd`'s schema registry is a stub
  (`schemas/registry.rs`) — needs either typed views or a light codeless-schema registry contribution.
  *Verify:* `has_api_schema` gates dispatch.
- **P2.2 🟡 — Gate on applied schema, not the `simWires`-presence heuristic** (`cosim.rs:149`). Identity =
  "has `LunCoModelBindingAPI`" / domain schema, not "has a wire attr."
- **P2.3 🟡 — Unify the behavior binding:** `lunco:modelicaModel`/`pythonModel`/`scriptModel` →
  **`lunco:behavior:model`** + **`lunco:behavior:kind`** (`modelica|python|rhai|fmu`); `modelicaClassName`
  → `behavior:className`. This is a **SysML allocation** and the USD+FMI convergence hook (doc 38 §14.2,
  §14.5, §A10). *Verify:* cosim binding resolves identically across all three backends.

**Phase 2 done when:** domain identity is a load-bearing applied schema; the binding is one neutral
`lunco:behavior:*` allocation.

---

## Phase 3 — Domain params & standard-schema adoption (doc 38 §14.3, §14.7) 🟡

Mostly independent of Phases 1–2; can interleave.

- **P3.1 — Fold EPS/motor params into model parameters.** `lunco:voltage`/`capacity`/`resistance`/
  `torqueConstant`/… are the bound model's parameters (FMI parameter / SysML attribute), authored via
  `lunco:params` → **typed USD attributes**; use the MSL class's own name where it exists (`R`, …). Drop
  bespoke duplicates.
- **P3.2 — Camera → `UsdGeomCamera`; lights → `UsdLux`; sensors → mirror Isaac shapes** (doc 38 §8.5).
  Keep `lunco:cameraMode` (behavior). *Verify:* camera/light/sensor behavior unchanged.
- **P3.3 — `lunco:params` string-dict → typed USD attributes** (removes a bespoke encoding; aligns to
  "no JSON for internal logic").
- **P3.4 — Lean placeholder/asset resolution on USD payloads + Ar;** `lunco:resolvedAsset`/`assetMode`
  shrink to a thin runtime cache (doc 38 §14.7).

**Phase 3 done when:** params are typed attributes/model parameters; camera/light/sensor use standard (or
Isaac-mirrored) schemas.

---

## Phase 4 — Generic graph editor over USD (doc 38 §4, §A4, §A7) 🟡

Depends on Phase 1 (connections exist to edit).

- **P4.1 — `DomainDescriptor` trait + open `DomainRegistry`** (model on `DocumentKindRegistry`);
  **re-express the existing Modelica editor as the first descriptor** — pure refactor of the four
  Modelica-locked canvas free-functions (palette, `project→Scene`, `SceneEvent→ops`, connection-rule) into
  descriptor slots. *Verify:* Modelica canvas behaves identically.
- **P4.2 — USD-graph canvas adapter:** `USD stage → Scene` (prims→nodes, `inputs:`/`outputs:`→ports,
  connections→edges) + `SceneEvent → Vec<UsdOp>` (`EdgeCreated`→author connection, `NodeMoved`→
  `ui:nodegraph:node:pos` via `UsdUINodeGraphNodeAPI`), through journaled `ApplyUsdOp`. *Verify:* editing a
  USD electrical graph round-trips to the stage.
- **P4.3 🔴 — `ConnectableAPIBehavior` per domain** for connection legality (fills the canvas greenfield,
  `tool.rs:14`). *Risk:* `openusd` lacks the behavior-plugin registry — implement a lightweight per-domain
  rule hook (rhai/descriptor) instead of the full pxr plugin.

**Phase 4 done when:** one canvas edits Modelica *and* USD domain graphs, driven by descriptors; layout in
`NodeGraphNodeAPI`.

---

## Phase 5 — Model synthesis (doc 37) 🟡

Depends on Phase 1 (the connection graph is the synthesizer's input) + Phase 2 (domain identity). **The
runtime substrate this phase assumes is already in the `usd` base** (doc 37 §8): `journal_plane`
domain-op selection + scripted merge-policy hook + machine-unique `AuthorTag` + `ApplyUsdOp`→journal — so
"synthesis rides the journal + `AuthorTag::for_tool("synth")` + `rbac.authorize`" needs no new plumbing.

- **P5.1 — Spike:** author one real `connect()`-based `Electrical.mo` (battery + bus + 2 loads),
  `compile_str` → confirm it steps to correct bus voltage/currents (doc 37 §7, §6 caveat: no test proves
  MSL-electrical numeric sim yet). Decide MSL-import vs self-contained on cold-compile feel.
- **P5.2 — Netlist-from-USD synthesizer + `SynthesizerRegistry`** (rhai policy + Rust primitives + hooks;
  doc 37 §3, §8): read the connectable electrical graph → emit one acausal `Electrical.mo` → `compile_str`
  → `SimStepper`; boundary ports scalar-wired to other domains. Scaffold-and-own (hand-editable).
- **P5.3 — Enforce the two-level rule** (doc 37 §1): acausal within a `.mo`, causal across via connections.
  A `wiring` synthesizer can even emit the cross-domain `SimConnection` boundary.

**Phase 5 done when:** a rover's electrical layer is one synthesized, hand-editable `Electrical.mo`
boundary-wired via connections; the synthesizer is a registered rhai-authored entry with lifecycle hooks.

---

## Phase 6 — The comms feature rides on top (doc 36) 🟡

Depends on Phases 1–2 (ports/connections/identity) + optionally 5 (electrical draw).

- **P6.1 — `lunco-connectivity` crate:** `SightLine`/`CommsField` geometry core (analytic body-sphere
  occlusion + elevation), reading `EphemerisResource`/`world_position_seeded` (doc 36 §3).
- **P6.2 — `lunco:comms:antenna` flag → `CommsLink` component** via the USD-sim projection (doc 36 §5);
  outputs to `PortRegistry` + AOS/LOS `TelemetryEvent`.
- **P6.3 — `CommsLink.mo`** (doc 34 gap; template `Battery.mo`) + the comms **domain descriptor**; comms
  component as a reusable multi-layer part (doc 36 §2).
- **P6.4 — Sky:** reuse Earth `GlobeLod` (coarse `max_lod:0` default), honor `DomeLight.texture:file` →
  `Skybox`/`EnvironmentMapLight` (doc 36 §7).

---

## Cross-cutting: `openusd` contributions (the only work that leaves our repo) 🔴

The right home for standard-shaped library gaps (doc 38 §12.4). In dependency order:
1. **`connectionPaths` through flatten** — **already done on the networking branch** (lands on merge); no
   longer a required contribution, just a merge dependency.
2. **`UsdOp::SetConnection`** (P1.0) — **required, and in our repo** (`lunco-usd`), not `openusd`: a typed
   journal op so connection edits replicate finely instead of via `ReplaceSource`.
3. **Codeless applied-schema registry** (P2.1) — nice-to-have; typed views suffice interim.
4. **`ConnectableAPIBehavior` mechanism** (P4.3) — optional; rhai rules suffice interim.

Everything else our adopt-plan needs (attr connections + `ConnectionGraph`, UsdShade `Connectable`,
`kind`, `NodeGraphNodeAPI`, collections, apiSchemas author/read/multi-apply, `UsdPhysics`) **already works
in `openusd` v0.5** (doc 38 §12.2) — adoption is deletion on our side, not implementation.

---

## Federation seams (track, don't build now; doc 38 §11)

- **SPICE → USD xforms** — already done (`lunco-celestial`); carry NAIF ids as prim metadata.
- **USD+FMI** — the one live convergence; `lunco:behavior:*` (P2.3) is the hook. Track AOUSD; doc 37 already
  implements the pattern (candidate to contribute).
- **SysML v2 API → USD projection** — future; the naming (part/port/connection/flow/allocation, doc 38 §14.5)
  makes it near-mechanical when built.
- **XTCE/PUS** — telemetry refs as port metadata, not USD-owned.

---

## Sequencing summary & critical path

```
networking merge (CanonicalStage + connectionPaths) ─┐   ← land first if possible
P0 (warm-ups, parallel) ─────────────────────────────┤
P1.0 UsdOp::SetConnection ───────────────────────────┴─► P1 (connections cutover) ─┬─► P4 (graph editor)
                          P2 (identity) ───────────────────────────────────────────┼─► P5 ─► P6 (comms)
                          P3 (params/schemas, parallel) ───────────────────────────┘
```

- **Critical path:** networking merge (or cherry-pick `connectionPaths`) → **P1.0 `UsdOp::SetConnection`**
  → P1.3 (projector) → P1.4/1.5 (deletes). The old "flatten keystone" is retired — the keystone is now
  **the merge + the `SetConnection` op**; target the live-Stage reader, not flatten.
- **Coordinate with networking:** prefer landing its merge first and building P1 on `CanonicalStage`/
  `UsdRead`; otherwise keep both readers surfacing connections.
- **Parallelizable now:** all of P0, P1.0, and P3.
- **Guardrail:** the co-sim `propagate.rs` hot loop must not appear in any Phase-1 diff (doc 38 §13.2) —
  if it does, the PR is wrong.

## Definition of done (whole migration)

Wiring/ports/identity/layout expressed in USD-native mechanisms; the bespoke middle deleted (three wiring
encodings, `PortType`, dead `LunCo*API` roles, `lunco:` duplicates); `lunco:` reduced to genuine glue
(behavior binding, comms/celestial/net/scenario/render); one generic descriptor-driven editor; the core is
`openusd (structure) + projection + runtime (solvers + PortRegistry)`. Standards federated across clean
seams; nothing invented where USD, FMI/SSP, or SysML v2 already has the name.
