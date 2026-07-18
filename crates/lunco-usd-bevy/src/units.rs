//! Stage metrics → canonical frame: the **one** conversion seam for USD import.
//!
//! `docs/architecture/41-axes-and-units.md` mandates *"convert once, at the
//! importer"*: the engine runs in one fixed canonical frame (spec 009 — Y-up,
//! right-handed, SI metres) and every external representation is converted at
//! the boundary. USD **declares** its convention (`upAxis`, `metersPerUnit`)
//! and does **not** auto-convert — "I tell you the convention, you adapt".
//!
//! Until this module existed the importer adapted to nothing: an Omniverse /
//! Isaac Sim stage (`upAxis = "Z"`, `metersPerUnit = 0.01` — *their* defaults)
//! imported rotated 90° and 100× too small, silently.
//!
//! ## Where the conversion is applied
//!
//! Baked into the shared decoders, **not** onto a root entity — a root-only
//! rotation/scale is explicitly rejected by doc 41 (avian colliders, `big_space`
//! and the f64 frame tree all assume SI Y-up, so non-SI must never flow
//! downstream). Every consumer — `lunco-usd-bevy`'s visual sync, `lunco-usd-avian`'s
//! colliders, `lunco-sandbox-edit`'s gizmo — already funnels through these:
//!
//! | decoder | conversion |
//! |---|---|
//! | [`local_transform_at`](crate::local_transform_at) (→ `read_transform_from_usd`, the mount/footprint walks) | [`ConventionTransform::local_transform`] |
//! | [`read_shape_dims`](crate::read_shape_dims) | [`ConventionTransform::length`] on every dimension |
//! | [`build_usd_mesh`](crate::build_usd_mesh) / [`read_usd_mesh_indexed`](crate::read_usd_mesh_indexed) | [`ConventionTransform::point`] on points, [`dir`](ConventionTransform::dir) on normals |
//! | the `axis` token of a `Cylinder`/`Cone`/`Capsule`/`Plane` | [`ConventionTransform::orient`] |
//!
//! ## The maths
//!
//! The stage-to-canonical map is a **similarity** `S = k·Q` (uniform scale `k` =
//! `metersPerUnit`, rotation `Q` from the up-axis). For a prim chain
//! `W = L₁·L₂·…·Lₙ` acting on local geometry `p`, the canonical world position is
//! `S·W·p`. Rewriting:
//!
//! ```text
//! S·L₁·L₂·…·Lₙ·p  =  (S·L₁·S⁻¹)(S·L₂·S⁻¹)…(S·Lₙ·S⁻¹) · S·p
//! ```
//!
//! ⇒ **conjugate every local transform** (`Lᵢ' = S·Lᵢ·S⁻¹`,
//! [`local_transform`](ConventionTransform::local_transform)) **and convert the
//! leaf geometry** (`p' = S·p`, [`point`](ConventionTransform::point)). Both, not
//! either — which is why the decoders convert transforms *and* points/dims.
//!
//! ## Save
//!
//! Not yet inverted on write-back: authoring a transform onto a **non-canonical**
//! stage writes canonical values into a stage that declares other metrics.
//! [`StageMetrics::from_reader`] logs that once, loudly, when it sees such a
//! stage. Doc 41's staged plan puts `from_canonical`-on-save at step 4.

use std::f32::consts::FRAC_PI_2;

use bevy::log::{error_once, warn_once};
use bevy::math::{DVec3, Quat, Vec3};
use bevy::prelude::Transform;

use crate::read::UsdRead;

/// The stage's declared up axis. USD's default is `Y` (AOUSD); DCC/robotics
/// stages (Omniverse, Isaac Sim, Blender, ROS) overwhelmingly author `Z`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpAxis {
    /// `upAxis = "Y"` — the USD default, and our canonical frame ⇒ no rotation.
    #[default]
    Y,
    /// `upAxis = "Z"` — Omniverse / Isaac / Blender.
    Z,
}

