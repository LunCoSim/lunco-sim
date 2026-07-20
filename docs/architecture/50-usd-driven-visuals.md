# USD-driven visuals — authored geometry, Rust logic, binding by name

A sensor beam, an exhaust plume, a route ribbon: geometry whose *shape* is authored and
whose *size* tracks a live simulation value. This is the contract for that class of thing.

Three rules, in order of how often they are broken:

1. **Geometry and look are authored in USD.** A `Cylinder`/`Cone` with a bound `Material`.
   Not a gizmo, not a mesh built in Rust from constants.
2. **Logic is Rust.** Not arithmetic in a script. The driver is a registered function
   selected by name.
3. **The live value never round-trips through the document.** It is ECS state.

## Why not gizmos

A gizmo line has no depth, a fixed screen-space width, and bypasses tonemapping. It draws
over terrain instead of being occluded by it, and it cannot be authored — its colour is a
Rust constant. `draw_waypoint_overlay` records the same conclusion for the route ribbon:
the egui screen-space stroke was removed because it had no depth and painted over the
terrain it was supposed to lie on.

A mesh occludes properly, has a real world thickness, and takes its colour from a
`Material` an author can edit without a compiler.

## The unit-primitive idiom — live size is `xformOp:scale`

**`radius`/`height` are read once, at instantiation, and baked into the `Mesh` handle.**
They are never re-read. A driver that writes `height` every frame changes nothing on
screen and pushes a per-frame edit through the document — which is the undo/journal/network
plane, not a scratchpad.

So authored dimensions are **unit**, and the live channel is the transform:

```usda
def Cone "Flame" {
    uniform token axis = "Y"
    double radius = 1.0          # unit primitive...
    double height = 1.0
    double3 xformOp:scale = (0.02, 0.02, 0.02)   # ...live size here
}
```

`axis` (`"X"`/`"Y"`/`"Z"`) is folded into the entity `Transform` at projection time, not
into the mesh — so it costs nothing to author and orients the primitive correctly.

## Binding a name to Rust — `ProgramDriverRegistry`

`LunCoProgram` names its implementation one of two ways, mirroring `UsdShade.Shader`'s own
`info:id` (a named built-in the renderer implements) versus `info:sourceAsset` (external
source):

| Attribute | Resolves to |
|---|---|
| `uniform asset lunco:program:sourceAsset = @scenarios/flame.rhai@` | the script engine (`.rhai`) or the behaviour-tree engine (`.xml`) |
| `uniform token lunco:program:id = "range_beam"` | a Rust driver, from `ProgramDriverRegistry` |

Same prim type, same discovery, same per-instance params. The registry follows
`ControlKernelRegistry` (`lunco-core/src/kernels.rs`) — a `Resource` holding a
`HashMap<String, fn>`, seeded idempotently in the owning plugin's `build`, with three
properties that are not optional:

- **built-in wins**, a script hook is the open fallback
- **an unknown id is a fail-safe no-op with a deduped warning** — never a panic
- USD *selects*; it does not *define*

This is the pattern `lunco:driveKernel` already ships. It is not new machinery.

> `kernels.rs` cites `register_commands!` as the same pattern. It is not — that macro
> dispatches by Rust *type* via bevy observers and has no string key. `ControlKernelRegistry`
> and `PortRegistry` are the name-dispatch precedents.

A driver reads its parameters from `ScriptParams`, the same `lunco:param:*` map a rhai
script reads through `param(me, key, default)` — **not** off the USD reader. A driver is
an ordinary Bevy system, and a system has no reader: everything it needs is projected
into the ECS at load, by `attach_programs`.

That makes `ScriptParams`' `HashMap<String, f64>` bind drivers too. It is `f64` because
rhai's `FLOAT` is (`script_param() -> Option<f64>`, `bridge_core.rs`), which is a
script-marshalling detail that leaked into a shared component — but the constraint is
real today, so **a driver's parameters must be numbers**. That is not the hardship it
sounds like: a colour belongs in a bound `Material`, where USD says it belongs, and
`UsdPreviewSurface` expresses it far better than a float map would.

## Casting shadows — `primvars:doNotCastShadows`

Alpha does not answer this question. A blended surface is still rasterised opaquely into
the shadow map, so a translucent plume throws a hard shadow until told not to:

```usda
def Cone "Flame" {
    bool primvars:doNotCastShadows = true
}
```

