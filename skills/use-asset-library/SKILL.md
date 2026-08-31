---
name: use-asset-library
description: >
  Add or locate LunCoSim assets under `assets/`: USD components, WGSL shaders,
  Modelica models, or event-driven Rhai policies. Use for discovery, spawn
  palette entries, `lunco://` references, Twin-mounted assets, source programs,
  or web manifests. This skill owns placement and resolution; use
  author-usd-component for USD authoring and validate-assets for pre-flight.
---

# Use the asset library

Almost everything in LunCoSim is an **asset file**, not a Rust type. A part, a
vehicle, a material, a subsystem's physics, a mission script — all of them are
files under `assets/` that the engine discovers at runtime.

> **Rust ships parameterized behaviours; it never hardcodes a thing.** If you
> are about to add a Rust struct for a specific rover, habitat, or shader, stop
> and add a file instead.

Related: [`author-usd-component`](../author-usd-component/SKILL.md) (how to write
the `.usda`), [`build-vehicle`](../build-vehicle/SKILL.md) (assemble parts),
[`build-usd-scene`](../build-usd-scene/SKILL.md) (assemble a scene),
[`author-scenario`](../author-scenario/SKILL.md) (rhai),
[`run-modelica`](../run-modelica/SKILL.md) (`.mo`),
[`validate-assets`](../validate-assets/SKILL.md) (**pre-flight before you run**).
Design: [`56-asset-resolution-and-cache.md`](../../docs/architecture/56-asset-resolution-and-cache.md),
[`50-usd-driven-visuals.md`](../../docs/architecture/50-usd-driven-visuals.md).

## Where things live

The canonical human/AI map is [`assets/README.md`](../../assets/README.md).
Read it before adding a file; the filesystem is the runtime manifest and the
README defines the stable taxonomy.

| Folder | Holds |
|---|---|
| `assets/components/` | reusable part prims referenced into vessels — domain folders include `avionics/`, `cameras/`, `comms/`, `environment/`, `gnc/`, `lights/`, `mobility/`, `mounting/`, `payload/`, `power/`, `terrain/`, `thermal/` |
| `assets/vessels/` | whole vehicles — `rovers/`, `landers/`, `satellites/`, `balloons/`, plus `control_profiles.usda` |
| `assets/structures/` | surface installations — habitat, mast, ISRU plant, landing pad |
| `assets/props/` | simple scene objects — ball, ramp, wall |
| `assets/scenes/` | loadable stages — `base/`, `luncosim/`, `tests/`, `celestial/` |
| `assets/models/` | behaviour sources: `.mo` (Modelica), `.py` |
| `assets/scenarios/` | `.rhai` bound as a `LunCoProgramAPI` source |
| `assets/behaviors/` | reusable `.btxml` behavior trees |
| `assets/scripting/` | importable rhai modules — `lib/`, `prelude/`, `policy/`, `tools/` |
| `assets/shaders/` | `.wgsl` |
| `assets/celestial/`, `missions/`, `tutorials/`, `lighting/`, `config/` | global/application data |

## Architecture boundary

- USD owns assembly, transforms, variants, references, and wiring.
- Modelica owns continuous/domain math.
- Rhai owns scenario orchestration, rules, scoring, and assertions.
- BT.CPP XML owns reusable inspectable behavior.
- Rust owns only general heavy runtime capabilities and bridges; expose their
  controls to Rhai instead of baking mission policy into Rust.

Environment facts are produced by a distinct
`components/environment/probe.usda` source prim. Never make a Modelica/Python
consumer declare `gravity_accel`, `sun_mount_*`, or `earth_mount_*` as its own
output and connect that output back to itself.

The engine-recognized **source** extensions are walked into the discovery manifest
(`crates/lunco-assets/src/discovery.rs`): **`.usda`, `.wgsl`, `.rhai`, `.mo`,
`.py`, `.btxml`**. `.mo` (Modelica), `.py` (Python), and `.btxml` (BT.CPP behaviour trees)
are catalogued both because a `.usda` names them and so they can be browsed
directly — the Scenarios menu lists every registered source file, grouped by type.
Non-source data (`.json`, `.toml`) is not walked: it is read by a subsystem or
evaluated ad hoc, not browsed as an authored asset.

