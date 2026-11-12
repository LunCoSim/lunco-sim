//! S3: camera-driven CDLOD **visual tile streaming** — the production surface.
//!
//! When a DEM terrain streams (default; opt out per build), the single static
//! visual mesh is suppressed (the heightfield COLLIDER still spawns, so physics
//! is unchanged) and a set of LOD tiles is streamed every frame:
//!
//! 1. read the camera position in the terrain's local XZ frame → `focus`,
//! 2. [`Quadtree::select_3d`] the node set for that focus + eye height (fine under
//!    the camera, coarse far away),
//! 3. diff against the currently-spawned tiles ([`LodTiles`]): bake + spawn the
//!    new nodes ([`bake_tile_mesh`], real DEM-sampled geometry), despawn the gone,
//! 4. draw each tile with the terrain material authored by the owning USD
//! `UsdShade` network: its CDLOD **vertex morph**
//!    (`POSITION → MORPH_TARGET` by camera distance, so no LOD pop) + the
//!    fragment stage declared by the terrain's authored `UsdShade` material.
//!
//! # Appearance is INTENT here, not a material
//!
//! A tile carries a clone of the owning terrain's [`ShaderLook`] — the shader
//! source asset, its authored parameters, the CDLOD vertex source, and its
//! texture layers — plus engine-owned morph/map values. **Nothing in this crate
//! names a production shader asset.** `lunco-render-bevy` binds the intent,
//! caching by `ShaderLook::key()`, so every tile in the same LOD band
//! shares ONE material and ONE bind group. That cache *is* the old hand-rolled
//! `LodMaterials`/`MatKey` table, done generically: the `(depth, band bucket)`
//! that `MatKey` encoded is simply the look's own content now.
//!
//! Keep the key COARSE. It is what lets tiles batch, so any per-tile parameter
//! added here mints a material per tile and costs draw calls. The topology
//! stitch mask is the deliberate exception: it is per-tile material identity,
//! because neighbouring tiles can require different masks in one draw cover.
//! It is changed only when the authoritative draw topology changes, so stable
//! covers still reuse the content-keyed material cache.
//!
//! The companion canonical-res collider ring is [`crate::collider_ring`].

use bevy::camera::visibility::RenderLayers;
use bevy::math::{DMat4, DQuat, DVec3};
use bevy::prelude::*;
#[cfg(target_arch = "wasm32")]
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};
use big_space::prelude::{CellCoord, Grid, Stationary};
use lunco_core::{Command, HorizonShadowTerrain, on_command, register_commands};
use lunco_environment::HorizonMap;
use lunco_materials::{
    ATTRIBUTE_MORPH_EDGE, ATTRIBUTE_MORPH_NORMAL, ATTRIBUTE_MORPH_TARGET, ParamValue, ShaderLook,
    ShaderLookReady, TextureLayer,
};
use lunco_obstacle_field::grid_mesh;
use lunco_render::camera::{SceneViewDemand, SceneViewLayers};
use lunco_terrain_core::{HeightSource, NodeGeometry, measure_node_geometry};
#[cfg(not(target_arch = "wasm32"))]
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::{
    collections::VecDeque,
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use crate::band::SurfaceBand;
use crate::derived_layers::{TerrainAuthoredMaps, TerrainDerivedMaps};
use crate::oracle::{DemHeightField, SurfaceOracle};
use crate::surface_change::TerrainSurfaceChange;
use crate::terrain::DemTerrainSurface;
use lunco_terrain_core::quadtree::{QuadCoord, Quadtree, Selected, Square};

/// Terrain tile resolution, refinement, bake pacing, and cache ceilings are
/// persisted in [`lunco_render::RenderingQualitySettings`]. Keeping them there
/// makes the Graphics menu the sole owner of render-quality policy; this module
/// only consumes the selected profile.
///
/// Configures the near-field detail policy for a consumed camera output.
///
/// Every consumed camera output owns one camera-centred detail cover; this
/// component optionally overrides that output's near-field policy.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
pub struct TerrainVisualFocus {
    /// Optional per-camera override for the radius around the camera guaranteed
    /// to use the finest visual geometry. `None` uses
    /// [`lunco_settings::TerrainSettings::visual_detail_radius_m`].
    ///
    /// Nearby subjects share the camera cover; unused physical bodies do not
    /// contribute visual refinement demand.
    pub near_detail_radius_m: Option<f64>,
    /// Optional per-camera override for the extra distance a previously refined
    /// near-field tile remains selected after the camera moves away. `None` uses
    /// [`lunco_settings::TerrainSettings::visual_detail_hysteresis_m`].
    pub near_detail_hysteresis_m: Option<f64>,
}

/// A generated render mesh continuing a finite DEM surface beyond its crop.
///
/// `source` is the authored terrain prim whose composed `UsdShade` material
/// supplies the appearance. The generated geometry owns no independent material
/// choice; `half_extent_m` is render-space context for that existing material.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct TerrainVisualContinuation {
    /// Authored finite terrain that supplies the cropped DEM maps.
    pub source: Entity,
    /// Authored declaration retained for source diagnostics. Runtime reads the composed look.
    pub authored_look_source: Option<Entity>,
    /// Current composed body look copied from its `GlobeLod` presentation state.
    pub surface_look: ShaderLook,
    pub half_extent_m: f64,
    pub collar_widths_m: [f64; 4],
}

/// Special material interface for a USD-selected body look that accepts local
/// DEM maps on generated visual continuation geometry.
pub const LUNAR_SURFACE_CONTINUATION_INTERFACE: &str = "lunco.lunar-surface-continuation.v1";

/// Rhai owns the action taken when a body shader cannot satisfy the declared
/// continuation interface. Rust validates the chosen action before applying it.
pub const LUNAR_SURFACE_CONTINUATION_HOOK: &str = "terrain.lunar_surface_continuation";

lunco_hooks::declare_hook! {
    id: LUNAR_SURFACE_CONTINUATION_HOOK,
    owner: "lunco-terrain-surface",
    description: "Choose how visual DEM continuation responds to the USD-selected surface shader interface.",
    signature: [facts: Map],
    output: Map,
    deterministic: true,
    required: true,
    installable: true,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum TerrainProjection {
    Perspective {
        fov_y_rad: f64,
        clip_from_local: DMat4,
    },
    Orthographic {
        pixels_per_metre: f64,
        area: [f64; 4],
        output_pixels: [u32; 2],
        clip_from_local: DMat4,
    },
}

impl TerrainProjection {
    fn fov_y_rad(self) -> Option<f64> {
        match self {
            Self::Perspective { fov_y_rad, .. } => Some(fov_y_rad),
            Self::Orthographic { .. } => None,
        }
    }
    fn write_signature(self, signature: &mut lunco_precompute::Fnv1a) {
        match self {
            Self::Perspective {
                fov_y_rad,
                clip_from_local,
            } => {
                signature.write_u64(0);
                signature.write_u64(fov_y_rad.to_bits());
                for value in clip_from_local.to_cols_array() {
                    signature.write_u64(value.to_bits());
                }
            }
            Self::Orthographic {
                pixels_per_metre,
                area,
                output_pixels,
                clip_from_local,
            } => {
                signature.write_u64(1);
                signature.write_u64(pixels_per_metre.to_bits());
                for value in area {
                    signature.write_u64(value.to_bits());
                }
                for value in output_pixels {
                    signature.write_u64(u64::from(value));
                }
                for value in clip_from_local.to_cols_array() {
                    signature.write_u64(value.to_bits());
                }
            }
        }
    }
    fn clip_from_local(self) -> DMat4 {
        match self {
            Self::Perspective {
                clip_from_local, ..
            }
            | Self::Orthographic {
                clip_from_local, ..
            } => clip_from_local,
        }
    }
}

/// A retained, quantized clip-plane envelope. The full-quantum retention
/// interval exceeds the half-quantum snap interval, so a pose near a rounding
/// boundary cannot alternate selection requests under subpixel physics jitter.
#[derive(Clone, Copy, Debug)]
struct SelectionClip {
    center: DVec3,
    planes: [bevy::math::DVec4; 6],
    normal_quantum: f64,
    offset_quantum: f64,
}

impl SelectionClip {
    fn normalized_planes(projection: TerrainProjection, center: DVec3) -> [bevy::math::DVec4; 6] {
        let rows = projection.clip_from_local().transpose();
        [
            rows.w_axis + rows.x_axis,
            rows.w_axis - rows.x_axis,
            rows.w_axis + rows.y_axis,
            rows.w_axis - rows.y_axis,
            rows.z_axis,
            rows.w_axis - rows.z_axis,
        ]
        .map(|plane| {
            let length = plane.truncate().length();
            // Infinite reverse perspective has an exact constant far
            // inequality. Preserve it instead of inventing a unit normal.
            if length == 0.0 {
                plane
            } else {
                let normalized = plane / length;
                normalized
                    .truncate()
                    .extend(normalized.w + normalized.truncate().dot(center))
            }
        })
    }
    fn new(
        projection: TerrainProjection,
        screen_height_px: f64,
        pixel_error: f64,
        eye: DVec3,
    ) -> Option<Self> {
        let (normal_quantum, offset_quantum) = match projection {
            TerrainProjection::Perspective { fov_y_rad, .. } => {
                let tangent = (0.5 * fov_y_rad).tan();
                // A configured pixel-error interval expressed as a normal
                // direction increment at the perspective projection boundary.
                (
                    2.0 * tangent * pixel_error / (screen_height_px * (1.0 + tangent * tangent)),
                    TERRAIN_FOCUS_QUANT_M,
                )
            }
            TerrainProjection::Orthographic {
                output_pixels,
                pixels_per_metre,
                ..
            } => (
                pixel_error / f64::from(output_pixels[0].max(output_pixels[1])),
                TERRAIN_FOCUS_QUANT_M.min(pixel_error / pixels_per_metre),
            ),
        };
        // A unit normal has a component of magnitude at least 1/sqrt(3);
        // limiting its lattice step to one preserves a nonzero plane normal.
        if !normal_quantum.is_finite()
            || normal_quantum <= 0.0
            || !offset_quantum.is_finite()
            || offset_quantum <= 0.0
        {
            return None;
        }
        let normal_quantum = normal_quantum.min(1.0);
        // The spatial owner supplies the actual terrain-local camera eye;
        // projection depth mapping does not choose the envelope reference.
        if !eye.is_finite() || !projection.clip_from_local().inverse().is_finite() {
            return None;
        }
        // Orthographic scale is independent of axial distance. Anchor its
        // angular envelope on the view axis nearest the terrain chart origin,
        // so translating along that axis cannot inflate the selected footprint.
        let reference = match projection {
            TerrainProjection::Perspective { .. } => eye,
            TerrainProjection::Orthographic { .. } => {
                let axis = projection
                    .clip_from_local()
                    .transpose()
                    .z_axis
                    .truncate()
                    .try_normalize()?;
                eye - axis * eye.dot(axis)
            }
        };
        let center = (reference / offset_quantum).round() * offset_quantum;
        if !center.is_finite() {
            return None;
        }
        let planes = Self::normalized_planes(projection, center).map(|plane| {
            if plane.truncate() == DVec3::ZERO {
                return plane;
            }
            let normal = (plane.truncate() / normal_quantum).round() * normal_quantum;
            normal.extend((plane.w / offset_quantum).round() * offset_quantum)
        });
        if planes.iter().any(|plane| !plane.is_finite()) {
            return None;
        }
        Some(Self {
            center,
            planes,
            normal_quantum,
            offset_quantum,
        })
    }
    fn retains(&self, projection: TerrainProjection, candidate: Self) -> bool {
        self.normal_quantum == candidate.normal_quantum
            && self.offset_quantum == candidate.offset_quantum
            && self
                .planes
                .iter()
                .zip(Self::normalized_planes(projection, self.center))
                .all(|(anchor, current)| {
                    if anchor.truncate() == DVec3::ZERO || current.truncate() == DVec3::ZERO {
                        return *anchor == current;
                    }
                    let difference = (*anchor - current).abs();
                    difference.truncate().max_element() <= self.normal_quantum
                        && difference.w <= self.offset_quantum
                })
    }
    fn write_signature(self, signature: &mut lunco_precompute::Fnv1a) {
        for value in self.center.to_array() {
            signature.write_u64(value.to_bits());
        }
        signature.write_u64(self.normal_quantum.to_bits());
        signature.write_u64(self.offset_quantum.to_bits());
        for plane in self.planes {
            for value in plane.to_array() {
                signature.write_u64(value.to_bits());
            }
        }
    }
    fn intersects(self, region: Square, heights: [f64; 2]) -> bool {
        if !heights[0].is_finite() || !heights[1].is_finite() || heights[0] > heights[1] {
            return true;
        }
        let region = Square {
            center: [
                region.center[0] - self.center.x,
                region.center[1] - self.center.z,
            ],
            half: region.half,
        };
        let heights = [heights[0] - self.center.y, heights[1] - self.center.y];
        let magnitude = DVec3::new(
            region.center[0].abs() + region.half,
            heights[0].abs().max(heights[1].abs()),
            region.center[1].abs() + region.half,
        );
        self.planes.iter().all(|plane| {
            if !plane.is_finite() {
                return true;
            }
            let normal = plane.truncate();
            if normal == DVec3::ZERO {
                return plane.w >= 0.0;
            }
            let maximum = normal.x * region.center[0]
                + normal.z * region.center[1]
                + region.half * (normal.x.abs() + normal.z.abs())
                + normal.y
                    * if normal.y >= 0.0 {
                        heights[1]
                    } else {
                        heights[0]
                    }
                + plane.w;
            // Every admitted pose may differ by the FULL retained quantum in
            // every coefficient, including Y and the plane offset.
            let guard = self.normal_quantum * magnitude.element_sum() + self.offset_quantum;
            // Plane recentering and point subtraction also round at the
            // original source-frame magnitudes, before common translation cancels.
            let arithmetic = normal.abs().dot(magnitude)
                + plane.w.abs()
                + guard
                + 2.0 * (normal.abs() + DVec3::splat(self.normal_quantum)).dot(self.center.abs());
            let pad = 64.0 * f64::EPSILON * arithmetic.max(1.0);
            !maximum.is_finite() || !guard.is_finite() || maximum + guard >= -pad
        })
    }
}

#[derive(Clone)]
struct VisualDemand {
    entity: Entity,
    render_layer: usize,
    source: Entity,
    terrain_poses: HashMap<Entity, (DVec3, DVec3)>,
    terrain_clips: HashMap<Entity, DMat4>,
    preparation_corridors: HashMap<Entity, TerrainPreparationCorridor>,
    position: DVec3,
    forward: DVec3,
    /// Physical height of the camera's rendered viewport, in pixels.
    screen_height_px: f64,
    projection: TerrainProjection,
    near_detail_radius_m: f64,
    near_detail_hysteresis_m: f64,
}

/// Spatial resolution of camera-driven cover reselection in terrain metres.
const TERRAIN_FOCUS_QUANT_M: f64 = 0.5;

#[derive(Clone, Copy)]
struct TerrainVisualDemand {
    camera: Entity,
    focus: [f64; 2],
    camera_y: f64,
    heading: Option<bevy::math::DVec2>,
    view_direction: DVec3,
    screen_height_px: f64,
    projection: TerrainProjection,
    selection_clip: SelectionClip,
    /// This demand comes from an active rendering camera.
    required: bool,
    near_detail_radius_m: f64,
    near_detail_hysteresis_m: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TerrainPreparationCorridor {
    start: DVec3,
    end: DVec3,
    radius_m: f64,
    source_pose: crate::surface_query::TerrainPoseInPhysicsFrame,
    source_key: u64,
}

/// Navigation intent stays admitted until its owner clears it or an exact
/// source/output lease retires; presentation cadence does not consume it.
#[derive(Clone)]
struct TerrainCameraTarget {
    source: crate::surface_query::TerrainSurfaceSnapshot,
    start: lunco_spatial::coords::GridPos,
    end: lunco_spatial::coords::GridPos,
    radius_m: f64,
    outputs: Vec<(Entity, SceneViewDemand, usize)>,
    camera_position: Option<DVec3>,
}

/// Persistent predictive intent owned and explicitly withdrawn by navigation.
#[derive(Resource, Default)]
pub struct TerrainCameraTargets {
    targets: HashMap<Entity, TerrainCameraTarget>,
}
impl lunco_core::SceneResource for TerrainCameraTargets {}
impl TerrainCameraTargets {
    pub fn request(
        &mut self,
        camera: Entity,
        source: crate::surface_query::TerrainSurfaceSnapshot,
        start: lunco_spatial::coords::GridPos,
        end: lunco_spatial::coords::GridPos,
        radius_m: f64,
    ) -> Result<(), &'static str> {
        if !start.0.is_finite() || !end.0.is_finite() || !radius_m.is_finite() || radius_m <= 0.0 {
            self.targets.remove(&camera);
            return Err(
                "terrain camera preparation requires finite endpoints and a positive finite radius",
            );
        }
        if let Some(target) = self.targets.get_mut(&camera)
            && target.source.same_source(&source)
            && target.start == start
            && target.radius_m == radius_m
        {
            target.end = end;
            return Ok(());
        }
        self.targets.insert(
            camera,
            TerrainCameraTarget {
                source,
                start,
                end,
                radius_m,
                outputs: Vec::new(),
                camera_position: None,
            },
        );
        Ok(())
    }
    pub fn clear(&mut self, camera: Entity) {
        self.targets.remove(&camera);
    }
}

/// Frame-local detail demands collected from finalized consumed camera outputs.
#[derive(Resource, Default)]
pub struct TerrainDetailDemands {
    visual: Vec<VisualDemand>,
}
/// Finalized camera demands and their latest-cover readiness are published
/// before render/capture consumers read this presentation boundary.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerrainPresentationSet {
    PublishDemands,
}
impl lunco_core::SceneResource for TerrainDetailDemands {}

impl TerrainDetailDemands {
    pub(crate) fn camera_projection(&self, camera: Entity) -> Option<TerrainProjection> {
        self.visual
            .iter()
            .find(|demand| demand.entity == camera)
            .map(|demand| demand.projection)
    }
    pub(crate) fn has_preparation(&self, camera: Entity, terrain: Entity) -> bool {
        self.visual
            .iter()
            .find(|demand| demand.entity == camera)
            .is_some_and(|demand| demand.preparation_corridors.contains_key(&terrain))
    }
    fn preparation_signature(&self, camera: Entity, terrain: Entity) -> Option<u64> {
        let lease = self.preparation_lease_signature(camera, terrain)?;
        let demand = self.visual.iter().find(|demand| demand.entity == camera)?;
        let corridor = demand.preparation_corridors.get(&terrain)?;
        let mut signature = lunco_precompute::Fnv1a::new();
        signature.write_u64(lease);
        for value in corridor.end.to_array() {
            signature.write_u64(value.to_bits());
        }
        Some(signature.finish())
    }
    fn preparation_lease_signature(&self, camera: Entity, terrain: Entity) -> Option<u64> {
        let demand = self.visual.iter().find(|demand| demand.entity == camera)?;
        let corridor = demand.preparation_corridors.get(&terrain)?;
        let (position, _) = demand.terrain_poses.get(&terrain)?;
        let mut signature = lunco_precompute::Fnv1a::new();
        signature.write_u64(camera.to_bits());
        signature.write_u64(demand.source.to_bits());
        signature.write_u64(terrain.to_bits());
        signature.write_u64(demand.render_layer as u64);
        signature.write_u64(demand.screen_height_px.to_bits());
        signature.write_u64(demand.near_detail_radius_m.to_bits());
        signature.write_u64(demand.near_detail_hysteresis_m.to_bits());
        demand.projection.write_signature(&mut signature);
        // The corridor and distance morph depend on eye position, source and
        // intrinsic projection. Coalescing desired follow endpoints must not
        // retire preparation while the admitted rig pose remains held.
        for value in position.to_array() {
            signature.write_u64(value.to_bits());
        }
        for value in corridor.start.to_array() {
            signature.write_u64(value.to_bits());
        }
        signature.write_u64(corridor.radius_m.to_bits());
        signature.write_u64(corridor.source_pose.frame.to_bits());
        signature.write_u64(corridor.source_key);
        for value in corridor
            .source_pose
            .position
            .to_array()
            .into_iter()
            .chain(corridor.source_pose.rotation.to_array())
        {
            signature.write_u64(value.to_bits());
        }
        Some(signature.finish())
    }
    pub(crate) fn cameras_for_source(&self, source: Entity) -> impl Iterator<Item = Entity> + '_ {
        self.visual
            .iter()
            .filter(move |demand| demand.entity == source || demand.source == source)
            .map(|demand| demand.entity)
    }

    pub(crate) fn terrain_pose_snapshot(
        &self,
        camera: Entity,
        terrain: Entity,
    ) -> Option<(DVec3, DVec3)> {
        self.visual
            .iter()
            .find(|demand| demand.entity == camera)?
            .terrain_poses
            .get(&terrain)
            .copied()
    }
    /// Snapshot active camera poses in their composed BigSpace root coordinates
    /// for API diagnostics. Selection does not consume these root values: it
    /// projects each camera entity into each terrain through
    /// [`camera_frame_in_terrain`], preserving the nearest shared precision frame.
    pub(crate) fn visual_focus_snapshot(&self) -> Vec<(u64, [f64; 3], [f64; 3], f64, Option<f64>)> {
        self.visual
            .iter()
            .map(|demand| {
                (
                    demand.entity.to_bits(),
                    [demand.position.x, demand.position.y, demand.position.z],
                    [demand.forward.x, demand.forward.y, demand.forward.z],
                    demand.screen_height_px,
                    demand.projection.fov_y_rad(),
                )
            })
            .collect()
    }
}

/// Snapshot only rendering cameras admitted by their output owner.
/// Runs after camera pose, viewport and physical output publication. Terrain
/// preparation consumes these immutable terrain-local poses on the next Update.
pub(crate) fn collect_terrain_detail_demands(
    mut demands: ResMut<TerrainDetailDemands>,
    mut targets: ResMut<TerrainCameraTargets>,
    terrain_settings: Option<Res<lunco_settings::TerrainSettings>>,
    cameras: Query<
        (
            Entity,
            &Camera,
            &Projection,
            &SceneViewDemand,
            &SceneViewLayers,
        ),
        With<Camera3d>,
    >,
    policies: Query<&TerrainVisualFocus>,
    terrains: Query<
        (
            Entity,
            Option<&RenderLayers>,
            Option<&crate::surface_query::TerrainPoseInPhysicsFrame>,
        ),
        With<DemTerrainSurface>,
    >,
    parents: Query<&ChildOf>,
    grids: Query<&Grid>,
    spatial: Query<(Option<&CellCoord>, &Transform)>,
    sources: Query<(Entity, &DemHeightField)>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    quality: Option<Res<lunco_render::RenderingQualitySettings>>,
    mut terrain_views: Query<(Entity, &DemHeightField, &mut LodTiles, Has<LodFrozen>)>,
    mut stream_status: Option<ResMut<TerrainStreamStatus>>,
) {
    let defaults = terrain_settings.as_deref().cloned().unwrap_or_default();
    demands.visual.clear();
    let frame = active_frame.as_deref().map(|frame| frame.0);
    let live_sources = frame.map(|frame| {
        sources
            .iter()
            .map(|(entity, field)| {
                (
                    entity,
                    field.0.surface_key(),
                    lunco_spatial::coords::pose_in_grid(entity, frame, &parents, &grids, &spatial),
                )
            })
            .collect::<Vec<_>>()
    });
    targets.targets.retain(|owner, target| {
        let (Some(frame), Some(live_sources)) = (frame, live_sources.as_ref()) else {
            return false;
        };
        if !target
            .source
            .matches_live_sources(frame, live_sources.iter().copied())
        {
            return false;
        }
        let Some(pose) =
            lunco_spatial::coords::pose_in_grid(*owner, frame, &parents, &grids, &spatial)
        else {
            return false;
        };
        if target
            .camera_position
            .is_some_and(|admitted| admitted != pose.0)
        {
            return false;
        }
        let mut outputs = cameras
            .iter()
            .filter(|(entity, camera, _, output, _)| {
                camera.is_active && (*entity == *owner || output.source == *owner)
            })
            .map(|(entity, _, _, output, layers)| (entity, *output, layers.layer))
            .collect::<Vec<_>>();
        outputs.sort_by_key(|output| output.0);
        if outputs.is_empty()
            || (!target.outputs.is_empty()
                && !target.outputs.iter().any(|lease| outputs.contains(lease)))
        {
            return false;
        }
        target.outputs = outputs;
        target.camera_position = Some(pose.0);
        true
    });
    let requests = &targets.targets;
    let profile = quality
        .as_deref()
        .and_then(|quality| quality.validated_profile().ok());
    let mut findings = Vec::new();
    for (entity, camera, projection, output, layers) in &cameras {
        if !camera.is_active {
            continue;
        }
        let Some(size) = camera.physical_viewport_size() else {
            continue;
        };
        let screen_height_px = f64::from(size.y);
        let metric = match projection {
            Projection::Perspective(perspective) => {
                let fov_y_rad = f64::from(perspective.fov);
                let clip_from_local = projection.get_clip_from_view().as_dmat4();
                (size.x > 0
                    && size.y > 0
                    && fov_y_rad.is_finite()
                    && fov_y_rad > 0.0
                    && fov_y_rad < std::f64::consts::PI
                    && clip_from_local.is_finite())
                .then_some(TerrainProjection::Perspective {
                    fov_y_rad,
                    clip_from_local,
                })
            }
            Projection::Orthographic(orthographic) => {
                let area = orthographic.area;
                let pixels_per_metre = (f64::from(size.x) / f64::from(area.width()))
                    .max(f64::from(size.y) / f64::from(area.height()));
                let clip_from_local = projection.get_clip_from_view().as_dmat4();
                (size.x > 0
                    && size.y > 0
                    && area.width() > 0.0
                    && area.height() > 0.0
                    && pixels_per_metre.is_finite()
                    && pixels_per_metre > 0.0
                    && clip_from_local.is_finite())
                .then_some(TerrainProjection::Orthographic {
                    pixels_per_metre,
                    area: [
                        f64::from(area.min.x),
                        f64::from(area.min.y),
                        f64::from(area.max.x),
                        f64::from(area.max.y),
                    ],
                    output_pixels: [size.x, size.y],
                    clip_from_local,
                })
            }
            Projection::Custom(_) => None,
        };
        let Some(metric) = metric else {
            findings.push(lunco_core::RuntimeDiagnostic {
                producer: "terrain-view-projection".into(), code: "projection-unavailable".into(),
                severity: lunco_core::DiagnosticSeverity::Error, subject: format!("camera {entity:?}"),
                message: "consumed terrain camera requires a finite perspective or orthographic projection and nonzero output pixels; custom projections require an explicit terrain screen metric".into(),
            });
            continue;
        };
        if profile.as_ref().is_some_and(|profile| {
            SelectionClip::new(
                metric,
                screen_height_px,
                profile.terrain_lod_pixel_error,
                DVec3::ZERO,
            )
            .is_none()
        }) {
            findings.push(lunco_core::RuntimeDiagnostic {
                producer: "terrain-view-projection".into(), code: "projection-unavailable".into(),
                severity: lunco_core::DiagnosticSeverity::Error, subject: format!("camera {entity:?}"),
                message: "consumed terrain camera has no finite selection clip envelope for its native projection and configured pixel error".into(),
            });
            continue;
        }
        let Ok((position, rotation)) =
            lunco_spatial::coords::world_pose(entity, &parents, &grids, &spatial)
        else {
            continue;
        };
        let policy = policies
            .get(entity)
            .or_else(|_| policies.get(output.source))
            .copied()
            .unwrap_or_default();
        let mut terrain_clips = HashMap::new();
        let terrain_poses = terrains
            .iter()
            .filter_map(|(terrain, terrain_layers, _)| {
                if !layers
                    .base
                    .intersects(&terrain_layers.cloned().unwrap_or_default())
                {
                    return None;
                }
                let (position, rotation) =
                    camera_frame_in_terrain(entity, terrain, &parents, &grids, &spatial)?;
                terrain_clips.insert(
                    terrain,
                    metric.clip_from_local()
                        * DMat4::from_rotation_translation(rotation, position).inverse(),
                );
                Some((terrain, (position, rotation * DVec3::NEG_Z)))
            })
            .collect();
        let mut preparation_corridors = HashMap::new();
        if let Some(target) = requests
            .get(&entity)
            .or_else(|| requests.get(&output.source))
        {
            for (terrain, terrain_layers, pose) in &terrains {
                if !layers
                    .base
                    .intersects(&terrain_layers.cloned().unwrap_or_default())
                {
                    continue;
                }
                if !pose.is_some_and(|pose| pose.frame == target.source.frame()) {
                    continue;
                }
                // The exact source lease was checked against freshly resolved
                // poses above; use that same pose for the local corridor.
                let Some((position, rotation)) = lunco_spatial::coords::pose_in_grid(
                    terrain,
                    target.source.frame(),
                    &parents,
                    &grids,
                    &spatial,
                ) else {
                    continue;
                };
                let Ok((_, field)) = sources.get(terrain) else {
                    continue;
                };
                preparation_corridors.insert(
                    terrain,
                    TerrainPreparationCorridor {
                        start: rotation.inverse() * (target.start.0 - position),
                        end: rotation.inverse() * (target.end.0 - position),
                        radius_m: target.radius_m,
                        source_pose: crate::surface_query::TerrainPoseInPhysicsFrame {
                            frame: target.source.frame(),
                            position,
                            rotation,
                        },
                        source_key: field.0.surface_key(),
                    },
                );
            }
        }
        demands.visual.push(VisualDemand {
            entity,
            source: output.source,
            render_layer: layers.layer,
            terrain_poses,
            terrain_clips,
            preparation_corridors,
            position: position.0,
            forward: rotation.0 * DVec3::NEG_Z,
            screen_height_px,
            projection: metric,
            near_detail_radius_m: policy
                .near_detail_radius_m
                .unwrap_or(defaults.visual_detail_radius_m)
                .max(0.0),
            near_detail_hysteresis_m: policy
                .near_detail_hysteresis_m
                .unwrap_or(defaults.visual_detail_hysteresis_m)
                .max(0.0),
        });
    }
    demands.visual.sort_by_key(|demand| demand.entity);
    let mut selection_pending = 0;
    let mut presentation_pending = 0;
    for (terrain, field, mut tiles, frozen) in &mut terrain_views {
        for demand in demands
            .visual
            .iter()
            .filter(|demand| demand.terrain_poses.contains_key(&terrain))
        {
            let Some(view) = tiles.views.get_mut(&demand.entity) else {
                selection_pending += 1;
                continue;
            };
            view.publish_latest_demand(
                &demands,
                terrain,
                demand,
                field.0.surface_key(),
                profile.as_ref(),
                frozen,
            );
            selection_pending += usize::from(!view.latest_ready());
            presentation_pending += usize::from(view.presentation_pending);
        }
    }
    if let Some(status) = stream_status.as_deref_mut() {
        status.publish_selection_pending(selection_pending);
        status.presentation_pending = presentation_pending;
    }
    if let Some(diagnostics) = diagnostics.as_deref_mut() {
        if diagnostics
            .findings
            .iter()
            .filter(|finding| finding.producer == "terrain-view-projection")
            .ne(findings.iter())
        {
            diagnostics.replace_producer("terrain-view-projection", findings);
        }
    }
}

/// Resolve one camera into one terrain's local DEM frame through their nearest
/// shared BigSpace grid.
///
/// This is the sole coordinate boundary used by visual LOD selection. A camera
/// and terrain may live in different nested grids, but they must be connected by
/// real grid ancestry. A disconnected hierarchy produces no demand; substituting
/// root-space or local coordinates would silently refine the wrong location.
#[cfg(test)]
fn camera_pose_in_terrain(
    camera: Entity,
    terrain: Entity,
    parents: &Query<&ChildOf>,
    grids: &Query<&Grid>,
    spatial: &Query<(Option<&CellCoord>, &Transform)>,
) -> Option<(DVec3, DVec3)> {
    camera_frame_in_terrain(camera, terrain, parents, grids, spatial)
        .map(|(position, rotation)| (position, rotation * DVec3::NEG_Z))
}

fn camera_frame_in_terrain(
    camera: Entity,
    terrain: Entity,
    parents: &Query<&ChildOf>,
    grids: &Query<&Grid>,
    spatial: &Query<(Option<&CellCoord>, &Transform)>,
) -> Option<(DVec3, DQuat)> {
    let (_, camera_position, camera_rotation, terrain_position, terrain_rotation) =
        lunco_spatial::coords::common_grid_poses(camera, terrain, parents, grids, spatial)?;
    let grid_to_terrain = terrain_rotation.inverse();
    Some((
        grid_to_terrain * (camera_position - terrain_position),
        grid_to_terrain * camera_rotation,
    ))
}

/// Convert a camera pose into the visual demand used by the finite terrain
/// selector.
///
/// The camera remains in the terrain-local f64 frame even outside the DEM.
/// Per-node sampled height envelopes supply the vertical distance independently.
fn terrain_visual_demand(
    demand: &VisualDemand,
    terrain_local: DVec3,
    local_forward: DVec3,
    terrain_clip: DMat4,
    pixel_error: f64,
) -> Option<TerrainVisualDemand> {
    if !terrain_local.is_finite() || !local_forward.is_finite() || !terrain_clip.is_finite() {
        return None;
    }
    let heading = bevy::math::DVec2::new(local_forward.x, local_forward.z);
    let heading = (heading.length() > 1e-3).then(|| heading.normalize());
    let focus = [terrain_local.x, terrain_local.z];
    let projection = match demand.projection {
        TerrainProjection::Orthographic {
            pixels_per_metre,
            area,
            output_pixels,
            ..
        } => TerrainProjection::Orthographic {
            pixels_per_metre,
            area,
            output_pixels,
            clip_from_local: terrain_clip,
        },
        TerrainProjection::Perspective { fov_y_rad, .. } => TerrainProjection::Perspective {
            fov_y_rad,
            clip_from_local: terrain_clip,
        },
    };
    Some(TerrainVisualDemand {
        camera: demand.entity,
        focus,
        camera_y: terrain_local.y,
        heading,
        view_direction: local_forward,
        screen_height_px: demand.screen_height_px,
        projection,
        selection_clip: SelectionClip::new(
            projection,
            demand.screen_height_px,
            pixel_error,
            terrain_local,
        )?,
        required: true,
        near_detail_radius_m: demand.near_detail_radius_m,
        near_detail_hysteresis_m: demand.near_detail_hysteresis_m,
    })
}

/// Marker: this terrain streams visual LOD tiles. Inserted by the build when
/// the request set `lod_viz`. The authoritative tile-resolution and refinement
/// policy lives in [`lunco_render::RenderingQualitySettings`], so this marker
/// intentionally carries no second copy that could go stale after a Graphics
/// edit.
#[derive(Component)]
pub struct TerrainLodViz;

/// One spawned LOD tile + the regen **generation** it was baked at. When a live
/// re-bake (Inspector edit) changes the heights, the terrain's generation bumps; a
/// tile whose `gen` is older is *stale* — it keeps rendering (so the surface never
/// opens a hole) while a fresh replacement bakes near-camera-first, and is despawned
/// only after the complete replacement draw passes camera admission. This makes regeneration *progressive*
/// instead of a synchronous despawn-everything flash.
#[derive(Clone, Copy)]
struct TileSlot {
    resolution: usize,
    half_extent: f64,
    entity: Entity,
    r#gen: u32,
    /// The tile's selected morph-band end (parent refine range) — kept so a live
    /// quality-profile change can rebuild the right band material without re-selecting.
    morph_end: f32,
    /// Whether this tile is currently VISIBLE. A tile can be resident but hidden:
    /// the coarse base is always resident, and only draws where the finer tile
    /// that should cover its area is not ready yet. Tracked so the `Visibility`
    /// command is issued on a flip, not every frame.
    drawn: bool,
    /// A new entity stays hidden until the render binder has attached a concrete
    /// material whose shader layout is ready. This is a domain/render boundary
    /// contract, not an ECS-timing guess: [`ShaderLookReady`] is inserted only
    /// after shader loading and parameter reflection have completed.
    ready: bool,
    /// The edge-stitch topology currently bound to this tile's material. The
    /// stitch mask is per-tile material identity, not a shared live uniform: two
    /// neighbouring tiles can require different masks in the same frame. Keep it
    /// here so a stable draw cover does not dirty every tile's `ShaderLook` on
    /// every update.
    stitch_edges: [f32; 4],
}

#[derive(Clone, Copy)]
struct DrawPublicationEntry {
    resolution: usize,
    half_extent: f64,
    coord: QuadCoord,
    entity: Entity,
    morph_end: f32,
    stitch_edges: [f32; 4],
}

fn publication_look_matches(
    current: &ShaderLook,
    entry: &DrawPublicationEntry,
    ratio: f32,
) -> bool {
    let (start, end, _) = snap_band(entry.morph_end, ratio);
    current.values().get("morph_start") == Some(&ParamValue::F32(start))
        && current.values().get("morph_end") == Some(&ParamValue::F32(end))
        && current.values().get("stitch_edges") == Some(&ParamValue::Vec4(entry.stitch_edges))
}

fn publication_look(
    current: &ShaderLook,
    entry: &DrawPublicationEntry,
    ratio: f32,
) -> Option<ShaderLook> {
    if publication_look_matches(current, entry, ratio) {
        return None;
    }
    let (start, end, _) = snap_band(entry.morph_end, ratio);
    let mut look = current.clone();
    set_param(&mut look, "morph_start", ParamValue::F32(start));
    set_param(&mut look, "morph_end", ParamValue::F32(end));
    set_param(
        &mut look,
        "stitch_edges",
        ParamValue::Vec4(entry.stitch_edges),
    );
    Some(look)
}

#[derive(Default)]
struct DrawPublicationChanges {
    retired: Vec<Entity>,
    visibility: Vec<(Entity, bool)>,
}

/// Render extraction can still reference an entity for the frame in which main
/// world visibility changes. Hide retired tiles first, then despawn after two
/// complete updates so Bevy's render-world slab never observes a freed entity.
#[derive(Component)]
pub(crate) struct RetiringTerrainTile {
    updates_left: u8,
}

fn retire_tile(commands: &mut Commands, entity: Entity) {
    commands
        .entity(entity)
        .try_insert((Visibility::Hidden, RetiringTerrainTile { updates_left: 2 }));
}

pub(crate) fn retire_terrain_tiles(
    mut commands: Commands,
    mut retiring: Query<(Entity, &mut RetiringTerrainTile)>,
) {
    for (entity, mut retirement) in &mut retiring {
        if retirement.updates_left == 0 {
            commands.entity(entity).try_despawn();
        } else {
            retirement.updates_left -= 1;
        }
    }
}

/// Keep the terrain stream's readiness cache coupled to the render owner's
/// actual material lifecycle. `ShaderLookReady` is removed when a shader layout
/// changes or a declared image disappears; ordinary in-place image updates do
/// not hide a resident tile. An observer gives the tile streamer the same event
/// without rescanning every resident tile on stable frames.
pub(crate) fn invalidate_removed_shader_look_ready(
    trigger: On<Remove<ShaderLookReady>>,
    mut commands: Commands,
    mut terrains: Query<&mut LodTiles>,
) {
    let entity = trigger.entity;
    for mut tiles in &mut terrains {
        // A tile whose concrete material lost readiness must leave the draw
        // partition immediately. Keeping its visibility bit set lets the
        // render world submit an entity after its material handle was removed,
        // which is the opaque/black frame seen during streamed replacements.
        if tiles.invalidate_material_readiness(entity) {
            commands.entity(entity).try_insert(Visibility::Hidden);
        }
    }
}

/// Build the DRAW partition: which tiles actually render this frame.
///
/// For each selected node, use its ready self, its complete ready child cover, or the deepest
/// ready ancestor — this is Cesium `ForbidHoles` / MSFS "best currently available data".
/// The child-cover case keeps outgoing detail visible while a newly-selected coarse parent
/// bakes. Then any node an ancestor already covers is dropped, so the result is DISJOINT —
/// exactly one tile per point, which keeps a coarse stand-in from z-fighting through the fine
/// surface it replaces.
///
/// Pure and separated from the streaming system on purpose: this is the invariant the whole
/// design rests on ("never a hole, never an overlap"), it was previously three copies of the
/// same ancestor walk inline in a 700-line system, and a bug in it renders as terrain
/// flicker rather than as anything a type error would catch.
///
/// `scratch` is caller-owned to keep this allocation-free on the hot path.
/// The one retained fallback. The quadtree root is a complete cover of the
/// DEM, so it is the only permanent stand-in needed while selected detail is
/// baked. Keeping a second hierarchy of coarse tiles creates extra render
/// surfaces and lets an incomplete coarse level become visible as a tile-sized
/// hole.
fn fallback_coords() -> impl Iterator<Item = QuadCoord> {
    std::iter::once(QuadCoord::ROOT)
}

fn is_coarse_fallback(coord: QuadCoord) -> bool {
    coord == QuadCoord::ROOT
}

fn coarse_fallback_tile_count() -> usize {
    1
}

/// Keep the direct parents of the live cover resident as local, hidden fallback
/// tiles. A refinement child is allowed to arrive asynchronously, but its parent
/// must not be evicted in the meantime: otherwise an unready child can only fall
/// all the way back to the root, which temporarily suppresses unrelated fine
/// terrain in the draw partition.
///
/// These are not a second rendered LOD hierarchy. They stay hidden unless an
/// exact child is briefly unavailable, at which point the normal disjoint
/// partition selects the parent only for that local quadrant.
fn wanted_parent_fallbacks(wanted: &HashSet<QuadCoord>, parents: &mut HashSet<QuadCoord>) {
    parents.clear();
    parents.extend(wanted.iter().filter_map(|coord| coord.parent()));
}

#[cfg(test)]
const TEST_MAX_COVER_EDITS: usize = 64;
#[cfg(test)]
const TEST_LOD_HYSTERESIS_RATIO: f64 = 1.30;

/// Depth at which the cover covers `q`'s region from ABOVE: `Some(d ≤ q.depth)`
/// if `q` itself or an ancestor is in the cover, `None` if the region is covered
/// by strictly deeper nodes. Exactness (every point under exactly one cover node)
/// is what makes these three cases exhaustive.
fn covering_depth(cover: &HashSet<QuadCoord>, mut q: QuadCoord) -> Option<u8> {
    loop {
        if cover.contains(&q) {
            return Some(q.depth);
        }
        q = q.parent()?;
    }
}

/// Would replacing `p`'s four children by `p` break the 2:1 edge restriction?
/// True iff some region edge-adjacent to `p` is covered at depth ≥ `p.depth + 2`.
/// Tested on the depth-`p.depth+1` rim coords just outside `p`: by exactness,
/// "neither the coord nor any ancestor is in the cover" ⟺ covered strictly
/// deeper than `p.depth + 1`.
fn merge_violates_restriction(cover: &HashSet<QuadCoord>, p: QuadCoord) -> bool {
    let d1 = p.depth + 1;
    let side = 1i64 << d1;
    let (x0, z0) = (2 * p.x as i64, 2 * p.z as i64);
    let rim = [
        (x0 - 1, z0),
        (x0 - 1, z0 + 1), // left
        (x0 + 2, z0),
        (x0 + 2, z0 + 1), // right
        (x0, z0 - 1),
        (x0 + 1, z0 - 1), // below
        (x0, z0 + 2),
        (x0 + 1, z0 + 2), // above
    ];
    rim.into_iter().any(|(x, z)| {
        x >= 0
            && z >= 0
            && x < side
            && z < side
            && covering_depth(
                cover,
                QuadCoord {
                    depth: d1,
                    x: x as u32,
                    z: z as u32,
                },
            )
            .is_none()
    })
}

/// Evolve the PERSISTENT cover one bounded step toward what the metric wants.
///
/// This replaces the global budget fit. That fit re-derived `pixel_error` every
/// frame to hit the tile budget, and because every refine distance is a function of
/// `pixel_error`, ANY change re-selected the entire cover — measured on moonbase as
/// `wanted` alternating 349 <-> 532 every frame. It also had no fixed point (coarsen
/// above 100% of budget, refine below 85%, accept up to 100%, rungs ~1.5x apart), so
/// it oscillated and needed a dwell timer to damp a loop that should not exist.
///
/// Here the budget is enforced by REFUSING FURTHER SPLITS, not by moving a global
/// metric, so there is nothing to re-derive and no mass republish is possible. The
/// metric itself is fixed (the configured `pixel_error`), which also restores
/// view-independent, peer-identical selection.
///
/// Split priority is `dist / refine_range`, so the budget is spent nearest-first.
/// Merges run before splits (they free budget) and only past the same
/// configured hysteresis band, so a camera parked on a
/// threshold cannot flip a quad every frame.
///
/// The cover stays an exact, disjoint REPLACE cover throughout: a split swaps one
/// node for its four children, a merge swaps four siblings for their parent, and
/// nothing else touches it.
///
/// It also stays RESTRICTED (edge-adjacent nodes differ by ≤ 1 depth — the CDLOD
/// morph contract; a 2-level seam has no band wide enough to blend it). The cover
/// starts at `{ROOT}`, which is trivially restricted, so the invariant holds by
/// induction: every merge is guarded ([`merge_violates_restriction`]) and every
/// split first force-splits any coarser edge-neighbour (which, ON a restricted
/// cover, is at most one level up — so the cascade is depth-1 per neighbour,
/// recursing only through the chain it builds).
///
/// Returns the number of splits refused because they would exceed `budget` —
/// zero at any fixed point whose wanted set fits (see
/// [`TerrainStreamStatus::budget_refused`]).
#[cfg(test)]
fn evolve_cover(
    qt: &Quadtree,
    cover: &mut HashSet<QuadCoord>,
    focus: [f64; 2],
    camera_y: f64,
    node_error: &impl Fn(QuadCoord, Square) -> f64,
    budget: usize,
) -> usize {
    evolve_cover_for_foci_with_retention(
        qt,
        cover,
        &[(focus, camera_y)],
        node_error,
        &|_, _| [0.0, 0.0],
        budget,
        TEST_MAX_COVER_EDITS,
        TEST_LOD_HYSTERESIS_RATIO,
        &|_, _| false,
        &mut CoverScratch::default(),
    )
    .1
}

/// Reused candidate collections for [`evolve_cover_for_foci_with_retention`] —
/// previously three fresh heap collections per terrain per frame, allocated to
/// apply the configured cover-edit budget.
#[derive(Default)]
struct CoverScratch {
    parents: HashSet<QuadCoord>,
    merges: Vec<(f64, QuadCoord)>,
    splits: Vec<(f64, QuadCoord)>,
    chain: Vec<QuadCoord>,
}

/// The persistent-cover evolution with an optional retain band. A retain band
/// blocks voluntary merges of already-refined tiles, but never requests new
/// splits; it is therefore hysteresis rather than a larger detail radius.
///
/// Returns `(edits applied, splits refused by the budget)`. Zero edits means
/// the cover is at its fixed point FOR THESE INPUTS — the caller can skip the
/// pass until an input changes (see [`LodView::settled_sig`]).
fn evolve_cover_for_foci_with_retention(
    qt: &Quadtree,
    cover: &mut HashSet<QuadCoord>,
    foci: &[([f64; 2], f64)],
    node_error: &impl Fn(QuadCoord, Square) -> f64,
    node_heights: &impl Fn(QuadCoord, Square) -> [f64; 2],
    budget: usize,
    max_cover_edits: usize,
    hysteresis_ratio: f64,
    retain_refinement: &impl Fn(QuadCoord, Square) -> bool,
    scratch: &mut CoverScratch,
) -> (usize, usize) {
    let CoverScratch {
        parents,
        merges,
        splits,
        chain,
    } = scratch;
    if cover.is_empty() {
        cover.insert(QuadCoord::ROOT);
    }
    let range = |c: QuadCoord| qt.error_refine_range(node_error(c, qt.region(c)));
    let dist = |c: QuadCoord| {
        foci.iter()
            .map(|(focus, camera_y)| {
                qt.focus_distance(
                    c,
                    [focus[0], *camera_y, focus[1]],
                    node_heights(c, qt.region(c)),
                )
            })
            .reduce(f64::min)
            .unwrap_or(f64::INFINITY)
    };
    // How far past its refine range a node sits: < 1 wants to be finer, > 1 coarser.
    let slack = |c: QuadCoord| dist(c) / range(c).max(1e-9);

    // Full quads whose parent could take over, ranked by how far past the band.
    parents.clear();
    for c in cover.iter() {
        if let Some(p) = c.parent() {
            parents.insert(p);
        }
    }
    merges.clear();
    merges.extend(
        parents
            .drain()
            .filter(|p| p.children().iter().all(|k| cover.contains(k)))
            .map(|p| {
                let slack = if retain_refinement(p, qt.region(p)) {
                    0.0
                } else {
                    slack(p)
                };
                (slack, p)
            }),
    );
    // Total order for peer determinism: slack, then coord (a slack-only sort
    // leaves equal keys in HashSet iteration order, so a budget truncation
    // would edit different nodes on different peers/runs).
    let merge_order = |a: &(f64, QuadCoord), b: &(f64, QuadCoord)| {
        b.0.total_cmp(&a.0)
            .then_with(|| (a.1.depth, a.1.x, a.1.z).cmp(&(b.1.depth, b.1.x, b.1.z)))
    };
    // Sorting the FULL merge list was one of the two per-frame O(n log n)
    // passes run to apply the configured cover-edit budget. Pass 1 only ever consumes
    // the candidates past the hysteresis band, so partition those to the
    // front and order just that (usually empty) prefix; the full ordering is
    // materialised only on the rare over-budget frame (pass 2 below).
    let mut band = 0;
    for i in 0..merges.len() {
        if merges[i].0 >= hysteresis_ratio {
            merges.swap(band, i);
            band += 1;
        }
    }
    merges[..band].sort_by(merge_order);

    let mut edits = 0usize;
    let mut budget_refused = 0usize;
    let merge_one = |cover: &mut HashSet<QuadCoord>, p: QuadCoord| {
        // Re-check: an earlier merge in this pass may have consumed these children.
        if !p.children().iter().all(|k| cover.contains(k)) {
            return false;
        }
        for k in p.children() {
            cover.remove(&k);
        }
        cover.insert(p);
        true
    };

    // 1. Voluntary merges — the node is genuinely past the hysteresis band
    //    (the sorted prefix holds exactly those, farthest first).
    for &(_, p) in merges.iter().take(band) {
        if edits >= max_cover_edits {
            break;
        }
        // Restriction guard — also what makes cascade splits STABLE: a node
        // force-split for a neighbour's sake drifts past the hysteresis band, but
        // merging it back would re-violate, so the guard refuses and the cover
        // keeps its fixed point instead of flip-flopping.
        if merge_violates_restriction(cover, p) {
            continue;
        }
        if merge_one(cover, p) {
            edits += 1;
        }
    }

    // 2. Forced merges — over budget (the budget shrank, or forced splits pushed us
    //    past it). Give up the LEAST valuable *non-retained* quad first. A retain
    //    band is visual stability policy, not merely a voluntary-merge hint: the
    //    old reverse sort put its zero-slack near-camera entries FIRST here, causing
    //    the precise fine → coarse → fine flicker it was meant to prevent. Only if
    //    no far merge can satisfy the hard budget may this pass touch retained tiles.
    //    Not bounded by the configured cover-edit budget: being over budget is a frame-rate problem,
    //    and unlike a global metric change this only touches the quads it drops.
    // Forced merges count toward the RETURNED edit total (a pass that shrank
    // the cover is not a fixed point) but not toward the cover-edit setting — being
    // over budget is a frame-rate problem the pass must fully resolve.
    let mut forced = 0usize;
    if cover.len() > budget {
        // Only NOW does the whole list need ordering (the fast path above
        // sorted just the hysteresis prefix).
        merges.sort_by(merge_order);
        for retained in [false, true] {
            // `merges` is sorted farthest-first (largest slack first). Preserve
            // that order here: the reverse iterator used to drop the nearest
            // non-retained quad before an available far one, producing a fine →
            // coarse → fine pulse as a moving camera crossed successive tiles.
            for &(_, p) in merges.iter() {
                if cover.len() <= budget {
                    break;
                }
                if retain_refinement(p, qt.region(p)) != retained {
                    continue;
                }
                // Restricted merges are skipped, so a budget shrink can stay over
                // budget for a frame or two — the deepest quads always merge freely
                // (their neighbours can be at most one level deeper than THEM, never
                // two below the parent), and each pass of bottom-up merging unlocks
                // the next, so it converges instead of stalling.
                if merge_violates_restriction(cover, p) {
                    continue;
                }
                if merge_one(cover, p) {
                    forced += 1;
                }
            }
            if cover.len() <= budget {
                break;
            }
        }
    }

    // 3. Splits, nearest-first, until the budget is spent.
    splits.clear();
    splits.extend(
        cover
            .iter()
            .copied()
            .filter(|c| c.depth < qt.max_depth)
            .map(|c| (slack(c), c))
            .filter(|&(s, _)| s < 1.0),
    );
    let split_order = |a: &(f64, QuadCoord), b: &(f64, QuadCoord)| {
        a.0.total_cmp(&b.0)
            .then_with(|| (a.1.depth, a.1.x, a.1.z).cmp(&(b.1.depth, b.1.x, b.1.z)))
    };
    // The second full O(n log n) sort per frame: on a moving camera the
    // wanting-to-split set can approach the whole cover, but at most the
    // remaining edit budget of them can LAND. Partition the best candidates
    // to the front (total order ⇒ the partition boundary is exact) and sort
    // only that prefix; the tail is ordered on demand ONLY if refusals or
    // consumed cascades leave edit budget unspent — so the visit order, the
    // applied edits and the `budget_refused` count are all identical to the
    // full sort.
    let prefix = max_cover_edits.saturating_sub(edits).min(splits.len());
    if prefix > 0 && prefix < splits.len() {
        splits.select_nth_unstable_by(prefix - 1, split_order);
    }
    splits[..prefix].sort_by(split_order);
    let mut sorted_upto = prefix;
    let mut next = 0;
    while next < splits.len() {
        if edits >= max_cover_edits {
            break;
        }
        if next == sorted_upto {
            // Earlier candidates were refused/consumed without spending the
            // edit budget — order the tail before visiting it.
            splits[sorted_upto..].sort_by(split_order);
            sorted_upto = splits.len();
        }
        let (_, c) = splits[next];
        next += 1;
        if !cover.contains(&c) {
            continue; // consumed by an earlier chain's cascade
        }
        // Splitting `c` puts depth `c.depth + 1` against every edge-adjacent
        // region, so any neighbour covered COARSER than `c` must split first —
        // the classic restricted-quadtree cascade. On a restricted cover each
        // such neighbour is exactly one level up, and its own cascade needs (the
        // ancestors the loop pushes transitively), so the chain stays short and
        // the walk terminates at the root.
        chain.clear();
        chain.push(c);
        let mut i = 0;
        while i < chain.len() {
            let n = chain[i];
            for nb in n.edge_neighbours() {
                if let Some(nd) = covering_depth(cover, nb) {
                    if nd < n.depth {
                        let shift = nb.depth - nd;
                        let a = QuadCoord {
                            depth: nd,
                            x: nb.x >> shift,
                            z: nb.z >> shift,
                        };
                        if !chain.contains(&a) {
                            chain.push(a);
                        }
                    }
                }
            }
            i += 1;
        }
        // The whole chain lands or none of it does — a partial cascade would leave
        // the violation it exists to prevent. `continue`, not `break`: a later,
        // cheaper split (no cascade) may still fit the remaining budget.
        if edits + chain.len() > max_cover_edits {
            continue; // transient: retried next pass once the edit budget frees up
        }
        if cover.len() + 3 * chain.len() > budget {
            // Unlike the edit cap this does not resolve by waiting, so it is
            // counted and surfaced.
            budget_refused += 1;
            continue;
        }
        // Coarsest first, so every forced split lands on an already-conforming rim.
        chain.sort_unstable_by_key(|q| q.depth);
        for &q in chain.iter() {
            if !cover.remove(&q) {
                continue;
            }
            for k in q.children() {
                cover.insert(k);
            }
            edits += 1;
        }
    }
    (edits + forced, budget_refused)
}

/// Construct a disjoint candidate partition from ready leaves and ancestors.
/// Freshness controls bake admission; stale published tiles remain valid visual
/// candidates until the complete successor surface passes camera admission.
fn build_draw_partition(
    selected: impl Iterator<Item = QuadCoord>,
    is_drawable: impl Fn(QuadCoord) -> bool,
    draw: &mut HashSet<QuadCoord>,
    scratch: &mut Vec<QuadCoord>,
) {
    draw.clear();
    for coord in selected {
        let mut c = coord;
        loop {
            if is_drawable(c) {
                draw.insert(c);
                break;
            }
            // A just-coarsened parent is a new selected leaf, so it often has
            // no mesh yet. Its four outgoing children were the previous exact
            // cover and are already resident. Keep that complete child cover
            // visible until the parent lands; otherwise the renderer drops to
            // a distant ancestor and flashes coarse before fine disappears.
            // A partial child set is unsafe because it would make a hole, so it
            // follows the normal ancestor fallback below.
            let children = c.children();
            if children.iter().all(|child| is_drawable(*child)) {
                draw.extend(children);
                break;
            }
            match c.parent() {
                Some(p) => c = p,
                // Nothing RESIDENT anywhere up the chain — only reachable before the
                // coarse base has baked at all. The area draws nothing for now.
                None => break,
            }
        }
    }
    scratch.clear();
    for c in draw.iter() {
        let mut p = *c;
        while let Some(up) = p.parent() {
            p = up;
            if draw.contains(&p) {
                scratch.push(*c);
                break;
            }
        }
    }
    for c in scratch.drain(..) {
        draw.remove(&c);
    }
}

/// Return the four CDLOD edge-stitch bits for a drawn tile.  `draw` is a
/// disjoint cover, so looking at the same-depth coordinate across each edge is
/// sufficient: an ancestor there means that this tile is the finer side of a
/// 2:1 boundary; a same-depth tile or descendants mean no stitch.  This is
/// topology, not a distance heuristic, and therefore cannot drift as the
/// camera or terrain scale changes.
fn stitch_edges(coord: QuadCoord, draw: &HashSet<QuadCoord>) -> [f32; 4] {
    if coord.depth == 0 {
        return [0.0; 4];
    }
    let side = 1_u32 << coord.depth;
    let neighbours = [
        if coord.z > 0 {
            Some(QuadCoord {
                depth: coord.depth,
                x: coord.x,
                z: coord.z - 1,
            })
        } else {
            None
        },
        if coord.z + 1 < side {
            Some(QuadCoord {
                depth: coord.depth,
                x: coord.x,
                z: coord.z + 1,
            })
        } else {
            None
        },
        if coord.x > 0 {
            Some(QuadCoord {
                depth: coord.depth,
                x: coord.x - 1,
                z: coord.z,
            })
        } else {
            None
        },
        if coord.x + 1 < side {
            Some(QuadCoord {
                depth: coord.depth,
                x: coord.x + 1,
                z: coord.z,
            })
        } else {
            None
        },
    ];
    let mut result = [0.0; 4];
    for (edge, neighbour) in neighbours.into_iter().enumerate() {
        let Some(neighbour) = neighbour else { continue };
        if draw.contains(&neighbour) {
            continue;
        }
        let mut ancestor = neighbour.parent();
        while let Some(candidate) = ancestor {
            if draw.contains(&candidate) {
                result[edge] = 1.0;
                break;
            }
            ancestor = candidate.parent();
        }
    }
    result
}

/// Visual products of one terrain, scoped to each consumed rendering camera.
#[derive(Component, Default)]
pub struct LodTiles {
    frame_cursor: usize,
    resolution: usize,
    r#gen: u32,
    views: BTreeMap<Entity, LodView>,
}

pub(crate) struct TerrainViewStatus {
    pub wanted: usize,
    pub resident: usize,
    pub drawn: usize,
    pub budget_refused: usize,
    pub resolution: usize,
    pub effective_max_depth: u8,
    pub selection_current: bool,
    pub ready: bool,
    pub presented_ready: bool,
    pub presentation_pending: bool,
    pub presentation_reason: Option<String>,
}

impl LodTiles {
    pub(crate) fn tile_entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.views.values().flat_map(LodView::tile_entities)
    }
    pub(crate) fn tile_material_specs(
        &self,
    ) -> impl Iterator<Item = (Entity, u32, f32, f32, [f32; 4])> + '_ {
        self.views.values().flat_map(LodView::tile_material_specs)
    }
    fn invalidate_material_readiness(&mut self, entity: Entity) -> bool {
        self.views.values_mut().fold(false, |changed, view| {
            view.invalidate_material_readiness(entity) || changed
        })
    }
    pub(crate) fn focus_tile(&self, camera: Entity, point: [f64; 2]) -> Option<(u8, bool, bool)> {
        self.views.get(&camera)?.focus_tile(point)
    }
    pub(crate) fn draw_tiles(
        &self,
        camera: Entity,
    ) -> Option<impl Iterator<Item = (QuadCoord, Entity, f32, [f32; 4])> + '_> {
        Some(
            self.views
                .get(&camera)?
                .tiles
                .iter()
                .filter(|(_, slot)| slot.ready && slot.drawn)
                .map(|(coord, slot)| (*coord, slot.entity, slot.morph_end, slot.stitch_edges)),
        )
    }
    pub(crate) fn draw_lattice(
        &self,
        camera: Entity,
        coord: QuadCoord,
    ) -> Option<(usize, f64, bool)> {
        let view = self.views.get(&camera)?;
        let slot = view.tiles.get(&coord)?;
        let current = slot.r#gen == view.r#gen
            && slot.resolution == view.resolution
            && slot.half_extent.to_bits() == view.half_extent.to_bits();
        (slot.ready && slot.drawn).then_some((slot.resolution, slot.half_extent, current))
    }
    pub(crate) fn draw_revision(&self, camera: Entity) -> Option<u64> {
        let view = self.views.get(&camera)?;
        (!view.draw_revision_exhausted).then_some(view.draw_revision)
    }
    pub(crate) fn preparation_refused(
        &self,
        camera: Entity,
        demands: &TerrainDetailDemands,
        terrain: Entity,
    ) -> bool {
        let Some(view) = self.views.get(&camera) else {
            return false;
        };
        let Some(held) = &view.held_preparation else {
            return false;
        };
        view.preparation_refusal_count(
            demands.preparation_signature(camera, terrain),
            held.lease_signature,
        ) > 0
    }
    pub(crate) fn view_status(&self, camera: Entity) -> Option<TerrainViewStatus> {
        let view = self.views.get(&camera)?;
        Some(TerrainViewStatus {
            wanted: view.cover_selected.len(),
            resident: view
                .cover_selected
                .iter()
                .filter(|selected| {
                    view.tiles
                        .get(&selected.coord)
                        .is_some_and(|slot| slot.ready)
                })
                .count(),
            drawn: view.tiles.values().filter(|slot| slot.drawn).count(),
            budget_refused: view.budget_refused,
            resolution: view.resolution,
            effective_max_depth: view.effective_max_depth,
            selection_current: view.selection_current(),
            ready: view.latest_ready(),
            presentation_pending: view.presentation_pending,
            presentation_reason: view
                .presentation_wait
                .as_ref()
                .map(|(_, reason)| reason.clone()),
            // The authoritative draw partition is disjoint. Its dyadic node
            // areas sum exactly to the root area at supported depths <=20.
            presented_ready: view
                .tiles
                .iter()
                .filter(|(_, slot)| slot.ready && slot.drawn)
                .map(|(coord, _)| 0.25_f64.powi(i32::from(coord.depth)))
                .sum::<f64>()
                == 1.0,
        })
    }
    pub fn invalidate(&mut self) {
        self.invalidate_region(None, 1.0);
    }
    pub fn invalidate_region(&mut self, bounds: Option<[f64; 4]>, half: f64) {
        self.r#gen = self.r#gen.wrapping_add(1);
        for view in self.views.values_mut() {
            view.invalidate_region(bounds, half);
        }
    }
    pub fn reap_stale(&mut self) -> Vec<Entity> {
        self.views
            .values_mut()
            .flat_map(LodView::reap_stale)
            .collect()
    }
}

/// Bounded fine-node intent retained under one source/output/profile/eye lease.
/// Current requests precede historical retention in balanced-cover admission.
#[derive(Default)]
struct HeldTerrainPreparation {
    lease_signature: u64,
    input_signature: Option<u64>,
    nodes: HashSet<QuadCoord>,
    latest_nodes: HashSet<QuadCoord>,
    capacity_refused: bool,
    requirements_signature: u64,
}

impl HeldTerrainPreparation {
    fn admit(
        &mut self,
        lease: u64,
        input: u64,
        corridor: TerrainPreparationCorridor,
        quadtree: &Quadtree,
        capacity: usize,
    ) {
        if self.lease_signature != lease {
            *self = Self {
                lease_signature: lease,
                ..default()
            };
        }
        if self.input_signature == Some(input) {
            return;
        }
        self.input_signature = Some(input);
        let Ok(nodes) = preparation_corridor_nodes(quadtree, corridor, capacity) else {
            self.capacity_refused = true;
            self.latest_nodes.clear();
            return;
        };
        self.capacity_refused = false;
        self.latest_nodes = nodes.into_iter().collect();
        let union_count = self.nodes.len()
            + self
                .latest_nodes
                .iter()
                .filter(|node| !self.nodes.contains(*node))
                .count();
        if union_count > capacity {
            // Superseded intent cannot make the feasible latest corridor fail.
            self.nodes.clone_from(&self.latest_nodes);
        } else {
            self.nodes.extend(self.latest_nodes.iter().copied());
        }
        let mut nodes = self.nodes.iter().copied().collect::<Vec<_>>();
        nodes.sort_unstable_by_key(|node| (node.depth, node.x, node.z));
        let mut signature = lunco_precompute::Fnv1a::new();
        signature.write_u64(lease);
        for node in nodes {
            signature.write_u64(u64::from(node.depth));
            signature.write_u64(u64::from(node.x));
            signature.write_u64(u64::from(node.z));
        }
        let mut latest = self.latest_nodes.iter().copied().collect::<Vec<_>>();
        latest.sort_unstable_by_key(|node| (node.depth, node.x, node.z));
        signature.write_u64(latest.len() as u64);
        for node in latest {
            signature.write_u64(u64::from(node.depth));
            signature.write_u64(u64::from(node.x));
            signature.write_u64(u64::from(node.z));
        }
        self.requirements_signature = signature.finish();
    }
    fn requirements(&self) -> TerrainPreparationRequirements {
        TerrainPreparationRequirements {
            nodes: self.nodes.clone(),
            latest_nodes: self.latest_nodes.clone(),
        }
    }
}

/// Per-view selected cover and resident tile entities for one terrain.
#[derive(Default)]
struct LodView {
    selection_clip: Option<SelectionClip>,
    draw_revision: u64,
    draw_revision_exhausted: bool,
    render_layer: usize,
    resolution: usize,
    effective_max_depth: u8,
    half_extent: f64,
    tiles: HashMap<QuadCoord, TileSlot>,
    /// Hidden same-coordinate replacements cannot retire the published surface
    /// until its complete successor partition passes stationary admission.
    pending_replacements: HashMap<QuadCoord, TileSlot>,
    presentation_pending: bool,
    presentation_wait: Option<(u64, String)>,
    morph_ratio: f32,
    r#gen: u32,
    /// Signature of the inputs the tile SELECTION is a pure function of (camera
    /// focus + eye height + heading — heading drives bake PRIORITY via
    /// `benefit()` — generation, oracle identity, LOD knobs). When it
    /// matches last frame's and no bake is in flight, the
    /// resident tile set is already correct and the whole reselection is skipped —
    /// the idle-camera fast path (see [`update_lod_tiles`]). `None` = never selected.
    last_sig: Option<u64>,
    requested_signature: Option<u64>,
    /// Hard source/view inputs that retire queued preparation and wake cover
    /// reconciliation independently of the presentation cadence.
    last_admission_sig: Option<u64>,
    /// The PERSISTENT cover: the leaves currently selected. Evolved incrementally by
    /// `evolve_cover` instead of re-derived each frame, which is what removes the
    /// mass re-selection the old global budget fit caused.
    cover: HashSet<QuadCoord>,
    /// The immutable selection derived for `cover`, committed with it from one
    /// completed cover-preparation operation.
    cover_selected: Vec<Selected>,
    /// Camera patches already requested inside the detail radius, retained
    /// only through the configured hysteresis band and their exact source.
    camera_patches: CameraPatchRetention,
    /// At most one asynchronous cover preparation may own this terrain at a time.
    pending_cover: Option<(u32, u64, u64, u64, u64)>,
    cover_operation: u64,
    /// A failed preparation stays terminal for its exact input signature.
    failed_cover_signature: Option<u64>,
    /// Whether the always-resident coarse fallback cover is fully render-ready.
    ///
    /// Load-bearing for correctness, not just speed: while this is false the idle
    /// fast path MUST NOT skip the frame body. The gate gives up when nothing is
    /// in flight and the camera has not moved — but "nothing in flight" also
    /// happens the frame the last queued bakes land, and with a still camera the
    /// remaining coarse tiles would then never be queued again. The base would sit
    /// permanently incomplete and the fallback it exists to provide would silently
    /// not be there. Also lets the enumeration be skipped once complete.
    coarse_ready: bool,
    /// Budget-refused split count from the last live [`evolve_cover`] pass. Kept
    /// on the component because the idle fast path skips selection — a
    /// starved-but-idle terrain must keep reporting, not read as healthy.
    budget_refused: usize,
    preparation_refused_nodes: HashSet<QuadCoord>,
    preparation_signature: Option<u64>,
    held_preparation: Option<HeldTerrainPreparation>,
    /// Whether every resident tile has crossed the render-material readiness
    /// boundary. New slots clear this once; settled terrains must not scan the
    /// entire resident map on every frame just to rediscover that fact.
    materials_ready: bool,
    /// The first camera cover is immutable until its full near-camera patch is ready.
    /// Camera rigs often settle over several startup frames; re-selecting in that
    /// interval cancelled and replaced the same near tiles repeatedly, producing
    /// coarse → fine → coarse flicker before normal traversal had even begun.
    bootstrap_ready: bool,
    /// Selection signature at which the last `evolve_cover` pass applied ZERO
    /// edits — the cover's fixed point for those inputs. While the signature
    /// still matches, re-running the pass can only rebuild and re-rank the
    /// same refused split chains: a permanently `budget_refused` terrain
    /// otherwise retried them EVERY frame the body ran (all through a long
    /// fill, while bakes are in flight), keeping idle CPU burnt on a decision
    /// that cannot change until the focus/gen/oracle/budget signature does.
    settled_sig: Option<u64>,
}

impl LodView {
    fn preparation_refusal_count(&self, input: Option<u64>, admission: u64) -> usize {
        let Some(held) = self.held_preparation.as_ref() else {
            return 0;
        };
        if held.input_signature != input
            || held.lease_signature != admission
            || self.requested_signature.is_none()
        {
            return 0;
        }
        if held.capacity_refused {
            return 1;
        }
        if !self.selection_current() || self.last_admission_sig != Some(admission) {
            return 0;
        }
        held.latest_nodes
            .intersection(&self.preparation_refused_nodes)
            .count()
    }
    fn cover_selection_settled(&self, signature: u64) -> bool {
        self.last_sig == Some(signature) && self.settled_sig == Some(signature)
    }
    fn cover_preparation_needed(&self, signature: u64) -> bool {
        self.cover.is_empty() || (self.bootstrap_ready && !self.cover_selection_settled(signature))
    }
    fn commit_frozen_cover(&mut self, selected: &[Selected], signature: u64, admission: u64) {
        if self.cover_selected != selected {
            self.cover_selected = selected.to_vec();
            self.bump_draw_revision();
        }
        self.last_sig = Some(signature);
        self.settled_sig = Some(signature);
        self.last_admission_sig = Some(admission);
        self.preparation_signature = self
            .held_preparation
            .as_ref()
            .map(|held| held.requirements_signature);
        self.preparation_refused_nodes = self
            .held_preparation
            .as_ref()
            .map(|held| held.latest_nodes.clone())
            .unwrap_or_default();
    }
    fn selection_current(&self) -> bool {
        self.requested_signature
            .is_some_and(|signature| self.cover_selection_settled(signature))
            && self.pending_cover.is_none()
            && self
                .held_preparation
                .as_ref()
                .is_none_or(|held| self.preparation_signature == Some(held.requirements_signature))
    }
    fn latest_ready(&self) -> bool {
        !self.presentation_pending
            && self.selection_current()
            && selected_cover_status(self, self.r#gen).complete
            && self.held_preparation.as_ref().is_none_or(|held| {
                !held.capacity_refused
                    && !held
                        .latest_nodes
                        .iter()
                        .any(|node| self.preparation_refused_nodes.contains(node))
            })
    }
    fn retain_selection_clip(&mut self, visual: &mut TerrainVisualDemand) {
        let candidate = visual.selection_clip;
        let retained = self
            .selection_clip
            .filter(|anchor| anchor.retains(visual.projection, candidate))
            .unwrap_or(candidate);
        self.selection_clip = Some(retained);
        visual.selection_clip = retained;
    }
    fn publish_latest_demand(
        &mut self,
        demands: &TerrainDetailDemands,
        terrain: Entity,
        demand: &VisualDemand,
        surface_key: u64,
        profile: Option<&lunco_render::RenderQualityProfile>,
        frozen: bool,
    ) {
        self.requested_signature = None;
        let Some(profile) = profile else {
            return;
        };
        let Some(&(position, forward)) = demand.terrain_poses.get(&terrain) else {
            return;
        };
        let Some(clip) = demand.terrain_clips.get(&terrain).copied() else {
            return;
        };
        let Some(mut visual) = terrain_visual_demand(
            demand,
            position,
            forward,
            clip,
            profile.terrain_lod_pixel_error,
        ) else {
            return;
        };
        self.retain_selection_clip(&mut visual);
        let (signature, admission) = terrain_view_signatures(
            demands,
            terrain,
            demand,
            std::slice::from_ref(&visual),
            self.r#gen,
            surface_key,
            profile,
            frozen,
        );
        let preparation = match (
            demands.preparation_signature(demand.entity, terrain),
            &self.held_preparation,
        ) {
            (None, None) => None,
            (Some(input), Some(held))
                if held.input_signature == Some(input) && held.lease_signature == admission =>
            {
                Some(held.requirements_signature)
            }
            _ => return,
        };
        self.requested_signature = Some(finish_terrain_view_signature(
            signature,
            admission,
            preparation,
        ));
    }
    fn bump_draw_revision(&mut self) {
        if let Some(next) = self.draw_revision.checked_add(1) {
            self.draw_revision = next;
        } else {
            self.draw_revision_exhausted = true;
        }
    }
    fn insert_slot(&mut self, coord: QuadCoord, slot: TileSlot) -> Option<TileSlot> {
        self.bump_draw_revision();
        self.materials_ready = false;
        if self.tiles.contains_key(&coord) {
            self.pending_replacements.insert(coord, slot)
        } else {
            self.tiles.insert(coord, slot)
        }
    }

    /// The commit boundary owns mesh replacement, topology and the CPU copy of
    /// material geometry state together. Refusal leaves every published slot intact.
    fn publish_draw_partition(
        &mut self,
        candidate: &[DrawPublicationEntry],
        morph_ratio: f32,
        admitted: bool,
    ) -> DrawPublicationChanges {
        if !admitted {
            self.presentation_pending = true;
            return DrawPublicationChanges::default();
        }
        let mut changes = DrawPublicationChanges::default();
        let draw: HashMap<_, _> = candidate.iter().map(|entry| (entry.coord, entry)).collect();
        let mut changed = self.morph_ratio.to_bits() != morph_ratio.to_bits();
        for entry in candidate {
            if self
                .pending_replacements
                .get(&entry.coord)
                .is_some_and(|slot| slot.entity == entry.entity)
            {
                if let Some(replacement) = self.pending_replacements.remove(&entry.coord) {
                    if let Some(old) = self.tiles.insert(entry.coord, replacement) {
                        changes.retired.push(old.entity);
                    }
                    changed = true;
                }
            }
        }
        // Ready replacements outside the admitted draw are hidden residents.
        // Committing them too retires stale fallback payloads and lets the view
        // return to its idle gate once all staged materials have arrived.
        let hidden: Vec<_> = self
            .pending_replacements
            .iter()
            .filter_map(|(coord, slot)| (slot.ready && !draw.contains_key(coord)).then_some(*coord))
            .collect();
        for coord in hidden {
            if let Some(replacement) = self.pending_replacements.remove(&coord) {
                if let Some(old) = self.tiles.insert(coord, replacement) {
                    changes.retired.push(old.entity);
                }
                changed = true;
            }
        }
        for (coord, slot) in &mut self.tiles {
            let entry = draw.get(coord);
            let visible = entry.is_some_and(|entry| entry.entity == slot.entity);
            if visible != slot.drawn {
                changes.visibility.push((slot.entity, visible));
                slot.drawn = visible;
                changed = true;
            }
            if let Some(entry) = entry.filter(|entry| entry.entity == slot.entity) {
                changed |= slot.morph_end.to_bits() != entry.morph_end.to_bits()
                    || slot.stitch_edges != entry.stitch_edges;
                slot.morph_end = entry.morph_end;
                slot.stitch_edges = entry.stitch_edges;
            }
        }
        self.morph_ratio = morph_ratio;
        self.presentation_pending = false;
        self.presentation_wait = None;
        if changed {
            self.bump_draw_revision();
        }
        changes
    }

    fn candidate_slot(&self, coord: &QuadCoord) -> Option<&TileSlot> {
        self.pending_replacements
            .get(coord)
            .filter(|slot| slot.ready)
            .or_else(|| self.tiles.get(coord).filter(|slot| slot.ready))
    }

    fn fresh_slot(&self, coord: &QuadCoord, generation: u32) -> bool {
        self.pending_replacements
            .get(coord)
            .or_else(|| self.tiles.get(coord))
            .is_some_and(|slot| slot.r#gen == generation)
    }

    /// Invalidate the streamer's cached render-readiness when the render binder
    /// removes `ShaderLookReady` from one of its tile entities. The marker can
    /// be removed by a shader or texture reload after the terrain has reached
    /// its idle fast path; leaving the slot's cached `ready` bit set would then
    /// keep an invalid material visible until another tile happened to spawn.
    fn invalidate_material_readiness(&mut self, entity: Entity) -> bool {
        let mut was_drawn = false;
        let mut changed = false;
        if self
            .tiles
            .values()
            .chain(self.pending_replacements.values())
            .any(|slot| slot.entity == entity)
        {
            self.materials_ready = false;
            for slot in self
                .tiles
                .values_mut()
                .chain(self.pending_replacements.values_mut())
                .filter(|slot| slot.entity == entity)
            {
                changed |= slot.ready || slot.drawn;
                slot.ready = false;
                was_drawn |= slot.drawn;
                slot.drawn = false;
            }
        }
        if changed {
            self.bump_draw_revision();
        }
        was_drawn
    }

    /// Every resident tile entity (the late-bind / live-tune targets).
    pub(crate) fn tile_entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.tiles
            .values()
            .chain(self.pending_replacements.values())
            .map(|s| s.entity)
    }

    /// Material geometry inputs for the live diagnostic tool. The morph band is
    /// snapped at the same boundary as normal tile creation so changing the
    /// diagnostic mode cannot silently create a different material topology.
    pub(crate) fn tile_material_specs(
        &self,
    ) -> impl Iterator<Item = (Entity, u32, f32, f32, [f32; 4])> + '_ {
        self.tiles
            .iter()
            .chain(self.pending_replacements.iter())
            .map(move |(coord, slot)| {
                let (morph_start, morph_end, _) = snap_band(slot.morph_end, self.morph_ratio);
                (
                    slot.entity,
                    coord.depth as u32,
                    morph_start,
                    morph_end,
                    slot.stitch_edges,
                )
            })
    }

    /// Read the selected camera leaf without deriving a parallel cover.
    pub(crate) fn focus_tile(&self, point: [f64; 2]) -> Option<(u8, bool, bool)> {
        self.cover_selected
            .iter()
            .find(|selected| selected.region.distance_to(point) <= f64::EPSILON)
            .map(|selected| {
                (
                    selected.coord.depth,
                    self.tiles
                        .get(&selected.coord)
                        .is_some_and(|slot| slot.r#gen == self.r#gen && slot.ready && slot.drawn),
                    self.tiles
                        .get(&selected.coord)
                        .is_some_and(|slot| slot.morph_end >= 1.0e20),
                )
            })
    }

    /// Invalidate only the tiles whose world footprint overlaps the committed
    /// surface change's bounds — the incremental
    /// re-bake for a **bounded** edit (a brush/flatten touches a small patch, not
    /// the whole terrain). Tiles outside the region are re-stamped to the new
    /// generation so they read as fresh and are never re-selected or re-baked; only
    /// overlapping tiles fall through to a re-bake. `None` bounds = whole terrain →
    /// same as [`invalidate`]. `root_half_extent` is the DEM half-extent (the
    /// quadtree root region), so each tile's square derives from its `QuadCoord`.
    fn invalidate_region(&mut self, bounds: Option<[f64; 4]>, root_half_extent: f64) {
        self.bump_draw_revision();
        let new_gen = self.r#gen.wrapping_add(1);
        if let Some(aabb) = bounds {
            for (coord, slot) in self
                .tiles
                .iter_mut()
                .chain(self.pending_replacements.iter_mut())
            {
                // Non-overlapping tiles keep the NEW gen → stay fresh (skipped).
                // Overlapping tiles keep their OLD gen → stale → re-baked.
                if !node_overlaps_aabb(*coord, root_half_extent, aabb) {
                    slot.r#gen = new_gen;
                }
            }
        }
        self.r#gen = new_gen;
        self.coarse_ready = false;
    }

    /// Keep the one published partition across repeated invalidations. Hidden
    /// stale candidates retire immediately; a visible old mesh is released only
    /// by a successfully admitted successor transaction.
    fn reap_stale(&mut self) -> Vec<Entity> {
        let cur = self.r#gen;
        let mut dead = Vec::new();
        self.tiles.retain(|_, slot| {
            let keep = slot.r#gen == cur || slot.drawn;
            if !keep {
                dead.push(slot.entity);
            }
            keep
        });
        self.pending_replacements.retain(|_, slot| {
            let keep = slot.r#gen == cur;
            if !keep {
                dead.push(slot.entity);
            }
            keep
        });
        if !dead.is_empty() {
            self.bump_draw_revision();
        }
        dead
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectedCoverStatus {
    wanted: usize,
    resident: usize,
    complete: bool,
}

/// Report whether the authoritative selected cover is fully current and drawn.
///
/// A resident stale-generation tile still covers the ground while a live edit
/// re-bakes, so it contributes to `resident`. It cannot make the cover complete:
/// the idle gate may stop only after every selected leaf is current-generation,
/// material-bound, and selected by the exact draw partition.
fn selected_cover_status(tiles: &LodView, current_generation: u32) -> SelectedCoverStatus {
    let wanted = tiles.cover_selected.len();
    let resident = tiles
        .cover_selected
        .iter()
        .filter(|selected| {
            tiles
                .tiles
                .get(&selected.coord)
                .is_some_and(|slot| slot.ready)
        })
        .count();
    let complete = wanted > 0
        && tiles.cover_selected.iter().all(|selected| {
            tiles
                .tiles
                .get(&selected.coord)
                .is_some_and(|slot| slot.r#gen == current_generation && slot.ready && slot.drawn)
        });
    SelectedCoverStatus {
        wanted,
        resident,
        complete,
    }
}

/// Whether the world square of quadtree node `coord` (derived from the DEM
/// `root_half_extent`, origin-centred — matching [`lunco_terrain_core::Quadtree::region`])
/// overlaps the axis-aligned `[min_x, min_z, max_x, max_z]` box.
fn node_overlaps_aabb(coord: QuadCoord, root_half_extent: f64, aabb: [f64; 4]) -> bool {
    let nodes_per_side = (1u64 << coord.depth) as f64;
    let side = (2.0 * root_half_extent) / nodes_per_side;
    let half = 0.5 * side;
    let cx = -root_half_extent + (coord.x as f64 + 0.5) * side;
    let cz = -root_half_extent + (coord.z as f64 + 0.5) * side;
    Square {
        center: [cx, cz],
        half,
    }
    .overlaps_aabb(aabb)
}

/// Back-pointer from a spawned LOD tile to its owning terrain. Tiles are parented
/// to the big_space **grid** (so each can carry its own `CellCoord`), not to the
/// terrain entity — so this tag lets [`despawn_orphaned_lod_tiles`] reap them when
/// the terrain is gone (e.g. on twin reload) instead of leaking under the grid.
#[derive(Component)]
pub struct LodTileOf(pub Entity);

/// Quantise a morph-band end onto the shared bucket lattice. `u32::MAX` = the
/// "never morphs" root sentinel.
fn band_bucket(morph_end: f32) -> u32 {
    if !morph_end.is_finite() || morph_end >= 1.0e19 {
        return u32::MAX;
    }
    // Quarter-log steps (~±12% band granularity).
    (morph_end.max(1.0).ln() * 4.0).floor() as u32
}

/// Snap a selected morph band to its bucket's representative values, so the tile
/// and its cached material agree exactly. Snapping DOWN (floor bucket) means a
/// tile always finishes morphing *before* the selection swaps in its parent — a
/// slightly early morph, never a pop.
fn snap_band(morph_end: f32, morph_ratio: f32) -> (f32, f32, u32) {
    let bucket = band_bucket(morph_end);
    if bucket == u32::MAX {
        return (1.0e20, 1.0e21, bucket);
    }
    let end = (bucket as f32 * 0.25).exp();
    (morph_ratio * end, end, bucket)
}

/// Edit the persisted terrain rendering-quality fields through the same typed
/// command bus used by the Graphics settings menu. Omitted values remain
/// unchanged; an invalid candidate is rejected as a whole instead of being
/// clamped into an undocumented quality level.
#[Command(default)]
pub struct SetTerrainRenderingQuality {
    pub tile_resolution: Option<usize>,
    pub cinematic_resolution: Option<usize>,
    pub pixel_error: Option<f64>,
    pub max_depth: Option<u8>,
    pub probe_resolution: Option<usize>,
    pub bakes_per_frame: Option<usize>,
    pub max_inflight_bakes: Option<usize>,
    pub tile_budget: Option<usize>,
    pub cover_edits_per_frame: Option<usize>,
    pub hysteresis_ratio: Option<f64>,
    pub morph_start_ratio: Option<f64>,
}

#[on_command(SetTerrainRenderingQuality)]
fn on_set_terrain_rendering_quality(
    trigger: On<SetTerrainRenderingQuality>,
    mut settings: ResMut<lunco_render::RenderingQualitySettings>,
) -> Result<lunco_command_contracts::Ack, lunco_command_contracts::Reject> {
    let event = trigger.event();
    let mut candidate = *settings;
    if let Some(value) = event.tile_resolution {
        candidate.terrain_lod_tile_resolution = value;
    }
    if let Some(value) = event.cinematic_resolution {
        candidate.terrain_lod_cinematic_resolution = value;
    }
    if let Some(value) = event.pixel_error {
        candidate.terrain_lod_pixel_error = value;
    }
    if let Some(value) = event.max_depth {
        candidate.terrain_lod_max_depth = value;
    }
    if let Some(value) = event.probe_resolution {
        candidate.terrain_lod_probe_resolution = value;
    }
    if let Some(value) = event.bakes_per_frame {
        candidate.terrain_lod_bakes_per_frame = value;
    }
    if let Some(value) = event.max_inflight_bakes {
        candidate.terrain_lod_max_inflight_bakes = value;
    }
    if let Some(value) = event.tile_budget {
        candidate.terrain_lod_tile_budget = value;
    }
    if let Some(value) = event.cover_edits_per_frame {
        candidate.terrain_lod_cover_edits_per_frame = value;
    }
    if let Some(value) = event.hysteresis_ratio {
        candidate.terrain_lod_hysteresis_ratio = value;
    }
    if let Some(value) = event.morph_start_ratio {
        candidate.terrain_lod_morph_start_ratio = value;
    }
    if let Err(reason) = candidate.validate() {
        bevy::log::warn!(target: "terrain", "rejected terrain rendering quality: {reason}");
        return Err(lunco_command_contracts::Reject::InvalidOp(reason.into()));
    }
    *settings = candidate;
    Ok(lunco_command_contracts::Ack::new(
        lunco_command_contracts::OpId::new(),
    ))
}

register_commands!(on_set_terrain_rendering_quality);

/// Cap on lazily measured node geometry per terrain, keyed by quadtree node.
/// At roughly 48 bytes per entry the cap retains about 1.5 MB, above the live
/// cover and ancestor chains. Complete surface identity invalidates the cache.
const NODE_GEOMETRY_CACHE_CAP: usize = 32 * 1024;

#[derive(Component, Default)]
pub struct TerrainNodeGeometry {
    map: HashMap<QuadCoord, NodeGeometry>,
    /// `surface_key()` of the oracle the cached geometry was measured against —
    /// content identity, never the Arc pointer (allocator address reuse would
    /// keep a stale map alive).
    oracle_key: u64,
}

/// Cache of baked tile meshes keyed by quadtree node and mesh resolution (a 49²
/// streamed tile and a 2049² cinematic tile at the same coord must not collide).
/// A tile's geometry is a pure
/// function of its `QuadCoord` (deterministic DEM sampling), so a despawned tile
/// re-selected later (LOD-boundary oscillation, revisiting an area) reuses its mesh
/// handle instead of re-baking + re-uploading — the "tile caching" that was missing.
/// Upload admission enforces the graphics settings' logical GPU upload byte
/// ceiling across all sources and views. Invalidated meshes remain accounted
/// while live or retiring tile entities own them. LRU eviction releases only
/// unused handles; ready CPU bakes wait when residents leave insufficient room.
/// Value: the cached mesh handle AND the `origin_y` its vertices were rebased by at
/// bake time. `origin_y` MUST travel with the mesh — the tile is placed at it, so a
/// recompute at spawn (against a since-changed oracle, e.g. a crater layer composed
/// mid-load) would seat the mesh at a different height than it was baked for and the
/// tile would jump/jitter. Stored together, mesh and placement can never disagree.
type MeshCacheKey = (Entity, QuadCoord, usize);

/// One cached tile mesh plus its LRU/byte bookkeeping.
struct MeshCacheEntry {
    mesh: Handle<Mesh>,
    /// Surface height the mesh Y was rebased by at bake time (see the type doc).
    origin_y: f64,
    draw_bounds: Option<TerrainDrawBounds>,
    /// The cache's monotone clock at last hit/insert — the LRU stamp. Unique
    /// per touch, so eviction order is a total (deterministic) order.
    last_used: u64,
    /// Measured packed mesh bytes for retained CPU and uploaded GPU copies.
    bytes: usize,
}

#[derive(Resource, Default)]
pub struct LodMeshCache {
    map: HashMap<MeshCacheKey, MeshCacheEntry>,
    /// Monotone LRU clock, bumped on every touch.
    clock: u64,
    /// Invalidated cache identities still held by live or retiring geometry.
    retired: HashMap<AssetId<Mesh>, usize>,
    /// All live tile handles plus handles admitted or reused this publication frame.
    protected: HashSet<AssetId<Mesh>>,
}

impl LodMeshCache {
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Cache hit: return the mesh + its baked `origin_y`, refreshing the LRU stamp.
    fn get(
        &mut self,
        key: &MeshCacheKey,
    ) -> Option<(Handle<Mesh>, f64, Option<TerrainDrawBounds>)> {
        let now = self.tick();
        let entry = self.map.get_mut(key)?;
        entry.last_used = now;
        self.protected.insert(entry.mesh.id());
        Some((entry.mesh.clone(), entry.origin_y, entry.draw_bounds))
    }

    fn insert(
        &mut self,
        key: MeshCacheKey,
        mesh: Handle<Mesh>,
        origin_y: f64,
        draw_bounds: Option<TerrainDrawBounds>,
        bytes: usize,
    ) {
        let now = self.tick();
        self.protected.insert(mesh.id());
        if let Some(old) = self.map.insert(
            key,
            MeshCacheEntry {
                mesh,
                origin_y,
                draw_bounds,
                last_used: now,
                bytes,
            },
        ) {
            self.retired.insert(old.mesh.id(), old.bytes);
        }
    }

    /// Invalidated cache identities remain accounted while geometry owns them.
    fn retain_entries(&mut self, mut keep: impl FnMut(&MeshCacheKey) -> bool) {
        let retired = &mut self.retired;
        self.map.retain(|key, entry| {
            let kept = keep(key);
            if !kept {
                retired.insert(entry.mesh.id(), entry.bytes);
            }
            kept
        });
    }

    /// Drop every entry whose terrain is dead (the orphan sweep's eviction).
    pub(crate) fn retain_terrains(&mut self, keep: impl Fn(Entity) -> bool) {
        self.retain_entries(|(t, _, _)| keep(*t));
    }

    /// Drop cached meshes a live height edit invalidated, scoped to one `terrain`.
    /// Geometry is a pure function of `(terrain, coord)` only while that terrain's
    /// oracle is fixed; a brush/flatten changes its surface, so a re-selected tile
    /// in the edited area would otherwise reuse its pre-edit mesh. `Some(bounds)`
    /// drops just the overlapping nodes (the incremental patch); `None` (a whole-
    /// terrain spec change) clears the terrain's entries. Other terrains' entries —
    /// and this terrain's non-overlapping ones — survive, so revisiting unedited
    /// ground still hits the cache. Keying on the terrain `Entity` (not the coord
    /// alone) is what stops one terrain reusing another's mesh for a shared coord.
    pub fn drop_region(
        &mut self,
        terrain: Entity,
        bounds: Option<[f64; 4]>,
        root_half_extent: f64,
    ) {
        match bounds {
            None => self.retain_entries(|(e, _, _)| *e != terrain),
            Some(aabb) => self.retain_entries(|(e, coord, _)| {
                *e != terrain || !node_overlaps_aabb(*coord, root_half_extent, aabb)
            }),
        }
    }

    /// Capture every live view's mesh identities, including retiring tiles.
    fn begin_frame(&mut self, live: HashSet<AssetId<Mesh>>) {
        self.protected = live;
        self.retired.retain(|id, _| self.protected.contains(id));
    }

    fn unique_meshes(&self) -> HashMap<AssetId<Mesh>, usize> {
        let mut unique = self.retired.clone();
        for entry in self.map.values() {
            unique.insert(entry.mesh.id(), entry.bytes);
        }
        unique
    }

    fn accounted_bytes(&self) -> u64 {
        self.unique_meshes()
            .values()
            .map(|bytes| *bytes as u64)
            .sum()
    }

    /// Retire only unused cache handles. Actual global residents and handles
    /// published through deferred commands this frame always retain coverage.
    fn trim(&mut self, entry_cap: usize, byte_cap: u64) {
        let mut victims: Vec<(u64, MeshCacheKey)> = self
            .map
            .iter()
            .filter(|(_, entry)| !self.protected.contains(&entry.mesh.id()))
            .map(|(key, entry)| (entry.last_used, *key))
            .collect();
        victims.sort_unstable_by_key(|&(stamp, _)| stamp);
        for (_, key) in victims {
            if self.map.len() <= entry_cap && self.accounted_bytes() <= byte_cap {
                break;
            }
            if let Some(entry) = self.map.remove(&key) {
                debug_assert!(!self.protected.contains(&entry.mesh.id()));
            }
        }
    }

    /// Admission precedes upload. Lowering a limit can leave old residents
    /// above it; their replacement waits while ancestor geometry stays visible.
    fn admit_upload(&mut self, bytes: u64, byte_cap: u64) -> bool {
        if bytes > byte_cap {
            return false;
        }
        self.trim(usize::MAX, byte_cap.saturating_sub(bytes));
        self.accounted_bytes()
            .checked_add(bytes)
            .is_some_and(|total| total <= byte_cap)
    }

    fn publish_accounting(&self, status: &mut TerrainStreamStatus, byte_cap: u64) {
        let unique = self.unique_meshes();
        let cached = self
            .map
            .values()
            .map(|entry| entry.mesh.id())
            .collect::<HashSet<_>>();
        status.mesh_accounted_bytes = unique.values().map(|bytes| *bytes as u64).sum();
        status.mesh_cpu_bytes = status.mesh_accounted_bytes;
        status.mesh_resident_bytes = unique
            .iter()
            .filter(|(id, _)| self.protected.contains(id))
            .map(|(_, bytes)| *bytes as u64)
            .sum();
        status.mesh_cached_bytes = unique
            .iter()
            .filter(|(id, _)| cached.contains(id))
            .map(|(_, bytes)| *bytes as u64)
            .sum();
        status.mesh_retired_bytes = unique
            .iter()
            .filter(|(id, _)| !cached.contains(id))
            .map(|(_, bytes)| *bytes as u64)
            .sum();
        status.mesh_byte_limit = byte_cap;
    }
}

/// Byte estimate of one cached tile mesh as uploaded: per `res²` vertex, four
/// 12-B vec3 attributes (position, normal, morph target, morph normal), an
/// 8-B uv and a 16-B edge mask, plus `(res−1)²·6` u32 indices. A 49² tile
/// uses about 223 KiB per copy; a 2049² cinematic tile uses about 384 MiB.
/// Admission preserves the graphics setting's GPU upload semantics. The retained
/// CPU geometry is reported separately. Backend-derived resources, such as
/// path-tracing geometry, have their own owner.
#[cfg(test)]
fn tile_mesh_bytes(res: usize) -> usize {
    res * res * (4 * 12 + 8 + 16) + (res - 1) * (res - 1) * 6 * 4
}

/// Cache-entry cap derived from what streaming terrains can actually keep
/// RESIDENT — the selected cover (≤ `tile_budget`), one hidden direct-parent
/// fallback per selected leaf (worst case as many again), the coarse carpet and
/// a small stand-in allowance — ×2 retention head-room for the trailing edge.
/// The old flat `CACHE_CAP = 1024` sat BELOW the resident bound at the
/// default 768 budget, so the trim evicted exactly the entries the cache
/// exists for, every frame the camera traversed.
fn mesh_cache_entry_cap(tile_budget: usize, terrain_count: usize) -> usize {
    let resident_bound = 2 * tile_budget + coarse_fallback_tile_count() + 64;
    2 * resident_bound * terrain_count
}
// Tile insertion is immediate once the mesh and material are ready. The draw
// partition selects the deepest ready ancestor, and edge stitching makes every
// finer/coarser boundary conform to the same parent lattice. There is no
// time-dependent visibility or fabricated seam geometry in this path.

/// Result of an off-thread tile bake: the finished CPU `Mesh` (not yet uploaded)
/// plus the spawn metadata the main thread needs.
struct BakedTile {
    mesh: Mesh,
    center: [f64; 2],
    depth: u32,
    /// Verts per side the mesh was baked at — part of the mesh-cache key.
    res: usize,
    /// Surface height at the tile centre the mesh Y was rebased by (see `LodMeshCache`).
    origin_y: f64,
    draw_bounds: Option<TerrainDrawBounds>,
    bytes: usize,
}

/// Finite local hull of both exact fine and parent-morph postings. Every
/// distance morph and stitched edge lies inside this convex axis-aligned hull.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct TerrainDrawBounds {
    pub min: DVec3,
    pub max: DVec3,
    /// Componentwise displacement envelope of every fine-to-parent posting.
    pub parent_displacement: DVec3,
}

fn terrain_draw_bounds(fine: &[[f32; 3]], parent: &[[f32; 3]]) -> Option<TerrainDrawBounds> {
    if fine.is_empty() || parent.len() != fine.len() {
        return None;
    }
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    let mut parent_displacement = DVec3::ZERO;
    for (fine, parent) in fine.iter().zip(parent) {
        let fine = Vec3::from_array(*fine).as_dvec3();
        let parent = Vec3::from_array(*parent).as_dvec3();
        if !fine.is_finite() || !parent.is_finite() {
            return None;
        }
        min = min.min(fine).min(parent);
        max = max.max(fine).max(parent);
        parent_displacement = parent_displacement.max((parent - fine).abs());
    }
    Some(TerrainDrawBounds {
        min,
        max,
        parent_displacement,
    })
}

fn measured_mesh_bytes(mesh: &Mesh) -> usize {
    mesh.get_vertex_buffer_size() + mesh.get_index_buffer_bytes().map_or(0, |bytes| bytes.len())
}

fn assemble_baked_tile(
    tile_mesh: crate::tile_mesh::TileMesh,
    oracle: &SurfaceOracle,
    center: [f64; 2],
    depth: u32,
    resolution: usize,
) -> BakedTile {
    let draw_bounds = terrain_draw_bounds(&tile_mesh.positions, &tile_mesh.morph_targets);
    let usage = bevy::asset::RenderAssetUsages::default();
    let mut mesh = grid_mesh(
        tile_mesh.positions,
        tile_mesh.normals,
        tile_mesh.uvs,
        tile_mesh.indices,
        usage,
    );
    mesh.insert_attribute(ATTRIBUTE_MORPH_TARGET, tile_mesh.morph_targets);
    mesh.insert_attribute(ATTRIBUTE_MORPH_NORMAL, tile_mesh.morph_normals);
    mesh.insert_attribute(ATTRIBUTE_MORPH_EDGE, tile_mesh.edge_masks);
    let bytes = measured_mesh_bytes(&mesh);
    BakedTile {
        mesh,
        center,
        depth,
        res: resolution,
        origin_y: oracle.height_at(center[0], center[1]),
        draw_bounds,
        bytes,
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct PendingTileBake {
    generation: u32,
    key: lunco_core_runtime::AsyncWorkKey,
}

/// In-flight off-thread tile bakes for a terrain, keyed by quadtree node. The CPU
/// bake and grid-mesh assembly run under the shared bounded preparation admission;
/// the main thread only uploads the finished mesh + spawns the entity. Browser
/// builds keep their explicit asynchronous cache path until worker transport exists.
#[derive(Component, Default)]
pub struct PendingTileBakes {
    #[cfg(not(target_arch = "wasm32"))]
    jobs: HashMap<QuadCoord, PendingTileBake>,
    #[cfg(target_arch = "wasm32")]
    tasks: HashMap<QuadCoord, (u32, Task<BakedTile>)>,
    ready: Vec<(QuadCoord, u32, BakedTile)>,
    #[cfg(not(target_arch = "wasm32"))]
    next_operation: u64,
    #[cfg(not(target_arch = "wasm32"))]
    capacity_wait_revision: Option<u64>,
    failed: HashSet<(QuadCoord, u32)>,
}

impl PendingTileBakes {
    fn len(&self) -> usize {
        #[cfg(not(target_arch = "wasm32"))]
        let jobs = self.jobs.len();
        #[cfg(target_arch = "wasm32")]
        let jobs = self.tasks.len();
        jobs + self.ready.len()
    }

    fn is_empty(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        let jobs_empty = self.jobs.is_empty();
        #[cfg(target_arch = "wasm32")]
        let jobs_empty = self.tasks.is_empty();
        jobs_empty && self.ready.is_empty()
    }

    fn has_current(&self, coord: QuadCoord, generation: u32) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        let has_job = self
            .jobs
            .get(&coord)
            .is_some_and(|job| job.generation == generation);
        #[cfg(target_arch = "wasm32")]
        let has_job = self
            .tasks
            .get(&coord)
            .is_some_and(|(pending_generation, _)| *pending_generation == generation);
        has_job
            || self
                .ready
                .iter()
                .any(|(pending_coord, pending_generation, _)| {
                    *pending_coord == coord && *pending_generation == generation
                })
    }

    fn take_ready(&mut self, generation: u32, budget: usize) -> Vec<(QuadCoord, BakedTile)> {
        self.ready
            .retain(|(_, ready_generation, _)| *ready_generation == generation);
        self.ready.sort_by_key(|(coord, ready_generation, _)| {
            (coord.depth, coord.x, coord.z, *ready_generation)
        });
        let commit_count = self.ready.len().min(budget);
        self.ready
            .drain(..commit_count)
            .map(|(coord, _, baked)| (coord, baked))
            .collect()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn accept_completed_bakes(
        &mut self,
        terrain: Entity,
        completions: impl IntoIterator<Item = TerrainTileBakeCompletion>,
    ) {
        for completion in completions {
            let generation = self
                .jobs
                .get(&completion.coord)
                .filter(|job| job.key == completion.key)
                .map(|job| job.generation);
            let Some(generation) = generation else {
                continue;
            };
            self.jobs.remove(&completion.coord);
            match completion.result {
                Ok(baked) => self.ready.push((completion.coord, generation, baked)),
                Err(()) => {
                    self.failed.insert((completion.coord, generation));
                    bevy::log::error!(
                        target: "terrain_stream",
                        entity = ?terrain,
                        coord = ?completion.coord,
                        generation,
                        "terrain tile bake panicked; the tile is held until its surface changes"
                    );
                }
            }
        }
    }

    /// Drop in-flight bakes whose tile overlaps the committed surface change.
    /// A bounded change leaves pending work elsewhere valid; retag it to the
    /// new visual generation so the completed mesh can still be published.
    pub(crate) fn apply_surface_change(
        &mut self,
        change: TerrainSurfaceChange,
        generation: Option<u32>,
        root_half_extent: f64,
        admission: Option<&mut lunco_core_runtime::AsyncWorkAdmission>,
    ) {
        #[cfg(not(target_arch = "wasm32"))]
        let mut admission = admission;
        let Some(generation) = generation else {
            self.cancel_jobs(admission);
            self.ready.clear();
            self.failed.clear();
            return;
        };
        let Some(bounds) = change.dirty_bounds else {
            self.cancel_jobs(admission);
            self.ready.clear();
            self.failed.clear();
            return;
        };

        #[cfg(not(target_arch = "wasm32"))]
        self.jobs.retain(|coord, job| {
            if node_overlaps_aabb(*coord, root_half_extent, bounds) {
                if let Some(admission) = admission.as_deref_mut() {
                    admission.cancel_queued(job.key);
                }
                false
            } else {
                job.generation = generation;
                true
            }
        });
        #[cfg(target_arch = "wasm32")]
        self.tasks.retain(|coord, (pending_generation, _)| {
            if node_overlaps_aabb(*coord, root_half_extent, bounds) {
                false
            } else {
                *pending_generation = generation;
                true
            }
        });
        self.ready.retain_mut(|(coord, pending_generation, _)| {
            if node_overlaps_aabb(*coord, root_half_extent, bounds) {
                false
            } else {
                *pending_generation = generation;
                true
            }
        });
        self.failed
            .retain(|(coord, _)| !node_overlaps_aabb(*coord, root_half_extent, bounds));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut retagged = HashSet::with_capacity(self.failed.len());
            for (coord, _) in self.failed.drain() {
                retagged.insert((coord, generation));
            }
            self.failed = retagged;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.capacity_wait_revision = None;
        }
    }

    fn cancel_jobs(&mut self, admission: Option<&mut lunco_core_runtime::AsyncWorkAdmission>) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut admission = admission;
            for job in self.jobs.values() {
                if let Some(admission) = admission.as_deref_mut() {
                    admission.cancel_queued(job.key);
                }
            }
            self.jobs.clear();
            self.capacity_wait_revision = None;
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = admission;
            self.tasks.clear();
        }
        self.ready.clear();
    }

    fn cancel_for_missing_demand(
        &mut self,
        admission: &mut lunco_core_runtime::AsyncWorkAdmission,
    ) {
        #[cfg(not(target_arch = "wasm32"))]
        self.cancel_jobs(Some(admission));
        #[cfg(target_arch = "wasm32")]
        {
            let _ = admission;
            self.cancel_jobs(None);
        }
    }
}

/// Apply one committed surface change to every visual cache owned by the
/// streamer. Both native restamps and progressive worker upgrades use this
/// boundary so tile generations, pending tasks, and cached meshes cannot drift.
pub(crate) fn invalidate_visual_products(
    commands: &mut Commands,
    terrain: Entity,
    change: TerrainSurfaceChange,
    root_half_extent: f64,
    tiles: Option<Mut<LodTiles>>,
    pending: Option<Mut<PendingTileBakes>>,
    admission: Option<&mut lunco_core_runtime::AsyncWorkAdmission>,
    mesh_cache: &mut LodMeshCache,
) {
    let generation = if let Some(mut tiles) = tiles {
        for entity in tiles.reap_stale() {
            commands.entity(entity).try_despawn();
        }
        tiles.invalidate_region(change.dirty_bounds, root_half_extent);
        Some(tiles.r#gen)
    } else {
        None
    };
    if let Some(mut pending) = pending {
        pending.apply_surface_change(change, generation, root_half_extent, admission);
    }
    mesh_cache.drop_region(terrain, change.dirty_bounds, root_half_extent);
}

/// Cached terrain self-shadow for streamed tiles. Native terrain casting and
/// receiving remain enabled: the cache only shades the ground, while native
/// shadow maps also carry terrain occlusion onto vehicles and equipment.
/// The last committed image remains active until its replacement is published.
#[derive(Component, Clone)]
pub struct TileShadowCache {
    pub image: Handle<Image>,
    pub on: f32,
}

/// Write one named parameter into a look, reusing the existing slot when the key is
/// already present so the hot re-write path (the overlay sync)
/// doesn't allocate a `String` per call.
pub(crate) fn set_param(look: &mut ShaderLook, name: &str, v: ParamValue) {
    if look.values().get(name) != Some(&v) {
        look.set_value(name, v);
    }
}

/// Bind a terrain's far-shadow cache onto one streamed tile look.
pub(crate) fn apply_shadow_cache_to_look(look: &mut ShaderLook, cache: &TileShadowCache) {
    set_texture(
        look,
        TextureLayer::ShadowCache,
        Some(&cache.image.clone().into()),
    );
    set_param(look, "shadow_cache_on", ParamValue::F32(cache.on));
    // Cast intent remains the USD-authored look. The ground visibility cache
    // does not replace native terrain-to-object casting.
}

fn set_texture(
    look: &mut ShaderLook,
    layer: TextureLayer,
    handle: Option<&lunco_materials::ShaderTexture>,
) {
    match handle {
        Some(handle) if look.textures.get(&layer) != Some(handle) => {
            look.textures.insert(layer, handle.clone());
        }
        None if look.textures.contains_key(&layer) => {
            look.textures.remove(&layer);
        }
        _ => {}
    }
}

/// Bind one terrain's complete material source selection onto a streamed tile.
///
/// USD owns authored layer facts. The engine-derived maps are a per-role
/// material source for roles USD did not author, so the two sources share the
/// existing fixed texture slots without ever overwriting one another. The
/// source decision is made once when the look is built and repeated only when
/// the terrain's published map components change.
///
/// The map handles are part of `ShaderLook::key()`, so two terrains with
/// different material sources correctly get different materials, while every
/// physical-appearance parameter is identical across the LODs of one terrain.
pub(crate) fn apply_terrain_maps_to_look(
    look: &mut ShaderLook,
    maps: Option<&TerrainDerivedMaps>,
    authored: Option<&TerrainAuthoredMaps>,
) {
    let regional_albedo = authored.and_then(|maps| maps.regional_albedo.as_ref());
    set_texture(look, TextureLayer::RegionalAlbedo, regional_albedo);
    let registered = matches!(look.values().get("regional_uv_scale"), Some(ParamValue::F32(scale)) if *scale > 0.0);
    set_param(
        look,
        "regional_albedo_on",
        ParamValue::F32(if registered && regional_albedo.is_some() {
            1.0
        } else {
            0.0
        }),
    );
    let authored_surface = authored.is_some_and(TerrainAuthoredMaps::has_active_surface);
    let authored_normal = authored.is_some_and(TerrainAuthoredMaps::has_active_normal);

    let surface = if authored_surface {
        authored.and_then(|maps| maps.surface.clone())
    } else {
        maps.map(|maps| maps.surface.clone().into())
    };
    set_texture(look, TextureLayer::Surface, surface.as_ref());
    let normal = if authored_normal {
        authored.and_then(|maps| maps.normal.clone())
    } else {
        maps.map(|maps| maps.normal.clone().into())
    };
    set_texture(look, TextureLayer::Normal, normal.as_ref());

    set_param(
        look,
        "map_texel_size_m",
        ParamValue::F32(maps.map_or(1.0, |maps| maps.texel_size_m)),
    );
    set_param(
        look,
        "derived_surface_on",
        ParamValue::F32(if maps.is_some() && !authored_surface {
            1.0
        } else {
            0.0
        }),
    );
    set_param(
        look,
        "derived_normal_on",
        ParamValue::F32(if maps.is_some() && !authored_normal {
            1.0
        } else {
            0.0
        }),
    );
    set_param(
        look,
        "authored_surface_on",
        ParamValue::F32(if authored_surface { 1.0 } else { 0.0 }),
    );
    set_param(
        look,
        "authored_normal_on",
        ParamValue::F32(if authored_normal { 1.0 } else { 0.0 }),
    );
    set_param(
        look,
        "weight_rough",
        ParamValue::F32(if authored_surface {
            authored.map_or(0.0, |maps| maps.weight_rough)
        } else {
            0.0
        }),
    );
    set_param(
        look,
        "weight_ao",
        ParamValue::F32(if authored_surface {
            authored.map_or(0.0, |maps| maps.weight_ao)
        } else {
            0.0
        }),
    );
    set_param(
        look,
        "weight_normal",
        ParamValue::F32(if authored_normal {
            authored.map_or(0.0, |maps| maps.weight_normal)
        } else {
            0.0
        }),
    );

    match authored.and_then(|maps| maps.albedo.as_ref()) {
        Some(h) => {
            set_texture(look, TextureLayer::Albedo, Some(h));
            set_param(
                look,
                "weight_albedo",
                ParamValue::F32(authored.map_or(0.0, |maps| maps.weight_albedo)),
            );
        }
        None => {
            set_texture(look, TextureLayer::Albedo, None);
            set_param(look, "weight_albedo", ParamValue::F32(0.0));
        }
    }
}

/// Compose only the site-local inputs into the USD-selected body shader.
/// Mission DEM geometry and colliders do not pass through this render-only
/// material operation.
fn lunar_surface_continuation_look(
    surface: &ShaderLook,
    dem: &ShaderLook,
    dem_source: &lunco_materials::ShaderLookSourceInterface,
    derived: Option<&TerrainDerivedMaps>,
    authored: Option<&TerrainAuthoredMaps>,
    half_extent_m: f64,
    collar_widths_m: [f64; 4],
) -> Result<ShaderLook, &'static str> {
    if surface.interface.as_deref() != Some(LUNAR_SURFACE_CONTINUATION_INTERFACE) {
        return Err("USD-selected body shader does not declare the continuation interface");
    }
    if !half_extent_m.is_finite()
        || half_extent_m <= 0.0
        || collar_widths_m
            .iter()
            .any(|width| !width.is_finite() || *width <= 0.0)
    {
        return Err("continuation extent and width must be finite and positive");
    }
    let terrain_half_extent = half_extent_m as f32;
    let terrain_diameter = (2.0 * half_extent_m) as f32;
    let collar_widths = collar_widths_m.map(|width| width as f32);
    if !terrain_half_extent.is_finite()
        || !terrain_diameter.is_finite()
        || collar_widths
            .iter()
            .any(|width| !width.is_finite() || *width <= 0.0)
    {
        return Err("continuation dimensions exceed rendering precision");
    }

    let color = dem
        .values()
        .get("albedo")
        .or_else(|| dem_source.defaults.get("albedo"));
    // USD colours are carried as RGB or opaque RGBA; the shader consumes RGB.
    let color = match color {
        Some(ParamValue::Vec3([r, g, b]) | ParamValue::Vec4([r, g, b, _])) => [*r, *g, *b],
        _ => return Err("DEM material must provide a reflected albedo colour"),
    };
    if color.iter().any(|value| !value.is_finite() || *value < 0.0) {
        return Err("DEM material albedo must be finite and nonnegative");
    }
    let mut photometry = [0.0; 4];
    for (index, name) in ["surge_amp", "surge_width", "photometry_gain"]
        .into_iter()
        .enumerate()
    {
        let Some(ParamValue::F32(value)) = dem
            .values()
            .get(name)
            .or_else(|| dem_source.defaults.get(name))
        else {
            return Err("DEM material must provide reflected lunar photometry parameters");
        };
        if !value.is_finite() || *value < 0.0 || (index == 1 && *value <= 0.0) {
            return Err("DEM material lunar photometry parameters are invalid");
        }
        photometry[index] = *value;
    }
    let mut look = surface.clone();
    look.set_value("site_photometry", ParamValue::Vec4(photometry));
    look.set_value(
        "site_base_color",
        ParamValue::Vec4([color[0], color[1], color[2], 1.0]),
    );
    look.set_value("terrain_half_extent", ParamValue::F32(terrain_half_extent));
    look.set_value("site_blend_widths_m", ParamValue::Vec4(collar_widths));

    let site_albedo = authored.and_then(|maps| maps.albedo.as_ref());
    if let Some(image) = site_albedo {
        look.textures
            .insert(TextureLayer::ContinuationAlbedo, image.clone());
    } else {
        look.textures.remove(&TextureLayer::ContinuationAlbedo);
    }
    let albedo_weight = site_albedo
        .filter(|_| authored.is_some_and(|maps| maps.weight_albedo > 0.0))
        .map_or(0.0, |_| authored.map_or(0.0, |maps| maps.weight_albedo));
    look.set_value("site_weight_albedo", ParamValue::F32(albedo_weight));

    let authored_surface = authored.filter(|maps| maps.has_active_surface());
    let derived_surface = derived.filter(|_| authored_surface.is_none());
    let surface_texture = authored_surface
        .and_then(|maps| maps.surface.clone())
        .or_else(|| derived_surface.map(|maps| maps.surface.clone().into()));
    if let Some(image) = surface_texture {
        look.textures
            .insert(TextureLayer::ContinuationSurface, image.clone());
    } else {
        look.textures.remove(&TextureLayer::ContinuationSurface);
    }
    let (weight_rough, weight_ao) = if let Some(authored) = authored_surface {
        (authored.weight_rough, authored.weight_ao)
    } else if derived_surface.is_some() {
        (1.0, 1.0)
    } else {
        (0.0, 0.0)
    };
    look.set_value("site_weight_rough", ParamValue::F32(weight_rough));
    look.set_value("site_weight_ao", ParamValue::F32(weight_ao));
    look.set_value(
        "site_authored_surface_on",
        ParamValue::F32(if authored_surface.is_some() { 1.0 } else { 0.0 }),
    );
    Ok(look)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LunarSurfaceContinuationAction {
    Compose,
    Hold,
    Wait,
}

impl LunarSurfaceContinuationAction {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "compose" => Some(Self::Compose),
            "hold" => Some(Self::Hold),
            "wait" => Some(Self::Wait),
            _ => None,
        }
    }
}

fn continuation_policy_action(
    surface: &ShaderLook,
    source: Option<&lunco_materials::ShaderLookSourceInterface>,
    context: lunco_core::RuntimeExecutionContext,
) -> Result<LunarSurfaceContinuationAction, String> {
    let reflected = source.is_some_and(|source| source.shader == surface.shader);
    let declared_interface = surface.interface.as_deref().unwrap_or("");
    let actual_interface = source
        .filter(|source| source.shader == surface.shader)
        .and_then(|source| source.identifier.as_deref())
        .unwrap_or("");
    let facts = lunco_hooks::HookValue::map([
        (
            "expected_interface",
            lunco_hooks::HookValue::str(LUNAR_SURFACE_CONTINUATION_INTERFACE),
        ),
        (
            "declared_interface",
            lunco_hooks::HookValue::str(declared_interface),
        ),
        (
            "actual_interface",
            lunco_hooks::HookValue::str(actual_interface),
        ),
        ("source_reflected", lunco_hooks::HookValue::Bool(reflected)),
        (
            "source_valid",
            lunco_hooks::HookValue::Bool(
                source.is_some_and(|source| source.shader == surface.shader && source.source_valid),
            ),
        ),
        (
            "shader_path",
            lunco_hooks::HookValue::str(surface.shader.clone()),
        ),
    ]);
    let value =
        lunco_hooks::invoke_with_context(LUNAR_SURFACE_CONTINUATION_HOOK, &[facts], context)
            .ok_or_else(|| "continuation decision policy is not installed".to_owned())?
            .map_err(|error| format!("continuation decision policy failed: {error}"))?;
    let action = value
        .get("action")
        .and_then(lunco_hooks::HookValue::as_str)
        .and_then(LunarSurfaceContinuationAction::parse)
        .ok_or_else(|| {
            format!(
                "continuation decision policy returned invalid action in {}",
                value.type_name()
            )
        })?;
    Ok(action)
}

fn continuation_runtime_context(
    scene_transition: Option<&lunco_core::SceneTransitionCoordinator>,
) -> lunco_core::RuntimeExecutionContext {
    let route = scene_transition
        .and_then(lunco_core::SceneTransitionCoordinator::lifecycle_generation)
        .map(|generation| {
            lunco_core::RuntimeRoute::twin(lunco_core::RuntimeCycle::Visualization, generation)
        })
        .unwrap_or_else(|| {
            lunco_core::RuntimeRoute::application(lunco_core::RuntimeCycle::Visualization)
        });
    lunco_core::RuntimeExecutionContext {
        target_twin_id: None,
        route: Some(route),
        phase: lunco_core::RuntimePhase::Visualization,
        clock: lunco_core::RuntimeClock::None,
        time_seconds: None,
        delta_seconds: None,
        sequence: None,
        producer: None,
    }
}

fn raise_continuation_fault(
    faults: &mut lunco_core::RuntimeFaults,
    target: Entity,
    surface: &ShaderLook,
    detail: impl Into<String>,
) {
    faults.raise(
        "terrain-visual-continuation",
        Some(target),
        surface.shader.clone(),
        detail,
    );
}

fn bind_continuation_look(
    commands: &mut Commands,
    target: Entity,
    current: Option<&ShaderLook>,
    visibility: &mut Visibility,
    shader_ready: bool,
    admitted: bool,
    look: &ShaderLook,
) {
    if current != Some(look) {
        commands.entity(target).insert(look.clone());
    }
    *visibility = if admitted && shader_ready && current == Some(look) {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
}

/// Keep generated terrain continuation meshes on the USD-selected body shader
/// path. Source reflection, authored maps, and shader revisions are processed
/// only when their owner publishes a change; no LOD work runs on the physics
/// cadence, and no worker completion chooses the visual policy action.
pub(crate) fn sync_terrain_visual_continuations(
    mut commands: Commands,
    mut removed_derived: RemovedComponents<TerrainDerivedMaps>,
    mut removed_authored: RemovedComponents<TerrainAuthoredMaps>,
    mut removed_looks: RemovedComponents<ShaderLook>,
    mut removed_interfaces: RemovedComponents<lunco_materials::ShaderLookSourceInterface>,
    mut removed_shader_ready: RemovedComponents<ShaderLookReady>,
    mut removed_terrain: RemovedComponents<DemTerrainSurface>,
    terrain_sources: Query<
        (
            &ShaderLook,
            Option<&TerrainDerivedMaps>,
            Option<&TerrainAuthoredMaps>,
            Option<&lunco_materials::ShaderLookSourceInterface>,
        ),
        With<DemTerrainSurface>,
    >,
    changed_terrain_sources: Query<
        Entity,
        (
            With<DemTerrainSurface>,
            Or<(
                Changed<ShaderLook>,
                Changed<TerrainDerivedMaps>,
                Changed<TerrainAuthoredMaps>,
                Changed<lunco_materials::ShaderLookSourceInterface>,
            )>,
        ),
    >,
    mut targets: Query<
        (
            Entity,
            &TerrainVisualContinuation,
            Option<&ShaderLook>,
            Option<&lunco_materials::ShaderLookSourceInterface>,
            &mut Visibility,
            Has<ShaderLookReady>,
        ),
        With<Mesh3d>,
    >,
    changed_interfaces: Query<Entity, Changed<lunco_materials::ShaderLookSourceInterface>>,
    changed_targets: Query<
        Entity,
        (
            With<TerrainVisualContinuation>,
            Or<(Changed<TerrainVisualContinuation>, Changed<ShaderLook>)>,
        ),
    >,
    changed_shader_ready: Query<Entity, Changed<ShaderLookReady>>,
    scene_transition: Option<Res<lunco_core::SceneTransitionCoordinator>>,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
) {
    let retired_terrain: HashSet<Entity> = removed_terrain.read().collect();
    let mut changed_terrain_entities: HashSet<Entity> = changed_terrain_sources.iter().collect();
    changed_terrain_entities.extend(retired_terrain.iter().copied());
    changed_terrain_entities.extend(removed_derived.read());
    changed_terrain_entities.extend(removed_authored.read());
    changed_terrain_entities.extend(removed_looks.read());
    let mut changed_interface_entities: HashSet<Entity> = changed_interfaces.iter().collect();
    changed_interface_entities.extend(removed_interfaces.read());
    changed_terrain_entities.extend(changed_interface_entities.iter().copied());
    let changed_target_entities: HashSet<Entity> = changed_targets.iter().collect();
    let mut changed_readiness_entities: HashSet<Entity> = changed_shader_ready.iter().collect();
    changed_readiness_entities.extend(removed_shader_ready.read());
    if changed_terrain_entities.is_empty()
        && changed_interface_entities.is_empty()
        && changed_target_entities.is_empty()
        && changed_readiness_entities.is_empty()
    {
        return;
    }

    let mut changed = targets
        .iter_mut()
        .filter(|(target, continuation, _, _, _, _)| {
            changed_target_entities.contains(target)
                || changed_readiness_entities.contains(target)
                || changed_terrain_entities.contains(&continuation.source)
                || changed_interface_entities.contains(target)
        })
        .map(
            |(target, continuation, current, reflected, visibility, shader_ready)| {
                (
                    target,
                    continuation.clone(),
                    current.cloned(),
                    reflected.cloned(),
                    visibility,
                    shader_ready,
                )
            },
        )
        .collect::<Vec<_>>();
    changed.sort_by_key(|(entity, _, _, _, _, _)| entity.to_bits());
    let runtime_context = continuation_runtime_context(scene_transition.as_deref());

    for (target, continuation, current, reflected_interface, mut visibility, shader_ready) in
        changed
    {
        // The DEM owner may retire while the globe reconciler is still
        // removing its handoff. Retire this derived binding immediately;
        // it no longer has a scene source and must not publish a fault.
        if retired_terrain.contains(&continuation.source) {
            *visibility = Visibility::Hidden;
            commands
                .entity(target)
                .remove::<TerrainVisualContinuation>();
            continue;
        }
        let surface_look = &continuation.surface_look;
        let reflected_interface = reflected_interface.as_ref();
        let policy_action =
            match continuation_policy_action(surface_look, reflected_interface, runtime_context) {
                Ok(action) => action,
                Err(reason) => {
                    bind_continuation_look(
                        &mut commands,
                        target,
                        current.as_ref(),
                        &mut visibility,
                        shader_ready,
                        false,
                        &continuation.surface_look,
                    );
                    raise_continuation_fault(&mut faults, target, surface_look, reason);
                    continue;
                }
            };
        let reflected =
            reflected_interface.is_some_and(|source| source.shader == surface_look.shader);
        let declared = surface_look.interface.as_deref();
        let actual = reflected_interface
            .filter(|source| source.shader == surface_look.shader)
            .and_then(|source| source.identifier.as_deref());

        let selected_look = match policy_action {
            LunarSurfaceContinuationAction::Wait if !reflected => surface_look.clone(),
            LunarSurfaceContinuationAction::Wait => {
                bind_continuation_look(
                    &mut commands,
                    target,
                    current.as_ref(),
                    &mut visibility,
                    shader_ready,
                    false,
                    &continuation.surface_look,
                );
                raise_continuation_fault(
                    &mut faults,
                    target,
                    surface_look,
                    "the Rhai policy returned wait after the shader source was already reflected",
                );
                continue;
            }
            LunarSurfaceContinuationAction::Compose
                if reflected
                    && reflected_interface.is_some_and(|source| source.source_valid)
                    && declared == Some(LUNAR_SURFACE_CONTINUATION_INTERFACE)
                    && actual == Some(LUNAR_SURFACE_CONTINUATION_INTERFACE) =>
            {
                let (dem_look, maps, authored, dem_source) = match terrain_sources
                    .get(continuation.source)
                {
                    Ok(source) => source,
                    Err(error) => {
                        bind_continuation_look(
                            &mut commands,
                            target,
                            current.as_ref(),
                            &mut visibility,
                            shader_ready,
                            false,
                            &continuation.surface_look,
                        );
                        raise_continuation_fault(
                            &mut faults,
                            target,
                            surface_look,
                            format!(
                                "terrain continuation {target:?} cannot read its DEM source {:?}: {error}. Simulation is stopped. Fix the source asset or use RestartScene to reload the complete scene.",
                                continuation.source
                            ),
                        );
                        continue;
                    }
                };
                let Some(dem_source) = dem_source
                    .filter(|source| source.shader == dem_look.shader && source.source_valid)
                else {
                    *visibility = Visibility::Hidden;
                    continue;
                };
                let inferred_authored = authored
                    .is_none()
                    .then(|| TerrainAuthoredMaps::from_shader_look(dem_look, maps));
                let authored = authored.or(inferred_authored.as_ref());
                match lunar_surface_continuation_look(
                    surface_look,
                    dem_look,
                    dem_source,
                    maps,
                    authored,
                    continuation.half_extent_m,
                    continuation.collar_widths_m,
                ) {
                    Ok(look) => look,
                    Err(reason) => {
                        bind_continuation_look(
                            &mut commands,
                            target,
                            current.as_ref(),
                            &mut visibility,
                            shader_ready,
                            false,
                            &continuation.surface_look,
                        );
                        raise_continuation_fault(&mut faults, target, surface_look, reason);
                        continue;
                    }
                }
            }
            LunarSurfaceContinuationAction::Compose => {
                bind_continuation_look(
                    &mut commands,
                    target,
                    current.as_ref(),
                    &mut visibility,
                    shader_ready,
                    false,
                    &continuation.surface_look,
                );
                raise_continuation_fault(
                    &mut faults,
                    target,
                    surface_look,
                    "the Rhai policy selected compose for an incompatible USD/WESL interface",
                );
                continue;
            }
            LunarSurfaceContinuationAction::Hold => {
                raise_continuation_fault(
                    &mut faults,
                    target,
                    surface_look,
                    format!(
                        "continuation interface mismatch: USD declares {:?}, WESL reflects {:?}",
                        declared, actual
                    ),
                );
                surface_look.clone()
            }
        };

        bind_continuation_look(
            &mut commands,
            target,
            current.as_ref(),
            &mut visibility,
            shader_ready,
            policy_action == LunarSurfaceContinuationAction::Compose,
            &selected_look,
        );
    }
}

/// The first streamed Lit cover is admitted only after the USD material
/// projection has completed and every role has a stable source. A terrain with
/// both surface and normal roles authored by USD does not need the derived
/// product; any missing role does.
fn initial_lit_material_ready(
    maps: Option<&TerrainDerivedMaps>,
    authored: Option<&TerrainAuthoredMaps>,
) -> bool {
    let Some(authored) = authored else {
        return false;
    };
    maps.is_some() || !authored.needs_derived_maps()
}

/// The appearance intent of one LOD tile: the owning terrain's authored shader
/// source, its morph band, material maps, and far-shadow cache. The diagnostic
/// tool temporarily replaces this look with its own authored diagnostic shader.
///
/// THE SHARING CONTRACT: two tiles in the same band bucket produce an EQUAL
/// `ShaderLook::key()`, so `lunco-render-bevy` hands them the same
/// `ShaderMaterial` handle — one bind group, one batch. This is the property the
/// hand-rolled `MatKey`/`LodMaterials` cache existed for; it is now a consequence of
/// the look's content rather than a table anyone has to remember to consult.
/// Anything that varies per-tile (a raw `morph_end` instead of the snapped band, an
/// un-bucketed value) would mint a material per tile and destroy batching — which is
/// exactly why the band is snapped before it lands here.
pub(crate) fn tile_look(
    template: &ShaderLook,
    depth: u32,
    morph_start: f32,
    morph_end: f32,
    maps: Option<&TerrainDerivedMaps>,
    terrain_half_extent: f32,
    regional_map_blend_m: f32,
    regional_uv_scale: f32,
    authored: Option<&TerrainAuthoredMaps>,
    shadow: Option<&TileShadowCache>,
    diagnostic: &ShaderLook,
    overlay: crate::overlay::OverlayUniforms,
) -> ShaderLook {
    if overlay.mode > 0.5 {
        let mut look = diagnostic.clone();
        if let Some(image) = template.textures.get(&TextureLayer::SurfaceAnnotations) {
            look.textures
                .insert(TextureLayer::SurfaceAnnotations, image.clone());
        }
        set_param(
            &mut look,
            "terrain_half_extent",
            ParamValue::F32(terrain_half_extent),
        );
        assert!(
            !look.shader.is_empty(),
            "terrain diagnostic requires an authored fragment shader source"
        );
        assert!(
            look.vertex_shader.is_some(),
            "terrain diagnostic requires an authored vertex shader source"
        );
        set_param(&mut look, "morph_start", ParamValue::F32(morph_start));
        set_param(&mut look, "morph_end", ParamValue::F32(morph_end));
        set_param(
            &mut look,
            "stitch_edges",
            ParamValue::Vec4([0.0, 0.0, 0.0, 0.0]),
        );
        set_param(&mut look, "mode", ParamValue::F32(overlay.mode));
        set_param(&mut look, "opacity", ParamValue::F32(overlay.opacity));
        set_param(&mut look, "safe_rad", ParamValue::F32(overlay.safe_rad));
        set_param(&mut look, "cliff_rad", ParamValue::F32(overlay.cliff_rad));
        set_param(&mut look, "lod", ParamValue::F32(depth as f32));
        return look;
    }

    let mut look = template.clone();
    assert!(
        !look.shader.is_empty(),
        "a streamed terrain must provide an authored fragment shader source"
    );
    assert!(
        look.vertex_shader.is_some(),
        "a streamed terrain must provide an authored CDLOD vertex shader source"
    );
    set_param(&mut look, "morph_start", ParamValue::F32(morph_start));
    set_param(&mut look, "morph_end", ParamValue::F32(morph_end));
    set_param(
        &mut look,
        "stitch_edges",
        ParamValue::Vec4([0.0, 0.0, 0.0, 0.0]),
    );
    set_param(
        &mut look,
        "terrain_half_extent",
        ParamValue::F32(terrain_half_extent),
    );
    if regional_map_blend_m > 0.0 {
        set_param(
            &mut look,
            "site_blend_widths_m",
            ParamValue::Vec4([regional_map_blend_m; 4]),
        );
    }
    set_param(
        &mut look,
        "regional_uv_scale",
        ParamValue::F32(regional_uv_scale),
    );
    // Every streamed tile is sampled from the terrain's composed DEM oracle.
    // Keep the relief-source fact on the look itself so the canonical fragment
    // does not guess from whichever derived map happens to be available.
    set_param(&mut look, "terrain_geometry_on", ParamValue::F32(1.0));
    apply_terrain_maps_to_look(&mut look, maps, authored);
    if let Some(shadow) = shadow {
        apply_shadow_cache_to_look(&mut look, shadow);
    }
    look
}

/// Spawn one LOD tile entity (mesh + its `ShaderLook` intent, anchored to its own
/// big_space cell). Shared by the cache-hit and finalized-bake paths.
#[allow(clippy::too_many_arguments)]
fn spawn_tile(
    commands: &mut Commands,
    grid: &Grid,
    grid_entity: Entity,
    terrain: Entity,
    view_camera: Entity,
    render_layer: usize,
    coord: QuadCoord,
    mesh: Handle<Mesh>,
    draw_bounds: Option<TerrainDrawBounds>,
    center: [f64; 2],
    depth: u32,
    morph_end: f32,
    morph_ratio: f32,
    maps: Option<&TerrainDerivedMaps>,
    terrain_half_extent: f32,
    regional_map_blend_m: f32,
    regional_uv_scale: f32,
    authored: Option<&TerrainAuthoredMaps>,
    shadow: Option<&TileShadowCache>,
    template: &ShaderLook,
    diagnostic: &ShaderLook,
    overlay: crate::overlay::OverlayUniforms,
    // Surface height at the tile centre — the SAME value the mesh was rebased by in
    // `bake_tile_mesh` (`origin_y`). Anchoring the tile's `CellCoord` here (not Y=0)
    // keeps the tile in the SAME big_space cell as the content standing on it, and its
    // rebased geometry local to that origin. On flat terrain this is ≈0 (unchanged).
    origin_y: f64,
    // Pose of the DEM owner's local frame directly in the receiving grid. Keeping
    // this conversion local avoids subtracting two large root-frame positions
    // when a celestial surface grid is rotating or being re-pinned.
    terrain_grid_position: DVec3,
    terrain_grid_rotation: DQuat,
) -> Entity {
    let terrain_local = DVec3::new(center[0], origin_y, center[1]);
    let grid_local = terrain_grid_position + terrain_grid_rotation * terrain_local;
    let (cell, local) = grid.translation_to_grid(grid_local);
    let local_rotation = terrain_grid_rotation.as_quat();
    // Snap the selected band onto the bucket lattice so tiles with near-identical
    // parent ranges share one batched material (`morph_start` is derived from the
    // snapped end at the quadtree's morph ratio).
    let (ms, me, _bucket) = snap_band(morph_end, morph_ratio);
    let mut tile = commands.spawn((
        Mesh3d(mesh),
        tile_look(
            template,
            depth,
            ms,
            me,
            maps,
            terrain_half_extent,
            regional_map_blend_m,
            regional_uv_scale,
            authored,
            shadow,
            diagnostic,
            overlay,
        ),
        cell,
        Transform::from_translation(local).with_rotation(local_rotation),
        // The render binder reacts to `ShaderLook` after this deferred spawn.
        // Keep the tile hidden for one complete ECS turn; the streamer promotes
        // it on the next update, after its material exists.
        Visibility::Hidden,
        // The placement is immutable for this entity's lifetime. Streaming
        // replaces stale or unwanted tiles by despawning/spawning entities;
        // it never edits a resident tile's CellCoord or Transform. Use
        // BigSpace's existing stationary path to prune this high-precision
        // leaf from propagation and spatial-hash work on stable frames.
        Stationary,
        lunco_core::SceneOwned,
        LodTileOf(terrain),
        lunco_render::camera::SceneViewGeometry {
            camera: view_camera,
        },
        bevy::camera::visibility::RenderLayers::none().with(render_layer),
        Name::new(format!("LodTile d{} {},{}", coord.depth, coord.x, coord.z)),
        // Streamed runtime detail, not scene content — hidden from the Entity
        // list unless the user opts in.
        lunco_core::SystemManaged,
        ChildOf(grid_entity),
    ));
    if overlay.mode > 0.5 {
        tile.try_insert(TerrainDiagnosticTile);
    }
    if let Some(bounds) = draw_bounds {
        tile.try_insert(bounds);
    }
    tile.try_remove::<bevy::light::NotShadowReceiver>();
    tile.id()
}

/// Marks a resident tile whose material has been replaced by the diagnostic
/// tool. Production map and shadow binders must not mutate that look.
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct TerrainDiagnosticTile;

/// Cross-terrain tile-streaming progress, derived fresh each frame by
/// [`update_lod_tiles`]: how much of the WANTED tile set is actually on
/// screen. UI layers read this to show a "terrain streaming…" indicator —
/// without one, a freshly opened scene is a black void until the first bakes
/// land, which reads as a hang. Kept here (engine-side, UI-free) so the UI is
/// a pure derived read.
#[derive(Resource, Default, Clone, Copy)]
pub struct TerrainStreamStatus {
    /// Unique GPU upload estimate across every source and view.
    pub mesh_accounted_bytes: u64,
    /// Retained CPU vertex/index payload, reported outside the GPU byte ceiling.
    pub mesh_cpu_bytes: u64,
    /// Accounted meshes referenced by live, retiring, or newly published tiles.
    pub mesh_resident_bytes: u64,
    /// Unique mesh estimate addressable by current cache keys; overlaps residents.
    pub mesh_cached_bytes: u64,
    /// Live old-generation meshes whose cache identity was invalidated.
    pub mesh_retired_bytes: u64,
    pub mesh_byte_limit: u64,
    /// CPU-ready meshes waiting for logical residency byte admission.
    pub mesh_upload_refused: usize,
    /// Largest individual waiting upload estimate, for an unsatisfied profile.
    pub mesh_upload_required_bytes: u64,
    /// Exhausted revisions withdraw exact drawn-surface query availability.
    pub draw_revision_exhausted: usize,
    /// Tiles the current selection wants on screen (all streaming terrains).
    pub wanted: usize,
    /// Wanted tiles with a resident mesh entity (stale-but-covering counts —
    /// the ground is visible, just not current).
    pub resident: usize,
    /// Stream work not yet render-ready: cover preparation, off-thread bakes,
    /// and selected tiles waiting for the render binder to install a material.
    pub pending: usize,
    /// Views whose finalized latest demand has not reached a settled,
    /// current-generation drawn cover, including work not yet queued.
    pub selection_pending: usize,
    /// Views retaining their published partition while a candidate awaits stationary camera clearance.
    pub presentation_pending: usize,
    /// Obsolete in-flight requests dropped this frame after selection moved on.
    pub stale_cancelled: usize,
    /// Splits the selection wanted but `tile_budget` refused (summed over
    /// terrains). Unlike `pending` this is not transient: non-zero at a still
    /// camera means areas hold on coarser parents until `tile_budget` is raised
    /// or `pixel_error` coarsened.
    pub budget_refused: usize,
    /// Navigation corridors whose finest-node or restricted-cover admission
    /// exceeded the configured tile/work capacity.
    pub preparation_refused: usize,
    /// Active camera demands resolved in a streaming terrain frame.
    pub focus_wanted: usize,
    /// Camera positions whose exact selected tile is ready; positions outside
    /// the finite footprint have no selected tile.
    pub focus_resident: usize,
}
impl lunco_core::SceneResource for TerrainStreamStatus {}
impl TerrainStreamStatus {
    fn publish_selection_pending(&mut self, pending: usize) {
        self.pending = self.pending.saturating_sub(self.selection_pending) + pending;
        self.selection_pending = pending;
    }
}

fn publish_mesh_pressure(
    status: &TerrainStreamStatus,
    diagnostics: &mut Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let Some(diagnostics) = diagnostics.as_deref_mut() else {
        return;
    };
    let mut findings = if status.mesh_upload_refused > 0
        || status.mesh_accounted_bytes > status.mesh_byte_limit
    {
        vec![lunco_core::RuntimeDiagnostic {
            producer: "terrain-mesh-residency".into(),
            code: "mesh-upload-waiting".into(),
            severity: lunco_core::DiagnosticSeverity::Info,
            subject: "terrain mesh residency".into(),
            message: format!(
                "terrain mesh residency admission pending: {} ready upload(s) waiting, {} estimated bytes retained, {} byte limit, largest pending mesh {} bytes",
                status.mesh_upload_refused,
                status.mesh_accounted_bytes,
                status.mesh_byte_limit,
                status.mesh_upload_required_bytes
            ),
        }]
    } else {
        Vec::new()
    };
    if status.draw_revision_exhausted > 0 {
        findings.push(lunco_core::RuntimeDiagnostic {
            producer: "terrain-mesh-residency".into(), code: "draw-revision-exhausted".into(),
            severity: lunco_core::DiagnosticSeverity::Error, subject: "terrain drawn surface".into(),
            message: "terrain drawn-surface revision is exhausted; exact camera collision queries are unavailable".into(),
        });
    }
    if status.preparation_refused > 0 {
        findings.push(lunco_core::RuntimeDiagnostic {
            producer: "terrain-mesh-residency".into(), code: "preparation-capacity-refused".into(),
            severity: lunco_core::DiagnosticSeverity::Info, subject: "terrain camera preparation".into(),
            message: format!("{} finest corridor requirement(s) exceed the configured terrain cover/work capacity", status.preparation_refused),
        });
    }
    if status.presentation_pending > 0 {
        findings.push(lunco_core::RuntimeDiagnostic {
            producer: "terrain-mesh-residency".into(), code: "presentation-waiting".into(),
            severity: lunco_core::DiagnosticSeverity::Info, subject: "terrain drawn surface".into(),
            message: format!("{} terrain view(s) retain their published surface while replacement geometry awaits camera clearance", status.presentation_pending),
        });
    }
    if diagnostics
        .findings
        .iter()
        .filter(|finding| finding.producer == "terrain-mesh-residency")
        .ne(findings.iter())
    {
        diagnostics.replace_producer("terrain-mesh-residency", findings);
    }
}

/// Count the stream work that still has to cross the render-readiness boundary.
///
/// A completed CPU bake is removed from `PendingTileBakes` before its spawned
/// entity receives `ShaderLookReady`. Counting only the task map therefore made
/// the final handful of tiles disappear from the pending count for one or more
/// frames, even though they were still hidden and the selected cover was not
/// complete. The root fallback is included while its readiness latch is false;
/// it is required even though it is not part of the selected cover.
fn render_pending_count(
    pending_bakes: usize,
    wanted: &HashSet<QuadCoord>,
    tiles: &LodView,
) -> usize {
    let wanted_unready = wanted
        .iter()
        .filter(|coord| {
            tiles
                .pending_replacements
                .get(coord)
                .or_else(|| tiles.tiles.get(coord))
                .is_some_and(|slot| !slot.ready)
        })
        .count();
    let root_unready = (!wanted.contains(&QuadCoord::ROOT)
        && !tiles.coarse_ready
        && tiles
            .tiles
            .get(&QuadCoord::ROOT)
            .is_some_and(|slot| !slot.ready)) as usize;
    pending_bakes + wanted_unready + root_unready
}

fn required_focus_depth(
    cover: &HashSet<QuadCoord>,
    demands: &[TerrainVisualDemand],
    quadtree: &Quadtree,
) -> Option<u8> {
    demands
        .iter()
        .filter(|demand| demand.required)
        .filter_map(|demand| {
            cover
                .iter()
                .copied()
                .find(|&coord| quadtree.region(coord).distance_to(demand.focus) <= f64::EPSILON)
                .map(|coord| coord.depth)
        })
        .min()
}

fn projected_posting_error_floor(region: Square, tile_res: usize) -> f64 {
    // Coarse probes intentionally omit unresolved source bands. Projected
    // posting density must therefore keep driving discovery at every camera
    // range, independently of measured intrinsic relief and the near-field pins.
    region.side() / (tile_res - 1) as f64
}

fn near_field_retains_refinement(
    region: Square,
    heights: [f64; 2],
    demands: &[TerrainVisualDemand],
) -> bool {
    demands.iter().any(|focus| {
        focus.required
            && matches!(focus.projection, TerrainProjection::Perspective { .. })
            && region
                .distance_to_height_range([focus.focus[0], focus.camera_y, focus.focus[1]], heights)
                <= focus.near_detail_radius_m + focus.near_detail_hysteresis_m
    })
}

fn cached_node_geometry(
    cache: &std::cell::RefCell<&mut HashMap<QuadCoord, NodeGeometry>>,
    coord: QuadCoord,
    measure: impl FnOnce() -> NodeGeometry,
) -> NodeGeometry {
    // Copy the value out in its own scope. Keeping the `Ref` temporary alive
    // through the miss branch and then calling `borrow_mut` is a runtime panic.
    if let Some(error) = { cache.borrow().get(&coord).copied() } {
        return error;
    }
    let error = measure();
    cache.borrow_mut().insert(coord, error);
    error
}

fn add_focus_readiness(
    status: &mut TerrainStreamStatus,
    demands: &[TerrainVisualDemand],
    selected: impl IntoIterator<Item = QuadCoord> + Clone,
    quadtree: &Quadtree,
    is_ready: impl Fn(QuadCoord) -> bool,
) {
    for demand in demands.iter().filter(|demand| demand.required) {
        status.focus_wanted += 1;
        let selected_coord = selected
            .clone()
            .into_iter()
            .find(|&coord| quadtree.region(coord).distance_to(demand.focus) <= f64::EPSILON);
        if selected_coord.is_some_and(&is_ready) {
            status.focus_resident += 1;
        }
    }
}

fn bootstrap_cover_is_ready(
    selected: &[Selected],
    demands: &[TerrainVisualDemand],
    is_ready: impl Fn(QuadCoord) -> bool,
) -> bool {
    demands.iter().filter(|focus| focus.required).all(|focus| {
        selected
            .iter()
            // Unlocking after only the tile under the camera was ready still made
            // its neighbours hand off one at a time. Keep the opening cover fixed
            // until every selected tile touching the guaranteed near field has a
            // mesh. The draw partition then reveals that local patch as one stable
            // cover instead of a succession of coarse/fine substitutions.
            .filter(|selected| {
                selected.region.distance_to(focus.focus) <= focus.near_detail_radius_m
            })
            .all(|selected| is_ready(selected.coord))
    })
}

/// While offline recording, select terrain cover once per captured frame rather
/// than on the interactive wall-clock cadence. Bake admission and mesh uploads
/// remain bounded; the recorder waits at its presentation boundary until the
/// selected cover is ready, so worker completion timing cannot change a captured
/// frame or block the UI schedule.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct TerrainStreamFrameDriven(pub bool);

/// Wall-clock cadence for camera-driven terrain-cover reselection.
///
/// Visual terrain refinement follows the rendered camera and does not advance
/// authoritative state. Recomputing its cover at display refresh rate wastes
/// UI-frame time while a camera moves. The first update is immediate; after
/// that, missed intervals are dropped so a slow frame never causes catch-up
/// selection work. Offline frame-driven capture explicitly bypasses this cadence.
#[derive(Resource)]
pub struct TerrainStreamCadence {
    interval: Duration,
    elapsed: Duration,
    due: bool,
    first_update: bool,
}

impl Default for TerrainStreamCadence {
    fn default() -> Self {
        Self {
            interval: Duration::from_nanos(33_333_333),
            elapsed: Duration::ZERO,
            due: true,
            first_update: true,
        }
    }
}

impl TerrainStreamCadence {
    fn advance(&mut self, delta: Duration) {
        if self.first_update {
            self.first_update = false;
            self.due = true;
            return;
        }

        let elapsed = self.elapsed.saturating_add(delta);
        self.due = elapsed >= self.interval;
        self.elapsed = if self.due {
            let remainder_nanos = elapsed.as_nanos() % self.interval.as_nanos();
            Duration::from_nanos(remainder_nanos as u64)
        } else {
            elapsed
        };
    }

    fn is_due(&self) -> bool {
        self.due
    }
}

fn terrain_cover_selection_due(
    cadence_due: bool,
    last_admission: Option<u64>,
    current: u64,
) -> bool {
    cadence_due || last_admission != Some(current)
}

#[derive(Resource, Default, Clone)]
pub(crate) struct TerrainCoverResults {
    queue: Arc<Mutex<VecDeque<TerrainCoverCompletion>>>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default, Clone)]
pub(crate) struct TerrainTileBakeResults {
    queue: Arc<Mutex<VecDeque<TerrainTileBakeCompletion>>>,
}

#[cfg(not(target_arch = "wasm32"))]
struct TerrainTileBakeCompletion {
    terrain: Entity,
    coord: QuadCoord,
    key: lunco_core_runtime::AsyncWorkKey,
    result: Result<BakedTile, ()>,
}

#[derive(bevy::ecs::system::SystemParam)]
#[doc(hidden)]
pub struct TerrainCoverWork<'w> {
    admission: ResMut<'w, lunco_core_runtime::AsyncWorkAdmission>,
    results: Res<'w, TerrainCoverResults>,
    #[cfg(not(target_arch = "wasm32"))]
    tile_results: Res<'w, TerrainTileBakeResults>,
}

#[derive(bevy::ecs::system::SystemParam)]
#[doc(hidden)]
pub struct TerrainStreamEnvironment<'w, 's> {
    overlay_params: Res<'w, crate::overlay::TerrainOverlayParams>,
    diagnostic: Res<'w, crate::overlay::TerrainDiagnosticLook>,
    parents: Query<'w, 's, &'static ChildOf>,
    grids: Query<'w, 's, &'static Grid>,
    spatial: Query<'w, 's, (Option<&'static CellCoord>, &'static Transform)>,
    material_bound: Query<'w, 's, (), With<ShaderLookReady>>,
    draw_geometry: Query<
        'w,
        's,
        (
            Ref<'static, Mesh3d>,
            Ref<'static, ShaderLook>,
            Ref<'static, GlobalTransform>,
            Option<&'static TerrainDrawBounds>,
        ),
    >,
    envelopes: Query<'w, 's, &'static lunco_camera_core::CameraCollisionEnvelope>,
    precision: lunco_spatial::coords::RenderPrecisionQuery<'w, 's>,
    occlusion_budget: Option<Res<'w, crate::occlusion::SurfaceOcclusionBudget>>,
    origin_controllers: Query<
        'w,
        's,
        Ref<'static, lunco_spatial::world::RenderOriginController>,
        With<lunco_spatial::world::OriginAnchor>,
    >,
    live_meshes: Query<
        'w,
        's,
        &'static Mesh3d,
        (
            With<LodTileOf>,
            bevy::ecs::query::Allow<bevy::ecs::entity_disabling::Disabled>,
        ),
    >,
    diagnostics: Option<ResMut<'w, lunco_core::RuntimeDiagnostics>>,
}

impl TerrainStreamEnvironment<'_, '_> {
    /// Read the navigating owner's current native pose, including a camera move
    /// already committed this Update before low-precision propagation runs.
    fn publication_eye(&self, source: Entity) -> Result<DVec3, String> {
        let parent = self
            .parents
            .get(source)
            .map_err(|_| "navigating camera has no grid parent")?
            .parent();
        let grid = self
            .grids
            .get(parent)
            .map_err(|_| "navigating camera parent is not a native grid")?;
        let (cell, transform) = self
            .spatial
            .get(source)
            .map_err(|_| "navigating camera pose is unavailable")?;
        let cell = cell.ok_or("navigating camera has no native cell")?;
        let eye = grid
            .global_transform(cell, transform)
            .translation()
            .as_dvec3();
        if !eye.is_finite() {
            return Err("navigating camera render pose is non-finite".into());
        }
        Ok(eye)
    }

    fn admit_publication(
        &self,
        source: Entity,
        grid_entity: Entity,
        candidates: &[DrawPublicationEntry],
        ratio: f32,
        resolution: usize,
        half: f64,
        meshes: &Assets<Mesh>,
        previous: Option<&(u64, String)>,
    ) -> Result<(), (u64, String)> {
        use std::hash::{Hash, Hasher};
        let Ok(envelope) = self.envelopes.get(source) else {
            return Ok(());
        };
        let mut signature = std::hash::DefaultHasher::new();
        source.hash(&mut signature);
        grid_entity.hash(&mut signature);
        resolution.hash(&mut signature);
        half.to_bits().hash(&mut signature);
        ratio.to_bits().hash(&mut signature);
        for scalar in [
            envelope.radius_m,
            envelope.skin_m,
            envelope.sweep_tolerance_m,
        ] {
            scalar.to_bits().hash(&mut signature);
        }
        for controller in &self.origin_controllers {
            controller.camera.hash(&mut signature);
            controller.world_grid.hash(&mut signature);
        }
        let eye = self.publication_eye(source);
        if let Ok(eye) = eye.as_ref() {
            for scalar in eye.to_array() {
                scalar.to_bits().hash(&mut signature);
            }
        }
        self.occlusion_budget
            .as_ref()
            .map(|budget| budget.max_intervals)
            .hash(&mut signature);
        // Native origin/grid state can alter the coordinate certificate even
        // while the authored camera and tile coordinates are unchanged.
        let source_grid = self.parents.get(source).ok().map(|parent| parent.parent());
        let mut visited = HashSet::new();
        for seed in [Some(grid_entity), source_grid].into_iter().flatten() {
            let mut entity = seed;
            for _ in 0..lunco_spatial::coords::MAX_SPATIAL_HIERARCHY_DEPTH {
                if !visited.insert(entity) {
                    break;
                }
                entity.hash(&mut signature);
                let Ok(grid) = self.grids.get(entity) else {
                    break;
                };
                grid.cell_edge_length().to_bits().hash(&mut signature);
                grid.maximum_distance_from_origin()
                    .to_bits()
                    .hash(&mut signature);
                let origin = grid.local_floating_origin();
                let cell = origin.cell();
                [cell.x, cell.y, cell.z].hash(&mut signature);
                for scalar in origin.grid_transform().to_cols_array() {
                    scalar.to_bits().hash(&mut signature);
                }
                let Ok(parent) = self.parents.get(entity) else {
                    break;
                };
                entity = parent.parent();
            }
        }
        for entry in candidates {
            entry.resolution.hash(&mut signature);
            entry.half_extent.to_bits().hash(&mut signature);
            entry.coord.hash(&mut signature);
            entry.entity.hash(&mut signature);
            self.parents
                .get(entry.entity)
                .ok()
                .map(|parent| parent.parent())
                .hash(&mut signature);
            entry.morph_end.to_bits().hash(&mut signature);
            entry.stitch_edges.map(f32::to_bits).hash(&mut signature);
            match self.draw_geometry.get(entry.entity) {
                Ok((mesh, look, transform, bounds)) => {
                    mesh.0.id().hash(&mut signature);
                    meshes.get(&mesh.0).is_some().hash(&mut signature);
                    look.last_changed().get().hash(&mut signature);
                    for scalar in transform.to_matrix().to_cols_array() {
                        scalar.to_bits().hash(&mut signature);
                    }
                    if let Some(bounds) = bounds {
                        for scalar in bounds
                            .min
                            .to_array()
                            .into_iter()
                            .chain(bounds.max.to_array())
                            .chain(bounds.parent_displacement.to_array())
                        {
                            scalar.to_bits().hash(&mut signature);
                        }
                    }
                    bounds.is_some().hash(&mut signature);
                }
                Err(_) => false.hash(&mut signature),
            }
        }
        let signature = signature.finish();
        if let Some((last, reason)) = previous.filter(|(last, _)| *last == signature) {
            return Err((*last, reason.clone()));
        }
        let result = (|| {
            let eye = eye?;
            let mut remaining = self
                .occlusion_budget
                .as_ref()
                .ok_or("terrain publication requires the source collision work budget")?
                .max_intervals;
            for entry in candidates {
                if self
                    .parents
                    .get(entry.entity)
                    .map(|parent| parent.parent())
                    .ok()
                    != Some(grid_entity)
                {
                    return Err("candidate terrain tile has no matching native grid lease".into());
                }
                let (handle, _, global, bounds) = self
                    .draw_geometry
                    .get(entry.entity)
                    .map_err(|_| "candidate terrain draw pose or material has not propagated")?;
                let mesh = meshes
                    .get(&handle.0)
                    .ok_or("candidate terrain CPU geometry is unavailable")?;
                let bounds =
                    bounds.ok_or("candidate terrain has no finite geometry certificate")?;
                let matrix = global.to_matrix().as_dmat4();
                let certificate = self.precision.certify(
                    source,
                    grid_entity,
                    grid_entity,
                    eye,
                    eye,
                    matrix,
                    bounds.min.abs().max(bounds.max.abs()),
                )?;
                let (start, end, _) = snap_band(entry.morph_end, ratio);
                if !crate::draw_collision::admits_stationary_mesh(
                    mesh,
                    entry.coord,
                    entry.resolution,
                    entry.half_extent,
                    matrix,
                    eye,
                    envelope,
                    start,
                    end,
                    entry.stitch_edges,
                    certificate,
                    bounds,
                    &mut remaining,
                )? {
                    return Err(format!(
                        "candidate terrain tile {:?} would intersect the navigating camera or has unresolved render precision",
                        entry.coord
                    ));
                }
            }
            Ok(())
        })();
        result.map_err(|reason| (signature, reason))
    }
}

fn view_owner(terrain: Entity, camera: Entity) -> u128 {
    (u128::from(terrain.to_bits()) << 64) | u128::from(camera.to_bits())
}

pub(crate) fn visual_max_depth(
    oracle: &SurfaceOracle,
    local_depth: u8,
) -> Result<u8, &'static str> {
    let local = f64::from(oracle.half_extent());
    let domain = f64::from(oracle.domain_half_extent());
    let ratio = domain / local;
    let levels = ratio.log2();
    if !ratio.is_finite() || ratio < 1.0 || levels.fract() != 0.0 {
        return Err("visual terrain domain must be a power-of-two local expansion");
    }
    let depth = u32::from(local_depth) + levels as u32;
    if depth > 20 {
        return Err("visual terrain finest spacing exceeds supported depth");
    }
    Ok(depth as u8)
}

/// One owner for camera selection inputs used by preparation and finalized
/// presentation readiness. It performs only bounded scalar/hash work.
fn terrain_view_signatures(
    demands: &TerrainDetailDemands,
    terrain: Entity,
    demand: &VisualDemand,
    visual_foci: &[TerrainVisualDemand],
    generation: u32,
    surface_key: u64,
    profile: &lunco_render::RenderQualityProfile,
    frozen: bool,
) -> (lunco_precompute::Fnv1a, u64) {
    let mut sig = lunco_precompute::Fnv1a::new();
    let mut admission = lunco_precompute::Fnv1a::new();
    for visual in visual_foci.iter() {
        admission.write_u64(visual.camera.to_bits());
        admission.write_u64(demand.source.to_bits());
        admission.write_u64(demand.render_layer as u64);
        admission.write_u64(visual.screen_height_px.to_bits());
        // Pose is a soft selection input; embedding the terrain-local clip pose
        // into hard admission would invalidate useful in-flight work on every
        // small eye/heading change. Native projection parameters remain exact.
        demand.projection.write_signature(&mut admission);
        admission.write_u64(visual.near_detail_radius_m.to_bits());
        admission.write_u64(visual.near_detail_hysteresis_m.to_bits());
        if !frozen {
            visual.selection_clip.write_signature(&mut sig);
        }
    }
    let preparation = demands.preparation_lease_signature(demand.entity, terrain);
    admission.write_u64(u64::from(preparation.is_some()));
    if let Some(preparation) = preparation {
        admission.write_u64(preparation);
    }
    admission.write_u64(generation as u64);
    // Oracle identity — a live re-compose changes the surface without
    // always bumping gen, and would otherwise be missed by the gate.
    // Content key, not the Arc pointer: a reused allocation address
    // would pass the gate with a stale surface.
    admission.write_u64(surface_key);
    // LOD knobs (Inspector) — a live tweak must re-select.
    admission.write_u64(profile.terrain_lod_pixel_error.to_bits());
    admission.write_u64(profile.terrain_lod_tile_budget as u64);
    admission.write_u64(profile.terrain_lod_max_depth as u64);
    admission.write_u64(profile.terrain_lod_tile_resolution as u64);
    admission.write_u64(profile.terrain_lod_cinematic_resolution as u64);
    admission.write_u64(profile.terrain_lod_probe_resolution as u64);
    admission.write_u64(profile.terrain_lod_cover_edits_per_frame as u64);
    admission.write_u64(profile.terrain_lod_hysteresis_ratio.to_bits());
    admission.write_u64(profile.terrain_lod_morph_start_ratio.to_bits());
    admission.write_u64(u64::from(frozen));
    let admission_sig = admission.finish();
    (sig, admission_sig)
}

fn finish_terrain_view_signature(
    mut signature: lunco_precompute::Fnv1a,
    admission: u64,
    preparation: Option<u64>,
) -> u64 {
    if let Some(preparation) = preparation {
        signature.write_u64(preparation);
    }
    signature.write_u64(admission);
    signature.finish()
}

#[derive(Clone, Copy)]
struct TerrainCoverToken {
    terrain: Entity,
    camera: Entity,
    generation: u32,
    signature: u64,
    admission_signature: u64,
    oracle_key: u64,
    operation: u64,
}

impl TerrainCoverToken {
    fn matches_current_inputs(
        &self,
        generation: u32,
        oracle_key: u64,
        admission_signature: u64,
    ) -> bool {
        self.generation == generation
            && self.oracle_key == oracle_key
            && self.admission_signature == admission_signature
    }
}

struct TerrainCoverCompletion {
    token: TerrainCoverToken,
    result: Result<PreparedTerrainCover, ()>,
}

#[derive(Default, Clone)]
struct CameraPatchRetention {
    oracle_key: u64,
    root_half_bits: u64,
    max_depth: u8,
    patches: HashMap<Entity, Vec<QuadCoord>>,
}

struct PreparedTerrainCover {
    cover: HashSet<QuadCoord>,
    selected: Vec<Selected>,
    camera_patches: CameraPatchRetention,
    node_geometry: HashMap<QuadCoord, NodeGeometry>,
    budget_refused: usize,
    preparation_refused_nodes: HashSet<QuadCoord>,
    preparation_signature: Option<u64>,
    settled_sig: Option<u64>,
    more_work: bool,
}

struct TerrainPreparationRequirements {
    nodes: HashSet<QuadCoord>,
    latest_nodes: HashSet<QuadCoord>,
}

struct TerrainCoverRequest {
    quadtree: Quadtree,
    oracle: Arc<SurfaceOracle>,
    oracle_key: u64,
    demands: Vec<TerrainVisualDemand>,
    camera_patches: CameraPatchRetention,
    cover: HashSet<QuadCoord>,
    node_geometry: HashMap<QuadCoord, NodeGeometry>,
    bootstrap_ready: bool,
    settled_sig: Option<u64>,
    budget_refused: usize,
    signature: u64,
    preparation_signature: Option<u64>,
    preparation: Option<TerrainPreparationRequirements>,
    tile_resolution: usize,
    probe_resolution: usize,
    tile_budget: usize,
    max_depth: u8,
    cover_edits_per_cycle: usize,
    hysteresis_ratio: f64,
}

/// Same-depth diagonal neighbours, clipped to the terrain root.
fn camera_diagonal_neighbours(centre: QuadCoord) -> impl Iterator<Item = QuadCoord> {
    centre
        .edge_neighbours()
        .flat_map(QuadCoord::edge_neighbours)
        .filter(move |neighbour| neighbour.x != centre.x && neighbour.z != centre.z)
}

/// Cover a continuous swept footprint with finest nodes. Samples are at most
/// half a finest-cell side apart; each disk includes half the sample spacing,
/// enclosing the entire segment between samples. Capacity failure admits no
/// truncated corridor and is reported as a refused preparation request.
fn preparation_corridor_nodes(
    quadtree: &Quadtree,
    corridor: TerrainPreparationCorridor,
    capacity: usize,
) -> Result<Vec<QuadCoord>, ()> {
    if !corridor.start.is_finite()
        || !corridor.end.is_finite()
        || !corridor.radius_m.is_finite()
        || corridor.radius_m <= 0.0
        || capacity == 0
    {
        return Err(());
    }
    let side = quadtree
        .region(QuadCoord {
            depth: quadtree.max_depth,
            x: 0,
            z: 0,
        })
        .side();
    if !side.is_finite() || side <= 0.0 {
        return Err(());
    }
    let a = bevy::math::DVec2::new(corridor.start.x, corridor.start.z);
    let b = bevy::math::DVec2::new(corridor.end.x, corridor.end.z);
    let length = a.distance(b);
    let intervals = (length / (side * 0.5)).ceil();
    if !intervals.is_finite() || intervals > capacity.saturating_mul(4) as f64 {
        return Err(());
    }
    let intervals = intervals as usize;
    let reach = corridor.radius_m
        + if intervals > 0 {
            length / intervals as f64 * 0.5
        } else {
            0.0
        };
    let root = quadtree.region(QuadCoord::ROOT);
    let mut nodes = HashSet::new();
    let mut remaining_work = capacity.saturating_mul(32);
    for index in 0..=intervals {
        let point = if intervals == 0 {
            a
        } else {
            a.lerp(b, index as f64 / intervals as f64)
        };
        if root.distance_to(point.to_array()) > reach {
            continue;
        }
        let lo = quadtree.node_containing(
            quadtree.max_depth,
            (point - bevy::math::DVec2::splat(reach)).to_array(),
        );
        let hi = quadtree.node_containing(
            quadtree.max_depth,
            (point + bevy::math::DVec2::splat(reach)).to_array(),
        );
        let count = u64::from(hi.x - lo.x + 1) * u64::from(hi.z - lo.z + 1);
        if count > capacity.saturating_mul(4) as u64 || count > remaining_work as u64 {
            return Err(());
        }
        remaining_work -= count as usize;
        for z in lo.z..=hi.z {
            for x in lo.x..=hi.x {
                let coord = QuadCoord {
                    depth: quadtree.max_depth,
                    x,
                    z,
                };
                if quadtree.region(coord).distance_to(point.to_array()) <= reach {
                    nodes.insert(coord);
                    if nodes.len() > capacity {
                        return Err(());
                    }
                }
            }
        }
    }
    let mut nodes = nodes.into_iter().collect::<Vec<_>>();
    nodes.sort_unstable_by_key(|node| (node.depth, node.x, node.z));
    Ok(nodes)
}

/// Bias one continuous detail region toward the visible ground while fading
/// the bias to zero as the hit approaches the finite near-field reach.
fn terrain_view_focus(oracle: &SurfaceOracle, demand: TerrainVisualDemand) -> [f64; 2] {
    if matches!(demand.projection, TerrainProjection::Orthographic { .. }) {
        return demand.focus;
    }
    let direction = demand.view_direction;
    let radius = demand.near_detail_radius_m;
    if radius <= 0.0 {
        return demand.focus;
    }
    let origin = DVec3::new(demand.focus[0], demand.camera_y, demand.focus[1]);
    // Detail finer than the selection quantum cannot affect a stable cover
    // decision. Filter only the selection ray; geometry and physics still
    // sample their own surface bands.
    let selection_surface = SurfaceBand::visual(TERRAIN_FOCUS_QUANT_M).limited(oracle);
    let hit =
        match crate::oracle::raycast_surface(&selection_surface, origin, direction, 2.0 * radius) {
            Ok(hit) => hit,
            Err(error) => {
                warn_once!("terrain view-focus ray could not be certified: {error}");
                return demand.focus;
            }
        };
    let Some(hit) = hit else { return demand.focus };
    let t = ((2.0 * radius - origin.distance(hit)) / radius).clamp(0.0, 1.0);
    let blend = 0.5 * t * t * (3.0 - 2.0 * t);
    let mut offset = bevy::math::DVec2::new(hit.x - origin.x, hit.z - origin.z) * blend;
    let half = oracle.domain_half_extent() as f64;
    if origin.x.abs() <= half && origin.z.abs() <= half {
        let clearance = (origin.y - oracle.height_at(origin.x, origin.z)).abs();
        let horizontal_reach = (radius * radius - clearance * clearance).max(0.0).sqrt();
        if offset.length_squared() > horizontal_reach * horizontal_reach {
            offset = offset.normalize() * horizontal_reach;
        }
    }
    [demand.focus[0] + offset.x, demand.focus[1] + offset.y]
}

/// Bound clipping work independently of raster size. Missing certification keeps
/// the node eligible; it can never discard geometry from an uncertain envelope.
fn projected_node_height_bounds(oracle: &SurfaceOracle, region: Square) -> Option<[f64; 2]> {
    const MAX_NODE_ENCLOSURE_CELLS: usize = 64;
    let resolution = oracle.domain_resolution();
    if resolution < 2 {
        return None;
    }
    let half = f64::from(oracle.domain_half_extent());
    let spacing = oracle.occlusion_spacing();
    if !spacing.is_finite() || spacing <= 0.0 {
        return None;
    }
    let rect = [
        region.center[0] - region.half,
        region.center[1] - region.half,
        region.center[0] + region.half,
        region.center[1] + region.half,
    ];
    let index = |value: f64| (((value + half) / spacing).floor() as usize).min(resolution - 2);
    let [x0, z0, x1, z1] = rect.map(index);
    if (x1 - x0 + 1).checked_mul(z1 - z0 + 1)? > MAX_NODE_ENCLOSURE_CELLS {
        return None;
    }
    let mut bounds = [f64::INFINITY, f64::NEG_INFINITY];
    let mut remaining = MAX_NODE_ENCLOSURE_CELLS;
    for z in z0..=z1 {
        for x in x0..=x1 {
            let cell_rect = [
                rect[0].max(-half + x as f64 * spacing),
                rect[1].max(-half + z as f64 * spacing),
                rect[2].min(-half + (x + 1) as f64 * spacing),
                rect[3].min(-half + (z + 1) as f64 * spacing),
            ];
            let enclosure = oracle
                .height_bounds_in_cell([x, z], cell_rect, &mut remaining)
                .ok()?;
            bounds[0] = bounds[0].min(enclosure[0]);
            bounds[1] = bounds[1].max(enclosure[1]);
        }
    }
    Some(bounds)
}

fn prepare_terrain_cover(mut request: TerrainCoverRequest) -> PreparedTerrainCover {
    if request.oracle_key != request.oracle.surface_key() {
        request.node_geometry.clear();
        request.oracle_key = request.oracle.surface_key();
    }
    let TerrainCoverRequest {
        quadtree,
        oracle,
        oracle_key: _,
        mut demands,
        camera_patches,
        mut cover,
        mut node_geometry,
        bootstrap_ready,
        mut settled_sig,
        mut budget_refused,
        signature,
        preparation_signature,
        preparation,
        tile_resolution,
        probe_resolution,
        tile_budget,
        max_depth,
        cover_edits_per_cycle,
        hysteresis_ratio,
    } = request;
    // The immutable source owns this cached full-surface fact. Preparation
    // runs on its existing worker; no sampled height can certify exclusion.
    #[cfg(not(target_arch = "wasm32"))]
    let global_height_bounds = oracle.global_height_bounds().ok();
    // The web fallback prepares synchronously. A missing optimization fact
    // keeps nodes eligible rather than scanning a complete source on the UI.
    #[cfg(target_arch = "wasm32")]
    let global_height_bounds = oracle.cached_global_height_bounds().and_then(Result::ok);
    let source_key = oracle.surface_key();
    let root_half_bits = quadtree.root_half_extent.to_bits();
    let retention_matches = camera_patches.oracle_key == source_key
        && camera_patches.root_half_bits == root_half_bits
        && camera_patches.max_depth == max_depth;
    let mut retained_patches = CameraPatchRetention {
        oracle_key: source_key,
        root_half_bits,
        max_depth,
        patches: HashMap::new(),
    };
    let camera_foci: Vec<_> = demands
        .iter()
        .copied()
        .filter(|demand| demand.required)
        .collect();
    // Resolve the view centre during cover preparation. Keep the camera's
    // own mandatory nodes separately.
    for demand in &mut demands {
        demand.focus = terrain_view_focus(&oracle, *demand);
    }
    let mut selected = Vec::new();
    let mut more_work = false;
    let preparation_refused_nodes;
    {
        let geometry_cache = std::cell::RefCell::new(&mut node_geometry);
        let geometry = |coord: QuadCoord, region: Square| {
            cached_node_geometry(&geometry_cache, coord, || {
                let probe_step = region.side() / (probe_resolution - 1) as f64;
                let limited = oracle.detail_limited(probe_step);
                measure_node_geometry(&limited, region, probe_resolution)
            })
        };
        let mut required = Vec::new();
        let mut preparation_required = HashSet::new();
        for (demand, camera_focus) in demands
            .iter()
            .map(|demand| (demand, false))
            .chain(camera_foci.iter().map(|demand| (demand, true)))
            .filter(|(demand, _)| demand.required)
        {
            if matches!(demand.projection, TerrainProjection::Orthographic { .. }) {
                continue;
            }
            let inside = demand.focus[0].abs() <= quadtree.root_half_extent
                && demand.focus[1].abs() <= quadtree.root_half_extent;
            let within_radius = inside
                && (demand.camera_y - oracle.height_at(demand.focus[0], demand.focus[1])).abs()
                    <= demand.near_detail_radius_m;
            if !within_radius {
                if camera_focus && retention_matches {
                    if let Some(patch) = camera_patches.patches.get(&demand.camera) {
                        let retained: Vec<_> = patch
                            .iter()
                            .copied()
                            .filter(|coord| {
                                let region = quadtree.region(*coord);
                                let heights = geometry(*coord, region).height_range;
                                near_field_retains_refinement(
                                    region,
                                    heights,
                                    std::slice::from_ref(demand),
                                )
                            })
                            .collect();
                        if !retained.is_empty() {
                            required.extend(retained.iter().copied());
                            retained_patches.patches.insert(demand.camera, retained);
                        }
                    }
                }
                continue;
            }
            let centre = quadtree.node_containing(max_depth, demand.focus);
            required.push(centre);
            required.extend(centre.edge_neighbours());
            if camera_focus {
                // Keep the exact already-requested patch through the retention
                // band; never request a new patch outside the detail radius.
                let mut patch = vec![centre];
                patch.extend(centre.edge_neighbours());
                patch.extend(camera_diagonal_neighbours(centre));
                patch.sort_unstable_by_key(|coord| (coord.depth, coord.x, coord.z));
                patch.dedup();
                required.extend(patch.iter().copied());
                retained_patches.patches.insert(demand.camera, patch);
            }
        }
        let mut historical = HashSet::new();
        if let Some(preparation) = preparation {
            preparation_required = preparation.latest_nodes;
            historical.extend(
                preparation
                    .nodes
                    .difference(&preparation_required)
                    .copied()
                    .filter(|node| !required.contains(node)),
            );
            required.extend(preparation.nodes);
        }
        // Current camera and current navigation requirements precede retained
        // superseded intent, so history cannot consume their mandatory budget.
        required.sort_unstable_by_key(|coord| {
            (historical.contains(coord), coord.depth, coord.x, coord.z)
        });
        required.dedup();
        let node_heights = |coord, region| geometry(coord, region).height_range;
        let clip_bounds = std::cell::RefCell::new(HashMap::new());
        let node_error = |coord, region| {
            let measured = geometry(coord, region);
            let bounds = *clip_bounds.borrow_mut().entry(coord).or_insert_with(|| {
                match (
                    projected_node_height_bounds(&oracle, region),
                    global_height_bounds,
                ) {
                    (Some(local), Some(global)) => {
                        let bounds = [local[0].max(global[0]), local[1].min(global[1])];
                        (bounds[0] <= bounds[1]).then_some(bounds)
                    }
                    (local, global) => local.or(global),
                }
            });
            if let Some(bounds) = bounds {
                if !demands
                    .iter()
                    .any(|demand| demand.selection_clip.intersects(region, bounds))
                {
                    return 0.0;
                }
            }
            measured
                .error
                .max(projected_posting_error_floor(region, tile_resolution))
        };
        let focus_metric = demands
            .iter()
            .map(|demand| (demand.focus, demand.camera_y))
            .collect::<Vec<_>>();
        let mut admitted = Vec::new();
        let mut reserved = vec![quadtree.selected(QuadCoord::ROOT, f64::INFINITY)];
        let mut refused_mandatory = 0;
        let mut refused_preparation_nodes = HashSet::new();
        for requested in required {
            let mut candidate = requested;
            loop {
                let mut trial = admitted.clone();
                trial.push(candidate);
                let mut restricted = vec![quadtree.selected(QuadCoord::ROOT, f64::INFINITY)];
                quadtree.constrain_selection(&mut restricted, &trial, &node_error);
                if restricted.len() <= tile_budget {
                    admitted = trial;
                    reserved = restricted;
                    refused_mandatory += usize::from(candidate != requested);
                    if candidate != requested && preparation_required.contains(&requested) {
                        refused_preparation_nodes.insert(requested);
                    }
                    break;
                }
                let Some(parent) = candidate.parent() else {
                    refused_mandatory += 1;
                    if preparation_required.contains(&requested) {
                        refused_preparation_nodes.insert(requested);
                    }
                    break;
                };
                candidate = parent;
            }
        }
        let required = admitted;
        // Union of two refinement trees has at most A+B-1 leaves. Reserve the
        // mandatory tree first; the camera's discretionary splits use the rest.
        let discretionary_budget = tile_budget.saturating_sub(reserved.len() - 1).max(1);
        let mut cover_scratch = CoverScratch::default();
        if cover.is_empty() {
            budget_refused = 0;
            for _ in 0..max_depth {
                let before = required_focus_depth(&cover, &demands, &quadtree);
                let (_, refused) = evolve_cover_for_foci_with_retention(
                    &quadtree,
                    &mut cover,
                    &focus_metric,
                    &node_error,
                    &node_heights,
                    discretionary_budget,
                    cover_edits_per_cycle,
                    hysteresis_ratio,
                    &|coord, region| {
                        near_field_retains_refinement(region, node_heights(coord, region), &demands)
                    },
                    &mut cover_scratch,
                );
                budget_refused += refused;
                let after = required_focus_depth(&cover, &demands, &quadtree);
                if after == before || after == Some(max_depth) {
                    break;
                }
            }
        } else if bootstrap_ready && settled_sig != Some(signature) {
            let (edits, refused) = evolve_cover_for_foci_with_retention(
                &quadtree,
                &mut cover,
                &focus_metric,
                &node_error,
                &node_heights,
                discretionary_budget,
                cover_edits_per_cycle,
                hysteresis_ratio,
                &|coord, region| {
                    near_field_retains_refinement(region, node_heights(coord, region), &demands)
                },
                &mut cover_scratch,
            );
            budget_refused = refused;
            more_work = edits > 0;
            settled_sig = (edits == 0).then_some(signature);
        }
        preparation_refused_nodes = refused_preparation_nodes;
        budget_refused += refused_mandatory;
        selected.extend(cover.iter().map(|&coord| {
            let parent_range = coord
                .parent()
                .map(|parent| node_error(parent, quadtree.region(parent)))
                .map(|error| quadtree.error_refine_range(error))
                .unwrap_or(f64::INFINITY);
            quadtree.selected(coord, parent_range)
        }));
        quadtree.constrain_selection(&mut selected, &required, &node_error);
    }
    selected.sort_by_key(|tile| (tile.coord.depth, tile.coord.x, tile.coord.z));
    PreparedTerrainCover {
        cover,
        selected,
        camera_patches: retained_patches,
        node_geometry,
        budget_refused,
        preparation_refused_nodes,
        preparation_signature,
        settled_sig,
        more_work,
    }
}

fn commit_prepared_terrain_cover(
    tiles: &mut LodView,
    node_geometry: &mut TerrainNodeGeometry,
    prepared: PreparedTerrainCover,
    oracle_key: u64,
    signature: u64,
    admission_signature: u64,
) {
    tiles.bump_draw_revision();
    tiles.cover = prepared.cover;
    tiles.cover_selected = prepared.selected;
    tiles.camera_patches = prepared.camera_patches;
    tiles.budget_refused = prepared.budget_refused;
    tiles.preparation_refused_nodes = prepared.preparation_refused_nodes;
    tiles.preparation_signature = prepared.preparation_signature;
    tiles.settled_sig = prepared.settled_sig;
    tiles.last_sig =
        (!prepared.more_work && prepared.settled_sig == Some(signature)).then_some(signature);
    tiles.last_admission_sig = Some(admission_signature);
    tiles.failed_cover_signature = None;
    node_geometry.oracle_key = oracle_key;
    node_geometry.map.extend(prepared.node_geometry);
}

type TerrainBakeSortKey = (u8, f64, Selected);

/// Close equal visual priorities with stable tile identity, not `HashSet` order.
fn compare_terrain_bake_keys(a: &TerrainBakeSortKey, b: &TerrainBakeSortKey) -> std::cmp::Ordering {
    a.0.cmp(&b.0)
        .then_with(|| a.1.total_cmp(&b.1))
        .then_with(|| {
            (a.2.coord.depth, a.2.coord.x, a.2.coord.z).cmp(&(
                b.2.coord.depth,
                b.2.coord.x,
                b.2.coord.z,
            ))
        })
        .then_with(|| a.2.morph_start.total_cmp(&b.2.morph_start))
        .then_with(|| a.2.morph_end.total_cmp(&b.2.morph_end))
}

pub(crate) fn advance_terrain_stream_cadence(
    mut cadence: ResMut<TerrainStreamCadence>,
    wall_time: Res<Time<Real>>,
) {
    cadence.advance(wall_time.delta());
}

pub(crate) fn cancel_terrain_view_preparation(
    removed: On<Remove<LodTiles>>,
    terrains: Query<&LodTiles>,
    mut admission: ResMut<lunco_core_runtime::AsyncWorkAdmission>,
) {
    if let Ok(tiles) = terrains.get(removed.entity) {
        for camera in tiles.views.keys() {
            admission.cancel_owner(
                lunco_core_runtime::AsyncWorkKind::VisualizationPreparation,
                0,
                view_owner(removed.entity, *camera),
            );
        }
    }
}

/// Withdraw presentation preparation when its terrain owner leaves the ECS
/// world. Running jobs remain fenced by entity identity and their result is
/// discarded if the owner no longer exists.
pub(crate) fn cancel_removed_terrain_preparation(
    mut removed: RemovedComponents<PendingTileBakes>,
    owners: Query<(), With<PendingTileBakes>>,
    mut admission: ResMut<lunco_core_runtime::AsyncWorkAdmission>,
    cover_results: Res<TerrainCoverResults>,
    #[cfg(not(target_arch = "wasm32"))] tile_results: Res<TerrainTileBakeResults>,
) {
    let removed = removed
        .read()
        .filter(|&entity| owners.get(entity).is_err())
        .collect::<HashSet<_>>();
    if removed.is_empty() {
        return;
    }
    for entity in &removed {
        admission.cancel_owner(
            lunco_core_runtime::AsyncWorkKind::VisualizationPreparation,
            0,
            entity.to_bits() as u128,
        );
    }
    cover_results
        .queue
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|completion| !removed.contains(&completion.token.terrain));
    #[cfg(not(target_arch = "wasm32"))]
    tile_results
        .queue
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|completion| !removed.contains(&completion.terrain));
}

/// Per-frame scratch for [`update_lod_tiles`] — the collections the streaming
/// pass used to heap-allocate EVERY frame per terrain (material swaps, finished
/// bakes, the sort keys, the hole-cover set, the wanted set, the selection
/// itself and the cover-evolution candidates). Hoisted into a
/// `Local` so a moving camera reuses the capacity instead of re-allocating; the
/// idle-signature gate already skips the whole pass when nothing moved.
#[derive(Default)]
pub struct StreamScratch {
    done: Vec<(QuadCoord, u32, BakedTile)>,
    keyed: Vec<TerrainBakeSortKey>,
    wanted: HashSet<QuadCoord>,
    /// Hidden, already-baked direct parents of wanted leaves. These make an
    /// unready refinement fall back locally instead of exposing the root tile.
    parent_fallbacks: HashSet<QuadCoord>,
    /// The DRAW partition: exactly one tile per covered area — a ready wanted node,
    /// or the deepest ready ancestor standing in for one that is not.
    draw: HashSet<QuadCoord>,
    /// The always-resident coarse fallback, as a bake target.
    coarse: Vec<Selected>,
    /// Scratch for the disjointness pass over `draw`.
    drop_covered: Vec<QuadCoord>,
    /// Active cameras projected into the terrain's local frame.
    visual_foci: Vec<TerrainVisualDemand>,
    /// The selection read off the persistent cover (~768 × 40 B) — the one
    /// per-frame collection this hoist had missed: it was rebuilt fresh per
    /// terrain per frame.
    sel: Vec<Selected>,
    /// Results returned by bounded background cover preparation.
    cover_completions: HashMap<(Entity, Entity), Vec<TerrainCoverCompletion>>,
    #[cfg(not(target_arch = "wasm32"))]
    /// Results returned by shared-priority streamed tile bakes.
    tile_completions: HashMap<Entity, Vec<TerrainTileBakeCompletion>>,
}

/// Freeze this terrain's LOD selection once its tiles are up: authored
/// `bool lunco:terrain:lodFrozen = true` on the Terrain prim.
///
/// For a SCRIPTED shot. Streaming exists to adapt an unbounded world to a viewpoint
/// nobody predicted; a cinematic's viewpoint is authored, so there is nothing to
/// adapt to — and adapting anyway is visible, because a camera crossing LOD bands
/// at speed evicts and re-bakes tiles mid-shot (the ground blinking under a moving
/// camera). Frozen, the set that streamed in before the shot started is the set that
/// is drawn, all the way through.
///
/// The first selection still runs — freezing an empty terrain would draw nothing at
/// all. It is the RE-selection that stops.
///
/// Not for a free-flying camera: a frozen terrain does not refine for one, so
/// anything outside the initial set stays coarse or absent.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct LodFrozen;

/// Stream the LOD tile set for each streaming terrain against the camera.
/// Cover reselection uses [`TerrainStreamCadence`]; tile completion and residency
/// commits continue every rendered frame.
pub fn update_lod_tiles(
    mut commands: Commands,
    demands: Res<TerrainDetailDemands>,
    mut terrains: Query<
        (
            Entity,
            &DemHeightField,
            &TerrainLodViz,
            &mut LodTiles,
            &mut PendingTileBakes,
            &mut TerrainNodeGeometry,
            Option<&TerrainDerivedMaps>,
            Option<&TerrainAuthoredMaps>,
            Option<&TileShadowCache>,
            Option<&HorizonShadowTerrain>,
            Option<&HorizonMap>,
            Option<&ShaderLook>,
            Has<LodFrozen>,
        ),
        With<DemTerrainSurface>,
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mesh_cache: ResMut<LodMeshCache>,
    quality: Res<lunco_render::RenderingQualitySettings>,
    mut stream_status: ResMut<TerrainStreamStatus>,
    // Set while an offline recording captures; selection remains per captured
    // frame while bake and publication budgets stay bounded.
    frame_driven: Res<TerrainStreamFrameDriven>,
    cadence: Res<TerrainStreamCadence>,
    mut cover_work: TerrainCoverWork,
    mut environment: TerrainStreamEnvironment,
    mut scratch: Local<StreamScratch>,
) {
    let profile = match quality.validated_profile() {
        Ok(profile) => profile,
        Err(reason) => {
            bevy::log::error!(target: "terrain", "terrain rendering quality is invalid: {reason}");
            return;
        }
    };
    // Snapshot the analysis-overlay uniforms once; every tile built this frame paints
    // the current params (live re-tuning of resident tiles rides `sync_terrain_overlay`).
    let overlay = environment.overlay_params.uniforms();
    *stream_status = TerrainStreamStatus::default();
    mesh_cache.begin_frame(
        environment
            .live_meshes
            .iter()
            .map(|mesh| mesh.0.id())
            .collect(),
    );
    mesh_cache.trim(usize::MAX, profile.terrain_mesh_cache_bytes);
    let mut refused_uploads = HashSet::new();
    // Split-borrow the per-frame scratch buffers (see [`StreamScratch`]).
    let StreamScratch {
        done,
        keyed,
        wanted,
        parent_fallbacks,
        draw,
        coarse,
        drop_covered,
        visual_foci,
        sel,
        cover_completions,
        #[cfg(not(target_arch = "wasm32"))]
        tile_completions,
    } = &mut *scratch;
    cover_completions.clear();
    #[cfg(not(target_arch = "wasm32"))]
    tile_completions.clear();
    {
        let mut queue = cover_work
            .results
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for completion in queue.drain(..) {
            cover_completions
                .entry((completion.token.terrain, completion.token.camera))
                .or_default()
                .push(completion);
        }
    }
    if demands.visual.is_empty() {
        for (terrain, _, _, mut tiles, mut pending, _, _, _, _, _, _, _, _) in &mut terrains {
            for (camera, view) in std::mem::take(&mut tiles.views) {
                for entity in view.tile_entities() {
                    retire_tile(&mut commands, entity);
                }
                cover_work.admission.cancel_owner(
                    lunco_core_runtime::AsyncWorkKind::VisualizationPreparation,
                    0,
                    view_owner(terrain, camera),
                );
            }
            pending.cancel_for_missing_demand(&mut cover_work.admission);
        }
        #[cfg(not(target_arch = "wasm32"))]
        cover_work
            .tile_results
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        mesh_cache.publish_accounting(&mut stream_status, profile.terrain_mesh_cache_bytes);
        publish_mesh_pressure(&stream_status, &mut environment.diagnostics);
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut queue = cover_work
            .tile_results
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for completion in queue.drain(..) {
            tile_completions
                .entry(completion.terrain)
                .or_default()
                .push(completion);
        }
    }
    // Per-frame bake budget shared across all terrains (amortise scale changes).
    //
    let frame_driven = frame_driven.0;
    let mut bake_budget = profile.terrain_lod_bakes_per_frame;
    let mut upload_budget = profile.terrain_lod_bakes_per_frame;
    let max_inflight = profile.terrain_lod_max_inflight_bakes;
    // Live streaming terrains — the mesh cache is GLOBAL (keyed by `(terrain,
    // coord)`), so its cap must scale with them or two terrains would fight over
    // one terrain's worth of entries and thrash each other every frame.
    let terrain_count = terrains.iter().count().max(1);

    {
        for (
            terrain,
            hf,
            _viz,
            mut terrain_tiles,
            mut pending,
            mut errs,
            maps,
            authored,
            shadow,
            horizon_shadow,
            horizon_map,
            template,
            frozen,
        ) in &mut terrains
        {
            #[cfg(not(target_arch = "wasm32"))]
            pending.accept_completed_bakes(
                terrain,
                tile_completions.remove(&terrain).unwrap_or_default(),
            );

            let resolution = if frozen {
                profile.terrain_lod_cinematic_resolution
            } else {
                profile.terrain_lod_tile_resolution
            };
            if terrain_tiles.resolution != resolution {
                terrain_tiles.resolution = resolution;
                terrain_tiles.invalidate();
                pending.cancel_for_missing_demand(&mut cover_work.admission);
                mesh_cache.drop_region(terrain, None, f64::from(hf.0.domain_half_extent()));
            }
            stream_status.pending += pending.len();
            let generation = terrain_tiles.r#gen;
            let live: HashSet<_> = demands
                .visual
                .iter()
                .filter(|demand| demand.terrain_poses.contains_key(&terrain))
                .map(|demand| demand.entity)
                .collect();
            terrain_tiles.views.retain(|camera, view| {
                if live.contains(camera) {
                    return true;
                }
                for entity in view.tile_entities() {
                    retire_tile(&mut commands, entity);
                }
                cover_work.admission.cancel_owner(
                    lunco_core_runtime::AsyncWorkKind::VisualizationPreparation,
                    0,
                    view_owner(terrain, *camera),
                );
                false
            });
            for demand in demands
                .visual
                .iter()
                .filter(|demand| live.contains(&demand.entity))
            {
                if terrain_tiles
                    .views
                    .get(&demand.entity)
                    .is_some_and(|view| view.render_layer != demand.render_layer)
                {
                    if let Some(view) = terrain_tiles.views.remove(&demand.entity) {
                        for entity in view.tile_entities() {
                            retire_tile(&mut commands, entity);
                        }
                        cover_work.admission.cancel_owner(
                            lunco_core_runtime::AsyncWorkKind::VisualizationPreparation,
                            0,
                            view_owner(terrain, demand.entity),
                        );
                    }
                }
                terrain_tiles
                    .views
                    .entry(demand.entity)
                    .or_insert_with(|| LodView {
                        render_layer: demand.render_layer,
                        r#gen: generation,
                        ..default()
                    });
            }
            let mut view_cameras: Vec<_> = terrain_tiles.views.keys().copied().collect();
            if !view_cameras.is_empty() {
                let offset = terrain_tiles.frame_cursor % view_cameras.len();
                view_cameras.rotate_left(offset);
                terrain_tiles.frame_cursor = terrain_tiles.frame_cursor.wrapping_add(1);
            }
            for view_camera in view_cameras {
                let mut tiles = terrain_tiles
                    .views
                    .get_mut(&view_camera)
                    .expect("live view state exists");
                let demand = demands
                    .visual
                    .iter()
                    .find(|demand| demand.entity == view_camera)
                    .expect("live view has a demand");
                let max_depth = match visual_max_depth(&hf.0, profile.terrain_lod_max_depth) {
                    Ok(depth) => depth,
                    Err(error) => {
                        bevy::log::error!(target: "terrain", "{error}");
                        continue;
                    }
                };
                let selection_cycle_due = frame_driven
                    || cadence.is_due()
                    || tiles.cover.is_empty()
                    || tiles.cover.iter().any(|coord| coord.depth > max_depth);
                // Horizon-shadow terrain has one complete lighting contract: its
                // heightfield must be present before the first streamed tile is
                // admitted. The horizon bake is an off-thread visual product, but
                // exposing tiles before it lands changes the material inputs after
                // the terrain is already visible. Non-opted-in DEMs keep the normal
                // path.
                if horizon_shadow.is_some() && horizon_map.is_none() {
                    continue;
                }
                let Some(template) = template else {
                    bevy::log::error!(
                        target: "terrain",
                        entity = ?terrain,
                        "streamed DEM terrain has no authored production material"
                    );
                    continue;
                };
                if template.vertex_shader.is_none() {
                    bevy::log::error!(
                        target: "terrain",
                        entity = ?terrain,
                        "streamed DEM terrain material has no authored CDLOD vertex source"
                    );
                    continue;
                }
                // A terrain may be mounted under the canonical WorldGrid or under a
                // body's surface sub-grid. Resolve the actual frame from its ancestry;
                // streamed tiles must share that frame, while their mesh coordinates
                // remain in the terrain owner's local DEM axes.
                let Some((grid_entity, grid)) = lunco_spatial::coords::ancestor_grid(
                    terrain,
                    &environment.parents,
                    &environment.grids,
                ) else {
                    continue;
                };
                let Some((terrain_grid_position, terrain_grid_rotation)) =
                    lunco_spatial::coords::grid_relative_pose(
                        terrain,
                        grid_entity,
                        &environment.parents,
                        &environment.grids,
                        &environment.spatial,
                    )
                else {
                    continue;
                };

                // A tile is drawable only after the render binder has installed its
                // concrete material. Checking the marker avoids exposing a spawned mesh
                // merely because one ECS turn happened to elapse; deferred command order
                // is not a material-readiness contract.
                let mut promoted_tiles = false;
                if !tiles.materials_ready {
                    let mut all_ready = true;
                    for slot in tiles
                        .tiles
                        .values_mut()
                        .chain(tiles.pending_replacements.values_mut())
                    {
                        if !slot.ready {
                            if environment.material_bound.get(slot.entity).is_ok() {
                                slot.ready = true;
                                promoted_tiles = true;
                            } else {
                                all_ready = false;
                            }
                        }
                    }
                    tiles.materials_ready = all_ready;
                }
                if promoted_tiles {
                    tiles.bump_draw_revision();
                }

                // Do not expose a tile before the USD material projection and any
                // required engine-derived product have selected every source. Creating
                // a procedural/derived cover first and rewriting it when either
                // producer publishes would mint a second set of material keys, upload
                // the whole terrain again, and visibly change its appearance. The UI
                // remains active while these off-thread inputs are pending.
                if tiles.tiles.is_empty() && !initial_lit_material_ready(maps, authored) {
                    continue;
                }
                // The frozen cover is ONE tile for the whole terrain, so it carries the
                // detail the whole quadtree used to spread over thousands — mesh it far
                // finer than a streamed tile. The configured streamed resolution over the full window
                // would be ~20 m between vertices: one tile, and no terrain.
                let tile_res = if frozen {
                    profile.terrain_lod_cinematic_resolution
                } else {
                    profile.terrain_lod_tile_resolution
                };
                tiles.resolution = tile_res;
                tiles.effective_max_depth = max_depth;
                // A live max-depth reduction invalidates the persistent cover: it may
                // still contain leaves deeper than the new tree and those leaves would
                // otherwise remain drawable forever. The cover is authoritative state,
                // so reset it to the root and let the normal selector rebuild it under
                // the new bound. Increasing the bound keeps the existing cover and
                // refines from that valid fixed point.
                if selection_cycle_due && tiles.cover.iter().any(|coord| coord.depth > max_depth) {
                    tiles.cover.clear();
                    tiles.cover_selected.clear();
                    tiles.bootstrap_ready = false;
                    tiles.settled_sig = None;
                    tiles.last_sig = None;
                    tiles.budget_refused = 0;
                }
                // The terrain's current height generation: a tile/bake tagged with an older
                // gen is stale (a live re-bake changed the heights) and is replaced near-first.
                let cur_gen = tiles.r#gen;
                // Geometry-affecting material inputs join the draw transaction below.
                let morph_ratio = profile.terrain_lod_morph_start_ratio as f32;

                // Project every active visual camera into this terrain's local DEM frame.
                // Cover preparation biases the continuous detail region toward
                // visible ground, while preserving the camera's finest nodes.
                let oracle = &hf.0;
                let h = oracle.domain_half_extent() as f64;
                tiles.half_extent = h;
                visual_foci.clear();
                for demand in std::iter::once(demand) {
                    let Some(&(terrain_local, local_forward)) = demand.terrain_poses.get(&terrain)
                    else {
                        continue;
                    };
                    let Some(clip) = demand.terrain_clips.get(&terrain).copied() else {
                        continue;
                    };
                    if let Some(mut visual_demand) = terrain_visual_demand(
                        demand,
                        terrain_local,
                        local_forward,
                        clip,
                        profile.terrain_lod_pixel_error,
                    ) {
                        tiles.retain_selection_clip(&mut visual_demand);
                        visual_foci.push(visual_demand);
                    }
                }
                // The actual camera owns the projection. A pending navigation
                // corridor adds exact required nodes during worker preparation.
                let Some(actual) = visual_foci.first() else {
                    continue;
                };
                let quadtree_for = |px: f64| match actual.projection {
                    TerrainProjection::Perspective { fov_y_rad, .. } => {
                        Quadtree::from_screen_metric(
                            h,
                            max_depth,
                            h,
                            actual.screen_height_px,
                            fov_y_rad,
                            px,
                            profile.terrain_lod_morph_start_ratio,
                        )
                    }
                    TerrainProjection::Orthographic {
                        pixels_per_metre, ..
                    } => Quadtree::from_orthographic_metric(h, max_depth, h, pixels_per_metre, px),
                };
                let pixel_error = profile.terrain_lod_pixel_error;
                let qt = quadtree_for(pixel_error);
                // ── Idle-camera fast path ────────────────────────────────────────────
                // The selection below (quadtree walks + budget-coarsen loop + sort + queue
                // + retain) is a pure function of (focus, eye height, generation, oracle
                // identity, LOD knobs). Re-deriving it EVERY
                // frame with a still camera was the dominant idle terrain CPU cost
                // (obs 23593). Fold those inputs into a signature; when it matches last
                // frame's AND no bake is mid-flight (nothing to finalize/spawn), last
                // frame's resident tiles are already correct — skip the whole body.
                // The retained clip envelope supplies one bounded pose identity for
                // both this requester and finalized readiness. Geometry remains
                // eligible throughout its full hysteresis interval.
                let (signature, admission_sig) = terrain_view_signatures(
                    &demands,
                    terrain,
                    demand,
                    visual_foci,
                    cur_gen,
                    hf.0.surface_key(),
                    &profile,
                    frozen,
                );
                let sig = {
                    if let (Some(input), Some(corridor)) = (
                        demands.preparation_signature(view_camera, terrain),
                        demand.preparation_corridors.get(&terrain),
                    ) {
                        tiles
                            .held_preparation
                            .get_or_insert_with(HeldTerrainPreparation::default)
                            .admit(
                                admission_sig,
                                input,
                                *corridor,
                                &qt,
                                profile.terrain_lod_tile_budget,
                            );
                    } else {
                        tiles.held_preparation = None;
                    }
                    finish_terrain_view_signature(
                        signature,
                        admission_sig,
                        tiles
                            .held_preparation
                            .as_ref()
                            .map(|held| held.requirements_signature),
                    )
                };
                tiles.requested_signature = Some(sig);
                let oracle_key = hf.0.surface_key();
                if errs.oracle_key != oracle_key {
                    errs.map.clear();
                    errs.oracle_key = oracle_key;
                }
                // Camera motion follows the wall-clock cadence. Changes to physics
                // support or other admission inputs wake selection immediately.
                let mut selection_due = terrain_cover_selection_due(
                    selection_cycle_due,
                    tiles.last_admission_sig,
                    admission_sig,
                );
                let completion =
                    cover_completions
                        .remove(&(terrain, view_camera))
                        .and_then(|mut results| {
                            let index = results.iter().position(|completion| {
                                tiles.pending_cover
                                    == Some((
                                        completion.token.generation,
                                        completion.token.signature,
                                        completion.token.oracle_key,
                                        completion.token.operation,
                                        completion.token.admission_signature,
                                    ))
                            })?;
                            Some(results.remove(index))
                        });
                let mut cover_committed = false;
                if let Some(completion) = completion {
                    tiles.pending_cover = None;
                    match completion.result {
                        Ok(prepared)
                            if completion.token.matches_current_inputs(
                                cur_gen,
                                oracle_key,
                                admission_sig,
                            ) =>
                        {
                            commit_prepared_terrain_cover(
                                &mut tiles,
                                &mut errs,
                                prepared,
                                oracle_key,
                                completion.token.signature,
                                completion.token.admission_signature,
                            );
                            cover_committed = true;
                        }
                        Ok(_) => selection_due = true,
                        Err(()) => {
                            bevy::log::error!(
                                target: "terrain_stream",
                                terrain = ?terrain,
                                signature = completion.token.signature,
                                "terrain cover preparation worker failed"
                            );
                            tiles.failed_cover_signature = Some(completion.token.signature);
                            if completion.token.signature != sig {
                                selection_due = true;
                            }
                        }
                    }
                }
                if let Some((generation, pending_sig, pending_oracle, operation, pending_admission)) =
                    tiles.pending_cover
                    && (generation != cur_gen
                        || pending_admission != admission_sig
                        || pending_oracle != oracle_key)
                {
                    // Hard source/view leases retire immutable work. Endpoint and
                    // heading updates retain valid queued work; its earlier subset
                    // can publish before the latest accumulated requirements.
                    let key = lunco_core_runtime::AsyncWorkKey::new(
                        lunco_core_runtime::AsyncWorkKind::VisualizationPreparation,
                        0,
                        view_owner(terrain, view_camera),
                        pending_sig,
                        operation,
                    );
                    if cover_work.admission.cancel_queued(key) {
                        tiles.pending_cover = None;
                    }
                    selection_due = true;
                }
                {
                    let cover_status = selected_cover_status(&tiles, cur_gen);
                    // A newly committed cover can change morph windows on resident meshes.
                    if !cover_committed
                        && !promoted_tiles
                        && !tiles.presentation_pending
                        && tiles.pending_replacements.is_empty()
                        && tiles.morph_ratio.to_bits() == morph_ratio.to_bits()
                        && pending.is_empty()
                        && tiles.pending_cover.is_none()
                        && tiles.selection_current()
                        && tiles.coarse_ready
                        && cover_status.complete
                    {
                        // Idle: resident tiles already match. Contribute this terrain's
                        // resident count so the status bar still reads "done", not "0/0".
                        stream_status.wanted += cover_status.wanted;
                        stream_status.resident += cover_status.resident;
                        stream_status.budget_refused += tiles.budget_refused;
                        stream_status.preparation_refused += tiles.preparation_refusal_count(
                            demands.preparation_signature(view_camera, terrain),
                            admission_sig,
                        );
                        stream_status.selection_pending += usize::from(!tiles.latest_ready());
                        stream_status.pending += usize::from(!tiles.latest_ready());
                        add_focus_readiness(
                            &mut stream_status,
                            visual_foci,
                            tiles.cover_selected.iter().map(|selected| selected.coord),
                            &qt,
                            |coord| {
                                tiles
                                    .tiles
                                    .get(&coord)
                                    .is_some_and(|slot| slot.ready && slot.drawn)
                            },
                        );
                        continue;
                    }
                }
                let budget = profile.terrain_lod_tile_budget;
                sel.clear();
                if frozen {
                    sel.push(Selected {
                        coord: QuadCoord::ROOT,
                        region: qt.region(QuadCoord::ROOT),
                        morph_start: f64::INFINITY,
                        morph_end: f64::INFINITY,
                    });
                    tiles.commit_frozen_cover(sel, sig, admission_sig);
                } else {
                    sel.extend(tiles.cover_selected.iter().copied());
                    let needs_cover_work = tiles.cover_preparation_needed(sig);
                    if selection_due
                        && needs_cover_work
                        && tiles.pending_cover.is_none()
                        && tiles.failed_cover_signature != Some(sig)
                    {
                        let Some(operation) = tiles.cover_operation.checked_add(1) else {
                            bevy::log::error!(
                                target: "terrain_stream",
                                entity = ?terrain,
                                "terrain cover operation identity is exhausted"
                            );
                            tiles.failed_cover_signature = Some(sig);
                            continue;
                        };
                        tiles.cover_operation = operation;
                        #[cfg(not(target_arch = "wasm32"))]
                        let token = TerrainCoverToken {
                            terrain,
                            camera: view_camera,
                            generation: cur_gen,
                            signature: sig,
                            admission_signature: admission_sig,
                            oracle_key,
                            operation,
                        };
                        let request = TerrainCoverRequest {
                            quadtree: qt,
                            oracle: Arc::clone(&hf.0),
                            oracle_key,
                            demands: visual_foci.clone(),
                            camera_patches: tiles.camera_patches.clone(),
                            cover: tiles.cover.clone(),
                            node_geometry: errs.map.clone(),
                            bootstrap_ready: tiles.bootstrap_ready,
                            settled_sig: tiles.settled_sig,
                            budget_refused: tiles.budget_refused,
                            signature: sig,
                            preparation_signature: tiles
                                .held_preparation
                                .as_ref()
                                .map(|held| held.requirements_signature),
                            preparation: tiles
                                .held_preparation
                                .as_ref()
                                .map(HeldTerrainPreparation::requirements),
                            tile_resolution: tile_res,
                            probe_resolution: profile.terrain_lod_probe_resolution,
                            tile_budget: budget,
                            max_depth,
                            cover_edits_per_cycle: profile.terrain_lod_cover_edits_per_frame,
                            hysteresis_ratio: profile.terrain_lod_hysteresis_ratio,
                        };
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            let queue = Arc::clone(&cover_work.results.queue);
                            let job = move || {
                                let result = catch_unwind(AssertUnwindSafe(|| {
                                    prepare_terrain_cover(request)
                                }))
                                .map_err(|_| ());
                                queue
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner)
                                    .push_back(TerrainCoverCompletion { token, result });
                            };
                            let key = lunco_core_runtime::AsyncWorkKey::new(
                                lunco_core_runtime::AsyncWorkKind::VisualizationPreparation,
                                0,
                                view_owner(terrain, view_camera),
                                sig,
                                operation,
                            );
                            match cover_work.admission.submit(
                            lunco_core_runtime::AsyncWorkPriority::Interactive,
                            key,
                            job,
                        ) {
                            Ok(()) | Err(lunco_core_runtime::AsyncWorkRejection::DuplicateKey) => {
                                tiles.pending_cover = Some((cur_gen, sig, oracle_key, operation, admission_sig));
                            }
                            Err(lunco_core_runtime::AsyncWorkRejection::QueueFull) => {
                                bevy::log::warn_once!(
                                    "terrain cover preparation is waiting for shared async capacity"
                                );
                            }
                            Err(
                                lunco_core_runtime::AsyncWorkRejection::NativeDispatcherUnavailable,
                            ) => {
                                bevy::log::error!(
                                    target: "terrain_stream",
                                    entity = ?terrain,
                                    "native cover preparation was rejected by the shared dispatcher"
                                );
                                tiles.failed_cover_signature = Some(sig);
                            }
                        }
                        }
                        #[cfg(target_arch = "wasm32")]
                        {
                            let prepared = prepare_terrain_cover(request);
                            commit_prepared_terrain_cover(
                                &mut tiles,
                                &mut errs,
                                prepared,
                                oracle_key,
                                sig,
                                admission_sig,
                            );
                            sel.clear();
                            sel.extend(tiles.cover_selected.iter().copied());
                        }
                    }
                }
                wanted.clear();
                wanted.extend(sel.iter().map(|s| s.coord));
                wanted_parent_fallbacks(wanted, parent_fallbacks);

                // Bound the node-error memo. It only ever wiped on an oracle swap, so a
                // long traverse accumulated every node the camera ever visited. Past the
                // cap, keep exactly the cover-relevant set — the wanted leaves, their
                // parent fallbacks and every ancestor chain (the coords the next
                // selection pass re-reads) — so the trim costs the live cover no
                // re-probes (a full clear would re-measure ~1.6 k nodes in one frame).
                if errs.map.len() > NODE_GEOMETRY_CACHE_CAP {
                    let mut keep: HashSet<QuadCoord> = HashSet::with_capacity(
                        wanted.len() * (profile.terrain_lod_max_depth as usize + 1),
                    );
                    for &c in wanted.iter().chain(parent_fallbacks.iter()) {
                        let mut n = c;
                        while keep.insert(n) {
                            let Some(p) = n.parent() else { break };
                            n = p;
                        }
                    }
                    errs.map.retain(|c, _| keep.contains(c));
                }

                // Selection is authoritative. Drop superseded generations and camera
                // requests no longer selected before they can occupy every worker slot.
                // Intelligent baking, two phases:
                //
                // 1. CARPET — the selection's coarsest tiles first (a depth-N tile costs
                //    the same 49² samples as a leaf but covers 4^(maxdepth−N)× the
                //    area), so the whole view is covered by a low-res carpet within the
                //    first few frames instead of staying BLACK.
                // 2. BENEFIT — everything finer ordered by distance measured in units
                //    of the tile's own size (≈ inverse screen-space error): the leaf at
                //    the camera's feet outranks a mid-depth ring 800 m out. Strict
                //    depth-major order here was wrong on open: every mid-depth tile
                //    across the whole 1.5 km view baked before the leaves under the
                //    camera — the user watched FAR detail land while the near ground
                //    lagged coarse.
                //
                // The DISTANCE morph settles children onto the parent lattice, so the
                // out-of-depth-order coarse->fine handoff never pops. It is a pure function
                // of world position, which is what keeps neighbouring tiles agreeing at
                // their shared edge — see the note where the per-tile reveal was removed.
                // NOTE: the old `CARPET_DEPTH` two-tier key is gone. It bucketed depth <= 2 ahead of
                // everything else to bake a "carpet" first — but the selection is a REPLACE cover and
                // a site DEM never selects depths 0-2, so the branch never fired and no carpet ever
                // existed (that absence is what made the terrain go black). The coarse base now does
                // that job properly: it is enumerated, not selected, and queued ahead of `sel`. So
                // this sort only has to order the SELECTION, by screen-space benefit.
                // Minimum screen-space benefit across actual active camera positions.
                // Prefetch samples shape the selected cover, but are not substitute
                // viewpoints: letting an 80 m look-ahead endpoint compete as an equal
                // focus delayed the tile directly beneath the camera.
                let benefit = |s: &Selected| -> f64 {
                    let size = s.region.half * 2.0;
                    visual_foci
                        .iter()
                        .filter(|visual| visual.required)
                        .map(|visual| {
                            let to = bevy::math::DVec2::new(
                                s.region.center[0] - visual.focus[0],
                                s.region.center[1] - visual.focus[1],
                            );
                            let base = to.length_squared() / (size * size).max(1e-9);
                            let Some(heading) = visual.heading else {
                                return base;
                            };
                            let distance = to.length();
                            if distance < 1e-6 {
                                return base;
                            }
                            // cos 1 (dead ahead) → ×1 … cos −1 (behind) → ×4.
                            base * (2.5 - 1.5 * (to / distance).dot(heading))
                        })
                        .reduce(f64::min)
                        .unwrap_or(f64::INFINITY)
                };
                let camera_underfoot = |s: &Selected| {
                    visual_foci.iter().any(|visual| {
                        visual.required && s.region.distance_to(visual.focus) <= f64::EPSILON
                    })
                };
                // Decorate-sort-undecorate: `dist2`/`benefit` pay a sqrt/dot per call and
                // the comparator re-ran them on BOTH sides of every comparison
                // (O(n log n) evaluations) — compute each tile's key once, sort on the
                // cached key.
                keyed.clear();
                keyed.extend(sel.drain(..).map(|s| {
                    let priority = if camera_underfoot(&s) { 0 } else { 1 };
                    (priority, benefit(&s), s)
                }));
                keyed.sort_by(compare_terrain_bake_keys);
                sel.extend(keyed.drain(..).map(|(_, _, s)| s));

                // A coord is *fresh* (no work needed) when it has a resident tile OR an
                // in-flight bake tagged with the current generation. A stale entry (older
                // gen, from before a live re-bake) still renders but no longer counts as
                // satisfied, so a current-gen replacement is queued for it.
                let fresh_tile = |tiles: &LodView, c: &QuadCoord| tiles.fresh_slot(c, cur_gen);

                // Worker arrivals are only data. Accept results for the current owner key,
                // then publish meshes in stable tile order under the per-frame budget.
                // Offline capture waits at its presentation boundary for readiness rather
                // than blocking this system.
                done.clear();
                #[cfg(target_arch = "wasm32")]
                pending.tasks.retain(|coord, (r#gen, task)| {
                    match block_on(future::poll_once(&mut *task)) {
                        Some(baked) => {
                            done.push((*coord, *r#gen, baked));
                            false
                        }
                        None => true,
                    }
                });
                pending.ready.append(done);
                let ready = pending.take_ready(cur_gen, upload_budget);
                for (coord, baked) in ready {
                    if refused_uploads.contains(&(terrain, coord, cur_gen)) {
                        pending.ready.push((coord, cur_gen, baked));
                        continue;
                    }
                    let upload_bytes = baked.bytes as u64;
                    if !mesh_cache.admit_upload(upload_bytes, profile.terrain_mesh_cache_bytes) {
                        refused_uploads.insert((terrain, coord, cur_gen));
                        stream_status.mesh_upload_refused += 1;
                        stream_status.mesh_upload_required_bytes =
                            stream_status.mesh_upload_required_bytes.max(upload_bytes);
                        pending.ready.push((coord, cur_gen, baked));
                        continue;
                    }
                    upload_budget = upload_budget.saturating_sub(1);
                    let view_morph_end = sel
                        .iter()
                        .find(|selected| selected.coord == coord)
                        .map(|selected| {
                            if selected.morph_end.is_finite() {
                                selected.morph_end as f32
                            } else {
                                1.0e21
                            }
                        })
                        .unwrap_or(1.0e21);
                    let handle = meshes.add(baked.mesh);
                    let oy = baked.origin_y;
                    mesh_cache.insert(
                        (terrain, coord, baked.res),
                        handle.clone(),
                        oy,
                        baked.draw_bounds,
                        baked.bytes,
                    );
                    // Coarse-base tiles are live residents even though they are not selected
                    // leaves. Direct parent fallbacks are also live residents. Other completed
                    // work that is no longer wanted stays in the content cache.
                    if !wanted.contains(&coord)
                        && !parent_fallbacks.contains(&coord)
                        && !is_coarse_fallback(coord)
                    {
                        continue;
                    }
                    let depth = baked.depth;
                    // `oy` (baked with the mesh) anchors the tile's cell to its geometry — see
                    // `spawn_tile`/`bake_tile_mesh` `origin_y`. Using the baked value (not a
                    // spawn-time recompute) keeps mesh and placement in lock-step across gens.
                    let ent = spawn_tile(
                        &mut commands,
                        grid,
                        grid_entity,
                        terrain,
                        view_camera,
                        demand.render_layer,
                        coord,
                        handle,
                        baked.draw_bounds,
                        baked.center,
                        depth,
                        view_morph_end,
                        morph_ratio,
                        maps,
                        oracle.half_extent(),
                        oracle.regional_map_blend_m(),
                        oracle.regional_uv_scale(),
                        authored,
                        shadow,
                        template,
                        &environment.diagnostic.0,
                        overlay,
                        oy,
                        terrain_grid_position,
                        terrain_grid_rotation,
                    );
                    // Stage same-coordinate geometry without retiring the published slot.
                    if let Some(old) = tiles.insert_slot(
                        coord,
                        TileSlot {
                            resolution: tile_res,
                            half_extent: h,
                            entity: ent,
                            r#gen: cur_gen,
                            morph_end: view_morph_end,
                            drawn: false,
                            ready: false,
                            stitch_edges: [0.0; 4],
                        },
                    ) {
                        retire_tile(&mut commands, old.entity);
                    }
                }

                // ── Queue new work, nearest-first ────────────────────────────
                // Cache hits spawn instantly; native misses use shared admission and the
                // browser keeps its explicit cache task path. Tiles anchor to their OWN
                // big_space `CellCoord` so far-from-origin tiles keep f32 precision.
                #[cfg(target_arch = "wasm32")]
                let pool = AsyncComputeTaskPool::get();
                // The retained coarse cover is queued before the selection. It is a
                // complete DEM-derived floor; finer work never creates an uncovered area.
                coarse.clear();
                if !frozen && !tiles.coarse_ready {
                    for c in fallback_coords() {
                        coarse.push(Selected {
                            coord: c,
                            region: qt.region(c),
                            // A coarse-base tile is a stand-in, not a member of the selected
                            // cover: it must not morph toward a parent of its own. The
                            // non-finite sentinel maps to the shader's no-morph bucket.
                            morph_start: f64::INFINITY,
                            morph_end: f64::INFINITY,
                        });
                    }
                }
                // The complete coarse cover is a correctness prerequisite, not just a
                // priority hint. Starting fine bakes beside it can saturate the worker
                // pool on a cold cache and leave an area without a ready ancestor. Admit
                // only coarse work until the complete cover exists; then refine nearest-first.
                for (_priority_order, s) in coarse.iter().chain(sel.iter()).enumerate() {
                    if !tiles.coarse_ready && !is_coarse_fallback(s.coord) {
                        continue;
                    }
                    // Skip coords already satisfied at the current generation (resident tile
                    // or in-flight current-gen bake). Stale tiles fall through → re-baked.
                    let have_pending = pending.has_current(s.coord, cur_gen);
                    if fresh_tile(&tiles, &s.coord) || have_pending {
                        continue;
                    }
                    if pending.failed.contains(&(s.coord, cur_gen)) {
                        continue;
                    }
                    let resolution = tile_res;
                    let depth = s.coord.depth as u32;
                    // Per-node morph band: finite for sub-root nodes, "never" for the root
                    // (the sentinel `snap_band` maps to the no-morph bucket).
                    let morph_end = if s.morph_end.is_finite() {
                        s.morph_end as f32
                    } else {
                        1.0e21
                    };
                    if let Some((cached, oy, draw_bounds)) =
                        mesh_cache.get(&(terrain, s.coord, resolution))
                    {
                        // Placed at the mesh's OWN baked `origin_y` (stored beside it), never a
                        // recompute — otherwise a cache hit against a since-composed oracle jumps.
                        let ent = spawn_tile(
                            &mut commands,
                            grid,
                            grid_entity,
                            terrain,
                            view_camera,
                            demand.render_layer,
                            s.coord,
                            cached,
                            draw_bounds,
                            s.region.center,
                            depth,
                            morph_end,
                            morph_ratio,
                            maps,
                            oracle.half_extent(),
                            oracle.regional_map_blend_m(),
                            oracle.regional_uv_scale(),
                            authored,
                            shadow,
                            template,
                            &environment.diagnostic.0,
                            overlay,
                            oy,
                            terrain_grid_position,
                            terrain_grid_rotation,
                        );
                        if let Some(old) = tiles.insert_slot(
                            s.coord,
                            TileSlot {
                                resolution: tile_res,
                                half_extent: h,
                                entity: ent,
                                r#gen: cur_gen,
                                morph_end,
                                drawn: false,
                                ready: false,
                                stitch_edges: [0.0; 4],
                            },
                        ) {
                            retire_tile(&mut commands, old.entity);
                        }
                        continue;
                    }
                    // Cache miss → needs a bake. Respect the per-frame and per-terrain
                    // budgets while the shared native admission enforces the global worker
                    // bound. Keep scanning for cheap cache hits regardless of bake budget.
                    if bake_budget == 0 || pending.len() >= max_inflight {
                        continue;
                    }
                    let oracle_arc = hf.0.clone();
                    let coord = s.coord;
                    let region = s.region;
                    let half = h;
                    let center = s.region.center;
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let capacity_revision = cover_work.admission.capacity_revision();
                        if pending.capacity_wait_revision == Some(capacity_revision) {
                            break;
                        }
                        pending.capacity_wait_revision = None;

                        let Some(operation) = pending.next_operation.checked_add(1) else {
                            bevy::log::error!(
                                target: "terrain_stream",
                                entity = ?terrain,
                                coord = ?coord,
                                "terrain tile bake operation identity is exhausted"
                            );
                            pending.failed.insert((coord, cur_gen));
                            continue;
                        };
                        pending.next_operation = operation;
                        let key = lunco_core_runtime::AsyncWorkKey::new(
                            lunco_core_runtime::AsyncWorkKind::VisualizationPreparation,
                            0,
                            terrain.to_bits() as u128,
                            cur_gen as u64,
                            operation,
                        );
                        let queue = Arc::clone(&cover_work.tile_results.queue);
                        let bake_oracle = Arc::clone(&oracle_arc);
                        let depth = coord.depth as u32;

                        let result_key = key;
                        let job = move || {
                            let result = catch_unwind(AssertUnwindSafe(|| {
                                let _span = bevy::log::info_span!("terrain_tile_bake").entered();
                                let tile_mesh = crate::tile_cache::bake_tile_mesh_cached(
                                    bake_oracle.as_ref(),
                                    coord,
                                    region,
                                    resolution,
                                    half,
                                    center,
                                );
                                assemble_baked_tile(
                                    tile_mesh,
                                    bake_oracle.as_ref(),
                                    center,
                                    depth,
                                    resolution,
                                )
                            }))
                            .map_err(|_| ());
                            queue
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .push_back(TerrainTileBakeCompletion {
                                    terrain,
                                    coord,
                                    key: result_key,
                                    result,
                                });
                        };
                        match cover_work.admission.submit_ordered(
                            lunco_core_runtime::AsyncWorkPriority::Interactive,
                            key,
                            _priority_order as u64,
                            job,
                        ) {
                            Ok(()) => {
                                pending.jobs.insert(
                                    coord,
                                    PendingTileBake {
                                        generation: cur_gen,
                                        key,
                                    },
                                );
                                pending.capacity_wait_revision = None;
                                bake_budget -= 1;
                            }
                            Err(lunco_core_runtime::AsyncWorkRejection::QueueFull) => {
                                pending.capacity_wait_revision =
                                    Some(cover_work.admission.capacity_revision());
                                break;
                            }
                            Err(rejection) => {
                                bevy::log::error!(
                                    target: "terrain_stream",
                                    entity = ?terrain,
                                    coord = ?coord,
                                    ?rejection,
                                    "terrain tile bake was rejected by shared preparation admission"
                                );
                                pending.failed.insert((coord, cur_gen));
                            }
                        }
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        bake_budget -= 1;
                        let tile_generation = cur_gen;
                        let task = pool.spawn(async move {
                            let _span = bevy::log::info_span!("terrain_tile_bake").entered();
                            let tile_mesh = crate::tile_cache::bake_tile_mesh_cached_async(
                                Arc::clone(&oracle_arc),
                                coord,
                                region,
                                resolution,
                                half,
                                center,
                            )
                            .await;
                            assemble_baked_tile(
                                tile_mesh,
                                oracle_arc.as_ref(),
                                center,
                                coord.depth as u32,
                                resolution,
                            )
                        });
                        pending.tasks.insert(coord, (tile_generation, task));
                    }
                }

                // The base is complete once every enumerated coarse node is resident at this
                // generation. Latching it stops the per-frame enumeration AND re-arms the idle
                // fast path (which is held off until then — see `LodView::coarse_ready`).
                if !tiles.coarse_ready && !coarse.is_empty() {
                    tiles.coarse_ready = coarse.iter().all(|s| {
                        tiles
                            .candidate_slot(&s.coord)
                            .is_some_and(|slot| slot.r#gen == cur_gen)
                    });
                }

                // ── Unrefinement: draw the best READY data, never a hole ─────────────
                // Cesium's `ForbidHoles`: "unrefine back to a parent tile when a child isn't done
                // loading… never rendered with holes, though the tile rendered instead may have low
                // resolution". MSFS states the same rule as "draw tiles using the best currently
                // available data ● tiles can use data from a parent". Both draw the parent INSTEAD
                // of its children — never underneath them, which would punch a coarse shell through
                // the fine surface wherever the terrain is concave.
                //
                // So: each selected node draws itself if ready, else its deepest ready ancestor.
                // The coarse base guarantees that walk terminates, which is what the old design
                // lacked — it could only hold a trailing tile that happened to overlap, so panning
                // somewhere new had nothing to fall back to and showed clear colour.
                build_draw_partition(
                    sel.iter().map(|s| s.coord),
                    // RESIDENCY. A stale tile keeps drawing itself until its own re-bake
                    // replaces it in place — see this function's docs.
                    |c| tiles.candidate_slot(&c).is_some(),
                    draw,
                    drop_covered,
                );

                // Mesh replacement, snapped morph bands and stitches describe one
                // candidate surface. None may replace the published partition until
                // that entire candidate is safe for the stationary navigating owner.
                let selected_ends: HashMap<_, _> = sel
                    .iter()
                    .map(|selected| {
                        (
                            selected.coord,
                            if selected.morph_end.is_finite() {
                                selected.morph_end as f32
                            } else {
                                1.0e21
                            },
                        )
                    })
                    .collect();
                let mut candidate: Vec<_> = draw
                    .iter()
                    .filter_map(|coord| {
                        let slot = tiles.candidate_slot(coord)?;
                        Some(DrawPublicationEntry {
                            resolution: slot.resolution,
                            half_extent: slot.half_extent,
                            coord: *coord,
                            entity: slot.entity,
                            morph_end: selected_ends.get(coord).copied().unwrap_or(slot.morph_end),
                            stitch_edges: stitch_edges(*coord, draw),
                        })
                    })
                    .collect();
                candidate.sort_unstable_by_key(|entry| {
                    (entry.coord.depth, entry.coord.x, entry.coord.z)
                });
                let changed = tiles.presentation_pending
                    || tiles.morph_ratio.to_bits() != morph_ratio.to_bits()
                    || tiles.tiles.values().filter(|slot| slot.drawn).count() != candidate.len()
                    || candidate.iter().any(|entry| {
                        tiles.tiles.get(&entry.coord).is_none_or(|slot| {
                            slot.entity != entry.entity
                                || !slot.drawn
                                || slot.morph_end.to_bits() != entry.morph_end.to_bits()
                                || slot.stitch_edges != entry.stitch_edges
                        })
                    });
                let admission = if changed {
                    environment.admit_publication(
                        demand.source,
                        grid_entity,
                        &candidate,
                        morph_ratio,
                        tile_res,
                        h,
                        &meshes,
                        tiles.presentation_wait.as_ref(),
                    )
                } else {
                    Ok(())
                };
                let admitted = admission.is_ok();
                if let Err(wait) = admission {
                    tiles.presentation_wait = Some(wait);
                }
                if admitted {
                    // One clone merges every geometry-affecting parameter. Applying
                    // independent morph and stitch writes could overwrite one another.
                    for entry in &candidate {
                        if let Ok((_, look, _, _)) = environment.draw_geometry.get(entry.entity) {
                            if !publication_look_matches(&look, entry, morph_ratio) {
                                let entry = *entry;
                                commands.entity(entry.entity).queue(
                                    move |mut entity: EntityWorldMut| {
                                        if let Some(mut look) = entity.get_mut::<ShaderLook>() {
                                            if let Some(replacement) =
                                                publication_look(&look, &entry, morph_ratio)
                                            {
                                                *look = replacement;
                                            }
                                        }
                                    },
                                );
                            }
                        }
                    }
                }
                let changes = tiles.publish_draw_partition(&candidate, morph_ratio, admitted);
                for entity in changes.retired {
                    retire_tile(&mut commands, entity);
                }
                for (entity, visible) in changes.visibility {
                    commands.entity(entity).try_insert(if visible {
                        Visibility::Inherited
                    } else {
                        Visibility::Hidden
                    });
                }
                // A rejected candidate may release hidden work, but never a single
                // member of the old visible partition. Staging stays bounded to one
                // unpublished replacement per retained coordinate.
                let retain = |coord: &QuadCoord| {
                    is_coarse_fallback(*coord)
                        || wanted.contains(coord)
                        || parent_fallbacks.contains(coord)
                };
                tiles.tiles.retain(|coord, slot| {
                    let keep = slot.drawn || retain(coord);
                    if !keep {
                        retire_tile(&mut commands, slot.entity);
                    }
                    keep
                });
                tiles.pending_replacements.retain(|coord, slot| {
                    let keep = retain(coord);
                    if !keep {
                        retire_tile(&mut commands, slot.entity);
                    }
                    keep
                });
                // Diagnostics below describe the actually published topology.
                draw.clear();
                draw.extend(
                    tiles
                        .tiles
                        .iter()
                        .filter_map(|(coord, slot)| slot.drawn.then_some(*coord)),
                );

                // The first cover may now begin adapting normally. Until its complete
                // camera-near patch is prepared it is intentionally immutable: changing
                // that cover while the rig settles only creates startup churn.
                if !tiles.presentation_pending
                    && !tiles.bootstrap_ready
                    && bootstrap_cover_is_ready(sel, visual_foci, |coord| {
                        tiles.tiles.get(&coord).is_some_and(|slot| slot.ready)
                    })
                {
                    tiles.bootstrap_ready = true;
                    // The cover was intentionally held while the camera rig settled;
                    // force one clean signature pass now that normal adaptation resumes.
                    if !frozen {
                        tiles.last_sig = None;
                    }
                }

                // How many selected areas have NO cover at all — the metric that must stay 0.
                // Non-zero means something rendered as clear colour, and it can only happen
                // before the coarse base has finished baking.
                let uncovered = sel
                    .iter()
                    .filter(|s| {
                        let mut c = s.coord;
                        loop {
                            if draw.contains(&c) {
                                return false;
                            }
                            match c.parent() {
                                Some(p) => c = p,
                                None => return true,
                            }
                        }
                    })
                    .count();
                if uncovered > 0 {
                    let ready = tiles.tiles.values().filter(|slot| slot.ready).count();
                    let selected_ready = sel
                        .iter()
                        .filter(|selected| {
                            tiles
                                .tiles
                                .get(&selected.coord)
                                .is_some_and(|slot| slot.ready)
                        })
                        .count();
                    debug!(
                        target: "terrain_stream",
                        uncovered,
                        wanted = wanted.len(),
                        drawn = draw.len(),
                        resident = tiles.tiles.len(),
                        ready,
                        selected_ready,
                        coarse_ready = tiles.coarse_ready,
                        materials_ready = tiles.materials_ready,
                        backlog = pending.len(),
                        "terrain has uncovered area (coarse base still baking?)"
                    );
                }

                // Streaming progress for the UI indicator (accumulated across terrains).
                stream_status.wanted += wanted.len();
                stream_status.resident += wanted
                    .iter()
                    .filter(|c| tiles.tiles.get(c).is_some_and(|slot| slot.ready))
                    .count();
                let selection_pending = usize::from(!tiles.latest_ready());
                stream_status.selection_pending += selection_pending;
                stream_status.presentation_pending += usize::from(tiles.presentation_pending);
                stream_status.pending +=
                    render_pending_count(0, wanted, &tiles) + selection_pending;
                stream_status.budget_refused += tiles.budget_refused;
                stream_status.preparation_refused += tiles.preparation_refusal_count(
                    demands.preparation_signature(view_camera, terrain),
                    admission_sig,
                );
                add_focus_readiness(
                    &mut stream_status,
                    visual_foci,
                    sel.iter().map(|selected| selected.coord),
                    &qt,
                    |coord| {
                        tiles
                            .tiles
                            .get(&coord)
                            .is_some_and(|slot| slot.ready && slot.drawn)
                    },
                );
            }
            // Shared bakes survive while any live view needs their geometry.
            let mut union = HashSet::new();
            for view in terrain_tiles.views.values() {
                for selected in &view.cover_selected {
                    union.insert(selected.coord);
                    if let Some(parent) = selected.coord.parent() {
                        union.insert(parent);
                    }
                }
            }
            let keep = |coord: &QuadCoord| union.contains(coord) || is_coarse_fallback(*coord);
            let before = pending.len();
            #[cfg(not(target_arch = "wasm32"))]
            pending.jobs.retain(|coord, job| {
                let retained = job.generation == generation && keep(coord);
                if !retained {
                    cover_work.admission.cancel_queued(job.key);
                }
                retained
            });
            #[cfg(target_arch = "wasm32")]
            pending
                .tasks
                .retain(|coord, (r#gen, _)| *r#gen == generation && keep(coord));
            pending
                .ready
                .retain(|(coord, r#gen, _)| *r#gen == generation && keep(coord));
            pending
                .failed
                .retain(|(coord, r#gen)| *r#gen == generation && keep(coord));
            stream_status.stale_cancelled += before.saturating_sub(pending.len());
            mesh_cache.trim(
                mesh_cache_entry_cap(
                    profile.terrain_lod_tile_budget,
                    terrain_count * live.len().max(1),
                ),
                profile.terrain_mesh_cache_bytes,
            );
        }
    }

    stream_status.draw_revision_exhausted = terrains
        .iter()
        .map(|(_, _, _, tiles, ..)| {
            tiles
                .views
                .values()
                .filter(|view| view.draw_revision_exhausted)
                .count()
        })
        .sum();
    mesh_cache.publish_accounting(&mut stream_status, profile.terrain_mesh_cache_bytes);
    publish_mesh_pressure(&stream_status, &mut environment.diagnostics);
}

/// Reap LOD tiles whose owning terrain no longer exists (or no longer streams) —
/// e.g. after a twin reload. Tiles are children of the big_space grid (so each can
/// carry its own `CellCoord`), so they don't die with the terrain entity; this is
/// their lifecycle tether.
///
/// Change-driven: a tile only orphans when its owner loses [`TerrainLodViz`]
/// (component removal or terrain despawn — both emit the removal event), so the
/// per-frame every-tile ownership poll is skipped until one fires. The liveness
/// re-check keeps the exact old semantics for a remove-and-re-add in one frame.
pub fn despawn_orphaned_lod_tiles(
    mut commands: Commands,
    mut removed: RemovedComponents<TerrainLodViz>,
    tiles: Query<(Entity, &LodTileOf)>,
    streaming: Query<(), With<TerrainLodViz>>,
    mut mesh_cache: ResMut<LodMeshCache>,
) {
    let orphaned: HashSet<Entity> = removed.read().collect();
    if orphaned.is_empty() {
        return;
    }
    for (ent, owner) in &tiles {
        if orphaned.contains(&owner.0) && streaming.get(owner.0).is_err() {
            commands.entity(ent).try_despawn();
        }
    }
    let dead = |t: &Entity| orphaned.contains(t) && streaming.get(*t).is_err();
    // (The dead terrain's cached MATERIALS — which pin its derived-map images,
    // megabytes of GPU texture — are no longer this crate's problem: the tiles hold
    // the only `ShaderLook`s that key them, so `lunco-render-bevy`'s binder cache
    // sweep drops them once they are unreferenced. This crate owns no material.)
    //
    // Its cached MESHES still are. The LRU trim in `update_lod_tiles` would
    // eventually cycle a dead terrain's entries out, but they'd squat the byte
    // ceiling meanwhile (hundreds of MB per dead terrain across every twin
    // reload / scene swap) — evict them the moment the owner dies.
    mesh_cache.retain_terrains(|t| !dead(&t));
}

/// Reconcile a terrain's complete material source selection after either USD
/// authored maps, engine-derived maps, or the authored `ShaderLook` arrive.
/// The owner and its streamed tiles use this one reader path, so two independent
/// late binders cannot observe the same frame in opposite orders and overwrite
/// one source with another.
pub(crate) fn bind_terrain_maps_to_materials(
    changed: Query<
        (
            Entity,
            Option<&LodTiles>,
            Option<&TerrainDerivedMaps>,
            Option<&TerrainAuthoredMaps>,
            Option<&DemHeightField>,
            &ShaderLook,
        ),
        (
            With<DemTerrainSurface>,
            Or<(
                Changed<TerrainDerivedMaps>,
                Changed<TerrainAuthoredMaps>,
                Changed<DemHeightField>,
                Changed<ShaderLook>,
            )>,
        ),
    >,
    tile_looks: Query<(&ShaderLook, Option<&TerrainDiagnosticTile>), Without<DemTerrainSurface>>,
    mut commands: Commands,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
) {
    for (terrain, tiles, maps, authored, heightfield, look) in changed {
        // The generic USD shader projection already put the material network into
        // this look. Publish the terrain-specific view from those same handles so
        // no second USD reader is needed. The temporary is also used immediately,
        // so the owner does not render one frame with the derived source before the
        // deferred component insertion becomes visible.
        let inferred = TerrainAuthoredMaps::from_shader_look(&look, maps);
        if inferred.regional_albedo.is_some()
            && heightfield.is_some_and(|field| field.0.regional_uv_scale() <= 0.0)
        {
            faults.raise(
                "terrain-regional-material",
                Some(terrain),
                look.shader.clone(),
                "regional albedo requires a registered regional DEM",
            );
            continue;
        }
        let source_changed = authored != Some(&inferred);
        if source_changed {
            commands.entity(terrain).try_insert(inferred.clone());
        }
        let authored = source_changed.then_some(&inferred).or(authored);
        let mut terrain_look = look.clone();
        apply_terrain_maps_to_look(&mut terrain_look, maps, authored);
        if terrain_look != *look {
            commands.entity(terrain).insert(terrain_look);
        }
        if let Some(tiles) = tiles {
            for entity in tiles.tile_entities() {
                if let Ok((look, diagnostic)) = tile_looks.get(entity) {
                    if diagnostic.is_none() {
                        let mut tile_look = look.clone();
                        apply_terrain_maps_to_look(&mut tile_look, maps, authored);
                        if tile_look != *look {
                            let maps = maps.cloned();
                            let authored = authored.cloned();
                            commands
                                .entity(entity)
                                .queue(move |mut tile: EntityWorldMut| {
                                    if tile.contains::<TerrainDiagnosticTile>() {
                                        return;
                                    }
                                    if let Some(mut current) = tile.get_mut::<ShaderLook>() {
                                        let mut replacement = current.clone();
                                        apply_terrain_maps_to_look(
                                            &mut replacement,
                                            maps.as_ref(),
                                            authored.as_ref(),
                                        );
                                        if replacement != *current {
                                            *current = replacement;
                                        }
                                    }
                                });
                        }
                    }
                }
            }
        }
    }
}

/// Reconcile resident looks when a published material source is removed. A
/// whole-terrain recompose may remove the derived product before the replacement
/// bake publishes; authored USD maps remain eligible in that interval.
pub(crate) fn sync_removed_terrain_maps_to_materials(
    mut removed_derived: RemovedComponents<TerrainDerivedMaps>,
    mut removed_authored: RemovedComponents<TerrainAuthoredMaps>,
    terrains: Query<
        (
            Entity,
            Option<&LodTiles>,
            Option<&TerrainDerivedMaps>,
            Option<&TerrainAuthoredMaps>,
        ),
        With<DemTerrainSurface>,
    >,
    mut looks: Query<&mut ShaderLook, (Without<TerrainDiagnosticTile>, With<DemTerrainSurface>)>,
    mut tile_looks: Query<
        (&mut ShaderLook, Option<&TerrainDiagnosticTile>),
        Without<DemTerrainSurface>,
    >,
) {
    let removed: HashSet<Entity> = removed_derived
        .read()
        .chain(removed_authored.read())
        .collect();
    for terrain in removed {
        let Ok((owner, tiles, maps, authored)) = terrains.get(terrain) else {
            continue;
        };
        if let Ok(mut look) = looks.get_mut(owner) {
            apply_terrain_maps_to_look(&mut look, maps, authored);
        }
        if let Some(tiles) = tiles {
            for entity in tiles.tile_entities() {
                if let Ok((mut look, diagnostic)) = tile_looks.get_mut(entity) {
                    if diagnostic.is_none() {
                        apply_terrain_maps_to_look(&mut look, maps, authored);
                    }
                }
            }
        }
    }
}

/// Same late-bind for the far-shadow cache: the horizon/shadow-cache bake (and
/// every sun-driven re-bake) lands long after tiles exist — restate it on the
/// resident production tiles' looks. Diagnostic tiles deliberately have no
/// terrain-map or shadow inputs.
pub(crate) fn bind_shadow_cache_to_tiles(
    mut commands: Commands,
    changed: Query<(&TileShadowCache, &LodTiles), (Changed<TileShadowCache>, With<TerrainLodViz>)>,
    mut looks: Query<(&mut ShaderLook, Option<&TerrainDiagnosticTile>)>,
) {
    for (cache, tiles) in &changed {
        for entity in tiles.tile_entities() {
            let mut tile = commands.entity(entity);
            // Keep already-resident tiles on the same terrain self-shadow
            // ownership contract as newly spawned tiles. They remain ordinary
            // CSM receivers for dynamic-object shadows.
            tile.try_remove::<bevy::light::NotShadowReceiver>();
            if let Ok((mut look, diagnostic)) = looks.get_mut(entity) {
                if diagnostic.is_none() {
                    apply_shadow_cache_to_look(&mut look, cache);
                }
            }
        }
    }
}

#[cfg(test)]
#[expect(
    deprecated,
    reason = "Bevy reexports DMat4 but does not expose glam's dcamera replacement"
)]
fn test_view(eye: DVec3, forward: DVec3, up: DVec3) -> DMat4 {
    DMat4::look_to_rh(eye, forward, up)
}

#[cfg(test)]
fn test_perspective(fov_y_rad: f64, eye: DVec3, forward: DVec3) -> TerrainProjection {
    let up = if forward.y.abs() > 0.99 {
        DVec3::NEG_Z
    } else {
        DVec3::Y
    };
    TerrainProjection::Perspective {
        fov_y_rad,
        clip_from_local: bevy::math::dproj::perspective_infinite_reverse(fov_y_rad, 1.0, 0.1)
            * test_view(eye, forward, up),
    }
}

#[cfg(test)]
mod draw_partition_tests {
    use super::*;

    #[test]
    fn initial_preparation_bootstraps_then_reaches_a_real_fixed_point() {
        let quadtree = Quadtree::new(100.0, 6, 20.0, 100.0);
        let oracle = Arc::new(SurfaceOracle::bare(Arc::new(
            lunco_obstacle_field::field::HeightGrid::new_flat(3, 100.0),
        )));
        let focus = TerrainVisualDemand {
            camera: Entity::PLACEHOLDER,
            focus: [11.3, 13.7],
            camera_y: 10.0,
            heading: None,
            view_direction: DVec3::NEG_Y,
            screen_height_px: 720.0,
            selection_clip: SelectionClip::new(
                test_perspective(2.8, DVec3::new(11.3, 10.0, 13.7), DVec3::NEG_Y),
                720.0,
                2.0,
                DVec3::new(11.3, 10.0, 13.7),
            )
            .unwrap(),
            projection: test_perspective(2.8, DVec3::new(11.3, 10.0, 13.7), DVec3::NEG_Y),
            required: true,
            near_detail_radius_m: 30.0,
            near_detail_hysteresis_m: 12.0,
        };
        let signature = 77;
        let mut view = LodView {
            requested_signature: Some(signature),
            ..default()
        };
        let mut geometry = TerrainNodeGeometry::default();
        let request = |view: &LodView, geometry: &TerrainNodeGeometry| TerrainCoverRequest {
            quadtree,
            oracle: oracle.clone(),
            oracle_key: oracle.surface_key(),
            demands: vec![focus],
            camera_patches: view.camera_patches.clone(),
            cover: view.cover.clone(),
            node_geometry: geometry.map.clone(),
            bootstrap_ready: view.bootstrap_ready,
            settled_sig: view.settled_sig,
            budget_refused: view.budget_refused,
            signature,
            preparation_signature: None,
            preparation: None,
            tile_resolution: 5,
            probe_resolution: 3,
            tile_budget: 512,
            max_depth: 6,
            cover_edits_per_cycle: 8,
            hysteresis_ratio: 0.8,
        };
        let acknowledge_selected = |view: &mut LodView| {
            for slot in view.tiles.values_mut() {
                slot.drawn = false;
            }
            for selected in &view.cover_selected {
                view.tiles.insert(
                    selected.coord,
                    TileSlot {
                        resolution: 3,
                        half_extent: 1.0,
                        entity: Entity::PLACEHOLDER,
                        r#gen: view.r#gen,
                        morph_end: if selected.morph_end.is_finite() {
                            selected.morph_end as f32
                        } else {
                            1.0e21
                        },
                        ready: true,
                        drawn: true,
                        stitch_edges: [0.0; 4],
                    },
                );
            }
        };
        let initial = prepare_terrain_cover(request(&view, &geometry));
        assert_eq!(initial.settled_sig, None);
        commit_prepared_terrain_cover(
            &mut view,
            &mut geometry,
            initial,
            oracle.surface_key(),
            signature,
            9,
        );
        assert_eq!(
            view.last_sig, None,
            "bootstrap selection is not evidence of a settled zero-edit pass"
        );
        acknowledge_selected(&mut view);
        view.bootstrap_ready = bootstrap_cover_is_ready(&view.cover_selected, &[focus], |coord| {
            view.tiles[&coord].ready
        });
        assert!(view.bootstrap_ready);
        assert!(view.cover_preparation_needed(signature));
        assert!(!view.latest_ready());
        let mut saw_more_work = false;
        for _ in 0..128 {
            if !view.cover_preparation_needed(signature) {
                break;
            }
            let prepared = prepare_terrain_cover(request(&view, &geometry));
            let more_work = prepared.more_work;
            saw_more_work |= more_work;
            commit_prepared_terrain_cover(
                &mut view,
                &mut geometry,
                prepared,
                oracle.surface_key(),
                signature,
                9,
            );
            acknowledge_selected(&mut view);
            if more_work {
                assert_eq!(
                    view.last_sig, None,
                    "a useful same-signature subset keeps its required follow-up runnable"
                );
                assert!(view.cover_preparation_needed(signature));
            }
        }
        assert!(saw_more_work);
        assert_eq!(view.settled_sig, Some(signature));
        assert!(!view.cover_preparation_needed(signature));
        assert!(
            view.latest_ready(),
            "a completed real zero-edit fixed point releases latest-cover readiness"
        );
    }

    fn ready_view_fixture() -> (
        TerrainDetailDemands,
        Entity,
        Entity,
        lunco_render::RenderQualityProfile,
        LodView,
    ) {
        let camera = Entity::from_bits(1);
        let terrain = Entity::from_bits(2);
        let profile = terrain_quality();
        let demands = TerrainDetailDemands {
            visual: vec![VisualDemand {
                entity: camera,
                source: camera,
                render_layer: 64,
                terrain_poses: HashMap::from([(terrain, (DVec3::Y, DVec3::NEG_Z))]),
                terrain_clips: HashMap::new(),
                preparation_corridors: HashMap::new(),
                position: DVec3::Y,
                forward: DVec3::NEG_Z,
                screen_height_px: 720.0,
                projection: test_perspective(1.0, DVec3::ZERO, DVec3::NEG_Z),
                near_detail_radius_m: 30.0,
                near_detail_hysteresis_m: 12.0,
            }],
        };
        let mut demands = demands;
        let visual = &mut demands.visual[0];
        visual.terrain_clips.insert(
            terrain,
            visual.projection.clip_from_local() * test_view(DVec3::Y, DVec3::NEG_Z, DVec3::Y),
        );
        let quadtree = Quadtree::new(100.0, 5, 1.0, 100.0);
        let mut view = LodView {
            r#gen: 4,
            cover: HashSet::from([QuadCoord::ROOT]),
            cover_selected: vec![quadtree.selected(QuadCoord::ROOT, f64::INFINITY)],
            ..default()
        };
        view.tiles.insert(
            QuadCoord::ROOT,
            TileSlot {
                resolution: 3,
                half_extent: 1.0,
                entity: camera,
                r#gen: 4,
                morph_end: 1.0e21,
                ready: true,
                drawn: true,
                stitch_edges: [0.0; 4],
            },
        );
        view.publish_latest_demand(
            &demands,
            terrain,
            &demands.visual[0],
            3,
            Some(&profile),
            false,
        );
        view.last_sig = view.requested_signature;
        view.settled_sig = view.requested_signature;
        (demands, camera, terrain, profile, view)
    }

    #[test]
    fn draw_publication_refusal_retains_partition_and_staged_replacement_recovers_atomically() {
        let (_, camera, _, _, mut view) = ready_view_fixture();
        let coord = QuadCoord::ROOT;
        view.morph_ratio = 0.65;
        view.resolution = 65;
        view.half_extent = 500.0;
        let old = view.tiles[&coord];
        view.r#gen += 1;
        let replacement = Entity::from_bits(3);
        assert!(
            view.insert_slot(
                coord,
                TileSlot {
                    entity: replacement,
                    r#gen: view.r#gen,
                    resolution: 65,
                    half_extent: 500.0,
                    morph_end: 4.0,
                    stitch_edges: [0.0; 4],
                    ready: true,
                    drawn: false
                }
            )
            .is_none()
        );
        let candidate = [DrawPublicationEntry {
            coord,
            entity: replacement,
            resolution: 65,
            half_extent: 500.0,
            morph_end: 12.0,
            stitch_edges: [1.0, 0.0, 1.0, 0.0],
        }];
        let before = view.draw_revision;
        let refused = view.publish_draw_partition(&candidate, 0.8, false);
        assert!(refused.retired.is_empty() && refused.visibility.is_empty());
        assert_eq!(view.tiles[&coord].entity, old.entity);
        assert!(view.tiles[&coord].drawn);
        assert_eq!(view.tiles[&coord].morph_end, old.morph_end);
        assert_eq!(view.tiles[&coord].stitch_edges, old.stitch_edges);
        assert_eq!(view.morph_ratio, 0.65);
        assert_eq!(
            view.draw_revision, before,
            "unpublished geometry must not rewrite the drawn lease"
        );
        assert_eq!(
            view.tile_entities().count(),
            2,
            "both published and staged meshes remain lifecycle-owned"
        );
        assert!(view.presentation_pending && !view.latest_ready());
        let mut terrain = LodTiles::default();
        terrain.views.insert(camera, view);
        assert_eq!(
            terrain.draw_lattice(camera, coord),
            Some((old.resolution, old.half_extent, false))
        );
        assert!(terrain.view_status(camera).unwrap().presented_ready);
        let view = terrain.views.get_mut(&camera).unwrap();
        let committed = view.publish_draw_partition(&candidate, 0.8, true);
        assert_eq!(committed.retired, vec![old.entity]);
        assert_eq!(committed.visibility, vec![(replacement, true)]);
        assert!(view.pending_replacements.is_empty());
        assert_eq!(view.tiles[&coord].entity, replacement);
        assert_eq!(view.tiles[&coord].stitch_edges, candidate[0].stitch_edges);
        assert_eq!(view.tiles[&coord].morph_end, 12.0);
        assert!(!view.presentation_pending);
        assert!(view.draw_revision > before);
        assert_eq!(terrain.draw_lattice(camera, coord), Some((65, 500.0, true)));
    }

    #[test]
    fn admitted_partition_commits_ready_hidden_fallback_replacements() {
        let (_, _, _, _, mut view) = ready_view_fixture();
        let root = view.tiles[&QuadCoord::ROOT];
        let child = QuadCoord::ROOT.children()[0];
        let old_hidden = Entity::from_bits(3);
        let new_hidden = Entity::from_bits(4);
        view.tiles.insert(
            child,
            TileSlot {
                entity: old_hidden,
                drawn: false,
                ..root
            },
        );
        view.insert_slot(
            child,
            TileSlot {
                entity: new_hidden,
                drawn: false,
                ..root
            },
        );
        let candidate = [DrawPublicationEntry {
            coord: QuadCoord::ROOT,
            entity: root.entity,
            resolution: root.resolution,
            half_extent: root.half_extent,
            morph_end: root.morph_end,
            stitch_edges: root.stitch_edges,
        }];
        let committed = view.publish_draw_partition(&candidate, view.morph_ratio, true);
        assert_eq!(committed.retired, vec![old_hidden]);
        assert!(committed.visibility.is_empty());
        assert!(
            view.pending_replacements.is_empty(),
            "unused ready fallback replacements cannot keep the view perpetually active"
        );
        assert_eq!(view.tiles[&child].entity, new_hidden);
        assert!(!view.tiles[&child].drawn);
        assert!(view.tiles[&QuadCoord::ROOT].drawn);
    }

    #[test]
    fn repeated_source_invalidation_reaps_hidden_work_but_preserves_published_cover() {
        let (_, _, _, _, mut view) = ready_view_fixture();
        let old = view.tiles[&QuadCoord::ROOT];
        let staged = Entity::from_bits(3);
        view.insert_slot(
            QuadCoord::ROOT,
            TileSlot {
                entity: staged,
                drawn: false,
                ..old
            },
        );
        let hidden_coord = QuadCoord::ROOT.children()[0];
        let hidden = Entity::from_bits(4);
        view.tiles.insert(
            hidden_coord,
            TileSlot {
                entity: hidden,
                drawn: false,
                ..old
            },
        );
        view.invalidate_region(None, 1.0);
        let dead = view.reap_stale();
        assert!(dead.contains(&staged) && dead.contains(&hidden));
        assert_eq!(dead.len(), 2);
        assert_eq!(view.tile_entities().collect::<Vec<_>>(), vec![old.entity]);
        view.invalidate_region(None, 1.0);
        assert!(view.reap_stale().is_empty());
        assert!(
            view.tiles[&QuadCoord::ROOT].drawn,
            "successive source revisions cannot withdraw the last safe partition"
        );
    }

    #[test]
    fn staged_material_readiness_and_merged_publication_look_preserve_published_state() {
        let (_, _, _, _, mut view) = ready_view_fixture();
        let coord = QuadCoord::ROOT;
        let old = view.tiles[&coord];
        let staged = Entity::from_bits(3);
        view.insert_slot(
            coord,
            TileSlot {
                entity: staged,
                drawn: false,
                ..old
            },
        );
        assert!(!view.invalidate_material_readiness(staged));
        assert!(view.tiles[&coord].drawn && view.tiles[&coord].ready);
        assert!(!view.pending_replacements[&coord].ready);
        assert_eq!(view.tile_material_specs().count(), 2);
        let entry = DrawPublicationEntry {
            coord,
            entity: staged,
            resolution: 3,
            half_extent: 1.0,
            morph_end: 12.0,
            stitch_edges: [1.0, 0.0, 0.0, 1.0],
        };
        let look = ShaderLook::new("inline-test.wesl").with("unchanged", ParamValue::F32(7.0));
        let merged = publication_look(&look, &entry, 0.8).unwrap();
        let (start, end, _) = snap_band(12.0, 0.8);
        assert_eq!(
            merged.values().get("morph_start"),
            Some(&ParamValue::F32(start))
        );
        assert_eq!(
            merged.values().get("morph_end"),
            Some(&ParamValue::F32(end))
        );
        assert_eq!(
            merged.values().get("stitch_edges"),
            Some(&ParamValue::Vec4(entry.stitch_edges))
        );
        assert_eq!(
            merged.values().get("unchanged"),
            Some(&ParamValue::F32(7.0))
        );
        assert!(publication_look(&merged, &entry, 0.8).is_none());
    }

    #[test]
    fn latest_readiness_counts_unqueued_followup_and_current_generation() {
        let (_, camera, _, _, mut view) = ready_view_fixture();
        assert!(view.latest_ready());
        let old = view.requested_signature.unwrap();
        view.requested_signature = Some(old.wrapping_add(1));
        let old_cover = PreparedTerrainCover {
            cover: view.cover.clone(),
            selected: view.cover_selected.clone(),
            camera_patches: CameraPatchRetention::default(),
            node_geometry: HashMap::new(),
            budget_refused: 0,
            preparation_refused_nodes: HashSet::new(),
            preparation_signature: None,
            settled_sig: Some(old),
            more_work: false,
        };
        commit_prepared_terrain_cover(
            &mut view,
            &mut TerrainNodeGeometry::default(),
            old_cover,
            3,
            old,
            9,
        );
        let mut status = TerrainStreamStatus::default();
        status.publish_selection_pending(usize::from(!view.latest_ready()));
        assert_eq!(
            status.pending, 1,
            "a fully resident older subset cannot hide a required follow-up before queue admission"
        );
        assert!(view.pending_cover.is_none());
        let mut tiles = LodTiles::default();
        tiles.views.insert(camera, view);
        let report = tiles.view_status(camera).unwrap();
        assert!(report.presented_ready && !report.ready && !report.selection_current);
        let view = tiles.views.get_mut(&camera).unwrap();
        view.last_sig = view.requested_signature;
        view.settled_sig = view.requested_signature;
        assert!(view.latest_ready());
        status.publish_selection_pending(usize::from(!view.latest_ready()));
        assert_eq!(
            status.pending, 0,
            "a stable completed fixed point releases readiness"
        );
        view.tiles.get_mut(&QuadCoord::ROOT).unwrap().r#gen -= 1;
        assert!(
            !view.latest_ready(),
            "retained stale-generation geometry remains presented, not current"
        );
        assert!(!view.focus_tile([0.0, 0.0]).unwrap().1);
        status.publish_selection_pending(1);
        status.publish_selection_pending(0);
        assert_eq!(
            status.pending, 0,
            "removed output demand withdraws its selection contribution"
        );
    }

    #[test]
    fn finalized_demand_publication_invalidates_readiness_without_a_worker() {
        let (mut demands, _, terrain, profile, mut view) = ready_view_fixture();
        assert!(view.latest_ready());
        let old = view.requested_signature;
        demands.visual[0]
            .terrain_poses
            .get_mut(&terrain)
            .unwrap()
            .0
            .x = 2.0;
        let demand = &mut demands.visual[0];
        let (eye, forward) = demand.terrain_poses[&terrain];
        demand.terrain_clips.insert(
            terrain,
            demand.projection.clip_from_local() * test_view(eye, forward, DVec3::Y),
        );
        view.publish_latest_demand(
            &demands,
            terrain,
            &demands.visual[0],
            3,
            Some(&profile),
            false,
        );
        assert_ne!(view.requested_signature, old);
        assert!(
            !view.latest_ready(),
            "PostUpdate's finalized pose must invalidate the already completed Update cover"
        );
        assert!(view.pending_cover.is_none());
        view.last_sig = view.requested_signature;
        view.settled_sig = view.requested_signature;
        assert!(view.latest_ready());
        let stable = view.requested_signature;
        view.publish_latest_demand(
            &demands,
            terrain,
            &demands.visual[0],
            3,
            Some(&profile),
            false,
        );
        assert_eq!(view.requested_signature, stable);
        assert!(
            view.latest_ready(),
            "unchanged finalized demands cannot create permanent pending work"
        );
        view.publish_latest_demand(
            &demands,
            terrain,
            &demands.visual[0],
            4,
            Some(&profile),
            false,
        );
        assert!(
            !view.latest_ready(),
            "a new source key cannot inherit old-cover readiness"
        );
    }

    #[test]
    fn mounted_pose_jitter_keeps_readiness_but_telephoto_pan_and_zoom_invalidate() {
        let (mut demands, _, terrain, profile, mut view) = ready_view_fixture();
        demands.visual[0].projection = test_perspective(0.01, DVec3::ZERO, DVec3::NEG_Z);
        let publish = |view: &mut LodView,
                       demands: &mut TerrainDetailDemands,
                       eye: DVec3,
                       rotation: DQuat| {
            let forward = rotation * DVec3::NEG_Z;
            let demand = &mut demands.visual[0];
            demand.terrain_poses.insert(terrain, (eye, forward));
            demand.terrain_clips.insert(
                terrain,
                demand.projection.clip_from_local()
                    * DMat4::from_rotation_translation(rotation, eye).inverse(),
            );
            view.publish_latest_demand(
                demands,
                terrain,
                &demands.visual[0],
                3,
                Some(&profile),
                false,
            );
        };
        // Exactly half a metre-lattice step in the production site's negative
        // Y frame exercises the snap boundary rather than a convenient bin centre.
        let eye = DVec3::new(-380.25, -1918.25, -380.25);
        publish(&mut view, &mut demands, eye, DQuat::IDENTITY);
        let stable = view.requested_signature;
        view.last_sig = stable;
        view.settled_sig = stable;
        assert!(view.latest_ready());
        let ulp = f64::from(f32::from_bits(1918.25f32.to_bits() + 4) - 1918.25f32);
        for index in 0..64 {
            let sign = if index % 2 == 0 { -1.0 } else { 1.0 };
            publish(
                &mut view,
                &mut demands,
                eye + DVec3::splat(sign * ulp),
                DQuat::from_rotation_y(sign * 1e-8),
            );
            assert_eq!(
                view.requested_signature, stable,
                "native ULP jitter must stay within the retained envelope"
            );
            assert!(
                view.latest_ready(),
                "a resident complete mounted view cannot remain pending from subpixel jitter"
            );
            // Update and finalized publication consult the same retained anchor.
            let demand = &demands.visual[0];
            let (position, forward) = demand.terrain_poses[&terrain];
            let mut visual = terrain_visual_demand(
                demand,
                position,
                forward,
                demand.terrain_clips[&terrain],
                profile.terrain_lod_pixel_error,
            )
            .unwrap();
            view.retain_selection_clip(&mut visual);
            let (signature, admission) = terrain_view_signatures(
                &demands,
                terrain,
                demand,
                &[visual],
                4,
                3,
                &profile,
                false,
            );
            assert_eq!(
                Some(finish_terrain_view_signature(signature, admission, None)),
                stable
            );
        }
        publish(&mut view, &mut demands, eye, DQuat::from_rotation_y(0.001));
        assert_ne!(
            view.requested_signature, stable,
            "meaningful telephoto panning must refresh the footprint"
        );
        assert!(!view.latest_ready());
        let previous = view.requested_signature;
        demands.visual[0].projection = test_perspective(0.02, DVec3::ZERO, DVec3::NEG_Z);
        publish(&mut view, &mut demands, eye, DQuat::from_rotation_y(0.001));
        assert_ne!(
            view.requested_signature, previous,
            "optical zoom changes the native hard projection lease"
        );
    }

    #[test]
    fn retained_clip_encloses_full_quantum_boundary_and_exact_infinite_far_plane() {
        let eye = DVec3::new(-380.25, -1918.25, -380.25);
        let projection = test_perspective(0.01, eye, DVec3::NEG_Z);
        let anchor = SelectionClip::new(projection, 1080.0, 2.0, eye).unwrap();
        let native = SelectionClip::normalized_planes(projection, anchor.center);
        assert_eq!(native[4].truncate(), DVec3::ZERO);
        assert_eq!(
            anchor.planes[4], native[4],
            "the native constant inequality must remain exact"
        );
        let rotation = DQuat::from_rotation_y(anchor.normal_quantum * 0.9);
        let current = TerrainProjection::Perspective {
            fov_y_rad: 0.01,
            clip_from_local: bevy::math::dproj::perspective_infinite_reverse(0.01, 1.0, 0.1)
                * DMat4::from_rotation_translation(rotation, eye + DVec3::splat(0.1)).inverse(),
        };
        assert!(anchor.retains(
            current,
            SelectionClip::new(current, 1080.0, 2.0, eye + DVec3::splat(0.1)).unwrap()
        ));
        for x in [-1.0, 0.0, 1.0] {
            for y in [-1.0, 0.0, 1.0] {
                let homogeneous =
                    current.clip_from_local().inverse() * bevy::math::DVec4::new(x, y, 0.001, 1.0);
                let point = homogeneous.truncate() / homogeneous.w;
                assert!(
                    anchor.intersects(
                        Square {
                            center: [point.x, point.z],
                            half: 1e-8
                        },
                        [point.y - 1e-8, point.y + 1e-8]
                    ),
                    "the retained full-quantum guard must include every current clip boundary, including negative source Y"
                );
            }
        }
        let tiny = test_perspective(1.0, DVec3::ZERO, DVec3::NEG_Z);
        let low_output = SelectionClip::new(tiny, 1.0, 32.0, DVec3::ZERO).unwrap();
        assert!(
            SelectionClip::new(tiny, 1.0, 32.0, DVec3::NAN).is_none(),
            "the authoritative camera-eye input must be finite"
        );
        for (native, quantized) in SelectionClip::normalized_planes(tiny, low_output.center)
            .into_iter()
            .zip(low_output.planes)
        {
            assert_eq!(
                native.truncate() == DVec3::ZERO,
                quantized.truncate() == DVec3::ZERO,
                "normal quantization cannot fabricate a constant inequality for a one-pixel output"
            );
        }
        assert!(low_output.intersects(
            Square {
                center: [0.0, -1.0],
                half: 0.1
            },
            [-0.1, 0.1]
        ));
    }

    #[test]
    fn orthographic_clip_retains_subpixel_jitter_and_detects_roll() {
        let native = bevy::math::dproj::orthographic(-10.0, 10.0, -10.0, 10.0, 0.1, 10_000.0);
        let eye = DVec3::new(-380.25, -1918.25, -380.25);
        let pose = test_view(eye, DVec3::NEG_Y, DVec3::NEG_Z).inverse();
        let projection = |roll, offset| TerrainProjection::Orthographic {
            pixels_per_metre: 50.0,
            area: [-10.0, -10.0, 10.0, 10.0],
            output_pixels: [1000, 1000],
            clip_from_local: native
                * (DMat4::from_translation(DVec3::splat(offset))
                    * pose
                    * DMat4::from_quat(DQuat::from_rotation_z(roll)))
                .inverse(),
        };
        let initial = projection(0.0, 0.0);
        let anchor = SelectionClip::new(initial, 1000.0, 2.0, eye).unwrap();
        let distant_far = TerrainProjection::Orthographic {
            pixels_per_metre: 50.0,
            area: [-10.0, -10.0, 10.0, 10.0],
            output_pixels: [1000, 1000],
            clip_from_local: bevy::math::dproj::orthographic(
                -10.0,
                10.0,
                -10.0,
                10.0,
                0.1,
                1_000_000.0,
            ) * pose.inverse(),
        };
        assert_eq!(
            SelectionClip::new(distant_far, 1000.0, 2.0, eye)
                .unwrap()
                .center,
            anchor.center,
            "projection far range cannot translate the camera-centred angular guard"
        );
        let distant_eye = eye + DVec3::Y * 1_000_000.0;
        let distant = TerrainProjection::Orthographic {
            pixels_per_metre: 50.0,
            area: [-10.0, -10.0, 10.0, 10.0],
            output_pixels: [1000, 1000],
            clip_from_local: bevy::math::dproj::orthographic(
                -10.0,
                10.0,
                -10.0,
                10.0,
                0.1,
                2_000_000.0,
            ) * test_view(distant_eye, DVec3::NEG_Y, DVec3::NEG_Z),
        };
        let distant_anchor = SelectionClip::new(distant, 1000.0, 2.0, distant_eye).unwrap();
        assert_eq!(
            distant_anchor.center, anchor.center,
            "axial distance cannot inflate an orthographic selection envelope"
        );
        for offset in [0.0, 30.0] {
            let region = Square {
                center: [eye.x + offset, eye.z],
                half: 0.01,
            };
            assert_eq!(
                anchor.intersects(region, [-1920.0, -1919.0]),
                distant_anchor.intersects(region, [-1920.0, -1919.0])
            );
        }
        let jitter = projection(1e-8, 0.0005);
        assert!(anchor.retains(
            jitter,
            SelectionClip::new(jitter, 1000.0, 2.0, eye + DVec3::splat(0.0005)).unwrap()
        ));
        let rolled = projection(0.01, 0.0);
        assert!(
            !anchor.retains(
                rolled,
                SelectionClip::new(rolled, 1000.0, 2.0, eye).unwrap()
            ),
            "orthographic roll changes the visible footprint even when its forward direction is unchanged"
        );
        let mut first = lunco_precompute::Fnv1a::new();
        anchor.write_signature(&mut first);
        let mut second = lunco_precompute::Fnv1a::new();
        SelectionClip::new(rolled, 1000.0, 2.0, eye)
            .unwrap()
            .write_signature(&mut second);
        assert_ne!(first.finish(), second.finish());
    }

    #[test]
    fn frozen_root_is_explicitly_settled_and_pose_independent() {
        let (mut demands, _, terrain, profile, mut view) = ready_view_fixture();
        view.publish_latest_demand(
            &demands,
            terrain,
            &demands.visual[0],
            3,
            Some(&profile),
            true,
        );
        let signature = view.requested_signature.unwrap();
        let selected = view.cover_selected.clone();
        view.commit_frozen_cover(&selected, signature, 9);
        assert!(view.latest_ready());
        demands.visual[0]
            .terrain_poses
            .insert(terrain, (DVec3::new(12.0, 10.0, -4.0), DVec3::X));
        view.publish_latest_demand(
            &demands,
            terrain,
            &demands.visual[0],
            3,
            Some(&profile),
            true,
        );
        assert_eq!(view.requested_signature, Some(signature));
        assert!(
            view.latest_ready(),
            "a root-wide fixed cinematic mesh does not require reselection when the capture camera moves"
        );
        view.tiles.get_mut(&QuadCoord::ROOT).unwrap().r#gen -= 1;
        assert!(!view.latest_ready());
    }

    #[test]
    fn held_preparation_coalesces_jitter_and_retires_superseded_capacity() {
        let quadtree = Quadtree::new(64.0, 6, 1.0, 64.0);
        let corridor = TerrainPreparationCorridor {
            start: DVec3::new(0.3, 2.0, 0.3),
            end: DVec3::new(8.3, 2.0, 0.3),
            radius_m: 0.1,
            source_pose: crate::surface_query::TerrainPoseInPhysicsFrame {
                frame: Entity::PLACEHOLDER,
                position: DVec3::ZERO,
                rotation: DQuat::IDENTITY,
            },
            source_key: 1,
        };
        let mut held = HeldTerrainPreparation::default();
        held.admit(9, 1, corridor, &quadtree, 128);
        let first = held.requirements_signature;
        let first_nodes = held.nodes.clone();
        held.admit(
            9,
            2,
            TerrainPreparationCorridor {
                end: corridor.end + DVec3::new(0.01, 0.001, 0.0),
                ..corridor
            },
            &quadtree,
            128,
        );
        assert_eq!(
            held.requirements_signature, first,
            "dt changes that need the same fine nodes do not replace queued work"
        );
        let farther = TerrainPreparationCorridor {
            end: DVec3::new(18.3, 2.0, 0.3),
            ..corridor
        };
        held.admit(9, 3, farther, &quadtree, 128);
        assert_ne!(held.requirements_signature, first);
        assert!(
            first_nodes.is_subset(&held.nodes),
            "an in-flight subset remains useful as new nodes are requested"
        );
        let token = TerrainCoverToken {
            terrain: Entity::PLACEHOLDER,
            camera: Entity::PLACEHOLDER,
            generation: 2,
            signature: first,
            admission_signature: 9,
            oracle_key: 4,
            operation: 1,
        };
        assert!(
            token.matches_current_inputs(2, 4, 9),
            "soft endpoint and heading changes retain source-valid queued work"
        );
        assert!(!token.matches_current_inputs(2, 4, 10));
        let current_signature = held.requirements_signature;
        let mut selected = vec![quadtree.selected(QuadCoord::ROOT, f64::INFINITY)];
        quadtree.constrain_selection(
            &mut selected,
            &first_nodes.iter().copied().collect::<Vec<_>>(),
            &|_, _| 0.0,
        );
        let mut view = LodView {
            held_preparation: Some(held),
            ..default()
        };
        commit_prepared_terrain_cover(
            &mut view,
            &mut TerrainNodeGeometry::default(),
            PreparedTerrainCover {
                cover: HashSet::from([QuadCoord::ROOT]),
                selected,
                camera_patches: CameraPatchRetention::default(),
                node_geometry: HashMap::new(),
                budget_refused: 0,
                preparation_refused_nodes: HashSet::new(),
                preparation_signature: Some(first),
                settled_sig: Some(first),
                more_work: false,
            },
            4,
            first,
            9,
        );
        assert_eq!(
            view.held_preparation
                .as_ref()
                .unwrap()
                .requirements_signature,
            current_signature,
            "publishing a useful earlier subset cannot erase the latest accumulated requirement"
        );
        assert_ne!(
            view.preparation_signature,
            Some(current_signature),
            "latest requirements still need their own follow-up publication"
        );
        let changed = TerrainPreparationCorridor {
            end: DVec3::new(0.3, 2.0, 8.3),
            ..corridor
        };
        let a = preparation_corridor_nodes(&quadtree, corridor, 128).unwrap();
        let b = preparation_corridor_nodes(&quadtree, changed, 128).unwrap();
        let capacity = a.len().max(b.len());
        held = HeldTerrainPreparation::default();
        held.admit(9, 4, corridor, &quadtree, capacity);
        held.admit(9, 5, changed, &quadtree, capacity);
        assert!(
            !held.capacity_refused,
            "historical requirements cannot poison a feasible current corridor"
        );
        assert_eq!(held.nodes, held.latest_nodes);
        held.admit(10, 6, corridor, &quadtree, 128);
        assert_eq!(
            held.nodes,
            a.into_iter().collect(),
            "hard lease changes retire accumulated intent"
        );
        held.admit(10, 7, farther, &quadtree, 1);
        assert!(
            held.capacity_refused,
            "an actually oversized current request stays explicitly refused"
        );
        held.admit(10, 8, corridor, &quadtree, 128);
        assert!(
            !held.capacity_refused,
            "a later feasible request can progress after genuine capacity refusal"
        );
    }

    #[test]
    fn current_corridor_precedes_historical_balanced_cover_pressure() {
        let quadtree = Quadtree::new(64.0, 6, 1.0, 64.0);
        let latest = QuadCoord {
            depth: 6,
            x: 62,
            z: 62,
        };
        let history = QuadCoord {
            depth: 6,
            x: 1,
            z: 1,
        };
        let mut current_only = vec![quadtree.selected(QuadCoord::ROOT, f64::INFINITY)];
        quadtree.constrain_selection(&mut current_only, &[latest], &|_, _| 0.0);
        let oracle = Arc::new(SurfaceOracle::bare(Arc::new(
            lunco_obstacle_field::field::HeightGrid::new_flat(3, 64.0),
        )));
        let prepared = prepare_terrain_cover(TerrainCoverRequest {
            quadtree,
            oracle: oracle.clone(),
            oracle_key: oracle.surface_key(),
            demands: vec![TerrainVisualDemand {
                camera: Entity::PLACEHOLDER,
                focus: [0.0, 0.0],
                camera_y: 1000.0,
                heading: None,
                view_direction: DVec3::Y,
                screen_height_px: 720.0,
                selection_clip: SelectionClip::new(
                    test_perspective(1.0, DVec3::Y * 1000.0, DVec3::Y),
                    720.0,
                    2.0,
                    DVec3::Y * 1000.0,
                )
                .unwrap(),
                projection: test_perspective(1.0, DVec3::Y * 1000.0, DVec3::Y),
                required: true,
                near_detail_radius_m: 0.0,
                near_detail_hysteresis_m: 0.0,
            }],
            camera_patches: CameraPatchRetention::default(),
            cover: HashSet::new(),
            node_geometry: HashMap::new(),
            bootstrap_ready: true,
            settled_sig: None,
            budget_refused: 0,
            signature: 7,
            preparation_signature: Some(8),
            preparation: Some(TerrainPreparationRequirements {
                nodes: HashSet::from([history, latest]),
                latest_nodes: HashSet::from([latest]),
            }),
            tile_resolution: 49,
            probe_resolution: 5,
            tile_budget: current_only.len(),
            max_depth: 6,
            cover_edits_per_cycle: 8,
            hysteresis_ratio: 0.8,
        });
        assert!(
            prepared.preparation_refused_nodes.is_empty(),
            "historical intent cannot steal the balanced-cover budget from a feasible current path"
        );
        assert!(
            prepared
                .selected
                .iter()
                .any(|selected| selected.coord == latest && selected.morph_end.is_infinite())
        );
    }

    #[test]
    fn continuous_preparation_corridor_covers_fine_cells_and_refuses_capacity() {
        let quadtree = Quadtree::new(500.0, 9, 1.0, 500.0);
        let corridor = TerrainPreparationCorridor {
            start: DVec3::new(-25.0, 20.0, -1.0),
            end: DVec3::new(25.0, 20.0, 1.0),
            radius_m: 0.7,
            source_pose: crate::surface_query::TerrainPoseInPhysicsFrame {
                frame: Entity::PLACEHOLDER,
                position: DVec3::ZERO,
                rotation: DQuat::IDENTITY,
            },
            source_key: 1,
        };
        let nodes = preparation_corridor_nodes(&quadtree, corridor, 512).unwrap();
        let selected: HashSet<_> = nodes.iter().copied().collect();
        for index in 0..=1000 {
            let center = corridor.start.lerp(corridor.end, index as f64 / 1000.0);
            for angle in 0..16 {
                let angle = angle as f64 * std::f64::consts::TAU / 16.0;
                let point = [
                    center.x + corridor.radius_m * angle.cos(),
                    center.z + corridor.radius_m * angle.sin(),
                ];
                assert!(
                    selected.contains(&quadtree.node_containing(quadtree.max_depth, point)),
                    "no unsampled corridor gap may retain a coarser morphing tile"
                );
            }
        }
        assert_eq!(
            nodes,
            preparation_corridor_nodes(
                &quadtree,
                TerrainPreparationCorridor {
                    start: corridor.end,
                    end: corridor.start,
                    ..corridor
                },
                512
            )
            .unwrap(),
            "ordered required nodes are independent of travel direction"
        );
        assert!(
            preparation_corridor_nodes(&quadtree, corridor, 4).is_err(),
            "capacity cannot truncate a long requested path"
        );
        assert!(
            preparation_corridor_nodes(
                &quadtree,
                TerrainPreparationCorridor {
                    radius_m: f64::NAN,
                    ..corridor
                },
                512
            )
            .is_err()
        );
    }

    #[test]
    fn preparation_intent_survives_render_frames_and_retires_exact_leases() {
        let mut app = App::new();
        app.init_resource::<TerrainDetailDemands>()
            .init_resource::<TerrainCameraTargets>()
            .add_systems(Update, collect_terrain_detail_demands);
        let frame = app.world_mut().spawn(Grid::new(1000.0, 100.0)).id();
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(frame));
        let terrain = app
            .world_mut()
            .spawn((
                DemTerrainSurface,
                DemHeightField(Arc::new(SurfaceOracle::bare(Arc::new(
                    lunco_obstacle_field::field::HeightGrid::new_flat(3, 100.0),
                )))),
                crate::surface_query::TerrainPoseInPhysicsFrame {
                    frame,
                    position: DVec3::ZERO,
                    rotation: DQuat::IDENTITY,
                },
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(frame),
            ))
            .id();
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Camera {
                    viewport: Some(bevy::camera::Viewport {
                        physical_size: UVec2::splat(720),
                        ..default()
                    }),
                    ..default()
                },
                Projection::default(),
                SceneViewDemand {
                    source: frame,
                    output: lunco_render::camera::SceneViewOutput::MainViewport,
                },
                SceneViewLayers {
                    layer: 64,
                    base: RenderLayers::default(),
                },
                CellCoord::ZERO,
                Transform::from_xyz(0.0, 2.0, 0.0),
                ChildOf(frame),
            ))
            .id();
        app.world_mut().entity_mut(camera).insert(SceneViewDemand {
            source: camera,
            output: lunco_render::camera::SceneViewOutput::MainViewport,
        });
        let snapshot = {
            let mut state =
                bevy::ecs::system::SystemState::<crate::surface_query::GridSurfaceQuery>::new(
                    app.world_mut(),
                );
            state.get(app.world()).unwrap().snapshot().unwrap()
        };
        let request = |app: &mut App| {
            app.world_mut()
                .resource_mut::<TerrainCameraTargets>()
                .request(
                    camera,
                    snapshot.clone(),
                    lunco_spatial::coords::GridPos(DVec3::new(0.0, 2.0, 0.0)),
                    lunco_spatial::coords::GridPos(DVec3::new(30.0, 2.0, 0.0)),
                    0.4,
                )
                .unwrap()
        };
        request(&mut app);
        let mut preparation_signature = None;
        for index in 0..4 {
            app.world_mut()
                .get_mut::<Transform>(camera)
                .unwrap()
                .rotation = Quat::from_rotation_y(index as f32 * 0.3);
            app.update();
            assert!(
                app.world()
                    .resource::<TerrainDetailDemands>()
                    .has_preparation(camera, terrain)
            );
            assert!(
                app.world()
                    .resource::<TerrainCameraTargets>()
                    .targets
                    .contains_key(&camera)
            );
            let signature = app
                .world()
                .resource::<TerrainDetailDemands>()
                .preparation_signature(camera, terrain);
            if index > 0 {
                assert_eq!(
                    signature, preparation_signature,
                    "stationary orientation damping cannot retire identical corridor preparation"
                );
            }
            preparation_signature = signature;
        }
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation
            .x = 1.0;
        app.update();
        assert!(
            !app.world()
                .resource::<TerrainCameraTargets>()
                .targets
                .contains_key(&camera),
            "moving the eye retires its held corridor lease"
        );
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation
            .x = 0.0;
        request(&mut app);
        app.update();
        app.world_mut().get_mut::<Camera>(camera).unwrap().is_active = false;
        app.update();
        assert!(
            !app.world()
                .resource::<TerrainCameraTargets>()
                .targets
                .contains_key(&camera)
        );
        app.world_mut().get_mut::<Camera>(camera).unwrap().is_active = true;
        request(&mut app);
        app.update();
        app.world_mut()
            .get_mut::<SceneViewDemand>(camera)
            .unwrap()
            .output = lunco_render::camera::SceneViewOutput::Observation { id: 8 };
        app.update();
        assert!(
            !app.world()
                .resource::<TerrainCameraTargets>()
                .targets
                .contains_key(&camera),
            "a reused camera entity cannot retain a retired output lease"
        );
        request(&mut app);
        app.update();
        app.world_mut()
            .get_mut::<Transform>(terrain)
            .unwrap()
            .translation
            .x = 1.0;
        app.update();
        assert!(
            !app.world()
                .resource::<TerrainCameraTargets>()
                .targets
                .contains_key(&camera),
            "fresh source pose changes retire the old snapshot before cached PreUpdate poses change"
        );
        app.world_mut()
            .get_mut::<Transform>(terrain)
            .unwrap()
            .translation
            .x = 0.0;
        request(&mut app);
        app.update();
        app.world_mut()
            .entity_mut(terrain)
            .insert(DemHeightField(Arc::new(SurfaceOracle::bare(Arc::new(
                lunco_obstacle_field::field::HeightGrid::new_flat(3, 200.0),
            )))));
        app.update();
        assert!(
            !app.world()
                .resource::<TerrainCameraTargets>()
                .targets
                .contains_key(&camera),
            "the same entity cannot retain an old oracle content lease"
        );
        let result = app
            .world_mut()
            .resource_mut::<TerrainCameraTargets>()
            .request(
                camera,
                snapshot,
                lunco_spatial::coords::GridPos(DVec3::ZERO),
                lunco_spatial::coords::GridPos(DVec3::NAN),
                0.4,
            );
        assert!(result.is_err());
        assert!(
            !app.world()
                .resource::<TerrainCameraTargets>()
                .targets
                .contains_key(&camera)
        );
    }

    #[test]
    fn orthographic_footprint_clips_nodes_and_keeps_uncertainty_eligible() {
        let clip = bevy::math::dproj::orthographic(-10.0, 10.0, -10.0, 10.0, 0.1, 10_000.0)
            * Transform::from_translation(Vec3::new(0.0, 1000.0, 0.0))
                .looking_at(Vec3::ZERO, Vec3::NEG_Z)
                .to_matrix()
                .as_dmat4()
                .inverse();
        let projection = TerrainProjection::Orthographic {
            pixels_per_metre: 50.0,
            area: [-10.0, -10.0, 10.0, 10.0],
            output_pixels: [1000, 1000],
            clip_from_local: clip,
        };
        assert!(
            SelectionClip::new(projection, 1000.0, 2.0, DVec3::Y * 1000.0)
                .unwrap()
                .intersects(
                    Square {
                        center: [0.0, 0.0],
                        half: 2.0
                    },
                    [-4.0, 4.0]
                )
        );
        assert!(
            !SelectionClip::new(projection, 1000.0, 2.0, DVec3::Y * 1000.0)
                .unwrap()
                .intersects(
                    Square {
                        center: [30.0, 0.0],
                        half: 2.0
                    },
                    [-4.0, 4.0]
                )
        );
        assert!(
            SelectionClip::new(projection, 1000.0, 2.0, DVec3::Y * 1000.0)
                .unwrap()
                .intersects(
                    Square {
                        center: [11.0, 0.0],
                        half: 2.0
                    },
                    [-4.0, 4.0]
                )
        );
        assert!(
            SelectionClip::new(projection, 1000.0, 2.0, DVec3::Y * 1000.0)
                .unwrap()
                .intersects(
                    Square {
                        center: [30.0, 0.0],
                        half: 2.0
                    },
                    [f64::NAN; 2]
                ),
            "without a certified height envelope an off-axis node remains eligible"
        );
        let demand = TerrainVisualDemand {
            camera: Entity::PLACEHOLDER,
            focus: [0.0, 0.0],
            camera_y: 1000.0,
            heading: None,
            view_direction: DVec3::NEG_Y,
            screen_height_px: 1000.0,
            projection,
            selection_clip: SelectionClip::new(projection, 1000.0, 2.0, DVec3::Y * 1000.0).unwrap(),
            required: true,
            near_detail_radius_m: 30.0,
            near_detail_hysteresis_m: 12.0,
        };
        assert!(demand.camera_y > demand.near_detail_radius_m);
        let floor = projected_posting_error_floor(
            Square {
                center: [0.0, 0.0],
                half: 20.0,
            },
            5,
        );
        assert_eq!(
            floor, 10.0,
            "visible posting density survives coarse height-probe aliasing at any altitude"
        );
        let quadtree = Quadtree::from_orthographic_metric(20.0, 5, 20.0, 50.0, 2.0);
        assert!(
            quadtree.error_refine_range(floor)
                > quadtree.focus_distance(QuadCoord::ROOT, [0.0, 1000.0, 0.0], [0.0, 0.0])
        );
        let oracle = SurfaceOracle::bare(Arc::new(
            lunco_obstacle_field::field::HeightGrid::new_flat(3, 20.0),
        ));
        let bounds = projected_node_height_bounds(
            &oracle,
            Square {
                center: [0.0, 0.0],
                half: 20.0,
            },
        )
        .expect("flat enclosure");
        assert!(bounds[0] <= 0.0 && bounds[1] >= 0.0 && bounds[1] - bounds[0] < 1e-12);
        let large = SurfaceOracle::bare(Arc::new(
            lunco_obstacle_field::field::HeightGrid::new_flat(33, 20.0),
        ));
        assert_eq!(
            projected_node_height_bounds(
                &large,
                Square {
                    center: [0.0, 0.0],
                    half: 20.0
                }
            ),
            None,
            "bounded enclosure work cannot silently certify exclusion of a large node"
        );
        let mut previous = lunco_precompute::Fnv1a::new();
        projection.write_signature(&mut previous);
        let mut zoomed = lunco_precompute::Fnv1a::new();
        TerrainProjection::Orthographic {
            pixels_per_metre: 100.0,
            area: [-5.0, -5.0, 5.0, 5.0],
            output_pixels: [1000, 1000],
            clip_from_local: clip,
        }
        .write_signature(&mut zoomed);
        assert_ne!(previous.finish(), zoomed.finish());
    }

    #[test]
    fn distant_perspective_zoom_discovers_probe_filtered_crater_with_bounded_cover() {
        let half = 128.0;
        let oracle = Arc::new(SurfaceOracle::new(
            Arc::new(lunco_obstacle_field::field::HeightGrid::new_flat(3, half)),
            vec![crate::oracle::HeightContribution {
                content_key: 1,
                modifier: Arc::new(lunco_terrain_core::Craters::new(vec![
                    lunco_terrain_core::Crater {
                        center: [0.0, 0.0],
                        radius: 4.0,
                        depth: 1.0,
                        rim_height: 0.2,
                        softness: 0.0,
                        bowl_power: 4.0,
                    },
                ])),
            }],
        ));
        let root = Square {
            center: [0.0, 0.0],
            half: f64::from(half),
        };
        let probe_step = root.side() / 4.0;
        let gated = measure_node_geometry(&oracle.detail_limited(probe_step), root, 5);
        assert_eq!(
            gated.error, 0.0,
            "the 8m crater is completely absent from 64m-gated coarse probes"
        );
        assert!(oracle.height_at(0.0, 0.0) < -0.5);
        let tile_budget = 512;
        let select = |fov_y_rad| {
            let quadtree = Quadtree::from_screen_metric(
                f64::from(half),
                8,
                f64::from(half),
                1080.0,
                fov_y_rad,
                3.0,
                0.55,
            );
            let demand = TerrainVisualDemand {
                camera: Entity::PLACEHOLDER,
                focus: [0.0, 0.0],
                camera_y: 1000.0,
                heading: None,
                view_direction: DVec3::NEG_Y,
                screen_height_px: 1080.0,
                selection_clip: SelectionClip::new(
                    test_perspective(fov_y_rad, DVec3::Y * 1000.0, DVec3::NEG_Y),
                    1080.0,
                    2.0,
                    DVec3::Y * 1000.0,
                )
                .unwrap(),
                projection: test_perspective(fov_y_rad, DVec3::Y * 1000.0, DVec3::NEG_Y),
                required: true,
                near_detail_radius_m: 30.0,
                near_detail_hysteresis_m: 12.0,
            };
            assert!(demand.camera_y > demand.near_detail_radius_m);
            let mut request = TerrainCoverRequest {
                quadtree,
                oracle: oracle.clone(),
                oracle_key: oracle.surface_key(),
                demands: vec![demand],
                camera_patches: CameraPatchRetention::default(),
                cover: HashSet::new(),
                node_geometry: HashMap::new(),
                bootstrap_ready: true,
                settled_sig: None,
                budget_refused: 0,
                signature: fov_y_rad.to_bits(),
                preparation_signature: None,
                preparation: None,
                tile_resolution: 49,
                probe_resolution: 5,
                tile_budget,
                max_depth: 8,
                cover_edits_per_cycle: 64,
                hysteresis_ratio: 1.3,
            };
            for _ in 0..64 {
                let result = prepare_terrain_cover(request);
                assert!(result.cover.len() <= tile_budget);
                if result.settled_sig == Some(fov_y_rad.to_bits()) {
                    return result;
                }
                request = TerrainCoverRequest {
                    quadtree,
                    oracle: oracle.clone(),
                    oracle_key: oracle.surface_key(),
                    demands: vec![demand],
                    camera_patches: result.camera_patches,
                    cover: result.cover,
                    node_geometry: result.node_geometry,
                    bootstrap_ready: true,
                    settled_sig: result.settled_sig,
                    budget_refused: result.budget_refused,
                    signature: fov_y_rad.to_bits(),
                    preparation_signature: None,
                    preparation: None,
                    tile_resolution: 49,
                    probe_resolution: 5,
                    tile_budget,
                    max_depth: 8,
                    cover_edits_per_cycle: 64,
                    hysteresis_ratio: 1.3,
                };
            }
            panic!("bounded cover preparation must reach a real fixed point");
        };
        let wide = select(std::f64::consts::FRAC_PI_3);
        let zoomed = select(0.01);
        let visible_depth = |cover: &HashSet<QuadCoord>| {
            cover
                .iter()
                .filter(|coord| {
                    let cells = f64::from(1u32 << coord.depth);
                    let side = root.side() / cells;
                    let square = Square {
                        center: [
                            -f64::from(half) + (f64::from(coord.x) + 0.5) * side,
                            -f64::from(half) + (f64::from(coord.z) + 0.5) * side,
                        ],
                        half: side * 0.5,
                    };
                    square.distance_to([0.0, 0.0]) == 0.0
                })
                .map(|coord| coord.depth)
                .min()
                .unwrap()
        };
        assert!(
            visible_depth(&zoomed.cover) > visible_depth(&wide.cover),
            "changing only native FOV must refine distant visible small relief"
        );
        assert_eq!(
            zoomed.budget_refused, 0,
            "the clipped telephoto footprint must fit the configured cover"
        );
        assert!(
            zoomed.camera_patches.patches.is_empty(),
            "telephoto discovery must not depend on near-field pins"
        );
        let off_axis = Square {
            center: [30.0, 0.0],
            half: 2.0,
        };
        let zoom = test_perspective(0.01, DVec3::Y * 1000.0, DVec3::NEG_Y);
        assert!(
            !SelectionClip::new(zoom, 1080.0, 3.0, DVec3::Y * 1000.0)
                .unwrap()
                .intersects(off_axis, oracle.global_height_bounds().unwrap())
        );
        assert!(
            SelectionClip::new(zoom, 1080.0, 3.0, DVec3::Y * 1000.0)
                .unwrap()
                .intersects(off_axis, [f64::NAN; 2]),
            "uncertain heights cannot certify exclusion"
        );
        let overlook = test_perspective(0.01, DVec3::Y * 1000.0, DVec3::NEG_Z);
        let mountain = Square {
            center: [0.0, -1000.0],
            half: 2.0,
        };
        assert!(
            !SelectionClip::new(overlook, 1080.0, 3.0, DVec3::Y * 1000.0)
                .unwrap()
                .intersects(mountain, [0.0, 0.0])
        );
        assert!(
            SelectionClip::new(overlook, 1080.0, 3.0, DVec3::Y * 1000.0)
                .unwrap()
                .intersects(mountain, [0.0, 1000.0]),
            "a possible tall feature excluded by flat probes must remain eligible under full-source bounds"
        );
    }

    #[test]
    fn preparation_refusal_requires_settled_exact_current_request() {
        let mut world = World::new();
        let camera = world.spawn_empty().id();
        let terrain = world.spawn_empty().id();
        let mut demands = TerrainDetailDemands {
            visual: vec![VisualDemand {
                entity: camera,
                source: camera,
                render_layer: 64,
                terrain_poses: HashMap::from([(terrain, (DVec3::Y, DVec3::NEG_Z))]),
                terrain_clips: HashMap::new(),
                preparation_corridors: HashMap::from([(
                    terrain,
                    TerrainPreparationCorridor {
                        start: DVec3::Y,
                        end: DVec3::new(4.0, 2.0, 3.0),
                        radius_m: 0.5,
                        source_pose: crate::surface_query::TerrainPoseInPhysicsFrame {
                            frame: Entity::PLACEHOLDER,
                            position: DVec3::ZERO,
                            rotation: DQuat::IDENTITY,
                        },
                        source_key: 1,
                    },
                )]),
                position: DVec3::Y,
                forward: DVec3::NEG_Z,
                screen_height_px: 720.0,
                projection: test_perspective(1.0, DVec3::ZERO, DVec3::NEG_Z),
                near_detail_radius_m: 30.0,
                near_detail_hysteresis_m: 12.0,
            }],
        };
        let clip = demands.visual[0].projection.clip_from_local()
            * test_view(DVec3::Y, DVec3::NEG_Z, DVec3::Y);
        demands.visual[0].terrain_clips.insert(terrain, clip);
        let mut tiles = LodTiles::default();
        let mut held = HeldTerrainPreparation::default();
        held.admit(
            9,
            demands.preparation_signature(camera, terrain).unwrap(),
            demands.visual[0].preparation_corridors[&terrain],
            &Quadtree::new(100.0, 5, 1.0, 100.0),
            64,
        );
        tiles.views.insert(
            camera,
            LodView {
                preparation_signature: Some(held.requirements_signature),
                preparation_refused_nodes: held.latest_nodes.clone(),
                held_preparation: Some(held),
                last_sig: Some(7),
                settled_sig: Some(7),
                requested_signature: Some(7),
                last_admission_sig: Some(9),
                ..default()
            },
        );
        assert!(demands.has_preparation(camera, terrain));
        assert!(tiles.preparation_refused(camera, &demands, terrain));
        let view = &tiles.views[&camera];
        let current_input = demands.preparation_signature(camera, terrain);
        assert!(view.preparation_refusal_count(current_input, 9) > 0);
        assert_eq!(
            view.preparation_refusal_count(current_input, 10),
            0,
            "a new profile/admission cannot inherit a refused requirement count"
        );
        let view = tiles.views.get_mut(&camera).unwrap();
        let signature = view.preparation_signature;
        view.preparation_signature = None;
        assert_eq!(
            view.preparation_refusal_count(current_input, 9),
            0,
            "uncommitted requirements must not publish old refused nodes"
        );
        view.preparation_signature = signature;
        demands.visual[0]
            .preparation_corridors
            .get_mut(&terrain)
            .unwrap()
            .end
            .x += 1.0;
        assert!(
            !tiles.preparation_refused(camera, &demands, terrain),
            "a retired request cannot reject a new navigation target"
        );
        demands.visual[0]
            .preparation_corridors
            .get_mut(&terrain)
            .unwrap()
            .end
            .x -= 1.0;
        tiles.views.get_mut(&camera).unwrap().pending_cover = Some((0, 7, 0, 0, 9));
        assert!(!tiles.preparation_refused(camera, &demands, terrain));
        let view = tiles.views.get_mut(&camera).unwrap();
        view.pending_cover = None;
        view.settled_sig = None;
        assert!(!tiles.preparation_refused(camera, &demands, terrain));
        let view = tiles.views.get_mut(&camera).unwrap();
        view.settled_sig = Some(7);
        view.preparation_refused_nodes.clear();
        view.budget_refused = 20;
        assert!(
            !tiles.preparation_refused(camera, &demands, terrain),
            "discretionary budget pressure is not a failed navigation patch"
        );
    }

    #[test]
    fn cover_completion_allows_camera_motion_but_fences_authoritative_inputs() {
        let token = TerrainCoverToken {
            camera: Entity::PLACEHOLDER,
            terrain: Entity::from_bits(1),
            generation: 3,
            signature: 4,
            admission_signature: 5,
            oracle_key: 6,
            operation: 7,
        };
        assert!(token.matches_current_inputs(3, 6, 5));
        assert!(!token.matches_current_inputs(2, 6, 5));
        assert!(!token.matches_current_inputs(3, 8, 5));
        assert!(!token.matches_current_inputs(3, 6, 9));
    }

    #[test]
    fn camera_cover_cadence_does_not_delay_admission_changes() {
        assert!(!terrain_cover_selection_due(false, Some(7), 7));
        assert!(terrain_cover_selection_due(true, Some(7), 7));
        assert!(terrain_cover_selection_due(false, Some(7), 8));
        assert!(terrain_cover_selection_due(false, None, 7));
    }

    fn terrain_quality() -> lunco_render::RenderQualityProfile {
        lunco_render::RenderQualityProfile {
            terrain_lod_pixel_error: 2.0,
            terrain_lod_morph_start_ratio: 0.55,
            terrain_lod_tile_resolution: 49,
            ..Default::default()
        }
    }

    fn production_template() -> ShaderLook {
        ShaderLook::new("shaders/terrain_layered.wesl")
            .with_vertex_shader("shaders/terrain_geomorph.wesl")
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn test_baked_tile(marker: f64) -> BakedTile {
        BakedTile {
            mesh: Mesh::new(
                bevy::mesh::PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::RENDER_WORLD,
            ),
            center: [marker, 0.0],
            depth: 1,
            res: 1,
            origin_y: 0.0,
            draw_bounds: None,
            bytes: 0,
        }
    }

    #[test]
    fn terrain_cover_cadence_is_wall_time_driven_and_drops_catch_up_work() {
        let mut cadence = TerrainStreamCadence::default();
        assert!(cadence.is_due());

        cadence.advance(Duration::ZERO);
        assert!(cadence.is_due());
        cadence.advance(Duration::from_millis(16));
        assert!(!cadence.is_due());
        cadence.advance(Duration::from_millis(18));
        assert!(cadence.is_due());

        cadence.advance(Duration::from_millis(100));
        assert!(cadence.is_due());
        cadence.advance(Duration::from_millis(16));
        assert!(!cadence.is_due());
    }

    #[test]
    fn terrain_cover_preparation_is_independent_of_hash_iteration_order() {
        let oracle = Arc::new(SurfaceOracle::bare(Arc::new(
            lunco_obstacle_field::field::HeightGrid::new_flat(3, 500.0),
        )));
        let make_request = |reverse: bool| {
            let mut cover = HashSet::new();
            let coords = if reverse {
                (0..4)
                    .rev()
                    .flat_map(|z| (0..4).rev().map(move |x| c(2, x, z)))
                    .collect::<Vec<_>>()
            } else {
                (0..4)
                    .flat_map(|z| (0..4).map(move |x| c(2, x, z)))
                    .collect::<Vec<_>>()
            };
            cover.extend(coords);
            TerrainCoverRequest {
                quadtree: Quadtree::from_screen_metric(
                    500.0,
                    5,
                    500.0,
                    720.0,
                    std::f64::consts::FRAC_PI_3,
                    2.0,
                    0.55,
                ),
                oracle: Arc::clone(&oracle),
                oracle_key: oracle.surface_key(),
                demands: vec![TerrainVisualDemand {
                    camera: Entity::PLACEHOLDER,
                    focus: [12.0, -8.0],
                    camera_y: 35.0,
                    heading: None,
                    view_direction: DVec3::Y,
                    screen_height_px: 720.0,
                    selection_clip: SelectionClip::new(
                        test_perspective(
                            std::f64::consts::FRAC_PI_3,
                            DVec3::new(12.0, 35.0, -8.0),
                            DVec3::Y,
                        ),
                        720.0,
                        2.0,
                        DVec3::new(12.0, 35.0, -8.0),
                    )
                    .unwrap(),
                    projection: test_perspective(
                        std::f64::consts::FRAC_PI_3,
                        DVec3::new(12.0, 35.0, -8.0),
                        DVec3::Y,
                    ),
                    required: true,
                    near_detail_radius_m: 30.0,
                    near_detail_hysteresis_m: 12.0,
                }],
                camera_patches: CameraPatchRetention::default(),
                cover,
                node_geometry: HashMap::new(),
                bootstrap_ready: true,
                settled_sig: None,
                budget_refused: 0,
                signature: 7,
                preparation_signature: None,
                preparation: None,
                tile_resolution: 49,
                probe_resolution: 5,
                tile_budget: 64,
                max_depth: 5,
                cover_edits_per_cycle: 8,
                hysteresis_ratio: 0.8,
            }
        };

        let forward = prepare_terrain_cover(make_request(false));
        let reverse = prepare_terrain_cover(make_request(true));

        assert_eq!(forward.cover, reverse.cover);
        assert_eq!(forward.selected, reverse.selected);
        assert_eq!(forward.node_geometry, reverse.node_geometry);
        assert_eq!(forward.budget_refused, reverse.budget_refused);
        assert_eq!(forward.settled_sig, reverse.settled_sig);
        assert_eq!(forward.more_work, reverse.more_work);
    }

    #[test]
    fn terrain_bake_order_is_total_when_camera_benefits_tie() {
        let qt = test_qt();
        let selected = |depth, x, z| {
            let coord = QuadCoord { depth, x, z };
            qt.selected(coord, 10.0)
        };
        let mut keyed = vec![
            (0, 0.5, selected(1, 1, 0)),
            (1, 0.1, selected(0, 0, 0)),
            (0, 0.5, selected(0, 0, 0)),
            (0, 0.5, selected(1, 0, 1)),
        ];

        keyed.sort_by(compare_terrain_bake_keys);

        assert_eq!(
            keyed.iter().map(|key| key.2.coord).collect::<Vec<_>>(),
            vec![
                QuadCoord {
                    depth: 0,
                    x: 0,
                    z: 0
                },
                QuadCoord {
                    depth: 1,
                    x: 0,
                    z: 1
                },
                QuadCoord {
                    depth: 1,
                    x: 1,
                    z: 0
                },
                QuadCoord {
                    depth: 0,
                    x: 0,
                    z: 0
                },
            ]
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn reversed_worker_completions_publish_in_coordinate_order_and_fence_stale_keys() {
        let mut pending = PendingTileBakes::default();
        let terrain = Entity::PLACEHOLDER;
        let tiles = [
            (c(1, 0, 0), 1, 10.0),
            (c(1, 0, 1), 2, 20.0),
            (c(1, 1, 0), 3, 30.0),
            (c(1, 1, 1), 4, 40.0),
        ];
        let key_for = |operation| {
            lunco_core_runtime::AsyncWorkKey::new(
                lunco_core_runtime::AsyncWorkKind::VisualizationPreparation,
                0,
                terrain.to_bits() as u128,
                7,
                operation,
            )
        };
        for (coord, operation, _) in tiles {
            pending.jobs.insert(
                coord,
                PendingTileBake {
                    generation: 7,
                    key: key_for(operation),
                },
            );
        }
        let mut arrivals = vec![TerrainTileBakeCompletion {
            terrain,
            coord: c(1, 0, 0),
            key: key_for(99),
            result: Ok(test_baked_tile(-1.0)),
        }];
        arrivals.extend(tiles.iter().rev().map(|(coord, operation, marker)| {
            TerrainTileBakeCompletion {
                terrain,
                coord: *coord,
                key: key_for(*operation),
                result: Ok(test_baked_tile(*marker)),
            }
        }));
        pending.accept_completed_bakes(terrain, arrivals);

        let ready = pending.take_ready(7, tiles.len());

        assert_eq!(
            ready.iter().map(|(coord, _)| *coord).collect::<Vec<_>>(),
            [c(1, 0, 0), c(1, 0, 1), c(1, 1, 0), c(1, 1, 1)],
        );
        assert_eq!(
            ready
                .iter()
                .map(|(_, baked)| baked.center[0])
                .collect::<Vec<_>>(),
            [10.0, 20.0, 30.0, 40.0],
        );
        assert!(pending.ready.is_empty());
        assert!(pending.jobs.is_empty());
    }

    fn diagnostic_template() -> ShaderLook {
        crate::overlay::TerrainDiagnosticLook::default().0
    }

    #[test]
    fn lod_update_queries_keep_terrain_templates_disjoint_from_tile_looks() {
        let mut app = App::new();
        app.init_resource::<TerrainDetailDemands>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<LodMeshCache>()
            .init_resource::<lunco_render::RenderingQualitySettings>()
            .init_resource::<TerrainStreamStatus>()
            .init_resource::<TerrainStreamFrameDriven>()
            .init_resource::<TerrainStreamCadence>()
            .init_resource::<lunco_core_runtime::AsyncWorkAdmission>()
            .init_resource::<TerrainCoverResults>()
            .init_resource::<crate::overlay::TerrainOverlayParams>()
            .init_resource::<crate::overlay::TerrainDiagnosticLook>()
            .add_systems(Update, update_lod_tiles);
        #[cfg(not(target_arch = "wasm32"))]
        app.init_resource::<TerrainTileBakeResults>();

        app.update();
    }

    #[test]
    fn derived_map_appearance_is_identical_across_mesh_depths() {
        let maps = TerrainDerivedMaps {
            surface: Handle::default(),
            normal: Handle::default(),
            surface_key: 0,
            res: 1024,
            texel_size_m: 7.820_137,
        };
        let near = tile_look(
            &production_template(),
            2,
            100.0,
            200.0,
            Some(&maps),
            512.0,
            0.0,
            0.0,
            None,
            None,
            &diagnostic_template(),
            crate::overlay::OverlayUniforms::OFF,
        );
        let far = tile_look(
            &production_template(),
            7,
            100.0,
            200.0,
            Some(&maps),
            512.0,
            0.0,
            0.0,
            None,
            None,
            &diagnostic_template(),
            crate::overlay::OverlayUniforms::OFF,
        );

        assert_eq!(near.textures, far.textures);
        assert_eq!(
            near.values().get("map_texel_size_m"),
            Some(&ParamValue::F32(maps.texel_size_m))
        );
        for name in ["derived_surface_on", "derived_normal_on"] {
            assert_eq!(
                near.values().get(name),
                Some(&ParamValue::F32(1.0)),
                "{name} must be enabled for a derived-only look"
            );
        }
        assert!(!near.values().contains_key("map_ratio"));
        assert_eq!(near.shader, "shaders/terrain_layered.wesl");
        assert_eq!(far.shader, "shaders/terrain_layered.wesl");
        for name in ["mode", "overlay_mode", "overlay_opacity", "lod_depth"] {
            assert!(
                !near.values().contains_key(name),
                "production terrain must not carry diagnostic input {name}"
            );
        }
    }

    #[test]
    fn diagnostic_material_is_a_separate_shader_path() {
        let look = tile_look(
            &production_template(),
            4,
            100.0,
            200.0,
            None,
            512.0,
            0.0,
            0.0,
            None,
            None,
            &diagnostic_template(),
            crate::overlay::OverlayUniforms {
                mode: crate::overlay::overlay_mode::LOD_DEPTH,
                opacity: 1.0,
                safe_rad: 0.0,
                cliff_rad: 0.0,
            },
        );

        assert_eq!(look.shader, "lunco://shaders/terrain_debug.wesl");
        assert_eq!(look.values().get("lod"), Some(&ParamValue::F32(4.0)));
        assert!(!look.values().contains_key("micro_scale"));
    }

    #[test]
    fn absent_derived_maps_are_explicitly_disabled() {
        let look = tile_look(
            &production_template(),
            7,
            100.0,
            200.0,
            None,
            512.0,
            0.0,
            0.0,
            None,
            None,
            &diagnostic_template(),
            crate::overlay::OverlayUniforms::OFF,
        );

        for name in [
            "derived_surface_on",
            "derived_normal_on",
            "authored_surface_on",
            "authored_normal_on",
        ] {
            assert_eq!(
                look.values().get(name),
                Some(&ParamValue::F32(0.0)),
                "{name} must be disabled without a published map"
            );
        }
        assert!(!look.textures.contains_key(&TextureLayer::Surface));
        assert!(!look.textures.contains_key(&TextureLayer::Normal));
    }

    #[test]
    fn authored_map_sources_survive_derived_map_publication() {
        let derived = TerrainDerivedMaps {
            surface: Handle::default(),
            normal: Handle::default(),
            surface_key: 0,
            res: 1024,
            texel_size_m: 7.0,
        };
        let authored = TerrainAuthoredMaps {
            regional_albedo: Some(Handle::<Image>::default().into()),
            surface: Some(Handle::<Image>::default().into()),
            weight_rough: 0.25,
            weight_ao: 0.75,
            normal: Some(Handle::<Image>::default().into()),
            weight_normal: 0.5,
            ..Default::default()
        };
        let look = tile_look(
            &production_template(),
            4,
            100.0,
            200.0,
            Some(&derived),
            512.0,
            1024.0,
            0.125,
            Some(&authored),
            None,
            &diagnostic_template(),
            crate::overlay::OverlayUniforms::OFF,
        );

        assert!(look.textures.contains_key(&TextureLayer::RegionalAlbedo));
        assert_eq!(
            look.values().get("regional_albedo_on"),
            Some(&ParamValue::F32(1.0))
        );
        assert_eq!(
            look.values().get("regional_uv_scale"),
            Some(&ParamValue::F32(0.125))
        );
        assert_eq!(
            look.values().get("authored_surface_on"),
            Some(&ParamValue::F32(1.0))
        );
        assert_eq!(
            look.values().get("authored_normal_on"),
            Some(&ParamValue::F32(1.0))
        );
        assert_eq!(
            look.values().get("derived_surface_on"),
            Some(&ParamValue::F32(0.0))
        );
        assert_eq!(
            look.values().get("derived_normal_on"),
            Some(&ParamValue::F32(0.0))
        );
        assert_eq!(
            look.values().get("weight_rough"),
            Some(&ParamValue::F32(authored.weight_rough))
        );
        assert_eq!(
            look.values().get("weight_ao"),
            Some(&ParamValue::F32(authored.weight_ao))
        );
        assert_eq!(
            look.values().get("weight_normal"),
            Some(&ParamValue::F32(authored.weight_normal))
        );
    }

    #[test]
    fn regional_albedo_without_registered_surface_faults_after_dem_admission() {
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Image>()
            .init_resource::<lunco_core::RuntimeFaults>()
            .add_systems(Update, bind_terrain_maps_to_materials);
        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        let terrain = app
            .world_mut()
            .spawn((
                DemTerrainSurface,
                ShaderLook::new("shaders/terrain_layered.wesl")
                    .with_texture(TextureLayer::RegionalAlbedo, image),
            ))
            .id();
        app.update();
        assert!(!app.world().resource::<lunco_core::RuntimeFaults>().active());
        let grid = Arc::new(lunco_obstacle_field::field::HeightGrid::new_flat(3, 10.0));
        let oracle = crate::oracle::SurfaceOracle::bare(grid);
        app.world_mut()
            .entity_mut(terrain)
            .insert(DemHeightField(Arc::new(oracle)));
        app.update();
        let fault = app
            .world()
            .resource::<lunco_core::RuntimeFaults>()
            .first
            .as_ref()
            .unwrap();
        assert_eq!(fault.kind, "terrain-regional-material");
        assert_eq!(fault.entity, Some(terrain));
    }

    #[test]
    fn reauthored_shader_look_refreshes_the_shared_terrain_source_view() {
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Image>()
            .init_resource::<lunco_core::RuntimeFaults>()
            .add_systems(Update, bind_terrain_maps_to_materials);
        let first = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        let second = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        let terrain = app
            .world_mut()
            .spawn((
                DemTerrainSurface,
                ShaderLook::new("shaders/terrain_layered.wesl")
                    .with_texture(TextureLayer::Albedo, first.clone()),
            ))
            .id();

        app.update();
        assert_eq!(
            app.world()
                .get::<TerrainAuthoredMaps>(terrain)
                .expect("initial USD look is projected")
                .albedo,
            Some(first.into())
        );

        *app.world_mut()
            .get_mut::<ShaderLook>(terrain)
            .expect("terrain look") = ShaderLook::new("shaders/terrain_layered.wesl")
            .with_texture(TextureLayer::Albedo, second.clone());
        app.update();

        assert_eq!(
            app.world()
                .get::<TerrainAuthoredMaps>(terrain)
                .expect("reauthored USD look keeps the terrain view")
                .albedo,
            Some(second.into())
        );
    }

    #[test]
    fn initial_lit_material_waits_for_the_complete_source_projection() {
        let authored = TerrainAuthoredMaps {
            surface: Some(Handle::<Image>::default().into()),
            weight_rough: 0.5,
            normal: Some(Handle::<Image>::default().into()),
            weight_normal: 0.5,
            ..Default::default()
        };
        let authored_normal_only = TerrainAuthoredMaps {
            normal: authored.normal.clone(),
            weight_normal: authored.weight_normal,
            ..Default::default()
        };
        let derived = TerrainDerivedMaps {
            surface: Handle::default(),
            normal: Handle::default(),
            surface_key: 0,
            res: 1024,
            texel_size_m: 7.0,
        };

        assert!(!initial_lit_material_ready(None, None));
        assert!(!initial_lit_material_ready(Some(&derived), None));
        assert!(initial_lit_material_ready(None, Some(&authored)));
        assert!(!initial_lit_material_ready(
            None,
            Some(&authored_normal_only)
        ));
        assert!(initial_lit_material_ready(
            Some(&derived),
            Some(&authored_normal_only)
        ));
    }

    #[test]
    fn removed_derived_maps_reconcile_resident_tiles() {
        let mut app = App::new();
        app.add_systems(Update, sync_removed_terrain_maps_to_materials);

        let tile = app
            .world_mut()
            .spawn(ShaderLook::new("shaders/terrain_layered.wesl"))
            .id();
        let mut tile_entity = app.world_mut().entity_mut(tile);
        let mut shader_look = tile_entity.get_mut::<ShaderLook>().expect("tile look");
        shader_look
            .textures
            .insert(TextureLayer::Surface, Handle::<Image>::default().into());
        shader_look
            .textures
            .insert(TextureLayer::Normal, Handle::<Image>::default().into());
        set_param(&mut shader_look, "map_texel_size_m", ParamValue::F32(7.0));
        set_param(&mut shader_look, "derived_surface_on", ParamValue::F32(1.0));
        set_param(&mut shader_look, "derived_normal_on", ParamValue::F32(1.0));
        let mut tiles = LodView::default();
        tiles.tiles.insert(
            QuadCoord::ROOT,
            TileSlot {
                resolution: 3,
                half_extent: 1.0,
                entity: tile,
                r#gen: 0,
                morph_end: f32::INFINITY,
                drawn: true,
                ready: true,
                stitch_edges: [0.0; 4],
            },
        );
        let terrain = app
            .world_mut()
            .spawn((
                LodTiles {
                    views: BTreeMap::from([(Entity::PLACEHOLDER, tiles)]),
                    ..Default::default()
                },
                DemTerrainSurface,
                TerrainDerivedMaps {
                    surface: Handle::default(),
                    normal: Handle::default(),
                    surface_key: 0,
                    res: 1024,
                    texel_size_m: 7.0,
                },
            ))
            .id();

        app.world_mut()
            .entity_mut(terrain)
            .remove::<TerrainDerivedMaps>();
        app.update();

        let tile_ref = app.world().entity(tile);
        let look = tile_ref.get::<ShaderLook>().expect("tile look retained");
        assert!(!look.textures.contains_key(&TextureLayer::Surface));
        assert!(!look.textures.contains_key(&TextureLayer::Normal));
        assert_eq!(
            look.values().get("map_texel_size_m"),
            Some(&ParamValue::F32(1.0))
        );
        assert_eq!(
            look.values().get("derived_surface_on"),
            Some(&ParamValue::F32(0.0))
        );
        assert_eq!(
            look.values().get("derived_normal_on"),
            Some(&ParamValue::F32(0.0))
        );
    }

    #[test]
    fn camera_projection_uses_the_nearest_shared_grid_for_sibling_branches() {
        let mut world = World::new();
        let root = world
            .spawn(lunco_spatial::WorldGridConfig::default().grid())
            .id();

        let camera_grid_cell = CellCoord::new(2, 0, -1);
        let camera_grid_transform =
            Transform::from_xyz(10.0, 20.0, 30.0).with_rotation(Quat::from_rotation_y(0.4));
        let camera_grid = world
            .spawn((
                Grid::new(800.0, 40.0),
                camera_grid_cell,
                camera_grid_transform,
                ChildOf(root),
            ))
            .id();
        let camera_cell = CellCoord::new(1, 0, 2);
        let camera_transform =
            Transform::from_xyz(3.0, 5.0, -7.0).with_rotation(Quat::from_rotation_x(-0.2));
        let camera = world
            .spawn((camera_cell, camera_transform, ChildOf(camera_grid)))
            .id();

        let terrain_grid_cell = CellCoord::new(-3, 1, 4);
        let terrain_grid_transform =
            Transform::from_xyz(-12.0, 8.0, 6.0).with_rotation(Quat::from_rotation_y(-0.7));
        let terrain_grid = world
            .spawn((
                Grid::new(1_200.0, 60.0),
                terrain_grid_cell,
                terrain_grid_transform,
                ChildOf(root),
            ))
            .id();
        let terrain_transform =
            Transform::from_xyz(-4.0, 2.0, 9.0).with_rotation(Quat::from_rotation_z(0.3));
        let terrain = world.spawn((terrain_transform, ChildOf(terrain_grid))).id();

        let mut state: bevy::ecs::system::SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = bevy::ecs::system::SystemState::new(&mut world);
        let (parents, grids, spatial) = state.get(&world).unwrap();
        let (actual_position, actual_forward) =
            camera_pose_in_terrain(camera, terrain, &parents, &grids, &spatial)
                .expect("sibling grids share the root grid");

        let root_grid = grids.get(root).unwrap();
        let camera_branch = grids.get(camera_grid).unwrap();
        let terrain_branch = grids.get(terrain_grid).unwrap();
        let camera_grid_position =
            root_grid.grid_position_double(&camera_grid_cell, &camera_grid_transform);
        let camera_position = camera_grid_position
            + camera_grid_transform.rotation.as_dquat()
                * camera_branch.grid_position_double(&camera_cell, &camera_transform);
        let camera_rotation =
            camera_grid_transform.rotation.as_dquat() * camera_transform.rotation.as_dquat();
        let terrain_grid_position =
            root_grid.grid_position_double(&terrain_grid_cell, &terrain_grid_transform);
        let terrain_position = terrain_grid_position
            + terrain_grid_transform.rotation.as_dquat()
                * terrain_branch.grid_position_double(&CellCoord::ZERO, &terrain_transform);
        let terrain_rotation =
            terrain_grid_transform.rotation.as_dquat() * terrain_transform.rotation.as_dquat();
        let expected_position = terrain_rotation.inverse() * (camera_position - terrain_position);
        let expected_forward = terrain_rotation.inverse() * (camera_rotation * DVec3::NEG_Z);

        assert!((actual_position - expected_position).length() < 1.0e-9);
        assert!((actual_forward - expected_forward).length() < 1.0e-9);
    }

    #[test]
    fn disconnected_big_spaces_do_not_manufacture_a_lod_demand() {
        let mut world = World::new();
        let camera_grid = world.spawn(Grid::new(1_000.0, 100.0)).id();
        let terrain_grid = world.spawn(Grid::new(1_000.0, 100.0)).id();
        let camera = world
            .spawn((Transform::default(), ChildOf(camera_grid)))
            .id();
        let terrain = world
            .spawn((Transform::default(), ChildOf(terrain_grid)))
            .id();

        let mut state: bevy::ecs::system::SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = bevy::ecs::system::SystemState::new(&mut world);
        let (parents, grids, spatial) = state.get(&world).unwrap();
        assert!(camera_pose_in_terrain(camera, terrain, &parents, &grids, &spatial).is_none());
    }

    #[test]
    fn finite_terrain_keeps_a_demand_when_camera_is_beyond_its_edge() {
        let camera = VisualDemand {
            render_layer: 64,
            source: Entity::PLACEHOLDER,
            terrain_poses: HashMap::new(),
            terrain_clips: HashMap::new(),
            preparation_corridors: HashMap::new(),
            entity: Entity::PLACEHOLDER,
            position: DVec3::new(-550.0, 100.0, -550.0),
            forward: DVec3::NEG_Z,
            screen_height_px: 1080.0,
            projection: test_perspective(std::f64::consts::FRAC_PI_4, DVec3::ZERO, DVec3::NEG_Z),
            near_detail_radius_m: 30.0,
            near_detail_hysteresis_m: 12.0,
        };

        let visual = terrain_visual_demand(
            &camera,
            camera.position,
            camera.forward,
            camera.projection.clip_from_local()
                * test_view(camera.position, camera.forward, DVec3::Y),
            2.0,
        )
        .expect("finite camera pose remains a valid visual demand");

        assert_eq!(visual.focus, [-550.0, -550.0]);
        assert_eq!(visual.camera_y, 100.0);
        assert!(visual.required);
        assert!(visual.near_detail_radius_m > 0.0);
    }

    fn c(depth: u8, x: u32, z: u32) -> QuadCoord {
        QuadCoord { depth, x, z }
    }

    #[test]
    fn late_shadow_cache_preserves_terrain_caster_and_receiver() {
        let mut app = App::new();
        app.add_systems(PostUpdate, bind_shadow_cache_to_tiles);

        let tile = app
            .world_mut()
            .spawn((
                ShaderLook::new("shaders/terrain_layered.wesl"),
                // A resident tile may carry stale shadow flags when its cache
                // is rebound. The shared helper must restore the current
                // terrain caster/receiver contract.
                bevy::light::NotShadowReceiver,
            ))
            .id();
        let mut tiles = LodView::default();
        tiles.tiles.insert(
            QuadCoord::ROOT,
            TileSlot {
                resolution: 3,
                half_extent: 1.0,
                entity: tile,
                r#gen: 0,
                morph_end: f32::INFINITY,
                drawn: true,
                ready: true,
                stitch_edges: [0.0; 4],
            },
        );
        let shadow_image = Handle::<Image>::default();
        app.world_mut().spawn((
            TerrainLodViz,
            LodTiles {
                views: BTreeMap::from([(Entity::PLACEHOLDER, tiles)]),
                ..Default::default()
            },
            TileShadowCache {
                image: shadow_image.clone(),
                on: 1.0,
            },
        ));

        app.update();

        let tile_ref = app.world().entity(tile);
        assert!(!tile_ref.get::<ShaderLook>().unwrap().no_shadow_cast);
        assert!(!tile_ref.contains::<bevy::light::NotShadowReceiver>());
        let look = tile_ref.get::<ShaderLook>().expect("tile look retained");
        assert_eq!(
            look.textures
                .get(&TextureLayer::ShadowCache)
                .and_then(lunco_materials::ShaderTexture::image),
            Some(&shadow_image)
        );
        assert_eq!(
            look.values().get("shadow_cache_on"),
            Some(&ParamValue::F32(1.0))
        );
    }

    #[test]
    fn disabled_shadow_cache_preserves_terrain_caster_and_receiver() {
        let mut app = App::new();
        app.add_systems(PostUpdate, bind_shadow_cache_to_tiles);

        let tile = app
            .world_mut()
            .spawn((
                ShaderLook::new("shaders/terrain_layered.wesl"),
                bevy::light::NotShadowReceiver,
            ))
            .id();
        let mut tiles = LodView::default();
        tiles.tiles.insert(
            QuadCoord::ROOT,
            TileSlot {
                resolution: 3,
                half_extent: 1.0,
                entity: tile,
                r#gen: 0,
                morph_end: f32::INFINITY,
                drawn: true,
                ready: true,
                stitch_edges: [0.0; 4],
            },
        );
        app.world_mut().spawn((
            TerrainLodViz,
            LodTiles {
                views: BTreeMap::from([(Entity::PLACEHOLDER, tiles)]),
                ..Default::default()
            },
            TileShadowCache {
                image: Handle::<Image>::default(),
                on: 0.0,
            },
        ));

        app.update();

        let tile_ref = app.world().entity(tile);
        assert!(!tile_ref.get::<ShaderLook>().unwrap().no_shadow_cast);
        assert!(!tile_ref.contains::<bevy::light::NotShadowReceiver>());
    }

    #[test]
    fn fallback_is_exactly_one_root_tile() {
        assert_eq!(fallback_coords().collect::<Vec<_>>(), vec![QuadCoord::ROOT]);
        assert_eq!(coarse_fallback_tile_count(), 1);
        assert!(is_coarse_fallback(QuadCoord::ROOT));
        assert!(!is_coarse_fallback(c(1, 0, 0)));
    }

    #[test]
    fn render_pending_count_includes_material_publication() {
        let wanted = QuadCoord::ROOT
            .children()
            .into_iter()
            .collect::<HashSet<_>>();
        let unready = wanted.iter().next().copied().expect("four children");
        let mut tiles = LodView::default();
        tiles.tiles.insert(
            QuadCoord::ROOT,
            TileSlot {
                resolution: 3,
                half_extent: 1.0,
                entity: Entity::PLACEHOLDER,
                r#gen: 0,
                morph_end: f32::INFINITY,
                drawn: false,
                ready: false,
                stitch_edges: [0.0; 4],
            },
        );
        tiles.tiles.insert(
            unready,
            TileSlot {
                resolution: 3,
                half_extent: 1.0,
                entity: Entity::PLACEHOLDER,
                r#gen: 0,
                morph_end: 0.0,
                drawn: false,
                ready: false,
                stitch_edges: [0.0; 4],
            },
        );

        assert_eq!(render_pending_count(2, &wanted, &tiles), 4);
        tiles.coarse_ready = true;
        assert_eq!(render_pending_count(2, &wanted, &tiles), 3);
        tiles.tiles.get_mut(&unready).unwrap().ready = true;
        assert_eq!(render_pending_count(2, &wanted, &tiles), 2);
    }

    #[test]
    fn material_readiness_invalidation_reopens_cached_gate() {
        let tile = Entity::PLACEHOLDER;
        let mut tiles = LodView {
            materials_ready: true,
            ..Default::default()
        };
        tiles.tiles.insert(
            QuadCoord::ROOT,
            TileSlot {
                resolution: 3,
                half_extent: 1.0,
                entity: tile,
                r#gen: 0,
                morph_end: f32::INFINITY,
                drawn: true,
                ready: true,
                stitch_edges: [0.0; 4],
            },
        );

        tiles.invalidate_material_readiness(tile);

        assert!(!tiles.materials_ready);
        let slot = tiles.tiles.get(&QuadCoord::ROOT).unwrap();
        assert!(!slot.ready);
        assert!(!slot.drawn);
    }

    #[test]
    fn shader_look_removal_observer_invalidates_matching_terrain_tile() {
        let mut app = App::new();
        app.add_observer(invalidate_removed_shader_look_ready);

        let tile = app.world_mut().spawn(ShaderLookReady).id();
        let mut terrain_tiles = LodView {
            materials_ready: true,
            ..Default::default()
        };
        terrain_tiles.tiles.insert(
            QuadCoord::ROOT,
            TileSlot {
                resolution: 3,
                half_extent: 1.0,
                entity: tile,
                r#gen: 0,
                morph_end: f32::INFINITY,
                drawn: true,
                ready: true,
                stitch_edges: [0.0; 4],
            },
        );
        app.world_mut()
            .entity_mut(tile)
            .insert(Visibility::Inherited);
        let terrain = app
            .world_mut()
            .spawn(LodTiles {
                frame_cursor: 0,
                resolution: 49,
                r#gen: 0,
                views: BTreeMap::from([(Entity::PLACEHOLDER, terrain_tiles)]),
            })
            .id();

        app.world_mut().entity_mut(tile).remove::<ShaderLookReady>();

        let terrain_tiles =
            &app.world().get::<LodTiles>(terrain).unwrap().views[&Entity::PLACEHOLDER];
        assert!(!terrain_tiles.materials_ready);
        let slot = terrain_tiles.tiles.get(&QuadCoord::ROOT).unwrap();
        assert!(!slot.ready);
        assert!(!slot.drawn);
        assert_eq!(
            app.world().get::<Visibility>(tile),
            Some(&Visibility::Hidden)
        );
    }

    #[test]
    fn edge_stitching_is_topological_and_safe_at_dem_boundaries() {
        let draw = HashSet::from([QuadCoord::ROOT]);
        // This leaf touches the root's north and west DEM boundaries. Those
        // edges have no neighbour and must never be treated as seams; only the
        // two interior edges face the coarser root stand-in.
        assert_eq!(stitch_edges(c(1, 0, 0), &draw), [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(stitch_edges(c(1, 1, 1), &draw), [1.0, 0.0, 1.0, 0.0]);
        assert_eq!(stitch_edges(QuadCoord::ROOT, &draw), [0.0; 4]);
    }

    #[test]
    fn wanted_children_retain_only_their_direct_parent_fallback() {
        let parent = c(2, 1, 2);
        let wanted = parent.children().into_iter().collect::<HashSet<_>>();
        let mut fallbacks = HashSet::new();

        wanted_parent_fallbacks(&wanted, &mut fallbacks);

        assert_eq!(fallbacks, HashSet::from([parent]));
    }

    /// Every selected area must end up covered by exactly ONE drawn node — itself or an
    /// ancestor. This is the invariant whose absence rendered as black terrain.
    fn assert_covered_exactly_once(sel: &[QuadCoord], draw: &HashSet<QuadCoord>) {
        for s in sel {
            let mut covers = 0;
            let mut cur = Some(*s);
            while let Some(n) = cur {
                if draw.contains(&n) {
                    covers += 1;
                }
                cur = n.parent();
            }
            assert_eq!(
                covers, 1,
                "{s:?} covered {covers}× (want exactly 1); draw={draw:?}"
            );
        }
    }

    #[test]
    fn all_ready_draws_the_leaves_themselves() {
        let sel = c(1, 0, 0).children().to_vec();
        let (mut draw, mut scratch) = (HashSet::new(), Vec::new());
        build_draw_partition(sel.iter().copied(), |_| true, &mut draw, &mut scratch);
        assert_eq!(draw.len(), 4);
        assert_covered_exactly_once(&sel, &draw);
    }

    #[test]
    fn unready_leaf_falls_back_to_its_ready_ancestor() {
        // Nothing fine is ready; only the root is. Every leaf must fall back to it, and the
        // root must be drawn ONCE — not once per leaf.
        let sel = c(1, 0, 0).children().to_vec();
        let (mut draw, mut scratch) = (HashSet::new(), Vec::new());
        build_draw_partition(
            sel.iter().copied(),
            |n| n.depth == 0,
            &mut draw,
            &mut scratch,
        );
        assert_eq!(
            draw.len(),
            1,
            "one ancestor stands in for the whole subtree"
        );
        assert!(draw.contains(&QuadCoord::ROOT));
        assert_covered_exactly_once(&sel, &draw);
    }

    #[test]
    fn unready_coarsened_parent_keeps_its_ready_outgoing_children() {
        // A cover merge selects `parent` immediately, but its mesh is only
        // queued afterwards. The four outgoing children are the exact previous
        // cover and must bridge that bake; otherwise the root flashes through
        // before the parent arrives.
        let parent = c(1, 0, 0);
        let children = parent.children();
        let (mut draw, mut scratch) = (HashSet::new(), Vec::new());
        build_draw_partition(
            std::iter::once(parent),
            |node| children.contains(&node),
            &mut draw,
            &mut scratch,
        );

        assert_eq!(draw, children.into_iter().collect());
        assert!(!draw.contains(&QuadCoord::ROOT));
    }

    #[test]
    fn ready_sibling_is_dropped_when_an_ancestor_stands_in() {
        // THE overlap case: one child ready, its sibling not. The parent must stand in for
        // the quad, and the ready child must NOT also draw — drawing both is the coarse
        // surface punching through the fine one (z-fighting), which is why ForbidHoles
        // unrefines instead of underlaying.
        let kids = c(1, 0, 0).children();
        let ready = kids[0];
        let sel = kids.to_vec();
        let (mut draw, mut scratch) = (HashSet::new(), Vec::new());
        build_draw_partition(
            sel.iter().copied(),
            |n| n.depth == 0 || n == ready,
            &mut draw,
            &mut scratch,
        );
        assert!(draw.contains(&QuadCoord::ROOT));
        assert!(
            !draw.contains(&ready),
            "ready child must not draw under a drawn ancestor"
        );
        assert_covered_exactly_once(&sel, &draw);
    }

    #[test]
    fn nothing_ready_leaves_the_area_uncovered_rather_than_wrong() {
        // Before the coarse base lands there is genuinely nothing to draw. It must degrade
        // to "no tile" — never to a bogus one — and must not panic walking off the root.
        let sel = [c(3, 5, 2)];
        let (mut draw, mut scratch) = (HashSet::new(), Vec::new());
        build_draw_partition(sel.iter().copied(), |_| false, &mut draw, &mut scratch);
        assert!(draw.is_empty());
    }

    /// THE REGRESSION TEST for the terrain blanking on a generation bump.
    ///
    /// `invalidate()` keeps every tile and only marks it stale. Keying visibility on
    /// freshness hid all of them at once — caught live as
    /// `uncovered=265 drawn=0 resident=510`. Keyed on residency, each tile keeps
    /// drawing ITSELF until its own re-bake replaces it in place.
    ///
    /// The check that matters is `draw == sel`: every node draws itself, so nothing
    /// is substituted for anything. The earlier rejected fix satisfied "not empty"
    /// while swapping in coarse ancestors, which alternated and looked worse.
    #[test]
    fn a_generation_bump_keeps_each_tile_drawing_itself() {
        let sel = vec![c(3, 0, 0), c(3, 1, 0), c(2, 1, 0), c(2, 0, 1)];
        // Everything resident, nothing fresh — exactly the post-invalidate state.
        let resident: HashSet<QuadCoord> = sel.iter().copied().collect();

        let (mut draw, mut scratch) = (HashSet::new(), Vec::new());
        build_draw_partition(
            sel.iter().copied(),
            |n| resident.contains(&n),
            &mut draw,
            &mut scratch,
        );

        let want: HashSet<QuadCoord> = sel.iter().copied().collect();
        assert_eq!(
            draw, want,
            "each stale-but-resident tile must keep drawing ITSELF; substituting a \
             different node is the rejected fix that looked worse"
        );
        assert_covered_exactly_once(&sel, &draw);
    }

    // ── incremental cover (`evolve_cover`) ───────────────────────────────────

    fn test_qt() -> Quadtree {
        // range_factor 1.0 → refine_range == node error, so the tests state
        // distances directly in metres.
        Quadtree::new(1000.0, 6, 1.0, 100.0)
    }

    #[test]
    fn camera_patch_retention_preserves_only_requested_nodes_and_source() {
        let mut world = World::new();
        let camera = world.spawn_empty().id();
        let other_camera = world.spawn_empty().id();
        let oracle = Arc::new(SurfaceOracle::bare(Arc::new(
            lunco_obstacle_field::field::HeightGrid::new_flat(3, 80.0),
        )));
        let make_request = |camera, focus, camera_y, camera_patches| TerrainCoverRequest {
            quadtree: Quadtree::from_screen_metric(
                80.0,
                5,
                80.0,
                720.0,
                std::f64::consts::FRAC_PI_3,
                2.0,
                0.55,
            ),
            oracle: Arc::clone(&oracle),
            oracle_key: oracle.surface_key(),
            demands: vec![TerrainVisualDemand {
                camera,
                focus,
                camera_y,
                heading: None,
                view_direction: DVec3::Y,
                screen_height_px: 720.0,
                selection_clip: SelectionClip::new(
                    test_perspective(
                        std::f64::consts::FRAC_PI_3,
                        DVec3::new(focus[0], camera_y, focus[1]),
                        DVec3::Y,
                    ),
                    720.0,
                    2.0,
                    DVec3::new(focus[0], camera_y, focus[1]),
                )
                .unwrap(),
                projection: test_perspective(
                    std::f64::consts::FRAC_PI_3,
                    DVec3::new(focus[0], camera_y, focus[1]),
                    DVec3::Y,
                ),
                required: true,
                near_detail_radius_m: 30.0,
                near_detail_hysteresis_m: 12.0,
            }],
            camera_patches,
            cover: HashSet::new(),
            node_geometry: HashMap::new(),
            bootstrap_ready: true,
            settled_sig: None,
            budget_refused: 0,
            signature: 7,
            preparation_signature: None,
            preparation: None,
            tile_resolution: 49,
            probe_resolution: 5,
            tile_budget: 128,
            max_depth: 5,
            cover_edits_per_cycle: 8,
            hysteresis_ratio: 1.3,
        };
        let initial = prepare_terrain_cover(make_request(
            camera,
            [0.0, 0.0],
            20.0,
            CameraPatchRetention::default(),
        ));
        let patch = initial
            .camera_patches
            .patches
            .get(&camera)
            .expect("near camera patch")
            .clone();
        assert_eq!(patch.len(), 9);
        assert!(
            patch.iter().all(|coord| initial
                .selected
                .iter()
                .any(|tile| tile.coord == *coord && tile.morph_end.is_infinite())),
            "the requested patch must be pinned in the selected cover"
        );
        let retained = prepare_terrain_cover(make_request(
            camera,
            [4.0, 0.0],
            35.0,
            initial.camera_patches.clone(),
        ));
        assert_eq!(
            retained.camera_patches.patches.get(&camera),
            Some(&patch),
            "the retention band must not request a new patch at the moved camera"
        );
        assert!(
            patch.iter().all(|coord| retained
                .selected
                .iter()
                .any(|tile| tile.coord == *coord && tile.morph_end.is_infinite())),
            "the retained patch must remain pinned in the selected cover"
        );
        let moved_away = prepare_terrain_cover(make_request(
            camera,
            [60.0, 0.0],
            35.0,
            retained.camera_patches.clone(),
        ));
        assert!(
            moved_away.camera_patches.patches.is_empty(),
            "a patch outside the three-dimensional retention range must be released"
        );
        let released = prepare_terrain_cover(make_request(
            camera,
            [4.0, 0.0],
            43.0,
            retained.camera_patches.clone(),
        ));
        assert!(
            released.camera_patches.patches.is_empty(),
            "patch must expire beyond hysteresis"
        );
        let replacement = prepare_terrain_cover(make_request(
            other_camera,
            [4.0, 0.0],
            35.0,
            initial.camera_patches.clone(),
        ));
        assert!(
            replacement.camera_patches.patches.is_empty(),
            "another camera must not inherit the patch"
        );
        let edge = prepare_terrain_cover(make_request(
            camera,
            [79.0, 0.0],
            20.0,
            CameraPatchRetention::default(),
        ));
        let edge_patch = edge
            .camera_patches
            .patches
            .get(&camera)
            .expect("edge patch")
            .clone();
        let outside =
            prepare_terrain_cover(make_request(camera, [82.0, 0.0], 35.0, edge.camera_patches));
        assert_eq!(
            outside.camera_patches.patches.get(&camera),
            Some(&edge_patch),
            "a camera just outside the terrain keeps nearby requested nodes"
        );
        let outside_far = prepare_terrain_cover(make_request(
            camera,
            [130.0, 0.0],
            35.0,
            outside.camera_patches,
        ));
        assert!(
            outside_far.camera_patches.patches.is_empty(),
            "a camera beyond the terrain must release distant nodes"
        );
        let mut wrong_source = initial.camera_patches.clone();
        wrong_source.oracle_key ^= 1;
        let stale = prepare_terrain_cover(make_request(camera, [0.0, 0.0], 35.0, wrong_source));
        assert!(
            stale.camera_patches.patches.is_empty(),
            "another surface must not inherit the patch"
        );
        let mut wrong_extent = initial.camera_patches.clone();
        wrong_extent.root_half_bits ^= 1;
        let stale = prepare_terrain_cover(make_request(camera, [0.0, 0.0], 35.0, wrong_extent));
        assert!(
            stale.camera_patches.patches.is_empty(),
            "another extent must not inherit the patch"
        );
        let mut wrong_depth = initial.camera_patches;
        wrong_depth.max_depth -= 1;
        let stale = prepare_terrain_cover(make_request(camera, [0.0, 0.0], 35.0, wrong_depth));
        assert!(
            stale.camera_patches.patches.is_empty(),
            "another depth must not inherit the patch"
        );
    }

    #[test]
    fn camera_diagonal_neighbours_cover_corners_and_clip_at_root() {
        let centre = QuadCoord {
            depth: 3,
            x: 4,
            z: 4,
        };
        let neighbours: HashSet<_> = camera_diagonal_neighbours(centre).collect();
        let expected: HashSet<_> = [(3, 3), (3, 5), (5, 3), (5, 5)]
            .into_iter()
            .map(|(x, z)| QuadCoord { depth: 3, x, z })
            .collect();
        assert_eq!(neighbours, expected);
        let corner = QuadCoord {
            depth: 3,
            x: 0,
            z: 0,
        };
        let clipped: HashSet<_> = camera_diagonal_neighbours(corner).collect();
        assert_eq!(
            clipped,
            HashSet::from([QuadCoord {
                depth: 3,
                x: 1,
                z: 1
            }])
        );
    }

    #[test]
    fn terrain_view_focus_ignores_subquantum_craters() {
        let grid = Arc::new(lunco_obstacle_field::field::HeightGrid::new_flat(3, 100.0));
        let flat = SurfaceOracle::bare(Arc::clone(&grid));
        let crater = lunco_terrain_core::Crater {
            center: [15.0, 0.0],
            radius: 0.4,
            depth: 0.5,
            rim_height: 0.1,
            softness: 0.0,
            bowl_power: 4.0,
        };
        let detailed = SurfaceOracle::new(
            grid,
            vec![crate::oracle::HeightContribution {
                modifier: Arc::new(lunco_terrain_core::Craters::new(vec![crater])),
                content_key: 1,
            }],
        );
        let direction = DVec3::new(15.0, -10.0, 0.0).normalize();
        let demand = TerrainVisualDemand {
            camera: Entity::PLACEHOLDER,
            focus: [0.0, 0.0],
            camera_y: 10.0,
            heading: None,
            view_direction: direction,
            screen_height_px: 720.0,
            selection_clip: SelectionClip::new(
                test_perspective(std::f64::consts::FRAC_PI_3, DVec3::Y * 10.0, direction),
                720.0,
                2.0,
                DVec3::Y * 10.0,
            )
            .unwrap(),
            projection: test_perspective(std::f64::consts::FRAC_PI_3, DVec3::Y * 10.0, direction),
            required: true,
            near_detail_radius_m: 30.0,
            near_detail_hysteresis_m: 12.0,
        };
        let origin = DVec3::new(0.0, 10.0, 0.0);
        let physical = crate::oracle::raycast_surface(&detailed, origin, direction, 60.0)
            .unwrap()
            .unwrap();
        assert!(
            (physical.x - 15.0).abs() > 0.1,
            "fixture must change the physical hit"
        );
        let a = terrain_view_focus(&flat, demand);
        let b = terrain_view_focus(&detailed, demand);
        assert!(
            (a[0] - b[0]).abs() < 1e-8 && (a[1] - b[1]).abs() < 1e-8,
            "sub-quantum crater must not move the LOD focus: {a:?} != {b:?}"
        );
    }

    #[test]
    fn terrain_view_focus_fades_at_the_finite_reach() {
        let oracle = SurfaceOracle::bare(Arc::new(
            lunco_obstacle_field::field::HeightGrid::new_flat(3, 500.0),
        ));
        let demand = TerrainVisualDemand {
            camera: Entity::PLACEHOLDER,
            focus: [0.0, 0.0],
            camera_y: 10.0,
            heading: None,
            view_direction: DVec3::new(15.0, -10.0, 0.0).normalize(),
            screen_height_px: 720.0,
            selection_clip: SelectionClip::new(
                test_perspective(
                    std::f64::consts::FRAC_PI_3,
                    DVec3::Y * 10.0,
                    DVec3::new(15.0, -10.0, 0.0).normalize(),
                ),
                720.0,
                2.0,
                DVec3::Y * 10.0,
            )
            .unwrap(),
            projection: test_perspective(
                std::f64::consts::FRAC_PI_3,
                DVec3::Y * 10.0,
                DVec3::new(15.0, -10.0, 0.0).normalize(),
            ),
            required: true,
            near_detail_radius_m: 30.0,
            near_detail_hysteresis_m: 12.0,
        };
        let near = terrain_view_focus(&oracle, demand);
        assert!((near[0] - 7.5).abs() < 0.01, "near hit: {near:?}");
        let edge = terrain_view_focus(
            &oracle,
            TerrainVisualDemand {
                view_direction: DVec3::new(58.9, -10.0, 0.0).normalize(),
                ..demand
            },
        );
        assert!(edge[0].abs() < 0.01, "reach boundary must fade: {edge:?}");
        let high = terrain_view_focus(
            &oracle,
            TerrainVisualDemand {
                camera_y: 29.0,
                view_direction: DVec3::new(30.0, -29.0, 0.0).normalize(),
                ..demand
            },
        );
        let horizontal_reach = (30.0_f64.powi(2) - 29.0_f64.powi(2)).sqrt();
        assert!(
            high[0] > 0.0 && high[0] <= horizontal_reach + 1e-9,
            "near field must include the camera ground point: {high:?}"
        );
        let above = terrain_view_focus(
            &oracle,
            TerrainVisualDemand {
                camera_y: 30.001,
                view_direction: DVec3::new(30.0, -30.001, 0.0).normalize(),
                ..demand
            },
        );
        assert_eq!(
            above, demand.focus,
            "bias must vanish at the clearance limit"
        );
        let sky = terrain_view_focus(
            &oracle,
            TerrainVisualDemand {
                view_direction: DVec3::Y,
                ..demand
            },
        );
        assert_eq!(sky, demand.focus);
    }

    #[test]
    fn camera_near_field_with_sky_view_reaches_finest_cover() {
        let qt = Quadtree::from_screen_metric(
            1000.0,
            6,
            1000.0,
            1080.0,
            std::f64::consts::FRAC_PI_4,
            terrain_quality().terrain_lod_pixel_error,
            terrain_quality().terrain_lod_morph_start_ratio,
        );
        let actual = TerrainVisualDemand {
            camera: Entity::PLACEHOLDER,
            focus: [0.0, 0.0],
            camera_y: 3.0,
            heading: None,
            view_direction: DVec3::Y,
            screen_height_px: 1080.0,
            selection_clip: SelectionClip::new(
                test_perspective(std::f64::consts::FRAC_PI_4, DVec3::Y * 3.0, DVec3::Y),
                1080.0,
                2.0,
                DVec3::Y * 3.0,
            )
            .unwrap(),
            projection: test_perspective(std::f64::consts::FRAC_PI_4, DVec3::Y * 3.0, DVec3::Y),
            required: true,
            near_detail_radius_m: 30.0,
            near_detail_hysteresis_m: 12.0,
        };
        let demands = [actual];
        let elevated = [TerrainVisualDemand {
            camera_y: 200.0,
            ..actual
        }];
        let root = qt.region(QuadCoord::ROOT);
        assert!(
            projected_posting_error_floor(root, 49) > 0.0,
            "projected posting discovery applies beyond the near-detail radius"
        );
        assert!(!near_field_retains_refinement(root, [0.0, 0.0], &elevated));
        assert!(projected_posting_error_floor(root, 49) > 0.0);
        let oracle = Arc::new(SurfaceOracle::bare(Arc::new(
            lunco_obstacle_field::field::HeightGrid::new_flat(3, 1000.0),
        )));
        let prepared = prepare_terrain_cover(TerrainCoverRequest {
            quadtree: qt,
            oracle: oracle.clone(),
            oracle_key: oracle.surface_key(),
            demands: demands.to_vec(),
            camera_patches: CameraPatchRetention::default(),
            cover: HashSet::new(),
            node_geometry: HashMap::new(),
            bootstrap_ready: true,
            settled_sig: None,
            budget_refused: 0,
            signature: 1,
            preparation_signature: None,
            preparation: None,
            tile_resolution: terrain_quality().terrain_lod_tile_resolution,
            probe_resolution: 5,
            tile_budget: 768,
            max_depth: qt.max_depth,
            cover_edits_per_cycle: 64,
            hysteresis_ratio: 1.3,
        });
        // Admission adds the mandatory camera patch to the published selection.
        // The discretionary tree may stay coarse when terrain is behind the view.
        let cover = prepared.selected.iter().map(|tile| tile.coord).collect();

        assert_eq!(
            required_focus_depth(&cover, &demands, &qt),
            Some(qt.max_depth),
            "the exact camera branch must reach finest detail in the opening update"
        );
        let nearby_depth = cover
            .iter()
            .find(|&&coord| qt.region(coord).distance_to([20.0, 0.0]) <= f64::EPSILON)
            .map(|coord| coord.depth)
            .expect("nearby subject point must remain covered");
        assert_eq!(
            nearby_depth, qt.max_depth,
            "the guaranteed camera near field must cover nearby visible subjects"
        );
        let forward_depth = cover
            .iter()
            .find(|&&coord| qt.region(coord).distance_to([80.0, 0.0]) <= f64::EPSILON)
            .map(|coord| coord.depth)
            .expect("forward point must remain covered");
        assert!(
            forward_depth < qt.max_depth,
            "a sky view must not force the distant point to finest detail"
        );
    }

    #[test]
    fn bootstrap_waits_for_the_entire_camera_near_field() {
        let qt = test_qt();
        let selected = QuadCoord::ROOT
            .children()
            .into_iter()
            .map(|coord| Selected {
                coord,
                region: qt.region(coord),
                morph_start: f64::INFINITY,
                morph_end: f64::INFINITY,
            })
            .collect::<Vec<_>>();
        let demand = TerrainVisualDemand {
            camera: Entity::PLACEHOLDER,
            focus: [0.0, 0.0],
            camera_y: 3.0,
            heading: None,
            view_direction: DVec3::Y,
            screen_height_px: 1080.0,
            selection_clip: SelectionClip::new(
                test_perspective(std::f64::consts::FRAC_PI_4, DVec3::Y * 3.0, DVec3::Y),
                1080.0,
                2.0,
                DVec3::Y * 3.0,
            )
            .unwrap(),
            projection: test_perspective(std::f64::consts::FRAC_PI_4, DVec3::Y * 3.0, DVec3::Y),
            required: true,
            near_detail_radius_m: 30.0,
            near_detail_hysteresis_m: 12.0,
        };
        let missing = selected[0].coord;

        assert!(
            !bootstrap_cover_is_ready(&selected, &[demand], |coord| coord != missing),
            "one unprepared neighbouring tile must keep the opening cover stable"
        );
        assert!(
            bootstrap_cover_is_ready(&selected, &[demand], |_| true),
            "the cover may adapt once every near-camera tile is prepared"
        );
    }

    #[test]
    fn selected_cover_status_requires_current_render_ready_leaves() {
        let cover = QuadCoord::ROOT
            .children()
            .into_iter()
            .collect::<HashSet<_>>();
        let mut tiles = LodView {
            cover: cover.clone(),
            cover_selected: cover
                .iter()
                .map(|&coord| Selected {
                    coord,
                    region: Square {
                        center: [0.0, 0.0],
                        half: 1.0,
                    },
                    morph_start: 0.0,
                    morph_end: 1.0,
                })
                .collect(),
            ..Default::default()
        };
        for coord in &cover {
            tiles.tiles.insert(
                *coord,
                TileSlot {
                    resolution: 3,
                    half_extent: 1.0,
                    entity: Entity::PLACEHOLDER,
                    r#gen: 7,
                    morph_end: 0.0,
                    drawn: true,
                    ready: true,
                    stitch_edges: [0.0; 4],
                },
            );
        }
        let not_bound = *cover.iter().next().expect("cover has four leaves");
        tiles.tiles.get_mut(&not_bound).unwrap().ready = false;
        let stale = *cover.iter().nth(1).expect("cover has four leaves");
        tiles.tiles.get_mut(&stale).unwrap().r#gen = 6;

        let status = selected_cover_status(&tiles, 7);
        assert_eq!(status.wanted, 4);
        assert_eq!(status.resident, 3, "the stale leaf still covers the ground");
        assert!(
            !status.complete,
            "an unbound cache hit and a stale-generation leaf must keep the streamer active"
        );

        tiles.tiles.get_mut(&not_bound).unwrap().ready = true;
        tiles.tiles.get_mut(&stale).unwrap().r#gen = 7;
        let status = selected_cover_status(&tiles, 7);
        assert_eq!(status.resident, status.wanted);
        assert!(status.complete);
    }

    #[test]
    fn repeated_same_frame_node_geometry_cache_miss_then_hits_do_not_overlap_borrows() {
        let coord = QuadCoord::ROOT;
        let mut storage = HashMap::new();
        let cache = std::cell::RefCell::new(&mut storage);
        let measurements = std::cell::Cell::new(0usize);

        for _ in 0..16 {
            let error = cached_node_geometry(&cache, coord, || {
                measurements.set(measurements.get() + 1);
                NodeGeometry {
                    error: 42.0,
                    height_range: [-5.0, 10.0],
                }
            });
            assert_eq!(
                error,
                NodeGeometry {
                    error: 42.0,
                    height_range: [-5.0, 10.0]
                }
            );
        }

        assert_eq!(
            measurements.get(),
            1,
            "one miss must insert once; repeated refinement passes must be cache hits"
        );
    }

    /// The cover must remain an exact, disjoint REPLACE cover of the root after any
    /// number of evolution steps: every point covered by exactly one node.
    fn assert_exact_cover(cover: &HashSet<QuadCoord>) {
        assert!(!cover.is_empty(), "cover went empty");
        // No node may be an ancestor of another (disjointness).
        for a in cover.iter() {
            let mut p = a.parent();
            while let Some(n) = p {
                assert!(
                    !cover.contains(&n),
                    "{a:?} lies under {n:?} — cover overlaps"
                );
                p = n.parent();
            }
        }
        // Total area must equal the root's: sum of 4^-depth == 1.
        let area: f64 = cover.iter().map(|c| 0.25f64.powi(c.depth as i32)).sum();
        assert!(
            (area - 1.0).abs() < 1e-9,
            "cover area {area} != 1 — holes or overlaps"
        );
    }

    /// Driving the camera around must never break the cover, and must never exceed
    /// the tile budget. This is the invariant the old global budget fit enforced by
    /// re-deriving everything; here it has to survive incremental edits.
    #[test]
    fn evolving_cover_stays_exact_and_within_budget() {
        let qt = test_qt();
        let err = |_c: QuadCoord, _r: Square| 120.0f64;
        let budget = 40;
        let mut cover = HashSet::new();

        // Sweep the focus across the terrain, then back out to a distance.
        for step in 0..60 {
            let x = -900.0 + (step as f64) * 30.0;
            evolve_cover(&qt, &mut cover, [x, 0.0], 2.0, &err, budget);
            assert_exact_cover(&cover);
            assert!(
                cover.len() <= budget,
                "cover {} exceeded budget {budget}",
                cover.len()
            );
        }
        for step in 0..30 {
            let h = 100.0 + (step as f64) * 400.0;
            evolve_cover(&qt, &mut cover, [0.0, 0.0], h, &err, budget);
            assert_exact_cover(&cover);
            assert!(cover.len() <= budget);
        }
    }

    /// Do two cover nodes share a boundary SEGMENT (not merely a corner)? Normalised
    /// `[0,1]` extents are exact powers of two, so the touch/overlap tests are exact
    /// — no epsilon.
    fn edge_adjacent(a: QuadCoord, b: QuadCoord) -> bool {
        let ext = |c: QuadCoord| {
            let s = 0.5f64.powi(c.depth as i32);
            (
                c.x as f64 * s,
                c.x as f64 * s + s,
                c.z as f64 * s,
                c.z as f64 * s + s,
            )
        };
        let (ax0, ax1, az0, az1) = ext(a);
        let (bx0, bx1, bz0, bz1) = ext(b);
        let touch_x = ax1 == bx0 || bx1 == ax0;
        let touch_z = az1 == bz0 || bz1 == az0;
        let over_x = ax0 < bx1 && bx0 < ax1;
        let over_z = az0 < bz1 && bz0 < az1;
        (touch_x && over_z) || (touch_z && over_x)
    }

    /// Worst neighbour depth gap in a cover, with the offending pair.
    fn worst_depth_gap(cover: &HashSet<QuadCoord>) -> (u32, Option<(QuadCoord, QuadCoord)>) {
        let v: Vec<QuadCoord> = cover.iter().copied().collect();
        let mut worst = 0u32;
        let mut pair = None;
        for i in 0..v.len() {
            for j in (i + 1)..v.len() {
                if !edge_adjacent(v[i], v[j]) {
                    continue;
                }
                let gap = (v[i].depth as i32 - v[j].depth as i32).unsigned_abs();
                if gap > worst {
                    worst = gap;
                    pair = Some((v[i], v[j]));
                }
            }
        }
        (worst, pair)
    }

    /// CONTROL for [`evolving_cover_stays_restricted`]: the checker itself must
    /// accept a hand-built restricted cover and flag a hand-built 2-level seam,
    /// or a pass over `evolve_cover` output proves nothing.
    #[test]
    fn the_restriction_checker_detects_seams() {
        let d1 = |x, z| QuadCoord { depth: 1, x, z };
        // (1,0,0) replaced by its four children: every edge crosses ≤ 1 level.
        let mut restricted: HashSet<QuadCoord> =
            [d1(1, 0), d1(0, 1), d1(1, 1)].into_iter().collect();
        restricted.extend((0..2).flat_map(|x| (0..2).map(move |z| QuadCoord { depth: 2, x, z })));
        let (gap, pair) = worst_depth_gap(&restricted);
        assert!(
            gap <= 1,
            "checker rejects a restricted cover: gap {gap} at {pair:?}"
        );
        // (1,0,0) replaced by its SIXTEEN depth-3 descendants: they touch the
        // depth-1 siblings directly — a 2-level seam the checker must flag.
        let mut seamed: HashSet<QuadCoord> = [d1(1, 0), d1(0, 1), d1(1, 1)].into_iter().collect();
        seamed.extend((0..4).flat_map(|x| (0..4).map(move |z| QuadCoord { depth: 3, x, z })));
        let (gap, _) = worst_depth_gap(&seamed);
        assert_eq!(gap, 2, "checker missed a 2-level seam");
    }

    /// THE CDLOD RESTRICTION: edge-adjacent selected nodes must differ by at most one
    /// level, because a single morph band is all that blends them — a 2-level jump has
    /// no band wide enough and the seam becomes a hard edge (visible as a staircase
    /// where sub-DEM detail switches on).
    ///
    /// `evolve_cover` enforces this by construction — guarded merges + cascade
    /// splits, from a trivially-restricted `{ROOT}` start (see its docs). This test
    /// is the proof it stays that way under a camera sweep; it FAILED (gap 2 at
    /// step 4) before the guards existed.
    #[test]
    fn evolving_cover_stays_restricted() {
        let qt = test_qt();
        // Depth-decaying error — see the control test for why a constant one is
        // degenerate here.
        let err = |c: QuadCoord, _r: Square| 120.0f64 * 0.5f64.powi(c.depth as i32);
        let budget = 40;
        let mut cover = HashSet::new();

        for step in 0..60 {
            let x = -900.0 + (step as f64) * 30.0;
            evolve_cover(&qt, &mut cover, [x, 0.0], 2.0, &err, budget);
            let (gap, pair) = worst_depth_gap(&cover);
            assert!(
                gap <= 1,
                "step {step}: neighbour depth gap {gap} > 1 at {pair:?}"
            );
        }
        for step in 0..30 {
            let h = 100.0 + (step as f64) * 400.0;
            evolve_cover(&qt, &mut cover, [0.0, 0.0], h, &err, budget);
            let (gap, pair) = worst_depth_gap(&cover);
            assert!(
                gap <= 1,
                "height step {step}: neighbour depth gap {gap} > 1 at {pair:?}"
            );
        }
    }

    /// A STATIONARY camera must reach a fixed point and then stop editing. The old
    /// fit could not: it alternated `wanted` 349 <-> 532 every frame forever, which
    /// is what re-cut hundreds of tiles per frame and read as jitter.
    #[test]
    fn a_still_camera_reaches_a_fixed_point() {
        let qt = test_qt();
        let err = |_c: QuadCoord, _r: Square| 120.0f64;
        let mut cover = HashSet::new();
        for _ in 0..200 {
            evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, 64);
        }
        let settled = cover.clone();
        for _ in 0..50 {
            evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, 64);
            assert_eq!(
                cover, settled,
                "cover still churning on a stationary camera — no fixed point"
            );
        }
    }

    #[test]
    fn retired_tile_survives_two_render_extractions_before_despawn() {
        let mut app = App::new();
        app.add_systems(Update, retire_terrain_tiles);
        let tile = app
            .world_mut()
            .spawn((Visibility::Hidden, RetiringTerrainTile { updates_left: 2 }))
            .id();

        app.update();
        app.update();
        assert!(app.world().get_entity(tile).is_ok());

        app.update();
        assert!(app.world().get_entity(tile).is_err());
    }

    /// The refusal counter is starvation, not fill progress: any budget that
    /// covers the metric's want reports zero at the fixed point, any budget
    /// below it reports the refused splits.
    #[test]
    fn budget_starvation_is_reported() {
        let qt = test_qt();
        let err = |_c: QuadCoord, _r: Square| 120.0f64;
        let settle = |budget: usize| {
            let mut cover = HashSet::new();
            for _ in 0..200 {
                evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, budget);
            }
            (
                evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, budget),
                cover,
            )
        };
        let (refused, unconstrained) = settle(usize::MAX);
        assert_eq!(
            refused, 0,
            "fixed point within budget must not report refusals"
        );
        let want = unconstrained.len();
        let (refused, _) = settle(want);
        assert_eq!(refused, 0, "budget == want must not report refusals");
        for budget in [want / 2, want / 8, 16] {
            let (refused, cover) = settle(budget);
            assert!(
                refused > 0,
                "budget {budget} < want {want} (cover {}) reached a fixed point \
                 without reporting refusals",
                cover.len()
            );
        }
    }

    /// Lowering the budget must be absorbed by merging, not by blowing past it.
    #[test]
    fn shrinking_the_budget_merges_down() {
        let qt = test_qt();
        let err = |_c: QuadCoord, _r: Square| 120.0f64;
        let mut cover = HashSet::new();
        for _ in 0..200 {
            evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, 256);
        }
        assert!(cover.len() > 16, "expected a refined cover to shrink from");
        for _ in 0..50 {
            evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, 16);
        }
        assert_exact_cover(&cover);
        assert!(
            cover.len() <= 16,
            "cover {} did not shrink to budget 16",
            cover.len()
        );
    }

    /// The cover evolution must be a pure function of its inputs — reusing the
    /// scratch collections across calls (the per-frame hoist) must not leak
    /// state between passes or terrains.
    #[test]
    fn cover_scratch_reuse_matches_fresh_scratch() {
        let qt = test_qt();
        let err = |c: QuadCoord, _r: Square| 120.0f64 * 0.5f64.powi(c.depth as i32);
        let mut reused = CoverScratch::default();
        let mut a = HashSet::new();
        let mut b = HashSet::new();
        for step in 0..40 {
            let x = -900.0 + (step as f64) * 45.0;
            let ra = evolve_cover_for_foci_with_retention(
                &qt,
                &mut a,
                &[([x, 0.0], 2.0)],
                &err,
                &|_, _| [0.0, 0.0],
                64,
                TEST_MAX_COVER_EDITS,
                TEST_LOD_HYSTERESIS_RATIO,
                &|_, _| false,
                &mut reused,
            );
            let rb = evolve_cover_for_foci_with_retention(
                &qt,
                &mut b,
                &[([x, 0.0], 2.0)],
                &err,
                &|_, _| [0.0, 0.0],
                64,
                TEST_MAX_COVER_EDITS,
                TEST_LOD_HYSTERESIS_RATIO,
                &|_, _| false,
                &mut CoverScratch::default(),
            );
            assert_eq!(a, b, "step {step}: scratch reuse changed the cover");
            assert_eq!(
                ra, rb,
                "step {step}: scratch reuse changed the returned counts"
            );
        }
    }

    /// Zero returned edits ⇔ the cover is at its fixed point for these inputs —
    /// the contract `LodView::settled_sig` latches on, so a budget-starved
    /// terrain stops re-ranking its refused split chains every frame while
    /// still reporting the starvation.
    #[test]
    fn zero_edits_marks_the_fixed_point_even_when_budget_refused() {
        let qt = test_qt();
        let err = |_c: QuadCoord, _r: Square| 120.0f64;
        let mut scratch = CoverScratch::default();
        let mut cover = HashSet::new();
        let mut step = |cover: &mut HashSet<QuadCoord>| {
            evolve_cover_for_foci_with_retention(
                &qt,
                cover,
                &[([0.0, 0.0], 5.0)],
                &err,
                &|_, _| [0.0, 0.0],
                16,
                TEST_MAX_COVER_EDITS,
                TEST_LOD_HYSTERESIS_RATIO,
                &|_, _| false,
                &mut scratch,
            )
        };
        let mut last = (usize::MAX, 0);
        for _ in 0..200 {
            last = step(&mut cover);
        }
        assert_eq!(last.0, 0, "a settled cover must report zero edits");
        assert!(
            last.1 > 0,
            "budget 16 starves this metric — refusals must still be reported"
        );
        let settled = cover.clone();
        step(&mut cover);
        assert_eq!(
            cover, settled,
            "zero edits must mean the cover did not change"
        );
    }

    // ── mesh cache (LRU + resident-derived cap) ──────────────────────────────

    #[test]
    fn drawn_bounds_cover_fine_parent_and_stitch_hulls_and_reject_nonfinite_postings() {
        let fine = [[-2.0, 1.0, -3.0], [4.0, 5.0, 6.0]];
        let parent = [[-4.0, 8.0, 0.0], [0.0, -7.0, 9.0]];
        assert_eq!(
            terrain_draw_bounds(&fine, &parent),
            Some(TerrainDrawBounds {
                min: DVec3::new(-4.0, -7.0, -3.0),
                max: DVec3::new(4.0, 8.0, 9.0),
                parent_displacement: DVec3::new(4.0, 12.0, 3.0),
            })
        );
        assert_eq!(terrain_draw_bounds(&[], &[]), None);
        assert_eq!(terrain_draw_bounds(&fine, &[[f32::NAN; 3]; 2]), None);
        assert_eq!(terrain_draw_bounds(&fine, &parent[..1]), None);
    }

    #[test]
    fn mesh_byte_estimate_includes_every_streamed_vertex_attribute_and_index() {
        let res = 3;
        let vertices = res * res;
        let mut mesh = grid_mesh(
            vec![[0.0; 3]; vertices],
            vec![[0.0, 1.0, 0.0]; vertices],
            vec![[0.0; 2]; vertices],
            vec![0; (res - 1) * (res - 1) * 6],
            bevy::asset::RenderAssetUsages::default(),
        );
        mesh.insert_attribute(ATTRIBUTE_MORPH_TARGET, vec![[0.0; 3]; vertices]);
        mesh.insert_attribute(ATTRIBUTE_MORPH_NORMAL, vec![[0.0, 1.0, 0.0]; vertices]);
        mesh.insert_attribute(ATTRIBUTE_MORPH_EDGE, vec![[0.0; 4]; vertices]);
        assert_eq!(
            tile_mesh_bytes(res),
            mesh.get_vertex_buffer_size() + mesh.get_index_buffer_bytes().unwrap().len()
        );
        assert_eq!(measured_mesh_bytes(&mesh), tile_mesh_bytes(res));
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0; 3]; vertices + 1]);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; vertices + 1]);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0; 2]; vertices + 1]);
        mesh.insert_attribute(ATTRIBUTE_MORPH_TARGET, vec![[0.0; 3]; vertices + 1]);
        mesh.insert_attribute(ATTRIBUTE_MORPH_NORMAL, vec![[0.0, 1.0, 0.0]; vertices + 1]);
        mesh.insert_attribute(ATTRIBUTE_MORPH_EDGE, vec![[0.0; 4]; vertices + 1]);
        assert_eq!(
            measured_mesh_bytes(&mesh),
            tile_mesh_bytes(res) + 72,
            "boundary remesh vertices participate in admission even beyond the regular lattice"
        );
    }

    #[test]
    fn mesh_cache_entry_cap_covers_the_resident_bound() {
        // T2: the cap must exceed one terrain's worst-case residents (cover +
        // parent fallbacks + coarse stand-ins) or the trim evicts the trailing
        // edge every frame — the exact failure the flat 1024 cap had at the
        // default 768 budget.
        let budget = 768;
        assert!(mesh_cache_entry_cap(budget, 1) > 2 * budget + coarse_fallback_tile_count());
        assert!(
            mesh_cache_entry_cap(budget, 1) > 1024,
            "must exceed the old flat cap"
        );
        assert_eq!(
            mesh_cache_entry_cap(budget, 3),
            3 * mesh_cache_entry_cap(budget, 1),
            "cap scales with the live terrain count"
        );
    }

    #[test]
    fn mesh_cache_trims_lru_first_and_never_evicts_protected() {
        let mut cache = LodMeshCache::default();
        let mut assets = Assets::<Mesh>::default();
        let terrain = Entity::PLACEHOLDER;
        let tile_res = terrain_quality().terrain_lod_tile_resolution;
        let key = |x: u32| (terrain, c(3, x, 0), tile_res);
        let mut handles = Vec::new();
        for x in 0..4 {
            let handle = assets.add(Mesh::new(
                bevy::mesh::PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::default(),
            ));
            handles.push(handle.clone());
            cache.insert(key(x), handle, 0.0, None, tile_mesh_bytes(tile_res));
        }
        cache.begin_frame(HashSet::from([handles[3].id()]));
        // Touch 0 so 1 becomes the oldest unprotected entry.
        assert!(cache.get(&key(0)).is_some());
        // Trim to 2 entries; 3 stands in for the current frame's cover.
        cache.trim(2, u64::MAX);
        assert_eq!(cache.map.len(), 2);
        assert!(
            cache.map.contains_key(&key(3)),
            "a cover-needed entry must survive any trim"
        );
        assert!(
            cache.map.contains_key(&key(0)),
            "the most recently touched unprotected entry survives (LRU order)"
        );
        assert_eq!(
            cache.accounted_bytes(),
            cache
                .map
                .values()
                .map(|entry| entry.bytes as u64)
                .sum::<u64>()
        );
        // The byte ceiling alone also forces eviction.
        cache.begin_frame(HashSet::from([handles[3].id()]));
        cache.trim(usize::MAX, (tile_mesh_bytes(tile_res)) as u64);
        assert_eq!(cache.map.len(), 1);
        assert!(cache.map.contains_key(&key(3)));
    }

    #[test]
    fn mesh_admission_keeps_invalidated_live_generations_and_counts_shared_handles_once() {
        let mut assets = Assets::<Mesh>::default();
        let mesh = |assets: &mut Assets<Mesh>| {
            assets.add(Mesh::new(
                bevy::mesh::PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::default(),
            ))
        };
        let old = mesh(&mut assets);
        let fresh = mesh(&mut assets);
        let mut cache = LodMeshCache::default();
        let source = Entity::PLACEHOLDER;
        let key = (source, QuadCoord::ROOT, 49);
        let bytes = (tile_mesh_bytes(49)) as u64;
        cache.insert(key, old.clone(), 0.0, None, bytes as usize);
        cache.begin_frame(HashSet::from([old.id()]));
        cache.drop_region(source, None, 10.0);
        assert_eq!(
            cache.accounted_bytes(),
            bytes,
            "invalidation cannot erase resident ownership"
        );
        assert!(!cache.admit_upload(bytes, bytes));
        assert!(cache.admit_upload(bytes, bytes * 2));
        cache.insert(key, fresh.clone(), 0.0, None, bytes as usize);
        cache.insert(
            (source, c(1, 0, 0), 49),
            fresh.clone(),
            0.0,
            None,
            bytes as usize,
        );
        assert_eq!(
            cache.accounted_bytes(),
            bytes * 2,
            "shared views retain one mesh allocation"
        );
        cache.begin_frame(HashSet::from([fresh.id()]));
        assert_eq!(
            cache.accounted_bytes(),
            bytes,
            "retired handles leave accounting only after all live geometry releases them"
        );
        assert!(!cache.admit_upload(bytes, bytes));
        cache.begin_frame(HashSet::new());
        assert!(cache.admit_upload(bytes, bytes));
        assert!(
            cache.map.is_empty(),
            "unreferenced cached aliases are evicted for admission"
        );
    }

    #[test]
    fn upload_limit_preserves_all_sources_and_newly_published_handles() {
        let mut assets = Assets::<Mesh>::default();
        let mut cache = LodMeshCache::default();
        let bytes = (tile_mesh_bytes(49)) as u64;
        let mut ids = HashSet::new();
        for source in [Entity::from_bits(1), Entity::from_bits(2)] {
            let handle = assets.add(Mesh::new(
                bevy::mesh::PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::default(),
            ));
            ids.insert(handle.id());
            cache.insert(
                (source, QuadCoord::ROOT, 49),
                handle,
                0.0,
                None,
                bytes as usize,
            );
        }
        assert!(
            !cache.admit_upload(bytes, bytes * 2),
            "deferred tile spawns already own their admitted handles"
        );
        cache.begin_frame(ids);
        assert!(
            !cache.admit_upload(bytes, bytes),
            "a lower cap cannot evict another source's residents"
        );
        assert_eq!(cache.map.len(), 2);
        assert!(
            !cache.admit_upload(bytes * 3, bytes * 2),
            "an oversized cinematic tile remains an unsatisfied request"
        );
        assert_eq!(cache.map.len(), 2);
    }

    #[test]
    fn drawn_surface_revision_tracks_readiness_and_refuses_exhaustion() {
        let camera = Entity::from_bits(1);
        let mut tiles = LodTiles::default();
        let mut view = LodView::default();
        assert_eq!(view.draw_revision, 0);
        view.insert_slot(
            QuadCoord::ROOT,
            TileSlot {
                resolution: 3,
                half_extent: 1.0,
                entity: camera,
                r#gen: 0,
                morph_end: 10.0,
                ready: true,
                drawn: true,
                stitch_edges: [0.0; 4],
            },
        );
        let inserted = view.draw_revision;
        assert!(view.invalidate_material_readiness(camera));
        assert!(view.draw_revision > inserted);
        let invalidated = view.draw_revision;
        assert!(!view.invalidate_material_readiness(camera));
        assert_eq!(
            view.draw_revision, invalidated,
            "unchanged unavailable material does not wake identical collision sweeps"
        );
        view.draw_revision = u64::MAX;
        view.bump_draw_revision();
        tiles.views.insert(camera, view);
        assert_eq!(tiles.draw_revision(camera), None);
    }

    #[test]
    fn budget_pressure_sheds_far_cover_before_a_retained_quad() {
        let qt = test_qt();
        let retained = c(1, 0, 0);
        let farthest = c(1, 1, 1);
        let mut cover = (0..4)
            .flat_map(|z| (0..4).map(move |x| c(2, x, z)))
            .collect::<HashSet<_>>();
        // Keep all quads inside the refine band so this exercises the forced,
        // not voluntary, merge path.
        let err = |_c: QuadCoord, _r: Square| 1_000_000.0f64;

        evolve_cover_for_foci_with_retention(
            &qt,
            &mut cover,
            &[([-700.0, -700.0], 5.0)],
            &err,
            &|_, _| [0.0, 0.0],
            13,
            TEST_MAX_COVER_EDITS,
            TEST_LOD_HYSTERESIS_RATIO,
            &|coord, _| coord == retained,
            &mut CoverScratch::default(),
        );

        assert!(
            retained
                .children()
                .iter()
                .all(|child| cover.contains(child)),
            "a retained near-camera quad must survive while another merge can satisfy the budget"
        );
        assert!(
            cover.len() <= 13,
            "non-retained quads should absorb the budget cut"
        );
        assert!(cover.contains(&farthest));
        assert!(
            farthest
                .children()
                .iter()
                .all(|child| !cover.contains(child)),
            "the farthest eligible quad must merge before nearer non-retained cover"
        );
    }

    #[test]
    fn mixed_depths_stay_disjoint() {
        // A realistic cover: deep near-field, shallower further out, one deep node unready.
        let deep = c(3, 0, 0);
        let sel = vec![deep, c(3, 1, 0), c(2, 1, 0), c(2, 0, 1)];
        let (mut draw, mut scratch) = (HashSet::new(), Vec::new());
        build_draw_partition(
            sel.iter().copied(),
            // Residency as it actually stands mid-stream: nothing BELOW the
            // selection is resident (deep's children were never baked). Without
            // the depth cap the predicate marked those children drawable, which
            // legitimately routes deep through the just-coarsened child-cover
            // bridge (tested in `unready_coarsened_parent_keeps_its_ready_-
            // outgoing_children`) — a full cover, but not the ancestor stand-in
            // this test is about, and one the ancestor-walk helper can't count.
            |n| n != deep && (1..=3).contains(&n.depth),
            &mut draw,
            &mut scratch,
        );
        assert_covered_exactly_once(&sel, &draw);
        // Disjointness: no drawn node may be an ancestor of another drawn node.
        for a in draw.iter() {
            let mut p = a.parent();
            while let Some(n) = p {
                assert!(!draw.contains(&n), "{a:?} drawn under drawn ancestor {n:?}");
                p = n.parent();
            }
        }
    }
}
