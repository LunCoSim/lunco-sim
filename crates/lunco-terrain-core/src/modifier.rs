//! The height-modifier stack — the ordered, runtime-dynamic composition of terrain
//! layers and edits.
//!
//! Static nesting (`CraterField<Dem>`) composes modifiers at compile time, but a
//! *dynamic* terrain — several crater layers, a dug pit, a flattened landing pad,
//! all added and removed at runtime from USD prims or a tool — needs a **runtime
//! list of heterogeneous modifiers folded over a base**. That is
//! [`LayeredHeightSource`]: `base` then each [`HeightModifier`] in order, so the
//! composed surface is `Edit_n ∘ … ∘ Craters ∘ Dem`.
//!
//! A [`HeightModifier`] takes the height accumulated so far and returns the new
//! height — `apply(x, z, h_in) -> h_out`. That single shape expresses **additive**
//! edits (a crater or a dig brush adds a delta) *and* **replacing** edits (a flatten
//! pulls the surface toward a target plane and must see the current height to blend
//! from it). Order matters: flatten-then-crater ≠ crater-then-flatten, exactly as USD
//! prim order defines the fold. Everything stays a pure function of its parts, so the
//! stack is deterministic, content-addressable, and peer-identical — the whole point.
//!
//! This is the substrate the dynamic-edit tools, per-layer identity, and the
//! schema-derived inspector attach to: adding a layer or a tool edit is "append a
//! modifier"; addressing one (edit / remove / reorder) is by its layer identity.

use std::sync::Arc;

use crate::source::HeightSource;

/// A layer that transforms the terrain height at a point. `apply` receives the height
/// produced by the base plus every lower modifier and returns the modified height, so
/// it can add to it (crater, brush) or pull it toward a target (flatten). Must be a
/// pure, deterministic function of position + input (same everywhere, every run).
pub trait HeightModifier: Send + Sync {
    /// Modified height (metres) at world `(x, z)` given the accumulated `h_in`.
    fn apply(&self, x: f64, z: f64, h_in: f64) -> f64;

    /// For **detail-synthesising** modifiers (procedural over-zoom): a variant of
    /// this modifier Nyquist-gated for a consumer sampling at `min_wavelength`
    /// metres — features below that scale fade out instead of aliasing, and the
    /// synthesis cost drops with them. Default `None`: the modifier is
    /// resolution-independent (craters, brushes, flattens) and is used as-is.
    fn with_min_wavelength(&self, _min_wavelength: f64) -> Option<Arc<dyn HeightModifier>> {
        None
    }
}

/// A base [`HeightSource`] plus an ordered stack of [`HeightModifier`]s folded over
/// it — the runtime-dynamic, multi-layer terrain surface. Append / remove / reorder
/// modifiers to add craters, dig pits, or flatten pads; the composed source is always
/// the current truth, so "modify" and "describe" are the same operation.
#[derive(Clone)]
pub struct LayeredHeightSource {
    /// The surface everything folds over (DEM, globe, analytic).
    pub base: Arc<dyn HeightSource>,
    /// Modifiers applied in order — index 0 first, last on top.
    pub modifiers: Vec<Arc<dyn HeightModifier>>,
}

impl LayeredHeightSource {
    /// A stack over `base` with no modifiers (samples `base` directly).
    pub fn new(base: Arc<dyn HeightSource>) -> Self {
        Self { base, modifiers: Vec::new() }
    }

    /// Builder: append a modifier and return self.
    pub fn with(mut self, m: Arc<dyn HeightModifier>) -> Self {
        self.modifiers.push(m);
        self
    }

    /// Append a modifier on top of the stack.
    pub fn push(&mut self, m: Arc<dyn HeightModifier>) {
        self.modifiers.push(m);
    }

    /// Number of modifiers in the stack.
    pub fn len(&self) -> usize {
        self.modifiers.len()
    }

    /// Whether the stack has no modifiers.
    pub fn is_empty(&self) -> bool {
        self.modifiers.is_empty()
    }
}

impl HeightSource for LayeredHeightSource {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        let mut h = self.base.height_at(x, z);
        for m in &self.modifiers {
            h = m.apply(x, z, h);
        }
        h
    }
}

