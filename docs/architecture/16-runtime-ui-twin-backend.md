# 16 — Runtime: UI ↔ Twin ↔ Language Backend

> Status: Design · Audience: contributors converging the UI ↔ Twin ↔ backend responsiveness model
>
> ⚠️ **NORTH-STAR / ASPIRATIONAL DESIGN — not the current implementation.**
> The named types in this doc — `LanguageBackend`, `EmbeddedBackend` /
> `RemoteBackend`, `Snapshot`, and `Aspect` — **do not exist in the codebase.**
> The responsiveness problem it addresses is real and is solved today by
> concrete, narrower machinery: `lunco-modelica/src/worker.rs` (off-thread
> compile/solve) and the unified diagnostics substrate
> (`lunco-doc` `Diagnostic`/`CompileState`, `lunco-doc-bevy`'s
> `DocumentDiagnostics` resource + `status_json`). Read this as the target
> model the system is converging toward, not a map of existing code.

How the three runtime layers fit together, and how the system stays
responsive when the language backend is slow.

This doc builds on:
- [`01-ontology.md`](01-ontology.md) — `Command` / `Action` / `ControlStream` definitions.
- [`11-workbench.md`](11-workbench.md) — UI shell, panels, perspectives.
- [`13-twin-and-workflow.md`](13-twin-and-workflow.md) — Twin as runtime
  Resource, command queue, transports.

It introduces three additions on top of those:

1. **`Snapshot` + `Aspect` model** — the read side of the Twin, missing
   from `13-twin-and-workflow.md`.
2. **`LanguageBackend` trait** — abstracts the language layer
   (rumoca, future pyright, future SysML) behind a single Twin-facing
   surface; both *embedded* (in-process LSP handlers) and *remote*
   (subprocess / WebSocket) implementations satisfy it.
3. **Stale-UI taxonomy** — five distinct kinds of staleness with
   explicit handling rules. The system is structurally incapable of
   stalling the UI thread regardless of backend speed.

---

## 1. The three layers

```
┌─────────────────────────────────────────────────────────────────┐
│  UI LAYER  (lunco-ui, lunco-workbench, panel crates)            │
│  • Translates raw input → UserIntent → Command/ControlStream     │
│  • Reads Snapshot synchronously, never awaits                    │
│  • Owns rendering only — no parsers, no caches, no I/O           │
│  • Subscribes to per-Aspect change events                        │
└─────────────────────────────────────────────────────────────────┘
        │   Command + client_seq           ▲   Snapshot (Arc, per-Aspect gens)
        │   ControlStream sample           │   AspectChanged events
        │   Action.start / Action.cancel   │   ActionStateChanged events
        ▼                                  │   CommandResponse (ack/nack/discarded)
┌─────────────────────────────────────────────────────────────────┐
│  TWIN LAYER  (lunco-twin)                                        │
│  • Owns: source-of-truth handle, interaction state,              │
│    Snapshot cache, Action registry, Command queue,               │
│    ControlStream slots, reconciliation policy                    │
│  • Single-flights work by (twin, aspect, generation)             │
│  • Applies optimistic edits, reconciles authoritative ones       │
│  • Local Twin OR OptimisticTwin wrapping a RemoteTwin            │
└─────────────────────────────────────────────────────────────────┘
        │   handler call                              ▲   typed result + epoch tag
        ▼                                              │
┌─────────────────────────────────────────────────────────────────┐
│  LANGUAGE BACKEND  (LanguageBackend trait)                       │
│  • EmbeddedBackend: rumoca-tool-lsp handlers without `server`    │
│    feature, runs on AsyncComputeTaskPool                         │
│  • RemoteBackend: tower-lsp client over stdio / WebSocket        │
│  • Owns: Session per doc, mutation epoch, MSL class cache,       │
│    parser, type-checker, projection compiler                     │
│  • Same crate powers VS Code extension and the in-workbench LSP  │
└─────────────────────────────────────────────────────────────────┘
```

### Crate boundaries (enforced by dependency, lint by CI)

- `lunco-ui`, `lunco-workbench`, panel crates → depend on `lunco-twin`
  only. **No** rumoca, **no** parol, **no** `parse_*` calls.
- `lunco-twin` → depends on `lunco-language-backend` (trait + types) and
  on `Storage` / Snapshot / event types.
- `lunco-language-backend` → trait + payload types only; no rumoca.
- `lunco-language-backend-rumoca` → implements the trait via
  `rumoca-tool-lsp` handlers without the `server` feature. **The only
  workbench-side crate that imports rumoca.**
