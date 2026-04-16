# 10 — Document System

> The foundational data model of LunCoSim. Establishes what a "Document"
> is, how views observe it, how edits flow, and how runtime ECS is derived.

## 1. Why this exists

LunCoSim is an engineering simulator users *edit, save, and collaborate on*.
The data the user creates and modifies — scenes, behavioral models, missions,
subsystem wiring — must persist across app restarts, version cleanly in git,
export to external tools, and eventually sync live between multiple users.

Treating the Bevy ECS world as the source of truth fails all of these
requirements. The ECS world is *transient by nature*: entities come and go
as scenes load, simulations run, bodies spawn, timelines advance. You can't
save an ECS world as a portable artifact. You can't diff two ECS worlds in
git. You can't export an ECS world to Dymola or Blender.

**Documents are the canonical, serialized, durable representation of user
intent.** The ECS world is a live projection of the documents plus
simulation state.

The Document System makes this distinction explicit and builds machinery
around it:

- **A `Document` trait** per domain (Modelica, USD, SysML, mission, ...)
- **Typed operations** that mutate documents in well-defined ways
- **Change notification** so views redraw on edit
- **Free undo/redo** from op inverses
- **Serialization** per-domain (`.mo`, `.usda`, etc.)
- **Future: collaborative sync** via Op transmission over CRDT/OT

## 2. Concepts

### Document

A Document is the canonical representation of a structured artifact in the
simulator. Each domain defines its own Document type.

Properties of a Document:

1. **Identified.** Every Document has an ID, typically the Bevy `Entity`
   that hosts it, or a stable UUID for entities that come and go.
2. **Structured.** Internal data is typed (not opaque bytes). The AST of a
   `.mo` file, the prim tree of a USD stage, etc.
3. **Mutable only via Ops.** All changes go through typed operations
   (see below). Direct field access is read-only from outside.
4. **Observable.** Views can subscribe to change events.
5. **Serializable.** Round-trips through a canonical on-disk format with
   bounded fidelity loss (round-trip is lossless for the in-scope data,
   may lose non-essential comments/whitespace).

### Operation (Op)

An Op is a typed, serializable mutation. Every Op has a defined inverse.

For a Modelica document:

```rust
enum ModelicaOp {
    AddComponent   { name: String, type_name: String, pos: Pos2 },
    RemoveComponent{ name: String },
    AddConnection  { from: (String, String), to: (String, String) },
    RemoveConnection{ from: (String, String), to: (String, String) },
    SetParameter   { component: String, param: String, value: f64 },
    RenameComponent{ old: String, new: String },
    MoveComponent  { name: String, pos: Pos2 },
    // ... plus ops for equations, annotations, etc.
}
```

For a USD document:

```rust
enum UsdOp {
    AddPrim    { path: SdfPath, type_name: TfToken },
    RemovePrim { path: SdfPath },
    SetAttribute { path: SdfPath, attr: TfToken, value: VtValue },
    SetTransform { path: SdfPath, xform: GfMatrix4d },
    // ...
}
```

Ops are the **unit of undo**, the **unit of collaborative sync**, and the
**unit of replay/recording**. Designing the op set is the most important
decision per domain.

### Op properties

Ops should be:

- **Self-contained.** An op carries enough information to apply and reverse
  itself without external context.
- **Idempotent where reasonable.** Re-applying `SetParameter(R, 100)` yields
  the same result. (Not all ops — `AddComponent` is not idempotent — but
  when ops *can* be idempotent, they should be.)
- **Composable.** Multiple related ops can be grouped into an `OpBatch` and
  undone atomically. "Apply batch" or "undo batch" is one history step.
- **Network-serializable.** For future collab, ops must round-trip through
  a wire format (probably postcard or bincode).

### DocumentView

A DocumentView is a panel that observes a specific document and renders a
projection of it. The `DiagramPanel`, `CodeEditor`, `ParameterInspector`,
`PlotPanel` — all are views of a `ModelicaDocument`.

A view:

- **Reads** the document to render
- **Emits** ops when the user edits (via drag, click, keypress)
- **Receives** change notifications when the document changes from elsewhere
- **Never directly mutates** the document — always goes through `apply(op)`

