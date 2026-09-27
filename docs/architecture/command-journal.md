# Command Journal — authored mutations and session replay inputs

> Status: Partial typed input capture with bounded archive export; whole-session replay remains design work · Audience: contributors adding new domain mutations
>
> This page covers the future command/session journal. The current authored
> document journal is defined in [`18-unified-journal-and-history.md`](18-unified-journal-and-history.md).

`#[Command]` dispatch is not journaled as a general session input. A bounded
in-memory `SessionInputStream` captures physical intent frames, admitted
external `SimulateIntent`/`SimulateIntentEdge` payloads, and raw-file runtime
spawns, but it is not a complete session log. Some command owners translate
actions into authored document operations: document-backed `SpawnEntity` uses
`ApplyUsdOps` and the Twin journal, while a raw-file scene admits a fixed-tick
`RuntimeSpawn` and preserves its reserved root identity for `NetSpawn`
replication.
`AcquireControl`, `SetPorts`, terrain spawning, time control, and other
transient runtime actions still lack complete session capture, so deterministic
session replay is not built. A completed capture can be exported as a bounded,
versioned binary archive; baseline state, remaining command inputs, and a
playback consumer are still absent.

The networking-owned `ScenarioManifestMsg` carries a scenario Merkle revision
and asset CIDs, but that resource is optional and exists only when networking
is installed. Offline replay therefore needs a transport-neutral baseline
assembled from the admitted simulation owners; capture must not depend on the
network manifest.

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
| HTTP, MCP, and Rhai command calls | `ApiCommandEvent` → `api_command_dispatcher` → typed command event | The event retains `ApiTransport` or the Rhai execution context through `ActiveCommandId` and `CommandOccurred`; direct typed triggers remain outside this path. |
| UI and Rust subsystem systems | Direct typed command events via Bevy `Commands` | Capture cannot be attached only to the API dispatcher. |
| Keyboard/gamepad vessel control | Bevy input state → admitted `PhysicalIntentFrame` semantic snapshot in `drive_from_bindings` at `FixedUpdate` → `SetPorts` before `ControlDacSet` | Admission requires the local input `SessionId`, target `GlobalEntityId`, and committed scene generation; the frame also carries the current `SimTick` and shared per-tick sequence. Missing admission facts and duplicate target/session order keys hold the input with a structured runtime error; ordering never falls back to Bevy `Entity` bits. While explicitly active, `SessionInputStream` retains the sorted canonical intent ids and admission identity in a bounded in-memory record. Do not record resolved port writes as external input. |
| Networked vessel control | Wire input → `SetPorts`; remote frames are consumed by `GlobalEntityId` and per-vessel sequence order at the fixed simulation step | `InputFrame` and `OwnedInputLog` serve one-vessel prediction rollback and acknowledgement. They are not a whole-session log. |
| Scheduled Rhai and hook behavior | Evaluated serially in the owning scenario/hook cycle against live simulation state | These outputs are derived behavior. Re-run them from the same state and inputs during replay; do not record them as independent external inputs. Direct scenario bridge writes (`set`, `port_set`) are part of that ordered evaluation and must stay behind the same replay boundary. |
| One-shot Rhai / workbench tool evaluation | Bounded `Repl` or tool queue → live-world evaluation outside the fixed simulation transaction | This is an external action when it changes authoritative state. The replay contract must retain the source/tool revision and typed arguments, assign an effective simulation boundary, and reproduce the result there; an application-cycle evaluation cannot mutate authoritative state at an arrival-dependent time. This path is not yet captured or fenced to a simulation tick. |
| Async preparation and owner results | Prepared off-thread, then validated and committed by the owning lifecycle or simulation boundary | Worker completion is not an input. Replay the admitted source revision and deterministic commit order, not completion timing. |

