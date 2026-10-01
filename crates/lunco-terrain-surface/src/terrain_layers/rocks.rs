//! Built-in **rocks** layer: scatters faceted boulders ON the DEM surface (static
//! drivable obstacles, LOD-culled), ground height resolved from the composed
//! surface oracle (so rocks sit correctly in/around analytic craters and edits).

use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    sync::Arc,
};

use avian3d::prelude::{Collider, RigidBody};
#[cfg(not(target_arch = "wasm32"))]
use bevy::camera::visibility::VisibilityRange;
use bevy::prelude::*;
use lunco_obstacle_field::rock::faceted_rock_mesh;
use lunco_obstacle_field::sampler::{Placement, salt, sample_layer};
use lunco_obstacle_field::spec::{Pattern, RockLayer, SizeDist};

use super::{
    LayerAttrSource, LayerScatterCx, SharedRockAssets, TerrainLayer, TerrainScatterEntity,
    TerrainScatterOwner,
};

/// One scattered rock (kept distinct from [`TerrainScatterEntity`] for selection).
#[derive(Component)]
pub struct TerrainRock;

/// Marks a rock whose entity is owned by the procedural scatterer and may be
/// recycled on the next refresh. Hand-placed rocks intentionally do not carry
/// this marker: their identity belongs to the authored layer instance.
#[derive(Component)]
pub(crate) struct ProceduralRock;

#[derive(Bundle)]
struct ProceduralRockBundle {
    terrain_rock: TerrainRock,
    procedural_rock: ProceduralRock,
    scatter_entity: TerrainScatterEntity,
    scatter_owner: TerrainScatterOwner,
    child_of: ChildOf,
    name: Name,
    system_managed: lunco_core::SystemManaged,
    transform: Transform,
    visibility: Visibility,
    rigid_body: RigidBody,
    collider: Collider,
}

#[derive(Bundle)]
struct ProceduralRockVisuals {
    mesh: Mesh3d,
    look: lunco_render::PbrLook,
    visibility: Visibility,
    #[cfg(not(target_arch = "wasm32"))]
    visibility_range: VisibilityRange,
}

struct PendingRockBody {
    terrain: Entity,
    existing: Option<Entity>,
    body: ProceduralRockBundle,
    visuals: Option<ProceduralRockVisuals>,
}

struct PendingRockVisual {
    terrain: Entity,
    entity: Entity,
    visuals: ProceduralRockVisuals,
}

#[derive(Default)]
struct TerrainAdmissionCounts {
    bodies: usize,
    visuals: usize,
}

const MAX_ROCK_BODIES_PER_UPDATE: usize = 8;
const MAX_ROCK_VISUAL_INSERTIONS_PER_UPDATE: usize = 64;

/// Ordered, bounded admission for generated rock bodies and their visuals.
#[derive(Resource, Default)]
pub(crate) struct PendingTerrainRockAdmission {
    bodies: VecDeque<PendingRockBody>,
    visuals: VecDeque<PendingRockVisual>,
    counts: HashMap<Entity, TerrainAdmissionCounts>,
    fingerprints: HashMap<Entity, u64>,
}

impl PendingTerrainRockAdmission {
    fn enqueue_bodies(&mut self, bodies: Vec<PendingRockBody>) {
        self.bodies.reserve(bodies.len());
        for body in bodies {
            let counts = self.counts.entry(body.terrain).or_default();
            counts.bodies += 1;
            if body.visuals.is_some() {
                counts.visuals += 1;
            }
            self.bodies.push_back(body);
        }
    }

    pub(super) fn register_terrain(&mut self, terrain: Entity, fingerprint: u64) {
        self.fingerprints.insert(terrain, fingerprint);
    }

    pub(super) fn has_terrain_work(&self, terrain: Entity) -> bool {
        self.counts
            .get(&terrain)
            .is_some_and(|counts| counts.bodies > 0 || counts.visuals > 0)
    }

    pub(super) fn cancel_terrain(&mut self, terrain: Entity) {
        self.bodies.retain(|body| body.terrain != terrain);
        self.visuals.retain(|visual| visual.terrain != terrain);
        self.counts.remove(&terrain);
        self.fingerprints.remove(&terrain);
    }

