# 56 — Asset Resolution and the Cache

> Status: Active · Audience: contributors on asset loading, `lunco://`, and the twin cache

Companion to [`55-scene-addressing-and-roots.md`](55-scene-addressing-and-roots.md)
— same principle (*identity is not location*), applied to referenced assets
rather than scenes.

The implementation is split by change and dependency cost. `lunco-assets-path`
owns URI and relative-path algebra without Bevy or filesystem dependencies. `lunco-assets-core`
owns cache/Twin roots and storage-facing identity/resolution contracts;
`lunco-assets-runtime` owns registered source readers, runtime sources, and
discovery.
`lunco-assets-datasets` owns the lightweight manifest and lifecycle contract.
Native provisioning is layered on top: `lunco-assets-transport` owns
HTTP byte policy, `lunco-assets-download` owns manifest-aware verification and
atomic installation, `lunco-assets-processing` owns native decode/raster/glTF
baking, and `lunco-assets` composes those workers for the explicit application
boundary and CLI. Runtime readers depend on the core/contract packages;
ordinary readers do not inherit the native baking graph.

The GLB processor uses the pure-Rust `draco-gltf` reader/writer to materialize
`KHR_draco_mesh_compression` into ordinary accessors. It preserves authored
`EXT_texture_webp` semantics and rejects such input until a Rust-owned WebP
conversion can update the image bytes, MIME types, and references together;
discarding only the extension is not a valid normalization.

## The rule

**Authored content names logical identities; only the resolver knows locations.**

This is one asset system, including source libraries. Language consumers may
interpret language syntax and supply an importing source or package identity;
they must not implement their own filesystem search, cache precedence,
containment, transport decoding, or storage reader. USD contributes the
strongest opinion's layer anchor and search-path spelling. Rhai contributes
the importing script identity and its module suffix. Modelica name lookup and
package mapping remain compiler facts; an external resource becomes an asset
address before bytes are requested. UI widgets dispatch those same typed
requests and show their pending, ready, or failed result.

Internal asset-source addresses preserve literal filename characters. Standard
`file:`, HTTP, and Modelica URIs have their specified encoding and fragment
semantics, interpreted once at their boundary. A language adapter must not
decode an internal address as a URL or join an encoded URI onto a filesystem
root. Missing, ambiguous, malformed, out-of-root, and retired-source results
must remain distinct from an ordinary cache miss. Preparation captures source
revision and owning mount; publication validates both before admitting bytes.

Authored forms retain logical identities. Custom schemes require the registered
resolver when another USD tool opens the document:

- `@lunco://models/perseverance.glb@` — the engine asset library
- `@terrain/apollo15@` — search beside the contributing layer, then in its supplied Twin context
- `@./terrain/apollo15@` — strictly relative to the contributing layer

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
runs and packaged runs use the same logical identities. `lunco_assets_core::library_roots()`
is the single place that constructs the order for a particular `assets/` root;
the `AssetSource`, synchronous resolver (`engine_asset_local_path`), and byte
reader all use it, so a file the loader finds is a file the validator finds.

Both `twin://` readers implement that fallback — the `AssetReader` and the
`SchemeRegistry` handler — because they must agree: a file the asset server can
load but scenario-sync cannot see is worse than one neither can.

