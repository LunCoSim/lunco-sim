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

## 2a. Documents, file references, and endpoints

Not every thing the user references from a Twin is a Document. We
distinguish three kinds of persisted/addressable artifacts. Only the
first is in scope for `lunco-doc` today; the others are named so we
know what we are *not* building yet and why.

| Kind | Editable inside LunCoSim? | Addressing | Examples | Crate |
|---|---|---|---|---|
| **Document** | Yes — via typed ops on structured content | `DocumentId` (+ path in Twin) | `.mo`, `.usda`, `.sysml`, `mission.ron` | `lunco-doc` |
| **File reference** | No — opaque blob, edited externally | Path in Twin | `.png`, `.glb`, `.wav`, `.stl`, `.pdf` | (Twin manifest only) |
| **Endpoint** | N/A — live, remote | URL | FMI slave, telemetry stream, Nucleus connection | future |

**Unix convention — Document ⊆ file.** Every Document is a file inside
a Twin folder. We do not invent a bespoke container format. A `.mo`
file on disk *is* the serialized form of a `ModelicaDocument`; a
`.usda` file *is* the serialized form of a `UsdDocument`. This keeps
domain files editable in their native tools (Dymola, usdview) and
makes LunCoSim a *participant* in each ecosystem rather than a walled
garden.

**File references are tracked, not edited.** A PNG used as a texture
is listed in the Twin and versioned by the user's VCS of choice, but
LunCoSim does not expose ops for editing pixels. If you want to edit
the PNG, open it in an external editor and save — the Twin notices
the file changed. Same for meshes, audio, PDFs. Mutating them inside
the simulator is not a goal of the Document System.

**Endpoints are out of scope today.** FMI slaves, live telemetry
streams, and Omniverse Nucleus connections are addressable resources
the simulator talks to, but they aren't local structured artifacts
with ops. When we build the co-sim bridge and Nucleus integration we
will likely introduce a `Resource` or `Endpoint` trait sitting next
to `Document`. We don't introduce it now because:

- We have exactly one concrete kind today — Documents.
- We don't know the shape yet (sync? subscribe? RPC?) — designing
  an abstraction over one example invents constraints we'll regret.
- Nothing in `Document` / `DocumentOp` forecloses it.

### Forward compatibility with live-sync (Nucleus, Yjs, CRDT)

LunCoSim will eventually support multi-user live editing similar to
Omniverse Nucleus (USD layers) and Yjs/Automerge (documents). That
implies, in the long run, ops that may need to commute, merge, or
apply out of order, and possibly *binary* edits with delta/xdelta
encoding.

The current `DocumentOp` trait is deliberately minimal so it can
evolve to support those without breaking callers:

- `apply` today assumes a single authoritative order. A future
  `apply_remote` or a CRDT-capable `merge` can be added in addition.
- Ops are not yet `Serialize + Deserialize`. When collab lands we add
  those bounds; domain Op types already derive serde in practice.
- Binary editing (e.g., streaming texture updates over Nucleus) will
  likely live behind a different trait (not `Document`), so we don't
  stretch `Document` to cover opaque blobs with weak op semantics.

Key rule: **don't foreclose Nucleus-style live sync**, but **don't
pay for it until we need it**.

## 3. API sketch

> The real, working version of this API ships in the `lunco-doc`
> crate (zero runtime deps, headless-capable). The sketch below is
> kept close to what's actually implemented; see `crates/lunco-doc/`
> for the authoritative source.

**In `lunco-doc`**:

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

**Also in `lunco-doc`** (host — already implemented):

```rust
/// Holds a Document plus its undo/redo stacks. Headless-capable:
/// not a Bevy component, works in tests and CLI tools.
pub struct DocumentHost<D: Document> {
    document: D,
    undo_stack: Vec<D::Op>,
    redo_stack: Vec<D::Op>,
}

impl<D: Document> DocumentHost<D> {
    pub fn apply(&mut self, op: D::Op) -> Result<(), DocumentError>;
    pub fn undo(&mut self) -> Result<bool, DocumentError>;
    pub fn redo(&mut self) -> Result<bool, DocumentError>;
    pub fn document(&self) -> &D;
    pub fn generation(&self) -> u64;
}
```

