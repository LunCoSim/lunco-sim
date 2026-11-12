# 55 — Scene Addressing and Roots

> Status: Active · Audience: contributors on scene loading, twins, and path resolution
>
## Address identities and the boundary

A scene enters the runtime in one of two address forms. A filesystem path is an
input to an open command only; it is converted to a rooted address before the
scene lifecycle starts:

| Identity | Rooted at | Who produces it |
|---|---|---|
| bare relative (`scenes/x.usda`) | *implicitly* `assets/` | authoring/CLI input only; never a `LoadScene` address |
| `lunco://<rel>` | the engine asset library | shipped/portable refs |
| `twin://<source-identity>/<rel>` | a stable Twin source | scene UI and user commands |
| `twin://<authority>/<rel>` | a registered Twin mount | internal asset loading |
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

## Principle

> There is exactly one question: **given a path a user chose, what is its root,
> and what is the path relative to that root?** Everything else is a consequence.

Two corollaries, both non-negotiable:

**Every scene address is scheme-qualified.** Bare relative paths do not survive
past the boundary. User-facing scene commands accept stable source URIs:

| Scheme | Root | Use |
|---|---|---|
| `lunco://` | the workspace asset library (`assets/`) | **all shipped/in-tree assets** |
| `twin://<source-identity>/…` | a registered Twin source | anything the user opened |

The asset reader uses `twin://<authority>/…` internally for each admitted
mount. The user-facing source URI resolves to that address at scene-command
admission. The authority stays out of status text, scene menus, and
`CurrentScenePath`.

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

### 3. Stable source URIs and internal mount leases

The user-facing scene URI is stable across reloads and Twin reopenings:
`twin://<source-identity>/<relative-path>`. The loading notice, scenario-menu
entries and `CurrentScenePath` publish this address, never the asset-cache
authority. Generic simulation hold reasons stay pathless; detailed
`/api/ready` scene-load labels publish the stable source URI. The readiness
owner converts its internal load lease with `source_uri_for_address` before
creating the ticket and uses a generic `Twin scene` label if that conversion is
unavailable. The same displayed URI is accepted by `LoadScene` after reopening.
`lunco_assets_core::asset_path::source_uri` owns conversion from a load address;
`load_source_uri` resolves a user command against the currently mounted source
before scene-transition admission. The command rejects internal mount authorities,
including live ones; only a stable logical source identity is accepted. Unknown
sources are rejected before teardown. A logical source identity that collides
with a current or retired mount authority is rejected at mount admission.
Background readers retain exact mount leases rather than resolving public
addresses again after their owner retires.

`TwinRoots` assigns a fresh opaque UUID authority for each mount lifetime and
keeps source identity and display metadata separately. The authority remains
inside the Rust asset owner: Twin lifecycle hooks, Twin policy actions, and
Twin-level scripting APIs do not receive it. The lifecycle owner binds its
known workspace Twin to reflected commands that declare a typed `twin_id`
field. Application-level selection may use that workspace ID; Twin policy does
not. A Twin manifest UUID
produces the stable source component `uuid-<simple-uuid>`; legacy manifests and
plain folders without a UUID use their percent-encoded display name. Renaming a
UUID-backed Twin preserves its source identity; for a source without a UUID,
the display name is its stable identity. Duplicate live UUIDs are rejected. Any
duplicate source identity from a different live root is rejected; the registry
does not assign an order-dependent suffix. The display label is never used as
an asset cache key.

The Rhai boundary applies the same scope rule to query, command, hook,
policy-status, and compound reflected `get`/`get_setting` results: Twin-scoped
scripts do not receive workspace selector fields such as `twin_id`,
`owner_twin_id`, `target_twin_id`, `active_twin`, `workspace_twin`, `scope_id`,
or mount-authority fields, including nested records in arrays or maps. Direct
reflected reads of a selector field or nested workspace Twin ID return no value
in Twin scope; app-scoped scripts retain them. Twin settings and generic engine
exposure values pass through the same projection, so an extension cannot
reintroduce a selector through a scalar or nested map.
`ScriptingCatalog` hides those identity parameters from Twin-scoped command and
query discovery, and `ScriptComplete` applies the same filtering to completion
schemas. Twin-scoped query schemas omit response descriptions that could name
application selectors. The Rust bridge still binds the typed command target
from the active runtime owner. Mount-local authorities in script values and
diagnostics are translated through `TwinRoots` to stable source identities,
including late failures from retired mounts. Unknown opaque mount tokens are
redacted when their lease metadata is unavailable. Application-scoped scripts
and external application APIs retain workspace selector fields, but never
receive the internal mount lease. Rhai callers request only the catalog sections they
consume so the typed result stays within the shared runtime value limit;
omitting `sections` preserves the full application catalog.