This is classic MVC/MVP, formalized for ECS.

## 3. API sketch

**In `lunco-ui`**:

```rust
/// Canonical identifier for a document. Usually maps to a Bevy Entity,
/// but Documents can outlive their ECS projection.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct DocumentId(pub u64);

/// A structured, mutable, observable piece of user intent.
pub trait Document: Send + Sync + 'static {
    type Op: DocumentOp;

    fn id(&self) -> DocumentId;

    /// Apply an op. Returns the inverse op (for undo).
    /// Errors if the op is structurally invalid (e.g., removing a
    /// non-existent component). Validation logic lives here.
    fn apply(&mut self, op: Self::Op) -> Result<Self::Op, DocumentError>;

    /// Generation counter — bumped on every successful `apply`.
    /// Views compare generation to decide if they need to re-render.
    fn generation(&self) -> u64;
}

/// Typed, serializable mutation.
pub trait DocumentOp:
    Clone + Send + Sync + 'static + serde::Serialize + serde::de::DeserializeOwned
{
    /// True if this op can be freely re-ordered with other ops against
    /// the same document (for future collab conflict resolution).
    /// Conservative default: false.
    fn commutes_with(&self, _other: &Self) -> bool { false }
}

/// A panel that observes a specific document.
pub trait DocumentView<D: Document> {
    fn on_change(&mut self, doc: &D, op: &D::Op);

    /// Render. Return any ops the user initiated during this frame.
    fn render(&mut self, ui: &mut egui::Ui, doc: &D) -> Vec<D::Op>;
}
```

**In `lunco-workbench`** (host):

```rust
/// Runs the doc-view loop: collect ops from views, apply to documents,
/// broadcast change events, record undo history.
pub struct DocumentHost<D: Document> {
    document: D,
    history: UndoStack<D::Op>,
    views: Vec<Box<dyn DocumentView<D>>>,
}

impl<D: Document> DocumentHost<D> {
    pub fn frame(&mut self, ui: &mut egui::Ui) {
        for view in &mut self.views {
            let ops = view.render(ui, &self.document);
            for op in ops {
                if let Ok(inverse) = self.document.apply(op.clone()) {
                    self.history.push(op.clone(), inverse);
                    for other in &mut self.views {
                        other.on_change(&self.document, &op);
                    }
                }
            }
        }
    }

    pub fn undo(&mut self) { /* ... */ }
    pub fn redo(&mut self) { /* ... */ }
}
```

Shape is approximate — real implementation needs Bevy system integration
(`Res<T>` access, `World` borrow, etc.) — but the conceptual API is this.

## 4. Documents and the ECS projection

Documents live in Tier 1. Bevy ECS lives in Tier 2. They must be kept in
sync, but the relationship is *Documents own, ECS reflects*.

### Projection direction

```
Document  ─── project ──►  ECS entities
        ◄─── events ─────  user edits
```

- On document load or change: project into ECS (spawn entities, update
  components, despawn removed).
- On user interaction in the 3D view: emit ops, apply to document, which
  then re-projects (or the view short-circuits by updating the relevant
  ECS component immediately, with the document catching up the same frame).

### Implementation approaches

**Option A: Document as resource, projection systems**

Document is a Bevy Resource. A projection system watches for generation
changes and reconciles ECS state to match.

```rust
fn project_modelica_to_ecs(
    doc: Res<ModelicaDocument>,
    mut commands: Commands,
    q_existing: Query<(Entity, &ModelicaComponent)>,
) {
    if !doc.is_changed() { return; }
    // Diff doc.components against q_existing, spawn/despawn/update.
}
```

Simple, but can be expensive for large documents. Fine for Modelica
(tens of components per model) and Mission (hundreds of events).

**Option B: Document inlined into ECS components**

The Document IS a Bevy component on an entity. Each op mutates the
component in place. Views query the component directly.

```rust
#[derive(Component)]
pub struct ModelicaDocument {
    pub ast: ModelicaAst,
    generation: u64,
}
```

