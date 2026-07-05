# 18 — Unified Edit Journal & Twin History

> Status: Active · Extends: [10-document-system](10-document-system.md) and [13-twin-and-workflow](13-twin-and-workflow.md).
>
> Realized in [`lunco-twin-journal`](../../crates/lunco-twin-journal) (`JournalEntry`,
> `EntryId {author, lamport}`, DAG `parents`, `EntryKind::{Op, TextEdit, Snapshot,
> Lifecycle}`, `LamportClock`) with the Bevy bridge in `lunco-doc-bevy` and network
> replication via the journal plane ([`31-networking-and-state-sync.md`](31-networking-and-state-sync.md)).

## Goal

One **edit journal** shared by every editable domain (USD, Modelica, and future
SysML/Python/mission), owned by the **Twin** as its primary feature, and
**persisted to a file inside the Twin folder** so a Twin carries its own edit
history (undo across sessions, named versions, branches, and — later —
multi-user replication).

The headline finding of the audit: **the unified journal already exists.** The
work is *adoption + persistence + wiring*, not a greenfield design.

---

## 1. Document & Journal Architecture

### 1a. Generic document core — `lunco-doc` (shared by USD + Modelica)
- `Document` trait (`src/lib.rs`): `type Op: DocumentOp`, `apply(op) -> Result<inverse_op>`.
  `apply` returns the **inverse op**; undo is "apply the inverse".
- `DocumentHost<D>` (`src/lib.rs`): per-document undo/redo as `Vec<D::Op>` inverse
  stacks + `OpId` dedup ring + `parent_gen` staleness check (networking seed).
  `last_applied_inverse()` (`src/lib.rs`) is the journal-sink hook.
- Both `UsdDocument` (`lunco-usd/src/document.rs`) and `ModelicaDocument`
  (`lunco-modelica/src/document/core.rs`) implement this. **Undo/redo is
  already unified.**
- Bevy lifecycle events in `lunco-doc-bevy`: `DocumentOpened/Changed/Closed/Saved`,
  each `{doc: DocumentId, origin: EventOrigin}`. `EventOrigin::{Local, Remote{peer},
  Replay}` — the replication seam (reserved, unwired).

### 1b. The canonical journal — `lunco-twin-journal` (already domain-generic)
- One `Journal` per **Twin** (`TwinId`). Append-only `entries: HashMap<EntryId,
  JournalEntry>` + insertion order; `EntryId = (AuthorId, lamport)` with a
  `LamportClock` for causal merge.
- `EntryKind` (`src/lib.rs`): `Op{ domain: DomainKind, op: Value, inverse: Value }`,
  `TextEdit`, `Snapshot{source}`, `Lifecycle`.
- Version-control data model present: `Marker` (= version / git tag), `ChangeSet`
  (atomic undo group), `Branch`, `Stream` + `Composition`
  (Composition mode: Sequential/Layered/LastWriteWins), `JournalState`/`project_main`.
