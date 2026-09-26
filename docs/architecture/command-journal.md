# Command Journal — authored mutations and session replay inputs

> Status: Design · Audience: contributors adding new domain mutations
>
> This page covers the future command/session journal. The current authored
> document journal is defined in [`18-unified-journal-and-history.md`](18-unified-journal-and-history.md).

`#[Command]` execution is not currently journaled. Runtime actions such as
`SpawnEntity`, `AcquireControl`, `SetPorts`, terrain spawning, and time control
remain transient; deterministic session replay is therefore not built.

The Twin journal owns authored document mutations. A separate session replay
input stream must own transient external inputs such as per-tick controls and
runtime commands. Telemetry remains observational output, not an input source.
Recording every control change as an authored journal operation would produce
an unusable history and could double-apply networked commands.

The design below defines those constraints and the intended adoption path. It
does not describe command journaling as shipped.

## The thesis

Authored document edits and runtime commands both change what a user sees, but
they have different lifecycles. Some mutations use `#[Command]`; authored USD
operations use the Twin journal, while session inputs need tick-scoped capture.
Command types are serializable, but they do not share one ingress: transport
requests use the API dispatcher, while UI and subsystem code can trigger typed
command events directly. Any session recorder must attach at owners that cover
the supported external inputs; observing only the API dispatcher would miss
direct typed events.

The table below describes what an authored document operation inherits from the
Twin journal. Runtime inputs such as possession, time transport, and controls
have different lifetimes and need a session replay owner.

| Capability | Where it comes from |
|---|---|
| **Stable identity** | the op's `EntryId` (monotonic per author, collision-free across authors) |
| **Ordering** | the log order / `LamportClock` (causal across peers) |
| **Undo / redo** | the op's recorded **inverse** (`record_op` stores both) |
| **Multi-peer sync** | the journal-merge plane replicates entries (`merged_order`) |
| **Persistence / audit** | the journal serializes (`to_bytes`) and is human-readable |
| **Authored-state reconstruction** | replay authored document operations and their recorded inputs |

This shared journal is the owner for authored document history. Terrain layer
identity and dynamic document edits may use it when they are authored as USD
operations. Runtime spawn, possession, and control behavior need an explicit
session-input contract; they do not become document operations merely because
they are exposed as commands or tools.

## Existing substrate

The current document-journal implementation and ownership boundaries are
defined in [`18-unified-journal-and-history.md`](18-unified-journal-and-history.md).
Authored document operations reuse the journal's `EntryId`, inverse,
change-set, and merge machinery. The transient session-input stream has a
different record shape and lifecycle; it is not a second authored journal.

The typed command surface has no universal ingress. The audited runtime paths
are:

| Producer | Current path | Replay implication |
|---|---|---|
| HTTP, MCP, and Rhai command calls | `ApiCommandEvent` → `api_command_dispatcher` → typed command event | The API dispatcher sees these calls, but not direct typed triggers. |
| UI and Rust subsystem systems | Direct typed command events via Bevy `Commands` | Capture cannot be attached only to the API dispatcher. |
| Keyboard/gamepad vessel control | Bevy input state → `drive_from_bindings` in `FixedUpdate` → `SetPorts` before `ControlDacSet` | The effective port writes and `SimTick` are known at the fixed-step producer. Record the consumed semantic frame, not device events. |
| Networked vessel control | Wire input → `SetPorts`; remote frames are consumed by `GlobalEntityId` and per-vessel sequence order at the fixed simulation step | `InputFrame` and `OwnedInputLog` serve one-vessel prediction rollback and acknowledgement. They are not a whole-session log. |
| Scheduled Rhai and hook behavior | Evaluated in the caller's declared cycle against live simulation state | These outputs are derived behavior. Re-run them from the same state and inputs during replay; do not record them as independent external inputs. |
| Async preparation and owner results | Prepared off-thread, then validated and committed by the owning lifecycle or simulation boundary | Worker completion is not an input. Replay the admitted source revision and deterministic commit order, not completion timing. |

`CommandOccurred` projects only the command type name; it does not retain
parameters, target, origin, scene generation, tick, or sequence. A generic
event observer therefore does not by itself provide replay data, even though it
sees typed command events from direct and API paths.

