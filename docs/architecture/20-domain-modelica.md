# 20 — Modelica Domain

> Status: Active · Audience: contributors on behavioral modeling and the Modelica workbench
>
> Behavioral modeling using Modelica + the rumoca runtime. Wired into
> Bevy ECS as a `ModelicaDocument` per model, stepped in `FixedUpdate` on
> a background worker thread.
>
> Engineering docs live in
> [`../../crates/lunco-modelica/`](../../crates/lunco-modelica/) and
> [`../../crates/lunco-cosim/README.md`](../../crates/lunco-cosim/README.md).

## Contents

- [1. Scope](#1-scope)
- [2. Architecture in layers](#2-architecture-in-layers)
- [3. Runtime architecture — background worker](#3-runtime-architecture--background-worker)
- [4. Execution pipeline](#4-execution-pipeline)
- [5. Document System integration](#5-document-system-integration)
- [6. The `output` convention (rumoca workaround)](#6-the-output-convention-rumoca-workaround)
- [7. Type vs. instance distinction](#7-type-vs-instance-distinction)
- [7a. Document identity — three tiers, one truth](#7a-document-identity--three-tiers-one-truth)
- [8. New-model workflow (target)](#8-new-model-workflow-target)
- [9. The Modelica diagram editor](#9-the-modelica-diagram-editor)
- [9c. Canvas animation + multi-user roadmap](#9c-canvas-animation--multi-user-roadmap)
- [10. Panels (current + planned)](#10-panels-current--planned)
- [11. Current gaps](#11-current-gaps)
- [12. Dymola-workflow parity](#12-dymola-workflow-parity)
- [13. See also](#13-see-also)

## 1. Scope

Modelica is LunCoSim's language for declarative behavioral models —
electrical circuits, thermal flow, life-support systems, balloon dynamics,
anything with equations of motion or state. Models live in `.mo` files
attached to 3D entities (Space Systems) in the USD scene.

The Modelica runtime is **rumoca**, our fork:
[`github.com/LunCoSim/rumoca`](https://github.com/LunCoSim/rumoca).

## 2. Architecture in layers

```
  ModelicaDocument  (Tier 1: canonical, persistent)
       │
       │  Op-based editing (add/remove components, set params, ...)
       │  Serializes to .mo files via rumoca_parser
       │
       ▼
  ECS projection  (Tier 2: runtime)
    - ModelicaModel component: parameters, inputs, variables, paused, session_id
    - ModelicaChannels resource: crossbeam channels to background worker
    - Background worker thread: owns SimStepper instances, async
       │
       │  DocumentView<ModelicaDocument>
       │
       ▼
  Views  (Tier 3: panels)
    - DiagramPanel (lunco-canvas)
    - CodeEditorPanel (text view)
    - ModelicaInspectorPanel (params + live variables)
    - GraphsPanel (time-series plots)
    - PackageBrowser / LibraryBrowser (MSL + project models)
```

## 3. Runtime architecture — background worker

`SimStepper` (rumoca's solver) is `!Send` — it can't cross threads. So we
own it on a dedicated background worker thread; the main Bevy thread
communicates via `crossbeam` channels:

- `ModelicaCommand::Compile { entity, source, session_id }` — parse + build
  DAE + instantiate stepper
- `ModelicaCommand::UpdateParameters { entity, source, session_id }` —
  substitute new parameter values, recompile
- `ModelicaCommand::Reset { entity, session_id }` — rebuild stepper from
  cached DAE, reset to initial conditions
- `ModelicaCommand::Step { entity, inputs, dt }` — advance one timestep
- `ModelicaResult { entity, session_id, outputs, variables, error, ... }`
  — returned after each command

Session IDs fence stale results: when a Compile or UpdateParameters bumps
the session, any in-flight Step for the old session is discarded.

Panics in the worker are caught (`catch_unwind`) and reported as solver
errors rather than crashing the app. This tolerance is essential for
interactive parameter tuning — an unstable parameter shouldn't kill the
whole sim.

## 4. Execution pipeline

All cosim and stepping happens in `FixedUpdate` at a shared fixed
timestep (60 Hz by default). Ordering is enforced via system sets:

```
FixedUpdate:
  ModelicaSet::HandleResponses    — drain results from worker channel
  (sync_modelica_outputs)         — ModelicaModel.variables → SimComponent.outputs
  CosimSet::Propagate             — propagate_connections
  CosimSet::ApplyForces           — apply_sim_forces
  (sync_inputs_to_modelica)       — SimComponent.inputs → ModelicaModel.inputs
  ModelicaSet::SpawnRequests      — send next Step command with fixed dt
```

See [`22-domain-cosim.md`](22-domain-cosim.md) for the full pipeline.

### 4.1 Run-state machine + command semantics

Live stepping is gated per-entity by run-state on `ModelicaModel`.
**Compiling a model never auto-starts a live realtime sim** — a fresh
compile leaves the model paused/ready, and live stepping begins only on
an explicit Run.

```
Uncompiled/Stale ──[Compile]──▶ Ready (paused) ──[Run]──▶ Running
                                      ▲                      │
                                      └────────[Pause]───────┘
Compile error ─────────────────▶ Blocked (paused)
```

State on `ModelicaModel`:

- `paused: bool` — the per-frame gate in `spawn_modelica_requests`
  (`if model.paused { continue }`). Running ⇔ `is_compiled && !paused`.
- `is_compiled: bool` — worker has installed a stepper.
- `is_compiling: bool` — a Compile is in flight.
- `compiled_generation: u64` — document `generation_owned()` at the last
  *successful* compile.
- `pending_generation: u64` — generation captured at compile dispatch;
  promoted to `compiled_generation` on success so an edit landing
  mid-compile does not mark the just-built model as up to date.
- `resume_after_compile: bool` — transient; set by `RunActiveModel` so
  the post-compile success handler unpauses (instead of staying paused).
  Cleared on both success and error so a failed Run never silently
  auto-plays on a later unrelated success.

Staleness: `stale = !is_compiled || compiled_generation != gen`, where
`gen` is the document's current `generation_owned()`.

Verb semantics:

| Verb | Effect |
|---|---|
| `CompileModel` / `CompileActiveModel` | Compile only, idempotent. Skips the worker dispatch when `is_compiled && !stale && !is_compiling` (logged at debug); pass `force: true` to override. Never changes `paused`. |
| `RunActiveModel` | Compile-if-stale, then play. If already compiled & clean, just sets `paused = false` (no recompile); otherwise sets `resume_after_compile = true` and triggers `CompileModel`, which resumes on success. |
| `ResumeActiveModel` | Unpause (`paused = false`); no compile. |
| `PauseActiveModel` | Pause (`paused = true`). |
| `ResetActiveModel` | Bump `session_id`, send `ModelicaCommand::Reset`, zero `current_time` / `last_step_time`. Cheap — no recompile. |
| `RestartActiveModel` | Composition of `ResetActiveModel` + `RunActiveModel`. |
| `FastRunActiveModel` | Orthogonal: batch compile + simulate off-thread → `Experiment`. Never touches live run-state. |

The toolbar (`ui/panels/model_view/render.rs`) maps these to one
Compile button (🚀 → `CompileActiveModel`, compile only), a Run/Pause
toggle (▶ → `RunActiveModel`, ⏸ → `PauseActiveModel`), Reset (⟲), and
Restart (⟳ → `RestartActiveModel`). The `CompileStatus` API query
reports the run-state (`is_compiled`, `is_compiling`, `paused`,
`running`, `stale`, `current_time`) alongside the compile state.

## 5. Document System integration

`ModelicaDocument` implements the Document System trait
(see [`10-document-system.md`](10-document-system.md)) and is the
authoritative in-memory representation of a `.mo` file.

### 5.1 Canonical state: text + cached AST

Source text is canonical. The AST is a **cached projection**,
refreshed eagerly after every mutation so panels that need structural
access (diagram, parameter inspector, placement extractor) can read
`doc.ast()` without reparsing.

```rust
pub struct ModelicaDocument {
    id: DocumentId,
    source: String,                    // canonical
    ast: Arc<AstCache>,                // derived, refreshed per op
    generation: u64,
    origin: DocumentOrigin,
    last_saved_generation: Option<u64>,
    changes: VecDeque<(u64, ModelicaChange)>,
}

pub struct AstCache {
    pub generation: u64,
    pub result: Result<Arc<StoredDefinition>, String>,
}
```

Text is canonical (not AST) because:

- Comments and hand-chosen formatting must round-trip losslessly.
  A code editor that reformats the file on every parameter tweak is
  not usable — IDE-style edits require that only *edited regions*
  change text, not the whole file.
- AI tooling (e.g. Claude's `Edit` / `Write`) operates on text ranges.
  A text-canonical document is compatible with those flows out of the
  box; an AST-canonical one would require a dedicated text adapter.

The trade-off is that we need **span-aware AST ops** — see § 5.3.

### 5.2 Op set

`ModelicaOp` is `#[non_exhaustive]` — all variants are implemented and
cover the full Phase α editing surface:

| Op | Effect | Used by |
|----|--------|---------|
| `ReplaceSource { new }` | Full-buffer swap | Coarse text commits |
| `EditText { range, replacement }` | Byte-range replacement | Granular text edits |
| `AddComponent { class, decl }` | Insert a new component declaration | Diagram: drag from palette |
| `RemoveComponent { class, name }` | Delete a component's full declaration | Diagram: right-click delete |
| `AddConnection { class, eq }` | Insert a `connect(...)` equation | Diagram: wire ports |
| `RemoveConnection { class, from, to }` | Delete a `connect(...)` equation | Diagram: disconnect wire |
| `SetPlacement { class, name, placement }` | Set/replace the `Placement` annotation | Diagram: drag-to-move |
| `SetParameter { class, component, param, value }` | Replace one parameter modifier | Parameter inspector |

`class` accepts qualified dotted paths (`Pkg.Inner`,
`Modelica.Blocks.Continuous.Integrator`) so ops work on nested classes
too.

### 5.3 Apply pipeline

Every op funnels through a single pure function
`op_to_patch(source, ast, op) -> (range, replacement, change)` and a
single mutation path `apply_patch(range, replacement, change)` that:

1. Validates bounds + char-boundary alignment
2. Splices the source buffer
3. Bumps generation, refreshes `AstCache`
4. Pushes the structured change onto the ring buffer
5. Returns an `EditText` inverse carrying the exact removed bytes

All ops — including AST-level ops — are implemented as **span-locate +
text patch**:

- `AddComponent` → locate insertion point via `ClassDef.equation_keyword` /
  `end_name_token` tokens, render the decl via the pretty-printer,
  splice
- `RemoveComponent` → use `Component.location` for the full decl span,
  extend through the terminating `;` using a paren-aware scanner
- `RemoveConnection` → find the matching `Equation::Connect` by port
  pair (order-insensitive, Modelica `connect` is symmetric), scan
  backward to the `connect` keyword, forward to `;`
- `SetPlacement` → locate the component's annotation span, find the
  `Placement(...)` sub-call at paren depth 0, replace in place;
  sibling annotations (Dialog, Documentation) are preserved
- `SetParameter` → locate the component's modifications list, parse
  entries at depth 0, replace `name = value` or append as needed

This keeps all source mutation on one code path (uniform undo,
uniform change emission) and means every op produces a byte-level
minimal patch — comments and formatting outside the edited span stay
intact.

### 5.4 Pretty-printer for new nodes

The pretty-printer only renders **new** nodes being spliced in. It is
deliberately *not* a full AST round-trip serialiser — existing nodes
live in the source text, never re-emitted.

```rust
pub struct PrettyOptions {
    pub indent: String,
    pub continuation_indent: String,
}
PrettyOptions::tabs()       // "\t" / "\t\t" — workbench default
PrettyOptions::two_space()  // "  " / "    " — library default + tests
```

The workbench installs tab indentation at startup (`ModelicaPlugin::build()`).
Options are process-wide so every op path (diagram panel, scripts,
tests) produces consistent output. Annotations go on their own
continuation line so generated source stays readable:

```modelica
	Modelica.Blocks.Continuous.Integrator I1
		annotation(Placement(transformation(extent={{-10,-10},{10,10}})));

	connect(I1.y, Gain1.u)
		annotation(Line(points={{0,0},{10,10}}));
```

### 5.4a External API surface (`api_edits.rs`)

Each `ModelicaOp` variant has a Reflect-registered command wrapper so
external callers (HTTP / MCP / agent SDK) hit the same code path as the
GUI panels (per AGENTS.md §4.1):

| API command | Wraps | Purpose |
|-------------|-------|---------|
| `SetDocumentSource { doc, source }` | `ReplaceSource` | Whole-buffer rewrite — agent batch edits, lint apply, source import |
| `AddModelicaComponent { doc, class, type_name, name, x, y, w, h }` | `AddComponent` | Drop a component into a class with a placement |
| `RemoveModelicaComponent { doc, class, name }` | `RemoveComponent` | Delete a component declaration |
| `ConnectComponents { doc, class, from, to }` | `AddConnection` | Add a `connect(a.p, b.q);` equation; `from`/`to` are dot-paths |
| `DisconnectComponents { doc, class, from, to }` | `RemoveConnection` | Drop the matching connect equation |
| `ApplyModelicaOps { doc, ops: Vec<ApiOp> }` | All structural variants | Batch fan-out: `AddComponent / RemoveComponent / AddConnection / RemoveConnection / SetPlacement / SetParameter` in order |
| `RenameModelicaClass { doc, old_name, new_name }` | string-level rewrite | Rename a top-level class declaration + its `end OLD;` closer; if the doc origin is `Untitled`, the origin name is updated too so the tab title follows |

`ApplyModelicaOps` is the primary path for agent / canvas drag-drop —
each op in the batch becomes its own undoable step, but the caller
gets a single round-trip and a guaranteed ordering. The single-op
wrappers exist because hand-written agent code reads cleaner with
named commands than with a flat `ApiOp::AddComponent { ... }` payload.

API edits backdate the AST debounce timer (`waive_ast_debounce`), so
the canvas + text editor refresh inside the same frame instead of
waiting out the keystroke debounce window — see § 5.7.

**Gaps as of Phase α** — typed wrappers we don't expose yet, available
via `ApplyModelicaOps` or planned as standalones:

- `SetPlacement` / `SetParameter` — only via `ApplyModelicaOps`
- `EditText { range, replacement }` — no API surface (use
  `SetDocumentSource` for now); needed for granular LSP-style edits
- `RenameModelicaComponent`, `RenameModelicaPort` — not implemented
- `AddClass` / `RemoveClass` / `MoveClass` (between packages) — not
  implemented; `RenameModelicaClass` covers in-place rename only
- `SetClassAnnotation` (Icon, Diagram graphics) — not implemented;
  whole-source replace is the workaround
- `AddImport`, `AddExtends`, `SetDocumentation` — not implemented

### 5.5 Structured change events

After every successful mutation the document pushes a
`ModelicaChange` onto a bounded ring buffer
(`CHANGE_HISTORY_CAPACITY = 256`). Consumers poll via
`doc.changes_since(last_seen_generation)`:

```rust
pub enum ModelicaChange {
    TextReplaced,               // text-level ops + undo/redo
    ComponentAdded   { class, name },
    ComponentRemoved { class, name },
    ConnectionAdded  { class, from: PortRef, to: PortRef },
    ConnectionRemoved{ class, from: PortRef, to: PortRef },
    PlacementChanged { class, component, placement },
    ParameterChanged { class, component, param, value },
}
```

`changes_since` returns `None` when the consumer has fallen further
behind than the retention window — caller must then do a full rebuild
from the current AST. Panels (diagram, inspector) use this to patch
their render state incrementally rather than rebuild on every frame.

### 5.6 Type resolution (MLS §5.3)

Modelica's type lookup follows the rules in
[Modelica Language Spec §5.3 — Static Name Lookup](https://specification.modelica.org/maint/3.7/class-predefined-types-and-declarations.html#static-name-lookup).
Our implementation (used by the diagram panel's AST→canvas rebuild and
by the class resolver on AST-level ops) follows a subset of those
rules:

1. **Fully-qualified path** — a reference containing `.` (e.g.
   `Modelica.Blocks.Continuous.Integrator`) is treated as absolute
   and matched directly against the MSL index by path.
2. **Simple name with import** — a reference without `.` is resolved
   against the containing class's `import` declarations:
   - `import A.B.C;` → `C` → `A.B.C`
   - `import D = A.B.C;` → `D` → `A.B.C`
   - `import A.B.{C, D};` → `C` → `A.B.C`, `D` → `A.B.D`
3. **Unresolved** → the reference is surfaced as a skipped component
   (non-fatal in the diagram rebuild; would be a compile error in
   rumoca's type checker).

**Deliberately not implemented (yet):**

- `import A.B.*;` (unqualified) expansion — needs an MSL index walk
  for every `A.B` child
- Enclosing-class scope lookup (MLS §5.3.1 step 2) — flat top-level
  package is the only source of types today
- Short-name-tail heuristics (e.g. match `Integrator` to the first
  MSL entry whose path ends in `.Integrator`) — rejected as unsafe;
  multiple MSL classes share short names across branches, so a
  suffix match picks whichever loaded first. Not what the Modelica
  spec means by name resolution.

See
[`../../crates/lunco-modelica/src/ui/panels/canvas_projection.rs`](../../crates/lunco-modelica/src/ui/panels/canvas_projection.rs)
(`import_model_to_diagram`) for the call site, and
[`../../crates/lunco-modelica/src/document/core.rs`](../../crates/lunco-modelica/src/document/core.rs)
(`resolve_class`) for the class-path resolver used by AST ops.

## 6. The `output` convention (rumoca workaround)

**Critical.** Every variable in a Modelica model that needs to be
observable by co-simulation must have explicit `input` or `output`
causality. Bare `Real` declarations are eliminated by rumoca's DAE
preparation and disappear from the solver.

```modelica
model Balloon
  parameter Real mass = 4.5;

  input Real height = 0;            // wired in from Avian
  input Real velocity = 0;

  Real volume(start = 4.0);         // state — always kept

  output Real temperature;          // MUST be output or it vanishes
  output Real airDensity;
  output Real buoyancy;
  output Real drag;
  output Real netForce;
equation
  // ...
end Balloon;
```

Full rationale, including planned upstream fixes to the rumoca fork, in
[`../../crates/lunco-cosim/README.md#modelica-model-convention`](../../crates/lunco-cosim/README.md).

## 7. Type vs. instance distinction

Every `.mo` file defines a **Modelica type** (`model Balloon`, `model SolarPanel`).
A `ModelicaModel` component on a Bevy entity is an **instance** of that type.

| Concept | Lives in | Analogy |
|---------|----------|---------|
| Type (definition) | `models/Balloon.mo` — single file, one per model | Rust `struct` declaration |
| Instance (running) | Bevy entity with `ModelicaModel { model_name: "Balloon", ... }` | Rust instance of that struct |

- Editing the type (changing `Balloon.mo`) affects **all** instances on
  the next recompile.
- Editing an instance (changing one balloon's `R=100` → `R=200`) affects
  **only** that instance — stored on the instance's
  `ModelicaModel.parameters`.

Dymola has the same distinction. Our UI should keep it visible — a
parameter edit in the Inspector is instance-level unless the user
explicitly promotes it ("save as default in model").

## 7a. Document identity — three tiers, one truth

A Modelica document has three identity caches that historically drift:

| Tier | Source of truth | Used by |
|------|-----------------|---------|
| **File** | `Document::origin` (`Untitled{name}` or `File{path, writable}`) | Save logic, dirty check, Files browser |
| **Workspace entry** | `WorkspaceResource.DocumentEntry.title` | Tab label, Recents |
| **Modelica class** | AST top-level class name (in source) | Compile, references, drill-in, Class browser, journal entries |

**Modelica's class-first identity is authoritative** — same as Dymola
and OMEdit, where the class name is what you see in tabs and the
file is filesystem implementation detail. Workbench follows this
convention with VS Code's untitled handling for the unsaved case.

### 7a.1 Title derivation

`DocumentEntry.title` is **derived**, not stored. A `Last`-schedule
system recomputes it whenever `(origin, ast_first_class,
drilled_in_class, dirty)` changes:

```
title = derive(origin, ast_first_class, drilled_in_class, dirty)

  primary  = ast_first_class            (Modelica name lookup wins)
           | drilled_in_class           (multi-class file → focused class)
           | origin.display_name()      (parse failed → fall back)
           | "Untitled-N"               (no source yet)

  prefix   = drilled_pkg.drilled_class  (if drilled into a sub-class)

  postfix  = "●"                        (dirty)

  style    = italic                     (origin.is_untitled || dirty)
```

Multi-class file shows `Pkg.Active` on the tab plus a
breadcrumb above the active editor (Dymola-style: `Pkg ▸ Active`,
each segment clickable to drill out). Switching drilled class flips
the tab label without remounting the document.

### 7a.2 Rename behaviour

`RenameModelicaClass`:
1. Rewrites the source declaration + `end OLD;` closer (string-level,
   first match only — multi-class files require explicit
   class-targeted renames in v1).
2. If the doc origin is `Untitled`, updates `origin.name` to match.
   Title derivation picks up the new class name automatically next
   frame.
3. **If the doc is File-backed**, behaviour is governed by the
   `modelica.naming.rename_class_renames_file` setting:
   - `Always` — rename the `.mo` file in lock-step (Dymola default).
   - `Ask` — prompt the user; default for the workbench.
   - `Never` — file stays at its old path; class name and filename
     diverge until the user does a Save-As (VS Code default).

The setting lives in the `modelica.naming` section of
`settings.json` (see `11-workbench.md` § 9b.2). Per-twin overrides
let library projects pin `Always` while sandbox twins keep `Never`.

### 7a.3 Save-As default

When an Untitled doc transitions to File via Save-As, the suggested
filename is `<ast_first_class>.mo` in the active Twin's models
folder, governed by `modelica.naming.save_as_default_uses_class_name`
(default `true`). Users can override the suggested name; the setting
just controls the default.

### 7a.4 Implementation notes

- `WorkspaceResource.DocumentEntry.title` becomes a derived field —
  the system that maintains it reads from
  `(ModelicaDocumentRegistry, drilled_in_class, dirty)` and writes
  the entry. No call site sets `entry.title` directly any more; that
  was the source of the drift.
- The italic + dirty-dot styling is handled by the tab renderer
  (`lunco-workbench`) reading `UiSettings + DocumentEntry.origin +
  dirty`. No per-domain logic in the renderer.
- `RenameModelicaClass` no longer needs to touch
  `WorkspaceResource` directly — the title-derive system picks up
  the AST change. (Today's implementation does write the entry
  manually as a compatibility shim; remove once the derive system
  lands.)

## 8. New-model workflow (target)

1. **File → New Modelica Model** (or Ctrl+Shift+N in Analyze workspace)
2. Dialog: name, kind (`model | block | connector | package`), template
   (empty, from MSL, copy existing), location (project `models/` folder
   by default)
3. New `ModelicaDocument` created in memory, opens in Analyze workspace
   with empty Diagram + skeleton source
4. User edits via Diagram (drag MSL components, connect ports) or Code
5. Each edit is a `ModelicaOp` applied to the document; views re-render
6. Ctrl+S saves the document as a `.mo` file; Library Browser refreshes
7. To use the model: drag it from Library onto viewport, or right-click
   an entity → "Attach Modelica model"

Today (pre-Document-System) the workflow is rougher:
- Code Editor and Diagram are disconnected (see § 11 Gaps)
- Parameter changes may not trigger recompile (fixed in
  `ModelicaInspectorPanel`; legacy `TelemetryPanel` still has the bug)

## 9. The Modelica diagram editor

The diagram panel (`lunco-modelica/src/ui/panels/canvas_diagram.rs`)
renders on top of `lunco-canvas` — the workbench's own canvas
substrate. The panel is a thin *view* over a `ModelicaDocument`: the
document is the authoritative state, the canvas scene is a rendered
projection.

```
         ModelicaDocument (source + cached AST)
                   ▲             │
         AST ops   │             │  changes_since(gen)
  (drag, connect,  │             │  (TextReplaced,
   delete, move,   │             │   ComponentAdded, …)
   paramedit)      │             ▼
                  CanvasDiagramPanel ◀──── canvas_projection
                   │
                   ▼
                  lunco-canvas Scene  (renders nodes / wires;
                                       owns pan/zoom/selection)
```

### 9.1 Sync flow

Each frame:

1. **Open-model rebind** — if `WorkbenchState.open_model.doc` changed,
   the panel resets the change-stream cursor so the next sync does a
   clean rebuild.
2. **Document → scene projection** — if `doc.generation() !=
   last_seen_gen`, re-parse the source and rebuild the canvas scene
   (synchronous — parse of a typical Modelica model is
   sub-millisecond).
3. **Canvas render** — user interaction happens in `lunco-canvas`; it
   owns pan/zoom/selection/drag state between frames.
4. **User action → op emission** —
   - Right-click Add Component → `AddComponent`
   - Right-click Delete → `RemoveComponent`
   - Wire draw/disconnect → frame-to-frame wire-set diff →
     `AddConnection` / `RemoveConnection`
   - Drag-to-move → frame-to-frame position diff → `SetPlacement`
5. **Apply + echo suppression** — pending ops are applied to the
   `DocumentHost`, `last_seen_gen` is advanced past our own
   generations so step 2 doesn't rebuild in response to edits we
   just made.

Text edits in the Code Editor flow through the same pipe: the
editor's debounced commit (≈ 350 ms idle or focus-loss) calls
`checkpoint_source` → `ReplaceSource`, the generation bumps past
`last_seen_gen`, and the diagram rebuilds on its next frame.

### 9.2 Visual details

- MSL palette on the left (right-click menu adds components)
- Custom component body rendering — zigzag for resistor, parallel
  plates for capacitor, blue circles for electrical pins
- Small port dots rather than labeled rectangles
- Dot-grid background
- Borderless node frames to reduce chrome

### 9.3 Why our own canvas

The diagram panel originally rode on `egui-snarl` (see git history
prior to the canvas migration). `lunco-canvas` replaced it because we
needed:

- **Ports on every side** — schematic-style placement (top, bottom,
  left, right), not just left/right inputs/outputs.
- **Acausal connectors** — Modelica electrical / fluid ports are
  bidirectional; node-graph libraries built around `OutPin → InPin`
  edge direction don't fit.
- **Animation hooks** (see § 9c) — render-side tweens, glow,
  per-origin policies need access to the draw loop, which a
  third-party node-graph crate doesn't expose.
- **Grid snapping, custom shapes, multi-domain reuse** —
  `lunco-canvas` is the substrate for non-Modelica diagrams too
  (mission planner, cosim graphs).

Alternatives we evaluated and rejected for the workbench-wide
canvas substrate:

| Alternative | Verdict |
|-------------|---------|
| `egui_node_graph` | Node-graph oriented, not schematic |
| `egui_node_editor` | Less documented |
| `egui_graph` | New, less battle-tested |
| `egui-snarl` | What we started on; lacks port-side and acausal wires; no animation hook surface |
| Forking `egui-snarl` | ~50–200 LOC patch but ties us to upstream forever |
| **Custom `lunco-canvas`** | Picked — owned, extensible, animation-ready, no upstream coupling |

## 9c. Canvas animation + multi-user roadmap

The Modelica canvas is a Miro-style diagram surface — components,
connections, free placement. This section captures how we want it to
*feel* (animated, alive) and how that scales to multi-user later. The
guiding principle is Figma's: **animate the change, not the state.**

### 9c.1 Op origin tag

Every structural mutation funnels through `apply_ops_public` and
arrives carrying an origin:

```rust
pub enum OpOrigin {
    /// Mouse drag, keyboard, paste — user already saw the action,
    /// no animation needed.
    Local,
    /// API / agent / test script — animate so the viewer can follow
    /// what's happening.
    Api,
    /// Future: incoming op from a collaborator. Animated, with that
    /// user's color.
    Remote { user_id: UserId },
}
```

Origin is threaded through `apply_ops_public(world, doc, ops, origin)`
and recorded alongside the op in a `RecentChanges` ring buffer:

```rust
#[derive(Resource, Default)]
struct RecentChanges {
    entries: VecDeque<RecentChange>,  // bounded ~256
}
struct RecentChange {
    doc:    DocumentId,
    op:     ModelicaChange,            // structural summary
    origin: OpOrigin,
    at:     Instant,
}
```

Render-side systems read `RecentChanges` to decide what to animate.
The journal subsystem already records the same op surface — origin is
an extra annotation, not a separate event channel.

### 9c.2 Tween primitive

Animation is **render-only**. The source AST `Placement` is the truth
the moment the op applies; the renderer interpolates between *previous
rendered position* and *new placement* over a short window. Source
mutation already happens in one frame — animating the source itself
would corrupt undo, journal, and AST refresh.

```rust
#[derive(Component)]
struct CanvasTween {
    from:     Placement,        // last-rendered pre-op
    to:       Placement,        // post-op (matches AST)
    start:    Instant,
    duration: Duration,         // 0 ⇒ skip / instant
    ease:     EaseKind,         // EaseOutCubic | Spring | Linear
}
```

The render system reads `lerp(from, to, ease(t))` instead of the raw
placement when a tween is active; despawns the tween at `t ≥ 1`.

### 9c.3 Per-origin animation policy

| Origin | Tween | Pulse | Camera focus |
|---|---|---|---|
| `Local` | 0 ms (instant) | none | none — user has the cursor |
| `Api` | `tween_ms` (default 250) | `pulse_ms` (default 1000) | per `add.focus_behavior` |
| `Remote { user }` | same as Api but pulse colored from `user`'s presence color | yes | optional ("follow user") |

The user can override per-call: `AddModelicaComponent { animate: false }`
forces an instant local-style apply even for an API caller, and
`ApplyModelicaOps { animate: true }` forces animation for what would
otherwise be a `Local` mouse-drag batch (e.g. an "import diagram"
scripted action).

### 9c.4 Pulse glow

Figma-style outer glow on newly-added components — a soft ring
around the node that fades to transparent over `pulse_ms`. Implemented
as a transient `PulseGlow { until: Instant, color: Color32 }`
component on the canvas node; renderer adds the glow at draw time and
the system despawns the component when `now > until`.

Color: theme-driven for `Api` origin, user-presence-color for
`Remote`. Pulse style is fixed (outer glow) but `pulse_ms` is
settings-driven, so users can tune intensity / duration or disable
(0 ms).

### 9c.5 Batch focus debounce

A scripted `AddComponent × N` (the rocket-build flow) shouldn't
ping-pong the camera. Strategy:

1. Each `AddComponent` with `Api` origin schedules a single-component
   `Center` focus.
2. If another `AddComponent` arrives within
   `add.batch_debounce_ms` (default 200), cancel the per-component
   focus.
3. After `batch_debounce_ms` of idle, fire one `FitVisible` over the
   accumulated set.

So a 10-component build animates each spawn (with pulse) but only
runs one camera move at the end — frames the whole diagram for the
viewer.

### 9c.6 Camera tween

The per-component `Center` and end-of-batch `FitVisible` use a
camera tween that interpolates `(pan, zoom)` toward the target over
`tween_ms`. Same ease curve as node tweens for consistency. The
existing `SetZoom` / `PanCanvas` commands set the camera directly;
the animation system layers a smooth-pan above them so manual
`SetZoom` from a script also has the option to animate.

### 9c.7 Settings tree

Following the `11-workbench.md` § 9b multi-level convention. Defaults
shown.

```
modelica.canvas.animation.tween_ms          u32   250
modelica.canvas.animation.ease              enum  ease_out_cubic | spring | linear   default ease_out_cubic
modelica.canvas.animation.pulse_ms          u32   1000
modelica.canvas.animation.local_origin      enum  Instant | Animated  default Instant   (you already see it)
modelica.canvas.animation.api_origin        enum  Instant | Animated  default Animated  (script readability)
modelica.canvas.animation.remote_origin     enum  Instant | Animated  default Animated  (future, multi-user)

modelica.canvas.add.focus_behavior          enum  None | Center | FitVisible   default Center
modelica.canvas.add.batch_debounce_ms       u32   200

ui.reduce_motion                            bool  false
                                            (when true, all tween_ms → 0; honours OS prefers-reduced-motion)
```

`ui.reduce_motion` is the global override — accessibility +
matches macOS/iOS conventions. Mirror it from the OS preference at
startup, allow user override in the Settings panel.

Per-call API override: every structural-edit command (`AddComponent`,
`ConnectComponents`, `ApplyModelicaOps`) gains an optional `animate:
Option<bool>` field. `None` ⇒ read from settings; `Some(true)` /
`Some(false)` ⇒ override.

### 9c.8 Presence (deferred — multi-user precursor)

Pre-CRDT, but useful even single-user (paired with an agent
co-pilot):

```rust
struct CanvasPresence {
    user_id: UserId,
    color:   Color32,                   // stable hash of user_id
    cursor:  Option<CanvasPos>,
    selection: HashSet<ComponentName>,
    drilled_in_class: Option<String>,   // they're focused on Pkg.Sub
}
```

Broadcast over a presence channel separate from doc state — matches
Yjs's `awareness` separation. Each remote presence renders as a
colored cursor + ghost-selection rectangle. Stale entries decay
after 30 s without ping. `OpOrigin::Remote` reuses the same color.

Settings:

```
modelica.canvas.collab.show_remote_cursors   bool   true
modelica.canvas.collab.show_remote_selection bool   true
modelica.canvas.collab.user_color            "auto" | "#RRGGBB"   "auto"
modelica.canvas.collab.follow_user           Option<UserId>       (camera follows that user)
```

Status: **deferred**. Lives in this spec for shape; not yet built.

### 9c.9 CRDT-backed source (deferred — full multi-user)

Two approaches were evaluated:

**(a) Text CRDT on the `.mo` source (Yjs `Y.Text` / `yrs`)**
- Pro: works for any future text-shaped doc kind with no domain
  changes.
- Con: structural ops (`AddComponent`, `ConnectComponents`) become
  bursts of character inserts on the wire; the journal loses its
  "Alice added a Pump" granularity unless we re-derive from a diff.

**(b) Structural CRDT over the AST (preferred)**
- Each top-level class is a `Y.Map`. Components a `Y.Map<name,
  ComponentDecl>`. Connections a `Y.Array<ConnectEq>`. Annotations
  stay text-CRDT'd internally.
- Render → text via the existing pretty-printer; persist that text
  on disk so non-collaborating users still get readable `.mo`.
- Pro: structural ops stay structural across the wire. Remote
  `AddComponent` becomes a single "Alice added Pump" event with the
  same animation path as `OpOrigin::Api`.
- Con: more upfront work, but aligns with the existing `ModelicaOp`
  vocabulary.

**Decision: (b)** when the work lands. `OpOrigin::Remote` plugs in
directly — same animation code, different origin tag. The journal
becomes a shared event log: each user's edits flow into a single
ordered history (Lamport timestamps), which dovetails with the
Twin-journal subsystem in `13-twin-and-workflow.md` § 5a and the
SysML v2 REST API path in that same doc.

**Library choice (deferred):** `yrs` (Rust port of Yjs, same wire
format) — picking it later means a future web-collab room "just
works" against a JS Yjs server. Automerge is the alternative but
slower in our shape.

**Server (deferred):** WebSocket relay snapshotting to the Twin's
`.lunco/journal/` is the leading option; simpler to persist than
WebRTC P2P and reuses the journal store. Spec lives in the
twin-journal doc; not in scope here.

### 9c.10 Implementation order

1. **Layer 0 — Tween primitive.** `CanvasTween` component, render
   interpolation. Single-user, no behavioural change for `Local`
   origin yet.
2. **Layer 1 — Op origin tag.** Thread `OpOrigin` through
   `apply_ops_public`, populate `RecentChanges`.
3. **Layer 2 — Pulse + auto-focus.** `PulseGlow` component, camera
   tween, batch debounce. This is the demo-worthy quick win — turns
   a scripted rocket build into something visually beautiful.
4. **Layer 3 — Presence.** Cursors + selections over a websocket
   channel.
5. **Layer 4 — CRDT.** `yrs`-backed structural CRDT, journal merge.

Layers 0–2 are days; 3 is days; 4 is weeks and warrants its own
sprint with the journal subsystem.

## 10. Panels (current + planned)

| Panel | Current | Notes |
|-------|---------|-------|
| **Diagram** | ✅ Working, generic rectangles, Dymola-style shapes in progress | `canvas_diagram.rs`, on `lunco-canvas` |
| **Code Editor** | ✅ Working | 423 LOC, plain egui TextEdit |
| **MSL Palette** | ✅ Working | ~20 MSL components |
| **Library Browser** | ✅ Working | File tree of `.mo` files |
| **Package Browser** | ✅ Working | MSL package hierarchy |
| **Telemetry / Parameters** | ⚠️ Has parameter-update bug (see gaps) | Legacy; being replaced by `ModelicaInspectorPanel` |
| **`ModelicaInspectorPanel`** | ✅ New, compact, context-aware | Fixes the parameter-update bug |
| **Graphs** | ✅ Working | Time-series via `egui_plot` |

## 11. Current gaps

The following issues are tracked as implementation work, not architectural
decisions:

### P0 — Blocking

**Parameter changes not propagated** (legacy `TelemetryPanel`): drag
a parameter value → the UI updates the hashmap but doesn't send
`ModelicaCommand::UpdateParameters` to the worker. Simulation keeps using
the old value. Fixed in new `ModelicaInspectorPanel`; legacy panel to be
retired.

~~**Diagram ↔ Code disconnect**~~ — **resolved in Phase α**. The
Diagram and Code editor now share a single `ModelicaDocument`. Edits
in either panel flow through the document and update the other on
the next frame. Opening a file from the Library Browser populates
both views from the same source. See § 5 and § 9 above.

**Acausal connector visual** (in progress on `lunco-canvas`):
Modelica electrical / fluid connectors are acausal — wires shouldn't
have an arrow direction. The migration off egui-snarl unblocks this;
the rendering work to drop the directional arrow on connector wires
is tracked separately on the canvas crate.

### P1 — Degrading workflow

- **No icon annotation rendering** for MSL components beyond hardcoded
  shapes. Plan: hardcode shapes for common types first; parse annotations
  later.
- **No initial conditions in VisualDiagram** — `ParamDef` only stores a
  single value, not `start`, `fixed`, `min`, `max`.
- **No Modelica class hierarchy** in the visual editor — only flat models.
  OK for Phase 1; subsystems/packages are Phase 3+.
- **No simulation configuration UI** — hardcoded solver + tolerances.
- **Orthogonal wire routing** — current bezier wires work, Dymola-style
  orthogonal paths are "nice to have."

### P2 / P3 — Polish

- Undo/redo for diagram ops (comes free from Document System op inverses)
- Component search / filter in palette
- Right-click context menu on components
- Pre-compile validation (unconnected ports, missing ground, cycles)
- Editable model name (currently auto-generated `VisualModel1`, `VisualModel2`)

## 12. Dymola-workflow parity

LunCoSim's Modelica workspace aims at Dymola-familiarity but isn't 1:1.
Feature parity snapshot:

| Feature | Dymola | LunCoSim |
|---------|--------|----------|
| Package browser | ✅ | ✅ |
| Library browser | ✅ | ✅ |
| Diagram canvas | ✅ custom icons | ✅ generic rects (Dymola-style in progress) |
| Text view | ✅ | ✅ |
| Parameter dialog | ✅ | ⚠️ partial (P0 bug in legacy panel) |
| Plot variables | ✅ | ✅ (`egui_plot`) |
| Variables browser | ✅ | ✅ |
| Compilation pipeline | ✅ | ✅ (rumoca) |
| Simulation setup dialog | ✅ | ❌ (continuous stepping instead) |
| Live-parameter editing during sim | ❌ | ✅ (LunCoSim advantage) |
| Icon designer | ✅ | ❌ |
| Documentation view | ✅ | ❌ |
| Animation view | ✅ | ✅✅ (3D world IS this — LunCoSim advantage) |

Rough **80 %** feature parity on the core loop. The gaps are solvable
within 2–3 months of focused work; the biggest wins come from the
Document System migration (unlocks live diagram↔code sync) and
finishing the acausal-connector visuals on `lunco-canvas`.

## 13. See also

### Source

- [`../../crates/lunco-modelica/`](../../crates/lunco-modelica/) — crate root
- [`../../crates/lunco-modelica/src/document/core.rs`](../../crates/lunco-modelica/src/document/core.rs) — `ModelicaDocument`, op set, apply pipeline, span-based patch helpers, qualified-path `resolve_class`
- [`../../crates/lunco-modelica/src/pretty.rs`](../../crates/lunco-modelica/src/pretty.rs) — subset pretty-printer, `PrettyOptions`
- [`../../crates/lunco-modelica/src/ui/panels/canvas_projection.rs`](../../crates/lunco-modelica/src/ui/panels/canvas_projection.rs) — diagram panel, sync-from-document, wire/position diffing, scope-aware type lookup
- [`../../crates/lunco-modelica/src/ui/panels/code_editor.rs`](../../crates/lunco-modelica/src/ui/panels/code_editor.rs) — code editor, debounced commit (`EDIT_DEBOUNCE_SEC`), word-wrap toggle

### Adjacent docs

- [`../../crates/lunco-cosim/README.md`](../../crates/lunco-cosim/README.md) — cosim-level engineering docs
- [`10-document-system.md`](10-document-system.md) — shared Document System foundation
- [`13-twin-and-workflow.md`](13-twin-and-workflow.md) — how Modelica files live inside a Twin
- [`14-simulation-layers.md`](14-simulation-layers.md) — Twin / Scenario / Run / Model lifecycle the Modelica stepper participates in
- [`15-adaptive-fidelity.md`](15-adaptive-fidelity.md) — multi-clock + LoD (future)
- [`22-domain-cosim.md`](22-domain-cosim.md) — cosim pipeline ordering
- [`23-domain-environment.md`](23-domain-environment.md) — how environment (gravity, atmosphere) flows into Modelica inputs
- [`24-domain-sysml.md`](24-domain-sysml.md) — SysML as the structural peer; references Modelica models as realizations
- `specs/014-modelica-simulation` — detailed spec

### External references

- [Modelica Language Specification §5.3 — Static Name Lookup](https://specification.modelica.org/maint/3.7/class-predefined-types-and-declarations.html#static-name-lookup) — the scope/import resolution rules our type lookup follows
- [Modelica Language Specification §18 — Annotations](https://specification.modelica.org/maint/3.7/annotations.html) — `Placement`, `Line`, `Icon` annotation shapes our pretty-printer emits
- [rumoca on GitHub](https://github.com/LunCoSim/rumoca) — the parser + runtime crate family