Repeated admission of the same live root and source identity is idempotent.
Unmount removes the root and composed overlays; a reopened Twin receives a new
authority while keeping its source identity. No retired authority is rebound,
so Bevy's same-path asset cache and late readers cannot supply a replacement
Twin with outgoing bytes.

Internal asset owners use the authority returned by registration for `twin://`
loads, document coordinates, overlays, relative dependencies, and Rhai source
imports. These are Rust-held mount leases.
`lunco_assets_core::stable_source_path(path, roots)` converts a Bevy `AssetPath`
to the stable, scheme-stripped source used for USD content provenance.
Non-Twin sources require no registry; Twin sources require the supplied
`TwinRoots` owner. Retired mounts keep only authority-to-logical-identity
strings for that conversion; no roots, bytes, or handles remain. Live mount
metadata also carries the display label used by runtime consumers. An unknown
Twin authority is an error, never a raw-name identity fallback. Application-
level workspace selection may use `TwinId`. Twin-scoped Rhai facts expose the
stable source identity and a separate display name; they do not expose the
workspace ID or mount lease.

Scenario manifests export the stable source identity; each peer resolves it
to its own current mount authority. Twins with authored UUIDs therefore keep
matching content identities across names, paths, and local mount histories.
`logical_identity_for_authority` performs that lookup and
`mounted_authority_for_logical` lets a downloaded scenario reuse an existing
local root. These conversions belong to the asset owner.
Load addresses and script import registry keys retain their mount lifetime
instead of being normalized to a reusable logical identity.

Composed USD asset facts can retain an authored stable source identity, such as
`twin://astrobotic-griffin-1/models/controller.mo`. Scene dataset admission
resolves that logical source against the currently mounted Twin before reading
its root or dataset declarations. A current opaque mount authority is also
accepted internally; a retired authority is never reinterpreted as a source
identity. If neither address names a current mount, the scene reports a generic
missing-source fault without exposing the mount lease.

A Twin without an authored UUID derives its source identity from its encoded
display name. Two same-named legacy roots therefore cannot be mounted at once;
author distinct UUIDs when both must be open simultaneously.

### Document source admission and lifetime

The document projection owner records the committed `UsdStageAsset` ID.
Document queries, typed edit validation, and Rhai entity lookup read that ID
through `stage_asset_for_document`; they do not rediscover it by filename.
Source-text and stage assets can share a literal path while retaining different
typed identities. Before projection commits an ID, no live document stage is
available through this boundary.

An explicit `LoadScene` for a Twin's default scene can arrive while its initial
document source is still being prepared. `PendingTwinDocs` retains that stable
URI and root selection, coalesces later requests for the same source, and
dispatches the selected request only after the exact document stage-input lease
has been published. The asynchronous stage loader therefore observes one
document-input owner for its whole preparation; it never starts from raw file
bytes and changes to document-backed input mid-load.

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
The replacement root stays in `UsdSceneAwaitingStage` until the reload publishes
`LoadedWithDependencies`. A cached stage under the same handle cannot admit
the new mount before its fresh source closure and native addresses are prepared.
The active document projector defers source-reset replay during this reload.
After the matching loaded event, it records the generation serialized into the
restart overlay and closes its projection cursor only after ECS admission.
Edits after that serialized generation retain their normal pending suffix.

Composed `asset` and `asset[]` reads retain the contributing spec reported by
OpenUSD's `Attribute::resolve_info_at`. Prepared plans capture source-layer
identity with default and sampled asset values; live reads ask the same native
owner. `UsdRead::asset` is the authored/query spelling, and fallible
`asset_identifier` anchors the evaluated value against its source layer before
external bytes are fetched. A consumed value without contributing-layer context
fails visibly. Authored strings and serialized values remain native USD data.

