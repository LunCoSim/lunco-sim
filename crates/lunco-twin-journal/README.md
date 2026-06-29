# lunco-twin-journal

Append-only, author-scoped **journal** of every change within a Twin.

Records every applied document op, raw text edit, and lifecycle event into a
single canonical log scoped to a Twin. Per-author undo and per-document /
per-twin scopes are **filtered views** over the same log — so a user can undo
*their* edits without clobbering a peer's interleaved work.

## Architectural shape

- **Entries** are immutable, identified by `(author, lamport)` pairs. Lamport
  clocks give causal ordering without wall-clock dependence and align with `yrs`
  CRDT ids `(client_id, clock)` for a future swap-in.
- **Streams** are named sequences of entries with a composition policy
  (Sequential / Layered / LastWriteWins). Branches and USD-style layers are both
  Streams under different policies.
- **JournalState** is the projected state computed by replaying entries from one
  or more streams under a Composition policy (lazy; foundation = Sequential
  only).
- **ChangeSets** are optional atomic groups (transaction-style undo units).
- **Markers** are user-named milestones (Onshape Versions, git tags, SysML v2
  named Versions). **Branches** are mutable named refs to entries on a stream.

Domains (Modelica, USD, SysML v2, Python, …) plug in by implementing `OpPayload`
for their op type and emitting op+inverse pairs through a `JournalSink`. The
journal is generic; domains know nothing of it beyond that.

## Key types

`Journal`, `JournalEntry`, `EntryKind` / `LifecycleKind` / `EntryCategory`,
`EntrySummary`, `AuthorId` / `AuthorTag`, `TwinId`, `DomainKind`, `EntityRef`,
`EntryId`, `LamportClock`.

## Status

Foundation: **in-memory, single `main` Sequential stream, single user**. The
schema is shaped so multi-stream / multi-author / `Layered` USD composition /
`yrs` CRDT backend slot in without API changes. The Bevy wrapper
(`JournalResource` / `BevyJournalSink`) and lifecycle observers live in
`lunco-doc-bevy`.

Not a runtime telemetry pipe — telemetry stays on Bevy events.
