# 55 — Scene Addressing and Roots

> Status: Active · Audience: contributors on scene loading, twins, and path resolution
>
> Supersedes the former ad-hoc "promote an out-of-assets path" loader.

## The former failure mode

Historically, opening a scene outside the workspace `assets/` directory failed:

```
WARN [scene] `/home/rod/Documents/models/summer_space_school/sim/scenes/traverse.usda`
     is outside assets dir — load it via the Twin (`twin://`) source
```

The runtime now resolves the owning root before loading and reports an invalid
root visibly. The rest of this document is the current contract; the historical
failure is retained only to explain why the boundary exists.

## Address identities and the boundary

A scene enters the runtime in one of two address forms. A filesystem path is an
input to an open command only; it is converted to a rooted address before the
scene lifecycle starts:

| Identity | Rooted at | Who produces it |
|---|---|---|
| bare relative (`scenes/x.usda`) | *implicitly* `assets/` | authoring/CLI input only; never a `LoadScene` address |
| `lunco://<rel>` | the engine asset library | shipped/portable refs |
| `twin://<name>/<rel>` | a registered Twin root | the Twin-open flow |
| absolute fs path | nothing | **every user-facing picker** |

The last is what a human always has — a file dialog, a CLI arg, a drag-drop —
and it is the only one with no first-class home.

The first is actively dangerous. A bare relative path means "resolve against the
default source" — but once a Twin root is open, the same string resolves against
*the twin* instead, and a miss is a **silent no-load**: no error, just an empty
scene. Two spellings of the same intent with different, context-dependent
meanings is not a convenience; it is a correctness hazard.

The current entry points are deliberately split by ownership:

- `validate_scene_address` (`lunco-usd-bevy-runtime-core/src/scene.rs`) — accepts only
  registered scene schemes and rejects bare or filesystem paths.
- `lunco_assets_core::engine_asset_uri` — converts an in-tree library reference to
  its canonical `lunco://` address at command boundaries.
- `load_startup_scene` (`lunco-luncosim-services/src/lib.rs`) and the USD `on_open_file`
observer (`lunco-usd-commands/src/lib.rs`) both resolve the owning root and enter
  the same asynchronous Twin scan; its completion registers the root and
  enters the same doc-first `LoadScene` path.

The root resolver and async Twin scan are shared; only the entry-point adapter
differs (startup configuration versus the typed `OpenFile` command).

### The root cause

`assets/` is **privileged**. It is the default `AssetSource`; everything else is
a second-class citizen needing a "promotion" step. Every branch of the form
"…but what if it's outside assets?" descends from that asymmetry. Adding a
promotion path (as an earlier patch in this branch did) preserves the asymmetry
and adds a fourth conversion site. It is the wrong direction.

## Principle

> There is exactly one question: **given a path a user chose, what is its root,
> and what is the path relative to that root?** Everything else is a consequence.

Two corollaries, both non-negotiable:

**Every scene address is scheme-qualified.** Bare relative paths do not survive
past the boundary. There are exactly two schemes:

| Scheme | Root | Use |
|---|---|---|
| `lunco://` | the workspace asset library (`assets/`) | **all shipped/in-tree assets** |
| `twin://<root>/…` | a registered user root (Twin or Folder) | anything the user opened |

**`assets/` is addressed via `lunco://`, never as the implicit default.** It is
one root among several with no special powers. Its content is reached by
`lunco://…`, exactly like an external root is reached by `twin://…`. This is
what makes shipped assets portable: a `lunco://` ref means the same thing when
the scene is loaded from an external twin, whereas a bare relative path silently
re-roots and fails to load.

"Outside assets" then ceases to be a concept — there is no inside.

## Target model

### 1. A Root is the unit of resolution

A **root** is a folder that anchors relative references. USD references are
relative (`@terrain/apollo15@`, `@./wheel.usda@`), so a scene cannot be loaded
in isolation — it always resolves *through* a root. User-opened roots are
modelled by `lunco_twin::TwinMode`:

| `TwinMode` variant | Detected by | Notes |
|---|---|---|
| `Twin(Twin)` | folder contains `twin.toml` | manifest, libraries, ref repair — the full experience |
| `Folder(Twin)` | folder opened, no `twin.toml` | files indexed for browsing; no manifest, no ref repair — the VS Code "Open Folder" analog |
| `Orphan(PathBuf)` | a single file opened outside any folder context | no sibling files known; the file's parent directory is the root |

The builtin root — the workspace `assets/` dir — is not a `TwinMode` variant;
it is pre-registered directly as the `lunco://` root.

`Folder` and `Orphan` are **first-class** kinds, not degraded ones. This
answers "what if there's just one scene and no twin?" — its parent directory is
the root. No `twin.toml` is required, and its siblings resolve correctly.

