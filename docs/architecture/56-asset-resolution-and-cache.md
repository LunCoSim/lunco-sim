# Asset Resolution and the Cache

**Status:** proposal. Companion to
[`55-scene-addressing-and-roots.md`](55-scene-addressing-and-roots.md) — same
principle (*identity is not location*), applied to referenced assets rather than
scenes.

## The defect

`lunco-lib://` puts a **storage location** into **authored content**:

```usda
Xform "Visual" ( payload = @lunco-lib://models/perseverance.glb@ )
```

`lunco-lib://` resolves to `cache_dir()` — a machine-local, generated directory.
The `.usda` therefore asserts "this asset lives in my download cache", which is
an environment fact, not an identity fact. The workspace manifest says as much:

> resolves through that source — *only in our pipeline*. Third-party USD tools
> fall back to the prim's local Cube placeholder.

A file that only resolves inside one pipeline is not a portable USD file.

**Renaming the scheme to `cache://` does not fix this.** It makes the mistake
legible and permanent: the cache would still be an address space that authored
files reference. The cache must not be addressable at all.

## The pattern we already have right

The `summer_space_school` Twin solves this correctly today:

| | Downloads to | Scene authors |
|---|---|---|
| Twin `apollo15_dtm` | `output_root = "twin"` → **inside the Twin** | `@terrain/apollo15@` (relative) |
| Workspace `perseverance` | shared `<cache>/` | `@lunco-lib://…@` (cache-addressed) |

The Twin's `.cache/NAC_DTM_APOLLO15.TIF` staging file is an implementation
detail *inside* the Twin; the authored reference is an ordinary relative path
that any USD tool can follow. The Twin is the model to generalise — the
workspace is the outlier.

This also answers "how should it work in a user's own Twin?": it already does.
The fix is to make the workspace behave like a Twin, not to invent a second
mechanism for users.

## Industry practice

Every mature system separates *declared identity* from *materialised bytes*:

| System | Declared identity | Materialisation | Cache key |
|---|---|---|---|
| Git LFS | pointer file in repo | smudge filter fetches | content hash |
| Cargo | `Cargo.toml` + lock | `~/.cargo/registry` | name + version + hash |
| Nix / Bazel | derivation / target | local or remote store | content hash |
| **OpenUSD** | asset path in layer | **`ArResolver`** | resolver context |

The USD-native answer is the **asset resolver** (`Ar` 2.0): layers reference
logical asset paths, and a pluggable resolver maps them to bytes — that is
precisely the seam studios use to attach asset-management systems. We already
have a resolver seam (`crates/lunco-usd-bevy/src/resolver.rs`); the sustainable
move is to extend it, not to add one Bevy `AssetSource` per storage backend.

Note what our `Assets.toml` already carries: `url`, `dest`, `sha256`. That is a
lockfile. We are one step away from the standard design and currently spend that
step on an extra URI scheme instead.

## Target design

### 1. Authored content uses logical references only

Two forms, both location-independent, both resolvable by third-party tools when
the bytes are present:

- `@lunco://models/perseverance.glb@` — the engine asset library
- `@terrain/apollo15@` — Twin-relative, resolved against the Twin root

`lunco-lib://` disappears from authored files entirely.

### 2. The resolver consults `Assets.toml`

`Assets.toml` becomes the resolver's input, not just a download script's input.
Resolution order for a logical id:

1. present under the owning root (`assets/` or the Twin) → serve it
2. else present in the content-addressed cache (keyed by `sha256`) → serve it
3. else declared in an `Assets.toml` → **materialise on demand**, verify hash, serve
4. else → unresolved: report it (see *Failure is visible*)

Step 3 is the "realtime cache resolution" worth having: a missing declared asset
is a fetch, not an error. The cache stays an implementation detail of step 2/3 —
never a name anything can reference.

### 3. One mechanism, workspace and Twin alike

The workspace `assets/` dir is just a root with an `Assets.toml`, exactly like a
Twin. A user's custom Twin gets identical behaviour with no extra concepts:
declare in `Assets.toml`, reference relatively, let the resolver materialise.

Per-root manifests compose — the resolver reads the manifest of whichever root
owns the reference, so a Twin cannot be broken by a workspace rename, and
neither can shadow the other.

### 4. Content addressing, not path addressing

`sha256` is already declared, so the cache should be keyed by it. That buys
what path-keyed caches cannot: two roots requesting the same asset share one
copy, a changed URL with an unchanged hash is a cache hit, and a corrupted or
truncated download is detected rather than served.

### 5. Failure is visible

An unresolved reference must surface on the `StatusBus` naming the reference and
the manifest that should declare it. Today a missing `lunco-lib://` payload
yields a prim with no geometry and no error — indistinguishable from a modelling
mistake. Silence is the expensive part.

