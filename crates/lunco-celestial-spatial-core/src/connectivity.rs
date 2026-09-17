//! Render-independent connectivity facts shared by spatial consumers.
//!
//! The celestial runtime computes these facts, while renderers, exposure
//! adapters, USD projection, and cosimulation consume them. Keeping the
//! components here prevents those consumers from depending on the runtime
//! solver merely to name its published state.

use bevy::math::{DVec3, Vec3};
use bevy::prelude::{Component, Reflect, ReflectComponent};

/// A generic connectivity endpoint authored on a scene entity.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct LinkNode {
    /// Maximum geometric range in metres.
    pub max_range_m: f64,
    /// Minimum endpoint elevation in degrees.
    pub min_elevation_deg: f64,
    /// Optional authored role used by routing policy.
    pub class: Option<String>,
}

impl Default for LinkNode {
    fn default() -> Self {
        Self {
            max_range_m: 1.0e12,
            min_elevation_deg: -90.0,
            class: None,
        }
    }
}

/// An authored sight-line blocker represented by a local-space USD extent.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct LinkOccluder {
    /// Half-size of the local-space extent before the prim's scale.
    pub half_extents: DVec3,
    /// Centre of the local-space extent before the prim's scale.
    pub center: DVec3,
}

impl Default for LinkOccluder {
    fn default() -> Self {
        Self {
            half_extents: DVec3::splat(0.5),
            center: DVec3::ZERO,
        }
    }
}

impl LinkOccluder {
    /// Return the extent centre and half-size after applying a local scale.
    pub fn box_for(&self, scale: Vec3) -> (DVec3, DVec3) {
        let scale = scale.as_dvec3();
        (self.center * scale, (self.half_extents * scale).abs())
    }
}

/// Resolved link peers published by the connectivity runtime.
#[derive(Component, Debug, Clone, Default, Reflect)]
#[reflect(Component)]
pub struct LinkState {
    /// Peers resolved for this endpoint at the last connectivity pass.
    pub peers: Vec<LinkPeer>,
}

/// A policy-free pairwise geometry observation.
#[derive(Component, Debug, Clone, Default, Reflect)]
#[reflect(Component)]
pub struct LinkGeometryState {
    /// Geometry observations for the endpoint's peers.
    pub peers: Vec<LinkGeometryPeer>,
}

/// The geometric result for one potential peer before routing policy.
#[derive(Debug, Clone, Reflect)]
pub struct LinkGeometryPeer {
    /// Stable global id of the potential peer.
    pub peer: u64,
    /// Whether range, mask, and occlusion geometry permits the link.
    pub builtin: bool,
    /// Geometric range in metres.
    pub range_m: f64,
    /// One-way propagation time in seconds.
    pub light_time_s: f64,
    /// Endpoint elevation in degrees, when the endpoint has a horizon.
    pub elevation_deg: Option<f64>,
    /// Authored role of the potential peer.
    pub class: Option<String>,
}

/// One resolved peer in a published link state.
#[derive(Debug, Clone, Reflect)]
pub struct LinkPeer {
    /// Stable global id of the peer.
    pub peer: u64,
    /// Result of the authored connectivity verdict.
    pub connected: bool,
    /// Geometric range in metres.
    pub range_m: f64,
    /// One-way propagation time in seconds.
    pub light_time_s: f64,
    /// Elevation above this endpoint's horizon, when measured.
    pub elevation_deg: Option<f64>,
    /// Authored role of the peer.
    pub class: Option<String>,
}

/// A scene-authored short-range radio endpoint.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct WifiNode {
    /// Maximum radio range in metres for this endpoint.
    pub max_range_m: f64,
}

/// One endpoint's resolved short-range radio peers.
#[derive(Component, Debug, Clone, Default, Reflect)]
#[reflect(Component)]
pub struct WifiState {
    /// Peers resolved for this endpoint at the last radio projection pass.
    pub peers: Vec<WifiPeer>,
}

/// One resolved short-range radio peer.
#[derive(Debug, Clone, Reflect)]
pub struct WifiPeer {
    /// Stable global id of the peer.
    pub peer: u64,
    /// Whether both endpoints' range limits permit the connection.
    pub connected: bool,
    /// Geometric range in metres.
    pub range_m: f64,
    /// One-way propagation time in seconds.
    pub light_time_s: f64,
    /// Authored role of the peer.
    pub class: Option<String>,
}
