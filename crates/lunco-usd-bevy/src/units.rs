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
//! Write-back runs the same map backwards: every read-side conversion has a
//! `stage_*` counterpart applying `S⁻¹ = (1/k)·Q⁻¹`
//! ([`ConventionTransform::stage_point`] and friends). An authoring command
//! converts canonical → stage frame immediately before it writes, so a value
//! read off a Z-up centimetre stage and written straight back is the value that
//! was already there.
//!
//! The inverse always exists and is exact in the same sense the forward map is:
//! `Q` is a rotation (USD admits only the identity or one ±90° axis swap) and
//! [`StageMetrics::from_reader`] rejects any `metersPerUnit` that is not finite
//! and positive, so `S` is a similarity and is never singular. There is no stage
//! whose metrics can make a save unrepresentable.

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
    ///   one-shot warning naming what is being converted, so an unexpected
    ///   Omniverse/Isaac stage is visible rather than merely handled.
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
                 to canonical Y-up SI metres at import, and back to the stage's own frame on \
                 write. See docs/architecture/41-axes-and-units.md.",
                metrics.up_axis,
                metrics.meters_per_unit
            );
        }
        metrics
    }

    /// The same metrics off an authoring [`Stage`](openusd::usd::Stage).
    ///
    /// The AUTHORING counterpart to [`from_reader`](Self::from_reader). The write
    /// path opens the document's layer as a transient stage to author through
    /// (`open_doc_stage`), so it reads the metadata from THAT stage rather than
    /// poking the flattened `sdf::Data` behind it — the same `stage_metadata` call
    /// `StageView` makes on the read side.
    ///
    /// That symmetry is the point: read and write must agree on the stage's frame
    /// or a round-trip is not the identity. Going through the composed stage on
    /// both sides means a layered `upAxis`/`metersPerUnit` opinion resolves the
    /// same way for both, which a per-layer `sdf::Data` read cannot promise.
    ///
    /// Silent on a non-canonical stage: `from_reader` warned already at import, and
    /// this runs once per edit.
    pub fn from_stage(stage: &openusd::usd::Stage) -> Self {
        let up_axis = match stage
            .stage_metadata("upAxis")
            .ok()
            .flatten()
            .and_then(|v| v.as_str().map(str::to_string))
            .as_deref()
        {
            Some("Z") => UpAxis::Z,
            _ => UpAxis::Y,
        };
        let meters_per_unit = stage
            .stage_metadata("metersPerUnit")
            .ok()
            .flatten()
            .and_then(|v| v.clone().get::<f64>().or_else(|| v.get::<f32>().map(f64::from)))
            .filter(|m| m.is_finite() && *m > 0.0)
            .unwrap_or(1.0);
        Self { meters_per_unit, up_axis }
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
    /// Convert a typed attribute value CANONICAL → the stage's frame, choosing the
    /// transform from the attribute's USD type ROLE.
    ///
    /// The role cannot be recovered from the value: `point3f`, `vector3f`,
    /// `normal3f` and `color3f` all decode to `Value::Vec3f`. USD keeps them
    /// distinct in the TYPE precisely because they transform differently — a point
    /// is affected by unit scale, a direction is not, and a colour is not spatial at
    /// all. So the type name is the input that matters here, not the payload.
    ///
    /// Anything whose role is not spatial is returned untouched. That deliberately
    /// includes bare `float`/`double` lengths (`radius`, `height`, extents): USD
    /// gives them no role, so there is nothing to dispatch on, and converting on a
    /// guessed attribute-name list would corrupt every same-named attribute that
    /// meant something else. A missed conversion misplaces a value; a wrong one
    /// destroys it.
    pub fn stage_value(&self, type_name: &str, v: openusd::sdf::Value) -> openusd::sdf::Value {
        self.map_by_role(type_name, v, true)
    }

    /// The inverse of [`stage_value`](Self::stage_value): a value read out of the
    /// layer, in the stage's frame, converted back to canonical.
    pub fn canonical_value(&self, type_name: &str, v: openusd::sdf::Value) -> openusd::sdf::Value {
        self.map_by_role(type_name, v, false)
    }

    fn map_by_role(&self, type_name: &str, v: openusd::sdf::Value, to_stage: bool) -> openusd::sdf::Value {
        use openusd::gf;
        use openusd::sdf::Value as V;

        // The identity short-circuit is not just a speed win: every branch below
        // round-trips through `Vec3`/`Quat`, and for f64 payloads that would narrow
        // to f32 and back. On a canonical stage — every asset we author — the value
        // must come through with its authored digits intact.
        if self.is_identity() {
            return v;
        }

        // `[]` is USD's array suffix; an array transforms element-wise, by the same
        // role as its element type.
        let base = type_name.strip_suffix("[]").unwrap_or(type_name);

        #[derive(Clone, Copy)]
        enum Role {
            Point,
            Direction,
            Orientation,
            None,
        }
        let role = match base {
            "point3f" | "point3d" | "point3h" => Role::Point,
            "vector3f" | "vector3d" | "vector3h" | "normal3f" | "normal3d" | "normal3h" => Role::Direction,
            "quatf" | "quatd" | "quath" => Role::Orientation,
            _ => Role::None,
        };

        let f = move |p: Vec3| match role {
            Role::Point => {
                if to_stage {
                    self.stage_point(p)
                } else {
                    self.point(p)
                }
            }
            Role::Direction => {
                if to_stage {
                    self.stage_dir(p)
                } else {
                    self.dir(p)
                }
            }
            _ => p,
        };
        let fd = move |p: DVec3| match role {
            Role::Point => {
                if to_stage {
                    self.stage_point_d(p)
                } else {
                    self.point_d(p)
                }
            }
            Role::Direction => {
                if to_stage {
                    self.stage_dir_d(p)
                } else {
                    self.dir_d(p)
                }
            }
            _ => p,
        };
        let fq = move |q: Quat| {
            if to_stage {
                self.stage_orient(q)
            } else {
                self.orient(q)
            }
        };

        match (role, v) {
            (Role::None, v) => v,
            (_, V::Vec3f(a)) => {
                let r = f(Vec3::new(a.x, a.y, a.z));
                V::Vec3f(gf::Vec3f { x: r.x, y: r.y, z: r.z })
            }
            (_, V::Vec3d(a)) => {
                let r = fd(DVec3::new(a.x, a.y, a.z));
                V::Vec3d(gf::Vec3d { x: r.x, y: r.y, z: r.z })
            }
            (_, V::Vec3fVec(xs)) => V::Vec3fVec(
                xs.into_iter()
                    .map(|a| {
                        let r = f(Vec3::new(a.x, a.y, a.z));
                        gf::Vec3f { x: r.x, y: r.y, z: r.z }
                    })
                    .collect(),
            ),
            (_, V::Vec3dVec(xs)) => V::Vec3dVec(
                xs.into_iter()
                    .map(|a| {
                        let r = fd(DVec3::new(a.x, a.y, a.z));
                        gf::Vec3d { x: r.x, y: r.y, z: r.z }
                    })
                    .collect(),
            ),
            (Role::Orientation, V::Quatf(q)) => {
                let r = fq(Quat::from_xyzw(q.x, q.y, q.z, q.w));
                V::Quatf(gf::Quatf { w: r.w, x: r.x, y: r.y, z: r.z })
            }
            (Role::Orientation, V::Quatd(q)) => {
                let r = fq(Quat::from_xyzw(q.x as f32, q.y as f32, q.z as f32, q.w as f32));
                V::Quatd(gf::Quatd { w: r.w as f64, x: r.x as f64, y: r.y as f64, z: r.z as f64 })
            }
            (_, other) => other,
        }
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

    // ---- Write-back: the inverse map `S⁻¹ = (1/k)·Q⁻¹` --------------------
    //
    // Authoring is the read path run backwards. Each `stage_*` below is the
    // exact inverse of the same-named forward conversion, so a value decoded
    // from the stage and re-authored unchanged reproduces the authored number —
    // the round-trip is the identity, not merely "close". Any command that
    // writes a spatial value onto a stage must pass it through the matching one;
    // writing a canonical value directly is how a Z-up centimetre stage ends up
    // 100× off and rotated, which is precisely what the reader corrects for.

    /// A canonical **position** → the stage's frame and units: `Q⁻¹·p / k`.
    /// Inverse of [`point`](Self::point).
    pub fn stage_point(&self, p: Vec3) -> Vec3 {
        self.rot.inverse() * (p / self.scale as f32)
    }

    /// A canonical **direction** → the stage's frame: `Q⁻¹·v`. Rotated, never
    /// scaled. Inverse of [`dir`](Self::dir).
    pub fn stage_dir(&self, v: Vec3) -> Vec3 {
        self.rot.inverse() * v
    }

    /// [`stage_point`](Self::stage_point) in `f64`, for the joint anchors the
    /// physics bridge keeps in [`DVec3`]. Same `f32`-rotation caveat as
    /// [`point_d`](Self::point_d).
    pub fn stage_point_d(&self, p: DVec3) -> DVec3 {
        self.rot.as_dquat().inverse() * (p / self.scale)
    }

    /// [`stage_dir`](Self::stage_dir) in `f64` — a joint's rotation axis.
    pub fn stage_dir_d(&self, v: DVec3) -> DVec3 {
        self.rot.as_dquat().inverse() * v
    }

    /// A canonical **geometry orientation** → the stage's frame: `Q⁻¹·q`.
    /// Inverse of [`orient`](Self::orient).
    pub fn stage_orient(&self, q: Quat) -> Quat {
        self.rot.inverse() * q
    }

    /// A **length** in metres → the stage's linear unit: `x / k`. Inverse of
    /// [`length`](Self::length) — a 2.5 m radius authored onto a centimetre
    /// stage is written as `250`.
    pub fn stage_length(&self, x: f64) -> f64 {
        x / self.scale
    }

    /// A canonical **local rotation** → the stage's basis: `Q⁻¹·q·Q`. Inverse of
    /// [`rotation`](Self::rotation), and separable per channel for the same
    /// reason.
    pub fn stage_rotation(&self, q: Quat) -> Quat {
        self.rot.inverse() * q * self.rot
    }

    /// A canonical **local scale** → the stage's axis order. Inverse of
    /// [`scale_vec`](Self::scale_vec); exact for the only two `Q` USD can
    /// produce.
    pub fn stage_scale_vec(&self, s: Vec3) -> Vec3 {
        let s = self.rot.inverse() * s;
        Vec3::new(s.x.abs(), s.y.abs(), s.z.abs())
    }

    /// De-conjugate a prim's **local** transform back to the stage's frame:
    /// `L = S⁻¹·L'·S`. Inverse of
    /// [`local_transform`](Self::local_transform) — this is what an authoring
    /// command hands to `xformOp:translate` / `:orient` / `:scale`.
    pub fn stage_local_transform(&self, t: Transform) -> Transform {
        if self.is_identity() {
            return t;
        }
        Transform {
            translation: self.stage_point(t.translation),
            rotation: self.stage_rotation(t.rotation),
            scale: self.stage_scale_vec(t.scale),
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

    /// The save-side contract: read a value off a non-canonical stage, author it
    /// straight back, and the stage holds what it held. Anything less and an
    /// edit to one prim silently rescales or rotates it on every save.
    #[test]
    fn write_back_round_trips_through_a_non_canonical_stage() {
        let ct = ConventionTransform::from_stage_metrics(&StageMetrics {
            meters_per_unit: 0.01,
            up_axis: UpAxis::Z,
        });

        let p = Vec3::new(123.0, -45.0, 6.75);
        assert!(ct.stage_point(ct.point(p)).abs_diff_eq(p, 1e-3), "point");
        assert!(ct.stage_dir(ct.dir(p)).abs_diff_eq(p, 1e-4), "dir");
        assert!((ct.stage_length(ct.length(250.0)) - 250.0).abs() < 1e-9, "length");

        let pd = DVec3::new(123.0, -45.0, 6.75);
        assert!(ct.stage_point_d(ct.point_d(pd)).abs_diff_eq(pd, 1e-3), "point_d");
        assert!(ct.stage_dir_d(ct.dir_d(pd)).abs_diff_eq(pd, 1e-4), "dir_d");

        let q = Quat::from_rotation_y(0.7) * Quat::from_rotation_x(-0.2);
        assert!(ct.stage_orient(ct.orient(q)).abs_diff_eq(q, 1e-6), "orient");
        assert!(ct.stage_rotation(ct.rotation(q)).abs_diff_eq(q, 1e-6), "rotation");

        let t = Transform::from_xyz(400.0, 10.0, -25.0)
            .with_rotation(q)
            .with_scale(Vec3::new(1.0, 2.0, 3.0));
        let back = ct.stage_local_transform(ct.local_transform(t));
        assert!(back.translation.abs_diff_eq(t.translation, 1e-3), "local translation");
        assert!(back.rotation.abs_diff_eq(t.rotation, 1e-6), "local rotation");
        assert!(back.scale.abs_diff_eq(t.scale, 1e-5), "local scale");
    }

    /// The write path must be a no-op on a canonical stage — every asset we ship
    /// takes it, so authoring must stay bit-for-bit what it was.
    #[test]
    fn canonical_stage_write_back_is_the_identity() {
        let ct = ConventionTransform::from_stage_metrics(&StageMetrics::default());
        let t = Transform::from_xyz(1.0, 2.0, 3.0).with_scale(Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(ct.stage_local_transform(t), t);
        assert_eq!(ct.stage_length(7.5), 7.5);
    }

    /// A centimetre stage's authored numbers, spelled out: 2.5 m is `250`, and a
    /// point 1 m up the canonical +Y is 100 units up the stage's +Z.
    #[test]
    fn write_back_uses_the_stages_own_units_and_axes() {
        let ct = ConventionTransform::from_stage_metrics(&StageMetrics {
            meters_per_unit: 0.01,
            up_axis: UpAxis::Z,
        });
        assert!((ct.stage_length(2.5) - 250.0).abs() < 1e-9);
        assert!(ct.stage_point(Vec3::Y).abs_diff_eq(Vec3::new(0.0, 0.0, 100.0), 1e-3));
    }
}
