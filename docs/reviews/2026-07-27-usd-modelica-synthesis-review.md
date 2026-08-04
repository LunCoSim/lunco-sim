# USD → Modelica dynamic model building — review (2026-07-27)

> Status: Acted on (2026-07-28); C2 was superseded by the synthesis-unit boundary
> on 2026-08-04 — every finding below is fixed or resolved; see
> "Disposition" at the end for what each one became. Kept as the record of WHY
> the code is shaped the way it now is.

Scope: the runtime path that turns composed USD into compiled Modelica.

- `crates/lunco-usd-sim/src/cosim.rs` — per-prim programs (`LunCoProgramAPI` + `info:sourceAsset`), wiring derivation, port publication.
- `crates/lunco-usd-sim/src/domain_projection.rs` — network synthesis (`Scope` + `CollectionAPI:components` → one generated `.mo`).
- `crates/lunco-usd-avian/src/lint.rs` — the network *facts* the rhai lint rules consume.
- `docs/architecture/37-model-synthesis-and-multidomain-composition.md` — the stated design.

## What is right

- **USD is the public contract, published at BIND** (`cosim.rs:539-560`). The interface exists before the source loads, so a wire never transiently reads as an unknown port. One extraction shared by every solver language.
- **Wires are a pure derived cache** rebuilt whole from the composed stage (`cosim.rs:1231`), keyed by *instance* identity, not path — two spawns of one asset don't collapse.
- **`STRUCTURAL_INPUT_BINDINGS`** (`cosim.rs:1206`) with a stated rule for adding a row is the correct answer to phantom wires, and the rule is enforceable by review.
- **Generated identifiers are injective and keyword-safe** (`domain_projection.rs:869-972`), with tests. `emit_modelica` is deterministic (BTree ordering) and edge-deduped.
- **Acausal-within / causal-across** is honoured: a `connect()` never leaves the generated island; boundary quantities cross as scalar wires.

## Concept-level problems

### C1. Historical divergence in "read a Modelica facet from USD"

| Concern | per-prim program | network member | lint fact |
|---|---|---|---|
| gate | `info:sourceAsset` extension (`cosim.rs:465-473`) | composed source reference (`program.rs::modelica_source_ref`) plus loaded declaration | composed source reference (`program.rs::modelica_source_ref`) |
| class name | irrelevant (whole file is the model) | resolved from loaded source text | deferred to the runtime source resolver |
| interface check | `UsdModelicaPortContract` vs compiled DAE (`cosim.rs:596-671`) | none | USD-vs-USD only |
| islands | n/a | not partitioned (see C2) | BFS island count (`lint.rs:648-661`) |

Three readers that must agree about one authoring contract, and they already don't. `lint.rs:539-700` re-derives member scan, program-source validity, connector-target existence, ambiguous boundary sources and causal fan-in — all of which `read_network`/`validate_network` already compute. Per the house rule (facts = Rust once, rules = rhai), the fact producer should *call* the runtime reader, not restate it.

The former divergence allowed an asset that linted clean (`…/parts/Battery.mo`) to fail at runtime because the projector invented a class from a path. That path-derived behavior is removed; the loaded source declaration is authoritative.

Same duplication one layer down: `cosim.rs:757-831` and `domain_projection.rs:350-396` are the same five steps (parse → extract name/params/inputs → build `ModelicaModel` → `send(Compile)` → handle a closed channel), written twice, with unexplained differences in `is_stepping`, `is_compiling`, `resume_after_compile` and error routing. One `dispatch_compile(entity, source, doc_uri, opts)` would remove the drift surface.

### C2. `partition_islands` is dead; islands are a lint rule instead of a projector behaviour

`partition_islands` (`domain_projection.rs:96`) is `pub`, tested, and **called from nowhere**. The runtime emits exactly one model per `Scope`, regardless of how many disconnected acausal islands the collection contains; the "one island per scope" invariant is enforced only by a rhai rule — and lint never runs on load. A scene that skips lint gets one DAE containing N electrically independent circuits (structurally singular unless every island is independently well-posed).

