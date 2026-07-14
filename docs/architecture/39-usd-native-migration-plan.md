# 39 — Migration Plan: USD-Native Core

Status: **active plan** · Audience: engineers working the USD-native migration. Consolidates the actionable
work from docs **36** (comms), **37** (model synthesis), **38** (domains-as-packages / standards / naming).
Constraint: **no legacy to preserve** — every step is a clean cutover-with-verify, one canonical form, no
back-compat shims.

> **Thesis (doc 38 §12):** `core = openusd (structure, mostly already there) + projection (USD→ECS) +
> runtime (solvers + PortRegistry value plane)`. This plan deletes the bespoke *middle* (parallel
> wiring/port/identity conventions) and moves the structure plane onto USD-native mechanisms the
> `openusd` crate already implements. The 60 Hz co-sim exchange loop is **never** in a diff.

## How to read this

Phases are **dependency-ordered**; within a phase, PRs are independent unless noted. Source-of-truth
sections in docs 36/37/38 are cited. Legend: 🟢 low risk / no behavior change · 🟡 behavior change,
verifiable · 🔴 needs an `openusd` contribution. Items already in place are marked **[in place]** with a
one-line statement of what exists; everything else is remaining work.

**Foundation already in place.** The canonical-stage substrate this plan targets has landed: a live
`CanonicalStage` reads the composed `openusd::Stage` directly through a generic `UsdRead` seam
(`lunco-usd-bevy/{canonical,read,view}.rs`), and both readers surface `connectionPaths`. The journal /
sync / hooks / RBAC / ports substrate Phases 2/5 build on also exists in the base: domain-scoped op replay
(`journal_plane::domain_ops_after`), the scripted merge-policy hook, machine-unique `AuthorTag` authors,
`ApplyUsdOp`→journal `EntryKind::Op{domain:Usd}`, `SetPorts`, `lunco-hash`. So the migration is now the
encoding switch + moving derivation onto the reconcile, not a reader rewrite.

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

USD is the main representation, so three hard requirements shape P1: **(1) full migration, no legacy** —
`connectionPaths` becomes the *only* wiring encoding; **(2) the migrated code must be fast** — the hot loop
stays by-slot and the projection must be change-driven, never a rescan; **(3) every sim change is journaled
+ distributed** — the *only* way a connection changes is a `UsdOp` that rides the journal to all peers.
These three collapse into one pipeline rather than a bolt-on projector.

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

Read-source migration is already in place: both producers are `UsdRead`-generic over the live `StageView`,
and `read.rs` folds relationship targets and `.connect` connections into `rel_target()`. So P1 is *only* the
encoding switch + moving derivation onto the reconcile.

- **P1.0 [in place] — `UsdOp::SetConnection`, the sole authoring op.** Variant
  `SetConnection { edit_target, path, name, type_name, sources: Vec<String> }` in `lunco-usd/document.rs`;
  apply = `require_prim_anywhere` → parse `sources`→`SdfPath` → `stage.create_attribute(path.name, type_name)`
  → `set_connections(...)` (explicit `connectionPaths` list-op; **empty `sources` = clear**).
  `ApplyUsdOp{op:UsdOp}` is generic → auto-dispatches via API/MCP/rhai, records as `EntryKind::Op{domain:Usd}`
  → journaled and distributed. Satisfies requirement (3) at the authoring end.
- **P1.1 [in place] — Connections survive composition + read off the live stage.** `StageView::rel_target`
  folds relationship targets and `.connect` uniformly; both `UsdRead` impls surface connections.
- **P1.1b [in place] — `UsdRead::connections(prim, name) -> Vec<String>` (all sources).** `rel_target()`
  returns only the first target; the derivation needs **every** source on a fan-in `inputs:` attr, so
  `connections()` returns the full list on both impls (`StageView`: `prim.attribute(name).connections()`;
  `sdf::Data`: the `connectionPaths` `PathListOp` items).
- **P1.2 🟡 — Ports become `inputs:`/`outputs:` attributes** on component prims. Attr base name = the
  `PortRegistry`/Modelica-output/avian-input name (one name, two planes) — already true for `netForce`,
  `force_y`. *Verify:* `PortRegistry` resolves the same names.
- **P1.2b [in place] — Transform metadata (SSP `LinearTransformation`).** `connectionPaths` carries the edge,
  not its factor/offset, so `rewire_usd_connections` reads `lunco:factor:<port>` (default 1.0) +
  `lunco:offset:<port>` (default 0.0) on the **sink** prim, keyed by the consuming port — each input owns its
  own scaling. The derived `SimConnection` takes `scale`/`offset` from these; the hot loop propagates
  `src*factor+offset` unchanged. Sibling-attr encoding (not attribute metadata) so it reads through the
  precision-tolerant `UsdRead::real`; this also completes the `lunco:scale`→`lunco:factor` rename. Covered by
  `rewire_applies_factor_and_offset` (double-authored) and `rewire_reads_float_authored_transform` (the
  float-authored case a strict `double` read would silently drop). *Fan-in note:* factor is per-input, so a
  multi-source input shares one transform — per-source factors would need per-list-element metadata (deferred;
  no current asset needs it, all migrated wires are identity).
