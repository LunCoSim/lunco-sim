---
name: use-asset-library
description: >
  How to GROW THE LUNCO ASSET LIBRARY without writing any Rust — drop in a new
  USD component, a WGSL shader, a Modelica `.mo` behaviour, or an event-driven Rhai policy,
  and have the engine find and use it.
  USE THIS SKILL when the user asks "where do I put this file", "how do I add a
  new part/shader/model/script", "how does the spawn palette find things", "why
  doesn't my asset show up in the palette", "why is my program never running",
  "what does `lunco://` mean", "my reference doesn't load", "how do I add an
  asset to the web build", or wants an entry point into `assets/` generally.
  Project-specific and non-obvious: a shipped asset MUST be referenced as
  `@lunco://…@` (a bare relative path resolves against the *anchoring document*,
  so the same file breaks once a scene is Twin-mounted — and a failed
  `info:sourceAsset` load is SILENT, the prim is just never driven),
  `lunco:spawnable` is only read on the stage `defaultPrim`, the palette
  category is the IMMEDIATE parent folder Title-cased, a `.mo` with no
  `inputs:`/`outputs:` ports is never stepped, rhai `import` must NOT use
  `lunco://`, and the web build needs `scripts/build_web.sh` re-run to
  regenerate `manifest.json`.
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

| Folder | Holds |
|---|---|
| `assets/components/` | reusable part prims referenced into vessels — `mobility/`, `power/`, `thermal/`, `lights/`, `gnc/`, `comms/` |
| `assets/vessels/` | whole vehicles — `rovers/`, `landers/`, `satellites/`, `balloons/`, plus `control_profiles.usda` |
| `assets/structures/` | surface installations — habitat, mast, ISRU plant, landing pad |
| `assets/props/` | simple scene objects — ball, ramp, wall |
| `assets/scenes/` | loadable stages — `sandbox/*.usda` |
| `assets/models/` | behaviour sources: `.mo` (Modelica), `.py` |
| `assets/scenarios/` | `.rhai` bound as a `LunCoProgramAPI` source |
| `assets/scripting/` | importable rhai modules — `lib/`, `prelude/`, `policy/`, `tools/` |
| `assets/shaders/` | `.wgsl` |
| `assets/celestial/`, `missions/`, `tutorials/`, `config/` | the rest |

Only **`.usda`, `.wgsl`, `.rhai`** are walked into the discovery manifest
(`MANIFEST_EXTS`, `crates/lunco-assets/src/discovery.rs:276`). A `.mo` is found
because a `.usda` names it, never by scanning.

## The `lunco://` scheme

`lunco://<rel>` = `<repo>/assets/<rel>`, with a fallback to the download cache
(`crates/lunco-assets/src/lunco_source.rs:87`). `twin://<name>/<rel>` is the
same shape one level down: the Twin root, then that Twin's own `<twin>/.cache`.
Authored bytes always win over materialised ones. Schemes are registered in
`crates/lunco-assets/src/asset_sources.rs:20`: `lunco://`, `twin://`,
`cached_textures://`.

Anything the cache fallback can serve is DECLARED in an `Assets.toml` and
downloaded only on request (Settings ▸ Downloadable data, or the
`lunco-assets` CLI) — the engine never fetches on its own, so an asset that is
merely declared resolves to nothing until someone asks for it.

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

**The failure is silent for programs.** `crates/lunco-usd-sim/src/cosim.rs:249`
reads `info:sourceAsset`; a `None` or an unresolvable asset is a bare
`return` — no warning, the prim is simply never driven. "My model does nothing"
is nearly always this. The guard test
`crates/lunco-usd/tests/program_sources_exist.rs` walks every `.usda` and asserts
each `sourceAsset` file exists — but only [`validate-assets`](../validate-assets/SKILL.md)
catches a broken `references` arc before you launch.

> **rhai `import` does NOT use `lunco://`.** Module ids are bare anchored paths:
> `import "/scripting/lib/shots"` ✅ / `import "lunco://scripting/lib/shots"` ❌
> ("Module not found").

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
    float lunco:spawnLift = 0.5     # metres lifted off terrain on spawn
}
```

**How it reaches the palette** (`crates/lunco-scene-commands/src/catalog.rs`):

| Palette field | Derived from |
|---|---|
| `id` | the **file stem** |
| `display_name` | stem Title-cased (splits `_` and `-`) |
| `category` | the **immediate parent folder**, Title-cased |
| description | the stage's `doc` metadata |
| lift | `lunco:spawnLift` |

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
2. **No `inputs:`/`outputs:` ⇒ never stepped.** `cosim.rs:264` requires at least
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
assemblies are the worked examples. Rhai is event and mission glue only; an
`on_tick` is appropriate in tests, not as an actuator or numerical bridge.

## Regenerate the web manifest

Native runtime walks the filesystem, so **adding a file needs no step at all**.
The **web** build has no filesystem: it fetches `assets/manifest.json`
(`discovery.rs:160`). After adding or removing any `.usda`/`.wgsl`/`.rhai`:

```bash
./scripts/build_web.sh build sandbox
```

which rsyncs `assets/` into `dist/` and runs
`cargo run -p lunco-assets --bin build_asset_manifest -- <dist>/assets`
(`scripts/build_web.sh:608-640`). That generator calls the **same**
`discovery::scan_library` the native runtime uses, so the two cannot drift.
There is no standalone regenerate command.

> **Known gap** (`# TODO(asset-staging)`, `build_web.sh:646`): fonts, `.png`
> textures and `.glb` models are still hardcoded lists in the script. A new
> binary asset does **not** reach the web bundle until someone edits
> `build_web.sh`, and the failure is a silent 404.

## Validate before you run

```bash
cargo run -p lunco-sandbox --bin sandbox -- --validate assets/vessels/rovers/my_rover.usda
```

Seconds, no GPU, no app. Composes the whole reference closure — so it catches
the broken `@lunco://…@` that would otherwise be a mystery at load — and runs the
strict wheel reader. See [`validate-assets`](../validate-assets/SKILL.md).

## Anti-patterns

- ❌ A bare relative reference to a **shipped** asset — it resolves against the
  anchoring document, so the file breaks the moment a Twin mounts it. Use
  `@lunco://…@`.
- ❌ `@../../…@` anywhere — `..` escapes the root and returns NotFound. There
  are zero such refs in the tree; keep it that way.
- ❌ `lunco://` in a rhai `import` — module ids are bare anchored paths.
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