Source-layer serialization selects the native format by exact layer identity
and requires its `FileFormatCaps::EDIT` capability before publishing replacement
bytes. Native USDZ supports a fresh single-layer export but rejects replacement:
the document's `Sdf Data` does not retain the package's sibling layers and assets.
Package entry identity and provenance must reach the storage/document owner
before USDZ can be admitted as an editable source.

Dependent-source authored-layer transport preserves composition arcs and uses
native recursive dictionary composition (`sdf::Dictionary::over`) and list-op
composition (`ListOp::combined_with`). Irreducible list operations reject the
transport with the field and path; neither source is changed. Full-stage flattening consumes those arcs
and cannot replace the authored-layer transport.

Asset values in a Twin follow OpenUSD's `ArDefaultResolver` spelling rules.
The Twin root is an explicit LunCoSim resolver context; USD does not infer a
project root from an asset path. Native scene-test discovery admits the same
owning Twin and uses the same composed asset preparation as runtime loading.
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

Programs, PBR textures, dome textures, WESL sources/texture inputs and body
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
The Twin directory resolver accepts the manifest's `.` root designator for
Modelica source directories and retains the admitted mount identity. Concrete
file references keep their stricter child-path contract; parent traversal is
rejected in either case.

Native USD composition normalizes authored Windows drive-absolute and UNC
references through `lunco-storage::windows_file_reference_uri` to standard
`file:` URIs. Drive-relative references are rejected, and a foreign Windows
filesystem reference on another host produces an explicit resolution error.
Logical root-relative `/...` references retain the engine/Twin URI rules.

Native USD composition uses standard `file:` URIs. Bevy does not register a
filesystem source. `lunco-assets-core::asset_path::PreparedAssetPaths` prepares
native references through `lunco-storage::canonicalize_file_path` on the stage
worker, checks canonical containment, and returns Twin-relative typed
`AssetPath` results. Filesystem canonicalization owns Windows case-variant
directory names, UNC/verbatim prefixes, and symlink/junction resolution;
projection never performs filesystem lookup or case folding. All consumers
pass the asset owner's typed `AssetPath` to Bevy for loads and handle lookup,
so a literal `#` in a filename cannot become an asset label. Labels remain
explicit fields at the typed boundary. Catalog spawns reject an invalid load
address before creating an entity. Modelica/Python bindings and Modelica
network-member discovery resolve source loads through the stage's prepared
native paths; pending participant sources retain the admitted load address
for parameter recompilation. The shared program-format reader interprets
query/fragment separators only for HTTP URLs;
named and native filenames retain literal `#` when selecting an executor.
The stage's `AssetServer` origin supplies the exact lifetime authority. Manifest names and
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
coarse document rebuilds share `ReferenceLayerInputs` preparation: bounded workers
prepare the address, retain the real Twin-source asset handle, and compose a
`StageRecipe::reanchor` snapshot under the authored canonical root. Reanchoring
uses the shared USD dependency reader, changes relative layer anchors, preserves
explicit absolute identifiers and layer bytes, and produces no alias keys.
`ReferenceLayerInputs` retains the actual source handles through scene lifetime.
Its prepared snapshot carries the native recipe and typed address admission;
ordered stage authoring consumes that closure before generic ECS projection.


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
resolve root  →  register root  →  open document  →  set overlay  →  LoadScene(stable twin://…)
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
| `LoadScene { path }` | `lunco://` or stable `twin://<source-identity>/…` | loads an already-addressable asset; internal mount authorities are rejected |

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
  the stable logical-source `twin://` scene URI. The asset-cache authority stays
  inside the reader and document bridges.
- Filesystem scene paths are canonicalized through `lunco-storage` before root
  discovery and containment checks. If canonicalization fails, `OpenFile`
  reports the error and stops; it never falls back to a lexical path.
- `LoadScene` accepts `lunco://` paths and stable logical-source `twin://`
  paths; it rejects internal mount authorities. Filesystem paths go through
  `OpenFile`, which owns root discovery and document mounting.
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