A **downloaded scenario is a Twin root** over its cache directory. Its load
authority is local to the mount, while content provenance uses the shared
logical source identity. The canonical ownership and conversion contract is
[`Mount addresses and stable source identity`](55-scene-addressing-and-roots.md#3-mount-addresses-and-stable-source-identity).

## Choosing a form when you author

A scene loaded by **absolute path** is mounted by `load_startup_scene`, which makes the
containing directory a twin root named after it. So a scene may live anywhere on disk —
including outside the engine repo entirely. What matters is not where the scene sits but
**how it names what it references**:

| Target | Author it as |
|---|---|
| Engine asset library (`assets/`) | `@lunco://vessels/rovers/six_wheel_rover.usda@` |
| A file co-located with the scene | `@<relative-path>@` |
| A scene inside `assets/` referencing `assets/` | a plain path from the assets root, e.g. `@scenarios/foo.rhai@` |
| ❌ A relative escape | ~~`@../../vessels/…@`~~ |

`lunco://` exists for exactly this case — so a scene living **outside** the project can
still reference shared parts (`lunco-assets-runtime/src/asset_sources.rs`). This is what removes any
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
`ScriptSources::canonical_id`, the same `lunco-assets-path` algebra used by the
synchronous module resolver:

```rhai
import "/scripting/lib/shots" as shots;       // assets-root absolute
import "lunco://scripting/lib/shots" as shots; // engine-library URI
import "helpers" as helpers;                  // relative to this script
```

The `RhaiSource` owner converts a resolved default Bevy `AssetPath` to its
explicit `lunco://` identity before publishing source text or its prepared AST.
Named sources keep their scheme, including `twin://`. Scenario roots, imports,
the source registry, prepared AST cache, and unload events all use this same
identity, so the text and AST for one file cannot be published under different
keys. `AssetEvent::Removed` retires the canonical source and its prepared AST;
an old asset removal cannot evict a newer registration under the same URI.
Startup tools, Twin tools, and file-backed scenarios commit this complete
loaded graph at their owner boundary before binding or lifecycle activation;
they do not depend on `AssetEvent::Added` being consumed in the same update as
the asset server reports readiness. Later asset events publish revisions and
hot reload through the same cache.

There is no global Rhai preload. An unused scenario or library does no I/O, and a
Twin mount does not trigger a project-wide script scan. Scenario preparation
captures the loaded transitive source closure and compiles its ASTs off-thread;
the live resolver evaluates only an owner-committed AST. Non-literal imports are
rejected while loading because an async asset graph cannot make an unknown
runtime path safe or deterministic.

A statically registered tool call such as `waypoint_editor::route_positions(...)`
does not declare an asset dependency. A scenario that needs a source-defined
tool during its first lifecycle hook must use a literal asset import such as
`import "lunco://scripting/tools/waypoint_editor" as waypoint_editor;`. This
keeps the tool source in the scenario's normal asynchronous dependency closure;
the bare tool-registry import form is for composing already registered tools.

## `lunco-assets-path` owns URI/path algebra; assets core/runtime own resolution

`lunco-assets-path` contains the platform-neutral rules that must be shared by
headless document crates, USD composition, and runtime asset sources. It has no
Bevy or filesystem dependency, so importing the rules does not pull the runtime
asset graph into a document-only package.

`lunco-assets-core` owns roots and logical identity mapping; the runtime package
registers Bevy sources and readers against those contracts. No other crate
re-derives that mapping:

| Concern | Entry point |
|---|---|
| Scheme parsing and URI construction | `lunco-assets-path::{split_scheme, uri}` |
| Canonicalize against a document or root | `lunco-assets-path::{canonicalize, canonicalize_root}` |
| Validate a relative read address | `lunco-assets-path::{is_safe_relative_path, relative_path}` |
| Validate transferable filenames and a materialized tree | `lunco-assets-path::{is_portable_relative_path, PortablePathSet}` |
| Native destination overlap identity | `lunco-storage::FilePathIdentity` (prepare on the scan worker, compare without I/O) |
| Register the sources | `lunco-assets-runtime::asset_sources::register_lunco_asset_sources` |
| Build a Twin URI | `twin_uri(name, rel)` |
| Parse a Twin URI | `parse_twin_uri` |
| "already addressable?" | `lunco-assets-path::has_scheme` / `is_anchored` |
| Library URI ⇄ relative | `engine_asset_uri` / `engine_asset_rel` |
| Document-root-relative asset URI | `asset_path::source_relative_uri` |
| Any URI → local path | `local_path(reference, twins)` |
| Library root | `lunco-assets-core::assets_dir_abs` (`LUNCO_ASSET_ROOT` when set; otherwise executable/package ancestry, then current-directory ancestry) |
| Library root (of a file) | `shipped_asset_root` |
| Id → disk path | `id_to_disk_path` |
| Scenario staging dir | `scenarios_dir` |

The same document-relative operation is available to authored Rhai as
`asset_source_relative_uri(document, relative)`. It performs URI algebra only
and preserves the document's registered source authority; the production
`scripting_asset_contracts` scene is the acceptance test for that public
surface, while the runtime shader loader calls the Rust owner directly.

`LUNCO_ASSET_ROOT` is an explicit native launch/test boundary. It names the
directory containing the selected `assets/` library. `assets_dir_abs`, derived
library paths, source registration, and application builders return a fallible
result. Invalid overrides, missing discovery, and unreadable roots produce a
terminal startup error; it never silently falls through to the packaged
binary root. This is how a production binary from another worktree can be run
against the current checkout's authored assets while retaining one resolver.

The reason this is a hard rule rather than a preference: a copy of the mapping
drifts from the readers actually registered, and then the *same URI resolves two
ways* depending on which crate asked. A hand-rolled `PathBuf::from("assets")`
join resolved against the caller's CWD while the loader used the absolute
library path — same reference, different file, no error.

**No crate outside `lunco-assets-core` performs filesystem path resolution.** Not a
style rule: a path derived anywhere else is native-only by construction, so it
breaks on web (where bytes live in OPFS) and on any Twin-owned asset (which has
no path under `assets/` at all). If code needs bytes, it goes through the
`AssetServer` or `lunco-storage`; if it needs to know *where* a reference points,
it asks `lunco-assets-core`. Joining `"assets"`, stripping a scheme prefix, or
splitting a `twin://` authority by hand are all the same defect.

What legitimately stays outside: `lunco-usd-compose`'s `canonicalize_at` and
`LuncoUsdResolver`. Those anchor a *relative* reference to its **referencing
layer** and plug into `openusd`'s `ar::Resolver` — USD composition semantics that
must sit next to the `Stage`, not asset-source knowledge.

### USD references stay logical through loading

USD asset literals are logical identifiers. They use forward slashes and either
the authored layer's relative namespace or an explicit `lunco://`/`twin://`
source; they never contain a native cache path. Every prefetch and resolver
lookup goes through the same `AssetPath`/`lunco-assets-path` algebra, so a
Windows path separator cannot turn a URI into a default-source filename.

Opening the root layer is terminal when it cannot be read. A missing
transitive sublayer, reference, or payload is different: the loader retains
the available closure, OpenUSD leaves only that arc unresolved, and the runtime
publishes a scene-scoped diagnostic containing the referring layer and logical
dependency. Malformed data, unsafe traversal, permission failures, and closure
resource limits remain terminal at the owning boundary. No missing asset is
silently aliased to another file.
The warning is published from the stage-load event itself, so it remains visible
even when the available root does not yet project a prim; it does not depend on
the missing arc producing a placeholder entity.

Consequently, a stale authored name such as `lunco://vessels/markers/waypoint.usda`
must be corrected in the Twin's USD/library owner (or restored as an actually
shipped asset). It must not be mapped at runtime to a newer name such as
`route_point.usda`: that would hide a broken composition contract and make
different Twins resolve the same authored identifier differently.

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
(`crates/lunco-usd-compose/src/resolver.rs`); it is extended rather than
supplemented with one Bevy `AssetSource` per storage backend.

