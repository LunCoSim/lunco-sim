# SysML v2 embedding in the terrain worktree

**Status:** Foundation implemented; Twin source discovery and UI projections remain follow-up work
**Reviewed:** 2026-09-13
**Worktree:** `terrain` (`terrain-streaming`)

## Executive decision

Use the `sysmlv2` family from `haradama/sysml-v2-rs`, embedded behind new
LunCoSim domain crates. The runtime dependency should be
`sysmlv2-semantics = 0.1.1`; it already depends on `sysmlv2-syntax = 0.1.2`
and `sysmlv2-model = 0.1.1`. Bundle `sysmlv2-stdlib = 0.1.0` as data.

Do not make the parser own paths, filesystem traversal, asset bytes, or
Rhai execution. The existing LunCoSim owners remain authoritative:

```text
Twin/FileEntry
  -> lunco-assets (canonical ids, twin://, storage, cache)
  -> SysmlSource asset
  -> SysmlDocument / domain engine
  -> sysmlv2 Workspace::add_file + resolve_reached
  -> lunco-doc diagnostics and RefIndex
  -> read-only Rhai requirement facade
  -> Rhai verification/test verdict
```

## Implemented in terrain

The first production slice now exists behind the opt-in `sysml` feature:

- `lunco-sysml-ast` pins the upstream parser/semantic crates, loads the
  embedded standard library, and emits serializable elements, references, and
  syntax/name/collision diagnostics. It never touches the filesystem.
- `lunco-sysml` provides the UTF-8 `.sysml`/`.kerml` Bevy asset loader, a
  generation-aware `SysmlDocument` with reversible `ReplaceSource` and
  `EditText` operations, `FileBacked`/fork behavior, and a generic
  `DocumentRegistry` plugin. SysML operations enter the canonical journal as
  `DomainKind::Sysml`.
- `lunco-sysml-rhai` exposes only a read-only `sysml_report_json()` Rhai
  function over an immutable analysis snapshot. Script policy and verdict
  ownership stay in the existing Rhai test runner.
- `lunco-scene-validation` registers a compact `ValidateSysml` API query for
  authored tests. It reuses `ValidateAsset`'s parser/resolver and projects
  requirement/verification names, scalar literals, diagnostics, and the
  source revision only; no second source walker or Griffin-specific Rust
  policy is introduced.
- `lunco-luncosim-core`, the GUI shell, and the headless server expose the
  `sysml` feature gate; default builds remain unchanged.

This deliberately does not add a full SysML editor, a filesystem source-root
walker, a BREP/CAD pipeline, or automatic requirement-test discovery. Those
pieces can consume the stable projections above without changing the parser or
document contracts.

## Rhai-to-SysML verification migration

The existing Rhai scene tests should not be translated line-for-line. They
contain three different kinds of material:

| Existing material | Destination |
|---|---|
| Requirement wording, stable ID, threshold, subject, and traceability | SysML `requirement def`/usage plus `doc`, attributes, constraints, `satisfy`, and realization links |
| Verification intent and requirement under test | SysML `verification def`/usage with `subject` and `verify` |
| Scene setup, command sequencing, sampling, public queries, anti-trivial guards, and measurement | Existing USD fixture and Rhai backend over production APIs |
| Solver/parser/schema/lifecycle mechanism assertions | Existing Rust owner tests |

The stable key is the qualified SysML name. A human label such as `GR-001`
can remain in `doc` until a standard metadata projection is implemented; do
not add a LunCo-only annotation to the portable source. A run must produce a
separate result artifact, not mutate the `.sysml` file. The minimum result
shape is:

```json
{
  "verification": "GriffinRequirements::Verify_GR001",
  "requirement": "GriffinRequirements::GR001_MassBudget",
  "source_revision": 12,
  "verdict": "pass",
  "observations": [{"name": "mass", "value": 438.2, "unit": "kg"}],
  "evidence": [{"source": "usd", "path": "/Griffin/Rover"}],
  "diagnostics": []
}
```

This maps to `VerificationCases::VerdictKind` (`pass`, `fail`,
`inconclusive`, `error`) and can be persisted in the journal/run directory.
`TESTS_OK`/`TESTS_FAIL` remains a compatibility envelope while the new CLI/API
selector and JSON report are introduced.

### Required implementation slices

1. Extend `lunco-sysml-ast` with requirement and verification records, subject
   bindings, constraint/doc spans, and typed `satisfy`/`verify`/realization
   links. Preserve source-set revision and diagnostics.
