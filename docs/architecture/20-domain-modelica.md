# 20 — Modelica Domain

> Behavioral modeling using Modelica + the rumoca runtime. Wired into
> Bevy ECS as a `ModelicaDocument` per model, stepped in `FixedUpdate` on
> a background worker thread.
>
> Engineering docs live in
> [`../../crates/lunco-modelica/`](../../crates/lunco-modelica/) and
> [`../../crates/lunco-cosim/README.md`](../../crates/lunco-cosim/README.md).

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
    - DiagramPanel (egui-snarl)
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
Our implementation (used by the diagram panel's AST→snarl rebuild and
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
[`../../crates/lunco-modelica/src/ui/panels/diagram.rs`](../../crates/lunco-modelica/src/ui/panels/diagram.rs)
(`import_model_to_diagram`) for the call site, and
[`../../crates/lunco-modelica/src/document.rs`](../../crates/lunco-modelica/src/document.rs)
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

The diagram panel (`lunco-modelica/src/ui/panels/diagram.rs`) is an
**egui-snarl-based** visual editor. Under Phase α the panel is a thin
*view* over a `ModelicaDocument` — the document is the authoritative
state, snarl is a rendered projection.

```
         ModelicaDocument (source + cached AST)
                   ▲             │
         AST ops   │             │  changes_since(gen)
  (drag, connect,  │             │  (TextReplaced,
   delete, move,   │             │   ComponentAdded, …)
   paramedit)      │             ▼
                  DiagramPanel ◀──── sync_from_document()
                   │
                   ▼
                  Snarl<DiagramNode>  (rendered, snarl owns pan/zoom/selection)
```

### 9.1 Sync flow

Each frame:

1. **Open-model rebind** — if `WorkbenchState.open_model.doc` changed,
   `DiagramState::bind_document` resets the change-stream cursor so
   the next sync does a clean rebuild.
2. **Document → snarl sync** — if `doc.generation() != last_seen_gen`,
   re-parse the source and rebuild snarl (synchronous — parse of a
   typical Modelica model is sub-millisecond). No more async
   `AsyncComputeTaskPool` / "Analyzing…" spinner.
3. **Snarl render** — user interaction happens in snarl; it owns
   pan/zoom/selection/drag state between frames.
4. **User action → op emission** —
   - Right-click Add Component → `AddComponent`
   - Right-click Delete → `RemoveComponent`
   - Wire draw/disconnect → detected via frame-to-frame wire-set
     diff → `AddConnection` / `RemoveConnection`
   - Drag-to-move → detected via frame-to-frame position diff →
     `SetPlacement`
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
- Custom component body rendering in `show_body()` — zigzag for resistor,
  parallel plates for capacitor, blue circles for electrical pins
- Small port dots rather than labeled rectangles
- Dot-grid background
- Borderless node frames to reduce chrome

### egui-snarl pros and cons

**What snarl gives us for free:**
- Pan / zoom via `egui::Scene`
- Bezier wires between pins
- Wire hit detection and drag
- Node selection, drag, resize, multi-select
- Right-click context menus

**What snarl cannot do without a fork:**

| Limitation | Impact |
|-----------|--------|
| Pins forced to node edges (left/right only) | Can't place ports on Top/Bottom |
| Strict `OutPin → InPin` wires | Acausal connectors (Modelica electrical) need bidirectional |
| No grid snapping | Minor UX issue |
| Node shapes are always rectangular | Can draw inside the body, but outer hit-area is a rect |

The acausal-connector limitation is the main one. Current workaround:
treat every port as both input and output. Real fix: fork egui-snarl to
add bidirectional pins. Tracked as Phase 2 work.

### What we evaluated and rejected

| Alternative | Verdict |
|-------------|---------|
| `egui_node_graph` | Not suitable — still node-graph oriented, not schematic |
| `egui_node_editor` | Less documented, smaller community |
| `egui_graph` | Too new, less battle-tested |
| Custom `egui::Scene` implementation | ~2000 LOC to rebuild from scratch |
| **Fork egui-snarl** | Best option (~50–200 LOC patch for pin sides + acausal) |

## 10. Panels (current + planned)

| Panel | Current | Notes |
|-------|---------|-------|
| **Diagram** | ✅ Working, generic rectangles, Dymola-style shapes in progress | 1701 LOC, egui-snarl |
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

**Diagram edges are directional** (acausal broken): egui-snarl enforces
`OutPin → InPin`. Modelica electrical connectors are acausal. Current
workaround — every port is both input and output — is confusing. Needs
egui-snarl fork.

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
Document System migration (unlocks live diagram↔code sync) and egui-snarl
fork (unlocks acausal connectors).

## 13. See also

### Source

- [`../../crates/lunco-modelica/`](../../crates/lunco-modelica/) — crate root
- [`../../crates/lunco-modelica/src/document.rs`](../../crates/lunco-modelica/src/document.rs) — `ModelicaDocument`, op set, apply pipeline, span-based patch helpers, qualified-path `resolve_class`
- [`../../crates/lunco-modelica/src/pretty.rs`](../../crates/lunco-modelica/src/pretty.rs) — subset pretty-printer, `PrettyOptions`
- [`../../crates/lunco-modelica/src/ui/panels/diagram.rs`](../../crates/lunco-modelica/src/ui/panels/diagram.rs) — diagram panel, sync-from-document, wire/position diffing, scope-aware type lookup
- [`../../crates/lunco-modelica/src/ui/panels/code_editor.rs`](../../crates/lunco-modelica/src/ui/panels/code_editor.rs) — code editor, debounced commit (`EDIT_DEBOUNCE_SEC`), word-wrap toggle

### Adjacent docs

- [`../../crates/lunco-cosim/README.md`](../../crates/lunco-cosim/README.md) — cosim-level engineering docs
- [`10-document-system.md`](10-document-system.md) — shared Document System foundation
- [`13-twin-and-workflow.md`](13-twin-and-workflow.md) — how Modelica files live inside a Twin
- [`22-domain-cosim.md`](22-domain-cosim.md) — cosim pipeline ordering
- [`23-domain-environment.md`](23-domain-environment.md) — how environment (gravity, atmosphere) flows into Modelica inputs
- [`24-domain-sysml.md`](24-domain-sysml.md) — SysML as the structural peer; references Modelica models as realizations
- `specs/014-modelica-simulation` — detailed spec

### External references

- [Modelica Language Specification §5.3 — Static Name Lookup](https://specification.modelica.org/maint/3.7/class-predefined-types-and-declarations.html#static-name-lookup) — the scope/import resolution rules our type lookup follows
- [Modelica Language Specification §18 — Annotations](https://specification.modelica.org/maint/3.7/annotations.html) — `Placement`, `Line`, `Icon` annotation shapes our pretty-printer emits
- [rumoca on GitHub](https://github.com/LunCoSim/rumoca) — the parser + runtime crate family