    fn take_body_batch(&mut self) -> Vec<PendingRockBody> {
        let count = self.bodies.len().min(MAX_ROCK_BODIES_PER_UPDATE);
        let mut batch = Vec::with_capacity(count);
        for _ in 0..count {
            let Some(body) = self.bodies.pop_front() else {
                break;
            };
            batch.push(body);
        }
        batch
    }

    fn take_visual_batch(&mut self) -> Vec<PendingRockVisual> {
        let count = self
            .visuals
            .len()
            .min(MAX_ROCK_VISUAL_INSERTIONS_PER_UPDATE);
        self.visuals.drain(..count).collect()
    }

    fn complete_body(&mut self, terrain: Entity) {
        if let Some(counts) = self.counts.get_mut(&terrain) {
            if counts.bodies == 0 {
                warn!("terrain rock body admission completed with no pending body for {terrain}");
            } else {
                counts.bodies -= 1;
            }
        } else {
            warn!("terrain rock body admission completed for untracked terrain {terrain}");
        }
    }

    fn complete_visual(&mut self, terrain: Entity) {
        if let Some(counts) = self.counts.get_mut(&terrain) {
            if counts.visuals == 0 {
                warn!(
                    "terrain rock visual admission completed with no pending visual for {terrain}"
                );
            } else {
                counts.visuals -= 1;
            }
        } else {
            warn!("terrain rock visual admission completed for untracked terrain {terrain}");
        }
    }

    fn take_completed(&mut self, terrains: impl IntoIterator<Item = Entity>) -> Vec<(Entity, u64)> {
        let mut seen = BTreeSet::new();
        let mut completed = Vec::new();
        for terrain in terrains {
            if !seen.insert(terrain) || self.has_terrain_work(terrain) {
                continue;
            }
            self.counts.remove(&terrain);
            if let Some(fingerprint) = self.fingerprints.remove(&terrain) {
                completed.push((terrain, fingerprint));
            }
        }
        completed
    }
}

/// Admit bounded body and render batches in stable queue order while the current
/// simulation continues.
pub(crate) fn admit_pending_terrain_rocks(
    mut commands: Commands,
    mut pending: ResMut<PendingTerrainRockAdmission>,
) {
    let bodies = pending.take_body_batch();
    if !bodies.is_empty() {
        commands.queue(ApplyRockBodyBatch { bodies });
    }
    let visuals = pending.take_visual_batch();
    if !visuals.is_empty() {
        commands.queue(ApplyRockVisualBatch { visuals });
    }
}

pub(crate) fn clear_pending_terrain_rocks(mut pending: ResMut<PendingTerrainRockAdmission>) {
    *pending = PendingTerrainRockAdmission::default();
}

struct ApplyRockBodyBatch {
    bodies: Vec<PendingRockBody>,
}

impl bevy::ecs::system::Command for ApplyRockBodyBatch {
    type Out = ();

