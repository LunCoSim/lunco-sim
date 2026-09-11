# General live-modeling capability audit

**Status:** Current open capability audit
**Reviewed:** 2026-09-11
**Scope:** Generic Editor and AI authoring of USD, Modelica, Rhai, and
physics-backed assemblies in the `tutorials` checkout.

## Result

The previous editing reports are stale as implementation plans. The recent
work has landed the generic authoring substrate: explicit USD documents and
edit targets, dry Rhai plans, standard-schema inspection, component bundle
regeneration, isolated previews, selection context, joint editing, topology
facts, lint, projection, and explicit save. Griffin and FLIP remain external
Twin content and regression consumers; they are not core concepts.

The actionable backlog is now concentrated in Editor presentation and
diagnostic navigation. There is no evidence that another vehicle-specific Rust
API, a second scene graph, a custom parametric schema, or a new USD writer is
needed for the next modeling slice.

## Capabilities confirmed implemented

These are closed findings and must not be reopened without a fresh regression.

| Area | Current owner and evidence |
|---|---|
| Typed, atomic USD authoring | `lunco-usd` `UsdOp`/`ApplyUsdOps`; document identity, edit target, generation checks, journal, undo, and structured acknowledgements |
| Composition and isolated previews | `lunco-usd-compose` plus `UsdPreviewId` sessions; `assembly_edit::fork_document`, preview views, text/visual modes, framing, reset, and explode presentation |
| AI authoring context | `assembly_builder::authoring_context`, `selected_authoring_context`, frame catalog, placement/attachment plans, and explicit selection identity |
| Schema-driven property editing | `editable_property_catalog` and `editable_property_patch_plan`; standard USD fields are preferred and operations remain dry until proposal/commit |
| Generic component bundles | `component_bundle_facts`, `component_bundle_plan`, and `component_bundle_update_plan`; visual/collision geometry, mass, frames, actuators, and bindings are data-driven Rhai |
| Human/AI component operation contract | `component_editor::update_context`, `selected_update_context`, and `update_plan`; the facade preserves topology and delegates mutation to the normal proposal/journal path |
| Repeated construction | Generic referenced-instance pattern and mirror plans; no Griffin/FLIP builder exists in the core |
| Assembly explainability | `QueryUsdPrim` topology facts and `assembly_audit::standard_component_report` cover visual/collision/material/body/joint relationships and source provenance |
| Selection and viewport UX | Focused-preview selection, multi-selection, prim-tree reveal/scroll, vehicle-root priority, transform gizmo ownership, joint overlays, and default Prims/Twin layout are implemented |
| Lint and collision checks | Document-scoped `RunLint`, `ValidateAsset`, standard component checks, and Twin namespace collision validation are available; old reports claiming the namespace lint is missing are obsolete |
| Runtime edit lifecycle | Authored edits advance generic `ModelStateRevision`; each backend owns its reaction. The USD projection does not know about Modelica recompilation or any future backend |
| Save and isolation policy | Explicit save is the default. `usd.editor_autosave=true` is opt-in; isolated document/fixture state is the default for runs and preview sessions |
| Physics and acceptance foundation | Existing telemetry, active-frame admission, non-finite input rejection, contact/settling/joint-distance evidence, and Rhai `physics_acceptance` helpers cover generic runtime checks |
| Semantic controls and tracing | Target-scoped semantic action edges, `SetPorts`, ownership, and bounded causal signal/actuator trace are implemented through the existing controller and diagnostics owners |

Primary implementation evidence includes commits `7058d9406`
(component bundles), `affccdae7` (topology facts), `8fb33c8c8` (semantic
edges), `a55191809` (causal trace), `3f90a9dae` (admission regression
coverage), `40420e271`/`06e537f53` (schema-aware component authoring and
Editor facade), and `84a14f769` (generic edit invalidation and lifecycle).

## Actual remaining gaps

### E1 — Make the generic component workflow usable by humans in the Editor

**Priority: P0 for efficient model authoring**

The backend and AI/Rhai contract exists, but the native Inspector still
renders `UsdParamView` as a scalar, bounded-slider draft and dispatches
`UsdAttributeBatchEditRequested` (`crates/lunco-luncosim-edit/src/ui/usd_params.rs`,
`ui/inspector.rs`). `InspectUsdSelection` advertises `UpdateComponent`, yet the
Inspector does not present the same recipe-driven component operation with its
catalog, bindings, topology-preserving plan, proposal review, and explicit
commit.

This is a presentation/bridge gap, not a reason to move component policy into
Rust. The next slice should let a human:

1. select a component and see the standard editable fields with units and
   source/provenance;
2. choose or provide the owning Twin/Rhai recipe explicitly;
3. review a dry typed plan and its affected paths;
4. apply, lint, reproject, undo, or save through the existing boundaries.

