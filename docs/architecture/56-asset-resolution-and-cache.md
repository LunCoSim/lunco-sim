# 56 — Asset Resolution and the Cache

> Status: Active · Audience: contributors on asset loading, `lunco://`, and the twin cache

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
| `lunco://` | `assets/`, then `assets/.cache`, then the machine-global cache | the shipped engine library (rovers, parts, shaders, stock textures) |
| `twin://<name>/…` | the Twin's authored root, then `<twin>/.cache`, then the global cache | Twin-owned content, downloaded scenarios, and intentional global reuse |
| (none) | no independent asset identity | derived outputs use their owning `lunco://` or `twin://` identity |

Both schemes resolve **authored first, then the cache that travels with the
unit, then the shared machine-global pool**. So a downloaded binary is reachable at its logical
address without any authored file naming a cache, and a file the author
committed always wins over a materialised copy of it.

The same boundary resolves delivered directories. A processed dataset such as
a DEM site may produce a directory containing several runtime products; its
consumer asks `TwinRoots::resolve_directory` (or the corresponding dataset
artifact URI) and never reconstructs a `.cache` path or treats the directory as
a single file.

### The unit you ship carries its own data

`assets/.cache` and `<twin>/.cache` are the same idea applied to the two things
we distribute:

- a **package** is a folder — binary, `assets/`, and the large binaries it
  needs. Those binaries are cache artifacts (downloaded, derived, git-ignored),
  so they cannot live in `assets/` proper; `build_native.sh` stages them into
  `assets/.cache/`, the resolver's second root. The package therefore resolves
  its own payload with no environment variable and no network — running the
  binary directly behaves exactly like running the launcher.
- a **Twin** is a folder someone tars up and sends. Its downloads and its
  derived products both land in `<twin>/.cache`, so the archive arrives
  complete: `DatasetRegistry` probes read roots rather than the one root it
  would have written to, and reports those datasets *installed* instead of
  offering to re-fetch files already on disk.

During native development, downloads and processed outputs go directly to the
machine-global cache. The packer copies selected manifest artifacts from that
cache into `assets/.cache`, so a package remains self-contained while source
runs and packaged runs use the same logical identities. `lunco_assets::library_roots()`
is the single place that constructs the order for a particular `assets/` root;
the `AssetSource`, synchronous resolver (`engine_asset_local_path`), and byte
reader all use it, so a file the loader finds is a file the validator finds.

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
> `info:sourceAsset` is resolved against the stage's logical asset identity before it
> reaches the scripting loader, so a co-located Twin scenario keeps its Twin source
> instead of silently falling back to the engine library. A failed root or imported
> script is reported by the scenario loader and the program is not attached.

### Rhai imports use the shared asset identity

`RhaiSourceLoader` discovers every literal `import` in the referenced source and
declares it through Bevy's normal nested-asset dependency API. Dependencies may be
top-level or inside a function; they are still loaded before the scenario becomes
executable. The loader canonicalizes each path through
`ScriptSources::canonical_id`, the same `lunco-assets` path algebra used by the
synchronous module resolver:

```rhai
import "/scripting/lib/shots" as shots;       // assets-root absolute
import "lunco://scripting/lib/shots" as shots; // engine-library URI
import "helpers" as helpers;                  // relative to this script
```

There is no global Rhai preload. An unused scenario or library does no I/O, and a
Twin mount does not trigger a project-wide script scan. The synchronous resolver
reads only the sources published by those dependency handles. Non-literal imports
are rejected while loading because an async asset graph cannot make an unknown
runtime path safe or deterministic.

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
| Library root | `assets_dir_abs` (`LUNCO_ASSET_ROOT` when set; otherwise executable/package ancestry, then current-directory ancestry) |
| Library root (of a file) | `shipped_asset_root` |
| Id → disk path | `id_to_disk_path` |
| Scenario staging dir | `scenarios_dir` |