/// A stage's declared metrics, as read from its pseudo-root metadata. The
/// **only** place these two tokens are interpreted (doc 41's "one choke point
/// per format").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StageMetrics {
    /// `metersPerUnit` — SI metres per authored linear unit. `1.0` = metres
    /// (USD default and ours), `0.01` = centimetres (Omniverse / Isaac / Unreal).
    pub meters_per_unit: f64,
    /// `upAxis` — `Y` (USD default, ours) or `Z`.
    pub up_axis: UpAxis,
}

impl Default for StageMetrics {
    /// The USD defaults, which are also our canonical frame: `upAxis = "Y"`,
    /// `metersPerUnit = 1.0`. An unauthored stage needs no conversion.
    fn default() -> Self {
        Self { meters_per_unit: 1.0, up_axis: UpAxis::Y }
    }
}

impl StageMetrics {
    /// Read the composed stage metrics from any [`UsdRead`] source.
    ///
    /// **Warns loudly** rather than importing wrong silently:
    /// - an `upAxis` token that is neither `Y` nor `Z` is an **error** (USD only
    ///   defines those two) → treated as `Y`;
    /// - a non-finite or non-positive `metersPerUnit` is an **error** → `1.0`;
    /// - a *supported but non-canonical* stage (Z-up and/or non-metre) logs a
    ///   one-shot warning naming what is being converted, plus the known save-side
    ///   gap (write-back is not yet inverted).
    pub fn from_reader<R: UsdRead>(reader: &R) -> Self {
        let up_axis = match reader.stage_up_axis().as_deref() {
            None | Some("Y") => UpAxis::Y,
            Some("Z") => UpAxis::Z,
            Some(other) => {
                error_once!(
                    "[usd-units] stage declares unsupported upAxis = {other:?} (USD defines only \
                     \"Y\" and \"Z\"); importing as Y-up — geometry may be rotated wrongly"
                );
                UpAxis::Y
            }
        };
        let meters_per_unit = match reader.stage_meters_per_unit() {
            None => 1.0,
            Some(m) if m.is_finite() && m > 0.0 => m,
            Some(bad) => {
                error_once!(
                    "[usd-units] stage declares unsupported metersPerUnit = {bad}; importing at \
                     1.0 (metres) — scene scale may be wrong"
                );
                1.0
            }
        };
        let metrics = Self { meters_per_unit, up_axis };
        if !metrics.is_canonical() {
            warn_once!(
                "[usd-units] non-canonical stage: upAxis = {:?}, metersPerUnit = {} — converting \
                 to canonical Y-up SI metres at import. NOTE: write-back (gizmo/authoring \
                 commands) is NOT yet inverted, so edits authored onto this stage will be written \
                 in canonical units. See docs/architecture/41-axes-and-units.md.",
                metrics.up_axis,
                metrics.meters_per_unit
            );
        }
        metrics
    }

    /// Whether the stage is already in the canonical frame (Y-up, metres) ⇒ the
    /// conversion is the identity and import is bit-for-bit what it was before
    /// this module existed. **True for every asset we ship.**
    pub fn is_canonical(&self) -> bool {
        self.up_axis == UpAxis::Y && (self.meters_per_unit - 1.0).abs() < 1e-12
    }
}

/// The precomputed stage → canonical similarity `S = k·Q`.
///
/// `Q` is a pure rotation (all our targets are right-handed, so an up-axis remap
/// is never a mirror) and `k` a uniform scale, so `S` preserves angles and
/// conjugation by it keeps a `Transform` a `Transform` (no shear).
#[derive(Debug, Clone, Copy, PartialEq)]
#[must_use]
pub struct ConventionTransform {
    /// Up-axis rotation: identity for Y-up, `Rx(-90°)` for Z-up (`(x,y,z) →
    /// (x, z, −y)`, the standard Z-up→Y-up remap).
    rot: Quat,
    /// `metersPerUnit`.
    scale: f64,
}

impl Default for ConventionTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl ConventionTransform {
    /// A canonical stage (Y-up, metres) needs no conversion.
    pub const IDENTITY: Self = Self { rot: Quat::IDENTITY, scale: 1.0 };