**This is Omniverse's name, not ours.** RTX reads it on the gprim and Composer surfaces it
as the mesh's "Cast Shadows" toggle, so a scene authored there arrives here with its shadow
intent intact, and one authored here keeps it there. Its polarity already matches
`PbrLook.no_shadow_cast` and Bevy's `NotShadowCaster` — no inversion.

It is a primvar, so it travels with the geometry and needs **no schema class of ours**.
`UsdGeom` has no say in gprim shadow casting (`UsdLuxShadowAPI` governs the *light*, not
the caster), which makes inventing one tempting — the first cut of this did exactly that,
adding a `LunCoRenderAPI` for a name Omniverse had already standardised.

Read on the **gprim**, not the shader: two prims sharing one material can disagree about
casting, and `material:binding` is not the place to say so.

> Reach for a vendor name before minting one. This repo already consumes NVIDIA's
> `PhysxRigidBodyAPI`, `PhysxGearJoint` and `PhysxVehicleTankDifferentialAPI` — Omniverse
> compatibility is the existing posture, not a new one.

## Unlit is not yours to author

A beam, a plume, a trajectory line is a **symbol**, not a surface: asking how the sun
falls on it is a category error. On the Moon it is not cosmetic either — there is no
atmosphere, so no ambient fill, and a lit surface facing away from the sun renders *pure
black*. A lit beam vanishes on the night side, exactly where an altimeter earns its keep.

`PbrLook.unlit` does this, but it is **render intent for Rust-spawned overlays** — the
brush, name labels, trajectory lines — and no `.usda` authors it. Authored scene content
says it the USD way, which `UsdPreviewSurface` expresses perfectly well:

```usda
color3f inputs:diffuseColor  = (0.0, 0.0, 0.0)
color3f inputs:emissiveColor = (1.0, 0.1, 0.1)
color3f inputs:specularColor = (0.0, 0.0, 0.0)
float   inputs:opacity       = 0.85   # sub-1 ⇒ Blend ⇒ alpha means something
```

The beam is authored, so it takes the USD route. The rule is the `unlit` doc's own.

## Engine-filled shader uniforms — a provider registry, not a branch

A dynamic WGSL shader declares its own parameters, and marks the ones the *author*
must not set with `//!@engine <name>`. Which names the engine knows how to fill,
what type each is, and where its value comes from is stated in exactly one place:
`lunco_materials::engine_params` (`EngineParams::builtin`). Adding an engine input
is one registry entry — never a new branch in a binder.

There are exactly two provider shapes, kept as separate `EngineSource` variants
because they differ in *when* the value is known:

| Variant | When | Examples |
|---|---|---|
| `PrimAttr` | per-prim, read from USD at look-authoring time and baked into the `ShaderLook`'s parameter map | `display_color` ← `primvars:displayColor[0]` |
| `Runtime` | written each frame by the engine system that owns the computation | `sun_vis` (horizon ray-march), the terrain heightfield family |

Because `PrimAttr` values ride in the look's parameter map, they follow the look
wherever it goes — including the wheel physics/visual split, which moves the look
onto a synthesized `*_visual` child. A live `SetAttribute` re-projects the prim and
re-authors the look, so the render follows the edit.

**Precedence: an authored `inputs:` always wins.** `//!@engine` marks a parameter
the engine *can* fill, not one the author is forbidden to set. An explicit
`inputs:display_color` on the `Shader` prim is already in the parameter map, and the
engine fill skips the name.

**`prop_fillable` is the registry's answer, not a literal.** A shader is offered in
the prop picker only if every `//!@engine` field it declares is one a plain prop
entity actually receives — terrain shaders declare `sun_dir`/`hf_size`, which only
the terrain binder fills, so they would render black on a prop.
`is_prop_pickable_source` asks the registry; registering a new prop-fillable
provider automatically widens the test.

### Colour is authored ONCE, as `primvars:displayColor`

`primvars:displayColor` is **the** place a colour is authored — shader-bound or not.
There is no parallel `inputs:hull_color`; that form was removed. A shader opts in
with `//!@engine display_color` and is painted the ordinary USD way, so the same
attribute drives a `UsdPreviewSurface` part and a procedural-shader part alike.
Use `inputs:*` only for what a colour cannot express — accents, panel scale, wear.

## Adding a `lunco:*` schema property — THREE files, or it is inert

`schema.usda` is the authoritative source **and is never read at runtime**.

| File | Role |
|---|---|
| `schema/schema.usda` | the authoritative source. **Not read at runtime.** |
| `schema/generatedSchema.usda` | what is compiled in (`include_str!`) and ingested by `lunco_usd::schema` |
| `schema/plugInfo.json` | the `Types` map, so external USD runtimes register the class |

