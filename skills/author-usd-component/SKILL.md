---
name: author-usd-component
description: >
  How to AUTHOR a `.usda` asset from scratch for LunCoSim — geometry, materials,
  physics, behaviour, tunable parameters, and getting it into the spawn catalog.
  USE THIS SKILL when the user asks to "build/model/make a &lt;thing&gt;" as a reusable
  asset: a habitat, a lander, a rover part, an antenna, a porthole, a tank; or
  says "add a material/shader to it", "make it collide", "give it physics", "make
  it spawnable", "expose that as a slider", "put a hole in it", "make this
  parametric". For the agent mid-code: writing `def Xform`/`def Mesh`/`def
  NurbsPatch`, `apiSchemas`, `material:binding`, `trimCurve:*`, `customData
  {min,max}`, `lunco:program:*`, `lunco:spawnable`.
  This is the AUTHORING side. To assemble a scene from assets that already exist
  use build-usd-scene; to drive the running app use test-via-api.
  Project-specific and non-obvious: `xformOpOrder` is MANDATORY (without it every
  `xformOp:*` is ignored and the prim sits at identity), `uRange`/`vRange` are
  never read, `customData` has no `doc` key, a scalar `displayColor` is silently
  dropped, a dynamic mesh collider needs `physics:approximation`, and a new
  `lunco:*` property is inert unless THREE schema files are edited.
---

# Author a USD component

USD is the **source of truth**, projected to Bevy ECS. You build a thing by
writing a `.usda` file; the engine reads it. Nothing here is a Rust change.

Frame is fixed: **Y-up, right-handed, −Z-forward, SI metres** (`docs/architecture/41-axes-and-units.md`).
Author in that frame. `upAxis = "Z"` / `metersPerUnit != 1` are converted once at
the importer (`crates/lunco-usd-bevy/src/units.rs:172`) — never branch on them.

Background: [`21-domain-usd.md`](../../docs/architecture/21-domain-usd.md),
[`50-usd-driven-visuals.md`](../../docs/architecture/50-usd-driven-visuals.md).
Related skills: [`use-asset-library`](../use-asset-library/SKILL.md) (where the
file goes, how it is discovered, the `lunco://` scheme),
[`build-usd-scene`](../build-usd-scene/SKILL.md) (assemble),
[`validate-assets`](../validate-assets/SKILL.md) (pre-flight it),
[`test-via-api`](../test-via-api/SKILL.md) (verify), [`compose-multidomain-twin`](../compose-multidomain-twin/SKILL.md).

## Skeleton

**One file = one spawnable thing.** The catalog keys off the file, and
`lunco:spawnable` must sit on the stage's `defaultPrim`.

```usda
#usda 1.0
(
    defaultPrim = "Widget"
    upAxis = "Y"
    metersPerUnit = 1.0
    doc = """What this is, and where its numbers came from."""
)

def Xform "Widget" (
    kind = "component"
    prepend apiSchemas = ["LunCoCatalogAPI"]
)
{
    uniform bool lunco:spawnable = true
    float lunco:spawnLift = 0          # lift off the terrain when spawned

    def Scope "Looks" { def Material "Shell" { ... } }

    def Mesh "Body" (prepend apiSchemas = ["MaterialBindingAPI"]) { ... }
}
```

`kind` is **authored but read by nothing** — standard-USD hygiene for DCC
interop, not an engine signal. Use `doc = "..."` prim metadata for descriptions;
`lunco:description` was deleted (`crates/lunco-scene-commands/src/spawn_meta.rs:44`).

## Transforms — the mandatory bit

```usda
double3 xformOp:translate = (0, 1.5, 0)
double3 xformOp:scale = (1, 0.5, 1)
uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:scale"]
```

**Without `xformOpOrder` the prim is at identity and every `xformOp:*` is
ignored, silently.** There is no piecewise fallback — it was deliberately deleted
(`crates/lunco-usd-bevy/src/lib.rs:3040-3064`). This is the single most common way
to author a correct-looking file that does nothing.

