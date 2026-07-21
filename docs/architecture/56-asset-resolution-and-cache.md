# Asset Resolution and the Cache

Companion to [`55-scene-addressing-and-roots.md`](55-scene-addressing-and-roots.md)
— same principle (*identity is not location*), applied to referenced assets
rather than scenes.

## The rule

**Authored content names logical identities; only the resolver knows locations.**

Two authored forms, both location-independent and both resolvable by third-party
USD tools when the bytes are present:

- `@lunco://models/perseverance.glb@` — the engine asset library
- `@terrain/apollo15@` — root-relative, resolved against the owning root

A cache directory is never an address. If authored USDA can name the cache, the
file asserts "this asset lives in my download cache" — an environment fact, not
an identity fact — and stops being portable. This is why there is no `cache://`
scheme: naming the mistake legibly would make it permanent.

## Schemes

| Scheme | Resolves to | For |
|---|---|---|
| `lunco://` | `<cwd>/assets`, then `<cache>` | the shipped engine library (rovers, parts, shaders, stock textures) |
| `twin://<name>/…` | the Twin's root, then `<twin>/.cache` | Twin-owned content, and downloaded scenarios |
| `cached_textures://` | texture cache dir | *derived* pipeline outputs |

Both schemes resolve **authored first, cache second**: `lunco://` tries
`assets/` then `<cache>`; `twin://` tries `<twin>/<rel>` then
`<twin>/.cache/<rel>`. So a downloaded binary is reachable at its logical
address without any authored file naming a cache, and a file the author
committed always wins over a materialised copy of it.

Both `twin://` readers implement that fallback — the `AssetReader` and the
`SchemeRegistry` handler — because they must agree: a file the asset server can
load but scenario-sync cannot see is worse than one neither can.

A **downloaded scenario is just a Twin root** over its cache directory, so it
needs no scheme of its own: one `twin://<name>/<rel>` names the scene on every
peer regardless of where that peer's bytes live. That is what keeps
`Provenance::Content`-derived ids identical across host and client.

## Choosing a form when you author

A scene loaded by **absolute path** is mounted by `load_startup_scene`, which makes the
containing directory a twin root named after it. So a scene may live anywhere on disk —
including outside the engine repo entirely. What matters is not where the scene sits but
**how it names what it references**:

| Target | Author it as |
|---|---|
| Engine asset library (`assets/`) | `@lunco://vessels/rovers/six_wheel_rover.usda@` |
| A file co-located with the scene | `@twin://<scene_dir_name>/<file>@` |
| A scene inside `assets/` referencing `assets/` | a plain path from the assets root, e.g. `@scenarios/foo.rhai@` |
| ❌ A relative escape | ~~`@../../vessels/…@`~~ |

`lunco://` exists for exactly this case — so a scene living **outside** the project can
still reference shared parts (`lunco-assets/src/asset_sources.rs`). This is what removes any
need to symlink external content into the engine tree.

> [!WARNING]
> **A relative `../` path escapes the twin root (or the asset root) and fails to load.**
> For `info:sourceAsset` on a `LunCoProgram` prim this failure is **silent**: the
> prim is simply never driven, with nothing in the log. If a scenario doesn't run, check
> the path before you debug the script.

### rhai `import` does NOT use `lunco://` — the asymmetry

Script module ids are registered by `asset_path::anchor_of`, which returns a **bare relative
path** for Bevy's default asset source. So the engine script library registers as
`scripting/lib/shots.rhai`, with **no scheme**, and:

```rhai
import "/scripting/lib/shots" as shots;      // ✅ absolute from the assets root
import "lunco://scripting/lib/shots";        // ❌ MEASURED: "Module not found"
```

`lunco://…` has a scheme, so `canonicalize` passes it through untouched and the lookup
misses. The leading slash is the "absolute from the assets root" form, resolved *without*
the importing script's anchor — a bare `"scripting/lib/shots"` would instead anchor to the
importer's own root. Since `lunco://` **is** the right spelling for a USD reference, this
asymmetry is an easy mistake to make twice.

## `lunco-assets` owns resolution

Every URI↔location mapping lives in `crates/lunco-assets`, and no other crate
re-derives one:

| Concern | Entry point |
|---|---|
| Register the sources | `asset_sources::register_lunco_asset_sources` |
| Build a Twin URI | `twin_uri(name, rel)` |
| Parse a Twin URI | `parse_twin_uri` |
| "already addressable?" | `has_scheme` |
| Library URI ⇄ relative | `engine_asset_uri` / `engine_asset_rel` |
| Any URI → local path | `local_path(reference, twins)` |
| Library root (CWD) | `assets_dir_abs` |
| Library root (of a file) | `shipped_asset_root` |
| Id → disk path | `id_to_disk_path` |
| Scenario staging dir | `scenarios_dir` |