`ApiCommandEvent` carries its transport origin. The reflected command
dispatcher scopes that origin with the active command id, and the generated
`CommandOccurred` fact carries it to downstream observers. `CommandOccurred`
does not retain typed parameters, target identity, scene generation, effective
tick, or per-tick input order. The bounded session-input owner records those
facts for its admitted semantic-control commands. Rhai scenario origins also
carry the executing actor's stable `GlobalEntityId` and source execution
sequence; application-level Rhai calls may have no actor. Direct typed triggers
remain outside the API dispatcher.

`SimulateIntentEdge` copies the reflected command origin onto its
`SemanticIntentEdge`. The bounded `CausalTrace` query now exposes API transport
origin or Rhai scope, cycle, phase, generation, sequence, scenario actor id,
and explicit producer id for that discrete edge. API, application-Rhai, and
direct typed submissions enter the bounded `PendingSessionInputs` queue owned
by `lunco-core-session`. Admission requires a stable target id, a completed
scene generation, and a fixed simulation clock; it assigns the next tick and a
per-tick sequence. The controller fixed-step consumer validates generation and
target again, then emits the semantic edge before control propagation.
`SimulationInputOrderAllocator` in `lunco-control-core` owns that per-tick
sequence across producers and resets on scene teardown. A central cross-domain
commit owner for runtime spawns and other typed actions remains necessary before
one total effect order is guaranteed.
`CausalTrace` and `intent.edge` retain that producer id and admission stamp.
The command acknowledgement includes the same `producer_id`, `correlation_id`,
and optional admission fields, so a client can query this exact edge after later
edges arrive.
Simulation-clock Rhai edges stay in their deterministic hook pass and carry no
external-input stamp.

The controller producer is not yet a complete input boundary.
`drive_from_bindings` captures physical `ActionState<UserIntent>` into a
by-value `PhysicalIntentFrame` semantic snapshot and reads the `SimulatedIntents`
resource separately, then combines them with OR only while evaluating an
intent. Admission requires the local input `SessionId`, target `GlobalEntityId`,
and committed scene generation; the snapshot carries those identities, the
current `SimTick`, and an order from the same per-tick sequence allocator used
by external semantic inputs. Missing facts and duplicate target/session order
keys hold physical input with a structured runtime error; there is no
world-local `Entity` ordering fallback. When capture is active, the controller
appends a `SessionInputRecord` containing sorted canonical intent ids and that
admission stamp to `SessionInputStream`; the controller-local frame object is
discarded after translation. `StartSessionInputCapture`,
`StopSessionInputCapture`, and `ClearSessionInputCapture` control the bounded
in-memory stream;
`ReadSessionInputStream` returns its state and typed records. Before retention,
the session owner validates canonical payload names, stable stamps,
producer/payload pairing, and runtime-spawn pose invariants. Malformed records,
capacity limits, or ordering violations stop capture visibly and preserve prior
records. The held resource keys API transport by its caller-supplied nonzero `producer_id`,
Twin Rhai by route and actor, actorless Rhai by route and `producer_id`, and
direct typed commands by `producer_id`; a release removes only that producer's
hold. API clients and direct typed producers must keep the same ID for their
session and use distinct IDs within each producer class. The ID records input
provenance and is not an authorization credential. External `SimulateIntent`
changes and `SimulateIntentEdge` submissions share the bounded 4,096-record queue, require
a stable target id and committed scene generation, and receive the next fixed
tick plus a per-tick sequence. The fixed-step owner rechecks target and
generation, captures each due record, and publishes a typed commit event in
shared sequence order. The controller applies those events; held-state commits
publish `intent.hold`, while discrete edges retain their correlation id and
admission stamp in both `CausalTrace` and the `intent.edge` event. Scene
teardown clears pending records. Physical
`ActionState` frames receive the local input `SessionId`, target `GlobalEntityId`,
and fixed-tick order at their controller boundary. Missing identity or committed
scene state holds the input visibly, and duplicate target/session order keys do
not use process-local entity bits to break ties. While active,
`SessionInputStream` captures physical frames and admitted `SimulateIntent` /
`SimulateIntentEdge` payloads in bounded memory. Semantic records retain the
producer class and stable producer ID (or Rhai route and actor), target,
committed generation, tick, sequence, and command correlation id. The typed
commands reject missing or zero IDs for API, direct typed, and actorless Rhai
producers; Twin Rhai commands use their stable actor identity. Physical records
retain the local input session and canonical intent ids. Capacity/order
failures stop capture without evicting admitted records. Raw-file `SpawnEntity`
uses that external ingress queue and capture stream. Its typed record retains
the producer, scene-root and active-frame identities, catalog entry, exact
`f64` pose, correlation id, assigned tick and sequence, and reserved root
`GlobalEntityId`; commit applies the reserved identity before ordinary entity
admission and network replication. The canonical `WorldGrid` has deterministic
content provenance for stable active-frame identity. Document-backed spawning
continues through `ApplyUsdOps` and the Twin journal, without a duplicate
session-input record. `SessionInputCaptureArchive` checks the record contract
and encodes a versioned archive bounded to 65,536 records and 16 MiB.
On native hosts, `ExportSessionInputCapture` accepts a completed capture,
shares its immutable records with a `Background`-priority `AsyncWorkAdmission`
job, encodes away from the simulation schedule, and writes through
`lunco-storage` on Bevy's I/O pool. The app writes under
`<user-config>/session-captures/` using a validated filename stem and unique
operation suffix, refuses an existing output, reads the file back, decodes it,
and compares the records before reporting `complete`. Capture IDs are
monotonic for the app session, and a successfully exported capture cannot be
exported twice; failed exports retain retry eligibility.
`ReadSessionInputArchiveExport` exposes the capture and export IDs with pending,
complete, or failed status.
This persists the bounded input slice; it does not include a baseline manifest
or playback consumer, so whole-session replay remains open. The ingress queue
and `CausalTrace` remain separate from durable replay storage. Other command
payloads also remain open.

