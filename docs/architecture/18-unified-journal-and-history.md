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
| Document undo/redo | `lunco-doc::DocumentHost` | Each domain owns typed inverse history groups; the recorder mirrors each member edit into the Twin journal |
| Twin selection and persistence policy | `lunco-twin` + `lunco-workspace` | `[journal] persist = true` opts a Twin into `history/journal.json`; session-only is the default |
| Network distribution | `lunco-networking-sync` | The journal replication plane sends entries and merges them by `EntryId`; `lunco-networking` supplies the transport adapter |

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

There are two deliberate exceptions to the authored-edit path. A source that
is refreshed by its external owner — a disk-backed `.rhai` file, USD
`info:sourceCode`, or a generated Modelica network — uses the shared
`FileBacked::reload_base` contract. It updates the live document generation,
parse/compile invalidation, and clean baseline without creating a second
`DocumentHost` undo entry or duplicating the source owner's journal entry.
Likewise, a derived presentation such as the USD route ribbon uses a typed
transient projection command. It may update the runtime view and its
projection cursor, but it is not authored content and therefore is not saved,
undone, or journaled.

User changes remain different: Rhai/ScriptDocument source and pin edits,
Modelica source/structural edits, and USD authored operations all go through
their existing `DocumentHost`/typed-command owners. Their undo and redo calls
use the same host and recorder, so both the forward edit and its inverse are
losslessly represented in the Twin journal.

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

- Do not mutate a user-owned document source directly; use its typed
  command/document-host path so inverse generation and journaling cannot
  diverge. For an externally owned source, use `FileBacked::reload_base`; for
  a derived view, use its typed transient projection command. Neither is a
  substitute for a user edit or a reason to emit a duplicate journal entry.
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