- **P1.3 [in place] — connection derivation *on the reconcile*, not a load-time scan.** `rewire_usd_connections`
  rebuilds the derived `SimConnection` set from `connectionPaths` when prim entities spawn/despawn (structural)
  or a connection edit is drained (`WiringDirty`) — never a marker-scan that can't see edits. For each changed
  sink prim it despawns that prim's `SimConnection`s, then enumerates its `inputs:*` attrs and for each source
  from `reader.connections(prim, "inputs:<port>")` spawns one `SimConnection { start_element: by_path[src_prim],
  start_connector: <src leaf minus `outputs:`>, end_element: by_path[this], end_connector: <sink leaf minus
  `inputs:`>, scale, offset }` (fan-in → multiple rows; `propagate` sums). Self-loop (`A==B`) and cross-entity
  (`A≠B`) fall out of the same rule. Empty drain = zero work; the hot loop (`RebuildOnChange` → `CompiledWiring`
  → `propagate_connections` by-slot) and `SimConnection`'s shape are untouched — they are derived caches.
  Covered by `usd_connection_derivation.rs`: derivation-at-load + clear, plus every migrated asset reads back
  the exact edges the old `lunco:simWires` / wire-prims encoded.
- **P1.4 🟡 — Migrate all assets to `connectionPaths` (via `SetConnection`, so the migration itself is journaled),
  then delete ALL legacy.** Remove: the `lunco:simWires` parse in `process_usd_cosim_prim_read` + `parse_wire`;
  `process_usd_cosim_wire_read` (whole system) + `lunco:wireFrom`/`wireTo`/`fromPort`/`toPort`; the
  `any_unprocessed_usd_cosim_wires` gate + the `UsdSourcedWire` marker; `rel lunco:epsBus`; **and any code path
  that spawns `SimConnection` outside the reconcile** (the bypass of requirement 3). One canonical form, no shim.
- **P1.5 [in place] — `PortType` / `port_type` / `classify` deleted entirely.** The tag was cosmetic: it fed
  only the port API's `"kind"` field, which had **zero** programmatic consumers (no UI, rhai, MCP, or
  connection-validation branch read it), and it was not even reliable — `classify(name)`, a lossy name
  heuristic, disagreed with the explicit const-table tags in 5 groups. So the whole taxonomy is gone: the
  `port_type` field is removed from `PortRef`/`SimPort`/`AvianPort` (and its const-table literals), `classify()`
  is deleted, and `port_to_json` no longer emits `"kind"`. Adding a new domain needs no core edit. If a real
  consumer ever needs a port's domain, it should be an authored USD attribute/token, not a name heuristic or a
  closed enum. (Kept: the unrelated Modelica connector-type `ComponentPort.port_type`, which carries
  "Pin"/"Flange" for diagram rendering.)

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
  → `SimulationSession`; boundary ports scalar-wired to other domains. Scaffold-and-own (hand-editable).
- **P5.3 — Enforce the two-level rule** (doc 37 §1): acausal within a `.mo`, causal across via connections.
  A `wiring` synthesizer can even emit the cross-domain `SimConnection` boundary.

**Phase 5 done when:** a rover's electrical layer is one synthesized, hand-editable `Electrical.mo`
boundary-wired via connections; the synthesizer is a registered rhai-authored entry with lifecycle hooks.

---

## Phase 6 — Connectivity rides on top (doc 49) 🟢 / Sky (doc 36 §2) 🟡

Depends on Phases 1–2 (ports/connections/identity) + optionally 5 (electrical draw).

- **P6.1–P6.3 — DONE, but not as a comms feature.** There is no comms crate, no comms component and no
  comms vocabulary: connectivity landed as a **generic link kernel** in `lunco-celestial`
  (`LinkNode`/`LinkState`, cadence-gated geometry: range + elevation + body occlusion + terrain LOS),
  with the verdict behind the language-neutral `link.connected` hook and routing authored in rhai over
  the `query("Links")` snapshot. The USD vocabulary is `lunco:linkNode` / `lunco:link:*`. See
  `49-connectivity-link-kernel.md`. A comms *domain* (link budget, `CommsLink.mo`, margin validation) is
  authored content on top of that kernel — the domain-package shape of doc 38 — and remains open work.
- **P6.4 — Sky:** reuse Earth `GlobeLod` (coarse `max_lod:0` default), honor `DomeLight.texture:file` →
  `Skybox`/`EnvironmentMapLight` (doc 36 §2).

---

## Cross-cutting: `openusd` contributions (the only work that leaves our repo) 🔴

The right home for standard-shaped library gaps (doc 38 §12.4). In dependency order:
1. **`connectionPaths` through flatten** — **in place**; no longer a required contribution.
2. **`UsdOp::SetConnection`** (P1.0) — **in place, in our repo** (`lunco-usd`), not `openusd`: a typed
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
CanonicalStage + connectionPaths + P1.0 SetConnection  [in place]
P0 (warm-ups, parallel) ───────────────────────────────────────► P1 (connections cutover) ─┬─► P4 (graph editor)
                          P2 (identity) ─────────────────────────────────────────────────────┼─► P5 ─► P6 (comms)
                          P3 (params/schemas, parallel) ─────────────────────────────────────┘
```

- **Critical path:** the remaining P1 work is **P1.2 (ports as `inputs:`/`outputs:` attrs)** → **P1.4 (asset
  migration + legacy deletes)**; the reader, `SetConnection` op, and derivation-on-reconcile (P1.0/P1.1/P1.3)
  are already in place and read off the live stage.
- **Parallelizable now:** P0 and P3 run independently of the P1 tail.
- **Guardrail:** the co-sim `propagate.rs` hot loop must not appear in any Phase-1 diff (doc 38 §13.2) —
  if it does, the PR is wrong.

## Definition of done (whole migration)

Wiring/ports/identity/layout expressed in USD-native mechanisms; the bespoke middle deleted (three wiring
encodings, `PortType`, dead `LunCo*API` roles, `lunco:` duplicates); `lunco:` reduced to genuine glue
(behavior binding, comms/celestial/net/scenario/render); one generic descriptor-driven editor; the core is
`openusd (structure) + projection + runtime (solvers + PortRegistry)`. Standards federated across clean
seams; nothing invented where USD, FMI/SSP, or SysML v2 already has the name.
