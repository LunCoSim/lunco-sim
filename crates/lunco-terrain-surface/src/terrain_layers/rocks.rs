//! Built-in **rocks** layer: scatters faceted boulders ON the DEM surface (static
//! drivable obstacles, LOD-culled), ground height resolved from the composed
//! surface oracle (so rocks sit correctly in/around analytic craters and edits).

use std::sync::Arc;

use avian3d::prelude::{Collider, RigidBody};
use bevy::camera::visibility::VisibilityRange;
use bevy::prelude::*;
use lunco_obstacle_field::rock::faceted_rock_mesh;
use lunco_obstacle_field::sampler::{salt, sample_layer};
use lunco_obstacle_field::spec::{Pattern, RockLayer, SizeDist};

use super::{LayerAttrSource, LayerScatterCx, TerrainLayer, TerrainScatterEntity};

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
const LOD_FAR: f32 = 2500.0;
const LOD_FADE: f32 = 500.0;

fn rock_visibility_range() -> VisibilityRange {
    VisibilityRange {
        start_margin: 0.0..0.0,
        end_margin: LOD_FAR..(LOD_FAR + LOD_FADE),
        use_aabb: false,
    }
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
    fn scatter(&self, cx: &mut LayerScatterCx) {
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
        let rock_material = cx.materials.as_deref_mut().map(|materials| {
            materials.add(StandardMaterial {
                base_color: Color::srgb(0.10, 0.10, 0.11),
                perceptual_roughness: 1.0,
                ..default()
            })
        });

        let bucket_of = |sz: f32| -> usize {
            let t = ((sz - size.min) / span).clamp(0.0, 1.0);
            ((t * (ROCK_BUCKETS - 1) as f32).round() as usize).min(ROCK_BUCKETS - 1)
        };

        let mut spawned = 0usize;
        cx.commands.entity(cx.terrain).with_children(|parent| {
            for p in &placements {
                let y = lunco_terrain_core::HeightSource::height_at(
                    oracle,
                    p.pos.x as f64,
                    p.pos.y as f64,
                ) as f32;
                let mut rock = parent.spawn((
                    TerrainRock,
                    TerrainScatterEntity,
                    Name::new("TerrainRock"),
                    Transform::from_xyz(p.pos.x, y - p.size * 0.25, p.pos.y)
                        .with_rotation(Quat::from_rotation_y(p.yaw)),
                    Visibility::Inherited,
                    RigidBody::Static,
                    Collider::sphere((p.size * 0.8) as f64),
                ));
                if let (Some(handles), Some(mat)) = (&bucket_handles, &rock_material) {
                    rock.insert((
                        Mesh3d(handles[bucket_of(p.size)].clone()),
                        MeshMaterial3d(mat.clone()),
                        rock_visibility_range(),
                        // Hundreds-to-thousands of scattered rocks: casting each into all
                        // 4 sun cascades every frame is a big chunk of the shadow pass.
                        // They still RECEIVE shadows; skip casting (their own tiny
                        // contact shadow isn't worth 4× re-submission of the whole field).
                        bevy::light::NotShadowCaster,
                    ));
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

/// Parse a `lunco:layer = "rocks"` prim: `density` (per ha, required > 0), `sizeMode`
/// (modal radius m), `regionM` (near-field scatter half-extent), `seed`.
pub(super) fn parse_rock_layer(a: &dyn LayerAttrSource) -> Option<Arc<dyn TerrainLayer>> {
    let density = a.get_f32("density").unwrap_or(0.0);
    if density <= 0.0 {
        return None;
    }
    let mode = a.get_f32("sizeMode").unwrap_or(0.6);
    let rocks = RockLayer {
        enabled: true,
        density,
        size: SizeDist::new(0.2, mode, (mode * 4.0).max(2.5), 0.6),
        dynamic_fraction: 0.0,
    };
    Some(Arc::new(RockScatterLayer {
        rocks,
        region_half_extent: a.get_f32("regionM").unwrap_or(300.0),
        pattern: Pattern::Uniform,
        seed: a.get_i64("seed").map(|s| s as u64).unwrap_or(0xB0A1),
    }))
}