    fn apply(self, world: &mut World) {
        let capacity = self.bodies.len();
        let mut spawn_bodies = Vec::with_capacity(capacity);
        let mut spawn_visuals = Vec::with_capacity(capacity);
        let mut update_bodies = Vec::with_capacity(capacity);
        let mut queued_visuals = Vec::with_capacity(capacity);
        let mut completed_bodies = Vec::with_capacity(capacity);
        let mut skipped_visuals = Vec::with_capacity(capacity);
        let mut touched_terrains = Vec::with_capacity(capacity);
        let mut canceled_terrains = BTreeSet::new();

        for pending in self.bodies {
            touched_terrains.push(pending.terrain);
            if world.get_entity(pending.terrain).is_err() {
                canceled_terrains.insert(pending.terrain);
                continue;
            }
            if let Some(entity) = pending.existing {
                if world.get_entity(entity).is_ok() {
                    update_bodies.push((entity, pending.body));
                    completed_bodies.push(pending.terrain);
                    if let Some(visuals) = pending.visuals {
                        queued_visuals.push(PendingRockVisual {
                            terrain: pending.terrain,
                            entity,
                            visuals,
                        });
                    }
                } else {
                    completed_bodies.push(pending.terrain);
                    if pending.visuals.is_some() {
                        skipped_visuals.push(pending.terrain);
                    }
                }
            } else {
                spawn_bodies.push(pending.body);
                spawn_visuals.push((pending.terrain, pending.visuals));
                completed_bodies.push(pending.terrain);
            }
        }

        let spawned = bevy::log::info_span!("terrain_rock_spawn_batch", count = spawn_bodies.len())
            .in_scope(|| world.spawn_batch(spawn_bodies).collect::<Vec<_>>());
        for ((terrain, visuals), entity) in spawn_visuals.into_iter().zip(spawned) {
            if let Some(visuals) = visuals {
                queued_visuals.push(PendingRockVisual {
                    terrain,
                    entity,
                    visuals,
                });
            }
        }
        if !update_bodies.is_empty() {
            if let Err(error) =
                bevy::log::info_span!("terrain_rock_update_batch", count = update_bodies.len())
                    .in_scope(|| world.try_insert_batch(update_bodies))
            {
                warn!(
                    count = error.entities.len(),
                    "terrain rock body batch referenced entities that were already removed"
                );
            }
        }

        let completed =
            if let Some(mut pending) = world.get_resource_mut::<PendingTerrainRockAdmission>() {
                for terrain in canceled_terrains {
                    pending.cancel_terrain(terrain);
                }
                pending.visuals.extend(queued_visuals);
                for terrain in completed_bodies {
                    pending.complete_body(terrain);
                }
                for terrain in skipped_visuals {
                    pending.complete_visual(terrain);
                }
                pending.take_completed(touched_terrains)
            } else {
                Vec::new()
            };
        for (terrain, fingerprint) in completed {
            if let Ok(mut entity) = world.get_entity_mut(terrain) {
                entity.insert((
                    super::TerrainLayersApplied,
                    super::ScatteredContent(fingerprint),
                ));
                entity.remove::<super::TerrainLayersPending>();
            }
        }
    }
}

struct ApplyRockVisualBatch {
    visuals: Vec<PendingRockVisual>,
}

impl bevy::ecs::system::Command for ApplyRockVisualBatch {
    type Out = ();

    fn apply(self, world: &mut World) {
        let mut insertions = Vec::with_capacity(self.visuals.len());
        let mut terrains = Vec::with_capacity(self.visuals.len());
        for pending in self.visuals {
            terrains.push(pending.terrain);
            if world.get_entity(pending.terrain).is_ok() && world.get_entity(pending.entity).is_ok()
            {
                insertions.push((pending.entity, pending.visuals));
            }
        }
        if !insertions.is_empty() {
            if let Err(error) =
                bevy::log::info_span!("terrain_rock_visual_insert_batch", count = insertions.len())
                    .in_scope(|| world.try_insert_batch(insertions))
            {
                warn!(
                    count = error.entities.len(),
                    "terrain rock visual batch referenced entities that were already removed"
                );
            }
        }
        if let Some(mut pending) = world.get_resource_mut::<PendingTerrainRockAdmission>() {
            for terrain in &terrains {
                pending.complete_visual(*terrain);
            }
            let completed = pending.take_completed(terrains);
            for (terrain, fingerprint) in completed {
                if let Ok(mut entity) = world.get_entity_mut(terrain) {
                    entity.insert((
                        super::TerrainLayersApplied,
                        super::ScatteredContent(fingerprint),
                    ));
                    entity.remove::<super::TerrainLayersPending>();
                }
            }
        }
    }
}

/// Bound the in-memory placement cache while still covering normal inspector
/// tuning. The cache stores only XZ/size/yaw data, never ECS entities or meshes.
const MAX_CACHED_ROCK_FIELDS: usize = 32;
/// Bump when the deterministic sampler or placement interpretation changes.
const ROCK_SCATTER_CACHE_VERSION: u64 = 1;
// Distance-LOD cross-fade for rocks, via bevy's `VisibilityRange`. Native only:
// WebGL2 does not provide the same visibility-range binding contract, so the
// authored rock population remains visible there rather than being silently
// deleted as a quality fallback.
#[cfg(not(target_arch = "wasm32"))]
fn rock_visibility_range(start_distance: f32, fade_distance: f32) -> VisibilityRange {
    VisibilityRange {
        start_margin: 0.0..0.0,
        end_margin: start_distance..(start_distance + fade_distance),
        use_aabb: false,
    }
}

