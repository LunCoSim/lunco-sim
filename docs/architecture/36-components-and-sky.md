# 36 — Reusable multi-layer components & sky visualization

Status: **design / analysis**. The connectivity half of this doc is gone: comms is no
longer a subsystem, and its Rust module and `lunco:comms:*` vocabulary were deleted.
The generic link kernel that replaced it is `49-connectivity-link-kernel.md`. What
survives here is the part that was never about comms: how a **reusable multi-layer
component** composes in USD (§1), and how the **sky** is drawn (§2–§3).

---

## 1. The component model — a reusable multi-layer asset

A component must be a **reusable unit with many layers** (3D, Modelica dynamics,
electrical, policy) that composes into rovers *and* robots. **USD's composition engine
already gives us exactly this**, and the shipped Power/Mobility components + the
drivetrain variantSet are the working precedent. What is missing is a *discipline*:
today's components cram every domain onto one prim, which does not scale past one
solver. The multi-layer form below is what generalizes.

### 1.1 What USD composition already supports (verified)

The `openusd` PCP engine composes and the loader flattens (all carrying `lunco:` attrs through):

| Arc | Works? | Use for a component |
|---|---|---|
| **`references` (incl. sub-tree `@file@</Prim>`)** | ✅ read+compose; `author_reference()` helper | the primary reuse arc — pull a component asset onto a child prim; per-instance param overrides win (LIVERPS) |
| **`variantSet` / variants** | ✅ read+compose (drivetrain `raycast\|physical` ships) | **fidelity levels** and config, selected per instance |
| **`subLayers` + `inherits`(class)** | ✅ (control-profile pattern ships) | shared defaults across a fleet |
| **`over`** | ✅ | sparse per-instance tweaks / runtime overlays |
| **binary `.glb` payload** | ✅ deferred → `lunco:resolvedAsset` (async) | the **heavy 3D mesh** — load it lazily so a schematic/electrical view opens without geometry |
| **nesting (BFS, unbounded)** | ✅ scene→rover→component depth ≥3 already | **robot = arm → gripper + motors ×N + antenna**, each a referenced asset |
| `.usda` payload | ⚠️ *eager* (composes like a reference) | not a lazy boundary — keep heavy geometry as `.glb`, not `.usda`, if you need deferral |
| `instanceable` / native prototypes | ❌ dropped by flatten (`PrimPredicate::DEFAULT`) | **not usable today** — many identical antennas = N plain `references` (like six_wheel_rover's 6× wheel), or extend flatten to `DEFAULT_PROXIES` |

Takeaway: **reuse, fidelity variants, fleet defaults, deferred geometry, and deep nesting
all work now by hand-authoring arcs** in `.usda`. Only `references` has an authoring
*helper*; variants/payloads/inherits must be written into the asset text (fine —
components are authored assets, not runtime-built).

### 1.2 Anatomy of a multi-layer component

A component is a **referenceable USD asset** (`defaultPrim` = an `Xform`, USD
`kind="component"`) whose children are **one program prim per layer/domain**, each binding
at most one model — because the runtime is **one solver per prim** (doc 34; a program with
no ports is inert documentation, so extra domains *must* be separate prims).

Worked example — a communications component. Note that **nothing here is a comms
primitive in the core**: the link layer is just a generic [`LinkNode`](49-connectivity-link-kernel.md)
(range/elevation/class), and the RF behaviour is authored Modelica on top of it.

```usda
def Xform "CommsSystem" (kind = "component") {          # ── the reusable unit; defaultPrim
    # public interface — the component's "connectors" (see §1.3):
    custom string lunco:ports = "rf_out:out, p_draw:out, cmd_in:in, data_out:out"

    def Xform "Geom" {                                  # ── Layer 1: 3D / structure
        custom bool   lunco:linkNode             = 1    #     a generic connectivity endpoint
        custom string lunco:link:class           = "hga"
        custom double lunco:link:minElevationDeg = 5.0
        prepend payload = @lunco-lib://models/hga.glb@  #     heavy mesh → deferred binary payload
        # + collider, mount frame, optional gimbal joint
    }
    def LuncoProgram "Link" {                           # ── Layer 2: RF dynamics (Modelica)
        uniform asset lunco:program:sourceAsset = @models/CommsLink.mo@   # Friis → data-rate → buffer
        float inputs:u_range.connect = </CommsSystem/Geom.outputs:range_km>
        float inputs:u_up.connect    = </CommsSystem/Geom.outputs:connected>

        def LuncoPortEvent "Loss" {
            uniform token lunco:event:port = "margin_db"
            uniform token lunco:event:op = "lt"
            double lunco:event:threshold = 0.0
            uniform token lunco:event:emit = "comms:loss"
        }
        def LuncoPortEvent "Acquire" {
            uniform token lunco:event:port = "margin_db"
            uniform token lunco:event:op = "gt"
            double lunco:event:threshold = 3.0
            uniform token lunco:event:emit = "comms:acquire"
        }
    }
    def LuncoProgram "Power" {                          # ── Layer 3: electrical draw (Modelica)
        uniform asset lunco:program:sourceAsset = @models/CommsPower.mo@  # TX state → DC watts
        float inputs:u_tx.connect = </CommsSystem/Link.outputs:txActive>
        # its `outputs:p_draw` is what the vehicle's EPS bus consumes
    }
    def LuncoProgram "Policy" {                         # ── Layer 4: mode/relay policy (rhai)
        uniform asset lunco:program:sourceAsset = @scripts/comms_policy.rhai@   # handover, duty-cycle, safe-mode
    }
}
```

Layers are wired **internally** by native USD connections through `PortRegistry`.
The **geometry/LOS layer is not per-component Modelica** — it is the shared generic link
kernel (doc 49) publishing `range_m` / `elevation_deg` / `connected` on `LinkState`, which
the Link `.mo` consumes. This is the house layering exactly: **USD = structure/wiring,
Modelica/rhai = per-vehicle dynamics/policy, Rust = reusable substrate never authored per
vehicle**.

### 1.3 The public port interface (the composability crux)

For components to snap together like LEGO, each must expose a **small, named, typed port
interface** — its SSP *connectors* — so an assembly can wire to `CommsSystem.rf_out` /
`.p_draw` **without knowing the internals**. Today ports are discovered per-backend but a
component does not *declare its public surface*. Recommended addition: a `lunco:ports`
manifest on the component root (above) that registers those names as the component's
boundary; internal prim ports stay private. This is the one genuinely new substrate piece
the component model needs, and it is small (a manifest attr + a registry entry).

If an interface port ever needs a domain tag (to make an `rf_out` vs a `p_draw`
self-describing for tooling or connection validation), author it as a USD attribute/token
on the port, **not a closed core enum** — the old `PortType`/`classify` name-heuristic was
deleted precisely because a closed taxonomy had no reliable consumer.

> The electrical layer's causal-vs-acausal question is settled in
> `37-model-synthesis-and-multidomain-composition.md`: **acausal *within* a domain, causal
> *across* domains.** A component contributes its draw as a causal `p_draw` boundary port;
> the vehicle's electrical network is ONE synthesized acausal DAE. Read doc 37 for the
> full treatment.

### 1.4 Fidelity as a variantSet (+ the runtime toggle)

Map the layers onto a `variantSet "fidelity"` on the component — the *exact* shape of the
shipping drivetrain `raycast|physical` variant:

```usda
def Xform "CommsSystem" (kind="component") {
    variantSet "fidelity" = {
        "ideal"     { over "Link" { } over "Power" { } }        # kernel LOS only → connected bool
        "linkbudget"{ over "Link" { uniform asset lunco:program:sourceAsset = @models/CommsLink.mo@ } }   # + Friis/buffer
        "full"      { over "Link" {…} over "Power" {…} over "Therm" {…} }             # + electrical + thermal
    }
    prepend variantSets = "fidelity"
}
```

An assembly selects per instance (`variants = { string fidelity = "linkbudget" }`).
Authoring granularity (USD variant, per twin) and runtime granularity (toggle, per
session) compose.

### 1.5 How this generalizes to robots

A robot is the same mechanism nested: `def Xform "Rover" (kind="assembly")` → `references`
an `arm.usda` (itself `kind="assembly"` → `references` `gripper.usda` + `joint_motor.usda`
×N + `CommsSystem.usda`). Each referenced component brings its own layer sub-prims + its
`lunco:ports` interface; **assembly-level wire-prims** connect subcomponent ports (comms
`p_draw` → EPS bus, GNC `pointing_cmd` → antenna gimbal). Nesting is unbounded (BFS
loader), per-instance overrides work at every level, and identical parts are N `references`
(until native instancing is wired).

> **Net:** the composition capability is already present (references + variants + nesting +
> deferred `.glb` + `over`). A multi-layer component needs only the small `lunco:ports`
> public-interface manifest (§1.3). Everything else is authoring.

---

## 2. Sky visualization

Current state: bodies are **real physical-radius spheres / LOD terrain in `big_space`**
viewed through the floating origin, so Sun/Earth **already hold correct angular size by
geometry** — but there is **no starfield, no skybox, no env map, no atmosphere** (pure-black
clear color, intentional for the airless Moon), and **no curated sun/Earth disk** decoupled
from the physics sphere.

### What USD offers here

USD's environment vocabulary is **UsdLux**, and the import layer already reads it
(`lunco-usd-bevy/src/light.rs`):

| USD prim | Standard meaning | Status here | Sky use |
|---|---|---|---|
| `DistantLight` | infinitely-far directional (the Sun) + `inputs:angle` (0.53°) | **mapped** → `DirectionalLight` + `SunAngularDiameter` | already drives sun lighting & shadow penumbra |
| `DomeLight` (+ `inputs:texture:file`) | image-based environment / **skybox** | **partially mapped** — collapsed to a scalar ambient; **texture ignored** | **this is the skybox hook**: promote `DomeLight.texture:file` → Bevy `Skybox` + `EnvironmentMapLight` |
| `SphereLight` / `DiskLight` | area lights | `SphereLight`→Point/Spot | (local, not sky) |

**USD has no first-class "star catalog" or "planet-in-sky" prim.** The idiomatic USD
approach to a star/space background is a **`DomeLight` with an equirectangular starfield
texture**. So the USD-native answer to "stars + sky" is: **finish the `DomeLight` texture
path** into a real environment map instead of the current ambient-scalar collapse
(`light.rs:184-190` is a clean promotion point).

### Reuse the existing Earth model (don't build a new one)

Earth is **not** a mesh — it is a `CelestialBody { ephemeris_id:399, radius_m:6371e3 }`
whose surface is the `GlobeLod` blueprint-shader cube-sphere tile globe. It is already
positioned at 399's ephemeris coordinate in `big_space`, so **from a lunar surface camera
it renders at correct angular size for free** — reuse it directly, no impostor needed in
the common case.

**Coarsest LOD by default (sandbox).** LOD depth is `GlobeLod.max_lod`, selected per-face by
camera-distance vs tile arc-size in `update_globe_lod`. Two ways to get "coarse by default":
- **Quick:** set `GlobeLod.max_lod = 0` at the Earth/Moon insert sites → each body is a
  static **6-tile cube-sphere**, never subdivides.
- **Better:** add a `min_lod`/`pin_coarsest` field to `GlobeLod` consulted in
  `subdivide_face`, plus a sandbox config. Lets the sandbox default to coarse while a
  "high detail" toggle raises `max_lod`. Recommended — one field + one early return.

### Recommended sky work

1. **Starfield / space background** — `DomeLight`-authored equirect texture → Bevy `Skybox`
   + `EnvironmentMapLight` (needs `.ktx2`/cubemap; convert the equirect at import). Purely
   additive; no new USD schema — reuse the existing `DomeLight` prim, just honor its
   `texture:file`.
2. **Sun disk** — already an emissive ico-sphere at true radius, decoupled from the light.
   Correct angular size already. If it reads too small/large, that is tuning/bloom, not
   missing geometry.
3. **Earth-in-sky** — reuse the `GlobeLod` Earth above. **Verify** the surface camera's grid
   frame actually draws body 399's tiles (they may cull at range). If tiles cull at that
   distance, the cheap fix is `max_lod:0` (always-present 6-tile globe) rather than a
   separate impostor. Only if that is still too heavy, fall back to a billboard/impostor
   disk (angular diameter `2·atan(R_earth/range)`). Prefer reusing the real globe.
4. **Atmosphere** — none, and correctly so for the Moon. Bevy has a built-in `Atmosphere`
   (Nishita) if an Earth-side or transit view ever needs it; out of scope here.

> Payoff of sharing the geometry core: the same range/direction that decides *connected*
> (doc 49) also places the Earth disk and points the antenna gizmo — one source of truth for
> "where is Earth in the sky," shared by physics-of-links and pixels-in-sky.

---

## 3. Moving the celestial-body definition into USD

Today the body list is a **hardcoded Rust `vec!`** (`registry.rs default_system()`) and the
Earth/Moon spawn **bypasses several registry fields with inline literals** (name/id/radius
hardcoded; texture hardcoded, ignoring `BodyDescriptor.texture_path`). There is **no
celestial↔USD bridge** — a plain USD `Sphere` prim imports as a static `PbrLook`
sphere, *not* the `GlobeLod` globe. So moving Earth into USD is a clean, worthwhile refactor
but it is **new work**, not a config change.

**Approach — a `lunco:celestialBody` schema + handler:**
```usda
def Xform "Earth" (prepend apiSchemas = ["LunCoCelestialBodyAPI"]) {
    custom int    lunco:celestial:naifId        = 399
    custom double lunco:celestial:radius        = 6371000
    custom double lunco:celestial:gm            = 3.986e14
    custom double lunco:celestial:rotRatePerDay = 6.30
    custom asset  lunco:celestial:texture       = @textures/earth.png@
    custom int    lunco:globeLod:maxLod         = 0     # coarse by default; raise for detail
    custom int    lunco:globeLod:res            = 32
}
```
Add a handler in the USD projection that, on seeing `lunco:celestial:naifId`, inserts
`CelestialBody` + `GlobeLod` + `GravityProvider`/`SOI`/`Collider` — the exact bundle spawned
today, but data-driven from the prim.

**Sequencing caveat:** bodies live in the `big_space` grid hierarchy (inertial anchor Grid +
rotating body + surface Grid). A USD-authored body must slot into that hierarchy, so the
handler needs to run in (or hand off to) the celestial big-space setup rather than the
generic prim→entity path. Recommended **two-step migration**: (1) move the *parameters* to
USD (the registry reads a USD `def Scope "CelestialConfig"` instead of the Rust `vec!`) —
low risk, unlocks authoring radius/texture/LOD/epoch per twin; (2) later move the *spawn*
itself behind the `lunco:celestialBody` handler. Step 1 alone gives "Earth defined in USD"
and the coarse-LOD knob without touching the fragile big_space wiring.