2. Build one Twin-indexed `.sysml`/`.kerml` source set from `Twin::files()` and
   resolve it once; never add a second filesystem walker or feed cache paths to
   the parser.
3. Add a Twin-owned verification registry from qualified verification name to
   the existing production scene and Rhai backend source. Missing mappings are
   terminal errors.
4. Add only the small Rhai bridge needed to expose the resolved SysML snapshot,
   selected verification key, and source revision. The authored Rhai observer
   owns the result map; extend the existing production runner/CLI to emit the
   JSON result above plus a human summary. Do not add a Rust test per
   requirement.
5. Add shadow-mode comparison against the legacy Rhai verdict, then switch the
   production gate and delete duplicate assertions only after positive,
   negative, anti-trivial-motion, stale-generation, and evidence checks pass.

Thresholds may move from Rhai into SysML attributes/constraints only when the
parser preserves their values and units. Runtime measurement, actuation, and
the executable test remain Rhai/public-query responsibilities; SysML is not a
second simulator. The Rust change should stay minimal: a read-only snapshot
registration, a key/revision handoff, and reuse of the existing `luncosim test`
runner/result protocol.
This preserves the ownership split: SysML states intent, USD owns geometry and
identity, Modelica owns continuous behavior, and Rhai executes policy and
collects evidence.

## What the existing domains already establish

### Modelica

`lunco-modelica-ast` is a pure leaf crate. It owns source normalization,
strict/recovered parsing, AST extraction, and lint facts without Bevy, storage,
workers, or UI. The runtime crate (`lunco-modelica-core`) then owns:

- `ModelicaSource` and its `.mo` `AssetLoader`;
- `ModelicaDocument` plus `DocumentHost` state and generation counters;
- a long-lived `ModelicaEngine`/Rumoca session;
- async parsing with stale-generation rejection and bounded completion work;
- source-root discovery and demand-driven library loading;
- journal, workspace, diagnostics, and API observers.

The important rule is AST-canonical input: parsing is explicit and can be
performed on a worker or satisfied from a cache before the engine is updated.
The engine does not read raw source itself.

### USD

USD separates raw source from the composed runtime stage:

- `UsdSourceText` loads one `.usda` layer through `AssetServer`;
- `UsdStageAsset` represents the prepared/composed stage;
- `UsdPlugins` composes visual, Avian, simulation, and document command
  plugins;
- the Twin projection waits for the source asset event, opens one document,
  publishes the composed bytes as a `twin://` overlay, and mounts the stage.

This is the correct model for SysML: load bytes through the existing asset
source, create one file-backed document, then build a domain-owned semantic
projection. SysML must not create a second Twin path or source reader.

### Rhai

Rhai is feature-gated in `lunco-scripting` and demonstrates the asynchronous
asset/synchronous-language boundary:

- `RhaiSourceLoader` loads text and discovers literal imports;
- imported handles are retained as Bevy dependencies;
- `ScriptSources` stores text by the same canonical id that `AssetServer` uses;
- `AssetModuleResolver` performs synchronous lookup and memoizes compiled
  modules by source text;
- `LunCoScriptingPlugin` owns registration, generation-aware recompilation,
  journaling, diagnostics, and fixed-step execution.

SysML should copy this lifecycle and cache discipline, but not Rhai's import
syntax. SysML imports are semantic qualified names, so the source set should
come from the already-indexed Twin and declared source roots rather than a
second textual path walk.

## AST and operation reuse: Rumoca and OpenUSD

The existing dependencies provide reusable *patterns and boundaries*, not a
common AST. Modelica and SysML have unrelated grammars and semantic types;
forcing either language through the other's tree would make diagnostics,
round-tripping, and requirement extraction less reliable.

### What can be reused from Rumoca

Rumoca already separates the parser artifact from the long-lived compiler
session:

- `rumoca-phase-parse::SyntaxFile` carries either a strict or recovered
  `StoredDefinition` together with parse errors;
- `rumoca_compile::Session` owns stable file ids, source sets, revisions,
  transactional `SessionChange` input updates, and query/snapshot caches;
- `CompiledSourceRoot::from_parsed_batch_tolerant` indexes a parsed batch once
  and defers strict target compilation;
- `lunco-modelica-ast` wraps those APIs and exposes only pure parsing/fact
  extraction, while `lunco-modelica-core` owns Bevy, workers, generations,
  document ops, and journal integration.