`LUNCO_ASSET_ROOT` is an explicit native launch/test boundary. It names the
directory containing the selected `assets/` library and fails at startup when
the directory does not exist; it never silently falls through to the packaged
binary root. This is how a production binary from another worktree can be run
against the current checkout's authored assets while retaining one resolver.

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
call in the engine that authorises traffic. For an interactive Twin, a missing
declared dataset is presented at Twin open in a consent window with an explicit
selection; the same request remains available in Settings ▸ Downloadable data
and the Twin inspector. This is a rule about trust, not bandwidth: a simulator
that phones home when you open a file has to be *explained* rather than *read*.

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
embedded manifest once, and a Twin's `Assets.toml` is discovered after that
Twin mounts. Workspace `TwinAdded` announces ownership; `TwinRoots` then emits
the typed `TwinAssetMounted` postcondition only after the exact `twin://`
authority is registered. Asset-consuming domains use that postcondition rather
than depending on observer registration order. `TwinClosed` is the
authoritative retirement edge: it acquires the dataset attempt commit barrier,
cancels and removes that root's dataset scopes, and only then allows a
same-name replacement to be scanned. Download installation and processed
output commits use the same barrier, so an outgoing worker may finish CPU work
but cannot publish a stale artifact after close returns. The update scan
discovers new roots only; it is not the teardown mechanism. A registry lock
failure is reported as an error; it is never treated as a successful mount or
unmount.

### Where a download lands

| Declaration | Destination |
|---|---|
| `shared = true` | the global pool `<cache>/sources/<url-hash>/<file>` |
| authored `dest` | `<owner cache>/<dest>` |
| neither | `<owner cache>/sources/<url-hash>/<file>` |

*Owner cache* is `<cache>` for a crate manifest and **`<twin>/.cache` for a
Twin entry by default**. A Twin always reads authored content, its local cache,
then the global cache. `shared = true` selects the global cache as the write
owner for a reusable product; it does not create a second logical URI or a
second reader path.

One resolver — `entry_dest_path` — answers this for the CLI downloader, the
runtime registry and the process step alike, so a file fetched from the app and
one fetched from a terminal cannot land in different places.

A Twin declaration may set `shared = true` for a product deliberately reused by
multiple Twins. That entry writes to the engine-wide cache and the Twin reader
checks that same cache after the Twin-local cache and authored root. The shared
choice therefore changes ownership explicitly; it does not create a second URI
or a second resolver.

Processing output roots are strict: `cache` requires its owning cache, `twin`
requires the Twin root, `assets` uses the canonical packaged/development assets
resolver, and an unknown root is an error. The processor writes into a unique
sibling staging directory and atomically commits the artifact, bake stamp, and
map sidecar under the attempt barrier. A cancelled or failed bake therefore
cannot leave a new final output with an old completion stamp.

Native HTTP reads have connect, response, and per-read body deadlines. The body
deadline is an inactivity bound, not a total transfer-duration limit, so a
large healthy DEM can continue while a silent peer releases its worker.

### One download policy and resumable recovery

`lunco-settings::DownloadSettings` is the single application-wide transport
policy. It is persisted in `<OS config dir>/lunco/settings.json` and is used by
the CLI, interactive asset registry, scenario HTTP, MSL, terrain, browser
Cache Storage fetches, and the desktop updater. `max_attempts` counts the first
request; subsequent waits use exponential backoff with a configured multiplier
and maximum delay. No downloader owns a second retry constant or settings file.

Native file downloads and update ranges retain the received prefix in their
staging file/vector. When the origin honors `Range`, the next attempt requests
only the missing suffix. If an origin ignores the range and returns a complete
`200`, the response is restarted safely; a partial `200` or an invalid
`Content-Range` is rejected. Browser fetches apply the same policy and retain
received chunks across `fetch()` attempts. Final cache publication remains
atomic/content-verified, so partial bytes are never exposed as an asset.

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

## Bodies and their textures

Body imagery is **declared data, not a path in Rust**. `lunco-celestial`'s
`Assets.toml` carries the Earth and Moon maps like any other dataset — listed in
Settings ▸ Downloadable data, fetched only when a user asks — and each entry
names the body it belongs to in a domain sub-table:

```toml
[earth.process]
target_resolution = [4096, 2048]
output = "textures/earth.png"

[earth.body]
naif_id = 399
```

`imagery::bind_dataset_body_imagery` walks the registry for entries carrying a
`[*.body]` table and binds the installed artifact onto the globe with that NAIF
id. Adding Mars imagery is a manifest entry; deleting a dataset removes the
imagery. No crate holds a texture filename, and `registry.rs`'s old
`texture_path` field — which named files the engine did not ship — is gone.

A scene overrides that default by authoring the map on the body prim:

```usda
def Xform "Earth" ( prepend apiSchemas = ["LunCoCelestialBodyAPI"] )
{
    uniform int lunco:body = 399
    asset lunco:body:albedoMap = @twin://myschool/textures/earth_1970.png@
}
```

Three tiers, weakest first, each overwriting the last in the same frame:
manifest default → `lunco:body:albedoMap` → a `UsdShade` Material bound to the
prim (which says strictly more, so it wins). A Twin can therefore dress its own
Earth without touching the engine's manifests.

Three arrival routes, one code path: downloaded now into the global cache,
found there from an earlier run, or supplied by the Twin in its own authored
tree/cache. Earth and Moon imagery is intentionally not bundled: it is a
user-consented resource, and a first-run resource prompt explains what is
missing and offers download plus processing. A body with no imagery renders its
own colour (ocean blue, regolith grey) while the status surface remains honest.

A derived dataset is only ready when its **completed process output** exists,
not its download. The processor's `.bakekey` completion stamp is part of that
contract, including for directory products such as DEM sites; a partial output
directory is still missing. File-based `map` products also require their
normaliser sidecar (`<output>.mean`); RGB maps write the explicit identity value
`1.0`, while measured grayscale maps retain their sampled mean. An in-app fetch
therefore runs the `[*.process]` step before reporting installed. Otherwise the
UI says "installed" while every consumer still finds nothing — the CLI's
two-command flow (`download` then `process`) has no equivalent second command in
the app.

`lunco:body:albedoMap` is not a new mechanism — it is what terrain already does
(`demSource = @terrain/apollo15@`), what materials do (`lunco:material:shader`),
and what HDRI does (`UsdLuxDomeLight`). It buys: the texture becomes a normal
USD reference inheriting cache fallback and web staging with no
celestial-specific path; a Twin can ship its own body map; third-party tools see
a material instead of a Rust constant; and the same logical `lunco://textures/...`
identity reaches genuinely derived outputs.

The stock maps are global-cache datasets, not release payloads. A Twin may read
`lunco://textures/earth.png` and `lunco://textures/moon.png` from that cache
without owning duplicate bytes. `lunar_color` follows the same rule: it is
declared but not downloaded until a consumer needs it.

## Manifest-driven bundle staging

Each `AssetEntry` may declare exact distribution targets in `bundle`. The
`lunco-assets download --bundle TARGET` and `stage --binary TARGET` commands
use that same metadata, so download selection and delivered selection cannot
drift. Native targets stage into `assets/.cache`; web targets stage into the
web bundle root. Entries with no target remain user-provisioned resources and
never enter a release by accident.

Target names distinguish delivery profiles where the same binary has different
packaging owners: `lunica-native`, `luncosim-native`, `lunica-web`, and
`luncosim-web`. The web build still creates its dedicated MSL bundle through
`build_msl_assets`; raw MSL entries therefore target native packaging only.

The downloader qualifies bundle keys as `<group>/<key>` for actionable errors,
but leaves those keys out of opaque on-cache scratch filenames. The process ID
and attempt number provide the uniqueness contract without allowing a manifest
separator or platform-reserved character to become a directory or invalid
filename; the final artifact still lands only at the `entry_dest_path` resolved
above.

This keeps the manifest authoritative without bundling runtime datasets that
must remain user-consented. The packaged `lunco://` reader checks authored
assets, packed cache, and the machine-global cache in that order; Twin reads
additionally check the Twin's authored/cache roots and then the same
machine-global cache.