/// Quantise a boulder radius onto a shared-mesh bucket (~12% steps, so a bucket's
/// mesh is never visibly the wrong size). The bucket index IS the mesh cache key in
/// [`SharedRockAssets`], so any two rocks of near-equal size draw the same mesh.
fn size_bucket(r: f32) -> u32 {
    // Eighth-log steps, biased by +64 so sub-metre radii (ln < 0) stay positive.
    ((r.max(0.02).ln() * 8.0).round() + 64.0).clamp(0.0, 255.0) as u32
}

/// The representative radius of a bucket (the inverse of [`size_bucket`]).
fn bucket_radius_of(bucket: u32) -> f32 {
    ((bucket as f32 - 64.0) / 8.0).exp()
}

/// Shared boulder appearance uses linear reflectance, brighter than mature soil.
/// The render owner caches this `PbrLook` by key, so scatter and placed rocks
/// reuse one material and bind group while sharing the size-bucket meshes.
fn rock_look() -> lunco_render::PbrLook {
    lunco_render::PbrLook {
        base_color: Color::linear_rgb(0.19, 0.19, 0.20).into(),
        perceptual_roughness: 1.0,
        // Shared geometry and material keep the scatter batchable. Boulder
        // shadows provide the contact and scale cues on low-sun terrain.
        no_shadow_cast: false,
        ..Default::default()
    }
}

/// The shared boulder mesh for a size bucket (built once, then reused by every rock
/// in that bucket, on every terrain).
fn shared_rock_mesh(
    rocks: &mut SharedRockAssets,
    meshes: &mut Assets<Mesh>,
    bucket: u32,
    cube_count: usize,
) -> Handle<Mesh> {
    rocks
        .meshes
        .entry(bucket)
        .or_insert_with(|| {
            let r = bucket_radius_of(bucket);
            meshes.add(faceted_rock_mesh(
                0xB0 ^ bucket as u64,
                cube_count,
                r.max(0.05),
            ))
        })
        .clone()
}

/// Scatters faceted boulders over the composed terrain, or over an explicit
/// authored near-field region when `regionM` is positive.
struct RockScatterLayer {
    rocks: RockLayer,
    region_half_extent: f32,
    pattern: Pattern,
    seed: u64,
}

/// Resolve the authored scatter scope against the composed terrain footprint.
/// `regionM = 0` is the USD schema default and means the whole terrain; a
/// positive value is an explicit half-extent. Invalid authored values are
/// rejected at the scatter owner instead of silently becoming another scope.
fn resolve_region_half_extent(
    authored: f32,
    terrain_half_extent: f32,
) -> Result<f32, &'static str> {
    if !authored.is_finite() || authored < 0.0 {
        return Err("regionM must be finite and non-negative");
    }
    if !terrain_half_extent.is_finite() || terrain_half_extent <= 0.0 {
        return Err("the composed terrain extent is not finite and positive");
    }
    Ok(if authored == 0.0 {
        terrain_half_extent
    } else {
        authored.min(terrain_half_extent)
    })
}

fn hash_size_dist(h: &mut lunco_precompute::Fnv1a, size: SizeDist) {
    h.write_u64(size.min.to_bits() as u64);
    h.write_u64(size.mode.to_bits() as u64);
    h.write_u64(size.max.to_bits() as u64);
    h.write_u64(size.sigma.to_bits() as u64);
}

fn hash_pattern(h: &mut lunco_precompute::Fnv1a, pattern: Pattern) {
    match pattern {
        Pattern::Uniform => {
            h.write_u64(0);
        }
        Pattern::PoissonDisk { min_spacing } => {
            h.write_u64(1);
            h.write_u64(min_spacing.to_bits() as u64);
        }
        Pattern::Clustered { clusters, spread } => {
            h.write_u64(2);
            h.write_u64(clusters as u64);
            h.write_u64(spread.to_bits() as u64);
        }
    }
}