SysML should reuse the same lifecycle decisions: tagged parse results,
source-set revisions, immutable read snapshots, tolerant diagnostics, and
stale-result rejection. Do not depend on Rumoca's `StoredDefinition`,
`Session`, or Modelica name-resolution code from a SysML crate; those types
encode Modelica grammar and equation semantics.

### What can be reused from OpenUSD

OpenUSD's authoring path demonstrates the correct structured-edit pattern:

- USDA is parsed/written by OpenUSD itself;
- `sdf::Data` is the send-safe authored representation;
- a transient `openusd::usd::Stage` performs path-addressed authoring;
- `SdfPath` and `NamespaceEditor` prevent ambiguous name-based edits;
- `UsdDocument` records typed reversible ops, generation numbers, an op log,
  and coarse `Resync` versus `InfoOnly` changes;
- the live composed stage remains a projection, not the saved source.

This operation model is valuable for SysML, but `sdf::Data` is not a suitable
SysML AST. Reuse the existing `lunco-doc::Document`, `DocumentHost`,
`FileBacked`, `DomainEngine`, diagnostics, and `RefIndex` machinery instead.
For the first SysML subset, support `ReplaceSource` and `EditText`; add typed
requirement/part operations only when the selected SysML library exposes
stable source spans or a safe rewriter. A structured edit should be addressed
by element id/qualified name, produce a new semantic snapshot, and record a
typed inverse—never splice text by an unqualified name.

### DRY rule

Keep Modelica-specific normalization, AST extraction, and AST mutation in
`lunco-modelica-ast`/`lunco-modelica-core`. Share only domain-neutral pieces:

1. `lunco-doc` for document identity, undo/redo, generation, diagnostics,
   file-backed reload, and cross-document references;
2. `lunco-assets`/`TwinRoots` for canonical ids, `twin://`, storage, and cache;
3. the Modelica/Rhai generation and worker ideas as a template, not as a
   second parser dependency or copied Modelica engine.

Only extract a generic parse-driver or source-set snapshot helper after SysML
and Modelica have two measured implementations with identical behavior. This
keeps the abstraction DRY without introducing a speculative framework.

## SysML crate layout

Keep third-party APIs behind LunCoSim types and preserve the existing Cargo
dependency firewall:

### `lunco-sysml-ast` (pure)

- `SysmlSyntax` / recovery diagnostics;
- requirement, part, port, connection, satisfy, verify, and realization facts;
- conversion to `lunco_doc::Diagnostic`, `SymbolPath`, and `SymbolRef`;
- no Bevy, filesystem, `AssetServer`, or Rhai dependency.

`sysmlv2-semantics` can be used here for the semantic snapshot. Do not expose
its internal model directly to the UI or scripting layer.

### `lunco-sysml` (runtime/domain)

- `SysmlSource` and `.sysml` asset loader;
- `SysmlDocument` implementing the existing `Document` contract;
- one long-lived per-process/per-Twin semantic workspace handle;
- generation/source-set invalidation;
- Twin mount/close observers;
- optional source-root registry for `[sysml] externals`;
- `DomainEngine` implementation whose `Index` is UI-facing.

### `lunco-sysml-rhai` (optional adapter)

- read-only requirement and traceability queries;
- report serialization for authored Rhai tests;
- no parser AST in `Dynamic` values;
- no USD reads of its own. Runtime facts continue through existing USD query
  providers.

Do not add `sysmlv2-*` dependencies to `lunco-assets`, `lunco-twin`, or
`lunco-doc`. Those crates are shared foundation and must remain domain-neutral.

## Feature and dependency proposal

Add exact workspace pins in the root `Cargo.toml`, following the existing
centralised Rumoca/OpenUSD/Rhai dependency policy:

```toml
sysmlv2-semantics = "=0.1.1"
sysmlv2-stdlib = "=0.1.0"
```

`sysmlv2-syntax` and `sysmlv2-model` should be direct dependencies only if
the wrapper needs their public types. Otherwise they remain transitive through
`sysmlv2-semantics`.

Expose a single application feature, analogous to `python`, `networking`, and
`tracy`:

```toml
sysml = ["dep:lunco-sysml", "dep:lunco-sysml-rhai"]
```

The initial feature can be opt-in for lean server builds. Once Griffin
requirements are required by the normal production test path, include `sysml`
in that profile. Keep any UI panels behind the existing `ui` feature; parsing
and requirement reports must work headlessly.

`sysmlv2-stdlib` is text data with an EPL-2.0 license. If it is redistributed,
retain its `LICENSE`/`NOTICE` attribution. It is small enough to parse once per
process initially; only add a serialized preparse cache if startup measurement
shows that it is needed.

