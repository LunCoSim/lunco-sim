# Scene Addressing and Roots

**Status:** proposal. Supersedes the ad-hoc "promote an out-of-assets path"
patching in `normalize_scene_asset_path`.

## The symptom

Opening a scene that lives outside the workspace `assets/` directory fails:

```
WARN [scene] `/home/rod/Documents/models/summer_space_school/sim/scenes/traverse.usda`
     is outside assets dir — load it via the Twin (`twin://`) source
```

This is not a missing feature. It is the visible edge of three overlapping
addressing systems that never got unified.

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
- `load_startup_scene` (`lunco-sandbox/src/lib.rs`) — walks up for `twin.toml`,
  opens the Twin, registers it, mounts doc-first. **This is the only place that
  actually does the right thing**, and it is reachable only from `--scene` at boot.
- `on_open_file` (`lunco-usd/src/commands.rs`) — routes `OpenFile` to
  `spawn_scene_root_world`, which funnels back into the refusing normalizer.

So the capability exists, once, in the least reusable place.

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
in isolation — it always resolves *through* a root. Three kinds, already modelled
by `lunco_twin::TwinMode`:

| Kind | Detected by | Notes |
|---|---|---|
| `Builtin` | the workspace `assets/` dir | pre-registered as `lunco://` |
| `Twin` | ancestor contains `twin.toml` | manifest, libraries, ref repair |
| `Folder` | no manifest found | the VS Code "Open Folder" analog |

`Folder` is a **first-class** kind, not a degraded one. This answers "what if
there's just one scene and no twin?" — its parent directory is the root. No
`twin.toml` is required, and its siblings resolve correctly.

### 2. One resolver, no World access

```rust
// lunco-twin — pure, testable, no ECS
pub fn root_for_file(file: &Path) -> PathBuf   // nearest twin.toml ancestor, else parent
```

Already implemented. `load_startup_scene`'s inline walk-up is now a duplicate
and must be deleted in favour of it.

### 3. Identity is `(root_id, rel)`, keyed by path — not by name

`TwinRoots` currently keys by **name**, taken from the directory basename or
`twin.toml`. Two unrelated folders named `scenes` collide, and
`register()` silently repoints the earlier one — breaking the first Twin's
asset reads with no diagnostic.

Fix the key, not the symptom: identity is the **canonical path**; the name is
presentation only. A hash suffix to dodge collisions (as the earlier patch did)
is a workaround for the wrong key.

### 4. One mount path, always doc-first

```
resolve root  →  register root  →  open document  →  set overlay  →  LoadScene(twin://…)
```

The overlay must be registered **before** `LoadScene`, or the load reads
base-only bytes and silently drops placed waypoints, runtime spawns, and moved
transforms. `load_startup_scene`'s "fall through to a direct `LoadScene`"
branch is exactly that bug in code form and should be **deleted**, not kept as a
fallback — a fallback that silently discards user edits is worse than an error.

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
command and no new UI surface**.

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

## What gets deleted

Not preserved as fallbacks:

- the "outside assets dir" concept and its `Err(_)` arm in `normalize_scene_asset_path`
- **implicit bare-relative → `assets/` resolution** — in-tree assets are
  addressed `lunco://`; a bare path is ambiguous once any root is open
- `load_startup_scene`'s inline `twin.toml` walk-up (duplicate of `root_for_file`)
- `load_startup_scene`'s direct-`LoadScene` fallback (the overlay-wipe path)
- name-collision hashing (obviated by path-keyed identity)
- the `promote_external_scene` patch on this branch (a fourth conversion site)

Dropping bare-relative resolution is the widest-blast-radius item: every
in-tree `.usda` and every caller passing `"scenes/…"` must move to `lunco://`.
It is worth doing precisely because the failure it prevents is silent.

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

## Implementation order

1. `root_for_file` in `lunco-twin` — **done** on this branch.
2. Re-key the roots registry by canonical path; name becomes display-only.
3. Extract `mount_scene` (resolve → register → doc → overlay → load) from
   `load_startup_scene`.
4. Point `OpenFile` / `OpenFolder` / `OpenTwin` / `LoadScene` at it.
5. Register `assets/` as the `lunco://` root and drop its default-source
   privilege; migrate in-tree refs and callers from bare paths to `lunco://`.
6. Delete the list under "What gets deleted", including this branch's patch.

Step 5 is the migration; do it before 6 so bare paths fail loudly during the
transition rather than silently re-rooting.

Steps 2–6 are behaviour-changing and want a regression test that a runtime edit
survives a reload, since the deleted fallback is precisely what used to mask
that failure. A second test should assert that a bare relative scene path is
**rejected**, not quietly resolved — that is the invariant the whole design
rests on.
