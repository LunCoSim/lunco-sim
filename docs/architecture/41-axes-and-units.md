# 41 — Axes and Units (coordinate + unit conversion boundary)

> Status: Active · Audience: contributors importing/exporting coordinates & units
>
> External tools disagree on **up-axis** and **units**; LunCoSim must not.
> The engine runs in **one fixed canonical frame** (spec 009: f64, Y-up, RH,
> −Z-forward, SI). Any external representation (USD, glTF, Blender, Isaac, ROS)
> is converted **once, at the importer**, and converted back on save. Internal
> code never branches on convention.

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

## API shape (pure: glam + f64; optional `bevy` feature)

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

Spec 030 SC-003 (`springStrength` / `dampingRate` parity USD↔Avian) is then a
*special case* of `stiffness()` / `damping()` — not a hand-tuned constant.

## Enforcement — convenient AND hard to forget

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

- **Import:** apply the transform to every prim xform **and every physical
  attribute** as Links materialize → the internal world is true SI Y-up. A
  root-only rotation is rejected: avian colliders, `big_space`, and the spec-009
  f64 tree all assume SI Y-up, so non-SI must never flow downstream.
- **Authoritative source keeps its convention.** The USD document stays in its
  declared metrics; we store `Convention` / `Units` as metadata and apply
  `from_canonical` on **save** so files round-trip. (Policy option:
  normalise-on-save to `upAxis=Y, metersPerUnit=1`.)

## Spokes

| Spoke | Convention source | Status |
|---|---|---|
| **USD** | stage metrics: `upAxis`, `metersPerUnit`, `kilogramsPerUnit` | **first** — load + save |
| glTF | fixed Y-up / metres (≈ identity spoke) | additive |
| Blender (live) | Z-up / −Y-fwd / metres, via USD interchange | additive |
| Isaac Sim | USD Z-up (+ its unit choice) | additive (USD spoke covers it) |
| ROS / URDF | Z-up / X-fwd / metres (REP-103) | additive |

`UsdComposer::get_default_prim` already exists for rooting; add a
`get_stage_metrics` sibling (reads `upAxis` / `metersPerUnit` /
`kilogramsPerUnit`) to feed the USD spoke.

## Staged plan — identity seam now, crate + trait when a 2nd convention lands

**Every asset we own is already `upAxis="Y", metersPerUnit=1`** → the transform
is **identity today**. So build the smallest correct thing first; promote to the
full machinery only when it earns its keep (YAGNI).

1. **Identity seam (now).** A small `ConventionTransform` *module* (not yet a
   separate crate): `from_stage_metrics`, `IDENTITY`, `point`/`dir`/`rot` +
   named scalars. Add `UsdComposer::get_stage_metrics`. `UsdStageAsset` holds a
   transform (identity for our content) and exposes named accessors; raw reads
   go `pub(crate)`. Switch `sync_usd_visuals` + the sim/avian translators to the
   named accessors. **Zero behavioural change today**, but the seam exists and
   is exercised.
2. **Round-trip test.** One synthetic Z-up/cm fixture proves the seam before any
   real non-conforming asset shows up. Add the grep/lint guard.
3. **Promote to `lunco-axes-and-units` crate + `CanonicalScene` trait** when the
   second convention actually arrives (live Blender, Isaac). That is when
   N-spokes pays off; building it now is speculative.
4. **Save path.** `from_canonical` on `SaveDocument` so files round-trip in
   their declared metrics.

Same end-state as the full hub-and-spoke design, built when it pays.

## Relationship to other specs

- [`009-coordinate-frame-tree`](../../specs/009-coordinate-frame-tree/spec.md) —
  defines the canonical f64 Link-Joint frame this layer targets.
- [`030-usd-scene-integration`](../../specs/030-usd-scene-integration/spec.md) —
  assumed upAxis handling + demands stiffness/damping parity; this layer
  delivers both.
- [`21-domain-usd.md`](21-domain-usd.md) — scene/stage ownership (Twin → active
  stage → Grid); the USD spoke runs at that document↔world boundary.
- [`40-asset-io.md`](40-asset-io.md) — asset I/O constraints and the wasm-safe I/O layer.