`DocumentHost` knows nothing about views, UI, or Bevy. Panels (in
`lunco-ui`, future) drive it: they read via `document()`, emit ops
by calling `apply()`, and re-render when `generation()` advances.
This keeps the core headless and testable.

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

**Unix convention: every Document is a file; every file is either a
Document or a file reference (§ 2a).** No LunCoSim-proprietary
monolithic save. No hidden container. A Twin is a folder; its contents
are real files a user can grep, diff, and edit in external tools.

Each Document domain owns its file format:

| Domain    | File format             | Library / code   | Kind |
|-----------|-------------------------|------------------|------|
| Modelica  | `.mo`                   | `rumoca_parser`  | Document |
| USD       | `.usda` / `.usdc`       | `openusd`        | Document |
| SysML     | `.sysml` (text) or XMI  | future           | Document |
| Mission   | `.ron` or `.yaml`       | future           | Document |
| Textures  | `.png`, `.jpg`, `.exr`  | —                | File reference |
| Meshes    | `.glb`, `.obj`, `.stl`  | —                | File reference |
| Docs      | `.md`, `.pdf`           | —                | File reference (today) |
| Twin      | folder + `twin.toml`    | `lunco-twin`     | — |

A LunCoSim Twin is a directory (see `13-twin-and-workflow.md` for the
full workflow):

```
my_colony/                  ← Twin root
├── twin.toml               ← tool config (which workspaces, panel layout)
├── system.sysml            ← optional: SysML source of truth for structure
├── base.usda               ← USD Documents at any level
├── rover.usda
├── solar_panel.mo          ← Modelica Documents at any level
├── battery.mo
├── day1.mission.ron
└── textures/
    └── regolith.png        ← file reference, edited externally
```

Folder structure inside a Twin is **convention, not enforced** — users
can organise files however they want. `lunco-twin` discovers Documents
by file extension, not by directory.

Each Document is independently editable in its native tool. A user
can edit `solar_panel.mo` in Dymola, commit it, and LunCoSim picks
up the change on next load. This is the core interop property — the
Document System is the *in-editor* mutation path, not a replacement
for the standard file format.

File references are tracked in the Twin (for dependency listing,
asset browser, missing-file warnings) but have no ops. Edit them
externally; the Twin notices.

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

## 8. Future: collaboration and live sync

Ops are the right abstraction for networked editing. Multiple clients
each have a local copy of the Document and apply local ops immediately
(optimistic concurrency). Ops are broadcast over the network; remote
ops are applied on each other client.

Target ecosystems we want to align with:

- **Omniverse Nucleus** — USD-native, layer-based live sync. Multiple
  clients edit the same `.usda` stage; the server reconciles layers.
  Our USD Document domain should be able to *act as* a Nucleus client
  when the Nucleus integration lands.
- **Yjs / Automerge** — CRDT libraries for text and structured JSON.
  Our text-heavy Documents (SysML prose, Modelica equation bodies,
  mission scripts) could use a CRDT text type underneath.
- **Plain Git** — not live, but the default "collab" for engineering
  work: branches, merges, PRs. All Documents round-trip through
  their domain file format so Git-based collab already works today.

Conflict resolution options for live sync:

- **Last-writer-wins** on concurrent edits to the same field (simplest)
- **Operational Transformation (OT)** — rewrite ops against concurrent
  ops so they still make sense (Google Docs model)
- **CRDT** — design ops to always commute (Automerge / Yjs / Nucleus)