## The `lunco://` scheme

`lunco://<rel>` = the runtime `assets/<rel>`, with that root's packed cache and
the shared cache after authored assets. The runtime root is selected by
`lunco_assets::assets_dir_abs()` from the executable/package ancestry before
the current-directory ancestry; the complete order is built by
`lunco_assets::library_roots()` (`crates/lunco-assets/src/lunco_source.rs`).
`twin://<name>/<rel>` is the same shape one level down: the Twin's authored
root, its `<twin>/.cache`, then the global cache. This lets Twins reuse a
global downloaded product without putting a machine path into USD.
Authored bytes always win over materialised ones. Schemes are registered in
`crates/lunco-assets/src/asset_sources.rs`; `twin://` is stateful, it is not a
second texture scheme. Use the existing logical `lunco://` or `twin://` identity
for every delivered artifact.

Anything the cache fallback can serve is DECLARED in an `Assets.toml` and
downloaded only on request (Settings ▸ Downloadable data, or the
`lunco-assets` CLI) — the engine never fetches on its own, so an asset that is
merely declared resolves to nothing until someone asks for it.

All requesters use the `download` section of the one settings file owned by
`lunco-settings`. `DownloadSettings.max_attempts` includes the first request;
retry waits are exponential and capped. Native and browser fetches retain
received bytes and resume with HTTP `Range` when the origin supports it.
Callers must pass the shared settings resource through the existing asset/API
surface rather than adding a local retry loop or path.

**A bare relative path is not "wrong" — it is resolved against the anchoring
document's directory, keeping that document's scheme.** That is why it bites:

```usda
# ✅ engine library — works no matter who mounts this file
prepend references = @lunco://components/mobility/wheel.usda@
prepend references = @lunco://components/power/battery.usda@

# ✅ a file sitting next to a Twin scene
uniform asset info:sourceAsset = @twin://my_mission/gnc.rhai@

# ⚠️ only legal when this file is itself inside assets/ AND never Twin-mounted
uniform asset info:sourceAsset = @scenarios/foo.rhai@

# ❌ always — `..` escapes the root and returns NotFound
prepend references = @../../components/mobility/wheel.usda@
```

The active program contract is visible in the runtime status. A Modelica or
Python source with no declared `inputs:`/`outputs:` is reported as source-only;
it is not treated as a running participant. `AttachProgram` is the canonical
way to add the source and its explicit scalar contract. "My model does nothing"
should be diagnosed by checking `CosimStatus` and `GetBrokenConnections`, not by
assuming a hidden fallback. The guard test
`crates/lunco-usd/tests/program_sources_exist.rs` walks every `.usda` and asserts
each `sourceAsset` file exists — but only [`validate-assets`](../validate-assets/SKILL.md)
catches a broken `references` arc before you launch.

> Rhai `import` uses the same canonical asset identity as USD. Use a logical
> `lunco://…` or `twin://…` URI, an assets-root `/…` path, or a path relative to
> the importing script. `RhaiSourceLoader` loads every literal import as a Bevy
> dependency; unused scripts are not preloaded.

## Add a USD component

Write one file = one spawnable thing. The full authoring reference is
[`author-usd-component`](../author-usd-component/SKILL.md); the *library* rules
are:

```usda
#usda 1.0
( defaultPrim = "Widget"   # ← lunco:spawnable is ONLY read here
  upAxis = "Y"  metersPerUnit = 1.0
  doc = """What this is." """ )

def Xform "Widget" ( kind = "component" prepend apiSchemas = ["LunCoCatalogAPI"] )
{
    uniform bool lunco:spawnable = true
}
```

**How it reaches the palette** (`crates/lunco-scene-commands/src/catalog.rs`):

