# Command Sequences & the Visual Sequence Editor — design

**Status:** design/analysis · 2026-07-11 · Companion: `behaviour-trees.md`, `rhai-integration-design.md`

Goal: dynamically build **sequences of commands**, author them with **in-scene tools** (drop
numbered waypoints joined by a line), let sequences **reference other sequences**, and see a running
sequence as both a **timeline** and a **graph of actions**. This asks: what's needed, and is there a
standard format?

The headline: **we already own the execution substrate.** Almost everything here is *views over data
we already produce*, plus two small kernel/format additions. Don't build a new sequencer.

---

## 1. What already exists (and must be reused, not rebuilt)

| Capability | Where | Note |
|---|---|---|
| **BT kernel** — Sequence, Selector, Parallel(RequireAll/One), Repeat, Retry, Invert, Force, Reactive{Sequence,Selector}, Action leaf, event predicates | `lunco-behavior` (`node.rs`) | language/engine-agnostic, deterministic, unit-tested; **every node maps 1:1 to a JSON `BehaviorSpec`** |
| **Sequences ARE serializable data** | `lunco-scripting/task_tree.rs` | the prelude's `seq/par_all/par_race/repeat/forever/once/wait/…` build **pure data maps**, compiled once to the kernel tree |
| **Flat timeline sequence + runner** | `lunco-scripting/timelines.rs`, `RunTimeline` cmd, `compile_timeline` | a mission as a serializable step array (`move_to/wait/emit/wait_event/cmd`), runnable over the API with **zero rhai** |
| **Op journal / oplog** | `lunco-twin-journal`, `registration_journal.rs` | records **document** ops (`Usd`/`Modelica`/`Script`/…) and **registrations** (`RegisterToolLibrary`, `RegisterTimeline`). ⚠️ It does **not** record executed commands — `api_command_dispatcher` has no journal interaction (see [`architecture/command-journal.md`](architecture/command-journal.md) Status). A "planned vs actual" view needs that first. |
| **Reusable node/edge graph canvas** | `lunco-canvas` (Nodes/Edges/Grid/Selection/ToolPreview layers, `VisualRegistry`) | the Modelica diagram runs on it; it is the ready-made substrate for a **graph-of-actions** view |
| **Waypoint markers** | `vessels/markers/waypoint.usda` | present and spawnable; **no on-terrain polyline** yet (known route-line gap) |

So a "sequence of commands" is already a first-class, serializable, executable artifact — but **not a
journaled one**: its *definition* (a `Timeline` registration) journals; its *execution* does not.
The missing pieces are **references, views, an authoring tool** — and, for any "actual run" view,
command journaling itself (unbuilt: see [`architecture/command-journal.md`](architecture/command-journal.md)).

---

## 2. Running BOTH linear sequences and reactive autonomy — one model

The requirement is to run *both* time-tagged command sequences (flight-style) *and* reactive autonomy.
The trap is building two engines. Don't — **a linear time-tagged sequence is a constrained behavior
tree**: a `Sequence` of `wait → cmd` leaves with no branching and no world-state guards. Our kernel
already has `Sequence`, `wait`/`wait_until`, and `Action(cmd)`, so it is already a **superset** that
runs both. One model, one executor; "linear" is just a profile of the tree.

### 2.1 One model, two profiles
The `BehaviorSpec` document carries a computed **profile**:
- **`linear`** — uses only `{Sequence, wait/wait_until, cmd, emit}`. Open-loop, time-driven,
  deterministic, replayable. ⇒ exportable to flight stored-command formats.
- **`reactive`** — also uses `{Selector/Fallback, Parallel, Reactive*, guards/checks, Retry}`.
  Closed-loop, ticks against world/telemetry state. ⇒ exportable to autonomy formats.
