//! Built-in **rocks** layer: scatters faceted boulders ON the DEM surface (static
//! drivable obstacles, LOD-culled), ground height resolved from the composed
//! surface oracle (so rocks sit correctly in/around analytic craters and edits).

use std::sync::Arc;

use avian3d::prelude::{Collider, RigidBody};
#[cfg(not(target_arch = "wasm32"))]
use bevy::camera::visibility::VisibilityRange;
use bevy::prelude::*;
use lunco_obstacle_field::rock::faceted_rock_mesh;
use lunco_obstacle_field::sampler::{salt, sample_layer};
use lunco_obstacle_field::spec::{Pattern, RockLayer, SizeDist};

use super::{LayerAttrSource, LayerScatterCx, SharedRockAssets, TerrainLayer, TerrainScatterEntity};

/// One scattered rock (kept distinct from [`TerrainScatterEntity`] for selection).
#[derive(Component)]
pub struct TerrainRock;

/// Number of size buckets → shared rock meshes (so N rocks reuse a few meshes).
const ROCK_BUCKETS: usize = 6;
/// Hard cap on scattered rock ENTITIES regardless of density × area. Scattering at a
/// real per-hectare density across a 16 km DEM would be hundreds of thousands of
/// bodies; cap the total and spread it over the whole map (LOD-culled near the
/// camera). Camera-following rock streaming is the real fix for uniform full-map
/// density at high counts; this gives full-map coverage cheaply meanwhile.
const MAX_ROCKS: usize = 6000;
/// Distance LOD: rocks fully visible to `LOD_FAR`, cross-fade out over `LOD_FADE`.
/// Rocks are scattered only in a near-origin region (`region_half_extent`, ~±300 m),
/// and unlike craters they have NO coarse always-on fallback — once culled they
/// just vanish. So the cull distance must comfortably exceed the region (else
/// backing away from origin drops every rock). The scatter is bounded + the meshes
/// are size-bucketed/shared, so keeping all of them visible is cheap. Camera-following
/// rock streaming (rocks around wherever you are) is the real fix for full-map coverage.
#[cfg(not(target_arch = "wasm32"))]
const LOD_FAR: f32 = 2500.0;
#[cfg(not(target_arch = "wasm32"))]
const LOD_FADE: f32 = 500.0;