### 2. One resolver, no World access

```rust
// lunco-twin — pure, testable, no ECS
pub fn root_for_file(file: &Path) -> PathBuf   // nearest twin.toml ancestor, else parent
```

Implemented. The services `load_startup_scene` and the interactive open path both use this
resolver; neither performs its own ancestor walk.

### 3. Mount addresses and stable source identity

`TwinRoots` owns both the load authority for one mount lifetime and its stable
logical source name. Simultaneous roots with the same requested logical name
are disambiguated as `name-2`, `name-3`, … . Repeated admission of the same live
root/name is idempotent. Unmount removes the root and composed overlays; a
reopened folder receives a new load authority even when its logical name is
unchanged. No retired load authority is rebound, so Bevy's same-path asset
cache and late readers cannot supply a replacement Twin with outgoing bytes.

Callers must use the authority returned by registration for `twin://` loads,
document coordinates, overlays, relative dependencies, and Rhai imports.
`lunco_assets_core::stable_source_path(path, roots)` converts a Bevy `AssetPath`
to the stable, scheme-stripped source used for USD content provenance.
Non-Twin sources require no registry; Twin sources require the supplied
`TwinRoots` owner. Retired mounts keep
only authority-to-logical-name strings for that conversion; no roots, bytes,
or handles remain. An unknown Twin authority is an error, never a raw-name
identity fallback.

Scenario manifests export the logical source name; each peer resolves it to
its own current mount authority. Different local mount histories therefore
keep matching content identities. `logical_name` performs that export and
`mounted_name_for_logical` lets a downloaded scenario reuse an existing local
root. These conversions belong to the asset owner. Load addresses and script
import registry keys retain their mount lifetime instead of being normalized
to a reusable logical name.

### Document source admission and lifetime

A file location is the transport and save identity, independent of the document's
runtime owner. `FileDocumentAdmission::capture` records the current canonical
Workspace roots and exact replicated owner before dispatch. Its shared `read`
resolves the native or OPFS source identity and reads through storage on the
existing worker. USD and SysML validate the returned `DocumentRuntimeOwner`
before installing source, and register its actual origin, runtime owner and dirty
state before document events. A pending read cannot publish into a successor
Twin; identical requests coalesce only with identical captured admission facts.
Dirty resident source cannot be rebound to another owner.
`read_bounded` uses the same admission and backend selection with a caller byte
limit. Native and OPFS readers enforce it before returning source bytes; private
browser storage checks decoded hex length before allocating its byte buffer.
The browser still supplies its encoded DOMString. Read failures retain their
storage diagnostic and never become a missing or empty source.

Browser file selection carries exact request-owned bytes through `PickedPath::BrowserFile`.
USD and SysML prepare those bytes through their existing asynchronous pipelines and install a fresh
pathless Application document using the registry's reserved identity and
`install_prebuilt`. The selected filename is presentation only; it never becomes
a native path, Twin address, or browser storage key. Repeated names retain
separate identities. Invalid UTF-8 produces a terminal diagnostic without
installing a document. Native picker results retain typed storage paths. Source
Save-As pickers capture the exact stored owner before backend dispatch; completion
revalidates the same pin. Browser download admission is fallible, and a failed
download does not mark source saved or publish `DocumentSaved`.

New documents capture their creation context at dispatch; forks retain the
source document's admitted owner. Private session snapshots explicitly belong to
Application. Saving or opening a folder never reassigns runtime lifetime from
the path. Indexed Twin source loads and automatic leases carry exact `TwinId`
plus their captured live mount. Retirement checks the stored document owner
before removing a host, including after a clean source replacement. Preview
admission likewise checks the stored owner before rehoming a document projection.

### Native payload and source admission

USD scene, document-source, preview, schema, reference, and transitive-layer
loads reconstruct the typed address through `load_asset_path`. The registered
source and filesystem path remain separate through Bevy loading; logical
filenames containing `#`, `%`, spaces, or Unicode remain filename data. Labels
are attached explicitly with `AssetPath::with_label`. Scene transition strings
and canonical recipe keys describe identities and never replace the typed load
address. Full scene restart retains the asset server's existing typed path.

Composed default `asset` and `asset[]` values retain their contributing layer's
canonical identifier. The vendored OpenUSD owner uses its existing strongest
opinion, expression-variable context and `create_identifier` result even when
no external payload is loaded. The derived annotation is not serialized and
does not change authored strings or equality. `UsdRead::asset` remains the
raw authoring/query read; I/O consumers use fallible `asset_identifier`. A
consumed value without context fails visibly. Time-sampled assets currently
lack contributing-layer annotations and are rejected when consumed rather
than anchored to the scene root.

