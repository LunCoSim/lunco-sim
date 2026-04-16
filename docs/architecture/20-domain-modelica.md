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

## 5. Document System integration (planned)

The current `ModelicaModel` component is an *ad-hoc* hybrid of document
state and runtime state. Under the Document System design
([`10-document-system.md`](10-document-system.md)), this splits:

```rust
// Tier 1: canonical document (future)
pub struct ModelicaDocument {
    ast: ModelicaAst,                 // via rumoca-parser
    generation: u64,
}

// Tier 2: runtime handle
#[derive(Component)]
pub struct ModelicaModel {
    pub document_id: DocumentId,      // points at the ModelicaDocument
    pub session_id: u64,
    pub paused: bool,
    pub is_stepping: bool,
    pub parameters: HashMap<String, f64>,  // per-instance overrides
    pub inputs: HashMap<String, f64>,
    pub variables: HashMap<String, f64>,
}

// Tier 1: typed ops
pub enum ModelicaOp {
    AddComponent   { name, type_name, pos },
    RemoveComponent{ name },
    AddConnection  { from: (String,String), to: (String,String) },
    RemoveConnection{ from, to },
    SetParameter   { component, param, value },
    RenameComponent{ old, new },
    MoveComponent  { name, pos },
    SetIcon        { name, icon_spec },
    SetModelKind   { kind },
    AddImport      { package },
}
```

Every editing action in the Diagram, Code Editor, or Parameter Inspector
produces a `ModelicaOp` that applies to the `ModelicaDocument`. All views
re-render from the same AST — diagram and code stay in sync automatically.

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

The diagram panel (`lunco-modelica/src/ui/panels/diagram.rs`, ~1700 lines)
is an **egui-snarl-based** visual editor for Modelica models. Key features:

- MSL palette on the left (drag components onto canvas)
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

**Diagram ↔ Code disconnect**: the Diagram and Code editor don't observe
the same source. Edits in one don't propagate to the other; opening
`Battery.mo` from the library browser shows it in the Code editor but NOT
the Diagram. Fixed by the Document System migration
([`10-document-system.md`](10-document-system.md)) — both panels become
`DocumentView<ModelicaDocument>`.

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

- [`../../crates/lunco-modelica/`](../../crates/lunco-modelica/) — source
- [`../../crates/lunco-cosim/README.md`](../../crates/lunco-cosim/README.md) — cosim-level engineering docs
- [`10-document-system.md`](10-document-system.md) — the data model Modelica will migrate into
- [`13-twin-and-workflow.md`](13-twin-and-workflow.md) — how Modelica files live inside a Twin
- [`22-domain-cosim.md`](22-domain-cosim.md) — cosim pipeline ordering
- [`23-domain-environment.md`](23-domain-environment.md) — how environment (gravity, atmosphere) flows into Modelica inputs
- [`24-domain-sysml.md`](24-domain-sysml.md) — SysML as the structural peer; references Modelica models as realizations
- `specs/014-modelica-simulation` — detailed spec
