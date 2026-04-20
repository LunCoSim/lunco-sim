//! Affine transform from a Modelica icon's local coords to canvas
//! world coords.
//!
//! ## Why one transform per node
//!
//! A `Placement(transformation(extent, origin, rotation))` is a single
//! 2D affine map from the icon's local coordinate system (typically
//! `-100..100` per axis, +Y up) to the parent diagram's coordinate
//! system. Earlier we hand-decomposed it into per-feature scalars
//! (`position`, `extent_size`, `rotation_degrees`, `mirror_x`,
//! `mirror_y`) and applied each piece in different code sites. Adding
//! any new feature (icon rotation visual, `iconTransformation` for
//! drilled-in views, scale per drill-in zoom) meant touching every
//! site and was easy to miss — we did miss it: ports rotated but the
//! icon body never did.
//!
//! [`IconTransform`] folds all of that — including the Modelica
//! `+Y up` → canvas `+Y down` flip — into a single 2×3 affine matrix
//! built once by the importer. Every consumer (port positioning,
//! edge-stub direction classifier, icon body painter, bounding-rect
//! computation) calls `apply` on the same matrix.
//!
//! ## Math
//!
//! A 2×3 matrix `[[a, c, tx], [b, d, ty]]` mapping
//!
//! ```text
//! world.x = a · local.x + c · local.y + tx
//! world.y = b · local.x + d · local.y + ty
//! ```
//!
//! Composition is right-to-left — `t1 * t2` applies `t2` first, then `t1`.

use serde::{Deserialize, Serialize};

/// Transform from a node's icon-local Modelica coords (+Y up) to the
/// canvas world coords (+Y down).
///
/// Built from [`Placement`](crate::annotations::Placement) by
/// [`IconTransform::from_placement`]. Identity is the no-op transform
/// (icon at origin, axis-aligned, +Y up — useful for tests and as a
/// fallback when no Placement is authored).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IconTransform {
    /// Row-major 2×3 affine: `[a, c, tx, b, d, ty]`. Stored flat so
    /// the JSON form is compact and we don't pay for nested arrays.
    pub m: [f32; 6],
    /// Rotation that went into building [`m`](Self::m), preserved
    /// separately because decomposing an arbitrary 2×2 back into
    /// rotation+mirror+scale is non-unique. Visual code (SVG
    /// renderer, `paint_graphics`) reads this directly when it needs
    /// to rotate/mirror its drawing primitives at the rect level
    /// instead of remapping every point through the full matrix.
    #[serde(default)]
    pub rotation_deg: f32,
    /// Mirror flags that went into building [`m`](Self::m). Same
    /// rationale as [`rotation_deg`](Self::rotation_deg).
    #[serde(default)]
    pub mirror_x: bool,
    #[serde(default)]
    pub mirror_y: bool,
}

impl IconTransform {
    /// Identity (no scaling, rotation, mirror, or translation, no Y flip).
    pub const IDENTITY: Self = Self {
        m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        rotation_deg: 0.0,
        mirror_x: false,
        mirror_y: false,
    };

    /// Build from raw matrix entries — use sparingly, prefer
    /// [`from_placement`](Self::from_placement) for the standard path.
    /// Rotation/mirror metadata defaults to zero/false; the caller
    /// must set them if they want the visual layer to rotate/mirror
    /// the icon body to match.
    pub const fn new(a: f32, b: f32, c: f32, d: f32, tx: f32, ty: f32) -> Self {
        Self {
            m: [a, c, tx, b, d, ty],
            rotation_deg: 0.0,
            mirror_x: false,
            mirror_y: false,
        }
    }