Supported: `translate`, `scale`, `orient` (quat, USD `(w,x,y,z)`), `transform`
(matrix4d), `rotateX/Y/Z` (degrees), all six Euler orders, and the `!invert!`
prefix. Ops compose in listed order, so the **last listed applies first** to the
geometry. An op that is listed but unreadable is skipped as identity — silently.

Also: a translation of `(0,0,0)`, an identity rotation, or an all-zero scale will
**not overwrite** an existing spawned transform (`lib.rs:1101-1114`). Authoring
zero is a no-op, not a reset.

## Geometry

| Type | Attributes (defaults) |
|---|---|
| `Cube` | `size` (**2.0**) — always uniform; use `xformOp:scale` for a box |
| `Sphere` | `radius` (1.0) |
| `Cylinder` / `Cone` | `radius` (1.0), `height` (2.0), `axis` (**"Z"**) |
| `Capsule` | `radius` (0.5), `height` (1.0) — height is the cylindrical section only |
| `Plane` | `width` (2.0), `length` (2.0) |
| `Mesh` | `points`, `faceVertexCounts`, `faceVertexIndices` — all three required |
| `NurbsPatch` | see below |
| `BasisCurves` / `NurbsCurves` | `points`, **`widths` required** |

`axis` defaults to `"Z"`, not Y — a `Cylinder` with no `axis` lies along Z
(`lib.rs:1144`). `Cube.width/height/depth` do **not** exist. `extent` is never read.

**Not supported at all:** `Points`, `GeomSubset`, `PointInstancer`,
`instanceable`, `subdivisionScheme` (a `catmullClark` mesh renders as its raw
control cage).

### Mesh rules

- Output is unindexed triangles; n-gons are **fan-triangulated**, so author
  convex faces or triangulate yourself.
- `orientation = "leftHanded"` flips winding; default is right-handed/CCW.
- Any malformed topology → **no mesh at all**, no fallback primitive.
- **Interpolation is inferred from array length only** — `interpolation`
  metadata is never read (`lib.rs:3864`). An array matching `points.len()` is
  per-vertex; one matching `faceVertexIndices.len()` is faceVarying; **any other
  length is silently ignored**. So `uniform`/`constant` normals or UVs vanish.
- UVs: **`primvars:st` only**, UV_0 only. Bare `st` and `primvars:st0` are not read.
- Normals: authored `normals` used, else flat-computed. No smoothing.

### NurbsPatch

```usda
def NurbsPatch "Wall" {
    int uVertexCount = 9
    int vVertexCount = 2
    int uOrder = 3                      # default is 4 if unauthored
    int vOrder = 2
    double[] uKnots = [0,0,0,1,1,2,2,3,3,4,4,4]
    double[] vKnots = [0,0,1,1]
    point3f[] points = [ ... ]          # v-major: v rows of u points
    double[] pointWeights = [ ... ]     # omit ⇒ all 1 ⇒ POLYNOMIAL, not rational
}
```

- **Point order is `index = iv * uVertexCount + iu`** — v-major rows of u-points.
  `pointWeights` uses the same index.
- **`uRange` / `vRange` are NEVER READ.** The parametric span comes from the
  knots: `[uKnots[uOrder-1], uKnots[uVertexCount]]`. Authoring a range that
  disagrees with the knots does nothing at all.
- **Omitting `pointWeights` silently gives you the wrong shape.** A circle needs
  rational weights `1, cos45, 1, cos45, …`; without them the "circle" is a
  quadratic B-spline through a square control polygon and bulges ~6% at the
  diagonals. Pinned by `nurbs::tests::dropping_weights_visibly_breaks_the_circle`.
- Tessellation is fixed, not adaptive: `clamp(count * 6, 8, 128)` per direction.
- Normals are analytic; a degenerate row (a dome apex) yields `+Y` rather than NaN.