`Assets.toml` already carries `url`, `dest`, `sha256`. That is a lockfile.

## Declared datasets: the runtime half of `Assets.toml`

`crates/lunco-assets-datasets` is where a *running* app describes and observes
the manifest. Its `DatasetRegistry` knows what is declared, what is on disk,
and what a user has asked for. `lunco-assets-datasets::DatasetRegistryPlugin`
is the lightweight discovery/read-side plugin. The optional
`lunco-assets::datasets::DatasetProvisioningPlugin` owns worker lifecycle and
composes `lunco-assets-download` and `lunco-assets-processing`; the GUI adds it
explicitly, while headless hosts can install only the registry contract.
Engine manifest enumeration, file reads, TOML parsing, and installed-artifact
checks run on Bevy's async-compute pool during startup. The main thread merges
the prepared typed registry and emits `DatasetScopeReady` only after that scan
finishes. Open Twin manifests use the same worker boundary. Independent scans
can overlap; their typed registry results commit in discovery order, and
`TwinClosed` discards any pending result owned by that root.

**The app never reaches the network on its own.** Launch, scene load and twin
open must not open a connection. `DatasetRegistry::request(key)` is the only
call in the engine that authorises traffic. For an interactive Twin, a missing
declared dataset is presented at Twin open in a consent window with an explicit
selection; the same request remains available in Settings ▸ Downloadable data,
the Twin inspector, and the Files section's Twin assets rows. Those rows are a
read-only projection of the registry: Download, Retry, and Cancel emit the
typed dataset commands, while progress, processing, integrity failure, and
installed state continue to come from `DatasetRegistry`. This is a rule about
trust, not bandwidth: a simulator that phones home when you open a file has to
be *explained* rather than *read*.