fn hash_rock_layer(h: &mut lunco_precompute::Fnv1a, rocks: RockLayer) {
    h.write_u64(rocks.enabled as u64);
    h.write_u64(rocks.density.to_bits() as u64);
    hash_size_dist(h, rocks.size);
    h.write_u64(rocks.dynamic_fraction.to_bits() as u64);
}

impl TerrainLayer for RockScatterLayer {
    fn id(&self) -> &'static str {
        "rocks"
    }

    fn scatter_fingerprint(&self) -> Option<u64> {
        let mut h = lunco_precompute::Fnv1a::new();
        h.write_u64(1); // fingerprint layout version
        hash_rock_layer(&mut h, self.rocks);
        hash_pattern(&mut h, self.pattern);
        h.write_u64(self.region_half_extent.to_bits() as u64);
        h.write_u64(self.seed);
        Some(h.finish())
    }
    fn scatter(&self, cx: &mut LayerScatterCx) {
        let oracle = cx.oracle;
        let half = match resolve_region_half_extent(self.region_half_extent, oracle.half_extent()) {
            Ok(half) => half,
            Err(reason) => {
                warn!(
                    "[terrain-layer/rocks] refusing scatter for invalid authored scope: {reason}"
                );
                return;
            }
        };
        let side = (2.0 * half) as f64;
        let requested_count = ((self.rocks.density as f64 * side * side) / 10_000.0)
            .round()
            .max(0.0) as usize;
        let count = requested_count.min(cx.quality.terrain_rock_max_instances);
        if count == 0 {
            return;
        }
        if count < requested_count {
            info!(
                "[terrain-layer/rocks] applying explicit Graphics cap of {} rocks over ±{:.0} m \
                 (requested density {}/ha would produce {requested_count})",
                cx.quality.terrain_rock_max_instances, half, self.rocks.density
            );
        }

        let mut cache_key = lunco_precompute::Fnv1a::new();
        cache_key.write_u64(ROCK_SCATTER_CACHE_VERSION);
        cache_key.write_u64(self.scatter_fingerprint().unwrap_or_default());
        cache_key.write_u64(half.to_bits() as u64);
        cache_key.write_u64(count as u64);
        let cache_key = cache_key.finish();
        let placements: Arc<[Placement]> =
            if let Some(cached) = cx.rock_assets.placements.get(&cache_key) {
                cached.clone()
            } else {
                let generated: Arc<[Placement]> = Arc::from(
                    sample_layer(
                        self.seed,
                        salt::ROCKS,
                        self.pattern,
                        half,
                        count,
                        self.rocks.size,
                        self.rocks.dynamic_fraction,
                    )
                    .into_boxed_slice(),
                );
                if cx.rock_assets.placements.len() >= MAX_CACHED_ROCK_FIELDS {
                    cx.rock_assets.placements.clear();
                }
                cx.rock_assets
                    .placements
                    .insert(cache_key, generated.clone());
                generated
            };

        let size = self.rocks.size;
        let span = (size.max - size.min).max(1e-3);
        let bucket_count = cx.quality.terrain_rock_mesh_buckets;

        // Build shared visual meshes per size bucket. Done BEFORE the spawn loop so the
        // `cx.meshes` borrow is released before `cx.commands` is.
        let rock_assets = &mut *cx.rock_assets;
        let bucket_handles: Option<Vec<Handle<Mesh>>> = cx.meshes.as_deref_mut().map(|meshes| {
            (0..bucket_count)
                .map(|b| {
                    let r = size.min + span * (b as f32 / (bucket_count - 1) as f32);
                    shared_rock_mesh(
                        rock_assets,
                        meshes,
                        size_bucket(r),
                        cx.quality.terrain_rock_mesh_cube_count,
                    )
                })
                .collect()
        });
        // ONE boulder look for every rock in the world (see `rock_look`); the binder's
        // key cache turns it into ONE material + ONE bind group.
        let look = rock_look();

        let bucket_of = |sz: f32| -> usize {
            let t = ((sz - size.min) / span).clamp(0.0, 1.0);
            ((t * (bucket_count - 1) as f32).round() as usize).min(bucket_count - 1)
        };
        // The VISUAL a rock gets is its bucket's shared mesh — extent ~0.5–0.7 of
        // the bucket radius (`faceted_rock_mesh` boxes: half-extents ≤ 0.48·r,
        // offsets ≤ 0.4·r) — NOT `p.size`. Size collider + sink from the same
        // bucket radius (derivable headless → identical colliders on the server)
        // or the wheel stops on an invisible shell up to a metre before the
        // visible rock: THE "rover hits an invisible wall" report. 0.6·r sunk
        // 0.25·r keeps the collider inside the visual mass.
        let bucket_radius = |b: usize| -> f32 {
            let r = size.min + span * (b as f32 / (bucket_count - 1) as f32);
            bucket_radius_of(size_bucket(r))
        };

        let mut reused = 0usize;
        let mut spawned = 0usize;
        let has_visuals = bucket_handles.is_some();
        let mut body_admissions = Vec::with_capacity(placements.len());
        for p in placements.iter() {
            let y =
                lunco_terrain_core::HeightSource::height_at(oracle, p.pos.x as f64, p.pos.y as f64)
                    as f32;
            let r_vis = bucket_radius(bucket_of(p.size)).max(0.05);
            #[cfg(not(target_arch = "wasm32"))]
            let recycled = cx.rock_pool.pop();
            #[cfg(not(target_arch = "wasm32"))]
            if recycled.is_some() {
                reused += 1;
            } else {
                spawned += 1;
            }
            #[cfg(target_arch = "wasm32")]
            let recycled: Option<Entity> = {
                spawned += 1;
                None
            };
            let bucket = bucket_of(p.size);
            let rock = ProceduralRockBundle {
                terrain_rock: TerrainRock,
                procedural_rock: ProceduralRock,
                scatter_entity: TerrainScatterEntity,
                scatter_owner: TerrainScatterOwner(cx.terrain),
                child_of: ChildOf(cx.terrain),
                name: Name::new("TerrainRock"),
                // Procedural scatter, re-spawned as the field restreams — runtime
                // detail, not authored content. (The *placed* rock below is
                // authored and stays visible.)
                system_managed: lunco_core::SystemManaged,
                transform: Transform::from_xyz(p.pos.x, y - r_vis * 0.25, p.pos.y)
                    .with_rotation(Quat::from_rotation_y(p.yaw)),
                visibility: if has_visuals {
                    Visibility::Hidden
                } else {
                    Visibility::Inherited
                },
                rigid_body: RigidBody::Static,
                collider: Collider::sphere((r_vis * 0.6) as f64),
            };
            let visuals = bucket_handles.as_ref().map(|handles| {
                ProceduralRockVisuals {
                    mesh: Mesh3d(handles[bucket].clone()),
                    // `no_shadow_cast` rides on the look — `lunco-render-bevy`
                    // inserts `NotShadowCaster` for it. Cloning the look does NOT
                    // clone a material: every clone keys to the same cached one.
                    look: look.clone(),
                    #[cfg(not(target_arch = "wasm32"))]
                    // Native visibility range is a culling optimization. Web keeps
                    // the authored population and uses the explicit instance cap.
                    visibility_range: rock_visibility_range(
                        cx.quality.terrain_rock_lod_start_distance,
                        cx.quality.terrain_rock_lod_fade_distance,
                    ),
                    visibility: Visibility::Inherited,
                }
            });
            body_admissions.push(PendingRockBody {
                terrain: cx.terrain,
                existing: recycled,
                body: rock,
                visuals,
            });
        }
        cx.pending_rock_admission.enqueue_bodies(body_admissions);

        debug!(
            "[terrain-layer/rocks] scattered {} rock(s), reused {reused}, spawned {spawned} \
             (±{:.0} m region, density {}/ha)",
            reused + spawned,
            half,
            self.rocks.density
        );
    }
}