The view must not clamp an invalid authored value into a valid slider value or
silently discard inherited/overridden state. Invalid input should be visible
and rejected by the owner. AI and human callers should converge on the same
`component_editor` plan and proposal contract.

### E2 — Show a candidate component result before commit

**Priority: P1**

Dry plans are inspectable and committed edits can be viewed in an isolated
preview, but there is no focused Editor flow that renders the candidate
regenerated geometry and a before/after diff before committing it. This makes
human review of a dimensional change slower and forces an AI to infer too much
from operation lists.

Reuse the existing fork/proposal/preview and projection owners. A candidate
must be disposable, document-scoped, generation-aware, and clearly marked as
uncommitted. Do not add a second scene graph, write temporary USDA files, or
mutate the source document merely to render a preview. The review should show
changed paths, dimensions/transforms, material/collision consequences, and
lint results beside the candidate view.

### E3 — Navigate from a diagnostic to its exact authored part

`assembly_audit`, `RunLint`, and topology reports produce path-addressed
findings, but no complete native Editor route was found from a general lint or
audit finding to `SelectUsdPrim`, ancestor expansion, tree reveal, and camera
focus. Mount-specific diagnostic text exists, while the general structured
diagnostic-to-selection bridge is missing.

Add one generic action carrying the explicit preview/document/path identity.
It should select the path, reveal its ancestors, focus the relevant Inspector
drill target, and optionally frame the preview. The same action should be
available to an AI client through the typed command/query surface. Never
resolve by display name or by searching all open documents.

### E4 — Recipe identity and freshness are not yet persistent

`component_editor` deliberately requires an explicit component bundle recipe
because USD has no general standard parametric-recipe schema that this project
has adopted. This is a real limitation when a saved component must be
regenerated later without its Twin-side Rhai context, but it is not an
immediate blocker for the current Griffin/FLIP prototypes, whose recipes can
remain in the owning Twin package.

Defer a persistent recipe binding until two independent Twins demonstrate the
same need. If that threshold is reached, first evaluate existing USD asset,
variant, payload, and procedural metadata mechanisms; add only the smallest
standard-compatible authored identity and a freshness/compatibility check.
Do not introduce a `LunCoParametricAPI`, global recipe registry, expression
language, or name-based recipe guessing as a default solution.

### E5 — Geometry and constraint evidence should be extended only from real use

The generic topology report and collision AABB/clearance checks are sufficient
for prototype assembly. Detailed narrow-phase contact geometry, joint reaction
impulses, drive saturation, and energy-decay evidence remain useful for landing
and deployment studies, but they should be added only when the Griffin/FLIP
acceptance scenarios show that existing contact/settling/joint evidence cannot
explain a failure. This is a lower-priority diagnostics extension, not a new
authoring architecture.

### E6 — Twin-owned model fidelity remains outside the core audit

The actual Griffin and FLIP asset hierarchy, dimensions, materials, deployed
states, and mission acceptance belong in the published Twin/model package.
They must consume the generic tools and standard USD schemas. Missing fidelity
in that external content is not evidence for a Griffin-specific core builder
or Rust API.

## Recommended order

1. E1: bridge the existing schema/catalog and component plan into the native
   Inspector, with one end-to-end human and Rhai/AI acceptance path.
2. E2: add disposable candidate preview and before/after plan review using the
   existing isolated preview/proposal lifecycle.
3. E3: add diagnostic-to-selection/reveal/frame navigation.
4. Use Griffin/FLIP landing and articulation runs to decide whether E5 needs
   more evidence facts.
5. Only if multiple Twins prove it necessary, implement the minimal E4 recipe
   identity/freshness contract.

## Authoring contract that remains authoritative

The caller must carry the exact document, preview, edit target, USD path, and
generation. Rhai chooses component policy and builds a dry plan; USD typed
operations validate and journal the change; projection and backend adapters
observe the resulting revision; lint reports invalid state; save is explicit.
Autosave is off unless the user explicitly enables the documented setting.
Runs and previews are isolated by default. Modelica is one backend observer,
not a dependency of the generic edit/session mechanism.

## Explicitly out of scope

- Griffin/FLIP-specific core schemas or builders;
- raw USDA writers or a second mutation path;
- OpenCascade/BREP/STEP integration for the prototype Editor workflow;
- a custom LunCo parametric schema before standard USD mechanisms are shown to
  be insufficient;
- automatic save, implicit proposal commit, or runtime-layer persistence;
- Rust implementations of Twin policy that can be expressed in Rhai.

## Definition of done for the next slice

From one selected component, a human or AI can discover the same explicit
authoring context, produce the same dry update plan, view its disposable
candidate, inspect path-addressed diagnostics, commit one typed change set,
reproject, undo, and save deliberately. The source document remains unchanged
until commit, and all checks identify the exact document/path/generation.