## Migration

1. Extend the resolver to read `Assets.toml` for the owning root.
2. Cache keyed by `sha256`; `dest` becomes staging, not identity.
3. Re-point the one real consumer (`perseverance`) at
   `output_root`-in-`assets/` + `@lunco://models/perseverance.glb@`.
4. Delete the `lunco-lib://` `AssetSource` and the `lunco_lib_path()` helper.
5. Add unresolved-reference reporting.

Step 3 is small — `lunco-lib://` has one real authored consumer despite 32
mentions, because most of those are docs and comments about the scheme itself.
That is the clearest signal that it is a concept carrying its own weight rather
than the project's.

## Consequence: bodies and their textures belong in USD

The same "identity is not location" argument reaches one layer further up.
Celestial bodies are currently a **hardcoded Rust registry**:

```rust
// crates/lunco-celestial/src/registry.rs
texture_path: Some("textures/earth.png".to_string()),
// crates/lunco-celestial/src/big_space_setup.rs
asset_server.load_with_settings("cached_textures://earth.png", seam_wrap);
```

Which bodies exist, and which map each one wears, is **scene content** — yet it
lives in Rust, where a Twin cannot change it without a recompile. That conflicts
with "USD is truth, ECS is a projection", and with celestial being opt-in per
scene: the scene decides whether there is an Earth, so the scene should also say
what it looks like.

Authored instead as an ordinary prim with an ordinary asset reference:

```usda
def Xform "Earth" ( prepend apiSchemas = ["LunCoCelestialBodyAPI"] )
{
    asset lunco:body:albedoMap = @lunco://textures/earth.png@
}
```

This is not a new mechanism — it is the pattern terrain already uses
(`demSource = @terrain/apollo15@`), that materials use
(`lunco:material:shader` as an asset path), and that HDRI uses
(`UsdLuxDomeLight`). What it buys:

- the texture becomes a **normal USD reference**, resolved by the `lunco://`
  resolver above — so it inherits cache fallback and web staging for free, with
  no celestial-specific code path
- a Twin can ship its own body map by authoring a different asset path
- third-party USD tools see the material instead of a Rust constant
- `cached_textures://` stops being needed for these, collapsing another address
  space (it remains for genuinely derived pipeline outputs)

### Why the default textures still ship

Because scenarios are dynamic. A scenario or Twin that authors an Earth at
runtime must find `lunco://textures/earth.png` already present — otherwise every
scene that wants a stock body would need its own download. Shipping the stock
maps in the engine library (declared `web = true`, resolved through the cache)
is what makes an authored body work immediately, while leaving a Twin free to
override with its own asset.

`lunar_color` is the same story in advance: it is declared but unshipped today
because nothing samples it (the runtime *derives* albedo from relief in
`derived_layers::albedo_map`). Once a body or terrain layer authors it as an
asset reference, it becomes a real consumer and flips to `web = true`.

## Open: manifest-driven web staging

`scripts/build_web.sh` hardcodes which assets a web bundle carries — the DejaVu
font by name, `for tex in earth.png moon.png`, and a `*.glb` glob gated on the
binary. That list duplicates what the per-crate `Assets.toml` files already
declare, so a newly declared runtime asset silently 404s in the browser until
someone edits the script too.

The fix is a manifest-driven staging step, mirroring `build_asset_manifest`
(which re-uses the runtime's own `scan_library` rather than reimplementing the
walk in shell). It is blocked on one question: **how does a manifest state "the
runtime fetches this from the bundle, and at what path"?**

That is not derivable from the fields we have. "Has a `[process]` step" is the
tempting proxy and is wrong in both directions — the DejaVu font has no process
step and is required at runtime, while the MSL and ThermofluidStream tarballs
also have none and must never ship (~200MB; they reach the web through
`build_msl_assets`). The destination varies too: egui fetches the font outside
any `AssetSource`, so it lives at `/fonts/…` rather than under `.cache/`.

A `web` field (`true` → `.cache/<rel>`, or an explicit bundle path) was
prototyped on 2026-07-18 and **backed out** — it worked, and staging was
verified, but it grows a shared manifest schema and that call belongs to whoever
owns the format. Alternatives if the schema is unwelcome: process runtime
artifacts to `output_root = "assets"` (already supported, already staged
wholesale — costs a gitignore entry for generated files), or derive the set from
what the asset closure actually references.

## What does not change

`lunco://`, `twin://`, and `scenario://` keep their roles. `cached_textures://`
remains for *derived* pipeline outputs — but per the section above, the celestial
maps are not that, and should move to `lunco://` when bodies become USD prims.