/// Build a rock layer from a typed [`RockLayer`] (e.g. the Inspector's
/// `ObstacleFieldSpec.rocks`) so live tuning can rebuild the terrain's rock layer
/// directly — honouring density, full size distribution, scatter `pattern`, and
/// the explicit `region_half_extent` (`0` means the whole composed terrain).
pub fn rock_layer(
    rocks: RockLayer,
    region_half_extent: f32,
    pattern: Pattern,
    seed: u64,
) -> Arc<dyn TerrainLayer> {
    Arc::new(RockScatterLayer {
        rocks,
        region_half_extent,
        pattern,
        seed,
    })
}

/// One hand-placed boulder — its own layer prim, addressable/removable by its
/// [`LayerId`](super::LayerId) (= prim path when doc-backed). Unlike the
/// procedural field it is NOT skipped on web: a handful of placed rocks is
/// cheap everywhere.
struct RockInstanceLayer {
    /// Terrain-local XZ (metres).
    position: [f64; 2],
    /// Boulder radius (metres).
    size: f32,
    /// Shape/orientation seed (mesh facets + yaw).
    seed: u64,
}

impl TerrainLayer for RockInstanceLayer {
    fn id(&self) -> &'static str {
        "rock"
    }

    fn scatter_fingerprint(&self) -> Option<u64> {
        let mut h = lunco_precompute::Fnv1a::new();
        h.write_u64(1); // fingerprint layout version
        h.write_u64(self.position[0].to_bits());
        h.write_u64(self.position[1].to_bits());
        h.write_u64(self.size.to_bits() as u64);
        h.write_u64(self.seed);
        Some(h.finish())
    }
    fn scatter(&self, cx: &mut LayerScatterCx) {
        let oracle = cx.oracle;
        let y =
            lunco_terrain_core::HeightSource::height_at(oracle, self.position[0], self.position[1])
                as f32;
        // SHARED assets: a placed rock used to mint a fresh `Mesh` AND a fresh
        // `StandardMaterial` — one permanent extra draw call + bind group per
        // `PlaceRock`. It now draws the shared boulder look (→ one cached material)
        // and its size bucket's shared mesh, exactly like the procedural scatter. Its
        // radius snaps to the bucket so collider, sink and visual all agree.
        let bucket = size_bucket(self.size);
        let r = bucket_radius_of(bucket).max(0.05);
        let rock_assets = &mut *cx.rock_assets;
        let mesh = cx.meshes.as_deref_mut().map(|meshes| {
            shared_rock_mesh(
                rock_assets,
                meshes,
                bucket,
                cx.quality.terrain_rock_mesh_cube_count,
            )
        });
        let look = rock_look();
        // Deterministic yaw from the seed (golden-ratio hash → well spread). The
        // MESH is shared now, so the yaw is what keeps placed boulders from all
        // looking identically oriented.
        let yaw = (self.seed as f32 * 0.618_034).fract() * std::f32::consts::TAU;
        cx.commands.entity(cx.terrain).with_children(|parent| {
            // Same collider/sink derivation as the procedural field (0.6·r sphere
            // sunk 0.25·r) so a placed rock drives identically.
            let mut rock = parent.spawn((
                TerrainRock,
                TerrainScatterEntity,
                TerrainScatterOwner(cx.terrain),
                Name::new("TerrainRock (placed)"),
                Transform::from_xyz(
                    self.position[0] as f32,
                    y - r * 0.25,
                    self.position[1] as f32,
                )
                .with_rotation(Quat::from_rotation_y(yaw)),
                Visibility::Inherited,
                RigidBody::Static,
                Collider::sphere((r * 0.6) as f64),
            ));
            if let Some(mesh) = mesh {
                rock.try_insert((Mesh3d(mesh), look));
            }
        });
    }
}