Every rejection path warns with a reason (`crates/lunco-usd-bevy/src/nurbs.rs:176-300`).
If a patch is missing from the render, **read the log first** — it will say which
guard fired, and untrimmed patches log their vert count.

### Curves

`widths` is **required** — no widths, no mesh. That gate is what stops a camera
rail becoming a pipe. Note `basis = "bspline"` is approximated as CatmullRom
(interpolating, not hull-approximating) — `lib.rs:3407`.

## Real holes — trim curves

`trimCurve:*` is the only standard-USD way to put a genuine hole in a surface, and
it **is implemented** (a stale doc comment at `lib.rs:3550` claims otherwise —
ignore it).

```usda
int[] trimCurve:counts = [1]           # curves per loop
int[] trimCurve:orders = [2]           # 2 = linear = a polyline
int[] trimCurve:vertexCounts = [16]
double[] trimCurve:knots = [0,0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,15]
point3f[] trimCurve:points = [ (u, v, w), ... ]   # HOMOGENEOUS 2D
```

- Points are **homogeneous**: the position is `(x/w, y/w)`. Skipping the divide
  gives a subtly wrong, plausible-looking loop.
- Coordinates are in the patch's **raw parameter space** (from the knots), not
  normalised, and are deliberately not unit/axis converted.
- **Winding does not matter.** Classification is even-odd with the domain
  rectangle as an implicit outer loop, so USD's unstated keep/discard rule never
  has to be guessed (`trim.rs:29-44`).
- Parameter space is **anisotropic and non-linear**. On a cylinder, u spans
  circumference while v spans height, and a rational arc parameterises
  non-uniformly — at the quarter point of a 90° span the true angle is 21.598°,
  not 22.5°. A circle authored naively renders as a squashed, mis-sized shape.
- **A trim failure renders UNTRIMMED with a warning** — bigger than authored,
  never smaller. A hole that doesn't appear is a log line, not a silent nothing.
- Trim gives you no **reveal**: a trimmed surface has no side walls, so the wall
  thickness at the opening is open. Closing it needs a ruled surface lofted
  between the two loops, authored separately.

## Materials

Bind a `UsdPreviewSurface`; the `Looks` scope is convention only, enforced nowhere.

```usda
def Scope "Looks" {
    def Material "Shell" {
        token outputs:surface.connect = </Widget/Looks/Shell/Shader.outputs:surface>
        def Shader "Shader" {
            uniform token info:id = "UsdPreviewSurface"
            color3f inputs:diffuseColor = (0.42, 0.40, 0.38)
            float inputs:roughness = 0.9
            float inputs:metallic = 0.0
        }
    }
}
```

Read: `diffuseColor`, `emissiveColor`, `metallic`, `roughness`, `normal`,
`occlusion`, `opacity`, `opacityThreshold`, `ior`, `clearcoat`,
`clearcoatRoughness`, `useSpecularWorkflow`, `specularColor`.

- **`MaterialBindingAPI` does NOT need applying.** Resolution uses
  `compute_bound_material` via `::on`, so bindings **inherit down namespace** and
  collection-based bindings work (`lib.rs:1568-1591`). Applying it is harmless.
- **`primvars:displayColor` must be an ARRAY.** `color3f[] primvars:displayColor
  = [(r,g,b)]`. A scalar `color3f` is silently ignored, and the bare
  `displayColor` alias is not read at all. Same for `float[] primvars:displayOpacity`.
  Values are **linear**, not sRGB.
- **`displayColor` is the ONE place a colour is authored, shader or not.** A WGSL
  shader opts in with `//!@engine display_color` and the engine fills that uniform
  from the prim's composed `primvars:displayColor` — so a shader-bound part is
  still painted the ordinary USD way. Don't author a parallel colour input on the
  Shader prim; use `inputs:*` only for what displayColor cannot express (accents,
  panel scale, wear). An explicit `inputs:display_color` overrides the fill.
- **`inputs:*` authored directly on a gprim is not read** — it must be on a bound
  Shader. This used to work and was removed as invalid USD.
