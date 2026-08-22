//! Built-in **rocks** layer: scatters faceted boulders ON the DEM surface (static
//! drivable obstacles, LOD-culled), ground height resolved from the composed
//! surface oracle (so rocks sit correctly in/around analytic craters and edits).

use std::sync::Arc;

use avian3d::prelude::{Collider, RigidBody};
#[cfg(not(target_arch = "wasm32"))]
use bevy::camera::visibility::VisibilityRange;
use bevy::prelude::*;
use lunco_obstacle_field::rock::faceted_rock_mesh;
use lunco_obstacle_field::sampler::{salt, sample_layer, Placement};
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

/// The ONE boulder look every rock — procedural or hand-placed — draws with.
/// Exposed boulders are BRIGHTER than mature regolith (~0.2 vs ~0.12 albedo — fresh
/// rock faces vs gardened dust). Near-black rocks with no cast shadow were literally
/// invisible inside shadowed crater bowls ("invisible wall").
///
/// It is a `PbrLook` — appearance INTENT, not a material — so this crate names no
/// material at all. `lunco-render-bevy` caches by `PbrLook::key()`, which means the
/// thousands of rocks still resolve to ONE `StandardMaterial` and one bind group
/// (the batching this scatter depends on), except that it can no longer be lost by
/// forgetting to thread a shared handle through the loop.
fn rock_look() -> lunco_render::PbrLook {
    lunco_render::PbrLook {
        base_color: Color::srgb(0.19, 0.19, 0.20).into(),
        perceptual_roughness: 1.0,
        // Hundreds-to-thousands of scattered rocks: casting each into all 4 sun
        // cascades every frame is a big chunk of the shadow pass. They still
        // RECEIVE shadows; skip casting (their own tiny contact shadow isn't worth
        // 4× re-submission of the whole field).
        no_shadow_cast: true,
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

/// Scatters faceted boulders bounded to a near-field region around the origin.
struct RockScatterLayer {
    rocks: RockLayer,
    region_half_extent: f32,
    pattern: Pattern,
    seed: u64,
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
        let half = self.region_half_extent.min(oracle.half_extent());
        if half <= 0.0 {
            return;
        }
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
        for p in placements.iter() {
            let y =
                lunco_terrain_core::HeightSource::height_at(oracle, p.pos.x as f64, p.pos.y as f64)
                    as f32;
            let r_vis = bucket_radius(bucket_of(p.size)).max(0.05);
            #[cfg(not(target_arch = "wasm32"))]
            let entity = if let Some(entity) = cx.rock_pool.pop() {
                reused += 1;
                entity
            } else {
                spawned += 1;
                cx.commands.spawn_empty().id()
            };
            #[cfg(target_arch = "wasm32")]
            let entity = {
                spawned += 1;
                cx.commands.spawn_empty().id()
            };
            let mut rock = cx.commands.entity(entity);
            // Reassert ownership on reuse as well as on first spawn. Presentation
            // and selection are allowed to reparent entities; a pooled rock must
            // always return to the terrain that owns its local X/Z coordinates.
            rock.try_insert(ChildOf(cx.terrain));
            rock.try_insert((
                TerrainRock,
                ProceduralRock,
                TerrainScatterEntity,
                TerrainScatterOwner(cx.terrain),
                Name::new("TerrainRock"),
                // Procedural scatter, re-spawned as the field restreams — runtime
                // detail, not authored content. (The *placed* rock below is
                // authored and stays visible.)
                lunco_core::SystemManaged,
                Transform::from_xyz(p.pos.x, y - r_vis * 0.25, p.pos.y)
                    .with_rotation(Quat::from_rotation_y(p.yaw)),
                Visibility::Inherited,
                RigidBody::Static,
                Collider::sphere((r_vis * 0.6) as f64),
            ));
            if let Some(handles) = &bucket_handles {
                // `no_shadow_cast` rides on the look — `lunco-render-bevy` inserts
                // `NotShadowCaster` for it. Cloning the look does NOT clone a
                // material: every clone keys to the same cached one.
                rock.try_insert((Mesh3d(handles[bucket_of(p.size)].clone()), look.clone()));
                // Native visibility range is a culling optimization. Web keeps the
                // authored population and uses the explicit instance cap instead.
                #[cfg(not(target_arch = "wasm32"))]
                rock.try_insert(rock_visibility_range(
                    cx.quality.terrain_rock_lod_start_distance,
                    cx.quality.terrain_rock_lod_fade_distance,
                ));
            }
        }

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
/// the near-field `region_half_extent`.
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
/// (near-field scatter half-extent), and `seed`.
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
    let region_half_extent = a.get_f32("regionM").unwrap_or(300.0);
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
}