// Distance-LOD cross-fade for rocks, via bevy's `VisibilityRange`. NATIVE ONLY:
// `VisibilityRange` drives the `visibility_ranges` view binding (group 0,
// binding 14). Its WebGL2 uniform-buffer fallback in bevy 0.18 declares a
// `min_binding_size` (16) smaller than the shader's `array<vec4, N>` (1024),
// so `create_render_pipeline` rejects EVERY `pbr_opaque_mesh_pipeline` → all
// meshes drop each frame → black viewport. The rock count is hard-capped
// (`MAX_ROCKS`) and meshes are size-bucketed/shared, so on web we skip the
// distance cull and keep all rocks visible — cheap, and avoids the bad binding.
#[cfg(not(target_arch = "wasm32"))]
fn rock_visibility_range() -> VisibilityRange {
    VisibilityRange {
        start_margin: 0.0..0.0,
        end_margin: LOD_FAR..(LOD_FAR + LOD_FADE),
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
) -> Handle<Mesh> {
    rocks
        .meshes
        .entry(bucket)
        .or_insert_with(|| {
            let r = bucket_radius_of(bucket);
            meshes.add(faceted_rock_mesh(0xB0 ^ bucket as u64, 4, r.max(0.05)))
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

impl TerrainLayer for RockScatterLayer {
    fn id(&self) -> &'static str {
        "rocks"
    }

    fn scatter_fingerprint(&self) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        // Debug covers every nested field (RockLayer / SizeDist / Pattern), so a
        // future param can't be silently missed. Runtime-only — never persisted.
        format!("{:?}|{:?}|{}|{}", self.rocks, self.pattern, self.region_half_extent, self.seed)
            .hash(&mut h);
        Some(h.finish())
    }
    fn scatter(&self, cx: &mut LayerScatterCx) {
        // WEB: scatter no rocks at all. Each rock is a distinct ECS entity with a
        // Static sphere Collider, and on WebGL the `VisibilityRange` distance cull
        // is unavailable (it breaks the PBR pipeline binding — see
        // `rock_visibility_range`), so all of them render + sit in the avian
        // broadphase every frame on the single wasm thread. Dropping the whole
        // field is the biggest steady-state win for the browser; native keeps rocks
        // (it has the distance cull + worker threads).
        #[cfg(target_arch = "wasm32")]
        {
            let _ = cx;
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
        let oracle = cx.oracle;
        let half = self.region_half_extent.min(oracle.half_extent());
        if half <= 0.0 {
            return;
        }
        let side = (2.0 * half) as f64;
        let count = ((self.rocks.density as f64 * side * side) / 10_000.0).round().max(0.0) as usize;
        let count = count.min(MAX_ROCKS);
        if count == 0 {
            return;
        }
        if count == MAX_ROCKS {
            info!(
                "[terrain-layer/rocks] capping scatter at {MAX_ROCKS} rocks over ±{:.0} m \
                 (requested density {}/ha would be more); LOD-culled, full-map coverage",
                half, self.rocks.density
            );
        }

        let placements = sample_layer(
            self.seed,
            salt::ROCKS,
            self.pattern,
            half,
            count,
            self.rocks.size,
            self.rocks.dynamic_fraction,
        );

        let size = self.rocks.size;
        let span = (size.max - size.min).max(1e-3);

        // Build shared visual meshes per size bucket (client only). Done BEFORE the
        // spawn loop so the `cx.meshes` borrow is released before `cx.commands` is.
        let bucket_handles: Option<Vec<Handle<Mesh>>> = cx.meshes.as_deref_mut().map(|meshes| {
            (0..ROCK_BUCKETS)
                .map(|b| {
                    let r = size.min + span * (b as f32 / (ROCK_BUCKETS - 1) as f32);
                    meshes.add(faceted_rock_mesh(self.seed ^ (0xB0 + b as u64), 4, r.max(0.05)))
                })
                .collect()
        });
        // ONE boulder look for every rock in the world (see `rock_look`); the binder's
        // key cache turns it into ONE material + ONE bind group.
        let look = rock_look();

        let bucket_of = |sz: f32| -> usize {
            let t = ((sz - size.min) / span).clamp(0.0, 1.0);
            ((t * (ROCK_BUCKETS - 1) as f32).round() as usize).min(ROCK_BUCKETS - 1)
        };
        // The VISUAL a rock gets is its bucket's shared mesh — extent ~0.5–0.7 of
        // the bucket radius (`faceted_rock_mesh` boxes: half-extents ≤ 0.48·r,
        // offsets ≤ 0.4·r) — NOT `p.size`. Size collider + sink from the same
        // bucket radius (derivable headless → identical colliders on the server)
        // or the wheel stops on an invisible shell up to a metre before the
        // visible rock: THE "rover hits an invisible wall" report. 0.6·r sunk
        // 0.25·r keeps the collider inside the visual mass.
        let bucket_radius =
            |b: usize| -> f32 { size.min + span * (b as f32 / (ROCK_BUCKETS - 1) as f32) };

        let mut spawned = 0usize;
        cx.commands.entity(cx.terrain).with_children(|parent| {
            for p in &placements {
                let y = lunco_terrain_core::HeightSource::height_at(
                    oracle,
                    p.pos.x as f64,
                    p.pos.y as f64,
                ) as f32;
                let r_vis = bucket_radius(bucket_of(p.size)).max(0.05);
                let mut rock = parent.spawn((
                    TerrainRock,
                    TerrainScatterEntity,
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
                    // Distance LOD cull — native only (see `rock_visibility_range`).
                    #[cfg(not(target_arch = "wasm32"))]
                    rock.try_insert(rock_visibility_range());
                }
                spawned += 1;
            }
        });

        info!(
            "[terrain-layer/rocks] scattered {spawned} rock(s) (±{:.0} m region, density {}/ha)",
            half, self.rocks.density
        );
        }
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
    Arc::new(RockScatterLayer { rocks, region_half_extent, pattern, seed })
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
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.position[0].to_bits(), self.position[1].to_bits(), self.size.to_bits(), self.seed)
            .hash(&mut h);
        Some(h.finish())
    }
    fn scatter(&self, cx: &mut LayerScatterCx) {
        let oracle = cx.oracle;
        let y = lunco_terrain_core::HeightSource::height_at(
            oracle,
            self.position[0],
            self.position[1],
        ) as f32;
        // SHARED assets: a placed rock used to mint a fresh `Mesh` AND a fresh
        // `StandardMaterial` — one permanent extra draw call + bind group per
        // `PlaceRock`. It now draws the shared boulder look (→ one cached material)
        // and its size bucket's shared mesh, exactly like the procedural scatter. Its
        // radius snaps to the bucket so collider, sink and visual all agree.
        let bucket = size_bucket(self.size);
        let r = bucket_radius_of(bucket).max(0.05);
        let rock_assets = &mut *cx.rock_assets;
        let mesh = cx
            .meshes
            .as_deref_mut()
            .map(|meshes| shared_rock_mesh(rock_assets, meshes, bucket));
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
                Name::new("TerrainRock (placed)"),
                Transform::from_xyz(self.position[0] as f32, y - r * 0.25, self.position[1] as f32)
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
    Arc::new(RockInstanceLayer { position, size, seed })
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
            assert!(err < 0.07, "radius {r} → bucket radius {q} ({:.1}% off)", err * 100.0);
        }
        // Near-equal rocks land in the SAME bucket → they share one mesh.
        assert_eq!(size_bucket(0.60), size_bucket(0.62));
        // Genuinely different sizes do not.
        assert_ne!(size_bucket(0.6), size_bucket(2.0));
    }
}

/// Parse a `lunco:layer = "rocks"` prim: `density` (per ha, required > 0), `sizeMode`
/// (modal radius m), `sizeMin`/`sizeMax` (radius band m), `dynamicFrac`,
/// `regionM` (near-field scatter half-extent), `seed`.
pub(super) fn parse_rock_layer(a: &dyn LayerAttrSource) -> Option<Arc<dyn TerrainLayer>> {
    let density = a.get_f32("density").unwrap_or(0.0);
    if density <= 0.0 {
        return None;
    }
    let mode = a.get_f32("sizeMode").unwrap_or(0.6);
    let size_min = a.get_f32("sizeMin").unwrap_or(0.2);
    let size_max = a.get_f32("sizeMax").unwrap_or((mode * 4.0).max(2.5));
    let rocks = RockLayer {
        enabled: true,
        density,
        // min ≤ mode ≤ max — same validity guard as the Inspector sliders.
        size: SizeDist::new(size_min.min(mode), mode, size_max.max(mode), 0.6),
        dynamic_fraction: a.get_f32("dynamicFrac").unwrap_or(0.0),
    };
    Some(Arc::new(RockScatterLayer {
        rocks,
        region_half_extent: a.get_f32("regionM").unwrap_or(300.0),
        pattern: Pattern::Uniform,
        seed: a.get_i64("seed").map(|s| s as u64).unwrap_or(0xB0A1),
    }))
}