Either the projector partitions (and the lint rule disappears), or `partition_islands` is deleted. Shipping both, with only one wired up, is the worst of the two.

### C3. The synthesizer is hardcoded, while doc 37 §8 describes a registry

Doc 37 specifies pluggable synthesizers (`electrical`, `thermal`, `harness`, `wiring`, …) dispatched by an open registry with rhai policy. What exists is one Rust system with the mapping baked in: `Scope` + `collection:components:` → one wrapper. There is no seam to add `thermal` without editing `project_domain_islands`. Either add the registry seam or mark §8 as unimplemented — right now the doc reads as description, not as plan.

### C4. Nothing checks USD's declared network interface against the actual `.mo`

`validate_network` (`domain_projection.rs:647`) validates USD against USD: `declared_connectors` and `declared_outputs` are just the prim's own attribute names. A battery prim that declares `outputs:soc_out` while `Battery.mo` names it `soc` passes every check and fails in the compiler — against generated source the user cannot read (B4). The per-prim path has exactly this check (`UsdModelicaPortContract`); the network path should get the equivalent after compile.

## Bugs

### B1. A causal-only collection member is compiled twice

`cosim.rs:441-449` skips standalone compilation only when the prim carries `connectors:` attributes. `read_network` includes **any** member with `LunCoProgramAPI` and explicitly supports causal-only members (`domain_projection.rs:585-587`). Such a member with `inputs:`/`outputs:` passes the active-cosim gate (`cosim.rs:474`) and gets its own `SimComponent` + solver **and** an instance inside the generated wrapper: two independent solvers for one authored part, the standalone one feeding the wire fabric.

Not reachable from shipped assets today (every member declares `connectors:p`), so this is a live trap for the next controller/PDU added to a collection. Gate on *collection membership*, not on the presence of a connector.

### B2. An acausal part outside every network scope is silently inert

Same gate, other direction: `connectors:` present → `UsdSimProcessed`, return. No model, no `SimComponent`, no notice. A battery placed in a rover with no `Electrical` scope simply does not exist to the simulation. `assets/scenes/tests/lint_selftest.usda:371` documents this as "THE PHANTOM WIRE" and the lint catches it — but lint is not on the load path, so at runtime it is silent. This deserves a `ModelicaNotice`/telemetry at bind time.

### B3. One dropped member rejects the whole network — the shipped failure

`retain_connected_acausal_components` (`domain_projection.rs:814`) deliberately drops an unwired acausal part ("a legitimate installed-but-unwired part"). The root-output validation (`domain_projection.rs:777-801`) then requires that same part to be in the surviving set, and rejects the **entire island** with `output source component … is outside collection …` — no ports, no `soc`, no `solar_power`.

That is verbatim the failure recorded in `assets/scenes/tests/solar_domain_nested_ref.usda:19-31` ("the rover silently lost its electrical domain … no error anywhere a driver would look"). The retain policy and the boundary validation contradict each other. Pick one: drop the orphaned boundary output with a warning, or keep an acausal component that a boundary output names.

Related silent half: a member that loses `LunCoProgramAPI` through composition is skipped without comment (`domain_projection.rs:494`), so the first symptom is the misleading boundary error.

### B4. The generated source is written and never read

`GeneratedModelicaSource` (`domain_projection.rs:39-47`) is documented as "inspectable runtime artifact for diagnostics and API/UI projection" and has **no reader anywhere in the workspace** — no API query, no UI panel, no test. Compiler diagnostics cite `generated://<name>.mo` line numbers against text nothing can display. Either expose it (a `GetGeneratedSource` query keyed by network root, or route it through the document registry) or delete the component and the clone it costs per projection.

### B5. Every generated island probably compiles twice on scene load

The model name embeds the instance GID (`domain_projection.rs:439`), and the fingerprint is taken over the source that contains that name. GIDs are minted in `PostUpdate` (`assign_global_entity_ids`), while `project_domain_islands` runs in `Update`: the first pass sees `None` and dispatches `…_System`, the `Added<GlobalEntityId>` pass then dispatches `…_G<gid>_System`. The worker compiles **serially**, so a wasted cold compile sits in front of everything else on the critical path. Verify with worker compile logs on a rover scene; if confirmed, gate projection on identity being present rather than re-running after it lands.

