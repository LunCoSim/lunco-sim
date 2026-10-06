# lunco-workspace

**LunCoSim's editor session — the VS Code-Workspace analog.**

A Workspace is what's open *right now in this window*: a set of
[`Twin`](../lunco-twin/README.md)s brought in from anywhere on disk (or
a remote URL), every open `Document` (including Untitled scratch
buffers and loose files outside any Twin), which tab and which Twin
are active, the chosen Perspective, and a bounded recents list.

Headless, UI-free, ECS-free. A Bevy `Resource` wrapper
(`WorkspaceResource`) lives in `lunco-workbench` so the core type
stays reusable from headless CI and API-only servers.

## Ontology at a glance

```text
┌─────────────────────────────────────────────────────────┐
│  Workspace — "what I'm editing right now"               │
│                                                         │
│    active_twin ─────┐                                   │
│    active_document ─┼──┐                                │
│    active_perspective                                   │
│    recents                                              │
│                    │  │                                 │
│   ┌────────────────┘  │                                 │
│   ▼                   ▼                                 │
│  Twin(s)           Document(s)                          │
│  (simulation        (open files + Untitled)             │
│   units — file                                          │
│   system scope)                                         │
└─────────────────────────────────────────────────────────┘
```

**Twin folders provide the authoring and display lens.** All open documents
live in the Workspace. `twin_for` answers folder association; each document's
`runtime_context` records its exact admitted source lifetime independently.
Scratch creation and file loading register that lifetime before async work or
deferred lifecycle delivery. Saving changes the authored origin without
transferring resident source to another session. Only a clean explicit file
reopen can install newly read source and its newly admitted owner. Active scope
and teardown use the stored runtime owner.

## Types

- **`Workspace`** — root session type. Methods: `add_twin`,
  `close_twin`, `twins()`, `twin(id)`, `add_document`,
  `close_document`, `documents()`, `document(id)`, `twin_for(entry)`,
  `documents_in_twin(id)`, `loose_documents()`. Active pointers for
  Twin / Document / Perspective are plain optional fields on the
  struct.
- **`TwinId(u64)`** — Workspace-minted stable id. `0` is the
  "unassigned" sentinel; actual ids start at 1.
- **`DocumentEntry`** — `{ id, kind, origin, runtime_context, title, dirty }`.
  Workspace-level metadata only; the parsed source + ops + undo stack
  live in generic domain registries (e.g. `DocumentRegistry<ModelicaDocument>`).
  Domain
  registries mirror the authoritative dirty state on document events.
- **`Recents`** — bounded lists (10 twin folders, 20 loose files),
  most-recent-first, deduplicated by canonical filesystem identity. Existing
  aliases are cleaned at startup; missing entries use lexical normalization
  and remain reopenable.

## Twin-document association rule

When asked "which Twin claims this doc?":

1. If the doc's origin is `File { path }` and `path` lies under any
   registered Twin's folder, return the **deepest** matching Twin
   (sub-Twins win over the enclosing Twin — matches the "nearest
   `twin.toml`" rule).
2. Otherwise, a `LocalTwin(id)` runtime context returns that pinned Twin.
3. An explicit replicated context retains its exact connection, host mount,
   authority, and root. It does not fabricate a local Workspace Twin id.
4. An application context is **loose** — shown under a "Loose" group in
   the Twin Browser.

```rust
match workspace.twin_for(entry) {
    Some(id) => println!("claimed by twin {}", id.raw()),
    None     => println!("loose doc"),
}
```

## Save flow uses the Workspace for defaults

`Save As` on an Untitled with a `LocalTwin(id)` runtime context pre-fills the
picker at that Twin's folder root, so scratch docs land inside the
project the user is working on without them having to navigate. After
the save, the doc's origin becomes `File { path, writable: true }`
and the local runtime context becomes advisory (path ownership is stronger).

`PinnedDocumentRuntimeOwner` captures document identity and one exact
`DocumentRuntimeOwner` when work is admitted. Workspace owns document context
and local Twin lenses; core-session supplies current replicated lifetime facts
from its existing connection and scene resources. Every reader validates the
pin before consuming async results. A retained document does not transfer
already admitted work to a new owner when its scene closes or is replaced.

## What's not here

- Manifest (`.lunco-workspace` on-disk format).
- Hot-exit (serialising unsaved buffers across restarts).
- External-change watcher.
- Manifest's `active_perspective` persistence.

Those land in follow-up milestones; the surface above is stable
and unit-tested.

## Related

- [`lunco-twin`](../lunco-twin/README.md) — the per-Twin folder +
  manifest + `owns()` predicate.
- [`lunco-doc`](../lunco-doc/README.md) — the `Document` trait,
  `DocumentId`, `DocumentOrigin`.
- [`lunco-storage`](../lunco-storage/README.md) — the I/O trait the
  Workspace goes through to read/write docs.
- [`lunco-workbench`](../lunco-workbench/README.md) — hosts
  `WorkspaceResource` (the Bevy `Resource` wrapper) + events
  (`RegisterDocument`, `TwinAdded`, `DocumentOpened`, …).
