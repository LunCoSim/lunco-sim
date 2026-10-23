//! Body-demanded physical globe cover, independent of presentation residency.
//!
//! The geometry spine supplies the same f64 globe cutout and DEM collar to
//! native collision and rendering. New cover becomes live before old cover is
//! retired; preparation owns a distinct hold while native admission settles.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use avian3d::prelude::*;
use bevy::math::{DMat3, DQuat, DVec3};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};
use big_space::prelude::{CellCoord, Grid};
use lunco_core::{AppSceneExt, SceneOwned};
use lunco_environment::GravityBody;
use lunco_physics::{PhysicsHolds, PhysicsPoseSeeded};
use lunco_spatial::ActivePhysicsFrame;
use lunco_terrain_globe::quad_sphere::{
    balance_cube_sphere_lod, cube_to_sphere, subdivide_face, tile_center_uv,
};
use lunco_terrain_globe::{GlobeSurfaceGeometry, TileCoord, create_quadsphere_tile_geometry};

use crate::globe_lod::{GlobeHandoff, GlobeLod};

const HOLD: &str = "globe-collision-ready";

/// Geometry/preparation budget for native planetary surfaces.
#[derive(Resource, Reflect, Clone, Debug)]
#[reflect(Resource)]
pub struct GlobeCollisionSettings {
    /// Largest nominal globe tile next to a physical surface demand, metres.
    pub contact_tile_span_m: f64,
    /// Refine within this many tile arc lengths of a surface demand.
    pub refinement_distance_factor: f64,
    /// Surface subdivisions on each edge of a collision tile.
    pub subdivisions: u32,
    /// Maximum independent collider cooks admitted in one preparation update.
    pub cooks_per_update: usize,
    /// Time horizon for the additional velocity-projected surface demand.
    pub lookahead_seconds: f64,
    /// Minimum pre-contact preparation band above the physical surface, metres.
    pub preparation_height_m: f64,
}