/// Build a single-rock layer (the `PlaceRock` command's doc-free tier).
pub fn rock_instance_layer(position: [f64; 2], size: f32, seed: u64) -> Arc<dyn TerrainLayer> {
    Arc::new(RockInstanceLayer {
        position,
        size,
        seed,
    })
}

/// Parse a `lunco:layer = "rock"` prim — ONE hand-placed boulder: `x`/`z`
/// (terrain-local m, required), `size` (radius m), `seed`.
pub(super) fn parse_rock_instance(a: &dyn LayerAttrSource) -> Option<Arc<dyn TerrainLayer>> {
    let x = a.get_f32("x")?;
    let z = a.get_f32("z")?;
    Some(Arc::new(RockInstanceLayer {
        position: [x as f64, z as f64],
        size: a.get_f32("size").unwrap_or(0.6),
        seed: a.get_i64("seed").map(|s| s as u64).unwrap_or(0x0C1),
    }))
}

/// Parse a `lunco:layer = "rocks"` prim: `enabled` (explicit visibility,
/// defaulting to true), `density` (per ha, required > 0), `sizeMode` (modal
/// radius m), `sizeMin`/`sizeMax` (radius band m), `dynamicFrac`, `regionM`
/// (optional scatter half-extent; zero or omitted covers the whole composed
/// terrain), and `seed`.
pub(super) fn params(a: &dyn LayerAttrSource) -> (RockLayer, f32, u64) {
    // Visibility is independent from density. Keeping density authored makes a
    // disable/enable cycle survive a document reload and a new session.
    let density = a.get_f32("density").unwrap_or(0.0);
    let mode = a.get_f32("sizeMode").unwrap_or(0.6);
    let size_min = a.get_f32("sizeMin").unwrap_or(0.2);
    let size_max = a
        .get_f32("sizeMax")
        .unwrap_or_else(|| (mode * 4.0).max(2.5));
    let rocks = RockLayer {
        enabled: a.get_bool("enabled") != Some(false) && density > 0.0,
        density,
        // min ≤ mode ≤ max — same validity guard as the Inspector sliders.
        size: SizeDist::new(size_min.min(mode), mode, size_max.max(mode), 0.6),
        dynamic_fraction: a.get_f32("dynamicFrac").unwrap_or(0.0),
    };
    // `regionM` is optional in USD. Its schema fallback is zero, and zero means
    // the full composed terrain; an arbitrary near-field default would make a
    // layer appear concentrated even when the authored terrain is much larger.
    let region_half_extent = a.get_f32("regionM").unwrap_or(0.0);
    let seed = a.get_i64("seed").map(|s| s as u64).unwrap_or(0xB0A1);
    (rocks, region_half_extent, seed)
}