Closer to current patterns, no projection step needed. Harder to undo
(need to store history externally) and harder to serialize from a
decomposed representation.

**Option C: Hybrid (recommended)**

- Document as resource (for UI/panel access, undo history, save/load)
- Projected state on ECS entities (for physics, rendering, fast query)
- Projection system reconciles on op apply

Gets the best of both. Starting point for implementation.

## 5. Serialization

Each domain owns its file format. No LunCoSim-proprietary monolithic save.

| Domain    | File format             | Library / code   |
|-----------|-------------------------|------------------|
| Modelica  | `.mo`                   | `rumoca_parser`  |
| USD       | `.usda` / `.usdc`       | `openusd`        |
| SysML     | `.sysml` (text) or XMI  | future           |
| Mission   | `.ron` or `.yaml`       | future           |
| Project   | directory + manifest    | future           |

A LunCoSim "project" is a directory:

```
my_colony/
├── project.toml           ← manifest: references, version, settings
├── scenes/
│   ├── base.usda
│   └── rover_yard.usda
├── models/
│   ├── solar_panel.mo
│   ├── battery.mo
│   └── balloon.mo
├── missions/
│   └── day1.mission.ron
└── assets/
    └── ...
```

Each document is independently editable in its native tool. A user can edit
`solar_panel.mo` in Dymola, commit it, and LunCoSim picks up the change on
next load. This is a major interoperability win.

## 6. Undo/redo

Automatic, via the Op/inverse pattern. Every op carries its inverse; the
inverse of a batch is the reverse batch of inverses.

The undo stack is **per-document**, not global. Undoing in the diagram
editor doesn't undo a 3D viewport move. This matches Dymola / Figma / VS Code
behavior — users expect undo to be contextual.

Design considerations:

- Batch boundaries: UI gestures produce batches (drag gesture = single
  undo step, not 60 per-frame `MoveComponent` ops)
- Branch handling: redoing after new edits discards the redo branch
  (standard linear history)
- Persistence: undo history is in-memory only; does not survive app
  restart (matches convention)

## 7. Change notification

When a view emits an op that the document accepts, other views need to
know. Three options:

**A. Polling via generation counter.** Views check `doc.generation()` and
re-render if it changed. Simple, wasteful (re-render per frame).

**B. Bevy Events.** `DocumentChanged<D>` event is emitted per apply.
Views subscribe. Standard Bevy pattern, works great for ECS-resident
views.

**C. Direct callback.** `on_change(op)` on each view, called by the
DocumentHost after applying. Synchronous, ordered.

**Recommendation: C for now, graduate to B if systems need it.**
Synchronous dispatch means a view that wants to react to its *own* op
(e.g., to clear an error banner) gets the callback, simplifying logic.

## 8. Future: collaboration

Ops are the right abstraction for networked editing. Multiple clients
each have a local copy of the Document and apply local ops immediately
(optimistic concurrency). Ops are broadcast over the network; remote
ops are applied on each other client.

Conflict resolution options:

- **Last-writer-wins** on concurrent edits to the same field (simplest)
- **Operational Transformation (OT)** — rewrite ops against concurrent
  ops so they still make sense (Google Docs model)
- **CRDT** — design ops to always commute (Automerge / Yjs model)

Our `DocumentOp::commutes_with(other)` method is the first step toward
CRDT. It's `false` by default (conservative); domain crates can mark
specific op pairs as commuting.

Actual implementation is out of scope for the initial Document System.
We're just ensuring the design doesn't foreclose it.

See [30-collab-roadmap.md](30-collab-roadmap.md) when written.

## 9. Per-domain implementation contract

When adding a new domain (Modelica, USD, SysML, Mission):

1. **Define the Document struct.** Usually wraps an existing parsed
   representation (AST, Stage, etc.) + a generation counter.
2. **Enumerate the Ops.** Think about every atomic user-visible change.
   Start with a minimal set; add more as views need them.
3. **Implement `Document::apply(op)`.** Validate, mutate, compute the
   inverse. This is where domain logic lives.
4. **Write the serializer.** `Document → file` and `file → Document`.
5. **Write the projector.** Sync document state to ECS entities/components.
6. **Build views.** Each panel is a `DocumentView<D>` — observes the
   document, renders a projection, emits ops.