## Asset and document integration

1. Consume `.sysml` entries from `Twin::files()`. Do not walk the Twin again.
2. Load source as `twin://<assigned-name>/<relative-path>` through the existing
   `TwinRoots`/`AssetServer` source.
3. Register `SysmlSource` with an idempotent plugin, like
   `ModelicaSourceAssetPlugin` and `UsdSourceTextLoader`.
4. On the terminal asset event, call the shared document open path with the
   file origin and source text. Preserve dirty documents and one-document-per-
   file identity.
5. Build a semantic snapshot by adding the loaded source set with
   `Workspace::add_file(relative_name, text)`.
6. Add standard-library files from `sysmlv2-stdlib::FILES` before project
   sources.
7. Resolve only entry documents and their reachable symbols with
   `resolve_reached`; do not call `Workspace::load_dir`.
8. Convert definitions/references into the shared `RefIndex`, and let
   `DocumentChanged`/Twin close events invalidate or retire the snapshot.

For external references such as `@"path"::"selector"`, canonicalize the path
with `lunco_assets::asset_path` and resolve it through `lunco://` or `twin://`.
The parser must receive a stable logical filename; it must not be handed an
absolute cache path.

## Efficiency rules

- Use one Twin index and one shared catalog traversal. If a requirement browser
  is added, join the existing `list_assets_with_extensions` generation rather
  than adding a SysML walk.
- Keep a content/source-set revision and reject stale async parse results, as
  Modelica does.
- Cache semantic snapshots by source-set revision, per-file content hashes,
  standard-library release, and parser version.
- Rebuild a workspace snapshot when a changed file has the same logical name;
  `Workspace::add_file` appends a second file for changed text.
- Clone a resolved standard-library workspace for read-only queries rather than
  rebuilding the library for every requirement request.
- Keep parse/resolve work off the UI thread. Use the existing Bevy task pool on
  native and the worker-transport pattern on wasm if measurements require it.
- Use `lunco-hash` fast hashes for local invalidation and content-addressed
  hashes only for durable artifacts or transfer.

## Remaining gaps after the foundation

- `TwinManifest` has no `[sysml]` field even though the architecture document
  specifies one; `deny_unknown_fields` means that optional section still needs
  a manifest/schema decision.
- The engine asset manifest lists `usda`, `wgsl`, and `rhai`, not `sysml`.
  Add SysML to the shipped manifest only if the engine library contains SysML
  assets. Twin-local documents already come from the Twin index.
- The runtime plugin does not yet scan `Twin::files()` and open every source
  automatically. A caller still supplies the existing canonical asset/source
  event to `DocumentRegistry::open_file`.
- No cross-file `RefIndex` adapter, async source-set worker, or requirement
  verification hook is wired into the production `luncosim test` command yet;
  `ValidateAsset` is now the read-only pre-flight entry point for one SysML
  source file.
- The AST projection currently exposes generic elements/references rather than
  structured requirement/verification records; extraction of subjects,
  constraints, and trace links is the next implementation slice.
- No verification-case registry, Rhai key/revision handoff, cross-file
  `RefIndex` adapter, async source-set worker, or production requirement
  selector exists yet. `ValidateAsset` remains the read-only pre-flight entry
  point for one source file.
- The architecture and Rhai test-strategy documents now define the migration
  contract; no existing Rhai gate has been switched until the shadow-mode and
  evidence requirements above are met.

## Griffin workflow

Griffin should keep its current ownership split:

- SysML: architecture, requirement IDs, satisfy/verify/realization links;
- USD: geometry, prim identity, frames, topology, and dimensions;
- Modelica: continuous equations and state;
- Rhai: executable checks, mission policy, and the final verdict.

The existing `GR-xxx` checks can be linked from SysML without duplicating their
numeric policy. A SysML requirement record should point to a Rhai verification
hook and, where needed, an explicit USD prim or Modelica realization. Planned
checks must remain non-passing until a production Rhai run produces evidence.

## Acceptance sequence

The first implementation should be validated with one small Griffin fixture:

1. load `.sysml` through `twin://`;
2. verify source identity, origin, and generation;
3. report syntax/semantic diagnostics;
4. resolve a requirement and its USD/Modelica realization;
5. run the existing Rhai check through the production binary;
6. inspect the authored requirement verdict and the runtime evidence.

This is an analysis artifact only. The terrain worktree's unrelated untracked
files were preserved.