    /// The one construction point (doc 41: `from_stage_metrics`).
    pub fn from_stage_metrics(m: &StageMetrics) -> Self {
        let rot = match m.up_axis {
            UpAxis::Y => Quat::IDENTITY,
            // Z-up → Y-up: rotate −90° about X, i.e. (x, y, z) ↦ (x, z, −y).
            // The stage's +Z (its up) lands on canonical +Y (our up).
            UpAxis::Z => Quat::from_rotation_x(-FRAC_PI_2),
        };
        Self { rot, scale: m.meters_per_unit }
    }

    /// Whether this is the no-op conversion (the common case: our own assets).
    /// Hot decoders early-out on it, so a canonical stage pays nothing.
    pub fn is_identity(&self) -> bool {
        (self.scale - 1.0).abs() < 1e-12 && self.rot.abs_diff_eq(Quat::IDENTITY, 1e-9)
    }

    /// A **position** in stage-local coordinates → canonical: `k·Q·p`. Mesh
    /// points, look-at targets, any authored point.
    pub fn point(&self, p: Vec3) -> Vec3 {
        (self.rot * p) * self.scale as f32
    }

    /// A **direction** → canonical: `Q·v`. Rotated, never scaled — normals,
    /// axes, unit vectors.
    pub fn dir(&self, v: Vec3) -> Vec3 {
        self.rot * v
    }

    /// [`point`](Self::point) in `f64`, for the values the physics bridge keeps
    /// in [`DVec3`] — joint anchors (`physics:localPos0/1`). Routing those
    /// through the `f32` [`point`](Self::point) would discard exactly the
    /// precision `DVec3` exists to preserve, so the physics path gets its own
    /// arm rather than a round-trip.
    ///
    /// What this does and does not buy: the INPUT and the `metersPerUnit`
    /// multiply stay in `f64`, but [`rot`](Self::rot) is an `f32` [`Quat`], so
    /// the rotation itself still carries ~3e-8 of `f32` error — identical to
    /// every other consumer. Full `f64` would mean storing the up-axis rotation
    /// in `f64` too, which is not worth it for a value USD restricts to the
    /// identity or one ±90° axis swap.
    pub fn point_d(&self, p: DVec3) -> DVec3 {
        (self.rot.as_dquat() * p) * self.scale
    }

    /// [`dir`](Self::dir) in `f64` — a joint's rotation axis. Rotated, never
    /// scaled. Same `f32`-rotation caveat as [`point_d`](Self::point_d).
    pub fn dir_d(&self, v: DVec3) -> DVec3 {
        self.rot.as_dquat() * v
    }

    /// A **geometry orientation** authored in stage-local coordinates (e.g. the
    /// `axis` token of a `UsdGeomCylinder`) → canonical: `Q·q`. Left-multiplied,
    /// because the geometry is generated in the *canonical* local frame while the
    /// token names an axis of the *stage's* frame.
    pub fn orient(&self, q: Quat) -> Quat {
        self.rot * q
    }

    /// A **length** (radius, height, size, extent) → metres: `k·x`.
    pub fn length(&self, x: f64) -> f64 {
        x * self.scale
    }

    /// A prim's **local rotation** → canonical: `Q·q·Q⁻¹` (re-expressed in the
    /// canonical basis). The rotation half of
    /// [`local_transform`](Self::local_transform), split out for the animation
    /// sampler, which drives translate/rotate/scale channels independently —
    /// conjugation is separable across the three, so a per-channel conversion
    /// agrees exactly with the whole-transform one.
    pub fn rotation(&self, q: Quat) -> Quat {
        self.rot * q * self.rot.inverse()
    }

    /// A prim's **local scale** → canonical: the axes permute with `Q`. Exact for
    /// the only two `Q` USD can produce (identity, ±90° axis swap) — see
    /// [`local_transform`](Self::local_transform).
    pub fn scale_vec(&self, s: Vec3) -> Vec3 {
        let s = self.rot * s;
        Vec3::new(s.x.abs(), s.y.abs(), s.z.abs())
    }