Fetching lives in the explicit provisioning package family. Domain crates
consume artifacts delivered by the shared asset runtime and do not choose
network behavior.

| Concern | Owner |
|---|---|
| URI/path algebra and traversal rules | `lunco-assets-path` |
| identity and roots | `lunco-assets-core` |
| registered sources and source readers | `lunco-assets-runtime` |
| manifest, URL, cache path, and lifecycle state | `lunco-assets-datasets` |
| retry policy and resumable HTTP bytes | `lunco-assets-transport` |
| manifest verification, extraction, and atomic source installation | `lunco-assets-download` |
| per-transaction network/cache/replacement/backup decision | `assets.download.prepare` Rhai policy through `lunco-assets-download` |
| decode, raster math, external glTF processing, and baked sidecars | `lunco-assets-processing` |
| Bevy worker lifecycle and CLI composition | `lunco-assets` |
| selecting scene dataset artifacts | `application.asset.lifecycle` Rhai policy |
| registry-validated asynchronous text artifact reads through canonical asset URIs | `lunco-assets-runtime::DatasetArtifactPlugin` |
| parsing delivered text into domain data | the consuming domain crate |
| listing and requesting | the UI (knows no dataset by name) |
| engine Modelica source URI | `lunco-assets-core::engine_model_asset_uri` |
| engine scene-test root | `lunco-assets-core::engine_scene_tests_root` |
| asset manifest path/URL | `lunco-assets-core::asset_manifest_path` / `asset_manifest_url` |

Registration follows what is OPEN, not what exists: the runtime asset manifest
describes the delivered library, and a Twin's `Assets.toml` is discovered after that
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

Transferable dataset destinations and scenario manifests use portable relative
components: Windows devices, control characters, reserved punctuation, trailing
dots/spaces, absolute paths, and traversal are rejected on every host. Literal
`#`, `%`, spaces and Unicode remain valid filenames. `PortablePathSet` also
rejects duplicate files, file/directory conflicts, and case-alias components
using a conservative Unicode uppercase key. Both server publication and client
admission validate the complete manifest before replacing a mounted scenario.
This portability key is not used for native filesystem identity.

Dataset scan workers prepare native source/output identities through
`FilePathIdentity`: canonical existing ancestors resolve links and Windows
verbatim prefixes; missing descendants inherit the ancestor's native directory
case mode. Windows comparisons use ordinal filesystem-name comparison. An
unknown case mode rejects ambiguous ownership visibly. Admission and worker
merge compare the prepared identities without I/O, rejecting outputs that
could replace another output or source. These snapshots do not lock out
external filesystem changes. Read roots travel with the prepared declaration.

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
the CLI, interactive asset registry, scenario HTTP, Modelica source libraries, terrain, browser
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

The generic downloader invokes `assets.download.prepare` with typed facts before
using the cache or opening the network. The authored policy returns the closed
decision map (`allow_network`, cache mode, install mode, backup disposition,
and bounded retained-backup count). Rhai owns mode behavior and consent; Rust
only validates that decision and performs safe staging, integrity verification,
atomic commit, rollback, and bounded cleanup under the install-backup prefix.
An absent optional policy uses the downloader's documented generic default; an
installed but malformed policy is a terminal transaction error.