The session stream must capture external authoritative inputs at the boundary
where they become eligible for simulation, before domain projection, with a
scene generation, stable target identity, effective `SimTick`, and stable
per-tick sequence. This is later than UI/API request arrival when a request is
assigned to a simulation tick. High-rate controls should be semantic per-tick
frames; discrete actions should retain their typed action and target. Capture
must distinguish external inputs from commands derived by deterministic
Rhai/hooks, since replaying both an input and its derived command would apply
the same effect twice.

The existing Twin journal remains the owner for authored document operations.
It does not record transient controls, scene-time inputs, or physics state and
cannot reproduce a live session by itself. A session replay input stream must
cover those transient inputs without recording authored document edits a
second time. This capture and replay path is not implemented yet. The ingress
inventory rules out treating `api_command_dispatcher`, `CommandOccurred`, or
the network rollback buffer as the whole-session boundary. The implementation
needs an explicit typed simulation-input contract that all supported external
simulation producers can submit to and the fixed-step owner can order and
consume.

## The authored-document model — one write path (record → project)

For a migrated authored document mutation, the journal is the **single source
of truth** and ECS is its **projection**. A document command does not both
record an op and separately mutate ECS; that dual-write creates two truths that
can diverge. Instead it records an op and the document projection applies it.
Local and remote authored document ops take the same path:

```
   authored document edit ─► document ingress ─► authored journal ─► domain projection ─► ECS
   remote authored op ─────► merge plane ──────► authored journal ─► domain projection ─► ECS
   UserIntent/InputFrame ───► session input log at (scene generation, SimTick, sequence)
   deterministic Rhai/hooks ───────────────────────────────────────────────► re-derived
```

The document ingress in this diagram describes the existing Twin-journal
boundary. It does not route every runtime command through that journal. Session
inputs and authored journal operations have separate records and lifecycles.

This gives authored document sync one source of truth: peers merge the same
document operations and project them into their local runtime state. It does
not by itself replay session controls or guarantee whole-simulation sync. A
migrated document command's mutation moves downstream of the journal into its
domain projection. Migration is per operation; an operation is either
imperative and not journaled, or journaled and projected, never both.

- **Op vocabulary = authored document operations.** A USD prim edit or a
  terrain edit authored as a USD prim is a typed document op. Runtime commands
  such as `SpawnEntity` and `AcquireControl` are not document ops by default.
- **Identity = `EntryId` within the authored journal.** A document layer's
  identity may derive from its creating entry. Session entities and inputs keep
  identities owned by their runtime contracts.
- **Undo = the inverse.** Each authored document operation declares its inverse.
  For example, an additive terrain edit may invert to
  `RemoveTerrainLayer { id: EntryId }`. `record_op<O, I>` stores both; undo
  applies the inverse; `ChangeSet` groups a multi-op action into one undo step.
- **Authored document sync = `merged_order`.** Document ops replicate and merge
  through the journal plane. Runtime spawns, possession, and controls need their
  own session or networking contract.
- **Authored-operation replay.** Ops carry their inputs (parameters and seeds);
  replaying `merged_order` reconstructs authored document state. Whole-session
  replay additionally requires the separate tick-stamped session input stream;
  Twin-journal order alone does not reconstruct physics, Modelica, or Rhai state.
- **Projection, and where granularity lives.** A migrated command's observer stops
  mutating ECS; a domain projection applies its ops. Crucially, **the fine-grained
  history lives in the journal, not in ECS**: each brush stroke is its own op
  (`EntryId`, invertible), but they project into **one consolidated layer**, not a new
  ECS layer per stroke. For terrain that is a single `EditsLayer` (a folded
  `SparseEditField` / edit-modifier list) that is the projection of the edit-op
  substream — bounded, re-baked once. Undo reverts an op in the journal and re-projects
  the one layer. The end-state is **state = snapshot + replay(log)**, ECS a pure
  projection membrane — converging with the USD-canonical projection the networking
  branch is building. (Authored layers — `dem`/`craters`/`rocks` USD prims — stay
  distinct, addressed by prim path; only *runtime edits* consolidate into the one layer.)

## Decisions the design must pin down

1. **Which document mutations are journaled.** Select authored mutations, not
   transient view/query commands (`FocusTarget`, reads) or session inputs. Any
   command recorder must observe every supported ingress; the existing API
   dispatcher does not observe direct typed event triggers and cannot be the
   sole recorder.