pub(super) fn parse_rock_layer(a: &dyn LayerAttrSource) -> Option<Arc<dyn TerrainLayer>> {
    let (rocks, region_half_extent, seed) = params(a);
    if !rocks.enabled {
        return None;
    }
    Some(Arc::new(RockScatterLayer {
        rocks,
        region_half_extent,
        pattern: Pattern::Uniform,
        seed,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R9: a placed rock draws its size BUCKET's shared mesh, so the bucket must
    /// track the requested radius closely (else a boulder visibly resizes) while
    /// still collapsing near-equal rocks onto one mesh (else there is no sharing).
    #[test]
    fn rock_size_buckets_are_tight_and_shared() {
        for r in [0.05f32, 0.2, 0.6, 1.0, 2.5, 5.0, 12.0] {
            let q = bucket_radius_of(size_bucket(r));
            let err = (q - r).abs() / r;
            assert!(
                err < 0.07,
                "radius {r} → bucket radius {q} ({:.1}% off)",
                err * 100.0
            );
        }
        // Near-equal rocks land in the SAME bucket → they share one mesh.
        assert_eq!(size_bucket(0.60), size_bucket(0.62));
        // Genuinely different sizes do not.
        assert_ne!(size_bucket(0.6), size_bucket(2.0));
    }

    #[test]
    fn region_scope_uses_the_composed_extent_when_unbounded() {
        assert_eq!(
            resolve_region_half_extent(0.0, 2_000.0),
            Ok(2_000.0),
            "the schema default must cover the composed terrain"
        );
        assert_eq!(
            resolve_region_half_extent(300.0, 2_000.0),
            Ok(300.0),
            "a positive regionM remains an explicit near-field scope"
        );
        assert!(resolve_region_half_extent(-1.0, 2_000.0).is_err());
        assert!(resolve_region_half_extent(f32::NAN, 2_000.0).is_err());
    }
}
