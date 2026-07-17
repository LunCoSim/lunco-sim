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

`allocate` is for path-less origins (Untitled, Bundled) and session restore only.

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

## 5. Target: the layer model

**Never edit the base layer live.** Edits land in a layer *above* it; composition
resolves by strength. Then a base reload is conflict-free — the user's opinions
ride on top — and most conflict UX dissolves instead of needing a merge dialog.

```
[ base layer    ]  file-backed, resolved via ar::Resolver, never live-edited
[ live layer    ]  replicated ops, LWW per property        (Nucleus `.live`)
[ session layer ]  local, ephemeral, never saved            (camera, selection)
```

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

| plane | what | mechanism |
|---|---|---|
| **authoring** | who moved the rover's spawn point | USD ops on a live layer, LWW, Nucleus-shaped |
| **sim** | where the rover *is* this tick | rollback netcode (lightyear), 60 Hz |

Omniverse's model is *authoring* collaboration. It is not a physics netcode and
does not subsume one. We have only the sim plane today, and it replicates
ECS/physics state, not USD deltas.

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
that is the correct place for it.

## 10. Known gaps

- **Referenced files and assets do not invalidate.** `SdfLayer::Reload()` consults
  *external dependency* timestamps; `open_file` checks only the root. Editing
  `wheel.usda` or swapping a DEM invalidates nothing, silently.
- **`allocate` still accepts a `File` origin**, so a new document type can bypass
  §2 and mint duplicates. The by-design fix is an origin type that *cannot express
  a path*, plus a named `restore()` for session restore.
- **The fork has no `find_or_open`** (a declared TODO in `sdf::LayerRegistry`,
  which today holds only a resolver and no layer cache). §5 is blocked on it.
  When that cache lands it **must** ship with `get_modification_timestamp`
  invalidation, or it reproduces our staleness bug inside `openusd`. Note the C++
  registry holds **weak** pointers — clients retain the strong refs.

## 11. The rule that generalises

`usd_tree.rs` found it first, for behaviour trees: topology stays BT.CPP XML
because it is *interchange* (Groot2, ROS/Nav2 edit it); the waypoints are USD
prims because a prim is *"selectable, draggable, deletable, journaled, undoable,
persisted, and replicated by the machinery that already serves every other prim."*

> **Before adding a document type, ask whether it should be a prim.**

A prim inherits everything. A document type inherits only what `FileBacked` gives
it — and owes you an answer on op addressing (§4) before anyone can collaborate on
it.