Asset values in a Twin follow OpenUSD's `ArDefaultResolver` spelling rules.
`./x` and `../x` are strictly layer-relative. A search path, a relative
spelling without a scheme or leading `./`/`../` such as `@terrain/site@`,
resolves beside its authoring layer when that location exists, otherwise from
the Twin root. Both candidates use the Twin's authored-tree, Twin-cache,
shared-cache order, so a scene under `sim/scenes/` can name a processed Twin
dataset declared as `output = "terrain/site"`. A search path found in neither
place keeps its Twin-root identity; its consumer reports the miss or, for a
declared dataset, offers the download. The existence check runs once in the
stage's native preparation worker (`PreparedAssetPaths`, typed
`AssetReference::Search`), including incremental live-change preparation;
`asset_identifier` returns the prepared result and rejects a search path whose
source revision has not been prepared. Composition arcs (references, payloads,
sublayers) remain layer-anchored. Browser Twins cannot probe OPFS from the
synchronous preparation pass, so they select the layer-anchored candidate.

Programs, PBR textures, dome textures, WGSL sources/texture inputs and body
albedo maps pass that identifier through stage preparation and the shared typed
load boundary. The render owner reconstructs typed shader/image paths once;
procedural sky uses the already admitted shader handle. Child references and
sublayers therefore keep their source directories on initial and live projection.
DEM projection likewise retains its canonical directory address and derives
dataset readiness from that address's actual authority and relative path.
`resolve_asset_directory_on_worker` selects the confined directory transport
inside the existing terrain I/O worker; mounted browser Twins stay in OPFS.
The existing bake task/job checks its never-rebound mount authority before
publication and scene teardown cancels it. Directory lookup never runs on the UI
thread and does not read a directory as an asset file.

Native USD composition uses standard `file:` URIs. Bevy does not register a
filesystem source. `lunco-assets-core::asset_path::PreparedAssetPaths` prepares
native references through `lunco-storage::canonicalize_file_path` on the stage
worker, checks canonical containment, and returns Twin-relative typed
`AssetPath` results. Filesystem canonicalization owns Windows case-variant
directory names, UNC/verbatim prefixes, and symlink/junction resolution;
projection never performs filesystem lookup or case folding. The stage's
`AssetServer` origin supplies the exact lifetime authority. Manifest names and
native recipe roots cannot replace it. Missing, retired, non-Twin, malformed,
and outside-root references retain terminal errors for their owning consumer.

The initial loader and dependent-plan worker discover native values through
composed USD asset attributes, arrays, time samples, and binary arcs. Their
immutable table travels with the projection plan. Live sink changes share one
bounded `AsyncWorkAdmission` gate before material, light, program, policy, or
structural consumption. The gate prepares only native inputs absent from the
current table and retains newer changes and transform hints while work is
pending. Publication rechecks the exact canonical-stage lifetime/generation,
origin and live mount; stale completion cannot admit or release newer work.
An authoritative stage holds `UsdNativeAssetPreparation` through the admitted
live projection, and teardown cancels queued work, drops its completion channel,
and releases only its own holds. Ordinary transform edits reuse the shared
table without native I/O or a whole-stage snapshot.

Native USD layer references use the same confined address preparation. The
closure loader prepares each native child on its existing I/O worker and keeps
the canonical file URI as its recipe key. Incremental reference spawns and
coarse document rebuilds share `PendingRefSpawns` preparation: bounded workers
prepare the address, retain the real Twin-source asset handle, and compose a
`StageRecipe::reanchor` snapshot under the authored canonical root. Reanchoring
uses the shared USD dependency reader, changes relative layer anchors, preserves
explicit absolute identifiers and layer bytes, and produces no alias keys.
Each live instance retains the real source handle and a shared
`UsdReferenceSnapshot`: the canonical recipe, unscoped prepared plan and actual
loaded source recipe/plan revision. The canonical stage's existing reference
cache holds only weak snapshot/recipe identities. A sibling obtains the actual
loaded handle through the cached typed source address, then rechecks the exact
scene origin, current source revisions and live native admission tables before
sharing that snapshot. This path performs no filesystem work or composition;
expired snapshots and changed sources require preparation on the reference lane.
Completion checks the exact stage lifetime/generation, source origin, worker
operation and live mount, plus the loaded source recipe/plan revision. Native
tables merge into the same scene admission cache before instance publication.
Reference and document-projection holds remain in force through their existing
ordered commit; failure retains the terminal hold. Replacement or teardown
cancels queued work and outgoing completion channels cannot reach a new request.

