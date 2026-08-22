# 18 — Unified Edit Journal & Twin History

> Status: Active · Audience: contributors working on document edits, history, undo, and journal sync

The Twin journal is the canonical record of authored document changes. It is
implemented by `lunco-twin-journal`, exposed to ECS through `lunco-doc-bevy`,
and persisted by `lunco-workspace` when the Twin opts in. Runtime command/session
replay is a separate design in [`command-journal.md`](command-journal.md).

## Ownership

| Concern | Owner | Current contract |
|---|---|---|
| Journal data model | `lunco-twin-journal` | Append-only `Journal` scoped by `TwinId`; entries use author/lamport identity, `EntryKind`, `ChangeSet`, and reversible op payloads |
| ECS access and automatic recording | `lunco-doc-bevy` | `JournalResource` wraps the active journal; `JournalOpRecorder` records successful document apply, undo, and redo operations |
| Document undo/redo | `lunco-doc::DocumentHost` | Each domain owns its typed inverse stacks; the recorder mirrors those edits into the Twin journal |
| Twin selection and persistence policy | `lunco-twin` + `lunco-workspace` | `[journal] persist = true` opts a Twin into `history/journal.json`; session-only is the default |
| Network distribution | `lunco-networking` | The journal replication plane sends entries and merges them by `EntryId` |

The document remains the authoritative authored state. The journal records the
operation and inverse that produced it; it does not replace the document or
become a second scene representation.

## What is journaled

Document-domain operations implement `lunco_twin_journal::OpPayload` and are
recorded losslessly through the generic host recorder. Current domains include
USD and Modelica, with additional definition domains such as scripts, shaders,
experiments, obstacle fields, tool libraries, and timelines using their own
payload types.

Lifecycle events are recorded as `EntryKind::Lifecycle`. Experiment results,
telemetry samples, and transient runtime state are not authored document ops:

- experiment definitions may be journaled; run results are artifacts;
- telemetry uses the signal/event paths;
- runtime overlays live under `.lunco/runtime` and are disposable derived state;
- `#[Command]` execution history and deterministic session replay are not yet
  journaled. See [`command-journal.md`](command-journal.md).

## Undo, replay, and sync

`DocumentHost` applies a typed op and obtains its inverse. The same host path
records the forward/inverse pair, including undo and redo, in the active
`JournalResource`. Remote entries are merged by the journal plane and applied
through the owning document registry without being recorded a second time.

The journal is therefore one cross-domain authored-edit stream, not a per-domain
undo stack or a network-specific copy. New domains should implement
`DocumentOp` and `OpPayload`, install the generic recorder, and use the existing
journal-plane transport.

## Persistence

The journal always exists in memory when the workspace installs
`JournalResource`. Disk persistence is opt-in in the Twin manifest:

```toml
[journal]
persist = true
```

With that setting, `lunco-workspace` loads and saves
`<twin-root>/history/journal.json` through `lunco-storage`. Without it, an
opened folder neither loads nor writes a journal file. This keeps a run's
derived history out of authored content unless the Twin explicitly owns it.

## Boundaries

- Do not mutate a document source directly; use its typed command/document host
  path so inverse generation and journaling cannot diverge.
- Do not put telemetry, per-frame controls, or runtime caches in the journal.
- Do not add a second domain broadcast for authored edits; use `OpPayload` and
  the networking journal plane.
- Do not treat `history/journal.json` as a scene/document asset. It is a
  persisted log owned by the Twin workspace.

## Related contracts

- [`10-document-system.md`](10-document-system.md) — document hosts and typed ops
- [`13-twin-and-workflow.md`](13-twin-and-workflow.md) — Twin contents and workflow
- [`command-journal.md`](command-journal.md) — future command/session replay
- [`../../crates/lunco-networking/SYNC_ARCHITECTURE.md`](../../crates/lunco-networking/SYNC_ARCHITECTURE.md) — journal-plane sync
- [`../../crates/lunco-doc-bevy/src/lib.rs`](../../crates/lunco-doc-bevy/src/lib.rs) — ECS journal bridge