| Palette field | Derived from |
|---|---|
| `id` | the **file stem** |
| `display_name` | stem Title-cased (splits `_` and `-`) |
| `category` | the **immediate parent folder**, Title-cased |
| description | the stage's `doc` metadata |

So `components/power/solar_panel.usda` lands under **"Power"** — not
"Components". A file with no parent folder lands in "Other". Nothing is
hardcoded; **moving the file changes its category.**

- `lunco:spawnable` defaults to **false** — it is opt-in.
- It must sit on the stage's `defaultPrim`. On any other prim the palette never
  sees it (child `lunco:spawnable` is a different feature — subpart selection).
- An unreadable file is not spawnable and logs `CATALOG: … unreadable`.
- Editing an already-scanned file? Send **`RescanSpawnCatalog`** — the scan
  caches per asset. Adding a *new* file is picked up automatically on native
  (the filesystem is the manifest).

## Add a shader (`.wgsl`)

Drop it in `assets/shaders/`. It is walked into the manifest and registered into
the `ShaderCatalog` automatically (`RescanShaders` to re-read edits). Bind it
**directly on the gprim**:

```usda
uniform asset info:wgsl:sourceAsset = @lunco://shaders/rover_hull.wgsl@
```

The tunable surface is reflected from a `struct Material` at
`@group(2) @binding(0)`, annotated with `//!@` comments
(`crates/lunco-materials/src/dyn_params.rs`). There are exactly **three**
directives — there is no `//!@param`:

```wgsl
struct Material {
    //!@engine display_color
    display_color: vec3<f32>,
    //!@ui 0.0 1.0 "Wear"
    wear: f32,
    //!@default albedo 0.17,0.17,0.17
    //!@ui color "Accent"
    accent_color: vec3<f32>,
}
```

| Directive | Effect |
|---|---|
| `//!@engine <name>` | the **engine fills this uniform** — see the registry below |
| `//!@ui <name> [args] "Label"` | `color` / `int min max` / `min max` (slider) / else free |
| `//!@default <name> <v>[,<v>…]` | packed value when nothing else supplies one |

### Engine-filled uniforms

`crates/lunco-materials/src/engine_params.rs` is the **provider registry** — a
process-wide `OnceLock`, so the validator, the prop picker and the renderer all
read the same list.

| `//!@engine` name | Filled from | Usable on a prop? |
|---|---|---|
| `display_color` | the prim's composed `primvars:displayColor` **element 0** | ✅ |
| `sun_vis` | horizon ray-march visibility | ✅ |
| `sun_dir`, `sun_dir_world`, `sun_tan_radius` | sun direction / angular radius | ❌ |
| `hf_size`, `hf_res`, `csm_far`, `shadow_cache_on` | terrain heightfield + shadow state | ❌ |

**The colour contract: author `primvars:displayColor`, the shader consumes it.**

```usda
color3f[] primvars:displayColor = [(0.30, 0.72, 0.35)]   # ARRAY, linear
```

One authored attribute, in the standard USD place, whether the part renders
through plain PBR or through WGSL. An authored `inputs:<name>` on the bound
Shader **always wins** over the engine fill — but authoring `inputs:display_color`
hides the colour from every other tool that reads USD, so use `inputs:` only for
what displayColor cannot express (accents, panel scale, wear, dust).

A shader using any ❌ param is refused by the **prop material picker** (it would
render black on a rover part) but still works as a scene shader — that is exactly
the `not prop-pickable` warning from
[`validate-assets`](../validate-assets/SKILL.md). An unregistered `//!@engine`
name warns and packs to its `//!@default` (or zero) — nothing fills it.

## Add a Modelica behaviour (`.mo`)

For an acausal physical domain, author component class/nameplate facts and
connector topology in USD. The runtime projector compiles each connected island
into one Modelica model; never bind one solver program per electrical part.

Three gates, each of which silently does nothing when unmet:

1. **The language is the file extension**, nothing else. `.mo` → Modelica,
   `.py` → Python, `.rhai` → rhai, `.btxml` → behaviour tree (`.xml` accepted for interop).
