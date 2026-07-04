# 18 — Unified Edit Journal & Twin History

Status: **as-built**. Extends
[10-document-system](10-document-system.md) and [13-twin-and-workflow](13-twin-and-workflow.md).
Realized in [`lunco-twin-journal`](../../crates/lunco-twin-journal) (`JournalEntry`,
`EntryId {author, lamport}`, DAG `parents`, `EntryKind::{Op, TextEdit, Snapshot,
Lifecycle}`, `LamportClock`) with the Bevy bridge in `lunco-doc-bevy` and network
replication via the journal plane ([`31-networking-and-state-sync.md`](31-networking-and-state-sync.md)).

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
- `Document` trait (`lib.rs:433`): `type Op: DocumentOp`, `apply(op) -> Result<inverse_op>`.
  `apply` returns the **inverse op**; undo is "apply the inverse".
- `DocumentHost<D>` (`lib.rs:472`): per-document undo/redo as `Vec<D::Op>` inverse
  stacks + `OpId` dedup ring + `parent_gen` staleness check (networking seed).
  `last_applied_inverse()` (`lib.rs:630`) is the journal-sink hook.
- Both `UsdDocument` (`lunco-usd/src/document.rs:369`) and `ModelicaDocument`
  (`lunco-modelica/src/document/core.rs:873`) implement this. **Undo/redo is
  already unified.**
- Bevy lifecycle events in `lunco-doc-bevy`: `DocumentOpened/Changed/Closed/Saved`,
  each `{doc: DocumentId, origin: EventOrigin}`. `EventOrigin::{Local, Remote{peer},
  Replay}` — the replication seam (reserved, unwired).

### 1b. The canonical journal — `lunco-twin-journal` (already domain-generic)
- One `Journal` per **Twin** (`TwinId`). Append-only `entries: HashMap<EntryId,
  JournalEntry>` + insertion order; `EntryId = (AuthorId, lamport)` with a
  `LamportClock` for causal merge.
- `EntryKind` (`lib.rs:238`): `Op{ domain: DomainKind, op: Value, inverse: Value }`,
  `TextEdit`, `Snapshot{source}`, `Lifecycle`.
- Version-control data model present: `Marker` (= version / git tag), `ChangeSet`
  (atomic undo group), `Branch`, `Stream` + `Composition`
  (Sequential/Layered/LastWriteWins), `JournalState`/`project_main`.
