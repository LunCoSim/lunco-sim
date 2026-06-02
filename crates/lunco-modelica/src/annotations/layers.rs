//! Icon and Diagram annotation layers.

use serde::{Deserialize, Serialize};
use super::types::{Extent, Point, CoordinateSystem};
use super::graphics::GraphicItem;

/// Decoded `Icon(coordinateSystem=..., graphics={...})` annotation.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Icon {
    pub coordinate_system: CoordinateSystem,
    pub graphics: Vec<GraphicItem>,
}

impl Icon {
    pub fn graphics_bbox(&self) -> Option<Extent> {
        let mut found = false;
        let mut xmin = f64::INFINITY;
        let mut ymin = f64::INFINITY;
        let mut xmax = f64::NEG_INFINITY;
        let mut ymax = f64::NEG_INFINITY;
        let mut merge = |x: f64, y: f64| {
            xmin = xmin.min(x);
            ymin = ymin.min(y);
            xmax = xmax.max(x);
            ymax = ymax.max(y);
            found = true;
        };
        for item in &self.graphics {
            match item {
                GraphicItem::Rectangle(r) => {
                    merge(r.extent.p1.x + r.origin.x, r.extent.p1.y + r.origin.y);
                    merge(r.extent.p2.x + r.origin.x, r.extent.p2.y + r.origin.y);
                }
                GraphicItem::Ellipse(e) => {
                    merge(e.extent.p1.x + e.origin.x, e.extent.p1.y + e.origin.y);
                    merge(e.extent.p2.x + e.origin.x, e.extent.p2.y + e.origin.y);
                }
                GraphicItem::Polygon(p) => {
                    for pt in &p.points {
                        merge(pt.x + p.origin.x, pt.y + p.origin.y);
                    }
                }
                GraphicItem::Line(l) => {
                    for pt in &l.points {
                        merge(pt.x + l.origin.x, pt.y + l.origin.y);
                    }
                }
                GraphicItem::Bitmap(b) => {
                    merge(b.extent.p1.x + b.origin.x, b.extent.p1.y + b.origin.y);
                    merge(b.extent.p2.x + b.origin.x, b.extent.p2.y + b.origin.y);
                }
                GraphicItem::Text(_) => continue,
            }
        }
        if found {
            Some(Extent {
                p1: Point { x: xmin, y: ymin },
                p2: Point { x: xmax, y: ymax },
            })
        } else {
            None
        }
    }

    pub fn full_bbox(&self) -> Option<Extent> {
        let mut xmin = f64::INFINITY;
        let mut ymin = f64::INFINITY;
        let mut xmax = f64::NEG_INFINITY;
        let mut ymax = f64::NEG_INFINITY;
        let mut found = false;
        let mut merge = |x: f64, y: f64| {
            xmin = xmin.min(x);
            ymin = ymin.min(y);
            xmax = xmax.max(x);
            ymax = ymax.max(y);
            found = true;
        };
        for item in &self.graphics {
            match item {
                GraphicItem::Rectangle(r) => {
                    merge(r.extent.p1.x + r.origin.x, r.extent.p1.y + r.origin.y);
                    merge(r.extent.p2.x + r.origin.x, r.extent.p2.y + r.origin.y);
                }
                GraphicItem::Ellipse(e) => {
                    merge(e.extent.p1.x + e.origin.x, e.extent.p1.y + e.origin.y);
                    merge(e.extent.p2.x + e.origin.x, e.extent.p2.y + e.origin.y);
                }
                GraphicItem::Polygon(p) => {
                    for pt in &p.points {
                        merge(pt.x + p.origin.x, pt.y + p.origin.y);
                    }
                }
                GraphicItem::Line(l) => {
                    for pt in &l.points {
                        merge(pt.x + l.origin.x, pt.y + l.origin.y);
                    }
                }
                GraphicItem::Bitmap(b) => {
                    merge(b.extent.p1.x + b.origin.x, b.extent.p1.y + b.origin.y);
                    merge(b.extent.p2.x + b.origin.x, b.extent.p2.y + b.origin.y);
                }
                GraphicItem::Text(t) => {
                    merge(t.extent.p1.x + t.origin.x, t.extent.p1.y + t.origin.y);
                    merge(t.extent.p2.x + t.origin.x, t.extent.p2.y + t.origin.y);
                }
            }
        }
        if found {
            Some(Extent {
                p1: Point { x: xmin, y: ymin },
                p2: Point { x: xmax, y: ymax },
            })
        } else {
            None
        }
    }
}

/// Decoded `Diagram(coordinateSystem=..., graphics={...})` annotation.
///
/// This maps *only* the standard Modelica `Diagram` annotation — the
/// `graphics` OMEdit (and every standards-compliant editor) renders.
/// LunCo's live plot tiles are deliberately NOT modelled here: they
/// live in the orthogonal `__LunCo(plotNodes={...})` vendor annotation
/// and are extracted independently via
/// [`super::parsing::extract_lunco_plot_nodes`]. Keeping them separate
/// means a pure behaviour model (no `Diagram` block at all) can still
/// carry plot tiles — bundling them here made plot extraction depend
/// on a `Diagram` block existing, which silently dropped the tiles.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Diagram {
    pub coordinate_system: CoordinateSystem,
    pub graphics: Vec<GraphicItem>,
}

/// Decoded `experiment(StartTime=..., StopTime=..., Tolerance=..., Interval=...)`
/// class annotation. The mere presence of this annotation is the
/// strongest possible signal that a class is meant to be a simulation
/// root — Dymola/OMEdit treat such classes as the obvious target.
/// All numeric fields are optional because authors commonly set
/// only `StopTime`. Pre-fill of Fast Run's start/stop fields uses
/// these values when available.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Experiment {
    pub start_time: Option<f64>,
    pub stop_time: Option<f64>,
    pub tolerance: Option<f64>,
    pub interval: Option<f64>,
}