The current `DocumentOp` trait is minimal but leaves room: a future
`commutes_with(other)` hook, stable ids on structural ops (so renames
don't break concurrent edits), and a `merge` path for CRDT-backed
fields are all additive.

Actual live-sync implementation is out of scope for the initial
Document System. The design explicitly does not foreclose it.

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

| Phase | Scope | Status |
|-------|-------|--------|
| 1 | Design complete (this doc). | ✅ |
| 2 | Implement `Document` / `DocumentOp` / `DocumentHost` / `DocumentError` / `DocumentId` in `lunco-doc`. Zero runtime deps. Unit tests. | ✅ |
| 3 | `lunco-twin` crate: TwinManifest, DocumentRegistry, CacheRegistry, TwinTransaction. File I/O per domain. | next |
| 4 | Modelica domain: `ModelicaDocument` + ops, migrate CodeEditor to DocumentView pattern to prove the API. | |
| 5 | Modelica: full op set, Diagram ↔ Code sync, solves `modelica-editor-gaps.md` P0 items. | |
| 6 | USD domain: basic ops (prim add/remove, attribute set, transform). Scene tree + 3D viewport + USDA text all view the same document. | |
| 7 | Mission document (first entirely-new domain). | |
| 8 | SysML document (structure + requirements only — see principles.md Article III). | |
| 9 | Live-sync layer (Nucleus / CRDT / Yjs integration). | |

Phase 4 is the go-no-go. If the API feels right after migrating one
panel, commit to the pattern across the codebase. If it feels wrong,
iterate in `lunco-doc` before going broader.

## 13. Open questions (to resolve as we migrate the first domain)

- Should ops be typed per-domain (current answer: yes) or a universal
  serializable format (dynamic)? Typed is safer and faster; dynamic
  enables generic tooling. Revisit if we grow a serialization need.
- How to handle cross-document ops (e.g., an op that creates a USD
  prim AND attaches a Modelica model)? Transactional batch? Saga
  pattern? Planned: `TwinTransaction` in `lunco-twin` — a stack of
  per-Document ops that commit/rollback together.
- Projection-to-ECS: `Changed<T>` filter vs. generation diffing —
  which scales better with 1000+ entity documents? Start with
  generation diffing per Document; measure when it matters.
- When do we introduce a `Resource` / `Endpoint` trait? Once we have
  at least two concrete kinds (FMI slave + Nucleus connection, most
  likely) so the abstraction is grounded in real examples. See § 2a.
- Binary editing ops (delta/xdelta) for Nucleus-synced textures: add
  behind a separate trait rather than stretching `Document`, because
  binary "undo" is memory-expensive and rarely structured.

These need real code to resolve. Don't answer speculatively.

## 14. Why not use an existing undo/redo library?

Short answer: we evaluated [`undo`](https://docs.rs/undo/),
[`redo`](https://crates.io/crates/redo), [`yrs`](https://docs.rs/yrs),
and [`automerge`](https://docs.rs/automerge/) and kept our own
`DocumentHost`. Three architectural differences matter:

- **Ops-as-pure-data vs. stateful commands.** Our ops are plain enum
  values — serializable, replayable, network-transportable. `undo`'s
  `Edit` trait puts methods on the command object itself, which is
  hostile to serialization (the Nucleus/live-sync future).
- **Inverse computed in one pass by domain logic.** Our `apply(op) ->
  inverse_op` is a single function with all the state context.
  `undo` requires two methods (`edit` + `undo`) that each reason
  about the same transition — two places for semantics to drift.
- **`Result`-typed apply.** We reject invalid ops at the boundary;
  `undo` assumes success.

`yrs` and `automerge` are CRDTs over JSON-like blobs. They don't
replace typed domain ops like `AddConnection { from, to }`; they're
candidates for use *inside* specific Document types (e.g. equation
bodies as `YText`), not in place of `Document`.

Full research write-up, maturity comparison, and triggers to revisit
this decision: [`research/undo-redo-libraries.md`](research/undo-redo-libraries.md).

## 15. See also

- [`00-overview.md`](00-overview.md) — three-tier architecture, where Documents fit
- [`01-ontology.md`](01-ontology.md) § 4c — Document/DocumentOp/DocumentView in the ontology
- [`11-workbench.md`](11-workbench.md) — how panels (DocumentViews) are hosted by the workbench
- [`20-domain-modelica.md`](20-domain-modelica.md) — first planned domain implementation (ModelicaDocument)
- [`21-domain-usd.md`](21-domain-usd.md) — USD as a second Document domain
- [`research/undo-redo-libraries.md`](research/undo-redo-libraries.md) — library survey & decision rationale