### Dataset meaning and scene selection

Dataset transport facts and domain meaning stay together in the manifest.
`AssetEntry` preserves domain tables and exposes them to the owning Rust
consumer through `AssetEntry::domain::<T>()`; for example,
`lunco-celestial-spatial` reads body imagery metadata. The dataset layer does
not select scene assets. When a scene needs a declared text artifact, the
authored `application.asset.lifecycle` Rhai policy returns its dataset id in
`dataset_text_artifacts`; the generic asset runtime resolves and reads it.
Missing requested assets are reported by that owner. A scene with no authored
dataset request does not probe for unrelated data during startup.

### Still open

1. present under the owning root (`assets/` or the Twin) → serve it — **done**
2. else present in that owner's cache → serve it — **done**
3. else declared in an `Assets.toml` → offer it; materialise **on request** — **done**
4. else → unresolved: report it on the `StatusBus` — **open for unrequested USD references**

Step 4 still matters for asset references that have no dataset declaration.
Declared datasets selected by scene policy are reported by the generic artifact
reader when unavailable. Step 3 is deliberately *not* automatic materialisation
— see the rule above; the resolver offers, the user decides.

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
`1.0`, while measured grayscale maps retain their sampled mean in normalized
linear contrast space (the PNG itself is sRGB-encoded for the runtime loader).
An in-app fetch
therefore runs the `[*.process]` step before reporting installed. Otherwise the
UI says "installed" while every consumer still finds nothing — the CLI's
two-command flow (`download` then `process`) has no equivalent second command in
the app.

`lunco:body:albedoMap` is not a new mechanism — it is the same asset-reference
pattern used by terrain data (`demSource = @terrain/apollo15@`), standard
`UsdShade` material inputs, and HDRI assets (`UsdLuxDomeLight`). It buys: the texture becomes a normal
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
`luncosim-web`. The web build still creates its dedicated source-library bundle through
`lunco-modelica-assets`'s `build_modelica_library_assets`; raw source-library entries therefore target
native packaging only.

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

## Source libraries and resource views

`lunco-assets-runtime::library` is the common installed-source store for native
filesystem and in-memory backends. `LibrarySource::filesystem` prepares a complete,
language-neutral resident inventory through `discovery::inventory_files`;
source-language consumers filter that inventory themselves. Source byte reads
revalidate containment after inventory and consume the resolved path. Source membership
never probes storage during painting. Installations after UI startup prepare
the store on a worker before publication. Relative paths use that owner's
shared validation; native reads use `existing_path_within_root`, including symlink
containment. Absolute source-file reads are accepted only by their owning
installed filesystem root. An unowned absolute file remains the document
admission owner's responsibility. Only `Ok(None)` advances an ordered search;
invalid paths and I/O failures terminate it.

The registered `library://<authority>/<relative>` source pins one installed
source. Its authority is process-local, like a Twin mount authority; authored
content uses its language's portable identity. The native scheme-registry mirror
and byte source share the same selection and containment functions. The source
reader streams native files and borrows resident memory bytes, so a bounded
consumer need not materialize the whole payload before checking its budget.

Default, engine-library and installed-library native sources share
`lunco-assets-core::NativeAssetReader`, a containment adapter which reads and
seeks owned storage handles on the asset I/O workers. Each read consumes only
the caller's buffer, without an independent read-ahead allocation. Payloads,
metadata sidecars and directory queries all validate their owning root. A containment or I/O error terminates
the search; only absence permits the next root. Native source roots must be
absolute. `FileStorage::open_regular_file` supplies the shared admission for
payload, sidecar, direct storage and library streaming reads; nonregular entries
are rejected before opening, including FIFOs with no writer. Path and handle
metadata checks are not an atomic guarantee against hostile filesystem replacement.

Twin readers also open streams before the common reader applies its byte limit.
Native handles use the same storage admission; OPFS holds an immutable File
snapshot and sends demand-driven slices of at most 8 KiB through Rust channels.
The browser-local task retains JavaScript values and exits when the reader is
dropped. Seeking after a cancelled read retains response offsets. Resident Twin
overlays share their immutable byte Arc without copying it at open.

