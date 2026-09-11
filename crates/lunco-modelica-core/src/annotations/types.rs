//! Common geometric and enum types for Modelica annotations.

use serde::{Deserialize, Serialize};

/// 2D point in Modelica diagram coordinates (millimetres, Y-up).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Axis-aligned bounding box in Modelica diagram coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Extent {
    pub p1: Point,
    pub p2: Point,
}

/// RGB colour as 0..=255 components (matches Modelica `lineColor` / `fillColor` arrays).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// MLS Annex D `FillPattern` enumeration (subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FillPattern {
    #[default]
    None,
    Solid,
    Horizontal,
    Vertical,
    Cross,
    Forward,
    Backward,
    CrossDiag,
    HorizontalCylinder,
    VerticalCylinder,
    Sphere,
}

impl FillPattern {
    pub fn from_ident(ident: &str) -> Option<Self> {
        let tail = ident.rsplit('.').next().unwrap_or(ident);
        Some(match tail {
            "None" => Self::None,
            "Solid" => Self::Solid,
            "Horizontal" => Self::Horizontal,
            "Vertical" => Self::Vertical,
            "Cross" => Self::Cross,
            "Forward" => Self::Forward,
            "Backward" => Self::Backward,
            "CrossDiag" => Self::CrossDiag,
            "HorizontalCylinder" => Self::HorizontalCylinder,
            "VerticalCylinder" => Self::VerticalCylinder,
            "Sphere" => Self::Sphere,
            _ => return None,
        })
    }
}

/// MLS Annex D `LinePattern` enumeration (subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LinePattern {
    None,
    #[default]
    Solid,
    Dash,
    Dot,
    DashDot,
    DashDotDot,
}

impl LinePattern {
    pub fn from_ident(ident: &str) -> Option<Self> {
        let tail = ident.rsplit('.').next().unwrap_or(ident);
        Some(match tail {
            "None" => Self::None,
            "Solid" => Self::Solid,
            "Dash" => Self::Dash,
            "Dot" => Self::Dot,
            "DashDot" => Self::DashDot,
            "DashDotDot" => Self::DashDotDot,
            _ => return None,
        })
    }
}

/// MLS Annex D `Arrow` enum — line endcap style. Default `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Arrow {
    #[default]
    None,
    Open,
    Filled,
    Half,
}

impl Arrow {
    pub fn from_ident(s: &str) -> Option<Self> {
        let leaf = s.rsplit('.').next().unwrap_or(s);
        match leaf {
            "None" => Some(Self::None),
            "Open" => Some(Self::Open),
            "Filled" => Some(Self::Filled),
            "Half" => Some(Self::Half),
            _ => None,
        }
    }
}

/// MLS Annex D `EllipseClosure` enum — how a partial ellipse arc is
/// closed. Default `Chord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EllipseClosure {
    None,
    #[default]
    Chord,
    Radial,
}

impl EllipseClosure {
    pub fn from_ident(s: &str) -> Option<Self> {
        let leaf = s.rsplit('.').next().unwrap_or(s);
        match leaf {
            "None" => Some(Self::None),
            "Chord" => Some(Self::Chord),
            "Radial" => Some(Self::Radial),
            _ => None,
        }
    }
}

/// Properties common to filled shapes (rectangles, polygons).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct FilledShape {
    pub line_color: Option<Color>,
    pub fill_color: Option<Color>,
    pub line_pattern: LinePattern,
    pub fill_pattern: FillPattern,
    pub line_thickness: f64, // mm; defaults to 0.25 per MLS
}

/// Coordinate system for an Icon or Diagram layer.
///
/// Defaults to `extent={{-100,-100},{100,100}}` per MLS Annex D.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CoordinateSystem {
    pub extent: Extent,
}

impl Default for CoordinateSystem {
    fn default() -> Self {
        Self {
            extent: Extent {
                p1: Point {
                    x: -100.0,
                    y: -100.0,
                },
                p2: Point { x: 100.0, y: 100.0 },
            },
        }
    }
}
