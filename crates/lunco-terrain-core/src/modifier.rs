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
        let crater = Craters::new(vec![Crater { center: [0.0, 0.0], radius: 10.0, depth: 4.0, rim_height: 0.0, softness: 0.0 }]);
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
            Crater { center: [0.0, 0.0], radius: 10.0, depth: 2.0, rim_height: 0.4, softness: 0.0 },
            Crater { center: [15.0, -8.0], radius: 6.0, depth: 1.5, rim_height: 0.3, softness: 0.0 },
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
        let a = Craters::new(vec![Crater { center: [0.0, 0.0], radius: 10.0, depth: 2.0, rim_height: 0.0, softness: 0.0 }]);
        let b = Craters::new(vec![Crater { center: [0.0, 0.0], radius: 10.0, depth: 2.0, rim_height: 0.0, softness: 0.0 }]);
        let one = LayeredHeightSource::new(Arc::new(Flat(0.0))).with(Arc::new(a.clone()));
        let two = LayeredHeightSource::new(Arc::new(Flat(0.0))).with(Arc::new(a)).with(Arc::new(b));
        assert!((two.height_at(0.0, 0.0) - 2.0 * one.height_at(0.0, 0.0)).abs() < 1e-9);
    }

    #[test]
    fn deterministic() {
        let s = LayeredHeightSource::new(Arc::new(Flat(1.0)))
            .with(Arc::new(BrushModifier::new([2.0, 3.0], 8.0, -1.5)))
            .with(Arc::new(FlattenModifier::new([0.0, 0.0], 5.0, 0.0)));
        assert_eq!(s.height_at(1.0, 1.0), s.height_at(1.0, 1.0));
    }
}