- `doubleSided` (on the **gprim**, default false) is required for anything you can
  see through — a trimmed surface reads as a hole from one side and nothing from
  the other without it.
- Alpha: `opacity < 1` or a connected `inputs:opacity` → Blend;
  `opacityThreshold > 0` → Mask. No blend-mode control, no unlit from USD (use an
  emissive-only surface: `diffuseColor` 0, `emissiveColor` C).
- Textures: `UsdUVTexture` via `inputs:file`, `wrapS`/`wrapT`,
  `inputs:sourceColorSpace`. **There is no UV primvar reader** —
  `UsdPrimvarReader_float2`/`inputs:st` is inert; UVs come from the mesh's own
  `primvars:st`. If distinct metallic and roughness textures are both connected,
  the **metallic one is silently dropped** (one Bevy slot).

Custom WGSL is bindable via `uniform asset info:wgsl:sourceAsset = @lunco://shaders/x.wgsl@`
with `inputs:*` as parameters — but that path uses a **weaker resolver** (no
inheritance, no collections), so bind it **directly on the gprim**. Binding a
library shader with no `@fragment` entry (e.g. `pbr_lit.wgsl`) is refused with a
warning, deliberately — an invalid pipeline poisons the cache until restart.

## Physics

```usda
def Xform "Body" (prepend apiSchemas = ["PhysicsRigidBodyAPI"]) {
    float physics:mass = 4.5
    def Mesh "Hull" (prepend apiSchemas = ["PhysicsCollisionAPI"]) {
        uniform token physics:approximation = "convexHull"
    }
}
```

Backend is **Avian3D**. One prim with `PhysicsRigidBodyAPI` becomes **one**
rigid body aggregating all descendant colliders into a compound; descendants
carry `PhysicsCollisionAPI` only and get **no** independent body.

