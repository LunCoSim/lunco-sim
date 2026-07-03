# Command Journal — one op log for identity, undo, sync, and replay

> **Design (not yet built).** This records how *every* mutating interaction —
> terrain edit, spawn, possess, model edit, USD prim edit — becomes a single kind of
> thing: an **op in one journal**. Identity, ordering, undo/redo, multi-peer sync,
> and deterministic replay then all come from one substrate instead of each feature
> reinventing a sliver of it. It realizes the **Input Log** of
> [`specs/020-world-state-and-replay`](../../specs/020-world-state-and-replay) (User
> Story 3), which is specced but unbuilt.

## The thesis

A terrain dig is not special. Neither is a spawn or a possession. They are all
**mutations of world state expressed as `#[Command]`s** — already serializable,
already dispatched through one bus. The moment you notice that, the bespoke machinery
each feature would otherwise grow — a monotonic id counter here, an undo stack there,
a replication path somewhere else — collapses into one question: *record the command
in the log.* Do that once, at the one place every command already funnels through, and
every interaction inherits:

| Capability | Where it comes from |
|---|---|
| **Stable identity** | the op's `EntryId` (monotonic per author, collision-free across authors) |
| **Ordering** | the log order / `LamportClock` (causal across peers) |
| **Undo / redo** | the op's recorded **inverse** (`record_op` stores both) |
| **Multi-peer sync** | the journal-merge plane replicates entries (`merged_order`) |
| **Persistence / audit** | the journal serializes (`to_bytes`) and is human-readable |
| **Deterministic replay** | replay the op stream + seeds = spec 020's Input Log |

This is the universal answer to three threads that kept recurring — *"several crater
layers,"* *"dynamic tool edits,"* *"per-layer identity"* — and to the tool question
(*"spawn/possess as tools"*). They are one capability: **an addressable op in a shared
journal.**

## Don't rebuild — the substrate already exists

`lunco-twin-journal` is not a sketch; it is a fully-formed op log:

- **`record_op<O: OpPayload, I: OpPayload>(author, doc, &op, &inverse, change_set) ->
  EntryId`** — records a typed op *and its inverse* in one call. Undo is not a feature
  to add; it is the second argument.
- **`EntryId { author, lamport }`** — *"two authors can never produce the same id;
  same author monotonically."* The universal handle.
- **`AuthorTag::for_tool(name)` / `local_user()`** — authorship is first-class, and
  **tools are authors**. A rhai `terrain::dig` is authored by `for_tool("terrain")`.
- **`LamportClock`** — causal ordering across peers.
- **`Stream` / `Branch` / `Composition` / `MergeStrategy` / `merged_order_ids_with(policy)`**
  — the merge plane, with a policy hook (RBAC / merge-policy — see
  the hook architecture).
- **`ChangeSet`** (group ops into one undo unit) and **`Marker`** (named checkpoints).
- **`to_bytes` / `from_bytes`** — persistence, replay input, audit.

And the **command bus** is the other half already in place: `#[Command]` types are
`Serialize + Reflect`, and **every** call path — rhai `cmd(...)`, HTTP `/api`, MCP
`execute_command`, UI — funnels through **one dispatcher**,
`api_command_dispatcher` (`lunco-api::executor`), which reflect-builds the command and
triggers its observer. That single chokepoint is the integration point: record there,
and you have captured *every* interaction, universally, with no per-command wiring.

So the work is **not** a new system. It is: make a mutating `#[Command]` an
`OpPayload` (add `fn domain()`; it is already `Serialize`), and record it at the
dispatcher.

## The model — one write path (record → project), never dual-write

The journal is the **single source of truth**; ECS is its **projection**. A command
does not both record an op *and* separately mutate ECS — that dual-write is two truths
that can diverge and is exactly what makes sync hard. Instead a command **records**;
a **projection** applies the op to ECS. Local and remote ops take the *identical* path:

```
   rhai cmd() ─┐
   HTTP /api  ─┼─►  api_command_dispatcher ─► journal.record_op(author,&op,&inverse) ─► EntryId
   MCP        ─┤                                          │
   UI         ─┘                                          ▼
   remote peer's op ─► merge plane ─► journal ─►  projection applies the op ─► ECS state
```

**This is why single-source makes sync trivial:** replicate the log, and every peer
projects the same log to the same state — no local-vs-remote special case, nothing to
reconcile write-against-write. A migrated command's observer stops mutating ECS
directly; its mutation moves *downstream* of the journal into a domain projection that
reacts to new ops. Migration is **per command** (a command is either not-yet-journaled
and imperative, or journaled and projected — never both), so this is incremental, not a
big-bang, and never dual-writes.

