# 16 — Document Identity, Conflicts, and Collaboration

> Status: Design · Audience: contributors touching documents, registries, assets, or multi-user
>
> ⚠️ **PART DESIGN SPEC.** §1–§4 describe what IS. §5 onward (layer model, live
> layer, resolver seam, permissions UI) is the **target end-state**: the
> `ar::Resolver` seam is unused, `find_or_open` does not exist in our `openusd`
> fork, and there is no live layer. Read those as intent, not code.

Complements [`10-document-system.md`](10-document-system.md), which defines what a
Document *is*. This one answers: **who owns a file, what happens when two writers
disagree, and how that scales to many people.**

## 1. Two kinds of data, and why conflating them hurts

Everything the app loads is one of two things. They are not interchangeable, and
most of the bugs in this area came from treating one as the other.

| | **Document** | **Asset** |
|---|---|---|
| examples | `.usda`, `.mo`, `.rhai` | DEM `.tif`, `.glb`, textures, HDRI |
| identity | **the path** | **content hash** (cid) |
| edited? | yes — typed ops, undo, journal | no — imported/regenerated |
| may diverge from disk? | yes, deliberately (dirty) | no |
| sync | op stream | content-addressed fetch |
| stale when | disk changed under a clean doc | cid mismatch |

A DEM heightmap has no ops, no undo, no dirty state; pushing it through a document
registry means diffing a megabyte of binary through an undo stack. Conversely a
`.usda` is not a blob to cache — it is the user's editable intent.

> `ShaderDocument` is currently a Document with **no `DocumentOrigin`** — a bare
> `path: String`, "keyed by its asset path". It is identified like an asset. That
> is an open question, not a settled design.

## 2. Identity is the path

**One file ⇒ one document.** Two `DocumentId`s for one path means two undo stacks,
two journal streams, two tabs, and racing saves — split-brain over the user's work.

`lunco_doc_bevy::DocumentRegistry<D>` owns this rule once, for every domain:

```rust
registry.open_file(path, source) -> (DocumentId, OpenOutcome)
```

`OpenOutcome` is `Allocated | Refreshed | KeptDirty | KeptUnparsable`. The typed
outcome exists so **"already open" can never quietly mean "keep whatever's in
memory"** — the caller must see which happened.

`allocate` is for path-less origins only — and **the type system enforces it**. It
takes `lunco_doc::PathlessOrigin` (`Untitled | Bundled`), which cannot express a
filesystem path, so a file-backed document can *only* be born through `open_file`,
where the one-document-per-path check lives. This used to be a doc comment asking
callers nicely; a `File` origin handed to `allocate` for an already-open path minted
the split-brain second document. The rule now rides on the signature, so a document
type added next year inherits it for free.

Session restore is the sole caller that must reinstate a stored `File` origin
verbatim — it reloads saved in-memory state, possibly dirty, rather than re-reading
disk. It uses `DocumentRegistry::restore(source, origin)`, which says so; nothing
else may. Do not widen `allocate` back to `DocumentOrigin`.

Three rules that are not obvious and were each paid for in bugs:

- **Reusing the IDENTITY must not reuse the CONTENT.** Both open paths were once
  shaped `if !already_open { allocate(source) }` — so a freshly-read `source` was
  silently dropped when the file was already open (USD replayed pre-edit scenes
  until an app restart), or no check happened at all (Modelica minted duplicate
  documents that saved over each other).
- **`source` is a parameter.** The registry never reads or caches a file. The
  caller decides where bytes come from — local disk, or a client's replicated
  bytes. *"Cache only on the client"* then holds by construction rather than by
  discipline.
- **No path→id index.** `document_mut()` is public and Save-As rebinds origins
  behind the registry's back, so a cached index would silently rot — the exact
  bug class this rule exists to kill. Origins are the truth; scan them.

> **`DocumentOrigin::canonical_path()` does not canonicalize.** It returns the
> stored path verbatim. Compare with `lunco_doc::same_file` — `==` misses
> `/a/./b`, `../a/b`, and symlinks, and mints a duplicate anyway.

## 3. Dirty means "memory won on purpose"

A **clean** document is a *cache* of the file — never trust it over disk.
A **dirty** document *is* the truth — disk is the stale copy.

Every reload policy follows from that one distinction, and it is the only reason
an in-memory copy is legitimate at all: unsaved edits cannot exist without a
divergent copy. That is not a cache; it is the edit.

## 4. Conflict granularity = op addressing

**This, not layers, decides whether collaboration is possible.** The journal
already records real, replayable ops for every document type, and
`DocumentRegistry::replay_op` already applies remote ones. The mechanism is built;
its *quality* is capped entirely by how ops address what they change:

| addressing | example | concurrent outcome |
|---|---|---|
| **name/path** | `/World/Rover.translate`, `AddComponent{class}` | merges per-property; last-writer-wins is safe |
| **byte offset** | `ModelicaOp::EditText{range}` | breaks the moment the base moves |
| **whole file** | `ScriptOp::SetSource(String)` | last writer silently erases the other |

Omniverse survives last-writer-wins because its deltas are **per-property**. Ours
are not, uniformly:

- USD ✅ path-addressed
- Modelica ⚠️ mixed — `AddComponent` merges, `EditText{range}` does not
- rhai ❌ **whole-file — single-writer until `SetSource` becomes
  `SetFunction{name, body}` or similar**

Shipping collaboration before fixing rhai's addressing would destroy work quietly.

## 5. Target: the layer model — **authoring only**

**Never edit the base layer live.** Edits land in a layer *above* it; composition
resolves by strength. Then a base reload is conflict-free — the user's opinions
ride on top — and most conflict UX dissolves instead of needing a merge dialog.

```
AUTHORING — USD layers. Slow to write. Composition, undo, journal, git.
  [ base .usda        ]  authored truth, resolved via ar::Resolver, never live-edited
  [ runtime sublayer  ]  runtime-authored state (spawns, gizmo moves, checkpoints),
                         PERSISTED to .lunco/runtime
  [ live layer        ]  other users' authoring deltas, LWW per property  (Nucleus `.live`)
  [ session layer     ]  truly ephemeral, NEVER saved (camera, selection, presence)
```

> **The `runtime` layer is a persisted sublayer, not a USD session layer.** A
> session layer is by definition never serialized; ours round-trips through
> `.lunco/runtime`. Do not "fix" it by calling it a session layer — the ephemeral
> slot above is a *different*, currently-unused thing.

**Per-tick simulation state is NOT in this stack, and must never be** — see §5a.

This is USD's native answer, and it generalises as **"base + addressable deltas"**:

- **USD** gets it natively — `Stage::open(root, session)` composes live.
- **Modelica/rhai** have no layer model, but *do* have a base + an op log. Replay
  is their composition — and it only works if ops are name-addressed (§4).

**What this deletes.** Today the document composes `base ⊕ runtime`, **serializes
it back to USDA text**, and publishes the bytes as a `twin://` overlay so the
asset loader reads them back — a `parsed → text → parsed` round trip whose only
purpose is to hand data to ourselves, because the stage is built by a *path
loader* when the content already exists in memory. With a real session layer the
overlay, `Assets<UsdSourceText>` (locally), and `composed_source()` all disappear.

## 5a. Runtime state lives in the ECS, not in a layer

USD is an **authoring** data model. It is fast to read and **slow to write** —
NVIDIA measures a write-back at *milliseconds to hundreds of milliseconds* — and
their guidance is explicit: *"changing the USD data in runtime is not recommended
because of performance reasons"*, and *"it's best not to write back to USD while
the simulation is running."*

So Omniverse splits it in two, and so do we — under different names:

| concern | Omniverse | LunCoSim |
|---|---|---|
| authoring truth | USD layers | USD documents / layers |
| per-tick runtime | **Fabric** (via USDRT) | **Bevy ECS + avian** |
| USD → runtime | USDRT Population (batched) | `StageSink` projection |
| runtime → USD | explicit, never per-frame | checkpoint / save only |
| who reads runtime | PhysX, render delegate | avian, renderer |

**The ECS *is* our Fabric.** That is what "USD = truth, ECS = projection (two
worlds)" has always meant; it is the same architecture NVIDIA ships, not a
LunCoSim invention.

The rule that falls out:

> **Never author per-tick simulation state as a USD op.** A rover's position each
> frame belongs in the ECS and is replicated by rollback netcode (§7). It reaches
> USD only when a human asks — save, checkpoint, promote-scenario.

This is not theoretical for us: `sync_twin_overlays` already debounces its
whole-stage serialize *because* per-stroke USD writes were unaffordable during
terrain brushing. That was this rule, discovered locally and paid for once.

**Three things, not two** — and the middle one is easy to miss:

1. **Authoring edits** (user drags a gizmo, spawns a rover) → base / live layer.
   Journaled, undoable, saved.
2. **Runtime-authored state** (a scenario's spawns, a saved checkpoint) → runtime
   sublayer. Persisted, but *not* the user's edit history.
3. **Per-tick sim state** (where the rover is this frame) → **ECS only**. Never a
   USD op, never journaled, never undoable.

Today (1) and (2) share the `runtime` sublayer. That conflation is why a gizmo
drag and a scenario checkpoint are indistinguishable to save/undo.

## 6. Target: the resolver is the only local/client seam

```rust
StageBuilder::new().resolver(DiskResolver)        // local: reads the file
StageBuilder::new().resolver(ReplicatedResolver)  // client: bytes off the wire
```

A client has **no file and no cache** — it has a layer stack whose base is
resolved and whose live layer is replicated. Caching exists only inside the
client's resolver, because that is the only place with no disk. Asset sync (DEM,
meshes) is the same seam: content-addressed fetch over the USD reference closure.