    /// Conjugate a prim's **local** transform: `L' = S·L·S⁻¹`.
    ///
    /// Conjugation (not a plain left-multiply) is what lets each level of the
    /// hierarchy be converted independently — see the module docs. The rotation
    /// is re-expressed in the canonical basis, the translation is a point, and
    /// the (possibly non-uniform) scale's *axes permute* with `Q`: `Q` is either
    /// the identity or a ±90° axis swap, so `|Q·s|` componentwise is exact here
    /// (it would not be for an arbitrary rotation — none can occur: USD defines
    /// only `upAxis` `Y`/`Z`).
    pub fn local_transform(&self, t: Transform) -> Transform {
        if self.is_identity() {
            return t;
        }
        Transform {
            translation: self.point(t.translation),
            rotation: self.rotation(t.rotation),
            scale: self.scale_vec(t.scale),
        }
    }
}

/// The stage → canonical conversion for `reader`'s stage. Cheap (two metadata
/// field reads); the decoders call it per prim rather than threading a gate
/// through every signature.
pub fn stage_convention<R: UsdRead>(reader: &R) -> ConventionTransform {
    ConventionTransform::from_stage_metrics(&StageMetrics::from_reader(reader))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Z-up remap sends the stage's up (+Z) to canonical up (+Y), keeps X,
    /// and sends the stage's +Y (its "forward-ish" horizontal) to canonical −Z.
    #[test]
    fn z_up_remap_sends_stage_up_to_canonical_up() {
        let ct = ConventionTransform::from_stage_metrics(&StageMetrics {
            meters_per_unit: 1.0,
            up_axis: UpAxis::Z,
        });
        assert!(ct.dir(Vec3::Z).abs_diff_eq(Vec3::Y, 1e-6), "+Z (stage up) → +Y (canonical up)");
        assert!(ct.dir(Vec3::X).abs_diff_eq(Vec3::X, 1e-6), "X is the rotation axis — fixed");
        assert!(ct.dir(Vec3::Y).abs_diff_eq(-Vec3::Z, 1e-6), "+Y → −Z");
    }

    /// Centimetres scale points and lengths by 0.01 and rotate nothing.
    #[test]
    fn centimetre_stage_scales_only() {
        let ct = ConventionTransform::from_stage_metrics(&StageMetrics {
            meters_per_unit: 0.01,
            up_axis: UpAxis::Y,
        });
        assert!(ct.point(Vec3::new(100.0, 0.0, 0.0)).abs_diff_eq(Vec3::X, 1e-6));
        assert_eq!(ct.length(250.0), 2.5);
        assert!(ct.dir(Vec3::Y).abs_diff_eq(Vec3::Y, 1e-6), "a direction is never scaled");
    }

    /// The canonical stage is exactly the identity — every asset we ship takes
    /// this path, so import is unchanged.
    #[test]
    fn canonical_stage_is_identity() {
        let ct = ConventionTransform::from_stage_metrics(&StageMetrics::default());
        assert!(ct.is_identity());
        let t = Transform::from_xyz(1.0, 2.0, 3.0).with_scale(Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(ct.local_transform(t), t);
    }

    /// Conjugation is the property the hierarchy relies on: converting a
    /// two-level chain level-by-level and then the leaf point must equal
    /// converting the composed world point in one go (`S·W·p = L₁'·L₂'·S·p`).
    #[test]
    fn conjugated_chain_equals_converted_world_point() {
        let ct = ConventionTransform::from_stage_metrics(&StageMetrics {
            meters_per_unit: 0.01,
            up_axis: UpAxis::Z,
        });
        // Two authored levels + a local point, all in stage units (cm, Z-up).
        let l1 = Transform::from_xyz(100.0, 0.0, 50.0)
            .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4));
        let l2 = Transform::from_xyz(0.0, 200.0, 0.0)
            .with_rotation(Quat::from_rotation_x(0.3));
        let p = Vec3::new(10.0, -20.0, 30.0);

        // Raw USD world point, then converted once (ground truth).
        let world_usd = (l1 * l2).transform_point(p);
        let expected = ct.point(world_usd);

        // What the importer actually builds: conjugated locals over a converted
        // leaf point.
        let got = (ct.local_transform(l1) * ct.local_transform(l2)).transform_point(ct.point(p));

        assert!(
            got.abs_diff_eq(expected, 1e-4),
            "conjugated chain {got:?} must equal converted world point {expected:?}"
        );
    }
}
