//! Placement and transformation annotations.

use super::types::{Extent, Point};
use serde::{Deserialize, Serialize};

/// Decoded `Placement(transformation(...), [iconTransformation(...)])` annotation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub transformation: Transformation,
}

/// `transformation(extent=..., origin=..., rotation=...)` payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transformation {
    pub extent: Extent,
    /// Defaults to (0, 0) per MLS Annex D when not given.
    pub origin: Point,
    /// Degrees CCW. Defaults to 0.
    pub rotation: f64,
}

impl Transformation {
    /// Centre `(cx, cy)` and size `(w, h)` of this placement box in
    /// diagram coordinates, with the box `origin` offset applied to the
    /// centre. Order-independent in the extent corners (a flipped
    /// `extent` yields the same centre and a positive size). The single
    /// source of this extent→centre/size math (indexer port layout and
    /// `index::annotation_placement_to_pretty` both call it).
    pub fn centre_size(&self) -> (f64, f64, f64, f64) {
        let e = &self.extent;
        let x_min = e.p1.x.min(e.p2.x);
        let x_max = e.p1.x.max(e.p2.x);
        let y_min = e.p1.y.min(e.p2.y);
        let y_max = e.p1.y.max(e.p2.y);
        (
            (x_min + x_max) * 0.5 + self.origin.x,
            (y_min + y_max) * 0.5 + self.origin.y,
            x_max - x_min,
            y_max - y_min,
        )
    }
}