## Replay implementation boundary

The current session-input slice covers external `SimulateIntentEdge`
submissions, held/released `SimulateIntent` changes, raw-file runtime spawns,
and physical semantic frames while explicit capture is active. Physical frames
are admitted at their consuming controller boundary and are not deferred
through the external queue. A complete session-input
implementation must extend the same typed ingress to all supported external
authoritative inputs, retain stable source and target identities, and persist
records outside the fixed schedule. High-rate controls need semantic per-tick
frames; discrete actions need their typed action and target. The fixed-step
owner must consume all records in their admitted order.

Physical frames and admitted API/Rhai/direct semantic inputs retain distinct
producer classes in the capture. Deterministic simulation-Rhai actions remain
derived behavior and are not recorded as external inputs. Playback feeds
recorded semantic input through controller translation and the normal
event/command path, so Rhai and Modelica behavior are re-derived once.
Capturing a derived command as an external input would apply its effect twice.

The current queue and in-memory stream have explicit bounds. Archive encoding
uses the shared background admission queue and durable file access uses the
I/O task pool, outside the fixed schedule. A full queue, record limit, or
failed writer leaves visible failure status; it must not drop frames silently
or stall simulation. Runtime-spawned entities carry their reserved
authoritative identity in the spawn action, while content-derived entities use
their existing stable `GlobalEntityId`.
Playback remains unimplemented until a baseline manifest and playback consumer
satisfy the lifecycle, ordering, and failure requirements above.

### Replay baseline ownership

The baseline must be assembled from the owners that admitted the simulation,
not inferred from networking state. `ScenarioManifestMsg` is optional and
networking-owned; it can supply network scenario provenance but is absent from
offline runs. Its Merkle revision cannot stand in for the local runtime
baseline.

The baseline contract needs stable identities and snapshots for the complete
admitted owner closure:

- The host's immutable build identity and the root USD composition closure,
  including stable content identities for every composed dependency. The
  `UsdStageRevision` counter is an invalidation signal, not a content identity.