`load_asset_path` resolves the prepared table without filesystem I/O, rechecks
the live mount, and rejects an absent current entry. The asynchronous Twin reader
continues to validate canonical containment before reading bytes.
The shared USD `resolve_stage_asset_path` returns `Result<AssetPath>` and all
binary, material, dome, scenario, and policy consumers keep that typed value
through `AssetServer::load`. A binary's glTF label is attached with
`AssetPath::with_label`, separately from the filename, so encoded `#`, `%`,
spaces, and Unicode remain filesystem characters. `EmbeddedScenarioPath` and
pending policy source keys carry the same typed address. Rhai import loading
uses the existing script-source canonical path/extension rule and this typed
asset adapter; filesystem `#` is never reparsed as an asset label. Interpreter
and prepared-module registry identities remain opaque string keys. Invalid live material
or dome replacements report the error and retain the previous valid intent.

### 4. One mount path, always doc-first

```
resolve root  →  register root  →  open document  →  set overlay  →  LoadScene(twin://…)
```

The overlay is registered **before** `LoadScene`, or the load reads base-only
bytes and silently drops placed waypoints, runtime spawns, and moved transforms.
If the owning root cannot be opened, the load reports an error and stops; it
does not fall through to a base-only `LoadScene`.

## Commands: no new ones

Four commands already cover the surface. They become thin delegates over one
implementation:

| Command | Takes | Role |
|---|---|---|
| `OpenFile { path }` | **filesystem path** (or scheme); empty opens the picker | resolves the owning root, registers it, mounts doc-first |
| `OpenFolder { path }` | folder | same mount, root given explicitly |
| `OpenTwin { path }` | folder, strict (requires `twin.toml`) | same mount |
| `LoadScene { path }` | **scheme address only** (`lunco://`, `twin://`) | loads an already-addressable asset |

`OpenFile` is already the UI's File→Open command and already accepts an
arbitrary path, so opening any `.usda` anywhere works from the UI with **no new
command and no new UI surface**. The USD observer resolves the owning folder,
uses the shared asynchronous Twin scan, selects the requested file, and lets
the `TwinAdded` observer perform the same doc-first mount as startup.

### Why `LoadScene` does not take filesystem paths

This is a layering constraint, not a preference. `LoadScene` lives in
`lunco-usd-sim`, which depends on neither `lunco-workspace` nor `lunco-twin`;
`lunco-workbench` in turn does not depend on `lunco-usd-sim`. The two sit in
disjoint layers, so `LoadScene` **cannot** resolve a root or fire `TwinAdded`
even if we wanted it to.

That falls out cleanly rather than awkwardly: path→root resolution is a
workspace concern and belongs with the other open commands, while `LoadScene`
stays the low-level "load this address" primitive. It also enforces the
scheme-qualified rule at the only place that can enforce it — a bare path is
*rejected* with a message naming `OpenFile`, instead of being silently
re-rooted.

Programmatic callers (API / MCP / rhai) that have a filesystem path therefore
call `OpenFile`, which is already API-accessible. Still no new commands.

## Current implementation invariants

- Root discovery is owned by `lunco_twin::root_for_file`.
- `TwinRoots` returns the mount authority and never rebinds it after retirement.
- Open flows register the root, mount the document overlay, and only then load
  the `twin://` scene.
- Filesystem scene paths are canonicalized through `lunco-storage` before root
  discovery and containment checks. If canonicalization fails, `OpenFile`
  reports the error and stops; it never falls back to a lexical path.
- `LoadScene` accepts already-addressable scheme paths; filesystem paths go
  through `OpenFile`, which owns root discovery and document mounting.
- Invalid roots and registry failures are reported at their owner. They are not
  converted into an empty scene or a base-only fallback.

## UX consequences

- **One Open.** File→Open… takes a scene file *or* a folder. No "Open Twin" vs
  "Open Folder" vs "Open Scene" decision forced on the user.
- **Opening a scene opens its root** as the workspace folder, so the browser
  panel shows its siblings — VS Code semantics, and the reason a root must be
  registered rather than the file loaded in isolation.
- **Recents** list roots, so reopening is one click.
- **Unresolved references surface.** A missing co-located ref must raise a
  `StatusBus` warning naming the ref. Today a scene whose refs fail can mount
  visibly empty, which reads as "the app is broken".

## Risks and edges

| Risk | Handling |
|---|---|
| wasm has no filesystem | roots stay overlay/HTTP-backed; the web autoload hook already loads its twin directly and must keep bypassing fs walk-up |
| read-only or system dirs as roots | registering a root must not imply write access; save-as chooses a writable root |
| a root nested inside another | prefer the nearest `twin.toml`; `root_for_file` already does this |
| ordering regressions | overlay-before-load is a correctness invariant, not a nicety — worth a test that asserts a runtime edit survives a reload |

## Verification

The root and overlay contracts are covered by focused registry/lifecycle tests
and the production `luncosim` API path. Any change to scene mounting must verify
both a valid Twin load and a rejected invalid root; a rejected load must leave
the previous scene intact and publish a visible status error.
