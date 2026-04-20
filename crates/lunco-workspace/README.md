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

**Key idea — Twin is a *view*, not a container.** All open documents
live in the Workspace. A Twin doesn't own a list; it answers "does
this document belong to me?" by checking whether the doc's storage
handle lies under its folder (or has been context-pinned to it while
Untitled). Opening an Untitled scratch doc while the Rover Twin is
active puts the doc in the Workspace with `context_twin =
Some(rover_id)`; on Save into `rover-twin/models/`, path ownership
takes over and the pin becomes irrelevant. One code path, no
ceremonial moves.

## Types

- **`Workspace`** — root session type. Methods: `add_twin`,
  `close_twin`, `twins()`, `twin(id)`, `add_document`,
  `close_document`, `documents()`, `document(id)`, `twin_for(entry)`,
  `documents_in_twin(id)`, `loose_documents()`. Active pointers for
  Twin / Document / Perspective are plain optional fields on the
  struct.
- **`TwinId(u64)`** — Workspace-minted stable id. `0` is the
  "unassigned" sentinel; actual ids start at 1.
- **`DocumentEntry`** — `{ id, kind, origin, context_twin, title }`.
  Workspace-level metadata only; the parsed source + ops + undo stack
  live in domain registries (e.g. `ModelicaDocumentRegistry`).
- **`Recents`** — bounded lists (10 twin folders, 20 loose files),
  most-recent-first, dedupe-on-push.

## Twin-document association rule

When asked "which Twin claims this doc?":

1. If the doc's origin is `File { path }` and `path` lies under any
   registered Twin's folder, return the **deepest** matching Twin
   (sub-Twins win over the enclosing Twin — matches the "nearest
   `twin.toml`" rule).
2. Otherwise, if the doc is `Untitled { context: Some(id) }`, return
   that pinned Twin.
3. Otherwise, the doc is **loose** — shown under a "Loose" group in
   the Twin Browser.

```rust
match workspace.twin_for(entry) {
    Some(id) => println!("claimed by twin {}", id.raw()),
    None     => println!("loose doc"),
}
```

## Save flow uses the Workspace for defaults

`Save As` on an Untitled with `context_twin = Some(id)` pre-fills the
picker at that Twin's folder root, so scratch docs land inside the
project the user is working on without them having to navigate. After
the save, the doc's origin becomes `File { path, writable: true }`
and the `context_twin` pin becomes advisory (path ownership is
stronger).

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