Today USD layers are routed through **Bevy's `AssetServer`** instead — a
load-once/cache-by-path content pipeline built for meshes that never change under
you. That is why documents went stale, and why the overlay hack exists to force
our own truth back *into* that cache. `ar::Resolver` is the seam USD provides for
exactly this; our fork exposes `StageBuilder::resolver` and
`Resolver::get_modification_timestamp` already.

## 7. Target: collaboration (the Nucleus model)

1. **Nobody edits the original during a session.** Deltas land in the live layer
   (session-layer slot, topmost). Non-destructive by construction.
2. **Conflicts are per-property and resolve last-writer-wins** — silently, at
   interactive speed. No locks, no prompts, because collisions are rare when
   granularity is a property rather than a file.
3. **One explicit merge, owned by one person.** The session owner ends the session
   and merges the live layer down. All the hard cases concentrate there.
4. **Presence** (cursors, selection, camera) is its own layer — per-user, never
   merged.

**Two planes, and they must not be conflated:**

| plane | what | lives in | replicated by |
|---|---|---|---|
| **authoring** | who moved the rover's spawn point | USD live layer | Nucleus-shaped op stream, LWW per property |
| **sim** | where the rover *is* this tick | **ECS** (our Fabric) | rollback netcode (lightyear), 60 Hz |

Omniverse's model is *authoring* collaboration. **It is not a physics netcode and
does not subsume one** — their own physics reads Fabric, not USD, for exactly the
reason in §5a. We have only the sim plane today, and it replicates ECS/physics
state, not USD deltas.

The tempting mistake is to unify them — "everything is a USD op, replicate the op
stream". That is a performance cliff, not a simplification: it puts a per-frame
write onto a data model whose write cost is measured in milliseconds. The two
planes stay separate, and they meet only at explicit checkpoints.

## 8. UX: we are an IDE *and* a DCC

The two conventions genuinely disagree, and picking one wholesale is wrong:

| external change, clean buffer | |
|---|---|
| **IDE** (VSCode) | auto-reload silently — cheap, harmless, expected |
| **DCC** (Maya USD) | **never** auto-reload — explicit **"Revert to File"** |

Maya is right for a DCC: reloading mid-session invalidates composition, selection,
and running state. Resolution is per-scenario:

| scenario | behaviour |
|---|---|
| user re-opens a Twin (explicit "open") | refresh from disk — they asked for the file |
| re-open, document **dirty** | **prompt**: *Keep mine / Revert to file / Show diff* |
| file changed on disk, sim **idle** | IDE-style auto-reload is defensible |
| file changed on disk, sim **running** | **never** auto-reload — badge it: *"changed on disk — Reload (restarts scene)"* |
| client | N/A — no file; the live layer is the channel |

Required surfaces:

- **Dirty is visible** — an asterisk per tab/layer (Maya's minimum). A correct
  `KeptDirty` reported only to a log is a user seeing the old scene and being told
  nothing.
- **The edit target is visible** — which layer am I authoring to? This is what
  *prevents* conflicts (§5); users cannot reason about a conflict they cannot see
  the shape of. `ApplyUsdOp` already carries `edit_target`; nothing surfaces it.
- **Permissions** — USD has `SetPermissionToEdit` / `SetPermissionToSave`; Maya
  exposes them as three states (Unlocked / Locked / System-Locked). Our
  `DocumentOrigin.writable` and `accepts_mutations()` are reinventions of these.

## 9. Policy is ours; primitives are USD's

`SdfLayer::Reload()` is a **revert**: on a dirty layer the mtime check is skipped
and unsaved edits are **discarded**. USD deliberately ships the destructive
primitive and pushes policy to the app — Maya names it "Revert to File" and makes
the user ask.

So `OpenOutcome::KeptDirty` is *not* USD's behaviour. It is our policy layer, and
that is the correct place for it. A kept-dirty (or kept-unparsable) re-open is a
surprise the user must *see*, not a silent no-op: the USD open path raises a status
badge in UI builds alongside the log line. A modal "Reload / Keep my edits?" prompt
is the eventual form; the workbench has no modal infrastructure yet, so the badge is
the honest interim.

## 10. Disk staleness: detection is generic; policy is "badge, never reload"

A file can change **behind the app's back** — a git pull, an external editor,
another tool. `DocumentRegistry<D>` notices, for every document type by
construction:

- A **watermark** side-table records each file's mtime at the moment its bytes were
  read (`open_file`) or written (`note_saved`). It is a side-table, not a field on
  `DocumentOrigin::File`, precisely so it doesn't ripple through that enum's many
  match sites.
- `stale_docs()` stats each watermarked file and returns those whose mtime advanced
  past the watermark. A *vanished* file is not stale — a failed stat must never
  masquerade as "changed".
- `note_saved(id)` re-baselines after a save, so the app's own write is never
  mistaken for an outside edit. Wire it wherever a generic-registry document is
  saved (today: the USD save path; Modelica and scripting keep separate registries).

**Detection is split from policy on purpose.** Per §8, an external change while a
sim is running must **badge, never auto-reload** — a silent reload would restart the
world. `badge_externally_changed_usd_docs` polls on a throttle, dedupes so a
persistently-stale file nags once, and re-arms when the file re-syncs.

### The dependency closure is USD domain knowledge, and lives once

A document's dependencies are found by walking `subLayers` / `references` /
`payload`. That walk had **two** implementations — the composition pre-fetch in
`lunco-usd-bevy`, and a near-identical copy in `lunco-networking` for scenario
manifests, whose own comment conceded it "mirrors" the first. The copy existed
because there was no shared home, and it is why a *networking* crate depended on
`openusd` directly.

It now lives once, in `lunco_usd_bevy::closure`, with the single axis the two
copies actually disagreed on turned into a parameter:

```rust
discover_arcs(data, ArcFilter::LayersOnly | ArcFilter::All)
reference_closure(roots) -> BTreeSet<PathBuf>
```

`LayersOnly` for the composition pre-fetch (a `.glb` is not a layer to fetch —
the resolver stubs it); `All` for manifests and staleness (a client must receive
the `.glb`, and swapping a DEM must invalidate the scene pointing at it). The BFS
*drivers* differ legitimately — async-`AssetServer` versus synchronous filesystem
— and stay separate; only the per-layer extraction is shared.

`lunco-networking` no longer talks to `openusd`. Networking is not a USD crate.

## 10a. Remaining gaps

- **Staleness stats the filesystem directly; USD would route it through the
  resolver.** `ar::Resolver::get_modification_timestamp` is USD's canonical
  staleness primitive — it is what `SdfLayer::Reload()` consults for external
  dependencies, and per §6 the resolver is meant to be the only local/client
  seam. Ours returns `None` **by design**: `LuncoUsdResolver` is a pure in-memory
  byte map (the loader pre-fetches through Bevy's `AssetServer` so composition
  never touches the filesystem, which is what makes wasm work), so it has no
  filesystem knowledge to report a timestamp from. Routing staleness through it
  today would therefore detect nothing.
  Making the resolver the real seam means giving it **source provenance**: a
  version token per resolved id, recorded at fetch.
  That token must be a **content id, not an mtime**. The loader fetches through
  `LoadContext::read_asset_bytes`, which returns *bytes only* — Bevy's asset
  reader surfaces no mtime and no etag, and reaching for one would mean touching
  the filesystem, which is exactly what this path avoids so that wasm works. So
  hash what was fetched (`lunco-hash`, the same content addressing
  `scenario_sync` already uses for manifests). Content ids are uniform across
  native and client, and — unlike mtime — a `touch` with no edit does not
  false-flag.
  Note this does **not** flow back into `ar::Resolver::get_modification_timestamp`:
  that returns `SystemTime`, and it exists to drive openusd's *own* layer-reload
  decisions, which we do not use (no `find_or_open`, no layer cache). Provenance
  serves *our* staleness, and the resolver is where it belongs because it is the
  one place that knows what was actually loaded.
  Note the layering constraint: `lunco-doc-bevy`'s watermark serves `.mo`,
  `.rhai`, and `.usda` alike and must never call a USD resolver. The generic
  registry watches paths it is *handed*; the domain decides what they are.
- **The fork has no `find_or_open`** (a declared TODO in `sdf::LayerRegistry`,
  which today holds only a resolver and no layer cache). §5 is blocked on it.
  When that cache lands it **must** ship with `get_modification_timestamp`
  invalidation, or it reproduces the root-only staleness gap inside `openusd`. Note
  the C++ registry holds **weak** pointers — clients retain the strong refs.

## 11. The rule that generalises

`usd_tree.rs` found it first, for behaviour trees: topology stays BT.CPP XML
because it is *interchange* (Groot2, ROS/Nav2 edit it); the waypoints are USD
prims because a prim is *"selectable, draggable, deletable, journaled, undoable,
persisted, and replicated by the machinery that already serves every other prim."*

> **Before adding a document type, ask whether it should be a prim.**

A prim inherits everything. A document type inherits only what `FileBacked` gives
it — and owes you an answer on op addressing (§4) before anyone can collaborate on
it.