- Scoped `UndoManager` (`src/lib.rs`): per-author, `UndoScope::{Document(id), Twin}`.
- `OpPayload: Serialize` plug-in trait + `DomainKind::{Modelica, Usd, Sysml, Python}`.
- **Everything derives serde.** Persistence is *deliberately deferred* (crate doc
  `src/lib.rs`: "Not the persistence layer — entries live in memory; backend swap
  (yrs / disk) replaces Journal internals only").
- Bevy: `JournalResource` (`Arc<Mutex<Journal>>`) + `TwinJournalPlugin`
  (`lunco-doc-bevy/src/lib.rs`), which records **Lifecycle entries only**.

### 1c. The Twin container — `lunco-twin` (on-disk)
- A Twin = a folder + `twin.toml` manifest (`Twin` struct `src/lib.rs`).
  `TwinMode::{Orphan, Folder, Twin}`. `TwinManifest` carries `UsdManifest.default_scene`
  (the "USD scene ownership = Twin" rule). Depends only on `lunco-doc` + `lunco-storage`.
- Tracks file paths + `FileKind`; does **not** own scenes/entities/Documents and has
  **no journal dependency** and **no history file**.

### 1d. Adoption reality
| Concern | USD | Modelica |
|---|---|---|
| `DocumentHost` undo/redo | ✅ | ✅ |
| Records **Op** entries into the journal | ❌ none | ⚠️ yes but **lossy** |
| Records Lifecycle entries | ✅ (generic plugin) | ✅ |
| Journal persisted to disk | ❌ | ❌ |

- Modelica records via `doc_ops.rs` `record_op_value`, but through
  `summarize_op` — a **flat JSON summary**, because `ModelicaOp` is *not*
  `Serialize` (`journal.rs`). Entries are **not replayable**.
- USD records **zero** ops into the journal (no `OpPayload` impl, no `record_op`
  call). `UsdOp` *already derives Serialize/Deserialize* → a real (non-lossy)
  payload is cheap.

---

## 2. Gap summary

**Unification gaps**
1. USD never emits ops to the journal; Modelica's are lossy summaries.
2. No automatic `apply → journal` bridge: `DocumentHost::apply` has no journal hook,
   so each domain must hand-wire recording. `DocumentChanged` is not auto-journaled.
3. Two undo systems unreconciled: live `DocumentHost` stacks vs built-but-unwired
   journal `UndoManager`.
4. No `TwinId ↔ DocumentId` ownership map (Twin tracks file paths, not DocumentIds).

**Persistence gaps**
5. `Journal` has no `save`/`load` (pure in-memory; schema is serde-ready).
6. `lunco-twin` has no journal dep and no history path in the folder/manifest.
7. `LamportClock`/`TwinId`/`AuthorId` not persisted or re-seeded on reload.
8. IO must go through `lunco-storage` (wasm-safe), not `std::fs`.

**USD non-destructive editing gap** (from [21-domain-usd](21-domain-usd.md))
9. `UsdDocument` is a single flat `source: String`; `LayerId` is inert; `apply`
   rejects any non-root edit target; all ops collapse to whole-buffer text
   splicing; inverse = `ReplaceSource{whole previous buffer}`.

---

## 3. The convergence: openusd `EditTarget` + `Diff` ⇒ journal

We already pin `openusd = 0.5.0`. openusd `main` (post-0.5.0) adds the two
primitives that make non-destructive editing **and** the journal fall out together:

- **`EditTarget` / `edit_context(EditTarget::for_layer(id))`** — routes authoring
  to a *named layer* (session / override / runtime), emitting `over` opinions
  without touching the base asset. This is the non-destructive primitive.
- **`Diff { edits: Vec<Edit> }`** (`src/usd/diff.rs`) — a transferable, replayable
  edit list (`Edit::{Create, SetField, EraseField, Remove}`, all plain serializable
  value-data), captured per committed edit via `stage.add_sink(...)` →
  `stage.extract_diff(change)`, replayed by `stage.apply_diff`.

A `Diff` **is** a journal `Op` entry. When USD authoring migrates onto an openusd
`Stage`, the journal's USD `Op{ op, inverse }` payload can simply *be* the openusd
`Diff` (forward) + its inverse `Diff` — already cross-process, already serde. So:

```
openusd Stage  --add_sink-->  CommittedChange  --extract_diff-->  Diff
      │                                                            │
      │ EditTarget routes write to an override/session layer       │  record as
      ▼ (base .usda stays pristine)                                ▼  EntryKind::Op{domain:Usd, op:Diff, inverse:Diff}
  non-destructive                                          lunco-twin-journal  --persist-->  <twin>/.lunco/journal/*
```

---

## 4. Phased roadmap and implementation status

The development of the journal and source unification system is structured as follows:

### Phase A — Lossless, shared journal (Implemented)
- **Lossless serialization:** Implemented `OpPayload` for `UsdOp` and `ModelicaOp` to enable exact round-trip serialization of document operations.
- **Auto-bridge:** Added the `OpRecorder` trait to `DocumentHost` (`lunco-doc`) and a Bevy `JournalOpRecorder` wrapper (`lunco-doc-bevy`) to automatically record undo/redo operations.

### Phase B — Persisting the journal inside the Twin (Implemented)
- **Twin persistence:** The journal is serialized to `<twin-root>/.lunco/journal/journal.json` using a custom DTO to flatten Lamport clocks and non-string-keyed maps.
- **Twin identity mapping:** Twin identity is stable and derived from the folder/manifest. The Lamport clock is re-seeded on load from the max persisted value.

### Phase C — USD non-destructive editing (Implemented)
- **Data-canonical document:** `UsdDocument` stores composed `sdf::Data` in-memory. Operations build a transient stage, edit in the target layer, and flatten back to `sdf::Data`.
- **Runtime overlays:** Implemented base/runtime layer division. Base edits serialize to `.usda`; runtime state (transforms/spawns) serializes to `.lunco/runtime/<scene-rel>.usda` to avoid clobbering the base scene.

### Phase D — Replication (Planned)
- **Replicated journal:** Wire the remote event origin and replication channel to broadcast journal entries across the network.

### Phase E — Composed Stage & ECS Projection (Partially Implemented)
- **Unified source (Implemented):** Unified the scene-load source onto `composed_source()`, eliminating the parallel live-world and document split.
- **Incremental transforms & structural changes (Implemented):** Project incremental transforms, spawns, and deletions onto the live Bevy world from the unified source.
- **Collaboration and overrides (Planned):** Extend unified projection to support collaborative multi-user sessions and individual user override layers.

---

## 5. Architectural Rationale

- Phase A/B established the core, headless-testable undo/redo journal logic, ensuring history is persisted inside the Twin.
- Phase C modernized USD editing by replacing destructive text-splicing with non-destructive Stage edits.
- Phase D will extend this journal over the networking layer for collaborative editing.
- Phase E unifies the parallel live-world and document split into a single composed USD stage with Bevy ECS acting as its projection.

## 6. Key references
- Generic core: `lunco-doc/src/lib.rs` (`Document`), `DocumentHost`, `last_applied_inverse`.
- Journal: `lunco-twin-journal/src/lib.rs` (`EntryKind`, `Journal`, `UndoManager`); `lunco-doc-bevy/src/lib.rs` (`JournalResource`, `TwinJournalPlugin`).
- Twin: `lunco-twin/src/lib.rs` (`Twin`), `manifest.rs` (`TwinManifest`).
- USD: `lunco-usd/src/document.rs` (`UsdOp`, `Document` impl, non-root reject); `text_edit.rs` (splicers); `commands.rs` (save = source only).
- Modelica: `lunco-modelica/src/document/ops.rs` (`ModelicaOp`), `doc_ops.rs` (`record_op_value`), `journal.rs` (lossy summary caveat).
- openusd: `src/usd/stage.rs` (`EditTarget::for_layer`, `edit_context`, `in_memory`, `session_layer`, `insert_sub_layer`, `mute_layer`); `src/usd/diff.rs` (`Diff`/`Edit`, `extract_diff`/`apply_diff`, `add_sink`).