Keep the *core document and ops in the domain crate* (`lunco-modelica`,
`lunco-usd`, etc.). Keep *views in UI sub-modules* (`lunco-modelica/ui`,
`lunco-sandbox-edit/ui`).

## 10. Design principles

1. **One source of truth per artifact.** The Modelica AST is the truth for
   its `.mo` file, not the VisualDiagram, not the generated code buffer.
2. **Ops are the only mutation path.** No "just this once" direct field
   writes. If an op doesn't exist for a change you need, add one.
3. **Validation lives in `apply`.** Not in the view, not in the projector.
4. **Domains are independent.** A change to Modelica ops doesn't ripple into
   USD code. Cross-document references go through well-defined link entities
   (`ModelicaAttachment { usd_prim: SdfPath }`, etc.).
5. **Serialization preserves semantic content.** Formatting, whitespace, and
   comments MAY be lost on a parse+write round-trip. Equations, parameters,
   structural relationships MUST be preserved.

## 11. What this unlocks

Once the Document System is in place:

- **True Dymola workflow.** Edit in diagram → ops → document → projector
  updates code view. Edit in code → parse → document → projector updates
  diagram. Everything always synced.
- **USD live editing.** Scene tree, 3D viewport, and USDA text panel all
  share one `UsdDocument`. Drag an entity in 3D → `SetTransform` op →
  scene tree updates, USDA text updates.
- **Undo/redo across the editor.** Works identically in every domain for
  free.
- **Import/export.** Each domain already has a serializer; projects are
  just directories of documents.
- **Recording and replay.** An op stream is a recording. Save it, play it
  back, re-derive the state — great for testing and demos.
- **Network collaboration.** Eventually. The op stream is already a wire
  protocol.

## 12. Roadmap

| Phase | Scope | Duration |
|-------|-------|----------|
| 1 | Design complete (this doc). | ✅ done when reviewed |
| 2 | Implement Document / DocumentOp / DocumentHost in `lunco-ui`. Minimal Modelica op set. One view migrated (CodeEditor) to prove the API. | 2 weeks |
| 3 | Modelica document: full op set, Diagram ↔ Code sync working. Solves `modelica-editor-gaps.md` P0 items. | 3–4 weeks |
| 4 | USD document: basic ops (prim add/remove, attribute set, transform). Scene tree + 3D viewport + USDA text all view the same document. | 3 weeks |
| 5 | Save/load: project directory format, per-domain file I/O. | 2 weeks |
| 6 | Mission document (first entirely-new domain). | 2–3 weeks |
| 7 | SysML document. | 4 weeks |
| 8 | Collaboration layer. | 8–12 weeks |

Phase 2 is the go-no-go. If the API feels right after migrating one panel,
commit to the pattern across the codebase. If it feels wrong, iterate before
going broader.

## 13. Open questions (to resolve during Phase 2)

- Should ops be typed per-domain (current sketch) or a universal serializable
  format (dynamic)? Typed is safer and faster; dynamic enables generic
  tooling.
- How to handle cross-document ops (e.g., an op that creates a USD prim AND
  attaches a Modelica model)? Transactional batch? Saga pattern?
- Should the DocumentHost live in `lunco-ui` (framework) or `lunco-workbench`
  (app scaffold)? Leaning `lunco-ui` so headless tests can use it.
- Projection-to-ECS: `Changed<T>` filter vs. generation diffing — which
  scales better with 1000+ entity documents?

These need real code to resolve. Don't answer speculatively.

## 14. See also

- [`00-overview.md`](00-overview.md) — three-tier architecture, where Documents fit
- [`01-ontology.md`](01-ontology.md) § 4c — Document/DocumentOp/DocumentView in the ontology
- [`11-workbench.md`](11-workbench.md) — how panels (DocumentViews) are hosted by the workbench
- [`20-domain-modelica.md`](20-domain-modelica.md) — first planned domain implementation (ModelicaDocument)
- [`21-domain-usd.md`](21-domain-usd.md) — USD as a second Document domain

