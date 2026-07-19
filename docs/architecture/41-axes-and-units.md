# 41 — Axes and Units (coordinate + unit conversion boundary)

> Status: Active — **the USD spoke is built in both directions** (import and
> save); the remaining spokes and the mass/stiffness/damping dimension machinery
> are design · Audience: contributors importing/exporting coordinates & units
>
> External tools disagree on **up-axis** and **units**; LunCoSim must not.
> The engine runs in **one fixed canonical frame** (spec 009: f64, Y-up, RH,
> −Z-forward, SI). Any external representation (USD, glTF, Blender, Isaac, ROS)
> is converted **once, at the importer**. Internal code never branches on
> convention.

## As built — the USD spoke (`lunco-usd-bevy/src/units.rs`)

The mandate above is **honoured for USD geometry and transforms**. Two types:

| Type | What it is |
|---|---|
| **`StageMetrics`** | the stage's declared convention — `meters_per_unit: f64` + `up_axis: UpAxis::{Y,Z}` — read from the pseudo-root metadata by `StageMetrics::from_reader`. This is the **only** place `upAxis` / `metersPerUnit` are interpreted. |
| **`ConventionTransform`** | the precomputed stage→canonical **similarity** `S = k·Q` (uniform scale `k = metersPerUnit`, rotation `Q` = identity for Y-up, `Rx(−90°)` for Z-up). Built solely by `ConventionTransform::from_stage_metrics`. `#[must_use]`. |

**The conversion is baked into the shared decoders, not applied to a root
entity.** A root-only rotation/scale is explicitly rejected here (see *Boundary
discipline* below): avian colliders, `big_space`, and the f64 frame tree all
assume SI Y-up, so a non-SI value must never flow downstream. Every consumer —
the visual sync, `lunco-usd-avian`'s colliders, the gizmo — already funnels
through these decoders:

| decoder | conversion applied |
|---|---|
| `local_transform_at` (→ `read_transform_from_usd`, the mount/footprint walks) | `ConventionTransform::local_transform` |
| `read_shape_dims` | `ConventionTransform::length` on every dimension |
| `build_usd_mesh` / `read_usd_mesh_indexed` | `point` on positions, `dir` on normals |
| the `axis` token of a `Cylinder`/`Cone`/`Capsule`/`Plane` | `ConventionTransform::orient` |
| the animation sampler's translate/rotate/scale channels | `point` / `rotation` / `scale_vec` (conjugation is separable across the three, so per-channel agrees exactly with the whole-transform conversion) |

**Why *conjugation*, not a left-multiply.** For a prim chain
`W = L₁·L₂·…·Lₙ` acting on local geometry `p`, the canonical world position is
`S·W·p`. Rewriting:

```text
S·L₁·L₂·…·Lₙ·p  =  (S·L₁·S⁻¹)(S·L₂·S⁻¹)…(S·Lₙ·S⁻¹) · S·p
```

⇒ conjugate **every local transform** (`Lᵢ' = S·Lᵢ·S⁻¹`) **and** convert the
leaf geometry (`p' = S·p`). Both, not either — which is exactly why the decoders
convert transforms *and* points/dims. Conjugation is what lets each level of the
hierarchy be converted independently, with no knowledge of its parents.

`Q` is either the identity or a ±90° axis swap (USD defines only `upAxis` `Y`/`Z`),
so componentwise `|Q·s|` on a non-uniform scale is exact. It would **not** be for
an arbitrary rotation — that is why `Q` is derived from a two-valued token and
never from free-form data.

**Failure behaviour is loud, never silent.** An `upAxis` that is neither `Y` nor
`Z`, or a non-finite/non-positive `metersPerUnit`, is an `error_once!` (and falls
back to the canonical value). A *supported but non-canonical* stage logs a
one-shot warning naming what is being converted.

**Every asset we ship is `upAxis="Y", metersPerUnit=1`, so
`ConventionTransform::is_identity()` holds and the hot decoders early-out** —
import of our own content is bit-for-bit what it was before this module existed.
The seam exists, is exercised by unit tests (Z-up remap, centimetre stage,
identity stage, and a conjugated-chain property test), and costs nothing.

### Save: the same map, backwards, at one seam

Every read-side conversion has a `stage_*` counterpart applying `S⁻¹ = (1/k)·Q⁻¹`,
and they are applied inside `UsdDocument::apply` — the single dispatch path every
editor, the API, MCP and scripts already funnel through. The invariant is worth
stating plainly:

