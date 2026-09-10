# lunco-doc-bevy

Bevy integration for the LunCoSim Document System.

## What This Crate Does

This crate provides the ECS-facing half of the Document System, adding generic document-lifecycle events and the **TwinJournal** (session-wide change log).

- **Document Lifecycle Events** — Fires `DocumentOpened`, `DocumentChanged`, `DocumentSaved`, and `DocumentClosed` triggers.
- **TwinJournal** — An append-only log of every document event in the session, useful for audit, replay, and diagnostics.
- **Command-Based Mutation** — Standardizes document edits via Bevy `Command` observers (Undo, Redo, Save, Close).
- **Intent-to-Command Resolution** — Maps abstract `EditorIntent` (e.g., `Undo`) to concrete document actions based on the active domain.
- **Keybindings** — Standard IDE-style keyboard shortcuts (Ctrl+S, Ctrl+Z, etc.) for document operations.

## Architecture

`lunco-doc` (the core crate) is pure data. `lunco-doc-bevy` provides the integration layer.

```
lunco-doc-bevy/
  ├── TwinJournal      — Append-only timeline of session events
  ├── EventOrigin      — Distinguishes Local vs Remote vs Replay sources
  ├── EditorIntent     — Domain-agnostic user actions (Undo, Save, Compile)
  ├── Keybindings      — Resource mapping KeyChords to EditorIntents
  └── observer_paths   — Lifecycle observers that populate the journal
```

### The Architectural Rule

**Documents are mutated only through `#[Command]` observers.** This ensures that:
1. Undo/redo works everywhere for free.
2. Scripting, API, and keyboard shortcuts share the same execution path.
3. The Twin journal is a complete, authoritative record of session changes.

## Usage

```rust
app.add_plugins((
    EditorIntentPlugin,
    TwinJournalPlugin,
));

// Request an undo on a specific document
commands.trigger(UndoDocument { doc_id: my_doc_id });
```

## See Also

- `lunco-doc` — The dependency-free data model for documents and operations.
- `lunco-ui` — Uses these events to refresh editors and diagram canvases.