Plus a reader to consume it, plus authoring on the asset.

**Pixar's `usdGenSchema` is NOT used here** — it is not installed, and the one-way
transform it performs is small enough to own. `python3 scripts/gen_schema.py`
regenerates `generatedSchema.usda` from `schema.usda`; run it after every edit to
the source. Do not hand-edit the generated file. Also add the class to
`generated_schema_parses_and_registers_every_type` and assert its property type in
`schema_declares_property_types`, or drift is silent.

### Slider bounds live in the schema's `customData`

A property's UI hints (`double min`, `double max`, `string unit`, plus
`string userDocBrief`) ride in the attribute's `customData` dictionary;
`SchemaRegistry::ui_hint` decodes them through `AttrUiHint::from_dict`, and a
per-asset authored `customData` still overrides. Core USD annotates nothing this
way, so the vendored `schema/core/*.usda` files carry ours.

> **ONE `customData` block per attribute.** The USDA parser folds metadata with
> `SpecData::add`, which **overwrites in place** — a second `customData = { … }`
> on the same attribute silently *replaces* the first rather than merging. Two
> blocks (say a min/max/unit block followed by a `userDocBrief` block) means the
> bounds are gone and every slider for that attribute is inert, with no warning.
> Put every key in one dictionary.

## Three ways to write a driver that does nothing

Every one of these compiled, type-checked, raised no warning, projected without error —
and did nothing. They are the reason this document exists.

**Reading a `token` with `scalar::<String>()`.** In `sdf::Value` a token is a distinct
variant from a string, so the call returns `None` for every authored value. `driver_id`
was always `None`, the program fell through to the rhai path, found no `.rhai`, and
`continue`d. No entity was ever stamped, so even the unknown-id warning stayed quiet —
there was nothing to warn about. Use `UsdRead::text()` for `token`, `::asset()` for
`asset`. The trap is that the neighbouring `lunco:program:sourceCode` is genuinely
`uniform string`, which makes the wrong accessor look right.

**Writing alpha to an opaque material.** `SurfaceAlpha` is derived from the authored
opacity: `opacity = 1.0` → `Opaque`, and an opaque material *discards* alpha. A driver
fading `base_color.alpha` on such a material writes into the void. The material must
author a sub-1 `inputs:opacity` to resolve to `Blend` before any alpha means anything.

**Verifying against a screenshot.** The undriven beam was a unit cylinder — 2 m across,
plainly visible, entirely inert. It photographs as "the beam renders". What caught it was
comparing two numbers: the sensor's `range` port against the beam's `scale.y`. Assert on
the value, not the pixels.

## What does not work, and why

| Approach | Why not |
|---|---|
| rhai drives the material | `set()` needs `register_type::<T>()` (auto-register is off workspace-wide) and `apply_dynamic` downcasts only scalars/vectors — no `LinearRgba`, no enums. `PbrLook` fails both gates. |
| rhai drives visibility | `Visibility` is registered but is an enum; `apply_dynamic` has no enum arm. Scripts work around this by scaling to zero. |
| `inventory` for the registry | Forces stateless bare `fn`s, adds link-time surface near the clang limit that already forced `reflect_auto_register` off, and `asset_sources.rs` steers explicitly toward a runtime resource for scriptable dispatch. |
| `register_commands!` | Dispatches by type, needs a Rust type per id, gives no string key. |
| A bespoke `lunco:sensor:beam*` attribute | One-off. The next visualization needs another attribute and another system — the taxonomy every new behaviour must edit, which `kernels.rs` exists to avoid. |
| A `lunco:*` name where a vendor already has one | `primvars:doNotCastShadows` is the worked example: a `LunCoRenderAPI` was written, then deleted, because Omniverse had already named the thing. Check Omniverse and core USD before minting. |

## Isaac Sim does the opposite, deliberately

Worth knowing rather than rediscovering: Isaac Sim's `RangeSensor` schema (`Lidar`,
`UltrasonicArray`) carries visualization as **bool flags on the sensor prim** —
`drawLines`, `drawPoints` — rendered by the extension's debug-draw, not as authored
geometry. That is the `rangeVisualize`-plus-gizmo shape this document replaces.

The split is *debug overlay* versus *scene content*. Isaac treats a lidar beam as the
former. Here the beam is authored because it is the visual of `rangeAxis`/`rangeMax`,
which are already authored on the same prim — splitting them is the incoherence being
removed. If a beam is ever wanted as a pure throwaway diagnostic, Isaac's answer is the
right one and this is the wrong one.