The reason this is a hard rule rather than a preference: a copy of the mapping
drifts from the readers actually registered, and then the *same URI resolves two
ways* depending on which crate asked. A hand-rolled `PathBuf::from("assets")`
join resolved against the caller's CWD while the loader used the absolute
library path — same reference, different file, no error.

**No crate outside `lunco-assets` performs filesystem path resolution.** Not a
style rule: a path derived anywhere else is native-only by construction, so it
breaks on web (where bytes live in OPFS) and on any Twin-owned asset (which has
no path under `assets/` at all). If code needs bytes, it goes through the
`AssetServer` or `lunco-storage`; if it needs to know *where* a reference points,
it asks `lunco-assets`. Joining `"assets"`, stripping a scheme prefix, or
splitting a `twin://` authority by hand are all the same defect.

What legitimately stays outside: `lunco-usd-bevy`'s `canonicalize` and
`LuncoUsdResolver`. Those anchor a *relative* reference to its **referencing
layer** and plug into `openusd`'s `ar::Resolver` — USD composition semantics that
must sit next to the `Stage`, not asset-source knowledge.

## Industry practice

Every mature system separates *declared identity* from *materialised bytes*:

| System | Declared identity | Materialisation | Cache key |
|---|---|---|---|
| Git LFS | pointer file in repo | smudge filter fetches | content hash |
| Cargo | `Cargo.toml` + lock | `~/.cargo/registry` | name + version + hash |
| Nix / Bazel | derivation / target | local or remote store | content hash |
| **OpenUSD** | asset path in layer | **`ArResolver`** | resolver context |

The USD-native answer is the asset resolver (`Ar` 2.0): layers reference logical
asset paths and a pluggable resolver maps them to bytes — the seam studios use
to attach asset-management systems. We have that seam
(`crates/lunco-usd-bevy/src/resolver.rs`); it is extended rather than
supplemented with one Bevy `AssetSource` per storage backend.

`Assets.toml` already carries `url`, `dest`, `sha256`. That is a lockfile.

## Declared datasets: the runtime half of `Assets.toml`

`crates/lunco-assets/src/datasets.rs` is where a *running* app meets the
manifest. `download.rs` knows how to fetch one entry; `DatasetRegistry` knows
what is declared, what is on disk, and what a user has asked for.

**The app never reaches the network on its own.** Launch, scene load and twin
open must not open a connection. `DatasetRegistry::request(key)` is the only
call in the engine that authorises traffic, and it is wired to a click
(Settings ▸ Downloadable data). This is a rule about trust, not bandwidth: a
simulator that phones home when you open a file has to be *explained* rather
than *read*.

That rule is also why fetching lives in this crate and nowhere else. A domain
crate owning its own downloader inevitably grows a "just fetch it at startup"
line — the ephemeris crate had exactly that, `ureq` and all, and the guarantee
dies one crate at a time.

| Concern | Owner |
|---|---|
| manifest, URL, cache path, task, bytes, status | `lunco-assets` |
| declaring datasets + reporting what it loaded | the domain crate |
| listing and requesting | the UI (knows no dataset by name) |

Registration follows what is OPEN, not what exists: a crate registers its
embedded manifest once, and a Twin's `Assets.toml` is scanned when that Twin
opens and forgotten when it closes.

### Where a download lands

| Declaration | Destination |
|---|---|
| `shared = true` | the global pool `<cache>/sources/<url-hash>/<file>` |
| authored `dest` | `<owner cache>/<dest>` |
| neither | `<owner cache>/sources/<url-hash>/<file>` |

*Owner cache* is `<cache>` for a crate manifest and **`<twin>/.cache` for a
Twin's**. Twin-local is the default, so a Twin is self-contained: copy the
folder and its data travels, delete it and nothing is orphaned in a global cache
nobody audits. `shared = true` is the opt-out for a multi-GB upstream product
several Twins legitimately reuse (the LROC DTM mosaics), trading
self-containment for one copy on disk.

One resolver — `entry_dest_path` — answers this for the CLI downloader, the
runtime registry and the process step alike, so a file fetched from the app and
one fetched from a terminal cannot land in different places.

### Domain metadata rides with the declaration

A dataset's transport (`url`, `dest`, `sha256`) and its *meaning* belong in one
place, because the meaning describes those exact bytes. `AssetEntry` keeps every
unrecognised key verbatim and hands it back through `AssetEntry::domain::<T>()`,
so the owning crate reads a sub-table this crate never interprets:

```toml
[artemis2_vectors]
url  = "https://ssd.jpl.nasa.gov/api/horizons.api?…&CENTER='500%40399'&…"
dest = "ephemeris/target_-1024_….csv"

[artemis2_vectors.ephemeris]      # read by lunco-celestial-ephemeris
naif_id = -1024
center  = "500@399"               # the CENTER= of the query above
```