`lunco-assets-core::registered_read` owns bounded reads through registered
sources and admitted directories. USD preparation and UI byte loading consume
that owner. `lunco-assets-runtime::ui_loader::AssetBytesLoader` supplies one
asynchronous pending/ready/failed state machine; a language adapter returns an
`AssetReadTarget`, not bytes or another reader. Standard `file:` URIs use
`lunco-storage` decoding, while internal logical addresses retain literal `#`
and `%`. Scheme names normalize to lower case; package names, authorities and
filename case remain unchanged.

Document views of admitted directory sources retain the directory's source-set
identity. Independent document-backed packages have document-scoped registrations;
duplicate namespaces fail with their source paths, rather than replacing another
document. Directory owners retain per-file declaration provenance: fresh document
parses replace that file's facts without removing sibling-file members. Pending
or invalid parses reject resources visibly. Saves commit facts to the existing
owner, including while an older worker admission is in flight. Closing a view
drops its unsaved overlay and preserves saved facts; replacing a Twin retires
that Twin's roots and dependent UI caches.

Native file/directory renames relocate each affected source root, saved file,
package provenance and document overlay together. Pre-rename admission results
are fenced immediately. After the document/class/save transaction, the existing
worker reloads affected disk roots in stable registration order; parsing and I/O
stay off the UI thread. Closed sources follow the same relocation path.

The storage URI boundary accepts RFC 8089's authority form (`file:///path`)
and minimal absolute form (`file:/path`); both use the maintained `url` crate
for native conversion and reject malformed escapes, queries and fragments.

The workbench's `OpenAssetResource` previews these same requests. Invalid links
handled by a registered scheme remain in-app with their diagnostic. Text
previews are bounded by `asset_ui.max_text_preview_bytes`; resource reads use
`asset_ui.max_resource_bytes`. Native raster decoding uses egui's maintained
asynchronous image loader. Source/root revisions clear bytes, decoded images
and textures together. Detached byte workers retain discarded result slots. Scope invalidation also
replaces the maintained raster decoder instance: completed old decodes retain
their old cache Arc and cannot publish into the new scope's image cache.
Invalidation runs before workbench widgets read resources.

The resource window bounds its scrolling content to the viewport. Long URIs
use an ellipsized label with full hover text and a Copy URI context action;
they do not consume the image area as wrapped lines. Image size hints and text
scroll regions use the visible body, including after asynchronous loading or a
resource switch. The text-size
notice precedes its bounded text region so it remains reachable. Opening or
reloading a resource resets the window's content scroll position.

TODO — browser HTTP allocation: the registered browser directory source uses
Bevy's `HttpWasmAssetReader`, which buffers the full response before returning a
reader. Its allocation therefore precedes the common consumer budget. Replace
that backend with bounded streaming through the shared transport/asset owner,
and add a browser regression for an oversized HTTP payload. OPFS streaming
coverage does not establish this HTTP contract.

Native source-directory preparation uses the storage owner's ordered listing,
shared root containment and canonical identity checks. Escapes, repeated
files/directories and enumeration failures produce admission diagnostics;
source sets cannot silently include an outside file or a partial traversal.

Native directory identities are admitted by `canonical_directory_root` before
source-store installation. Registered roots are absolute, canonical directories;
symlink/case spellings cannot create another source identity. Installation is
idempotent for the same ordered store and rejects a different store during the
application lifetime. A missing optional cache is `Ok(None)`; an invalid explicit
library override, malformed store or failed canonicalization is an error. Dataset
provisioning cannot replace that rejection with an unrelated cached library.
The settings panel reads the admitted resident root and performs no filesystem
resolution during painting.

Explicit native `file:` views use the storage boundary and its native filename
contract. They do not reinterpret a native filename as a portable asset path.
Language source identities require exact UTF-8; source discovery reports an
unrepresentable path instead of dropping components or replacing bytes. Reloading
an open resource is an explicit typed request which retires its byte/texture
entry and decoder generation before reading again.