2. **How the inverse is obtained.** Three tiers: (a) **natural inverse** — additive
   authored ops invert to a remove-by-`EntryId` (for example, terrain edits);
   (b) **computed inverse**
   — `fn inverse(&self, world) -> impl OpPayload` captures the pre-state it overwrites
   (flatten must snapshot the heights it replaced for a *true* undo, vs. the cheap
   "remove the flatten layer" which only pops it); (c) **snapshot/diff** for ops with no
   compact inverse. Start with (a).

3. **Determinism for authored-state reconstruction.** Document ops must be
   self-contained: required seeds and parameters belong in the payload. This
   reconstructs authored state; fixed-step simulation inputs and runtime RNG
   remain part of the separate whole-session contract (spec 020 US3).

4. **Keep record types aligned with their lifecycles.** Authored edits ride the
   USD-document journal and participate in undo, persistence, and merge. External
   runtime inputs use a separate tick-stamped session stream and are not Twin
   journal entries. A session replay implementation must not record deterministic
   Rhai or hook outputs as independent inputs when replay can derive them again.

## The Omniverse pattern: USD + Fabric, two tiers

USD is the source of truth, so authoring edits to it is the **default**. Omniverse
(OpenUSD-native) shows how to do that without paying composition cost in the inner
loop, and we follow it:

- **Editing is authoring layer opinions, never mutating geometry.** An edit authors a
  prim/attribute at an **edit target** (a layer); composition resolves the strongest
  opinion. Non-destructive by construction. The edit target is a *choice*: a
  **runtime/session layer** for ephemeral edits (not saved into the asset), a persistent
  sublayer once promoted, a live/merge layer for collaboration. Our `UsdOp` carries
  `edit_target`, so this is native — terrain edits default to a runtime layer over the
  base DEM, promotable on save.

- **Two tiers, mirroring USD + Fabric.** Omniverse never runs physics/render off
  authored USD; it projects USD into **Fabric** (a flat runtime cache) and PhysX/render
  read *that*, because composition is too heavy per-frame. We do the same:

  | Tier | Omniverse | Ours | Holds |
  |---|---|---|---|
  | **Authoring** | USD layers | USD terrain doc | committed edits as tiny param prims (the truth) |
  | **Runtime** | Fabric | ECS `EditsLayer` + bake | the projection physics/render read |

- **Commit-granularity, never per-frame.** High-frequency interaction (a sculpt drag)
  edits the **runtime projection** live for responsiveness and **authors one USD op on
  commit** (release), exactly as Omniverse edits Fabric during a drag and writes USD on
  mouse-up. A discrete edit (a click-dig) authors immediately. The design's one hard
  rule: **do not author to USD per frame** — that thrashes composition.

- **Prim-per-edit is affordable *because* geometry is never stored.** The height oracle
  keeps USD holding only tiny parameter records (a brush = center/radius/amplitude), not
  meshes — so we author **one prim per edit** (granular, each addressable by prim path =
  its identity, individually undoable) where Omniverse, whose prims carry geometry, often
  cannot. This dissolves the "one layer vs. per-edit" tension: **prim-per-edit is the
  USD authoring tier; the single `EditsLayer` is the runtime projection tier** — both,
  at once. (Authored `dem`/`craters`/`rocks` prims and edit prims are the same kind of
  thing — tiny descriptions; the geometry is always derived, à la Omniverse's procedural
  / OmniGraph terrain, never baked into USD.)

**Tradeoff, stated plainly.** Coupling edits to USD composition + journal is more
machinery than a bespoke ECS edit list, and composition is not free — mitigated by
param-only prims, commit-granularity, and the runtime projection absorbing interaction.
The payoff: undo/redo, authored-state sync, persistence, collaboration, and
audit use the existing USD journal. Authored-state reconstruction comes from
its operation stream; whole-session replay still requires a separate input log
and deterministic runtime contract. Given USD is the standard, this is the
right default; the per-frame-authoring trap is the one thing to forbid.

## Staged adoption (incremental, not a big-bang rewrite)