This replaced `assets/missions/*.ephemeris.json`, which restated the id and
centre next to a second copy of the Horizons query. Two files describing one
product is one too many: they drift, and the drift is silent — a mismatched
`center` places a spacecraft around the wrong body while looking like data.

The split that remains is deliberate: **USD says WHICH body**
(`lunco:body` / `lunco:spacecraft:ephemerisId`, a NAIF id — the join key the
schema already documents), the **dataset says what its own numbers mean**. A
scene does not author `center`, because two scenes could then disagree about the
same file and one would be wrong. And the prim names no path: unlike a `.mo`
behind `info:sourceAsset`, an ephemeris body has an identity of its own, so
binding by id is both stronger and immune to the download's date range changing.

### Still open

1. present under the owning root (`assets/` or the Twin) → serve it — **done**
2. else present in that owner's cache → serve it — **done**
3. else declared in an `Assets.toml` → offer it; materialise **on request** — **done**
4. else → unresolved: report it on the `StatusBus` — **open**

Step 4 still matters: a missing payload yields a prim with no geometry and no
error, indistinguishable from a modelling mistake. Silence is the expensive
part. Note step 3 is deliberately *not* automatic materialisation — see the rule
above; the resolver offers, the user decides.

Content addressing by `sha256` (rather than URL hash) remains open, and buys
what path-keyed caches cannot: a changed URL with an unchanged hash is a cache
hit, and a truncated download is detected rather than served.

Per-root manifests compose — whichever root owns the reference owns its
manifest, so a Twin cannot be broken by a workspace rename and neither can
shadow the other. The workspace `assets/` dir is just a root with an
`Assets.toml`, exactly like a Twin.

## Open: bodies and their textures belong in USD

The same argument reaches one layer up. Celestial bodies are a **hardcoded Rust
registry** (`lunco-celestial/src/registry.rs` `texture_path`,
`big_space_setup.rs` loading `cached_textures://earth.png`). Which bodies exist,
and which map each wears, is **scene content** — yet it lives in Rust, where a
Twin cannot change it without a recompile. That conflicts with "USD is truth,
ECS is a projection", and with celestial being opt-in per scene.

Authored instead as an ordinary prim with an ordinary asset reference:

```usda
def Xform "Earth" ( prepend apiSchemas = ["LunCoCelestialBodyAPI"] )
{
    asset lunco:body:albedoMap = @lunco://textures/earth.png@
}
```

Not a new mechanism — it is what terrain already does
(`demSource = @terrain/apollo15@`), what materials do (`lunco:material:shader`),
and what HDRI does (`UsdLuxDomeLight`). It buys: the texture becomes a normal
USD reference inheriting cache fallback and web staging with no
celestial-specific path; a Twin can ship its own body map; third-party tools see
a material instead of a Rust constant; and `cached_textures://` collapses back to
genuinely derived outputs only.

The stock maps still ship, because scenarios are dynamic: a Twin authoring an
Earth at runtime must find `lunco://textures/earth.png` already present, or every
scene wanting a stock body needs its own download. `lunar_color` is the same
story in advance — declared but unshipped because nothing samples it (the runtime
*derives* albedo from relief in `derived_layers::albedo_map`); once something
authors it as an asset reference it flips to `web = true`.

## Open: manifest-driven web staging

`scripts/build_web.sh` hardcodes which assets a web bundle carries — the DejaVu
font by name, `for tex in earth.png moon.png`, and a `*.glb` glob gated on the
binary. That duplicates what per-crate `Assets.toml` files already declare, so a
newly declared runtime asset silently 404s in the browser until someone edits the
script too.

The fix mirrors `build_asset_manifest` (which re-uses the runtime's own
`scan_library` rather than reimplementing the walk in shell). It is blocked on
one question: **how does a manifest state "the runtime fetches this from the
bundle, and at what path"?**

Not derivable from today's fields. "Has a `[process]` step" is the tempting proxy
and is wrong in both directions — the DejaVu font has no process step and is
required at runtime, while the MSL and ThermofluidStream tarballs also have none
and must never ship (~200MB; they reach the web through `build_msl_assets`). The
destination varies too: egui fetches the font outside any `AssetSource`, so it
lives at `/fonts/…` rather than under `.cache/`.

A `web` field (`true` → `.cache/<rel>`, or an explicit bundle path) was
prototyped and backed out — it worked, but it grows a shared manifest schema and
that call belongs to whoever owns the format. Alternatives if the schema is
unwelcome: process runtime artifacts to `output_root = "assets"` (already
supported and staged wholesale — costs a gitignore entry), or derive the set from
what the asset closure actually references.