> **A `UsdOp`'s spatial values are always canonical. Stage frame exists only
> inside the layer.**

That is what makes an op *portable*: the same journalled edit replays correctly
against a centimetre stage and a metre one. It also means the conversion belongs
at the boundary rather than at the dozen producers, each of which would otherwise
have to remember — and one forgotten producer is a silently corrupt file.

Both directions are converted at that seam. The forward path converts before
authoring; the INVERSE op converts the value it reads back out of the layer. That
second half is easy to miss and was its own bug: an undo op built from a raw
layer read carries stage-frame numbers, so on a centimetre stage undo moved a
prim to 1/100th of where it had been.

Three kinds of value, three mechanisms:

| Value | How the frame is known |
|---|---|
| `xformOp:translate` / `rotateXYZ` | the op itself is spatial by definition |
| `point3f`, `vector3f`, `normal3f`, `quatf`… | the USD **type role** — `point3f` and `color3f` are the same `Vec3f` once decoded, and only the type says which one scales |
| `radius`, `height`, `focalLength`… | the **schema**, via the registry's `(schema, property)` key — USD has no role type for a scalar length |

The scalar case carries a factor, not a flag: `UsdGeomCamera` defines focal
length and apertures in *tenths* of a world unit, so "this is a length" alone
would still author them 10× wrong.

Anything unannotated is left alone. Under-reach misplaces a value; over-reach
destroys one, so conversion is opt-in per declaration.

**Still open:** `UsdOp::SetTimeSample` does not yet convert — see the engineering
backlog.

## How other software does this (prior art)

Two philosophies exist; we pick the first.

**A. Fixed internal convention, convert once at the importer** — the
game-engine / interchange world:

| System | Internal frame | At the boundary |
|---|---|---|
| **glTF** | mandates Y-up, metres, RH — *no metadata, no config* | importer into a Z-up engine applies a fixed rotation |
| **Unreal** | cm, Z-up, LH | importers convert; nothing internal branches on convention |
| **Unity / Bevy** | m, Y-up | same — fixed convention, importer converts |
| **Omniverse** | Z-up USD/Fabric | converts non-conforming USD on import |
| **Blender** | Z-up | USD/glTF importer exposes "convert orientation + scale" as **import-time settings**, applied once |
| **USD itself** | — | *declares* `metersPerUnit` / `upAxis` as metadata; does **not** auto-convert — "I tell you the convention, you adapt" |

**B. Tag the data with its frame, convert lazily via a central broker** —
robotics / geospatial:

| System | Mechanism |
|---|---|
| **ROS tf2** (REP-103/105) | every message carries `frame_id`; a central transform tree answers "this point in frame X" |
| **PROJ / GDAL** | data carries its CRS; PROJ converts between any two via a central library |

**Unit / dimensional safety** also splits two ways:

- **Type-level** — `uom` (Rust), Boost.Units, F# units-of-measure: compile-time
  dimensional analysis via phantom types. Rigorous but **heavy**; even
  scientific codebases use it sparingly.
- **Discipline** — SI internally, convert at I/O, enforce with interface specs
  + tests. This is what JPL adopted after the Mars Climate Orbiter was lost to
  an lbf-vs-newton mismatch *at a boundary*. The dominant approach.

## Recommendation — pattern A, not B

LunCoSim is a **single-world engine** (Bevy + `big_space`), not multi-live-frame
robot middleware. So:

1. **Fixed internal convention, no config** — spec 009 SI Y-up. Internal code
   *never* branches on convention.