- **Op vocabulary = the commands.** `BrushTerrain`, `FlattenTerrain`, `SpawnEntity`,
  `PossessVessel`, a USD prim edit — each a typed op. No separate op language.
- **Identity = `EntryId`.** A terrain layer's `LayerId` *is* the `EntryId` of the edit
  that created it; a spawned entity traces to its spawn op; etc. One id space.
- **Undo = the inverse.** Each mutating command declares its inverse — often another
  command: the inverse of `BrushTerrain` (which appends layer `EntryId`) is
  `RemoveTerrainLayer { id: EntryId }`. `record_op<O, I>` stores both; undo applies the
  inverse; `ChangeSet` groups a multi-op action into one undo step.
- **Sync = `merged_order`.** Command ops replicate and merge like any entry; peers
  converge on the same op stream (the journal-merge plane), so terrain, spawns, and
  possessions stay consistent without per-feature netcode.
- **Replay = the stream + seeds.** Ops carry their inputs (params, seeds, sim-time);
  replaying `merged_order` reproduces state — spec 020's deterministic Input Log.
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

1. **Which commands are journaled.** Mutations, not transient view/query commands
   (`FocusTarget`, reads). Mark them — a `JournaledCommand` trait or an
   `EntryCategory` on the command — so the dispatcher records mutations only. Author is
   taken from the call context (tool name for scripts, user for UI, peer for remote).

2. **How the inverse is obtained.** Three tiers: (a) **natural inverse** — additive
   ops invert to a remove-by-`EntryId` (terrain edits, spawns); (b) **computed inverse**
   — `fn inverse(&self, world) -> impl OpPayload` captures the pre-state it overwrites
   (flatten must snapshot the heights it replaced for a *true* undo, vs. the cheap
   "remove the flatten layer" which only pops it); (c) **snapshot/diff** for ops with no
   compact inverse. Start with (a).

3. **Determinism for replay.** Ops must be self-contained: seeds, sim-time, and params
   in the payload; no hidden RNG/wall-clock in observers (spec 020 US3: fixed timestep,
   seeded RNG, deterministic ordering). This is a discipline the design imposes on
   journaled commands.

4. **One journal, not two.** The networking branch's canonical design says *edits ride
   the USD-doc-op journal*. This **is** that journal: a USD prim edit and a
   `BrushTerrain` are both `EntryKind` payloads in `lunco-twin-journal`. The command
   journal is not a parallel log to reconcile later — it is the same substrate, and a
   command op can *lower to* a USD-doc op where one exists. The design must forbid a
   second log.

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
The payoff: undo/redo, sync, persistence, collaboration, audit, and replay come free and
**standard**, matching Omniverse and converging with the canonical merge. Given USD is
the standard, this is the right default; the per-frame-authoring trap is the one thing
to forbid.

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
- **Phase 2 — Undo/redo.** Apply recorded inverses; `ChangeSet` grouping; a UI
  undo stack that is just a cursor over the journal.
- **Phase 3 — Replay / determinism.** Seeds + sim-time in ops; replay `merged_order`
  → spec 020's deterministic Input Log; divergence checks.
- **Phase 4 — Projection-authoritative.** State = snapshot + replay(log); ECS becomes a
  pure projection, converging with the USD-canonical projection membrane. This is the
  merge-coupled end-state, not a prerequisite for Phases 1–3.

## What each interaction becomes

| Interaction | Op (`#[Command]`) | Inverse | Author |
|---|---|---|---|
| Dig / raise | `BrushTerrain` | `RemoveTerrainLayer{EntryId}` | `for_tool("terrain")` / user |
| Flatten pad | `FlattenTerrain` | `RemoveTerrainLayer{EntryId}` (or heights snapshot) | as above |
| Spawn a rover | `SpawnEntity` | `Despawn{EntryId}` | user / script |
| Possess | `PossessVessel` | `ReleaseVessel` / prior possession | user |
| USD prim edit | doc op | doc inverse op | user / peer |

Every row is the same shape. That is the point: **the tools, the edits, the identity,
the undo, the sync, and the replay are one system** — the op log — and terrain editing
is simply its first, concrete consumer.

## See also

- [`specs/020-world-state-and-replay`](../../specs/020-world-state-and-replay) — the
  Input Log / deterministic replay this realizes.
- [`terrain-substrate.md`](terrain-substrate.md) → "Dynamic modification" — terrain
  edits as layers; the LayerId that becomes an `EntryId`.
- `lunco-twin-journal` — the op-log substrate (`record_op`, `EntryId`, `merged_order`).
- `lunco-api::executor::api_command_dispatcher` — the one chokepoint all commands pass.