/// A radial **brush** edit: adds `amplitude` metres at the centre, smoothly falling to
/// zero at `radius`. Positive amplitude raises a berm, negative digs a pit. The
/// generic dig/raise tool — `CraterField` is the parametric-profile cousin; this is
/// the free-placement brush a modelling tool drops.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushModifier {
    /// Centre `[x, z]` (metres).
    pub center: [f64; 2],
    /// Falloff radius (metres): contribution is zero at and beyond this.
    pub radius: f64,
    /// Peak height change at the centre (metres). `+` raises, `−` digs.
    pub amplitude: f64,
}

impl BrushModifier {
    pub fn new(center: [f64; 2], radius: f64, amplitude: f64) -> Self {
        Self { center, radius, amplitude }
    }

    /// The brush's additive delta (metres) at `(x, z)` — smooth bump, zero outside.
    #[inline]
    pub fn delta_at(&self, x: f64, z: f64) -> f64 {
        if self.radius <= 0.0 {
            return 0.0;
        }
        let d = (dist2(self.center, [x, z]).sqrt()) / self.radius;
        if d >= 1.0 {
            return 0.0;
        }
        // 1 at centre → 0 at edge, C1-smooth (so the collider slope stays sane).
        self.amplitude * (1.0 - smoothstep(d))
    }
}

impl HeightModifier for BrushModifier {
    fn apply(&self, x: f64, z: f64, h_in: f64) -> f64 {
        h_in + self.delta_at(x, z)
    }
}

/// A **flatten** edit: pulls the surface toward `target_y` inside `radius`, blending
/// back to the existing terrain at the edge. This is the modifier that *needs* the
/// incoming height — it replaces rather than adds — which is why [`HeightModifier`]
/// threads `h_in`. The "level a landing pad" tool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlattenModifier {
    /// Centre `[x, z]` (metres).
    pub center: [f64; 2],
    /// Radius (metres): full flatten at the centre, no effect at and beyond this.
    pub radius: f64,
    /// The height (metres) to flatten toward.
    pub target_y: f64,
}

impl FlattenModifier {
    pub fn new(center: [f64; 2], radius: f64, target_y: f64) -> Self {
        Self { center, radius, target_y }
    }

    /// Blend weight toward `target_y` at `(x, z)`: 1 at centre, 0 at the edge.
    #[inline]
    pub fn weight_at(&self, x: f64, z: f64) -> f64 {
        if self.radius <= 0.0 {
            return 0.0;
        }
        let d = (dist2(self.center, [x, z]).sqrt()) / self.radius;
        if d >= 1.0 {
            return 0.0;
        }
        1.0 - smoothstep(d)
    }
}

impl HeightModifier for FlattenModifier {
    fn apply(&self, x: f64, z: f64, h_in: f64) -> f64 {
        let w = self.weight_at(x, z);
        h_in + (self.target_y - h_in) * w // lerp(h_in, target_y, w)
    }
}