- Future: `lunco-language-backend-pyright`,
  `lunco-language-backend-sysml`, etc. follow the same shape.

CI lint: `grep -r 'rumoca\|parol\|parse_to_syntax' crates/lunco-ui
crates/lunco-workbench crates/lunco-modelica/src/ui/` returns zero.

## 2. Channels between the layers

Five distinct channel kinds, kept separate end-to-end so they never
queue behind each other.

### Down (UI → Twin → Backend)

| Channel | Purpose | Reliability | Ordering | Example |
|---|---|---|---|---|
| **Command** | discrete state mutation | reliable, ack'd | total per-twin | `MoveComponent`, `Rename`, `OpenExample` |
| **Action.start / Action.cancel** | long-work lifecycle control | reliable | per-Action | `BuildProjection`, `RunSimulation` |
| **ControlStream sample** | continuous setpoint | best-effort | latest-wins | joystick axes, slider scrub |
| **Snapshot read** | cheap synchronous (Arc clone) | n/a | n/a | every UI render |
| **Subscribe(Aspect)** | "wake me when X changes" | reliable | n/a | "rerender when AST gen advances" |

### Up (Backend → Twin → UI)

| Channel | Purpose | When emitted |
|---|---|---|
| **CommandResponse** | ack / nack / discarded with reason | every Command |
| **AspectChanged(aspect, gen)** | "this aspect of the snapshot just advanced" | when worker completes |
| **ActionStateChanged** | Started / Running / Progress / Completed / Cancelled / Preempted / Failed | every transition |
| **Snapshot patch** | new immutable `Arc<Snapshot>` | atomic with AspectChanged |

## 3. Aspects — the read side of the Twin

Every Twin's `Snapshot` is a tuple of independently-versioned
**Aspects**. Each aspect carries:

- An immutable `Arc<T>` payload of the aspect-specific data.
- A monotonic `generation: u64` counter.
- (Optional) a `produced_from: Vec<(Aspect, generation)>` dependency
  tag, recording which upstream aspect generations this one was
  computed from.

`twin.snapshot() -> Arc<Snapshot>` is cheap (Arc clone). UI panels
never await; they read the current snapshot and render. Bevy
`Changed<TwinSnapshot>` plus per-aspect change events drive
re-rendering.

### Aspect catalogue and dependency DAG

```
Source ──► Ast ──► Symbols
                 ├► Diagnostics
                 ├► Projection ──► Diagram (rendered shapes)
                 └► HoverIndex

Selection         (interaction-only)
HoverTarget       (interaction-only)
DragInProgress    (interaction-only)
Viewport          (interaction-only)
Presence          (interaction-only, shared in multiplayer)
Values            (sim runtime only, separate pipeline)
```

Two classes of Aspects:

**Backend-derived** — `Source`, `Ast`, `Symbols`, `Diagnostics`,
`Projection`, `HoverIndex`, `Values`. Produced by the language
backend or simulator. Expensive. Refreshed by Actions running on
worker threads. Single-flighted per (twin, aspect, generation).

**Interaction-only** — `Selection`, `HoverTarget`, `DragInProgress`,
`Viewport`, `Presence`. Owned by the Twin, mutated synchronously on
input, never depend on backend availability. **This is what makes
selection / drag / viewport survive parser stalls.**

### Stable IDs are mandatory

Backend-derived aspects reference entities by **stable identifier**
(qualified component name path, hash-based id, etc.) — never by AST
node ref or generation-bound pointer. Interaction-only aspects key
their references by the same stable id. Net effect: an edit that
triggers a reparse does not invalidate the user's selection — the
selection re-resolves by id against the new AST. If the id no longer
exists, the selection transitions to **dangling** (rendered greyed,
labelled "$name (no longer in source)"); it is not blanked.

## 4. The `LanguageBackend` trait

```text
trait LanguageBackend {
    fn open(uri, source)            -> SessionHandle
    fn change(session, edit, epoch) -> ()         // returns immediately
    fn close(session)               -> ()

    // Async aspect producers, called by Twin's Action runner.
    // Each result carries the mutation_epoch it was computed against;
    // Twin discards results whose epoch is stale.
    async fn parse(session)         -> (Ast, epoch)
    async fn diagnose(session)      -> (Diagnostics, epoch)
    async fn symbols(session)       -> (Symbols, epoch)
    async fn project(session)       -> (Projection, epoch)
    async fn hover_index(session)   -> (HoverIndex, epoch)

    // Custom requests for workbench-specific reads.
    async fn instantiate(session, qualified_name)
                                    -> (Component, epoch)
}
```