- **`hybrid`** — mostly linear with reactive guards wrapping steps (e.g. "drive to A **but abort if
  tilt > 30°**" = a `ReactiveSequence` around a linear body). The kernel already does this; the export
  validator decides how much a given flight target can absorb.

There is still exactly **one executor** (the ticked BT kernel). Timeline execution = ticking a tree
whose only gate is time. This is the invariant to hold: never fork a separate "sequence runner."

### 2.2 The portability boundary: leaves are typed dictionary commands
For a sequence to run on *real* flight software, its leaves cannot be arbitrary rhai closures — they
must be **typed commands from a Command Dictionary** (name + typed args), and guards must reference
**telemetry** points from that same dictionary. So the dictionary (commands **and** telemetry) becomes
a first-class artifact — and it is exactly what the space standard **XTCE** (and CCSDS **EDS**) exists
to describe. Rule: a leaf that is a dictionary command/TM is **portable**; a leaf that is a sim-only
`call(closure)` is allowed but **flagged non-exportable** in the editor. This one boundary is what
makes flight reuse real rather than aspirational.

### 2.3 Standards, by layer (export targets, NOT our runtime)
cFS and F´ are full flight-SW *frameworks* (software bus, C/C++, RTOS) — far too heavy to be our in-sim
engine. Adopt their **file formats + XTCE** as export/interchange lanes over our one document:

| Layer | Source of truth (ours) | Standard export |
|---|---|---|
| Command + TM dictionary | our command registry / ApiCommand providers | **XTCE** (CCSDS/OMG); **EDS** later (cFS-aligned) |
| Linear time-tagged sequence | `BehaviorSpec` (linear profile) + command journal | **F´ `.seq`** (`CmdSequencer`) primary; **cFS SC** RTS/ATS; ESA **PUS-11 / OBCP-PLUTO** |
| Reactive / hierarchical plan | `BehaviorSpec` (full tree) | **NASA PLEXIL** (space-native autonomy) and/or **BehaviorTree.CPP XML** (Groot2 tooling) |

Node mapping is close to 1:1 in both reactive targets (Selector↔`<Fallback>`/PLEXIL condition,
Reactive↔`<ReactiveSequence>`, our `Ref`↔`<SubTree>`/PLEXIL library node, `cmd`↔`<Action>`/PLEXIL
`Command` node). Each adapter is a **validator + emitter**: it checks the doc against what the target
can express and *reports what it drops* (a guard the flight target can't evaluate, a sim-only closure)
— never silently.

### 2.4 The loop closes on the journal — **once commands are journaled**
"Author in sim → export XTCE dict + F´ `.seq` → run on an F´/cFS target → diff its execution against
our journal" is a concrete **twin↔flight validation pipeline**. ⚠️ It has one unbuilt prerequisite:
**we do not record dispatched commands.** The journal records document ops and registrations, not the
executed command stream (`api_command_dispatcher` does no journaling —
[`architecture/command-journal.md`](architecture/command-journal.md)). The export side is plumbing;
the "actual" side is a feature that does not exist yet.

**Which reactive target first is the one open call** — PLEXIL (space-native, closest to a flight
autonomy executive) vs BehaviorTree.CPP (robotics, but ships Groot2 for free visual editing/monitoring).
Recommend BT.CPP first for the *tooling* (it doubles as our in-app graph view's interchange), PLEXIL
next for the flight-autonomy story.

---

## 3. What's needed — the build list

### 3a. Kernel: named subtree references (the "sequence refs a sequence")
The kernel doc already lists "named reusable subtrees" as the next node. Add:
- a **`Ref` node**: `#{ k: "ref", name: "approach_poi" }` resolves at compile/tick to another stored
  sequence by name (cycle-checked). Maps to `<SubTree>`.
- a **named-sequence registry**: sequences stored by name (in the twin — USD attrs / a `*.seq.json`
  layer, via the existing SaveScenario path) so refs resolve and survive reload.
This is the one genuine kernel/runtime add; small, and it composes with everything above.

### 3b. Format: the Sequence document + the Command Dictionary
- Formalise the JSON `BehaviorSpec` (it exists de facto) as **the** sequence doc, add `ref` + metadata
  (node ids, display names, authored positions for the graph view), optional **time tags** on leaves
  (`after: dt` / `at: t`), and a computed **profile** (`linear`/`reactive`/`hybrid`, §2.1).
- Surface the **Command Dictionary** (commands + telemetry, typed) as a first-class artifact derived
  from the command registry — the portability boundary (§2.2) and the XTCE/EDS export source.
- Persist both as twin artifacts (SaveScenario path) so sequences and the dictionary are
  shareable/reference-able like any model.

### 3b′. Export/interchange adapters (validator + emitter each, §2.3)
- **XTCE** (and later EDS) ← Command Dictionary. Do this first: it unlocks every downstream.
- **F´ `.seq`** (+ cFS SC / PUS-11) ← `BehaviorSpec` **linear** profile. Validator rejects/report-drops
  reactive nodes.
- **PLEXIL** and/or **BehaviorTree.CPP XML** ← full tree. BT.CPP first (doubles as the graph view's
  interchange + Groot2 monitor); PLEXIL for the flight-autonomy story.
- Each adapter **reports what it drops** — never silent.

### 3c. Three synchronized VIEWS of one sequence doc
All three read/write the same document; editing one updates the others (the Modelica
model↔canvas↔code pattern):

1. **3D waypoint map (in-scene).** A **Waypoint tool**: click terrain → drop a **numbered marker** +
   append a `move_to` step to the active sequence; render a **polyline on the terrain** joining them in
   order (the missing route-line renderer — `BasisCurves`→mesh or a gizmo line-strip). The number IS
   the step index. This makes the 3D scene a live editor of the sequence.
2. **Timeline view.** A horizontal lane: steps left→right in execution order, dwell durations as
   widths, events/`wait_event` as markers. The **planned** lane reads the step array and is buildable
   today. The **actual** lane needs command journaling, which does not exist — until then it must come
   from the telemetry-event bus (`cmd:<Name>` events), not from the journal.
3. **Graph-of-actions view.** Instantiate **`lunco-canvas`** with a `VisualRegistry` for BT node kinds;
   composites/decorators/leaves as nodes, child order as edges, `Ref` as a link into another sequence.
   **Live tick status** (Running/Success/Failure) colours nodes from the kernel tick — same data Groot2
   would show, in-app.

### 3d. Dynamic construction (the API/rhai surface)
Expose sequence editing as commands (already the shape of `RunTimeline`): `SeqNew/SeqAppend/SeqInsert/
SeqRef/SeqSave/SeqRun`, so the 3D tool, the graph editor, rhai, and the API all mutate the **same**
doc. Dynamic creation falls out for free — a sequence is just a growing data array.

---

## 4. The unifying picture

```
   Command Dictionary (commands + TM, typed) ──► XTCE / EDS ─────────────────────────────┐
                 │  (leaves bind to it — the portability boundary)                        │
                 ▼                                                                         ▼
   ┌────── one Sequence document (JSON BehaviorSpec; profile: linear | reactive | hybrid) ──────┐
 3D waypoint map ─edits►                                    ◄edits─ graph-of-actions (lunco-canvas)
   (numbered markers + terrain polyline)                    ◄edits─ timeline (steps + journal)
                 └── compiled once ──► lunco-behavior kernel ── ticks ──► journal (actual run: NOT BUILT) ──► views
                            │                                                   │
              linear profile├──► F´ .seq / cFS SC / PUS-11        full tree ────┴──► PLEXIL / BT.CPP XML
```

One dictionary, one document (two profiles), three views, one deterministic kernel, one journal — with
standard export lanes per layer. The linear/reactive split is a *property of the doc*, not a second
engine. Net-new engine work stays bounded: the `Ref` node + name registry, time tags + profile
computation, the terrain polyline, two view panels on the canvas we already ship, and the
validator+emitter adapters.

### Build order
1. `Ref` node + named-sequence registry + persist (unblocks nesting; pure data).
2. Command Dictionary artifact + **XTCE** export (the portability boundary; unlocks all downstream).
3. Sequence-edit commands (`Seq*`) + time tags + profile computation — dynamic construction over API/rhai.
4. Waypoint tool + terrain polyline — the in-scene authoring view (also fixes the route-line gap the
   Space-School SS2 lesson notes).
5. Graph view on `lunco-canvas` with live tick status; timeline panel (planned vs actual from journal).
6. Export adapters: **F´ `.seq`** (linear) + **BT.CPP XML** (reactive/tooling) first; **cFS SC** /
   **PLEXIL** as the flight targets firm up.

---

## 5. Core vs data, adapter implementation, and the standard to anchor on

### 5a. What is CORE (Rust), what is DATA/rhai
Mechanism → core; policy/content → data. (Same rule as the rest of the engine.)

| Core (Rust) | Data / rhai |
|---|---|
| BT kernel + `Ref`/subtree node (`lunco-behavior`) | the sequences/plans themselves |
| `BehaviorSpec` doc model + compiler (`task_tree`) | mission policy, scoring |
| **Command Dictionary** (typed cmds + TM) — the contract | **mapping tables** (node-kind ↔ target tag/line) |
| named-sequence registry + persistence | tutorial content |
| time / mission-clock (`wait_until`, lunco-time) | export *emitter* prototypes (line formats) |
| **profile computation + export validators** | |
| the XML/text **codec** (parse / emit / validate) | |

The dictionary + validators + kernel must be correct and deterministic → non-negotiably core. Anything
a mission author touches stays data.

### 5b. Adapters: mapping in data, codec in Rust
- The **mapping** (our node kind → `<Fallback>` / PLEXIL node / F´ `.seq` line) is a **data registry** —
  add/tweak a target with no recompile.
- The **codec** (XML/text parse, escaping, schema validation, round-trip) is **Rust** (serde/quick-xml).
  rhai has no XML lib and adapters often run offline/headless/CI.
- **Export emitters for line formats (F´ `.seq`)** may prototype in rhai, then promote. **Import and any
  XML (XTCE, BT.CPP, PLEXIL) is Rust from the start.**

### 5c. Reactive standard: anchor on BehaviorTree.CPP; PLEXIL is a later lane
- **BT.CPP** *is* a behavior tree → **1:1 with our kernel** (near-lossless), ships **Groot2** (free
  visual editor + live tick monitor = our graph view's interchange), and is **the ROS standard** (Nav2
  `bt_navigator`). Best fit + tooling + ecosystem.
- **PLEXIL** (NASA Ames) has richer node-state, synchronous-reactive semantics and stronger flight
  pedigree, but a *different* execution model (BT→PLEXIL is lossy) and academic tooling. Add it as a
  secondary flight-autonomy export, not the anchor.

### 5d. ROS alignment (comes largely for free)
- Reactive execution: **BehaviorTree.CPP** (Nav2/Groot) — anchoring on it buys ROS interop.
- Interface contract: **rosidl** `.msg`/`.srv`/`.action` (a BT leaf calls a ROS action). Our Command
  Dictionary is the analog — one dictionary, two emitters: **XTCE** (ground/flight) and **rosidl** (ROS).
- Planning above BTs: **Plansys2** (PDDL/HTN). State machines: **FlexBE/SMACH**, `py_trees_ros`.

**Anchor decision:** BT.CPP for the reactive standard; Command Dictionary + XTCE (± rosidl) for the
interface; mapping tables as data, codec as Rust; PLEXIL and cFS/F´ `.seq` as secondary export lanes.

### 5e. Keep `lunco-behavior` — adapter, not runtime replacement
Do **not** replace our kernel with a third-party BT crate (e.g. `behaviortree-rs`) to gain BT.CPP
compatibility. The need is a **format adapter**, not a new runtime.
- **Blast radius:** `lunco-behavior` is load-bearing — the rhai prelude lowers onto it, the autopilot
  `BehaviorSpec` compiles onto it, and scenario `task()` drives it. Swapping it rewrites all three for
  zero behavioural gain.
- **Determinism/netcode:** our kernel is clock-free, deterministic, unit-tested because the lockstep +
  predict/reconcile netcode **replays** it. A third-party crate's tick order / allocation / async is an
  unknown that may not survive rollback.
- **World model:** our kernel is generic over a `Ctx` into the port-registry/command-bus world;
  `behaviortree-rs` mirrors BT.CPP's blackboard+ports — an impedance mismatch.
- **wasm/maturity:** our kernel is known wasm-clean; a reimplementation's wasm/async/bus-factor are
  bets on core autonomy.
- **Our nodes are already ≈ BT.CPP** (Sequence, Selector=Fallback, Parallel, Repeat, Retry, Invert,
  Force, Reactive\*, `Ref`=`<SubTree>`), so interop is cheap.

**Plan:** (1) converge node names to BT.CPP (Selector→Fallback alias, add `Ref`/SubTree); (2) write a
small `quick-xml` adapter over `BehaviorSpec`; (3) *optionally* borrow `behaviortree-rs`'s XML serde as
a build-time dependency for parsing — reuse its parser, not its runtime.

#### Evidence: `behaviortree-rs` audit (checked 2026-07, from its README/source)
Corrected from an earlier claim that its async is fundamental — **it is not**. The crate has a
first-class **synchronous** path, so "make it sync" is mostly already there:
- **Sync tick exists:** `#[bt_node(SyncActionNode, Sync)]` + `impl SyncTick { fn tick(&mut self) ->
  Result<NodeStatus, NodeError> }` — same shape as our `Node::tick(&mut ctx) -> Status`. Async is the
  *default*, not the only mode.
- **State in cloneable structs** (`#[derive(Clone, Debug)]`), not futures → snapshot via `Clone`.
- **Present:** XML *parsing* ✅, SubTrees ✅, Blackboard ✅, Ports ✅, all composites ✅.
- **Missing / gating (the real work, NOT the tick model):** XML *generation* 🔴 (read-only — "format
  for free" is import-only today), Loggers/Observers 🔴 (no Groot2 live monitor), Scripting/pre-post
  conditions 🔴, **serde on state** not advertised, and it is **WIP**.

#### Spike results (scratch crate, 2026-07-12) — ran it
Built a throwaway crate against `behaviortree-rs` and tested the hard gates:
- ✅ **Sync driver, no tokio.** Source-confirmed: `Factory::create_sync_tree_from_text → SyncTree`;
  `SyncTree::tick_while_running()` = `futures::executor::block_on(root…)` (also a re-exported
  `sync::block_on`). `tokio` is a **dev-only** dependency; the library uses `futures` + `quick-xml`.
- ✅ **wasm32-unknown-unknown builds.** The whole dep tree — `futures-executor`, `quick-xml`, and even
  `env_logger`/`is-terminal`/`termcolor`/`pretty_env_logger` — compiled clean for wasm; `behaviortree-rs`
  itself compiled for wasm. The wasm hard-gate **passes**.
- ✅ **SubTree refs** (`<SubTree ID="…">`) are built in — the "sequence refs a sequence" feature.
- ~ **Determinism:** structurally certain (block_on of a pure future — no I/O/RNG/time), but not
  demonstrated at runtime because →
- ❌ **Maturity/stability — disqualifying for a *core* dep.** Last commit **2024-02-05** (~2.5 yr
  dormant). `main` HEAD **does not compile** (81 errors, derive/lib version skew). Published `0.2.4`'s
  public API **differs from the repo README/docs**, exposes **no documented tick trait**, ships **no
  examples**, is ~30% documented; **XML generation** and **Groot observers** are unimplemented. A
  trivial custom-node tree could not be stood up without fighting version/API drift.

**Verdict — keep `lunco-behavior`; take the format, not the runtime.** The *technical* fit is fine (sync
+ wasm + subtrees all work — the earlier "async blocks it" claim was wrong). But adopting a **dormant,
broken-at-HEAD, undocumented, API-unstable** crate as load-bearing core autonomy — in a
deterministic-netcode + wasm engine — to avoid writing an XML codec is a bad trade. Instead: keep our
maintained, deterministic, integrated kernel and write the BT.CPP-XML adapter with **`quick-xml`** (the
same parser `behaviortree-rs` itself uses); Groot2 stays an external editor. If the project ever revives
and stabilises, revisit — the sync/wasm story is genuinely OK.

---

## 6. `lunco-behavior` ↔ BehaviorTree.CPP v4 — parity, gaps, and compatibility work

Grounded in our source (`node.rs`, `task_tree.rs`): our `Status = {Running, Success, Failure}`;
composites Sequence/Selector/Parallel(RequireAll|RequireOne)/Repeat/Retry/Invert/Force/Reactive{Seq,Sel};
one unified `Leaf` (act / done-poll / check / wait-dwell / wait_for-event) whose actions are **rhai
closures**. There is **no blackboard and no typed ports** — data flows through the `Ctx`/world and rhai
capture. That single fact is the crux of compatibility.

### 6a. Parity matrix (BT.CPP v4 → us)
| BT.CPP | Us | Gap |
|---|---|---|
| Sequence / SequenceWithMemory | `seq` (our Sequence latches the running child = *with memory*) | ✅ / naming |
| Fallback | `sel` (Selector) | ✅ rename |
| ReactiveSequence / ReactiveFallback | `reactive_seq` / `reactive_sel` | ✅ |
| Parallel (N-of-M success/failure thresholds) | `all` / `race` (RequireAll / RequireOne only) | ⚠️ no arbitrary thresholds |
| IfThenElse · WhileDoElse · Switch2-6 | — (composable from sel+check) | ❌ no dedicated nodes |
| Inverter · ForceSuccess/Failure | `invert` · `force_ok`/`force_fail` | ✅ |
| Repeat · RetryUntilSuccessful | `repeat` · `retry` | ✅ |
| KeepRunningUntilFailure · RunOnce (decorator) · Delay · Timeout | — (Timeout belongs in the clock-owning layer) | ❌ |
| Precondition · Loop/ConsumeQueue · Script(assign) · SkipUnlessUpdated/EntryUpdated | — | ❌ |
| **SubTree** (+ port remap) | — (`Ref` planned) | ❌ (planned) |
| SyncAction · StatefulAction (onStart/onRunning/onHalted) · Condition · ScriptCondition | our `Leaf`: act / done-poll / check (rhai closures) | ✅ different shape |
| Coro/Threaded/async actions | — (we're sync; `done`-poll gives the same "running until true") | ❌ (by design) |
| SetBlackboard | — | ❌ (no blackboard) |
| **Blackboard** (scoped entries) · **typed Ports** (in/out/bidir, `convertFromString`) · **port remapping** | — (Ctx/world + rhai `this`/params) | ❌ **the big one** |
| Blackboard entry timestamps/seq (v4.1) | — | ❌ |
| **SKIPPED** status (v4) · IDLE | Running/Success/Failure only | ❌ |
| XML parse · XML generate · node-model manifest | serializable data maps (no XML) | ❌ format work |
| Pre/Post-condition attrs (`_skipIf`/`_successIf`/`_while`/`_onSuccess`…) | guards via reactive/check | ❌ (subset via composition) |
| Scripting mini-language | rhai (more powerful, different syntax) | ~ translate/subset |
| Loggers/Observers · Groot2 ZMQ publisher · Substitution/mock nodes | — (our own graph view) | ❌ (own tooling) |

### 6b. What WE have that BT.CPP does not
- **Language/engine-agnostic `Ctx`** — the *same* kernel driven by rhai **or** python bindings; BT.CPP
  is C++-only.
- **Leaves as live rhai closures** — authored as data, hot-reloadable, no compile step; BT.CPP nodes are
  compiled, statically-registered C++ types.
- **Sync, clock-free, deterministic kernel designed for lockstep + predict/reconcile replay and wasm** —
  BT.CPP targets real-time robotics, not networked-deterministic replay.
- **Domain-native, event-bus-integrated leaves**: `wait_for` with source resolution (Gid/Path), and (in
  the world layer) `entered_zone`/`exited_zone`/`zone_of`, `nav_to`, `world_up` — first-class and wired
  to the event bus + big_space, where BT.CPP leaves that to user-written ConditionNodes.
- **Compact unified `done`-poll leaf** — one leaf expresses action / poll-until-true / condition /
  dwell / event-wait.
- **Journal/oplog + virtual-time integration** (lunco-time: pause≠speed0) — the executed tree is
  recorded, and dwell leaves honor the sim clock.

### 6c. What compatibility actually takes (tiered)
**Tier 1 — structural (cheap, ~1:1):** map Selector→Fallback; add the `Ref`/SubTree node + named-tree
registry; add **SKIPPED** status (accept + emit); express IfThenElse/WhileDoElse/Switch/
KeepRunningUntilFailure/Delay by composition or thin nodes; add N-of-M Parallel thresholds.

**Tier 2 — the data model (the real cost):** BT.CPP's data flow **is** ports + blackboard + parent↔child
remapping; we have none. To round-trip real BT.CPP XML we must add **typed ports on nodes + a
lightweight blackboard** (leaves declare in/out ports; rhai reads/writes them; `SetBlackboard`,
`convertFromString`). Lighter alternative: a **port *view*** that maps named ports to Command-Dictionary
entries + `this`/params and synthesises ports for the XML without a full blackboard — cheaper, but lossy
on remapping. This tier, not the tree shape, is what "make it BT-compatible" really means.

**Tier 3 — format + tooling:** the **`quick-xml` codec** (parse + generate `<root main_tree_to_execute>
<BehaviorTree ID>…<SubTree/></root>`); a **node-model manifest** (`provided_ports`) for the Groot2
palette; optional pre/post-condition attrs, Script-node translation to rhai, and (native-only) a Groot2
ZMQ publisher. The adapter stays a **validator + emitter**: a supported subset that **reports every
drop**.

**Bottom line:** on tree *shape* we are ~1:1 and interop is easy. Real BT.CPP compatibility is gated by
the **ports/blackboard data model** (Tier 2) plus **SKIPPED** and **SubTree** — those are the missing
pieces. Everything else (scripting, pre/post, observers) is an optional reported-subset.

---

## 7. USD IS the ports+connections model — reuse the existing substrate

The "Tier 2" gap (BT.CPP's blackboard + typed ports + remapping) is **already solved in our stack** — by
USD connections and the runtime wiring we built for cosim/control. We do not need a new blackboard.

### 7a. The 1:1 mapping (verified in-tree)
| BT.CPP data concept | What we already have |
|---|---|
| typed **port** (input/output/bidir) | USD **`inputs:*` / `outputs:*` typed attributes** on a prim (e.g. `float inputs:force_x`) |
| **port remapping** / blackboard pointer `{key}` | USD **`.connect`** — `float inputs:force_x.connect = </Prim.outputs:force_x>` |
| **blackboard** (shared entries) | the **`PortRegistry`** substrate + **`SimConnection`** propagation (`propagate_connections`), reconciled from USD by **`reconcile_usd_connections`** — the FMI/SSP "any output → any input" pattern |
| **SubTree** (referenced tree) | USD **`references`** / `over` (already how vessels/rovers compose) |
| typed **convertFromString** | USD typed attributes (`float`/`int`/`token`) with defaults |
| node with ports | a USD **prim** carrying `inputs:`/`outputs:` + a `lunco:btNode` kind attr |

So a behavior tree can be authored **as a USD prim hierarchy**: nodes = prims (`lunco:btNode="Sequence"`
etc.), ports = `inputs:`/`outputs:` attributes, data flow = `.connect` (→ `SimConnection` at runtime),
subtrees = `references`. **The USD document *is* the tree plus its data wiring**, and it rides the exact
connection infra cosim (`descent_lander.usda: inputs:force_x.connect`) and control
(`control_profiles.usda: lunco:port`) already use.

### 7b. What this collapses
- **No new blackboard.** The runtime "blackboard" is the `PortRegistry` + `SimConnection` graph we
  already propagate each tick.
- **The BT.CPP-XML adapter becomes USD ⇄ XML** — both sides are "nodes with typed ports + connections +
  subtree refs." Structural, not a new data model.
- **The graph-of-actions view is the canvas we already ship.** `lunco-canvas` (`scene.rs`:
  `Node`/`Edge`/`Port`/`PortRef`) renders port-graphs today (the Modelica diagram); a BT is the same
  shape. `lunco-core/diagram.rs` already has the abstract `Node`/`Edge`/`Port` + `EdgeKind::Wire/Signal`
  model.
- **Convergent stack:** USD = document + connections · `PortRegistry`/`SimConnection` = runtime
  blackboard · `lunco-canvas` = graph editor · `lunco-behavior` = executor. Net-new is only the BT node
  *semantics* (a `lunco:btNode` schema) + the XML interop adapter.

### 7c. How rhai fits (authoring layer, not the data model)
Our rhai task layer (`task_tree.rs`) builds the tree as **serializable data maps**
(`seq/sel/all/race/repeat/…` + leaves), compiled once to the kernel — i.e. **rhai is an authoring
front-end that emits the tree as data, exactly like BT.CPP's XML, but richer and dynamic.** Mapping:
- rhai `seq([...])`/`sel([...])`/`reactive_seq` ↔ `<Sequence>`/`<Fallback>`/`<ReactiveSequence>` — 1:1.
- rhai **closures as leaves** (`|m| distance(a,b)<6`) ↔ BT.CPP **Script/ScriptCondition + inline
  Actions** — we are a **superset** (full rhai vs BT.CPP's mini expression language). Pre/post
  conditions (`_skipIf`/`_successIf`/`_while`) map to rhai predicates (`check`/`done`/reactive guards).
- **The one divergence — portability.** A rhai closure leaf is **not** a named, typed, serializable
  node, so it can't round-trip to XML or flight. Resolution (the §2.2 boundary, restated): a leaf is
  either **portable** — a named command bound to a **USD `inputs:`/`outputs:` port** (→ XML / F´ / XTCE)
  — or **sim-only** — a rhai closure (→ runs in-sim, flagged non-exportable). rhai stays the authoring +
  inline-scripting language; **the data flow rides USD ports/connections**, not closure capture, for
  anything that must leave the sim.

**Upshot:** USD + `PortRegistry` + `SimConnection` + `lunco-canvas` already supply the hardest parts
(ports, connections, remapping, graph editor). rhai is the authoring/scripting layer on top.

### 7d. Do NOT invent a `lunco:btNode` schema up front (revised)
There is **no OpenUSD standard schema for behavior trees** (USD standardizes the *connection mechanism*,
`UsdShade` `inputs:`/`outputs:`/`.connect`, and the *schema-extension mechanism* — but no BT content
schema). So making BT nodes USD prims would be *us inventing* a convention. Before doing that, note the
two binding models:
- **Action dispatch** ("do X now") → the **Command Dictionary by name** (command bus) — what our rhai
  tasks already do (`cmd(…)`, `nav_to(…)`). **No `.connect` needed.**
- **Continuous signal wiring** (`outputs:force`→`inputs:force` each tick) → USD `.connect` + `SimConnection`
  — what cosim uses, and what BTs mostly **don't** need.

BTs are overwhelmingly the first kind, so the "every node is a USD prim with ports" machinery solves a
problem BTs mostly don't have. **Default design (minimal invention):**
1. **Tree = a BT.CPP XML asset** (standard, Groot2-editable, subtrees for reuse).
2. **Leaf → world = Command Dictionary by name** (XTCE/rosidl) — the portability boundary.
3. **USD references the behavior file on an entity** (one attribute/reference, like a `.mo` or mesh):
   USD represents the *attachment*, not the tree internals.

Make BT nodes first-class USD prims (a lightweight `lunco:*` convention — **not** a generated schema)
**only** when a concrete need appears: USD variants/`over` on a shared subtree, BT ports `.connect`ing
into the cosim signal graph, or per-node canvas editing with USD undo. Until then: **YAGNI.**

The BT work then shrinks to: **SKIPPED status + `Ref`/SubTree** kernel bits, the **BT.CPP-XML codec**,
and **leaf→dictionary binding** — no custom USD schema.

---

## 8. Status (2026-07-12): most of this already exists — reuse realized

Verified against the running build and the source; **the "trees as savable/loadable/synced assets" ask
is already implemented** by the **timeline** subsystem (`lunco-scripting/src/timelines.rs` + commands):

- **Tree-as-data:** the declarative timeline JSON (`move_to`/`wait`/`emit`/`cmd`/`wait_event` steps) IS
  the serializable `BehaviorSpec`; `RunTimeline` lowers it onto the kernel via `compile_timeline`. (rhai
  `task()` closure trees remain the *sim-only* dynamic form — not legacy, a complementary layer.)
- **Save:** `RegisterTimeline{name,timeline}` validates → stores in `TimelineStore` → persists to
  `<twin>/timelines/<name>.json` → **records a `DomainKind::Timeline` journal op that syncs + persists
  via the journal plane** (remote peers get it on the replay leg). Journaled + net-synced via existing
  infra — no new stack.
- **Load:** `ListTimelines` / `GetTimeline` / `RunStoredTimeline`, plus auto-reload from
  `<twin>/timelines/` on Twin open (`TwinAdded` observer).
- **Proven live** (:3005, no rebuild): `RegisterTimeline demo_patrol` → `ListTimelines`
  `{count:1}` → `GetTimeline` returns the exact tree JSON. Save→load round-trips.

**"Retire legacy":** there is no duplicate sequence engine to retire — closure `task()` (dynamic,
sim-only) and declarative `timeline` (portable, stored, synced) are two authoring forms over the **one**
`lunco-behavior` kernel. The reuse guidance is satisfied by *not* building a parallel system.

**Genuinely net-new remaining (a real build, optional/interop):**
1. **BT.CPP-XML codec** — timelines are JSON; `quick-xml` USD/JSON ⇄ BT.CPP-XML adapter for Groot2 /
   robotics interop.
2. **Content migration (optional):** move tutorial closure autopilots that are pure command sequences
   into declarative timeline files — needs a `move_to_entity:"path"` step variant so `nav_to(find(...))`
   targets stay symbolic.
3. **Kernel bits:** `SKIPPED` status + `Ref`/SubTree (for full BT.CPP node parity).

### Tutorial tracks — wired + verified (2026-07-12)
Basic (`assets/tutorials/basic/`, B1–B5) + Space School (SS1–SS4) registered
into the shared registry (one-plugin pattern in `lunco-sandbox/src/ui/mod.rs`; `b1`/`ss1`
`first_start:false`). Verified live on a dev build: **B1** mission+autopilot → both objectives →
`MISSION_COMPLETE` → chain prompt (screenshot); **SS1** coach tour + `traverse.usda`; **B3** tip-over +
the new `world_rotation`/`tilt_deg` path (`[debug] tilt at start: 0.0°`); **B4** variant spawns
(`/RoverEasy` + `/RoverAwful`). Zero rhai errors. Core add `world_rotation` (bridge_core + world_bridge +
catalog + prelude `world_up`/`world_right`/`tilt_deg`/`is_tipped`) compiled + exercised green.

Since superseded on two points: the Space School track ships with its Twin
(`<twin>/sim/tutorials/`), not from `assets/`, and B2 ("Reading the Terrain") was dropped when its
`traverse.usda` moved out with it — the Basic track is B1/B3/B4/B5, numbered 1–4. A workspace lesson
must not `load_scene` a Twin asset; that is the split those two changes enforce.