/// **Body curvature**: curves a tangent-plane DEM patch down onto its parent
/// body's sphere and feathers the outer edge to land ON it.
///
/// A site-anchored DEM is authored in the local tangent frame (`y = 0` is the
/// plane tangent to the sphere at the site point), while the celestial globe
/// tiles ride the true sphere — so a flat patch floats above the globe by the
/// sagitta `R − √(R² − d²)` (≈ 37 m at an 8 km patch edge on the Moon) and
/// visibly doubles over it. This modifier, folded LAST over the composed
/// surface, makes every oracle consumer (tile meshes, colliders, shadow
/// heightfield, height queries) agree with the sphere:
///
/// * subtracts the sagitta at the true radial distance (curvature IS radial);
/// * feathers the composed RELIEF — the departure from `datum_m`, the site's own
///   ground elevation — to zero over the crop's outer FRAME, so at the DEM's
///   boundary the terrain sits at `datum_m` + `edge_lift_m` (a small lift so the
///   last row never z-fights globe tiles);
/// * outside the DEM the surface is fully feathered — a sphere-hugging apron at
///   the site's own elevation.
///
/// **The feather is square, because the DEM is square.** It used to run on the
/// radial distance, which put the whole feather band inside the crop and threw
/// away its corners: 21% of every DEM ever baked was ramped to the apron despite
/// having real measured heights, and `d > half_extent` — every corner beyond the
/// inscribed circle — was discarded outright. A scene authored where its own
/// survey said the ground was (the Summer Space School rover starts in the NW
/// corner of its 1 km crop) was standing on invented apron, not on Apollo 15.
/// Feathering on the Chebyshev distance `max(|x|, |z|)` matches the footprint the
/// data actually has, so every sample inside the crop keeps its measured height.
///
/// **`datum_m` is what makes this safe on an absolute-datum DEM.** LROC/LOLA
/// products state elevations against the body datum, and the pipeline never
/// rebases them, so a real site reads −1918 m (Hadley) or +1946 m (the moonbase
/// ridge) at *every* point — that offset is the site's elevation, not relief.
/// Feathering `h_in` itself toward zero therefore ramped the whole crop back to
/// the datum plane across the band: at Hadley the ground climbed 1.9 km between
/// `d = 300 m` and the crop edge, the scene sat at the bottom of a pit, and
/// anything authored at its real elevation outside `0.6 · half_extent` (the
/// rover, the base mast) spawned under the apron with no collider beneath it and
/// fell forever. Feathering `h_in − datum_m` keeps the interior identical and
/// lands the apron on the plain the crop was cut from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyCurvature {
    /// Body mean radius (metres).
    pub radius_m: f64,
    /// DEM half side length (metres) — feathering completes at this distance
    /// from the site origin along the dominant axis (the crop's edge).
    pub half_extent_m: f64,
    /// The site's ground elevation (metres, body datum) — the height the apron
    /// feathers to, and the sphere the sagitta is measured on. Zero for a DEM
    /// already expressed as height above the datum plane.
    pub datum_m: f64,
    /// Height above the apron at (and beyond) the feathered edge (metres).
    pub edge_lift_m: f64,
    /// Radial fraction of `half_extent_m` where the edge feather begins.
    pub feather_from: f64,
}

impl BodyCurvature {
    pub fn new(radius_m: f64, half_extent_m: f64, datum_m: f64) -> Self {
        // feather_from 0.85: what feathers is RELIEF ABOVE THE DATUM — metres,
        // not the site's elevation — so the band only has to be wide enough to
        // blend local relief, and every metre it spends is a metre of measured
        // DEM overwritten with apron. It was 0.6 back when the feather ran to
        // zero and had a kilometre of datum to swallow.
        Self { radius_m, half_extent_m, datum_m, edge_lift_m: 1.0, feather_from: 0.85 }
    }
}

impl HeightModifier for BodyCurvature {
    fn apply(&self, x: f64, z: f64, h_in: f64) -> f64 {
        // The ground rides the sphere at the SITE's radius, not the mean one —
        // a 1.9 km datum offset changes the sagitta by ~0.1% of itself, but it
        // keeps the apron on the surface the DEM actually describes.
        let r = self.radius_m + self.datum_m;
        // Sphere height below the tangent plane at RADIAL distance d (exact, not
        // the d²/2R approximation — free in f64). Curvature is radial even though
        // the feather below is square: they are different questions.
        let sag = (r * r - (x * x + z * z)).max(0.0).sqrt() - r;
        let edge = x.abs().max(z.abs()); // Chebyshev — the crop is a square
        let start = self.half_extent_m * self.feather_from;
        let band = (self.half_extent_m - start).max(1e-6);
        let f = 1.0 - smoothstep((edge - start) / band); // 1 interior → 0 at edge
        self.datum_m + sag + (h_in - self.datum_m) * f + self.edge_lift_m * (1.0 - f)
    }
    // No `with_min_wavelength` override: planet-scale wavelength, never aliases.
}

#[inline]
fn dist2(a: [f64; 2], b: [f64; 2]) -> f64 {
    let dx = a[0] - b[0];
    let dz = a[1] - b[1];
    dx * dx + dz * dz
}