Two implementations:

- **`EmbeddedBackend`** — owns a `rumoca_session::Session`; runs
  handler functions from `rumoca-tool-lsp` (built **without** the
  `server` feature) on the workbench's `AsyncComputeTaskPool`.
  Returns results via channels that bump Twin snapshot generations.
  **Default on native; the only option in WASM.**
- **`RemoteBackend`** — tower-lsp client over stdio (subprocess) or
  WebSocket (hosted twin / multiplayer). Used when fault isolation,
  version pinning, or a server-side language layer is needed.

The Twin holds `Box<dyn LanguageBackend>`; UI never knows which is
under it.

### `LanguageBackend` requirements for embedded use

- All handler I/O must go through the workspace's `Storage` trait
  ([`01-ontology.md` §4f](01-ontology.md#4f-storage-concepts-lunco-storage)),
  not `std::fs` directly — required for WASM.
- Time queries must use `web-time::Instant`, not
  `std::time::Instant` — required for WASM.
- Threading primitives must be channel-based, not OS-mutex-based —
  required for WASM.
- Every handler call from Twin is wrapped in `catch_unwind` so a
  parser panic doesn't take down the workbench.

## 5. Stale-UI taxonomy

Five distinct kinds of "stale". Every panel rendering anything must
know which kind it is dealing with and apply the corresponding rule.
There is **never** a synchronous wait or blocking spinner; the UI
thread always renders the best available data immediately.

### Class 1 — Optimistic-applied, authoritative-pending

User did something local; Twin updated the relevant interaction-aspect
immediately; backend has not yet caught up.

- Render the optimistic state plainly.
- **No spinner**, no staleness indicator.
- When the backend confirms, the generation bump usually causes no
  visible change because the prediction was right.
- Examples: `MoveComponent`, `SetParameter`, `Select`, viewport pan.

### Class 2 — Backend-derived aspect is N generations behind source

Source advanced (user typed); the AST gen is still at older version
because the parse Action has not completed yet.

- Render the **last-known-good** AST-derived data — diagram,
  diagnostics, hover index — with a **subtle "refreshing" affordance**
  in the panel header (faint indicator strip, not a dialog, not a
  spinner over content).
- Selection and viewport use stable IDs from the previous AST; they
  stay put.
- When the new generation lands, content updates; if a selected id no
  longer exists, transition to **dangling-selected** (greyed in
  selection, labelled in inspector). Never blank the panel.

### Class 3 — Backend-derived aspect failed to refresh

Parse error, cache miss, backend crashed, network down (remote).

- Render last-known-good data **without** the refreshing indicator
  (it isn't refreshing — it's stuck).
- Show a **persistent but unobtrusive error chip** in the panel
  header with click-for-detail.
- The user can keep working with the stale data. Often that's exactly
  what they want — "I just broke the parse, let me keep editing to
  fix it".
- Don't auto-retry on a tight loop; back off exponentially. Re-promote
  to "refreshing" the next time source actually changes.

### Class 4 — Optimistic prediction was wrong (mispredict)

Rare. Optimistic edit was applied; authoritative result disagrees
materially.

- For **LWW** policies (most cases — position, view mode, selection):
  silently snap to authoritative state, no toast. The visual jump
  is a frame or two of a few pixels; users barely notice.
- For **Validate** policies (rename, delete, structural restructure):
  roll forward into a **discarded toast** ("$op was rejected:
  $reason").
- **Never animate state backwards.** If subsequent edits depended on
  a discarded one, mark them discarded together and roll their toasts
  up into one.
- Per-Command merge policies are declared at type level — see §7.

### Class 5 — In-flight Action exceeds soft deadline

Parse / projection / simulation Action is `Running` past a soft
threshold (e.g., 500 ms).

- **Don't block.** Show a **per-aspect progress chip** in the affected
  panel header — never an overlay, never a modal.
- Provide a **Cancel** button that emits `Action.cancel`; Twin
  Preempts the Action. The next Action for the same (twin, aspect)
  will single-flight onto the new request.
- If the Action completes before the user reads the chip, fade it out
  silently. If it fails, transition to handling Class 3.

## 6. Lifecycle walk-throughs

### A. Single edit (typing a character) — single-user, local

```
1. UI: keystroke → CodeEditor panel
2. UI: emit Command::ReplaceSourceRange { client_seq=N }
3. Twin: apply to Source aspect immediately, bump Source gen
4. Twin: emit AspectChanged(Source, new_gen)
5. Twin: ack Command (CommandResponse::Accepted, seq=N)
6. UI: CodeEditor re-renders text; nothing dependent visible yet
7. Twin: schedule Action::Reparse for (twin, Ast, source_gen=new)
   — single-flighted: if a previous Reparse is in-flight at older
     gen, Preempt it first
8. Backend (off-thread): runs handlers.parse(source) on
   AsyncComputeTaskPool
9. Backend → Twin: ParseResult, mutation_epoch matches → accept
10. Twin: install new Ast Arc, bump Ast gen
11. Twin: emit AspectChanged(Ast, new_gen) → cascades to
    Diagnostics, Projection
12. UI: panels subscribed to Ast/Diagnostics/Projection re-resolve
    their stable ids against new gen, render. Selection survives if
    id still exists.
```

Main-thread work in this lifecycle: keystroke handling, Command
emission, CodeEditor text rendering. Total: under 1 ms. Everything
else off-thread.

### B. Single edit — remote / multi-user

Differences from A:

- Step 5: Twin (optimistic local) acks immediately AND forwards
  Command to RemoteTwin.
- Authoritative ack arrives async: `CommandResponse::Accepted(server_seq=M)`
  or `Discarded(reason)`.
- If `Accepted`: optimistic state matches authoritative; nothing
  visible changes.
- If `Discarded`: roll forward into toast (Class 4).
- If another client edited concurrently: server linearizes, rebroadcasts
  the chosen ordering as snapshot patches; all clients converge.
  LWW → silent reconciliation; Validate → toast.

### C. Selecting a node by clicking on the canvas

```
1. UI: click → CanvasPanel computes hit
   (which qualified-name was clicked)
2. UI: emit Command::Select { qualified_name }
3. Twin: update Selection aspect (interaction-only) synchronously
4. Twin: bump Selection gen, emit AspectChanged(Selection, new_gen)
5. UI: Inspector and Canvas, both subscribed to Selection,
   re-render immediately. Inspector shows component data from
   current Snapshot's Ast aspect.
```

Selection is instant regardless of parse state. If Ast aspect is
stale (Class 2), Inspector renders from the last-known-good Ast and
keys lookup by qualified_name. If the name no longer exists in the
new Ast, Inspector shows dangling state, not blank.

### D. Continuous drag

```
1. UI: pointer-down on node → emit Command::BeginDrag { node_id }
   (Twin marks DragInProgress, holds soft-lock on the node in
    multiplayer)
2. UI: pointer-move events → publish ControlStream::DragDelta
   { dx, dy } at native rate (60 Hz typical). NOT a Command.
3. Twin: ControlStream slot updates last_sample. Drag system on Twin
   reads latest sample at fixed tick (60 Hz), applies to
   interaction-aspect transform of the node, bumps Diagram-overlay
   gen (cheap, no Backend call).
4. UI: Canvas re-renders the node at the predicted position
   every frame.
5. UI: pointer-up → emit Command::CommitDrag { node_id, final_pos }
6. Twin: convert to Command::ReplaceSourceRange that updates the
   annotation in source (the authoritative edit). Cycle A from there.
```

The drag never round-trips through the Backend. Only the commit does.
Drag stays at 60 fps even if the parser is currently chewing on a
1.5 s reparse.

### E. Multiplayer realtime (two users dragging different nodes)

Same as D for each user, plus:

- Each user's `BeginDrag` Command goes to authoritative server Twin,
  is ack'd, and broadcast as Presence aspect change. Other clients
  render "$user is dragging $node" (greyed for them).
- Drag deltas go via ControlStream — best-effort, lossy, latest-wins
  — broadcast to other clients as Presence updates (low-priority,
  separate from authoritative state).
- `CommitDrag` becomes the authoritative edit; server linearizes;
  all clients reconcile via cycle B.
- If two users grab the same node: soft-lock on first `BeginDrag`
  rejects the second with a quiet toast ("$user is dragging this").

## 7. Per-Command merge policies

Every Command type declares its conflict-resolution policy at type
level. Reviewed in code, enforced by the dispatcher. Three policies:

| Policy | Behaviour | Example Commands |
|---|---|---|
| `LWW` | Last-arrival-at-server wins; loser silently re-snaps | `MoveComponent`, `SetViewMode`, `SetParameter`, viewport ops, `Select` |
| `Validate` | Server checks preconditions; on failure, forward toast with reason | `Rename`, `Delete`, structural restructure |
| `Commute` | Operations always succeed and commute trivially | `AddComponent`, `Connect`, additive ops |

Default is `Validate` (conservative); downgrade to `LWW` per Command
as confidence grows. Reach for CRDT merge only on text aspects (Yjs
or Automerge for the source-text CRDT) — see [Concurrency](#8-concurrency-model).

## 8. Concurrency model

For collaborative editing, this design follows the **Notion
structural-ops + Yjs text** model rather than full OT or CRDT
everywhere:

- **Source text** (inside a document) → text CRDT (Yjs / Automerge)
  for the `Source` aspect. Concurrent typing in the same `.mo` file
  merges automatically.
- **Structural Commands** on the Twin → server-linearized + LWW per
  attribute. Notion-style. Fast, predictable, lossy on conflict.
- **ControlStream / live signals** → last-sample-wins, no merge.
  Two operators on the joystick: server picks one (priority / role)
  or sums (per stream).
- **Validation-sensitive Commands** → validate-then-apply with forward
  toast on rejection. Never animate state backwards.

The single design rule: **don't roll back, re-converge.** Visible
rollback of state the user already saw is the worst UX in
collaborative editing.

## 9. Failure-mode degradation

| Failure | Layer that detects | What UI shows | What still works |
|---|---|---|---|
| Backend hung / slow | Twin (Action soft-deadline) | progress chip on affected panel | everything except the affected aspect; user can keep typing/navigating |
| Backend crashed (embedded) | Twin (`catch_unwind` boundary on handler call) | error chip on affected panel | everything except that aspect; Twin restarts session in background; user can keep editing |
| Backend disconnected (remote) | Twin (transport heartbeat) | "Disconnected" badge in window chrome | local interaction (Selection, Drag, Viewport) using last-known Snapshot; queued Commands wait for reconnect with exponential backoff |
| Optimistic command discarded | Twin (`CommandResponse::Discarded`) | forward toast with reason | everything; subsequent edits aren't blocked |
| Mispredict on LWW aspect | Twin (snapshot delta) | silent re-snap | everything |
| ControlStream sample missing > timeout | Twin (per-stream watchdog) | applies fallback policy (`hold_last` / `decay_to_zero` / `fail_safe`) | everything; user notices control input went idle |
| Source-of-truth lost (storage error) | Twin (Storage trait) | error toast + read-only mode for that doc | other twins/docs unaffected |

## 10. Why this resists the original problem class

- **No UI panel can call a parser.** Crate boundary enforces it.
- **No backend call runs on the main thread.** Twin guarantees it via
  worker pool.
- **No expensive work runs twice for the same input.** Single-flight
  per (twin, aspect, generation).
- **No selection / drag / viewport waits on a parse.** Interaction
  aspects are independent of backend aspects.
- **No mispredict yanks state backwards.** LWW silent or forward-toast
  for validation rejects.
- **No remote / embedded distinction visible to UI.** Same Twin trait,
  transport differs.
- **No WASM blocker.** Embedded backend builds for `wasm32-unknown-unknown`;
  same code as native.
- **No multi-language rewrite.** Each domain backend implements
  `LanguageBackend`; UI doesn't notice.

## 11. Implementation staging

1. **`LanguageBackend` trait + `EmbeddedBackend` over `rumoca-tool-lsp`
   handlers.** Verify WASM clean build of the handlers crate before
   committing the trait shape.
2. **Twin Snapshot / Aspect / Action infrastructure** in `lunco-twin`.
   Migrate panels to read from Twin only; CI lint forbids
   reach-around imports.
3. **`client_seq` + per-Command merge policies + `ControlStream`
   channel.** Single-user behaviour unchanged; protocol-shape ready
   for prediction and multiplayer.
4. **`RemoteBackend` (tower-lsp client) for hosted-twin and
   multiplayer scenarios.** Pipeline-swap (full reparse → AST patch in
   rumoca) is invisible to the workbench.

Stage 1 alone removes the synchronous main-thread parse stalls
documented in [`perf-parol-trace-overhead.md` over in
`../rumoca`](../../../rumoca/docs/design-notes/perf-parol-trace-overhead.md);
the parol Debug::fmt overhead becomes a backend-internal concern,
not a UI-blocking one.