> **A component that gets MOUNTED must not apply `PhysicsRigidBodyAPI`.** The
> loader honours the schema wherever it appears — ancestry is never consulted,
> because nesting-plus-joint is how a wheel is mounted — so a part inside a
> vehicle that no joint names is a free body and falls out of it. That shipped:
> `components/mobility/motor.usda` applied it, and four motors per rover dropped
> through the hull on the first physics step while every parity test stayed
> green. An internal part is **mass + geometry** (`PhysicsMassAPI`,
> `PhysicsCollisionAPI` on its gprims) — `gearbox.usda` is the model to copy. A
> part that must MOVE relative to its host gets a body **and** a joint, authored
> together, which is what a mount (`AttachSpec`) writes. `sandbox --validate`
> reports the mistake as `[usd/nested-body-no-joint]`; see
> [`author-usd-physics`](../author-usd-physics/SKILL.md#6-a-part-is-not-a-body).

- **`physics:approximation` defaults to `trimesh`, and a trimesh cannot be a
  moving rigid body in parry.** A dynamic mesh body must author `"convexHull"` or
  `"convexDecomposition"` or it will not behave.
- **There is no `physics:friction`.** Use `physics:dynamicFriction` /
  `physics:staticFriction` / `physics:restitution` on a material bound through
  `material:binding:physics`. The invented name survived months of use.
- `physics:density` is not read anywhere. Author `physics:mass`.
- `PhysicsScene` gravity attributes are vendored but not consumed.
- Non-cuboid colliders lose exactness under non-uniform scale (tessellated to a
  convex hull); cuboids stay exact.
- Joints: `PhysicsFixedJoint`, `PhysicsRevoluteJoint`, `PhysicsPrismaticJoint`.
  Generic D6 is unsupported and warns.

Render and collision are allowed to differ, and only a cutaway view can tell.
That is a legitimate technique, not a bug — but write down that you did it.

## Behaviour — one binding for every language

There is **no per-language schema**. `LunCoProgramAPI` / `LunCoProgramAPI` is modelled
on `UsdShade.Shader`, and **the engine comes from the source's file extension**,
exactly as USD picks a file-format plugin.

```usda
def Xform "Balloon" (prepend apiSchemas = ["LunCoProgramAPI"]) {
    uniform asset info:sourceAsset = @lunco://models/Balloon.mo@
    uniform bool lunco:program:realtimeSafe = true

    float inputs:force_y.connect = </Balloon.outputs:netForce>
    float inputs:height.connect  = </Balloon.outputs:height>
}
```

`.mo` → Modelica, `.py` → Python, `.rhai` → Rhai, `.btxml` → behaviour tree
(`.xml` is accepted only for upstream interoperability).
**Nothing else about the prim changes.**

- **Role is derived, never declared.** A program with `inputs:`/`outputs:` ports
  is a node in the port graph and is stepped; one without them runs for effects
  only. **Parameters are ports** — a gain is `float inputs:kv = 1.2`.
- `LunCoProgramAPI` when the program *is* the thing (a vessel's flight control);
  a child `LunCoProgramAPI` **prim** for a guidance law or patrol tree, so deleting
  the prim deletes the behaviour. Only the prim form has `sourceAsset:subIdentifier`
  for multi-model `.mo` files.
- **`realtimeSafe` defaults to `false`, and the wiring pass will then refuse it a
  force/torque port on a client-predicted body.** A correctly-wired program can do
  nothing until this is authored `true`.
- **`sourceAsset` must be typed `asset`, never `string`** — only an `asset` is
  visible to the resolver, the reference closure, and packaging.
- `lunco:program:id` names a registered Rust driver instead of a source. It is a
  `token`. An unregistered id is a **warning no-op, not an error** (forward compat) —
  easy to miss.
- Wiring is native USD `connectionPaths`. `lunco:simWires` and wire-prims are
  **deleted**; `SimConnection` is a derived cache, so hand-authoring one is pointless.

Older attributes still exist and **inline always wins over path**: `lunco:script` /
`lunco:scriptPath`. Prefer `lunco:program:*`. Behaviour trees follow the ordinary
`LunCoProgramAPI` rule — a child prim (conventionally `Mission`) whose `info:sourceCode` /
`info:sourceAsset` ends in canonical `.btxml`, exactly as `.rhai` and `.mo`
select their engines. Imported BehaviorTree.CPP/Groot/ROS `.xml` is also accepted.

Vehicles are a special case with **no fallbacks**: a wheel missing any
`LunCoWheelAPI` / `LunCoTireAPI` attribute logs an error and **refuses to spawn**.
Compose `components/mobility/wheel.usda` rather than authoring one.

## Tunable parameters → Inspector sliders

```usda
double radius = 7.345 (
    customData = {
        double min = 3.0
        double max = 12.0
        string unit = "m"
        string type = "double"
    }
)
```

- Keys are exactly **`min`, `max`, `unit`, `type`**. There is **no `doc` key** —
  documentation goes in USD's own `doc = "..."` metadata.
- **Both `min` and `max` are required, and `max > min`**, or the parameter is
  skipped silently.
- `type` drives write-back and **defaults to `"float"`** — set it for a `double`.
- Only scalars readable as `f64`.

USD has **no expressions**. A measured quantity and the transform encoding it are
two authored numbers you must keep consistent by hand. Author both, and write the
invariant in a comment — the measurement is the durable record, the transform is
an encoding of it, and losing the measurement to a scale factor is how a fitted
number quietly becomes a magic constant.

## Spawnable, variants, persistence

**Catalog** is fully derived, nothing hardcoded: `lunco:spawnable = true` on the
`defaultPrim`, `id` = file stem, **`category` = the immediate parent folder,
Title-cased** (`vessels/rovers/x.usda` → "Rovers"). An unreadable file is not
spawnable. `RescanSpawnCatalog` re-reads.

**Variants** — a variant should *choose* a component, not restate one:

```usda
prepend variantSets = "tire"
variants = { string tire = "regolith" }
variantSet "tire" = {
    "regolith" (prepend references = @lunco://components/mobility/tires/regolith.usda@</Tire>) { }
}
```

Switch at runtime with `SetVariantSelection`. **Every variant must author every
property the others do** — a variant that only sets what it needs leaves the
previous variant's opinions standing, so it accumulates rather than switches.

**Persistence:** only **doc-backed twin scenes** keep runtime edits. A scene
opened as a raw file path reloads base bytes and discards every edit on restart.
A twin is a folder with `twin.toml` (`name`, `[usd] default_scene`), addressed as
`twin://<name>/<rel>`; runtime state lands in `.lunco/runtime/`, journal in
`history/`.

## Adding a new `lunco:*` property — source + regenerate

A new property is **inert** until it reaches the registered layer:

1. Edit `crates/lunco-usd/schema/schema.usda` — the source, **never read at runtime**
2. Run `python3 scripts/gen_schema.py` — regenerates
   `crates/lunco-usd/schema/generatedSchema.usda`, the file actually compiled in
   (never hand-edit it)
3. A new CLASS additionally needs a `crates/lunco-usd/schema/plugInfo.json`
   Types entry (`every_schema_class_is_registered_in_pluginfo` pins this)

Registry tests pin source↔generated class parity and (for the wheel domain)
schema UI hints, so a forgotten regenerate fails loudly.

**Schema-level sliders.** `customData = { double min; double max; string unit }`
on a SCHEMA attribute gives every asset composing that schema a derived
Inspector slider with zero per-asset authoring (`SchemaRegistry::ui_hint`,
consumed by `produce_usd_param_view`). Per-asset authored `customData` still
overrides. Hints are UI metadata only — value defaults stay in the component
`.usda` (no-fallback doctrine), e.g. `components/mobility/wheel.usda` for
wheels.

## Verify

**Pre-flight first — it costs seconds and needs no app:**

```bash
cargo run -p lunco-sandbox --bin sandbox -- --validate assets/<your file>.usda
```

It parses the layer, **composes the whole reference closure** (so a dangling
`@lunco://…@` fails loudly here instead of silently at load), and runs the strict
wheel reader on any `PhysxVehicleWheelAPI` prim. See
[`validate-assets`](../validate-assets/SKILL.md).

Then author → load → look. Per [`test-via-api`](../test-via-api/SKILL.md): drive the
**already-running** workbench, never `pkill`, and always nest arguments under
`"params"` — with the `{"command":…}` spelling anything top-level is silently
dropped and the command runs with defaults (a camera command then quietly aims at
the origin). Nesting is the one shape both envelope spellings accept.

Check the log before concluding anything about geometry. The reader warns on
every skip path, and "no warning + no geometry" means the prim was never
traversed — a different bug from "patch rejected".

## Anti-patterns

- ❌ `xformOp:*` without `xformOpOrder` — identity, silently.
- ❌ A `NurbsPatch` circle without `pointWeights` — a bulged rounded square.
- ❌ Trusting `uRange`/`vRange` — unread; the knots define the span.
- ❌ Scalar `primvars:displayColor` — must be an array.
- ❌ `inputs:roughness` on the gprim — must be on a bound Shader.
- ❌ A dynamic mesh body without `physics:approximation` — trimesh can't move.
- ❌ `physics:friction` — does not exist.
- ❌ `string doc` inside `customData` — not a key; use prim `doc` metadata.
- ❌ `info:sourceAsset` typed as `string` — must be `asset`.
- ❌ Editing `schema.usda` without running `scripts/gen_schema.py` — the
  runtime reads only the generated layer.
- ❌ Hand-editing `generatedSchema.usda` — the next regenerate erases it.
- ❌ Assuming `kind` does something.
- ❌ Inferring geometry from a screenshot when a number would settle it. Trim
  loops, control nets and joints are arithmetic — check the arithmetic. A view
  chosen on a symmetry axis of the hypotheses you are deciding between cannot
  discriminate them, and will confidently confirm whichever you already believe.