impl Default for GlobeCollisionSettings {
    fn default() -> Self {
        Self {
            contact_tile_span_m: 256.0,
            refinement_distance_factor: 2.5,
            subdivisions: 16,
            cooks_per_update: 16,
            lookahead_seconds: 1.0,
            preparation_height_m: 128.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SurfaceKey {
    Tile {
        face: u8,
        level: u32,
        i: i32,
        j: i32,
    },
    Collar,
}

impl SurfaceKey {
    fn tile(tile: TileCoord) -> Self {
        Self::Tile {
            face: tile.face,
            level: tile.level,
            i: tile.i,
            j: tile.j,
        }
    }
    fn coord(self, body: Entity) -> Option<TileCoord> {
        match self {
            Self::Tile { face, level, i, j } => Some(TileCoord {
                body,
                face,
                level,
                i,
                j,
            }),
            Self::Collar => None,
        }
    }
}

struct PreparedSurface {
    collider: Collider,
    center: DVec3,
    rotation: DQuat,
}

#[derive(Default)]
struct BodyCover {
    handoff: Option<GlobeHandoff>,
    resident: BTreeMap<SurfaceKey, Option<Entity>>,
    pending: BTreeMap<SurfaceKey, Task<Result<Option<PreparedSurface>, String>>>,
    retired: Vec<Entity>,
    desired: BTreeSet<SurfaceKey>,
    last_foci: Vec<DVec3>,
    geometry_settings: Option<(u32, u64, u32)>,
    selection_factor: Option<u64>,
    failed: BTreeSet<SurfaceKey>,
    generation: u64,
}

#[derive(Resource, Default)]
struct GlobeCollisionState {
    covers: HashMap<Entity, BodyCover>,
}

impl lunco_core::SceneResource for GlobeCollisionState {}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<GlobeCollisionSettings>()
        .register_type::<GlobeCollisionSettings>()
        .init_scene_resource::<GlobeCollisionState>()
        .init_scene_resource::<PhysicsHolds>()
        .add_systems(lunco_core::SceneTeardown, clear_admission)
        .add_systems(
            PreUpdate,
            (
                crate::placement::sync_globe_handoffs,
                prepare_globe_collision,
            )
                .chain()
                .after(crate::CelestialEpochSet)
                .run_if(lunco_time::scene_time_ready),
        );
}

fn admission_key() -> lunco_core_runtime::SimulationProgressKey {
    lunco_core_runtime::SimulationProgressKey {
        owner: lunco_core_runtime::SimulationProgressOwner::GlobeColliderAdmission,
        operation_id: 0,
    }
}

fn clear_admission(mut progress: Option<ResMut<lunco_core_runtime::SimulationProgress>>) {
    if let Some(progress) = progress.as_mut() {
        progress.release(admission_key());
    }
}

fn cook_geometry(
    geometry: GlobeSurfaceGeometry,
    center: DVec3,
    rotation: DQuat,
) -> Result<Option<PreparedSurface>, String> {
    // An empty clipped tile is owned by the DEM/collar, not a missing cook.
    if geometry.indices.is_empty() {
        return Ok(None);
    }
    if geometry.positions.iter().any(|p| !p.is_finite())
        || !center.is_finite()
        || !rotation.is_finite()
    {
        return Err("planetary surface contains non-finite geometry".into());
    }
    let triangles = geometry
        .indices
        .chunks_exact(3)
        .map(|t| [t[0], t[1], t[2]])
        .collect();
    let collider = Collider::try_trimesh_with_config(
        geometry.positions,
        triangles,
        TrimeshFlags::FIX_INTERNAL_EDGES,
    )
    .map_err(|e| format!("native planetary surface cook failed: {e}"))?;
    if !lunco_physics::avian_backend_collider_shape_is_valid(&collider) {
        return Err("planetary surface cannot be admitted by the native backend".into());
    }
    Ok(Some(PreparedSurface {
        collider,
        center,
        rotation,
    }))
}

fn desired_cover(
    body: Entity,
    radius: f64,
    foci: &[DVec3],
    resident: &HashSet<TileCoord>,
    max_lod: u32,
    refinement_distance_factor: f64,
) -> BTreeSet<SurfaceKey> {
    let mut desired = HashSet::new();
    for face in 0..6 {
        subdivide_face(
            &mut desired,
            resident,
            body,
            face,
            0,
            0,
            0,
            foci,
            radius,
            max_lod,
            refinement_distance_factor,
            &[],
        );
    }
    balance_cube_sphere_lod(&mut desired, body, max_lod);
    desired.into_iter().map(SurfaceKey::tile).collect()
}

#[allow(clippy::type_complexity)]
fn prepare_globe_collision(
    mut commands: Commands,
    settings: Res<GlobeCollisionSettings>,
    mut state: ResMut<GlobeCollisionState>,
    mut holds: ResMut<PhysicsHolds>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut progress: Option<ResMut<lunco_core_runtime::SimulationProgress>>,
    active: Option<Res<ActivePhysicsFrame>>,
    bodies: Query<(
        Entity,
        &lunco_celestial::CelestialBody,
        &GlobeLod,
        Option<&GlobeHandoff>,
    )>,
    dynamic: Query<(
        &RigidBody,
        &Position,
        Option<&LinearVelocity>,
        &GravityBody,
        Option<&RigidBodyColliders>,
        Option<&PhysicsPoseSeeded>,
    )>,
    parents: Query<&ChildOf>,
    grids: Query<&Grid>,
    spatial: Query<(Option<&CellCoord>, &Transform)>,
    live: Query<&ColliderAabb>,
    sites: Query<&lunco_celestial::geo::GeodeticAnchor, With<lunco_celestial::geo::SiteAnchor>>,
    local_surfaces: Query<
        (),
        Or<(
            With<lunco_terrain_surface::DemTerrainRequest>,
            With<lunco_terrain_surface::DemHeightField>,
            With<lunco_terrain_surface::FlatSiteSurface>,
        )>,
    >,
) {
    let Some(active) = active else { return };
    let mut waiting = false;
    let mut budget = settings.cooks_per_update;
    let valid_settings = settings.contact_tile_span_m.is_finite()
        && settings.contact_tile_span_m > 0.0
        && settings.refinement_distance_factor.is_finite()
        && settings.refinement_distance_factor >= 1.0
        && (2..=64).contains(&settings.subdivisions)
        && budget > 0
        && settings.lookahead_seconds.is_finite()
        && settings.lookahead_seconds >= 0.0
        && settings.preparation_height_m.is_finite()
        && settings.preparation_height_m > 0.0;
    if !valid_settings {
        holds.set(HOLD, true);
        if let Some(faults) = faults.as_deref_mut() {
            faults.raise("globe-collision-settings-invalid", None, "planetary surface", "Planetary collision settings require positive span/budget/preparation height, refinement factor >= 1, 2..64 subdivisions and a finite non-negative lookahead");
        }
        return;
    }
    let current_bodies: HashSet<Entity> = bodies.iter().map(|(e, _, _, _)| e).collect();
    state.covers.retain(|entity, cover| {
        if current_bodies.contains(entity) {
            return true;
        }
        for entity in cover.resident.values().flatten().chain(&cover.retired) {
            commands.entity(*entity).try_despawn();
        }
        false
    });
    let mut ordered_bodies: Vec<_> = bodies.iter().collect();
    ordered_bodies
        .sort_by_key(|(entity, descriptor, _, _)| (descriptor.ephemeris_id, entity.to_bits()));
    for (body, descriptor, lod, handoff) in ordered_bodies {
        let relevant: Vec<_> = dynamic
            .iter()
            .filter(|(mode, _, _, gravity, _, _)| {
                **mode == RigidBody::Dynamic && gravity.body_entity == body
            })
            .collect();
        if relevant.is_empty() {
            continue;
        }
        // A declared local surface owns its datum and cutout. Do not expose a
        // mean-radius collider over below-datum DEM geometry while its shared
        // handoff is still preparing.
        if !local_surfaces.is_empty() && handoff.is_none() {
            let matching = match sites.single() {
                Ok(anchor) => anchor.body == descriptor.ephemeris_id,
                Err(_) => true,
            };
            if matching {
                waiting = true;
                continue;
            }
        }
        let Some((origin, rotation)) = lunco_spatial::coords::pose_in_grid(
            lod.surface_grid,
            active.0,
            &parents,
            &grids,
            &spatial,
        ) else {
            waiting = true;
            continue;
        };
        let radius = handoff.map_or(lod.radius_m, |h| h.radius_m);
        if !radius.is_finite() || radius <= 0.0 {
            waiting = true;
            holds.set(PhysicsHolds::SAFETY_FAILURE, true);
            if let Some(faults) = faults.as_deref_mut() {
                faults.raise(
                    "globe-collision-radius-invalid",
                    Some(body),
                    "planetary surface",
                    "Physical surface radius must be finite and positive",
                );
            }
            continue;
        }
        let max_lod = (radius * std::f64::consts::FRAC_PI_2 / settings.contact_tile_span_m)
            .log2()
            .ceil()
            .max(0.0);
        if !max_lod.is_finite() || max_lod > 24.0 {
            waiting = true;
            holds.set(PhysicsHolds::SAFETY_FAILURE, true);
            if let Some(faults) = faults.as_deref_mut() {
                faults.raise(
                    "globe-collision-resolution-invalid",
                    Some(body),
                    "planetary surface",
                    "Contact tile span requires more than 24 cube-sphere levels",
                );
            }
            continue;
        }
        let max_lod = max_lod as u32;
        let inverse = rotation.inverse();
        let mut foci = Vec::new();
        let mut contact_near = false;
        for (_, position, velocity, _, owned, seeded) in relevant {
            // Seeding may run later in this preparation schedule. Never read
            // Position's required-component default as a physical demand or
            // advance an unseeded body past missing surface support.
            if seeded.is_none() {
                waiting = true;
                continue;
            }
            let p = inverse * (position.0 - origin);
            if !p.is_finite() || p.length_squared() <= 0.0 {
                waiting = true;
                continue;
            }
            foci.push(p.normalize() * radius);
            let mut envelope = 0.0_f64;
            // A body without collider members has no contact envelope. For
            // actual members, every AABB must be admitted before its footprint
            // is known, including shapes still being attached to the body.
            let mut known = true;
            if let Some(owned) = owned {
                for collider in owned.iter() {
                    if let Ok(bounds) = live.get(collider)
                        && lunco_physics::avian_backend_aabb_is_valid(
                            bounds.min.as_dvec3(),
                            bounds.max.as_dvec3(),
                        )
                    {
                        envelope = envelope
                            .max(position.0.distance(bounds.min.as_dvec3()))
                            .max(position.0.distance(bounds.max.as_dvec3()));
                    } else {
                        known = false;
                    }
                }
            }
            let approach =
                velocity.map_or(0.0, |v| v.0.length() as f64) * settings.lookahead_seconds;
            contact_near |= !known
                || p.length() - radius - envelope <= settings.preparation_height_m + approach;
            if let Some(velocity) = velocity {
                let predicted = p + inverse * velocity.0.as_dvec3() * settings.lookahead_seconds;
                if predicted.is_finite() && predicted.length_squared() > 0.0 {
                    foci.push(predicted.normalize() * radius);
                }
            }
        }
        if foci.is_empty() {
            continue;
        }
        let cover = state.covers.entry(body).or_default();
        let geometry_settings = (settings.subdivisions, radius.to_bits(), max_lod);
        let changed =
            cover.handoff.as_ref() != handoff || cover.geometry_settings != Some(geometry_settings);
        if changed {
            cover.generation += 1;
            cover
                .retired
                .extend(cover.resident.values().flatten().copied());
            cover.resident.clear();
            cover.pending.clear();
            cover.last_foci.clear();
            cover.failed.clear();
            cover.handoff = handoff.cloned();
            cover.geometry_settings = Some(geometry_settings);
        }
        let moved = foci.len() != cover.last_foci.len()
            || foci.iter().zip(&cover.last_foci).any(|(a, b)| {
                a.distance_squared(*b) > (settings.contact_tile_span_m / 16.0).powi(2)
            });
        let selection_factor = settings.refinement_distance_factor.to_bits();
        if moved
            || changed
            || cover.desired.is_empty()
            || cover.selection_factor != Some(selection_factor)
        {
            let resident = cover
                .resident
                .keys()
                .filter_map(|key| key.coord(body))
                .collect();
            cover.desired = desired_cover(
                body,
                radius,
                &foci,
                &resident,
                max_lod,
                settings.refinement_distance_factor,
            );
            if handoff.is_some() {
                cover.desired.insert(SurfaceKey::Collar);
            }
            cover.last_foci = foci;
            cover.selection_factor = Some(selection_factor);
        }
        // Commit worker results in source-key order, independent of completion
        // timing. Native collider insertion order must not depend on worker races.
        let mut completed = Vec::new();
        while let Some((key, task)) = cover.pending.iter_mut().next() {
            let key = *key;
            let Some(result) = block_on(future::poll_once(task)) else {
                break;
            };
            cover.pending.remove(&key);
            completed.push((key, result));
        }
        for (key, result) in completed {
            match result {
                Ok(prepared) => {
                    let entity = prepared.map(|prepared| {
                        let grid = grids
                            .get(lod.surface_grid)
                            .expect("surface pose resolved through this live Grid");
                        let (cell, pose) = lunco_spatial::attach::local_pose_to_grid_storage(
                            grid,
                            prepared.center,
                            prepared.rotation,
                        );
                        commands
                            .spawn((
                                RigidBody::Static,
                                prepared.collider,
                                cell,
                                pose,
                                GlobalTransform::default(),
                                ChildOf(lod.surface_grid),
                                SceneOwned,
                                lunco_physics::PhysicsOrderKey(format!(
                                    "celestial:{}/surface:{}/{key:?}",
                                    descriptor.ephemeris_id, cover.generation
                                )),
                                Name::new(format!("Globe contact {body:?} {key:?}")),
                            ))
                            .id()
                    });
                    cover.resident.insert(key, entity);
                }
                Err(error) => {
                    cover.failed.insert(key);
                    waiting = true;
                    if let Some(faults) = faults.as_deref_mut() {
                        faults.raise(
                            "globe-collider-invalid",
                            Some(body),
                            "planetary surface",
                            error,
                        );
                    }
                    holds.set(PhysicsHolds::SAFETY_FAILURE, true);
                }
            }
        }
        for key in cover.desired.iter().copied() {
            if budget == 0 {
                break;
            }
            if cover.resident.contains_key(&key)
                || cover.pending.contains_key(&key)
                || cover.failed.contains(&key)
            {
                continue;
            }
            let cutout = handoff.map(GlobeHandoff::cutout);
            let res = settings.subdivisions;
            let task = match key {
                SurfaceKey::Tile { face, level, i, j } => {
                    let (u, v) = tile_center_uv(face, level, i, j);
                    let center = cube_to_sphere(face, u, v) * radius;
                    AsyncComputeTaskPool::get().spawn(async move {
                        cook_geometry(
                            create_quadsphere_tile_geometry(
                                body, face, level, i, j, radius, res, center, cutout,
                            ),
                            center,
                            DQuat::IDENTITY,
                        )
                    })
                }
                SurfaceKey::Collar => {
                    let handoff = handoff.expect("a collar key belongs to a prepared handoff");
                    let geometry = handoff.collar_geometry();
                    let center = handoff.dir * handoff.radius_m;
                    let rotation = DQuat::from_mat3(&DMat3::from_cols(
                        handoff.east,
                        handoff.dir,
                        -handoff.north,
                    ));
                    AsyncComputeTaskPool::get()
                        .spawn(async move { cook_geometry((*geometry).clone(), center, rotation) })
                }
            };
            cover.pending.insert(key, task);
            budget -= 1;
        }
        let complete = cover.desired.iter().all(|key| {
            cover.resident.get(key).is_some_and(|entity| {
                entity.is_none_or(|entity| {
                    live.get(entity).is_ok_and(|aabb| {
                        lunco_physics::avian_backend_aabb_is_valid(
                            aabb.min.as_dvec3(),
                            aabb.max.as_dvec3(),
                        )
                    })
                })
            })
        });
        waiting |= !complete && contact_near;
        if complete {
            for entity in cover.retired.drain(..) {
                commands.entity(entity).try_despawn();
            }
            cover.resident.retain(|key, entity| {
                if cover.desired.contains(key) {
                    return true;
                }
                if let Some(entity) = entity {
                    commands.entity(*entity).try_despawn();
                }
                false
            });
            cover.pending.retain(|key, _| cover.desired.contains(key));
        }
    }
    if holds.holds(HOLD) != waiting {
        holds.set(HOLD, waiting);
    }
    if let Some(progress) = progress.as_mut() {
        let key = admission_key();
        if waiting {
            let reason = "Preparing planetary ground collision support";
            progress.acquire(key, reason);
        } else {
            progress.release(key);
        }
    }
}