- **Phase 1 — Terrain gets its USD document; edits are USD doc ops (see the two-tier
  model below).** Terrain layers already *are* USD child prims (`dem`/`craters`/`rocks`),
  so terrain is nearly a document already — give it a `DocumentId` and route edits
  through the **existing** USD doc + journal machinery rather than any bespoke path. A
  committed edit **authors a USD doc op** — one tiny `AddPrim` per edit under an `edits`
  scope, on a **runtime/session edit-target layer** (non-destructive over the base;
  promotable to persistent). The house convention does the rest: `Document::apply`
  mutates and returns the inverse → `JournalOpRecorder` records it (USD domain,
  `EntryId`) → the projection re-parses the terrain doc → `TerrainLayerStack`, folding
  the edit prims into the one `EditsLayer`. **Reuse, not reinvent:** no
  `DomainKind::Terrain`, no `TerrainOp`, no synthetic counter — the USD domain,
  `UsdDocumentRegistry::replay_op`, the auto-recorder bridge, and `EntryId` already
  exist. Record-after-mutate on a **single** authoritative store (the doc; ECS projects)
  — not the divergence-prone dual-write — and a peer's edit syncs by replaying the same
  USD op. It **converges** with the USD-canonical merge (these edits are already Stage
  ops). The interim ECS `EditsLayer` (built now) is exactly the projection target.
- **Phase 2 — Undo/redo.** Apply recorded inverses; a UI undo stack that is just a
  cursor over the journal.

  > **`ChangeSet` grouping is already live for multi-op USD commands.**
  > `lunco_usd_core::commands::ApplyUsdOps` wraps a whole lowering in one
  > `JournalResource::change_set`, so a command that lowers to
  > several `UsdOp`s is **one undo unit**. `AttachComponent` is the canonical user:
  > undo removes the part, its placement, its joint and the joint's anchors
  > *together*.
  >
  > **Why this matters, and why a new multi-op command must use it.** It used to
  > journal one entry per op — so an undo peeled off a single op and left the object
  > **half-attached**. A partially-applied edit that the journal cannot undo as a
  > unit is worse than no undo at all.
  >
  > The complete lowered sequence is validated before the live document is
  > touched. A rejected sequence applies zero ops; a valid sequence is committed
  > as one `DocumentHost` history group and, when a `JournalResource` is present,
  > one journal change set. Headless builds without a journal retain the same
  > all-or-nothing document history, just without the Twin journal entry group.
- **Phase 3 — Authored-state reconstruction.** Replay authored document operations
  and their inputs. Whole-session replay remains separate and requires the
  tick-stamped session input stream described above, plus divergence checks.
- **Phase 4 — Projection-authoritative authored state.** Authored document state
  is reconstructed from snapshots plus journal operations; ECS becomes its pure
  projection, converging with the USD-canonical projection membrane. This does
  not replace the separate session input stream and is not a prerequisite for
  Phases 1–3.

## Interaction ownership

The current runtime and authored paths have different owners and ordering
contracts:

| Interaction | Owner and record | Identity and order |
|---|---|---|
| Dig / raise authored into a Twin | USD document operation in the Twin journal | `EntryId` and merged journal order |
| Flatten pad authored into a Twin | USD document operation in the Twin journal | `EntryId` and merged journal order |
| Spawn a rover during a session | Runtime command; session input capture is not implemented | scene generation, target, `SimTick`, stable sequence |
| Possess during a session | semantic user intent; whole-session capture is not implemented | scene generation, controlled target, `SimTick`, stable sequence |
| USD prim edit | USD document operation in the Twin journal | `EntryId` and merged journal order |

These rows do not all share one lifecycle. The USD prim edit belongs to the
authored document journal. Runtime interactions require session-input or
networking contracts with tick and target identity. Terrain editing uses the
document journal only when the edit is authored as a document operation.

## See also

- [`specs/020-world-state-and-replay`](../../specs/020-world-state-and-replay) —
  the broader world-state and session replay contract; this document covers only
  authored document history and does not implement its full Input Log.
- [`terrain-substrate.md`](terrain-substrate.md) → "Dynamic modification" — terrain
  edits as layers; the LayerId that becomes an `EntryId`.
- `lunco-twin-journal` — the op-log substrate (`record_op`, `EntryId`, `merged_order`).
- `lunco-api::executor::api_command_dispatcher` — transport command ingress; direct
  typed command events also exist and must be included in any replay capture audit.