2. **No `inputs:`/`outputs:` ⇒ never stepped.** The cosimulation projector requires at least
   one port-prefixed attribute. A model with no ports is a documentation-only
   reference.
3. **`realtimeSafe` defaults to `false`**, and the wiring pass then refuses the
   prim a force/torque port on a client-predicted body. Author it `true` when
   the program drives a force.

And `sourceAsset` must be typed **`asset`**, never `string` — only an `asset` is
visible to the resolver, the reference closure, and packaging.

**Write it branch-free.** rumoca's solver path has no `if`/`when` in equations —
express clamps as `der(x) = expr` with `max()`/`min()`.
[`validate-assets`](../validate-assets/SKILL.md) enforces this as an error;
`assets/models/LunCo/Electrical/Battery.mo` plus the reusable rover electrical
assemblies are the worked examples. Rhai is event and mission glue only;
production scenarios must not define `on_tick`. Authored tests may use it for
bounded state sampling and verdicts. Keep equations in Modelica and ordinary
progression in task/events.

## Regenerate the web manifest

Native runtime walks the filesystem, so **adding a file needs no step at all**.
The **web** build has no filesystem: it fetches `assets/manifest.json`
(`crates/lunco-assets/src/discovery.rs`). After adding or removing any catalogued source
(`.usda`/`.wgsl`/`.rhai`/`.mo`/`.btxml`):

```bash
./scripts/build_web.sh build luncosim
```

which rsyncs `assets/` into `dist/` and runs
`cargo run -p lunco-assets --bin build_asset_manifest -- <dist>/assets`
(`scripts/build_web.sh`). That generator calls the **same**
`discovery::scan_library` the native runtime uses, so the two cannot drift.
There is no standalone regenerate command.

Binary runtime assets are staged by the manifest-declared bundle target as part
of the same build. Add or change the `bundle` field on the authoritative
`Assets.toml` entry, then rebuild; do not add a second shell list. The staging
command validates the declared raw/processed artifact before copying it, so a
missing or incomplete asset fails the build instead of becoming a browser 404.
Bundle-qualified keys are for log identity; temporary download files use an
opaque process/attempt name, so adding a grouped asset never creates a
platform-dependent cache subdirectory.

## Validate before you run

```bash
target/debug/luncosim --validate assets/vessels/rovers/my_rover.usda
```

Seconds, no GPU, no app. Composes the whole reference closure — so it catches
the broken `@lunco://…@` that would otherwise be a mystery at load — and runs the
strict wheel reader. See [`validate-assets`](../validate-assets/SKILL.md).

## Anti-patterns

USD composition is owned by `lunco-usd-compose`: `lunco-assets` supplies
canonical IDs, traversal, and bytes; composition interprets USD arcs into an
inert stage. Modelica, Rhai, behavior trees, physics, and rendering bind later
in their own layers. A tutorial only projects metadata from that stage.

- ❌ A bare relative reference to a **shipped** asset — it resolves against the
  anchoring document, so the file breaks the moment a Twin mounts it. Use
  `@lunco://…@`.
- ❌ `@../../…@` anywhere — `..` escapes the root and returns NotFound. There
  are zero such refs in the tree; keep it that way.
- ❌ A dynamic/non-literal Rhai `import` — dependencies must be known while the
  asset is loading, so the loader rejects it visibly.
- ❌ `lunco:spawnable` on a prim that is not the stage `defaultPrim` — invisible
  to the palette.
- ❌ Encoding a category in the filename — the category IS the parent folder.
- ❌ A `LunCoProgramAPI` with no `inputs:`/`outputs:` and expecting it to run.
- ❌ `info:sourceAsset` typed `string` — must be `asset`.
- ❌ `if`/`when` in a `.mo` equation section — rumoca is branch-free.
- ❌ Authoring `inputs:display_color` instead of `primvars:displayColor` — it
  works, and it hides the colour from every other USD consumer.
- ❌ Adding a Rust struct for a specific vehicle/part/material. It is a file.
- ❌ Assuming the web build picked up a new asset without re-running
  `build_web.sh`.
