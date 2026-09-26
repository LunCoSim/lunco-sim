# 16 — Document Identity, Conflicts, and Collaboration

> Status: Design · Audience: contributors touching documents, registries, assets, or multi-user
>
> ⚠️ **PART DESIGN SPEC.** §1–§4 and §5–§5a describe the current identity,
> authoring-layer, and simulation-state contracts. §6 onward remains the
> collaboration target: the `ar::Resolver` seam is unused, `find_or_open` does
> not exist in our `openusd` fork, and there is no replicated live layer.

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

Live handles are allocated by `lunco_doc::DocumentId::fresh`, using the shared
core 53-bit ID generator for USD, Modelica, scripts, and shaders. These handles
fit losslessly in JSON/browser numbers. Opening, reserving an
async document, and forking all use this owner. Closing a document or replacing
a registry never reuses its handle. `DocumentId::new(raw)` decodes an existing
handle; it is not an allocation API. File-path deduplication still belongs to
the registry, and session restore remaps stored handles to fresh live ones.
Journal-only registration and experiment keys are not live document handles.

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

## 5. As-built: authored, runtime, and view layers

USD authoring follows the non-destructive layer pattern: keep the shipped or
Twin-authored source intact and write local edits into an explicit stronger
layer. Each edit names its target; the composed stage resolves the result.

```
DOCUMENT — authored USD opinions and derived view, composed in strength order.
  [ @root@    ] source scene; saved to its source file on explicit Save
  [ @runtime@ ] local authored edits; separately persisted under .lunco/runtime
  [ @view@    ] derived presentation; disposable, never saved or journaled
  [ ECS       ] continuous simulation and editor session state
```

`@runtime@` is an authored document layer, not a USD session layer. User edits
such as route points and gizmo transforms are journaled typed operations and
can be saved independently of the source `.usda`. When the owning Twin enables
`usd.runtime_persistence`, the runtime layer is serialized to
`.lunco/runtime/<scene-path>` and restored before the next scene mount.

`@view@` contains disposable projections such as route ribbons and visited
marker colors. `ApplyUsdOps` rejects this target; derived presentation uses the
transient USD command, which updates the live stage but does not alter the
runtime sidecar or undo history.

The initial Twin scene asset contains `@root@` and `@runtime@` only. Its live
projection tracks `@view@` operations separately and replays them onto the
mounted canonical stage, including view operations authored before that stage
was mounted. This keeps transient annotations visible in the live View without
adding them to the Twin overlay.

Omniverse makes the active authoring layer visible in its Layers panel, while its
Session Layer is temporary working state. LunCoSim's persisted `@runtime@` is a
different contract: it is Twin-owned authoring that can survive reopening, not
the transient Session Layer. Route and runtime tools select it by policy today.
The expected editor experience is to name that target beside the active tool,
show whether runtime persistence is enabled, and expose save progress and the
last durable state. A user should be able to tell that a waypoint was authored
to the Twin runtime sidecar without opening a layer inspector or guessing
whether the source `.usda` changed.

The viewport has the same visibility requirement for gestures. Omniverse's
`GestureManager` resolves competing gestures by priority and prevents one input
from triggering multiple actions. LunCoSim's Rhai router currently arbitrates
the unarmed route-edit and selection gestures; armed spawn, terrain, attachment,
possession, camera, and gizmo paths still have engine-owned input consumers.
The editor gizmo captures a primary gesture from the current gizmo-proxy hit
and retains preview pan suppression through release or cancellation. Other
tools do not yet share that lifecycle. The robust target is one pointer gesture
lifecycle: the engine gathers hit and capture facts, a typed Rhai policy chooses
one owner and action, and generic Rust mechanisms apply that action. A drag
remains captured until release or cancel. The USD per-button hit policy is
translated into Bevy's ordered-hit
contract before its picking backend runs, so a route marker may pass through
for primary selection while remaining the actual secondary context target.
Route target identity comes from the hit paths, not screen proximity. A
waypoint context click opens its menu without selection or gizmo activation;
selecting it is a separate menu action. Rhai owns the meaning and chosen action;
continuous picking, gesture capture, transform math, and generic gizmo
application remain engine work.

For route authoring, the user should see `Twin Runtime` as the active target,
receive immediate point and ribbon feedback without scene reload, and see
whether the sidecar is saving or saved. Visited points turn gray. Autopilot and
possession are separate controls: releasing the rover hides its driving HUD,
while the route program continues until stopped or complete. Repossessing the
rover restores its HUD without restarting the route.

**Per-tick simulation state is NOT in this stack, and must never be** — see §5a.

## 5a. Per-tick simulation state stays in the ECS

USD layers own scene structure and user-authored edits. They do not store
continuously changing simulation state. The runtime document layer changes on
explicit authoring actions; route ribbons and visited marker colors change on
route revisions or sensor events. Neither is rewritten every frame. Runtime
layer serialization runs asynchronously from coalesced snapshots, while the
live projector applies typed operations incrementally to the already-mounted
stage.

> **Never author per-tick simulation state as a USD op.** A rover's position each
> frame belongs in Bevy ECS / Avian, while Modelica owns continuous equations.
> Checkpointing is an explicit authored action with its own owner; it must not
> turn the simulation tick into a stream of USD writes.

The four lifetimes are distinct: source USD, journaled runtime-layer authoring,
disposable view-layer presentation, and continuous ECS/Modelica simulation.
Keep those owners separate so an editor gesture cannot rebuild the running
simulation or persist presentation state.

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
  the shape of. `ApplyUsdOp` carries `edit_target` through its typed `UsdOp`, and
  its acknowledgement exposes the selected layer and affected paths.
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
is still future policy work for this open path. The shared `lunco-ui::modal` host
now provides the queue, scrim, focus, Esc dismissal, and typed close/outcome
plumbing; until this policy is connected to it, the badge remains the honest
interim.

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

### Dependency closure separates asset traversal from USD interpretation

A document's dependencies are found by walking `subLayers`, `references`,
`payload`, and asset-valued attributes. There must be one filesystem traversal:
`lunco_assets_core::transitive_file_closure*` owns its queue, canonical paths, and
native reads. `lunco-usd-compose` supplies the format facts:

```rust
is_usd_layer(path)
layer_dependency_arcs(text)
```

The scenario manifest and live scene browser call those APIs directly. Thus
networking does not read or parse USD files, `lunco-usd-bevy` has no closure
adapter, and a `.glb`, Modelica model, policy, or texture reaches the closure as a
leaf once USD declares it. Stage pre-fetch remains its own async AssetServer
operation; it reuses `child_layer_ids` because it needs only parseable layers.

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

The current route design generalises the same rule: route topology and point
identity are USD prims because a prim is *"selectable, draggable, deletable,
journaled, undoable, persisted, and replicated by the machinery that already
serves every other prim."* Route execution policy is a sibling Rhai program.

> **Before adding a document type, ask whether it should be a prim.**

A prim inherits everything. A document type inherits only what `FileBacked` gives
it — and owes you an answer on op addressing (§4) before anyone can collaborate on
it.