    /// Build the icon-local → canvas world transform from a parsed
    /// [`Placement`](crate::annotations::Placement)'s raw fields.
    ///
    /// Bakes in the Modelica `+Y up` → canvas `+Y down` Y flip so
    /// callers never need to remember the convention.
    ///
    /// Composition (right-to-left, since `(A * B) * v = A(B(v))`):
    /// 1. **Scale + mirror** — map icon-local `(-100..100)` to the
    ///    placement's extent, with negative scale along an axis if
    ///    the source reversed that extent corner.
    /// 2. **Rotate** by `rotation_deg` (CCW in Modelica's +Y-up frame).
    /// 3. **Translate** to the extent centre + origin (in Modelica
    ///    parent coords).
    /// 4. **Y flip** to canvas screen convention.
    ///
    /// We're explicit that `rotation` rotates around the icon-local
    /// origin (per MLS Annex D `origin` defaults to `{0,0}`) and the
    /// icon's own coord system extent is the standard
    /// `{{-100,-100},{100,100}}`. Non-default icon `coordinateSystem`
    /// extents are a follow-up (rare in practice).
    pub fn from_placement(
        extent_centre: (f32, f32),
        extent_size: (f32, f32),
        mirror_x: bool,
        mirror_y: bool,
        rotation_deg: f32,
        origin: (f32, f32),
    ) -> Self {
        // Per-axis scale from local Modelica icon (-100..100) to
        // placement extent. Local extent is 200 wide; the placement
        // extent is `extent_size`. Negative when mirrored.
        let sx = (if mirror_x { -1.0 } else { 1.0 }) * extent_size.0 / 200.0;
        let sy = (if mirror_y { -1.0 } else { 1.0 }) * extent_size.1 / 200.0;
        let cx = extent_centre.0 + origin.0;
        let cy = extent_centre.1 + origin.1;

        // Compose in Modelica's +Y-up frame: scale, rotate, translate.
        let theta = rotation_deg.to_radians();
        let (s, c) = theta.sin_cos();

        // R * S — rotation acts on the scaled local point.
        let a_my = c * sx;
        let b_my = s * sx;
        let c_my = -s * sy;
        let d_my = c * sy;
        let tx_my = cx;
        let ty_my = cy;

        // Apply Modelica → screen Y flip on the result. That flips the
        // sign of the second matrix row + translation.
        let mut t = Self::new(a_my, -b_my, c_my, -d_my, tx_my, -ty_my);
        t.rotation_deg = rotation_deg;
        t.mirror_x = mirror_x;
        t.mirror_y = mirror_y;
        t
    }

    /// Apply the transform to a local point — typical use is
    /// `icon_transform.apply(port.x, port.y)` where the port carries
    /// Modelica icon-local coords (-100..100).
    pub fn apply(&self, lx: f32, ly: f32) -> (f32, f32) {
        let m = &self.m;
        (
            m[0] * lx + m[1] * ly + m[2],
            m[3] * lx + m[4] * ly + m[5],
        )
    }

    /// Apply only the linear part (no translation) — used to map
    /// direction vectors (e.g. a port's outward normal to determine
    /// wire-stub direction).
    pub fn apply_dir(&self, lx: f32, ly: f32) -> (f32, f32) {
        let m = &self.m;
        (m[0] * lx + m[1] * ly, m[3] * lx + m[4] * ly)
    }

    /// Axis-aligned bounding rect of `(x_min, y_min)..(x_max, y_max)`
    /// in local coords, after applying the transform. Returns
    /// `(min, max)` in world coords. Used to size each node's canvas
    /// rect so the bounding box honours rotation (a 45°-rotated
    /// 100×40 icon needs more screen rect than 100×40).
    pub fn local_aabb(
        &self,
        x_min: f32,
        y_min: f32,
        x_max: f32,
        y_max: f32,
    ) -> ((f32, f32), (f32, f32)) {
        let corners = [
            self.apply(x_min, y_min),
            self.apply(x_max, y_min),
            self.apply(x_max, y_max),
            self.apply(x_min, y_max),
        ];
        let mut wx_min = corners[0].0;
        let mut wy_min = corners[0].1;
        let mut wx_max = wx_min;
        let mut wy_max = wy_min;
        for &(x, y) in &corners[1..] {
            wx_min = wx_min.min(x);
            wy_min = wy_min.min(y);
            wx_max = wx_max.max(x);
            wy_max = wy_max.max(y);
        }
        ((wx_min, wy_min), (wx_max, wy_max))
    }
}

impl Default for IconTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f32, f32), b: (f32, f32)) {
        assert!(
            (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3,
            "expected {:?}, got {:?}",
            b,
            a
        );
    }

    #[test]
    fn identity_does_not_move_local_points() {
        let t = IconTransform::IDENTITY;
        close(t.apply(10.0, 20.0), (10.0, 20.0));
    }