- Scoped `UndoManager` (`lib.rs:841`): per-author, `UndoScope::{Document(id), Twin}`.
- `OpPayload: Serialize` plug-in trait + `DomainKind::{Modelica, Usd, Sysml, Python}`.
- **Everything derives serde.** Persistence is *deliberately deferred* (crate doc
  `lib.rs:43-44`: "Not the persistence layer — entries live in memory; backend swap
  (yrs / disk) replaces Journal internals only").
- Bevy: `JournalResource` (`Arc<Mutex<Journal>>`) + `TwinJournalPlugin`
  (`lunco-doc-bevy/src/lib.rs:752`), which records **Lifecycle entries only**.

### 1c. The Twin container — `lunco-twin` (on-disk)
- A Twin = a folder + `twin.toml` manifest (`Twin` struct `src/lib.rs:221`).
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

- Modelica records via `doc_ops.rs:190 record_op_value`, but through
  `summarize_op` — a **flat JSON summary**, because `ModelicaOp` is *not*
  `Serialize` (`journal.rs:9-22`). Entries are **not replayable**.
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

## 4. Plan (phased)

Ordering: make the journal real and shared **first** (pure-logic, testable,
no dep bumps), then do the USD authoring migration that feeds it.

### Phase A — Lossless, shared journal (no openusd change)
- A1. ✅ **DONE**. `impl OpPayload for UsdOp`
  (`lunco-usd/src/document.rs`, `domain() = DomainKind::Usd`) + recording on the
  apply funnel `on_apply_usd_op` (`lunco-usd/src/commands.rs`): after a successful
  `registry.apply`, the inverse is read from `host.last_applied_inverse()` and the
  pair is recorded **lossless** via typed `record_op` (not Modelica's lossy
  summary). Test `apply_usd_op_records_lossless_journal_entries` asserts the
  recorded op round-trips to the exact `UsdOp`. No-op when `JournalResource` is
  absent (headless without `TwinJournalPlugin`).
- A2. ✅ **DONE**. Modelica entries are now lossless. Derived
  `Serialize`/`Deserialize` on `ModelicaOp` + all 14 `pretty` payload types
  (none embed rumoca AST — all pure value types, so no rumoca edit needed).
  `apply_one_op_kernel` now captures the real `(forward, inverse)` `ModelicaOp`
  pair (was `summarize_op` JSON); `record_journal_entry` records via typed
  `record_op` (`ModelicaOp: OpPayload`, domain `Modelica`) — symmetric with the
  USD funnel. Lossy `crate::journal::summarize_op` deleted.
  **Per the headless rule**, journal-entry summarization moved out of the egui
  panel into headless `lunco_twin_journal::JournalEntry::summary()` →
  `EntrySummary { tag, label, category }` (generic over the recorded JSON, so it
  serves every domain + CLI/API, not just the panel). The egui panel is now a
  thin renderer: `category_color()` maps the semantic `EntryCategory` to a new
  theme token group `lunco_theme::JournalTokens` (colour is the only visual
  decision left). Tests: `modelica_op_serde_round_trips_losslessly` (lib),
  `summarize_op_value_reads_modelica_and_usd_shapes`,
  `journal_entry_summary_covers_lifecycle` (twin-journal). Removes the
  "not replayable" caveat.
- A3. ✅ **DONE**. **Auto-bridge** via a recorder hook on
  `DocumentHost`, not per-domain hand-wiring.
  - `lunco-doc`: new journal-free trait `OpRecorder<O> { fn record(&self, forward:&O, inverse:&O) }`
    + `DocumentHost.recorder: Option<Arc<dyn OpRecorder<D::Op>>>` (`set_recorder`/
    `has_recorder`). `apply`/`undo`/`redo` all call it on success — so **undo/redo
    are now journaled too**, closing the gap where they bypassed every domain's
    record path. Forward op cloned for the recorder only when one is installed
    (zero cost otherwise). Unit test `test_recorder_captures_apply_undo_redo`.
  - `lunco-doc-bevy`: concrete `JournalOpRecorder { journal, doc, author }`
    (`impl<O: OpPayload> OpRecorder<O>`, lossless `record_op`) + generic
    `attach_journal_recorder<D>(host, journal)`.
  - Domains: each registry holds an `Option<JournalResource>`, attaches a
    recorder at host creation (`allocate`/`install_prebuilt`), and is handed the
    handle by a **reactive** `wire_*_journal_handle` system gated on
    `run_if(resource_added::<JournalResource>)` (one-shot, no per-frame poll —
    `resource_added` is true on the system's first run regardless of plugin
    order). `set_journal` retro-fits any pre-existing hosts.
  - Removed all per-op recording: USD `record_usd_journal_entry` and Modelica
    `record_journal_entry` deleted; `apply_one_op_kernel` no longer returns/forwards
    a journal pair. `author` still threaded through the Modelica deferral queue but
    no longer drives journaling (recorder labels `local_user`; per-author/origin
    attribution is phase D).
  - `lunco-doc` stays journal-free (trait injection); whole-workspace `cargo check`
    green.
- A4. Establish the `TwinId ↔ DocumentId` map (Twin owns its open DocumentIds), so
  journal entries are correctly attributed to a Twin.

### Phase B — Persist the journal inside the Twin
- B1. ✅ **DONE**. Serialize + load the journal, routed through
  `lunco-storage` (wasm-safe), loaded on Twin-open and saved on `DocumentSaved`.
  - **Serde (pure logic, in `lunco-twin-journal`)**: `Journal::to_bytes()` /
    `from_bytes()` over a private `JournalDto`. `Journal` can't derive `Serialize`
    directly — its `entries`/`change_sets`/`markers` maps are keyed by non-string
    types (`EntryId`/`ChangeSetId`/`MarkerId`) which `serde_json` can't encode as
    object keys, and `LamportClock` wraps an `AtomicU64`. The DTO flattens those
    maps to `Vec`s (rebuilt from each value's own id on load), stores entries in
    `entry_order` (canonical order survives round-trip), and persists the clock as
    a `u64` high-water mark re-`observe`d on load so a reloaded journal keeps
    appending non-colliding lamports. Test `journal_persists_and_reloads`.
  - **I/O + wiring (in `lunco-workspace`, new `journal_persistence.rs`)**: file
    lives at `<twin-root>/.lunco/journal/journal.json`. Two observers on
    `WorkspacePlugin` — **load** on `TwinAdded` (read bytes → `from_bytes` → swap
    into the live `JournalResource` *in place* via `with_write(|j| *j = loaded)`,
    preserving the shared `Arc` so A3 recorders keep writing to the loaded journal),
    **save** on `DocumentSaved` (serialize the active Twin's journal, atomic
    `.tmp`+`rename` native / direct `localStorage` set on wasm, mirroring
    `recents.rs`). Both no-op without a `JournalResource`, so headless / journal-free
    builds are unaffected. Tolerant load: missing/corrupt file ⇒ start fresh, never
    an error. Tests `journal_file_round_trips_through_disk`, `missing_journal_reads_as_none`.
  - **Crate wiring**: `lunco-workspace` gains `lunco-twin-journal` + `lunco-doc-bevy`
    deps (neither depends back → no cycle; chose `lunco-workspace` over `lunco-twin`
    as the home because it already owns `Twin.root` + `TwinAdded` + `WorkspaceResource`
    and is the natural orchestration layer above `lunco-doc-bevy`).
  - **Deferred to B2/B4**: single `journal.json` (no append-log/Snapshot compaction
    yet); whole-journal rewrite on every save (fine at current scale); `TwinId`
    inside the journal still `local-twin` (folder-derived stable id is B4); load
    *replaces* rather than *merges* (correct for single-Twin first-open).
- B2. Define the on-disk location inside the Twin: `<twin>/.lunco/journal/` (append
  log + periodic `Snapshot` baselines for compaction). Add the path/field to
  `lunco-twin` (and optionally surface in `twin.toml`). *(B1 lands the
  `journal.json` location; append-log + Snapshot compaction still open.)*
- B3. ✅ FOLDED: load-on-open is an observer on `TwinAdded` (cleaner than
  `TwinMode::open`, which is storage-layer + native-only), save-on-`DocumentSaved`
  is an observer; the crate link lives in `lunco-workspace`, not `lunco-twin`.
  Explicit-version save remains for B5.
- B4. Persist + re-seed identity: stable `TwinId` derived from the Twin
  folder/manifest; re-seed `LamportClock` from max persisted lamport on load; stable
  `AuthorId`.
- B5. Surface history in UI: `Marker` = "Save version", timeline of `ChangeSet`s,
  restore/branch. (Onshape-style versions; git-tag analogue.)

### Phase C — USD non-destructive editing on openusd (feeds the journal)
- C1. ✅ **DONE**. **Decision: NO openusd bump for C1/C2 — track the
  pinned `=0.5.0`.** The audit was wrong that `EditTarget` is post-0.5.0: 0.5.0
  already ships `Stage`, `EditTarget::for_layer`, `Stage::edit_context`,
  `EditContext`, `override_prim`, `create_attribute().set()`. Only the **`Diff`
  extraction** API (`add_sink`/`extract_diff`/`apply_diff`, `struct Diff`) is
  main-only — that's C3, and it's deferrable because A1 already journals `UsdOp`
  losslessly.
  - Spike `crates/lunco-usd/src/edit_target_spike.rs` (test-only, `#[cfg(test)]`,
    deletable once C2 lands) proves on 0.5.0: (1) an override authored into a
    stronger layer wins on the composed stage; (2) **editing `parent.radius` in
    one layer does NOT clobber a nested `child.radius` in another** — the exact
    CQ-503 corruption text-splicing can't avoid.
  - **0.5.0 layer constraints C2 must honor** (learned the hard way in the spike):
    - **Disk-loaded layers are read-only** (`Layer(ReadOnly)`) — the session
      layer loaded from a path can't be authored into; the override/runtime
      layers must be **in-memory anonymous** layers, not files.
    - **Only `root_layer()` / `session_layer()` expose `.data()`** for per-layer
      USDA text; anonymous *sublayers* don't, and `sdf::Layer` isn't `Clone`
      (it's *moved* into `insert_sub_layer`). Serializing a multi-layer
      base/override/runtime model back to separate text files is therefore NOT
      straightforward in 0.5.0.
    - Strength order strongest→weakest: session > root's own opinions > root's
      sublayers. So "base in a weak anon sublayer, override in the strong root
      layer" is the working arrangement for a 2-layer test.
- **C-arch (LOCKED, user: "do properly, NO legacy, use openusd don't
  reinvent, composition via openusd too"). openusd bumped to `main`** via root
  `[patch.crates-io] openusd = { path = "../openusd" }` (local checkout
  fast-forwarded to rev `06d619d4`, has the Diff API; still version 0.5.0 so it
  satisfies `=0.5.0`). Two hard openusd facts shaped the design:
  - **In-memory composed Stages WORK on wasm** — `compose.rs` already does it via
    `LuncoUsdResolver` (a custom `ar` resolver serving USDA bytes from a
    `HashMap`) + `Stage::builder().resolver(r).open(id)`. So composition is
    openusd-native already; the only legacy is flattening to the removed
    `TextReader`.
  - **`Stage(Rc<StageInner>)` is `!Send`** → it cannot live in Bevy ECS
    resources/components or cross an async asset loader. `sdf::Data`
    (`HashMap<Path, SpecData>`) **is** `Send+Sync`. This forces a flatten step
    and makes the document **data-canonical**, not live-Stage-canonical.
- C2. **Data-canonical doc + transient-Stage authoring.** `UsdDocument` stores the
  Send-safe composed `sdf::Data`. Each op builds a *transient* openusd Stage
  (resolver, as `compose.rs` does), `edit_context(EditTarget::for_layer(override))`,
  authors via `define_prim`/`override_prim`/`create_attribute().set()`, then
  flattens back to `sdf::Data`. Base layers stay read-only (resolver-loaded);
  edits land in a writable anon **override** layer (and a **runtime** layer for
  generated state) → non-destructive, kills the `text_edit.rs` splicer + the
  CQ-503 nested-child corruption. `apply` stops rejecting non-root targets;
  `LayerId` becomes a real layer identifier. Keeps Bevy parallel ECS + async
  loaders intact (Send).
- C2-reads. **All downstream reads move to `sdf::Data`** (openusd `main` removed
  `TextReader`). New helper module `lunco-usd-bevy/src/usd_data.rs`
  (`field`/`field_as`/`prim_children`/`prim_type_name`/`prim_attribute_value`/
  `attribute_value`) replaces `TextReader::{try_get,prim_children,
  prim_attribute_value}` across `lunco-usd-bevy` (lib/light/compose), `lunco-usd`
  (loaded_stages/viewport/document), `lunco-usd-avian`, `lunco-usd-sim`,
  `lunco-materials`. `compose.rs` returns `sdf::Data` instead of `TextReader`.
- C3. Journal payload **stays the serializable `UsdOp`** — openusd's `Diff`/`Edit`
  are **not** serde and not `Clone`, and `extract_diff` captures no inverse
  (main-only, replay-oriented). Use the Diff API (`add_sink`→`extract_diff`→
  `apply_diff`) only for *in-memory* undo/replay if helpful. Typed per-op
  inverses come from **reading pre-state off the transient Stage** before
  authoring (replaces the coarse whole-buffer `ReplaceSource` inverse). Phase D
  replication would need serde added to openusd `Diff` upstream.
- C4. Persist runtime state (obstacle field, spawn transforms) into the **runtime
  layer** instead of ECS-only, so it round-trips and is journaled.
  - **C4a infra ✅ DONE.** `UsdDocument` is now two-layer:
    `base: sdf::Data` (authored, serialized by `source()`/Save) + `runtime:
    sdf::Data` (generated overlay, **never** saved). `LayerId::runtime()` is a
    real identity; `apply` routes ops by `edit_target` (root→base,
    runtime→runtime), rejecting unknown ids (no silent misroute). Inverses
    **target the same layer** (`AddPrim`→`RemovePrim{edit_target: runtime}`,
    coarse `ReplaceSource` carries that layer's source), so per-layer undo never
    crosses layers. Validation spans both layers for parent/prim existence;
    `RemovePrim` is restricted to prims the target layer authored. Tests prove
    routing, save-isolation (runtime state absent from `source()`), per-layer
    undo, and base/runtime independence.
  - **N.B.** the C2 design sketch said the multi-layer split would land in C2;
    in practice C2/C3 shipped **single-root** (author into the base layer by SDF
    path — already kills CQ-503), and the genuine base/runtime split + non-root
    routing landed here in C4a. The C1 spike's base-weak/override-strong layer
    *stacking* remains unused (author is single-layer-per-op + extract).
  - **C4b keystone ✅ DONE:** `lunco_usd_bevy::author::compose_layers(base, runtime)` is the **sdf
    layer-stack merge** openusd doesn't expose: runtime fields win per spec via
    `SpecData::add` (upsert), runtime-only specs copied wholesale, `primChildren`
    unioned. References survive as opinions (NOT a Stage/PCP flatten — that
    resolves refs, wrong for an authored view). `UsdDocument::composed()` /
    `composed_source()` expose it; `source()`/Save stay base-only.
    `viewport.rs` `install_active_doc`/`rebuild_active_asset` now refeed from
    `composed_source()`, so runtime-layer state RENDERS while the saved file
    stays base. Safe no-op while runtime is empty (composed == base). Tests:
    `compose_layers_overlays_runtime_onto_base`, `…_empty_runtime_is_base`,
    `composed_view_includes_runtime_but_source_excludes_it`.
  - **C4b move producer ✅ DONE.** `persist_move_to_runtime_layer`
    (a second observer on `MoveEntity` in `lunco-sandbox-edit::commands`,
    decoupled from the physics handler) persists an authored-scene entity's move
    as `SetTranslate{edit_target: runtime}` into the **active** USD doc (resolved
    via `lunco_workspace::WorkspaceResource.active_document`; kind-safe through
    `UsdDocumentRegistry::host`). **Ownership-guarded**: fires only when the
    moved entity's `UsdPrimPath` prim is present in the active doc's base/runtime
    layer — palette/sim spawns that aren't part of the authored scene are
    skipped, so no stray opinions. Round-trips via the journal + renders via the
    composed view; Save stays base-only. 2 integration tests
    (`move_of_authored_prim_persists_to_runtime_layer`,
    `move_of_unowned_entity_is_skipped`).
  - **C4b active-doc unification ✅ FIX.** The move producer reads
    `workspace.active_document` (the unified, cross-workbench focused-doc
    pointer Modelica already publishes to), but the **USD viewport never
    published into it** — it tracked its mounted doc only in the USD-private
    `UsdViewportState.active_doc` (a *render-mount* marker bundled with
    `current_handle`/`scene_root`/generation). So `workspace.active_document`
    was always `None`/Modelica → the producer silently no-oped in the real app
    (tests passed only because they set it by hand). Fix: `install_active_doc`
    (the single chokepoint all activation paths funnel through) now also writes
    `workspace.active_document = Some(doc)`, mirroring Modelica's publish — so
    USD finally participates in the one active-doc protocol (also benefits Save
    and every other consumer). `UsdViewportState.active_doc` stays as the
    legitimately viewport-private mount marker (drives generation rebuilds even
    when focus moves to a Modelica tab). The close path is already handled by
    `workspace.close_document` (repoints active before `DocumentClosed`), and a
    dangling pointer is a safe no-op via the producer's `host(doc)` guard.
    Regression test: `install_publishes_active_doc_to_workspace`.
  - **C4b spawn producer ✅ DONE.** Persisting a *spawn* needs a
    reference arc on the runtime prim — the new op capability. `UsdOp::AddPrim`
    gained `reference: Option<String>`; `Some` authors a `references = @asset@`
    opinion via `lunco_usd_bevy::author::author_reference` (openusd's Stage has
    no `add_reference`, so it's set at the `sdf` level — the symmetric
    counterpart of how `compose` *reads* `Value::ReferenceListOp`; survives
    `data_to_usda` as a `references = @…@` metadata opinion, resolved at render
    time by the PCP composer). Inverts cleanly to `RemovePrim`.
    `persist_spawn_to_runtime_layer` (a second observer on `SpawnEntity` in
    `lunco-sandbox-edit`, decoupled from the ECS spawn handler) resolves the
    catalog asset path + active doc and authors `AddPrim{runtime, reference}`
    under the doc's default prim + `SetTranslate{runtime}` for the drop
    position. The spawn rides the composed view + Twin journal; Save stays
    base-only. Tests: `author_reference_round_trips_through_usda` (lunco-usd-bevy),
    `spawn_op_authors_runtime_reference_excluded_from_save` (lunco-usd),
    `spawn_persists_referenced_prim_to_runtime_layer` (lunco-sandbox-edit).
    **Both runtime-state producers (move + spawn) now land — Phase C4b is
    complete.** The **obstacle-field generator** (`lunco-obstacle-field`)
    remains a deliberate **non-fit**: procedural by design, its spec+seed is the
    right persistence, not per-rock prims.

### Phase C5 — Runtime-state persistence + live-world bridge

The runtime overlay (C4b spawns + moves) lives only in memory + the offscreen
viewport; the live sandbox world is a **parallel** stage loaded from the base
`.usda` via the asset pipeline (`LoadScene → compose_to_data → instantiate_usd_prim`),
and the journal is a passive log that is **never replayed**. So without C5 the
runtime overlay is lost on reload and never reaches the live world.

  - **C5-A runtime-overlay persistence ✅ DONE.** The runtime layer
    is serialized to its **own** file, `<twin>/.lunco/runtime/<scene-rel>.usda`
    (parallel to the journal — *not* journal replay, which doesn't exist).
    `UsdDocument::restore_runtime(data)` = a session-restore load (bumps
    generation for viewport rebuild, preserves the base dirty flag, bypasses the
    op layer + journal since it *reconstructs* rather than authors). New
    `lunco-usd/src/runtime_persistence.rs` (in `lunco-usd`, not `lunco-workspace`
    — serializing `sdf::Data` needs `lunco-usd-bevy`, which would be a dep cycle
    from workspace): **load** on `DocumentOpened`, **save** on `DocumentChanged`
    (skips empty-runtime / non-twin docs); I/O via `lunco-storage` atomic
    tmp+rename. Path = `<twin-root>` (prefix-matched from the doc's
    `origin().canonical_path()`) `/.lunco/runtime/<scene-rel>`. 4 tests
    (path mapping, full spawn round-trip → restore → composed carries `@…@` /
    base clean, missing-file tolerance, empty-runtime skip).
  - **C5-B live-world bridge — SUPERSEDED by E1 (not built).** The original plan
    was a consumer that turns runtime-layer prims into live entities, with a real
    identity/dedup fork (b1 reload-scoped / b2 unified-spawn / b3 tag-at-spawn)
    against the networking-coupled palette spawn. **E1 dissolves this:** instead of
    bridging two stages, the live scene root is sourced *from* the document's
    composed (`base ⊕ runtime`) stage, so runtime spawns/moves are instantiated by
    the normal `instantiate_usd_prim` path — there is no second stage to reconcile
    and no dedup fork. See **Phase E → E1 (IMPLEMENTED)** below. The b1/b2/b3
    analysis is retained here only as the record of why the bridge was abandoned.

### Phase D — Replication (optional, later)
- D1. Wire `EventOrigin::Remote` + `Journal::append_remote` to `lunco-networking`;
  broadcast journal entries (USD entries are openusd `Diff`s — already
  process-portable). Folds document collaboration into the existing multiplayer
  transport.

---

## Phase E — One Stage: ECS as Projection (north-star, Omniverse-aligned)

**The root architectural debt.** The live sandbox world and the editable
`UsdDocument` are **two parallel stages**: the live world is Bevy ECS
instantiated from the `.usda` *file* (`LoadScene → AssetServer.load →
compose_to_data → instantiate_usd_prim`); the editable document is a composed
stage that renders **only** to the offscreen viewport. Every C5 question —
spawns reaching the live world, reload, multi-user collaboration, DCC interop —
is hard *because of that split*. The tactical C5-B bridge (b1/b2/b3) is
scaffolding over this debt, not a fix.

**The target (how Omniverse actually works).** There is **one composed USD
stage** as the single source of truth; everything else is a projection of it.
USD layers hold the **cold, authoritative, interoperable, collaborative** data;
the ECS world is the **hot, per-frame runtime projection** (Omniverse's
Fabric/USDRT). They sync only at *meaningful boundaries* (spawn, deliberate
move, save) — never per physics tick (the per-frame physics stays in ECS, exactly
as today).

| Omniverse | Role | luncosim target |
|---|---|---|
| USD composed stage | single source of truth | `UsdDocument` composed stage (`base ⊕ runtime ⊕ …`) — the live world instantiates from *this*, not the file |
| Layer stack (root + session + sublayers) | authored vs transient vs per-user | `base` (saved `.usda`) · `runtime`/session (`.lunco/runtime`, C5-A) · future per-user override layers |
| Fabric / USDRT | fast per-frame runtime scene | Bevy ECS — the runtime projection |
| Hydra render delegate | consumes stage change notices | `instantiate_usd_prim` + a **change-consumer** applying `UsdChange::{Resync,InfoOnly}` incrementally |
| Nucleus live-sync (`.live`) | broadcast layer deltas to clients | the journal as a change stream → `lunco-networking` (Phase D) |
| Ar resolver + `omniverse://` URLs | any USD app mounts the same stage | openusd `ar::Resolver` (`LuncoUsdResolver`) over the Twin / a server |
| Connectors (Maya/Blender/Houdini) | external SW reads/writes the same USD | `.usda` layers over that resolver — USD *is* the interchange format |

**Decomposition (incremental — builds on C2–C5, no big-bang rewrite):**
- **E1. Unify the source (highest leverage). — IMPLEMENTED
  (doc-backed scope).** The live scene root for a USD *file* document is mounted
  from the document's `composed_source()` (`base ⊕ runtime`) instead of the raw
  file; `instantiate_usd_prim` already consumes a `UsdStageAsset`, so it is fed
  the composed one. **Reloaded runtime spawns/moves now appear in the live world
  automatically — C5-B dissolves (no bridge, no dedup fork).**

  Implementation (`crates/lunco-usd/src/live_projection.rs`), mirroring the
  viewport's proven `install_active_doc` / `rebuild_active_asset` split:
  - `lunco-usd-sim::cosim::spawn_scene_root_with_stage(world, label, root_prim,
    handle)` — the file-free door: spawns a scene root from a caller-built
    `Handle<UsdStageAsset>`. `spawn_scene_root_world` now builds the file handle
    then delegates to it (crate layering preserved — cosim stays document-blind).
  - `on_open_file` no longer file-imports real paths; it records them in
    `PendingLiveImports`. (`mem://` / `bundled://` keep the file-backed path —
    they never become registered file docs.)
  - `project_pending_live_imports` (system) — first mount. Document allocation is
    *async* (`on_open_file_for_usd` → `drain_pending_usd_file_loads`), so the
    mount can't happen inline at `OpenFile`; this matches each pending path to its
    document by file origin once it exists, builds the composed
    `UsdStageAsset`, and tags the root `LiveDocScene { doc, generation }`.
  - `refresh_live_doc_scenes` (system) — generation-keyed re-mount. Every runtime
    edit bumps `UsdDocument` generation (and so does the open-time
    `restore_runtime`), so this rebuilds the projected stage in place whenever the
    doc moves past the mounted generation — order-independent w.r.t. restore.

  **Scope deviation from the original sketch:** the live *default* twin scene
  loads via `LoadScene` with **no** `UsdDocument` (`open_usd_docs_on_twin_added`
  fires `LoadScene`, it does not `allocate`), so "every live scene is doc-backed"
  is false today. E1 is therefore **doc-backed only** — it covers exactly the
  spawn/move-persistence case (which already requires an open `active_document`).
  Making the default twin scene auto-open a document (so the fallback disappears)
  is the **E1b** follow-up — IMPLEMENTED below.

  **E1b. Default twin scene = doc-backed, web-ready (IMPLEMENTED).**
  The default twin scene loads through the `twin://` asset source + the async
  `UsdLoader`, which already re-attaches the `scheme://` so co-located refs
  (terrain `.glb`) resolve on every platform the source supports. So rather than
  E1's native-only synchronous `compose_native_fs`, E1b keeps that async path and
  makes it doc-backed by serving the scene document's **composed** source as a
  **byte-overlay on the twin source**:
  - **lunco-assets** (`twin_source.rs`): `TwinRoots` gains an `overlays` map
    (`set_overlay`/`clear_overlay`); `TwinReader::read` returns overlay bytes when
    present, else falls back to fs/http. Keyed by the reader-facing `<name>/<rel>`.
  - **lunco-usd-bevy**: a tiny `UsdSourceText(String)` asset + loader so the
    document's base layer is read *through the twin source* (web-ready), not
    `FileStorage`/`std::fs`. Shares the `.usda` extension with `UsdLoader`; the
    requested asset type selects the loader.
  - **lunco-usd** (`twin_projection.rs`): `open_usd_docs_on_twin_added` keeps
    firing `LoadScene` (immediate file-backed mount) AND kicks a `UsdSourceText`
    load of `twin://<name>/<scene>`; `drain_pending_twin_docs` allocates the
    document once the base text arrives (origin = on-disk path, so Save + dedup
    work); `sync_twin_overlays` serializes the composed source into the twin
    overlay and `reload`s the scene asset whenever the document generation moves
    (initial, open-time `restore_runtime`, later spawns/moves). The existing
    asset-reload → re-instantiate machinery refeeds the live world.
  - **Why this is the web-ready path:** the live world re-composes from the
    document through the *same* `twin://`-anchored async loader, so the `twin://`
    identity (and thus all co-located/library refs) is preserved — native and web
    alike. The remaining web gap is **pre-existing and external to E1b**: the
    `twin://` source's *reader* is itself native-only today (`#[cfg(not(wasm32))]`,
    "web = TODO http" in `asset_sources.rs`). When that http reader lands, E1b
    works on web with no further change — the overlay mechanism has no native-only
    assumption.
  - Headless-safe: the E1b systems are `run_if(resource_exists::<AssetServer>)`
    and the observer's `AssetServer` is optional, so `MinimalPlugins` test apps
    skip E1b (and `LoadScene` still mounts the scene).
- **E2. ECS as a change-consumer.** Replace full re-instantiation with a system
  that reacts to `UsdChange` and incrementally spawns/despawns/transforms
  entities (the "sim/render delegate"). ECS stops being a parallel authority and
  becomes a view; the C4b producers (ECS → layer) + this consumer (layer → ECS)
  form the bidirectional sync at meaningful boundaries. Decomposed into 4 slices
  (the substrate is `UsdDocument::changes_since(gen)` yielding granular
  `UsdChange`: a move = `InfoOnly{xformOp:translate}`, spawn/remove/rename =
  `Resync`, wholesale = `FullReload`):
  - **E2-1. Incremental transforms — IMPLEMENTED.** New
    `lunco-usd/src/live_consume.rs`: `classify_changes_since(reg, doc, since,
    cur_gen)` splits the deltas into transform-only paths vs `needs_structural`
    (anything else, OR a change-ring overflow → conservative reload).
    `apply_translates(world, stage_handle_id, composed, paths)` reads the cheap
    `composed()` base⊕runtime merge (NOT the PCP flatten), decodes via
    `lunco_usd_bevy::get_attribute_as_vec3` (made `pub`), and writes
    `Transform.translation` on the matching `UsdPrimPath` entity (scoped to the
    scene stage handle). Both refresh systems became change-aware: E1
    `refresh_live_doc_scenes` and E1b `sync_twin_overlays` (now an exclusive
    `&mut World` system that *always* updates the overlay for persistence but only
    `reload`s on structural). **Result: dragging a gizmo no longer re-instantiates
    the whole scene — moves apply in place, no reload.** Known gap: a *non-interactive*
    move (undo/API) of an avian physics body sets only `Transform` (overwritten by
    avian) — interactive moves already positioned the body, so the common path is
    fine; full avian-aware writeback is a follow-up. Tests: classify (move=transform,
    spawn=structural), ring-overflow→structural. lunco-usd 54 green.
  - **E2-2 / E2-3 / E2-4. Incremental spawn + despawn, structural reload retired
    — IMPLEMENTED.** `classify_changes_since` now also returns
    `resync_paths` (every concrete-prim `Resync`) and a `full_reload` flag (set by
    a whole-source `FullReload`, a whole-stage `Resync { "/" }`, or a change-ring
    overflow). `live_consume::reconcile_structural(world, stage_handle_id, reader,
    resync_paths)` diffs each changed path against the **fresh** composed reader:
    *absent + live* → despawn, *present + not-live* → spawn, else no-op (a
    rename is the natural remove+add of two paths). The two ECS-side primitives
    live in `lunco-usd-sim/src/cosim.rs` (where `ModelicaChannels` / `ScriptRegistry`
    / `CellCoord` / `LoadIntoGrid` are in scope):
      - `despawn_usd_subtree(world, root)` — the per-prim analogue of
        `clear_scene_entities`: BFS the subtree, send `ModelicaCommand::Despawn`
        for each `ModelicaModel` + drop each `ScriptedModel`'s `ScriptRegistry`
        doc (resources read optionally, headless-safe), then recursive `despawn`.
        Siblings untouched. (SimConnection-wire GC for a single removed prim is a
        follow-up — wires aren't subtree children; whole-scene clears still GC them.)
      - `spawn_usd_child(world, stage_handle_id, reader, path)` — mirrors the
        loader's child branch: find the live **parent** entity by composed path,
        spawn the child with the same atomic `(UsdPrimPath, ChildOf, pre-read
        translate, grid-anchor/instance-membership)` bundle, and let
        `on_usd_prim_added` instantiate its subtree. Idempotent (skips a path that
        already has a live entity → palette double-spawn dedup falls out for free,
        superseding the old C5-B b3 `RuntimePrimPath` scheme).
    - **E1** (`refresh_live_doc_scenes`) reconciles **inline**: swap the asset
      reader (sync `compose_native_fs` flatten), then `reconcile_structural` over
      `resync_paths`. The whole-scene in-place rebuild now runs **only** on
      `full_reload` / unknown-batch — so a single spawn/delete no longer
      re-instantiates terrain + every other rover (E2-4 for E1).
    - **E1b** (`sync_twin_overlays`) reconciles **deferred**: the flattened reader
      refreshes *asynchronously* through the `twin://` loader (only it resolves
      `twin://` / `lunco://` refs — a sync compose can't), so the reconcile is
      queued in `PendingTwinReconciles` and run by `drain_twin_reconciles` once
      the reload's `LoadedWithDependencies` lands (buffered by
      `collect_reloaded_twin_assets`). This also **fixes a latent E1b bug**: the
      previous "just `reload`" structural path was a no-op for an already-built
      scene (a reload doesn't re-instantiate `UsdVisualSynced` entities), so twin
      structural edits never showed; the deferred reconcile (or `full_rebuild_twin_scene`
      for the coarse case) now actually refeeds them. First mount (`synced == None`)
      stays the plain initial load via `sync_usd_visuals` — no baseline to diff.
    Tests: `resync_paths` populated + `full_reload` clear for add/remove,
    `full_reload` set on ring overflow. lunco-usd 55 green; lunco-usd-sim builds.
    **Verification still pending:** none of E1/E1b/E2 has been exercised in the
    running sandbox — the async twin reconcile timing in particular wants an
    end-to-end check (spawn a rover → drag → delete → reopen).
- **E3. Journal → network = collaboration (= Phase D).** Local edits append to
  the journal; remote edits replay from it. This is Nucleus live-sync; openusd
  `Diff` is the process-portable delta.
- **E4. Resolver/URL interop.** Point the `ar::Resolver` at shared Twin storage
  (or a server); any USD-aware tool composes the same identifiers. External-SW
  integration is "open the Twin's USD over the resolver," not a bespoke format.
- **E5. Per-user override layers.** Add stronger per-user/per-tool sublayers in
  the stack for non-destructive concurrent editing (Omniverse's live layer +
  user opinions).

**First slice = E1.** Until E1 lands, build no permanent C5-B bridge — a
reload-scoped one-shot (b1) is acceptable *only* as a stopgap, and should be
deleted when E1 unifies the source. E1 is the single change that converts the
whole C5/D/interop surface from "hard, per-feature plumbing" to "falls out of the
one-stage model."

---

## 5. Why this ordering

- Phase A/B are **pure logic** — testable headless, no dependency bumps, immediate
  payoff (lossless cross-domain history persisted in the Twin). This is the core of
  the user's request and de-risks everything else.
- Phase C is the larger USD authoring migration; it *consumes* the journal interface
  from A and *replaces* the destructive text-splicer. Gated on the openusd `main`
  decision (C1 spike).
- Phase D reuses A–C with no new model — replication is "broadcast the journal".
- Phase E is the **north star**: collapse the parallel live-world/document split
  into one composed stage with ECS as its projection. It is what makes C5
  (reload), D (collaboration), and DCC interop *fall out* instead of each needing
  bespoke plumbing. E1 (unify the scene-load source onto `composed_source()`) is
  the highest-leverage first slice and supersedes the tactical C5-B bridge.

## 6. Key references
- Generic core: `lunco-doc/src/lib.rs:433` (Document), `:472` (DocumentHost), `:630`
  (last_applied_inverse).
- Journal: `lunco-twin-journal/src/lib.rs:238` (EntryKind), `:441` (Journal), `:841`
  (UndoManager); `lunco-doc-bevy/src/lib.rs:644` (JournalResource), `:752`
  (TwinJournalPlugin).
- Twin: `lunco-twin/src/lib.rs:221` (Twin), `manifest.rs:39` (TwinManifest).
- USD today: `lunco-usd/src/document.rs:135` (UsdOp), `:369` (Document impl), `:397`
  (non-root reject); `text_edit.rs` (splicers); `commands.rs:415` (save = source only).
- Modelica today: `lunco-modelica/src/document/ops.rs:123` (ModelicaOp),
  `doc_ops.rs:190` (record_op_value), `journal.rs:9` (lossy summary caveat).
- openusd: `src/usd/stage.rs` (`EditTarget::for_layer`, `edit_context`, `in_memory`,
  `session_layer`, `insert_sub_layer`, `mute_layer`); `src/usd/diff.rs`
  (`Diff`/`Edit`, `extract_diff`/`apply_diff`, `add_sink`).