- Rhai and SysML source closures, keyed by canonical source identity and
  stable source-content identity. Process-local registry counters alone cannot
  identify the same sources across sessions.
- Every live Modelica participant's model revision, resolved solver id,
  capability profile, and effective solver parameters.
- The fixed-step clock and physics execution profile, initial authoritative
  runtime state, and every seed that can affect authoritative state.
- The committed scene generation and the stable entity identities needed to
  resolve input targets and runtime-spawn results.

Each owner must expose its admitted snapshot through one typed capture boundary.
If any required owner cannot freeze its snapshot, baseline capture must fail
with that owner's diagnostic. The capture must not substitute a network
manifest, a volatile revision counter, a guessed seed, or a partial state.
Playback stays open until the same baseline can initialize the consumer and the
consumer can submit archived records through their normal typed owner paths.

The existing Twin journal remains the owner for authored document operations.
It does not record transient controls, scene-time inputs, or physics state and
cannot reproduce a live session by itself. The input stream covers transient
runtime state without recording authored document edits a second time. The
audited ingress paths rule out treating `api_command_dispatcher`,
`CommandOccurred`, or the network rollback buffer as the whole-session
boundary.

## The authored-document model — one write path (record → project)

For a migrated authored document mutation, the journal is the **single source
of truth** and ECS is its **projection**. A document command does not both
record an op and separately mutate ECS; that dual-write creates two truths that
can diverge. Instead it records an op and the document projection applies it.
Local and remote authored document ops take the same path:

```
   authored document edit ─► document ingress ─► authored journal ─► domain projection ─► ECS
   remote authored op ─────► merge plane ──────► authored journal ─► domain projection ─► ECS
   external UserIntent ─────► future session input stream at (generation, SimTick, sequence)
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
| Spawn a rover during a session | Document-backed scene: resulting `ApplyUsdOps` in the Twin journal; raw-file scene: `RuntimeSpawn` admitted through `PendingSessionInputs`, then committed by the scene-command owner with `NetSpawn` | document operation uses `EntryId` and merged order; raw-file record retains producer, scene generation, scene-root and active-frame identities, original `f64` pose, correlation, reserved root id, `SimTick`, and shared sequence |
| Possess during a session | semantic user intent; whole-session capture is not implemented | scene generation, controlled target, `SimTick`, stable sequence |
| USD prim edit | USD document operation in the Twin journal | `EntryId` and merged journal order |

### Raw-file runtime-spawn input

The raw-file `SpawnEntity` path is a transient simulation input. It enters the
session-owned typed commit coordinator alongside external held intents and
discrete edges. `lunco-core-session` drains admitted records, validates their
stable targets, related frame identities, and scene stamps, captures each
record, and publishes typed commit events in shared sequence order. The
controller applies semantic-control payloads; the scene-command owner resolves
the catalog and commits runtime spawns. Physical frames are sampled and
captured at their consuming fixed tick through the same per-tick order
allocator, after admitted session events and before control propagation.

An admitted spawn record retains the stable scene-root target and active
physics-frame identities, catalog entry, original `f64` position and optional
rotation, producer provenance, command correlation id, and reserved
`GlobalEntityId` for the runtime-spawn root. The command acknowledgement
returns the producer and admission stamp plus the reserved id. At its fixed
tick, the scene-command owner revalidates the scene root, active frame, catalog
entry, and pose, then inserts the reserved id before identity admission.
`NetSpawn` and network replication use that same root identity. Invalid
admission state is a typed command rejection; stale commit-time scene, frame,
catalog, or identity state raises a structured runtime error.
The persistent canonical `WorldGrid` has deterministic content provenance, so
even a scene using the default world frame has a stable active-frame identity.

The document-backed path stays outside this session-input record: it authors
`ApplyUsdOps`, whose Twin-journal entry already owns its identity and order.
Recording it again as a runtime spawn would duplicate the authored mutation.

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
