# 55 — Scene Addressing and Roots

> Status: Active · Audience: contributors on scene loading, twins, and path resolution
>
> Supersedes the ad-hoc "promote an out-of-assets path"
patching in `normalize_scene_asset_path`.

## The former failure mode

Historically, opening a scene outside the workspace `assets/` directory failed:

```
WARN [scene] `/home/rod/Documents/models/summer_space_school/sim/scenes/traverse.usda`
     is outside assets dir — load it via the Twin (`twin://`) source
```

The runtime now resolves the owning root before loading and reports an invalid
root visibly. The rest of this document is the current contract; the historical
failure is retained only to explain why the boundary exists.

## Diagnosis: three identities for one thing

A scene's location is currently expressed three ways, with conversions
scattered across at least four sites:

| Identity | Rooted at | Who produces it |
|---|---|---|
| bare relative (`scenes/x.usda`) | *implicitly* `assets/` | in-tree content, default `AssetSource` |
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

Each conversion site implements its own partial rules:

- `normalize_scene_asset_path` (`lunco-usd-sim/src/cosim.rs`) — absolute →
  asset-relative, **refuses** anything outside `assets/`.
- `twin_source_for_workspace_scene` (same file) — asset-relative → `twin://`,
  but only for roots already registered.
- `load_startup_scene` (`lunco-luncosim/src/lib.rs`) and the USD `on_open_file`
  observer (`lunco-usd/src/commands.rs`) both resolve the owning root, register
  it, and enter the same doc-first `LoadScene` path.

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

Implemented. `load_startup_scene` and the interactive open path both use this
resolver; neither performs its own ancestor walk.

### 3. Identity is `(assigned_authority, rel)`

`TwinRoots` assigns a runtime authority from the authored Twin name (or folder
name). If another open root requests the same authority, registration allocates
the next deterministic suffix (`name-2`, `name-3`, …) and returns it; callers
must use that returned authority. Reopening the same root is idempotent.
The registry never repoints an existing authority, so a live `twin://` read
cannot change roots underneath it.

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
- `TwinRoots` returns the assigned authority and never repoints a live name.
- Open flows register the root, mount the document overlay, and only then load
  the `twin://` scene.
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