### B6. The Modelica class name is guessed from the file path

The previous projector guessed a class from the asset path and never opened the `.mo`.
That rejected twin-owned sources outside a `models/` root and made directory renames
silently change the generated class. The current source-resolution boundary reads
`within` plus the declared class from the loaded file; a pending source stays pending
and a failed source becomes a terminal projection error.

### B7. Non-finite constants are emitted verbatim

`emit_modelica:160-167` writes `value.to_string()`. A NaN or infinite authored input becomes `NaN` / `inf` in the source: a compile error attributed to the model rather than to the authoring. Reject non-finite values in `read_network` as a `DomainProjectionError`.

### B8. `DefaultHasher` for the projection fingerprint

`source_fingerprint` (`domain_projection.rs:447`) uses `std::collections::hash_map::DefaultHasher`, which carries no cross-version stability guarantee; the workspace has `lunco-hash` for exactly this. In-process-only use makes it harmless today — it is a substrate-bypass, not a live bug.

## Test coverage

- `read_network` has **no tests at all**. Every `domain_projection` test hand-builds a `DomainNetwork`, so the reader — the layer where B3 shipped, and where composition/reference arcs actually bite — is untested.
- No golden test asserts the generated source for a shipped asset. `shipped_assets_lint_clean.rs` covers the lint facts, not the projector.
- Missing: compose `rocker_bogie.usda` (both `power` variants) → `read_network` → `emit_modelica` → snapshot; plus a case that the `solar` variant with a dropped panel reports something actionable (B3).

## Disposition (2026-07-28)

| # | What changed |
|---|---|
| B1 | `process_usd_cosim_prims` now skips a prim because a component collection OWNS it (`lunco_usd_bevy::program::network_member_paths`), not because it declares a connector. A causal-only member no longer gets a second solver. |
| B2 | The converse is now loud: a prim with `connectors:*` that no network owns warns, naming the prim and the fix, instead of silently never simulating. |
| B3 | `retain_connected_acausal_components` returns what it omitted; `read_network` drops the boundary outputs published through omitted parts (with a warning) instead of rejecting the whole island. Covered by `tests/domain_projection_reader.rs`. |
| B4 | `GeneratedModelicaSource` API query — lists every projected network or returns one by `network_root`, with the exact compiled text, its `generated://` URI and last error. |
| B5 | Projection defers while `UsdInstanceMember` is present (identity pending), so an island compiles once, under a name that is unique per spawn. Confirmed real: instance descendants are parked as `Provenance::Local` until the root id is minted, so two spawns previously shared one `generated://…` worker session. |
| B6 | Class resolution uses the loaded Modelica source; path-derived class naming was removed. The composed USD validator only checks the source reference and optional `subIdentifier`, while the runtime records pending/invalid source verdicts explicitly. |
| B7 | Non-finite constants are rejected at read time, against the property that carries them. |
| B8 | `lunco_hash::fnv1a64` replaces `DefaultHasher`. |
| C1 | One composed-USD source-reference reader (`lunco_usd_bevy::program`) is used by the projector AND lint facts; the loaded source resolver owns the declared class, with one `lunco_modelica::parse_model_interface` path for extraction. |
| C2 | Superseded on 2026-08-04: `ProgramGraph` and `partition_network` now belong to the synthesizer. A Scope remains one runtime participant, while disconnected graph units are emitted as explicit composite Modelica child models; the lint rejection was removed. |
| C3 | Doc 37 §8 now carries a `NOT IMPLEMENTED` status note describing what actually ships. |
| C4 | Projected networks carry a `UsdModelicaPortContract` too, so the existing post-compile validator holds a generated wrapper to its authored boundary. |
| Tests | `tests/domain_projection_reader.rs` covers `read_network` + `emit_modelica` against composed USD fixtures — the layer that had none. |