#[inline]
fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crater::{Crater, CraterField, Craters};

    /// Constant base so modifier contributions read directly.
    struct Flat(f64);
    impl HeightSource for Flat {
        fn height_at(&self, _x: f64, _z: f64) -> f64 {
            self.0
        }
    }

    #[test]
    fn empty_stack_is_base() {
        let s = LayeredHeightSource::new(Arc::new(Flat(7.0)));
        assert!(s.is_empty());
        assert_eq!(s.height_at(1.0, 2.0), 7.0);
    }

    #[test]
    fn brush_raises_and_digs_locally() {
        let raise = BrushModifier::new([0.0, 0.0], 10.0, 5.0);
        let dig = BrushModifier::new([0.0, 0.0], 10.0, -5.0);
        assert!((raise.delta_at(0.0, 0.0) - 5.0).abs() < 1e-12, "peak at centre");
        assert_eq!(raise.delta_at(20.0, 0.0), 0.0, "zero outside radius");
        let up = LayeredHeightSource::new(Arc::new(Flat(0.0))).with(Arc::new(raise));
        let down = LayeredHeightSource::new(Arc::new(Flat(0.0))).with(Arc::new(dig));
        assert!(up.height_at(0.0, 0.0) > 4.0);
        assert!(down.height_at(0.0, 0.0) < -4.0);
        assert_eq!(up.height_at(100.0, 0.0), 0.0, "far field untouched");
    }

    #[test]
    fn flatten_reaches_target_and_blends_out() {
        // A sloped base; flatten a pad toward y = 3.
        struct Ramp;
        impl HeightSource for Ramp {
            fn height_at(&self, x: f64, _z: f64) -> f64 {
                x * 0.5
            }
        }
        let flat = FlattenModifier::new([0.0, 0.0], 20.0, 3.0);
        let s = LayeredHeightSource::new(Arc::new(Ramp)).with(Arc::new(flat));
        assert!((s.height_at(0.0, 0.0) - 3.0).abs() < 1e-9, "centre pulled to target");
        assert!((s.height_at(200.0, 0.0) - 100.0).abs() < 1e-9, "far field = raw ramp");
    }

    #[test]
    fn fold_order_matters() {
        // Crater (adds −depth) then flatten (pull to 0) ≠ flatten then crater.
        let crater = Craters::new(vec![Crater { center: [0.0, 0.0], radius: 10.0, depth: 4.0, rim_height: 0.0, softness: 0.0, bowl_power: 4.0 }]);
        let flat = FlattenModifier::new([0.0, 0.0], 30.0, 0.0);
        let crater_then_flat = LayeredHeightSource::new(Arc::new(Flat(0.0)))
            .with(Arc::new(crater.clone()))
            .with(Arc::new(flat));
        let flat_then_crater = LayeredHeightSource::new(Arc::new(Flat(0.0)))
            .with(Arc::new(flat))
            .with(Arc::new(crater));
        // Flatten-last wipes the crater at the centre (pulled to 0); crater-last keeps it.
        assert!((crater_then_flat.height_at(0.0, 0.0)).abs() < 1e-9, "flatten last → level");
        assert!(flat_then_crater.height_at(0.0, 0.0) < -3.0, "crater last → still a bowl");
    }

    #[test]
    fn craters_modifier_matches_crater_field() {
        // The extracted `Craters` modifier over a base equals the `CraterField` wrapper.
        let list = vec![
            Crater { center: [0.0, 0.0], radius: 10.0, depth: 2.0, rim_height: 0.4, softness: 0.0, bowl_power: 4.0 },
            Crater { center: [15.0, -8.0], radius: 6.0, depth: 1.5, rim_height: 0.3, softness: 0.0, bowl_power: 4.0 },
        ];
        let field = CraterField::new(Flat(5.0), list.clone());
        let stack = LayeredHeightSource::new(Arc::new(Flat(5.0))).with(Arc::new(Craters::new(list)));
        for gx in -30..30 {
            for gz in -30..30 {
                let (x, z) = (gx as f64 * 1.3, gz as f64 * 1.3);
                assert!((field.height_at(x, z) - stack.height_at(x, z)).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn several_crater_layers_accumulate() {
        // Two crater modifiers (two layers) stack — deltas add.
        let a = Craters::new(vec![Crater { center: [0.0, 0.0], radius: 10.0, depth: 2.0, rim_height: 0.0, softness: 0.0, bowl_power: 4.0 }]);
        let b = Craters::new(vec![Crater { center: [0.0, 0.0], radius: 10.0, depth: 2.0, rim_height: 0.0, softness: 0.0, bowl_power: 4.0 }]);
        let one = LayeredHeightSource::new(Arc::new(Flat(0.0))).with(Arc::new(a.clone()));
        let two = LayeredHeightSource::new(Arc::new(Flat(0.0))).with(Arc::new(a)).with(Arc::new(b));
        assert!((two.height_at(0.0, 0.0) - 2.0 * one.height_at(0.0, 0.0)).abs() < 1e-9);
    }

    #[test]
    fn body_curvature_hugs_sphere_and_feathers_edge() {
        let (r, he) = (1.737e6, 8000.0);
        let c = BodyCurvature::new(r, he, 0.0);
        // Site centre: untouched (full relief, zero sagitta).
        assert!((c.apply(0.0, 0.0, 120.0) - 120.0).abs() < 1e-9);
        // Interior (inside the feather start): relief kept, sagitta subtracted.
        let d = 4000.0;
        let sag = (r * r - d * d).sqrt() - r;
        assert!(sag < -4.0, "sagitta at 4 km must be metres-scale, got {sag}");
        assert!((c.apply(d, 0.0, 50.0) - (50.0 + sag)).abs() < 1e-6);
        // Feathered edge: lands at sphere + edge_lift regardless of relief —
        // this is the invariant that meets the globe tiles.
        let sag_e = (r * r - he * he).sqrt() - r;
        assert!((c.apply(he, 0.0, 300.0) - (sag_e + c.edge_lift_m)).abs() < 1e-6);
        assert!((c.apply(0.0, -he, -300.0) - (sag_e + c.edge_lift_m)).abs() < 1e-6);
    }

    /// The DEM is a square, so its corners carry real measured heights and must
    /// survive. A radial feather ramped everything past the inscribed circle to
    /// the apron — 21% of every crop, including the corner the Summer Space
    /// School twin starts its rover in.
    #[test]
    fn body_curvature_feathers_on_the_square_not_the_inscribed_circle() {
        let (r, he) = (1.737e6, 500.0);
        let c = BodyCurvature::new(r, he, 0.0);
        // A corner sample well inside the crop on BOTH axes (Chebyshev 380) but
        // outside the inscribed circle (radial 537): fully interior, relief kept.
        let sag = (r * r - (380.0f64 * 380.0 + 380.0 * 380.0)).sqrt() - r;
        assert!(
            (c.apply(-380.0, -380.0, 42.0) - (42.0 + sag)).abs() < 1e-6,
            "a corner inside the square keeps its measured height"
        );
        // The frame still feathers: at the crop edge, relief is gone.
        assert!((c.apply(-he, -he, 42.0) - c.apply(-he, -he, -99.0)).abs() < 1e-9);
    }

    /// An absolute-datum DEM (Hadley reads ≈ −1918 m at every point) must keep
    /// its interior EXACTLY and land its apron on the same plain — not ramp
    /// 1.9 km back up to the datum plane across the feather band, which put the
    /// rover's authored start under the apron with nothing to stand on.
    #[test]
    fn body_curvature_feathers_to_the_site_datum_not_to_zero() {
        let (r, he, datum) = (1.737e6, 498.0, -1918.0);
        let c = BodyCurvature::new(r, he, datum);
        // Interior: relief preserved (sagitta at this scale is sub-millimetre).
        assert!((c.apply(0.0, 0.0, -1947.6) - -1947.6).abs() < 1e-6);
        // Outside the crop entirely: the apron. It must land on the site's own
        // plain — before this it read ≈ +0.9 m, i.e. 1.9 km above the ground the
        // scene is authored on, with nothing beneath the rover to stand on.
        let apron = c.apply(-700.0, -700.0, -1917.8);
        assert!(
            (apron - (datum + c.edge_lift_m)).abs() < 1.0,
            "apron must land on the site datum, got {apron}"
        );
        // And the relief must be gone out there — the apron is flat, whatever
        // the (extrapolated) input relief claims.
        assert!((c.apply(-700.0, -700.0, -1800.0) - apron).abs() < 1e-9);
    }

    #[test]
    fn deterministic() {
        let s = LayeredHeightSource::new(Arc::new(Flat(1.0)))
            .with(Arc::new(BrushModifier::new([2.0, 3.0], 8.0, -1.5)))
            .with(Arc::new(FlattenModifier::new([0.0, 0.0], 5.0, 0.0)));
        assert_eq!(s.height_at(1.0, 1.0), s.height_at(1.0, 1.0));
    }
}