2. **Convert once, at the importer** — one choke point per format.
3. **Store the source's convention as metadata** so save round-trips — exactly
   what USD does (declare, don't auto-apply). We adapt at import, write back on
   save.
4. **SI internally; dimensional factors live only at the boundary accessor.**
   Do not thread unit *types* through the engine (JPL lesson, applied
   pragmatically — not via the type system).
5. **No type-level units (`uom` / phantom types) for the whole app.** Too heavy,
   and the industry largely agrees. Reserve it for a single boundary only if
   that one proves error-prone.
6. **Guardrail = round-trip tests, not types.** The one thing pattern A can't
   catch — an importer that bypasses the layer — is caught by a fixture test
   (load cm/Z-up → assert SI Y-up out), not by the type system. tf2's
   tag-everything *would* catch it but is overkill for an import seam.

> tf2 (pattern B) **does** apply to us — but for a *different* problem: the
> robot's own moving frames (Link-Joint TF tree, spec 009). That is internal
> and already canonical SI. The **import boundary** is pattern A.

## Canonical frame

Per [`009-coordinate-frame-tree`](../../specs/009-coordinate-frame-tree/spec.md):
**f64, Y-up, right-handed, −Z-forward, SI (metre, kilogram, second, radian)**,
the Link-Joint TF tree, `big_space` floating origin. Nothing inside the engine
deviates from this.

## Hub-and-spoke (the eventual shape, not the first build)

```
   Blender(Z-up,m)   Isaac/USD(Z-up,?u)   glTF(Y-up,m)   ROS/URDF(Z-up,X-fwd,m)
         \                  |                  |                 /
          ▼                 ▼                  ▼                ▼
   ┌──────────────── convert once at the importer ────────────────────┐
   │   to_canonical(Convention, Units)   /   from_canonical(...)        │
   └───────────────────────────────┬──────────────────────────────────┘
                                    ▼
   CANONICAL (spec 009)
```

USD is the **interchange hub**: live Blender / Isaac sync through USD, so the
only conversion edge is *external-document ↔ canonical*. N spokes, not N²
adapters — but each spoke is only worth building when its convention actually
appears (see staged plan).

## Full API shape — **design; does not exist yet**

> **This section is a target, not a description.** What exists today is the
> `StageMetrics` / `ConventionTransform` pair described in *As built* above:
> a `metersPerUnit` scale + an up-axis rotation, with `point` / `dir` / `orient` /
> `rotation` / `scale_vec` / `length` / `local_transform`. There is **no `Units`
> struct, no `Dimension` exponents, no `kilogramsPerUnit`, and no
> `mass()` / `stiffness()` / `damping()` accessor** — mass and the physics scalars
> are read unconverted. Every asset we own is SI, so nothing is wrong today; the
> machinery below is what a non-SI *mass* would need.

```rust
/// Which way is up / forward + chirality. All our targets are right-handed
/// → axis remap is a pure rotation (never a mirror).
pub struct Convention { pub up: Axis, pub forward: Axis, pub handedness: Handedness }
impl Convention {
    pub const CANONICAL: Self;     // Bevy: Y-up, -Z-forward, RH
    pub const USD_DEFAULT: Self;   // Y-up
    pub const BLENDER: Self;       // Z-up, -Y-forward
    pub const GLTF: Self;          // Y-up
    pub const ROS: Self;           // Z-up, X-forward (REP-103)
    pub const ISAAC: Self;         // Z-up
}

/// Base ratios to SI. A scene declares these (USD stage metrics).
pub struct Units {
    pub meters_per_unit: f64,
    pub kilograms_per_unit: f64,
    pub seconds_per_unit: f64,     // ~always 1
    pub radians_per_unit: f64,     // degrees→radians lives here
}

/// SI dimension exponents. ONE set of base ratios converts ANY quantity:
///   factor = kg^M · m^L · s^T …
/// length L=1 ; mass M=1 ; velocity L=1,T=-1 ; force M=1,L=1,T=-2 ;
/// stiffness M=1,T=-2 ; damping M=1,T=-1 ; inertia M=1,L=2.
pub struct Dimension { pub mass: i8, pub length: i8, pub time: i8 }
impl Dimension {
    pub const LENGTH: Self; pub const MASS: Self; pub const STIFFNESS: Self;
    pub const DAMPING: Self; pub const FORCE: Self; pub const INERTIA: Self;
}

/// Precomputed rotation + dimensional scaling. `from_canonical` is the exact
/// inverse so files round-trip in their declared convention.
pub struct ConventionTransform { /* DQuat rot, base ratios */ }
impl ConventionTransform {
    pub const IDENTITY: Self;                                  // Y-up + SI → no-op
    pub fn from_stage_metrics(m: &StageMetrics) -> Self;       // the one construction point
    pub fn from_canonical(to: Convention, units: Units) -> Self;

    pub fn point(&self, p: DVec3) -> DVec3;       // position
    pub fn dir(&self, v: DVec3) -> DVec3;         // velocity/force dir (no translate)
    pub fn rot(&self, q: DQuat) -> DQuat;
    pub fn qty(&self, x: f64, d: Dimension) -> f64;

    // named shorthands — the dimension is fixed by the method, so a call site
    // CANNOT scale the wrong dimension:
    pub fn length(&self, x: f64) -> f64;
    pub fn mass(&self, x: f64) -> f64;
    pub fn stiffness(&self, x: f64) -> f64;       // spec 030 SC-003 falls out here
    pub fn damping(&self, x: f64) -> f64;
}
```

Spec 030 SC-003 (`springStrength` / `dampingRate` parity USD↔Avian) would then be
a *special case* of `stiffness()` / `damping()` rather than a hand-tuned constant.
It is not one today.

## Enforcement — convenient AND hard to forget

> **Design.** The as-built USD spoke takes the *spirit* of this — a single
> construction point (`from_stage_metrics`), named dimension-fixing accessors
> (`length`, never a bare `qty`), and `#[must_use]` on the transform — but the
> `CanonicalScene` trait and the `raw_*`-is-private reader gate below are **not
> built**. `UsdRead` still exposes raw reads; the discipline is that the *shared
> decoders* (`local_transform_at`, `read_shape_dims`, `build_usd_mesh`) are the
> only callers of them, and every consumer goes through those decoders.

The design serves both goals at once rather than trading one off.

- **Convenient** = call sites read like normal code (`stage.mass(prim)`, plain
  `f64` / `DVec3`, no viral generics, nothing to "remember to call").
- **Can't-forget** = an unconverted value cannot reach engine code, and a new
  importer cannot quietly bypass the layer.

### Bake the gate into the reader; expose only converted, named accessors

The boundary asset is *constructed with* its `ConventionTransform`, so its
public accessors are already canonical. There is no separate type to thread and
**no public raw accessor to forget**:

```rust
// lunco-usd-bevy — UsdStageAsset built with its gate
impl UsdStageAsset {
    pub fn translate(&self, p: &Path) -> Option<DVec3> { Some(self.tf.point(self.raw_translate(p)?)) }
    pub fn mass     (&self, p: &Path) -> Option<f64>   { Some(self.tf.mass(self.raw_scalar(p, "physics:mass")?)) }
    pub fn stiffness(&self, p: &Path) -> Option<f64>   { Some(self.tf.stiffness(self.raw_scalar(p, "…")?)) }

    fn raw_translate(&self, ..)  // pub(crate)/private — UNREACHABLE downstream
}
```

`sync_usd_visuals` just calls `stage.translate(p)`. **Visibility is the
guardrail** — the raw value isn't public, so it can't be used unconverted; the
named accessor fixes the dimension, so it can't be mis-scaled.

The uniform contract across spokes is a trait whose *shape is the nudge* — it
has converting accessors and **no `raw_*` method**, so adding a spoke forces you
to answer "how does each quantity convert":

```rust
pub trait CanonicalScene {        // every spoke's composed asset implements this
    fn translate(&self, p: &Path) -> Option<DVec3>;   // already canonical
    fn rotation (&self, p: &Path) -> Option<DQuat>;
    fn mass     (&self, p: &Path) -> Option<f64>;
    fn stiffness(&self, p: &Path) -> Option<f64>;
}
```

Downstream generic code takes `&impl CanonicalScene` → convention-agnostic, no
`<B>` leaking into call sites.

### Why not the alternatives

| Option | Convenient? | Can't-forget? |
|---|---|---|
| Plain transform `tf.point(v)` at every call site | ✅ | ❌ raw value still in hand |
| Funnel wrapper `Canonical<B>` threaded through | ❌ viral generic, reimplement readers | ✅ |
| **Gate baked into reader; raw private** (chosen) | ✅ plain method calls | ✅ no public raw |
| Phantom types `Point<Frame>` / `Qty<Dim>` everywhere | ❌ ceremony on every value | ✅✅ |

Phantom types (the `uom` family) are the most rigorous but were rejected as
overcomplicated — see prior art; even pro sci-code uses them sparingly.

### The three holes types can't close — each gets a cheap guard

1. **Boundary author mislabels a reader** (few sites, one per format) →
   **round-trip property tests**: load a Z-up/cm fixture, assert SI Y-up out;
   save → reload → identity. Rigor lives here, not in a type zoo.
2. **A new importer bypasses the crate entirely** (types never get called) →
   **grep/lint test in CI**: fail if `metersPerUnit` / `upAxis` / raw spatial
   attr names appear outside the boundary modules.
3. **A raw value genuinely must surface** (passthrough of an unknown attr) →
   wrap in a `#[must_use] Raw<T>` with no `Deref` and no getter except
   `tf.qty(raw, dim)`. One wrapper, not the phantom zoo — forces a decision
   exactly where ambiguity is real, nowhere else.

Plus `#[must_use]` on `ConventionTransform` so an unused gate warns.

## Boundary discipline — bake to canonical at import

- **Import:** the transform is applied to every prim xform and every geometric
  dimension as prims materialize → the internal world is true SI Y-up. **A
  root-only rotation/scale is rejected**: avian colliders, `big_space`, and the
  spec-009 f64 tree all assume SI Y-up, so non-SI must never flow downstream.
  This is the single most re-introducible mistake in this subsystem — parenting
  the import under a scaled/rotated root *looks* right in the viewport and is
  wrong everywhere else.
- **Authoritative source keeps its convention.** The USD document stays in its
  declared metrics — we never rewrite the stage's `upAxis`/`metersPerUnit`. The
  matching `from_canonical`-on-save is applied at the `UsdDocument::apply`
  boundary, so an edit authored onto a non-canonical stage is written in that
  stage's own units and a read-modify-write round trip is the identity.

## Spokes

| Spoke | Convention source | Status |
|---|---|---|
| **USD** | stage metrics: `upAxis`, `metersPerUnit` | **built, both directions** — `StageMetrics` / `ConventionTransform`, with save applied at the `UsdDocument::apply` seam. `SetTimeSample` and `kilogramsPerUnit` are not covered. |
| glTF | fixed Y-up / metres (≈ identity spoke) | not built — additive |
| Blender (live) | Z-up / −Y-fwd / metres, via USD interchange | not built — the USD spoke already covers a Z-up Blender export |
| Isaac Sim | USD Z-up + centimetres | **covered by the USD spoke** — this is the exact stage the spoke was built for |
| ROS / URDF | Z-up / X-fwd / metres (REP-103) | not built — additive |

## Staged plan

**Every asset we own is `upAxis="Y", metersPerUnit=1`** → the transform is the
identity for our own content, and the decoders early-out on it. So the smallest
correct thing was built first; the full machinery is promoted only when it earns
its keep (YAGNI).

1. **Identity seam — DONE.** `StageMetrics` + `ConventionTransform` in
   `lunco-usd-bevy/src/units.rs`, applied inside the shared decoders (not at a
   root entity). Zero behavioural change on canonical content; a Z-up/cm stage
   now imports correctly instead of rotated 90° and 100× too small.
2. **Round-trip tests — DONE for import.** Unit tests cover the Z-up remap, the
   centimetre stage, the identity stage, and the conjugated-chain property
   (`S·W·p = L₁'·L₂'·S·p`). The **save**-side round trip is pinned by
   `a_non_canonical_stage_is_authored_in_its_own_frame` and
   `undo_on_a_non_canonical_stage_restores_the_original_position` in `lunco-usd`.
3. **Promote to a `lunco-axes-and-units` crate + `CanonicalScene` trait** when a
   *second* format spoke actually arrives. Speculative today — USD is the
   interchange hub, so Blender/Isaac arrive *through* USD, not beside it.
4. **Save path — OPEN.** `from_canonical` on write-back so files round-trip in
   their declared metrics. Until it exists, authoring onto a non-canonical stage
   is a known, warned-about gap.
5. **Dimensional scalars — OPEN.** `kilogramsPerUnit`, and the
   `mass()`/`stiffness()`/`damping()` accessors that a non-SI mass would need.
   Nothing we load is non-SI in mass today.

## Relationship to other specs

- [`009-coordinate-frame-tree`](../../specs/009-coordinate-frame-tree/spec.md) —
  defines the canonical f64 Link-Joint frame this layer targets.
- [`030-usd-scene-integration`](../../specs/030-usd-scene-integration/spec.md) —
  assumed upAxis handling + demands stiffness/damping parity; this layer
  delivers both.
- [`21-domain-usd.md`](21-domain-usd.md) — scene/stage ownership (Twin → active
  stage → Grid); the USD spoke runs at that document↔world boundary.
- [`40-asset-io.md`](40-asset-io.md) — asset I/O constraints and the wasm-safe I/O layer.