    #[test]
    fn flip_only_negates_y() {
        // No rotation, no mirror — extent (-10,-10)..(10,10) at origin.
        let t = IconTransform::from_placement(
            (0.0, 0.0),    // extent centre at parent origin
            (20.0, 20.0),  // extent size (in Modelica)
            false, false,
            0.0,
            (0.0, 0.0),
        );
        // Local (100, 0) → maps to extent's right edge (+10 in Modelica)
        // → screen (+10, 0) (Y flip is a no-op for y=0).
        close(t.apply(100.0, 0.0), (10.0, 0.0));
        // Local (0, 100) → top of icon in Modelica → +10 in Modelica y
        // → -10 in screen y (Y flip).
        close(t.apply(0.0, 100.0), (0.0, -10.0));
    }

    #[test]
    fn mirror_x_flips_left_right() {
        let t = IconTransform::from_placement(
            (0.0, 0.0),
            (20.0, 20.0),
            true, false,   // mirror X
            0.0,
            (0.0, 0.0),
        );
        // Local right edge (100, 0) → screen left edge (-10, 0).
        close(t.apply(100.0, 0.0), (-10.0, 0.0));
        close(t.apply(-100.0, 0.0), (10.0, 0.0));
    }

    #[test]
    fn rotation_90_swaps_axes_and_y_flips() {
        // Centre at (0,0), 20×20 extent, rotation=90° CCW (Modelica frame).
        let t = IconTransform::from_placement(
            (0.0, 0.0),
            (20.0, 20.0),
            false, false,
            90.0,
            (0.0, 0.0),
        );
        // Local (100, 0) → after 90° CCW rotation in Modelica frame,
        // points to (0, +10) Modelica → (0, -10) screen.
        close(t.apply(100.0, 0.0), (0.0, -10.0));
        // Local (0, 100) → (-10, 0) Modelica → (-10, 0) screen.
        close(t.apply(0.0, 100.0), (-10.0, 0.0));
    }

    #[test]
    fn extent_centre_offsets_world_origin() {
        // Centre at (50, 30), no rotation/mirror, 20×20 extent.
        let t = IconTransform::from_placement(
            (50.0, 30.0),
            (20.0, 20.0),
            false, false,
            0.0,
            (0.0, 0.0),
        );
        // Local origin → screen (50, -30) (Y flipped).
        close(t.apply(0.0, 0.0), (50.0, -30.0));
        // Local right edge → (60, -30).
        close(t.apply(100.0, 0.0), (60.0, -30.0));
    }

    #[test]
    fn origin_translates_after_rotation_centre() {
        // Integrator's `reset` connector: extent (-20,-20)..(20,20),
        // origin (60, -120), rotation 90.
        let t = IconTransform::from_placement(
            (0.0, 0.0),       // extent centre
            (40.0, 40.0),     // extent size
            false, false,
            90.0,
            (60.0, -120.0),
        );
        // Local (0,0) (the connector's anchor point) lands at
        // origin in Modelica parent: (60, -120) → screen (60, +120).
        close(t.apply(0.0, 0.0), (60.0, 120.0));
    }

    #[test]
    fn local_aabb_grows_under_rotation() {
        // 200×100 extent, 45° rotation — bounding box must be larger
        // than 200×100 because the diagonal is exposed.
        let t = IconTransform::from_placement(
            (0.0, 0.0),
            (200.0, 100.0),
            false, false,
            45.0,
            (0.0, 0.0),
        );
        let ((minx, miny), (maxx, maxy)) =
            t.local_aabb(-100.0, -100.0, 100.0, 100.0);
        let w = maxx - minx;
        let h = maxy - miny;
        // Rotated 200×100 → both dimensions ≈ (200+100)/√2 ≈ 212
        assert!((w - 212.13).abs() < 0.5, "w={}", w);
        assert!((h - 212.13).abs() < 0.5, "h={}", h);
    }

    #[test]
    fn apply_dir_ignores_translation() {
        let t = IconTransform::from_placement(
            (50.0, 30.0),
            (20.0, 20.0),
            false, false,
            0.0,
            (0.0, 0.0),
        );
        // Direction (1, 0) scales by 0.1 (extent_size 20 / icon 200),
        // Y flip → (0.1, 0) — no translation contribution.
        close(t.apply_dir(1.0, 0.0), (0.1, 0.0));
        close(t.apply_dir(0.0, 1.0), (0.0, -0.1));
    }
}
