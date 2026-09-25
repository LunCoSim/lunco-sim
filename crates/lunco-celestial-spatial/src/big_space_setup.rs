//! Sets up the big_space coordinate hierarchy for the solar system.
//!
//! ## Architecture: Rotating Grid + Body-fixed children
//!
//! **The GRID rotates. The Body does not.** `body_rotation_system`
//! (`systems.rs`) rotates only [`ReferenceFrame::BodyFixed`] grids, and the
//! frame identity lives on the **grids** — never on the body
//! entities, which sit at identity. Everything else in the crate is built on
//! that fact (`placement.rs` inverse-rotates inertial orbits INTO the grid;
//! `coords.rs`'s stored-chain test assumes a spinning grid), which is why it is
//! the grid that spins and not the body:
//!
//! 1. **Body Grid (ROTATING)** — carries `Grid` + `ReferenceFrame::BodyFixed`.
//!    Positioned by the ephemeris system, **and rotated** by
//!    `body_rotation_system` with the body's IAU rotation (`geo::body_rotation`).
//!    Its children are therefore **body-fixed**: terrain tiles, ground stations
//!    and surface ops inherit the spin for free, in high precision — which is
//!    exactly what big_space recommends ("place the planet and all objects on
//!    its surface in the same grid").
//!
//! 2. **Body Entity** — child of the Grid, identity transform. Carries
//!    `CelestialBody`, mesh, picking collider, and `GravityProvider`. SOI
//!    ownership comes from the centralized body catalog, not a second ECS
//!    component copy.
//!
//! 3. **Inertial Anchor** — a NON-rotating sibling grid tracking the body's
//!    position but not its spin ([`ReferenceFrame::EclipticJ2000`]). This is where a
//!    star-fixed observer belongs; see "Why an inertial anchor" below.
//!
//! ```text
//! BigSpace Root
//!   └── Solar Grid (inertial — the Sun does not spin here)
//!         ├── Sun (simple entity, no grid)
//!         ├── Sun Light
//!         ├── EMB Grid (inertial — a barycenter has no rotation model)
//!         │     ├── Earth Grid (ROTATING: ephemeris + IAU spin)
//!         │     │     ├── Earth Body (mesh+collider, identity transform)
//!         │     │     └── Earth Surface Grid (physical surface sub-frame)
//!         │     │           └── terrain + rovers + surface ops
//!         │     ├── Earth Inertial Anchor (position only, NO spin)
//!         │     │     └── Observer Camera  ← star-fixed
//!         │     └── Moon Grid (ROTATING: ephemeris + IAU spin)
//!         │           ├── Moon Body (mesh+collider, identity transform)
//!         │           └── Moon Surface Grid (physical surface sub-frame)
//!         │                 └── terrain + rovers + surface ops
//!         └── Other planets (simple entities)
//! ```
//!
//! Streamed globe tiles share each body's physical surface Grid with the site
//! scene hierarchy. Render-only marker copies keep a separate presentation
//! branch, but follow the same completed `WorldTime` epoch as physical bodies.
//!
//! ## Why an inertial anchor
//!
//! This doc block used to assert the exact opposite of the code — "Grid Anchor
//! (inertial) … does NOT rotate", "Body Entity (rotating)" — and the Observer
//! Camera was parented to the Earth Grid on the strength of that claim, with the
//! comment "(inertial) for orbit view". The grid spins, so **the orbit view was
//! not star-fixed**: the camera was dragged around Earth once per sidereal day,
//! a ~19,000 km circle. The fix is not to flip the code (the rest of the crate
//! correctly assumes rotating grids) — it is to give the camera a frame that
//! really is inertial.
//!
//! ## Surface sub-Grids
//!
//! Surface ops (rovers, avatars, terrain) live in a sub-Grid under each body's
//! rotating Grid. Its precision contract is the shared [`WorldGridConfig`], not
//! a separate surface-specific edge. This keeps every BigSpace branch on the
//! same deterministic cell/rebranch boundary while keeping Avian's `Position`
//! near zero in the rover's frame.
//!
//! ## Why two layers?
//!
//! - Orbit cameras stay on the Grid (star-fixed, no rotation) → `OrbitCamera`
//! - Surface cameras also stay on the Grid, but use `SurfaceCamera` which
//!   recomputes world-space rotation every frame from `LocalGravityField.local_up`
//!   (world-space direction from body center to camera). This gives correct
//!   surface-relative viewing without inheriting Body rotation.
//!
//! The old "merged Body+Grid" design caused the center of rotation to shift
//! and broke Moon positioning. The two-layer design is correct.

use crate::gravity::PointMassGravity;
use avian3d::prelude::{Collider, CollisionLayers};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::*;
use lunco_celestial::{CelestialBody, CelestialBodyRegistry, ReferenceFrame};
use lunco_celestial_spatial_core::SolarSystemRoot;
use lunco_environment::{Gravity, GravityProvider};
use lunco_materials::ShaderLook;
use lunco_render::PbrLook;

/// Collision membership for celestial picking geometry.
///
/// Celestial spheres are queryable so the picker can focus a body, but they are
/// not physical surfaces.  An empty filter keeps them out of Avian's contact
/// graph while preserving their membership for `SpatialQuery` masks.  Using a
/// normal `ALL` filter here makes every streamed terrain collider that enters a
/// planet-sized sphere a narrow-phase pair, even though the sphere has no
/// `RigidBody` and can never contribute a physical response.
const CELESTIAL_PICKING_LAYERS: CollisionLayers =
    CollisionLayers::from_bits(lunco_core::CELESTIAL_COLLISION_LAYER, 0);

/// Adopt a look AUTHORED on the body's prim onto its globe tiles.
///
/// A celestial body is spawned in Rust (its radius, GM and rotation are physics,
/// not art), but how it LOOKS is content. `lunco-usd-sim-shader` already turns a
/// `UsdShade` Material binding on any prim into a [`ShaderLook`]. This carries
/// that look from the declaring prim to the globe. An authored albedo remains
/// authoritative; otherwise the installed body-imagery dataset supplies the
/// raster while this look continues to own its shader parameters.
pub(crate) fn adopt_authored_body_look(
    q_decl: Query<
        (
            &crate::CelestialBodyDecl,
            &ShaderLook,
            Option<&crate::AuthoredBodyAlbedo>,
        ),
        Changed<ShaderLook>,
    >,
    bound_imagery: Res<crate::imagery::BoundBodyImagery>,
    mut q_globes: Query<(
        Entity,
        &CelestialBody,
        &mut crate::globe_lod::GlobeLod,
        &crate::globe_lod::GlobeTiles,
    )>,
    mut commands: Commands,
) {
    for (decl, look, authored_albedo) in &q_decl {
        for (globe, body, mut lod, tiles) in &mut q_globes {
            if body.ephemeris_id != decl.naif {
                continue;
            }
            lod.look = if authored_albedo.is_none()
                && !look
                    .textures
                    .contains_key(&lunco_materials::TextureLayer::Albedo)
            {
                bound_imagery
                    .dataset_image(globe)
                    .map(|image| crate::imagery::bind_albedo(look, image))
                    .unwrap_or_else(|| look.clone())
            } else {
                look.clone()
            };
            crate::imagery::apply_look_to_tiles(tiles, &lod.look, &mut commands);
            info!(
                "[celestial] body {} adopted the look authored on its prim",
                decl.naif
            );
        }
    }
}

/// **The celestial ownership marker.** Every celestial-owned root spawned by the
/// subsystem carries this marker. Teardown despawns those roots recursively, so
/// their grids, bodies, terrain tiles, labels, and other structural descendants
/// are removed as one owned hierarchy by the scene teardown system.
///
/// This is the *architecture* that keeps scene reload correct: celestial content is
/// declared per scene (`CelestialBodyDecl`), and everything derived from that
/// declaration is owned by this marker, so every scene replacement tears the old sky
/// down completely before projecting the new one — no orbiting ghost bodies, stale
/// orbit lines, or old physics frame — without maintaining a despawn list. The invariant is
/// one line: *if the celestial subsystem owns a root, that root carries
/// `CelestialDerived`.*
#[derive(Component)]
pub struct CelestialDerived;

/// Marker for the Earth-Moon barycenter grid (genuinely inertial — the EMB is a
/// barycenter, so it has no IAU rotation model and `body_rotation_system` skips
/// it).
#[derive(Component)]
pub struct EMBRoot;

/// Marker for Earth's grid. **Rotating** (ephemeris position + IAU spin) — its
/// children are body-fixed. For a star-fixed frame at Earth use the
/// [`ReferenceFrame::EclipticJ2000`], not this.
#[derive(Component)]
pub struct EarthRoot;

/// Marker for the Moon's grid. **Rotating**, as [`EarthRoot`].
#[derive(Component)]
pub struct MoonRoot;

/// Marker for Earth's surface sub-grid.
///
/// Surface entities — rovers, avatars, terrain tiles, future surface ops —
/// live here so their `Transform.translation` stays small in `f32` and
/// inherits Earth's sidereal rotation via the parent Grid.
#[derive(Component)]
pub struct EarthSurfaceRoot;

/// Marker for Moon's surface sub-grid. See [`EarthSurfaceRoot`].
#[derive(Component)]
pub struct MoonSurfaceRoot;

/// A render-only copy of a celestial frame, updated from [`WorldTime`].
///
/// The physical body hierarchy remains authoritative for site terrain, physics,
/// and the streamed globe tiles. This copy serves render-only body-fixed marker
/// geometry and follows the same completed physical epoch without camera offsets
/// or a separate celestial clock. This marker deliberately is *not* a
/// `ReferenceFrame`, so the causal ephemeris and body-rotation systems cannot
/// accidentally write it.
#[derive(Component, Debug, Clone, Copy)]
pub struct CelestialPresentationGrid {
    /// NAIF id whose position is relative to this grid's parent.
    pub body: i32,
    /// Whether this frame carries the body's IAU rotation.
    ///
    /// A body-fixed globe and its co-located inertial camera anchor have the
    /// same translated origin but different orientation. Keeping that fact on
    /// the frame contract prevents orbit presentation from accidentally
    /// inheriting surface rotation.
    pub body_fixed: bool,
}

/// Sets up the complete big_space entity hierarchy.
///
/// Uses the two-layer pattern: inertial Grid + rotating Body child.
/// This matches the established LunCoSim architecture and keeps
/// orbit cameras star-fixed while surface cameras use world-space
/// rotation computed from `LocalGravityField`.
pub fn setup_big_space_hierarchy(
    mut commands: Commands,
    registry: Res<CelestialBodyRegistry>,
    config: Res<crate::CelestialConfig>,
    quality: Res<lunco_render::RenderingQualitySettings>,
    grid_config: Res<lunco_spatial::WorldGridConfig>,
    mut meshes: ResMut<Assets<Mesh>>,
    // (No `AssetServer`: this hierarchy loads no textures — see the imagery note below.)
    // The single world-shell grid (WorldShellPlugin) to nest under.
    q_world_grid: Query<Entity, (With<lunco_spatial::WorldGrid>, With<Grid>)>,
    body_looks: Query<(&crate::CelestialBodyDecl, &ShaderLook)>,
    subsystems: Option<ResMut<lunco_core_runtime::subsystems::SubsystemToggles>>,
    bindings: Res<lunco_input_core::InputBindingsSettings>,
    mut missing_look_reported: Local<bool>,
) {
    let Some(earth_look) = body_looks
        .iter()
        .find(|(decl, _)| decl.naif == lunco_celestial::ephemeris_id::EARTH)
        .map(|(_, look)| look.clone())
    else {
        if !*missing_look_reported {
            error!(
                "[celestial] Earth declaration has no authored visual look; refusing to build the celestial hierarchy"
            );
            *missing_look_reported = true;
        }
        return;
    };
    let Some(moon_look) = body_looks
        .iter()
        .find(|(decl, _)| decl.naif == lunco_celestial::ephemeris_id::MOON)
        .map(|(_, look)| look.clone())
    else {
        if !*missing_look_reported {
            error!(
                "[celestial] Moon declaration has no authored visual look; refusing to build the celestial hierarchy"
            );
            *missing_look_reported = true;
        }
        return;
    };
    *missing_look_reported = false;

    let Ok(input_map) = bindings.input_map() else {
        error!("[celestial] refusing to create the observer from invalid input bindings");
        return;
    };
    let sun_profile = match quality.validated_profile() {
        Ok(profile) => profile,
        Err(reason) => {
            error!(
                "[celestial] invalid Graphics quality; refusing to build celestial lighting hierarchy: {reason}"
            );
            return;
        }
    };
    // Every grid in the live hierarchy uses the same precision contract as the
    // persistent world shell.  A child with a different cell edge has a
    // different rebranch boundary and therefore can move relative to its
    // parent when BigSpace propagates the floating origin.  Keep the contract
    // in WorldGridConfig; this system must not grow a second set of grid
    // constants.
    let grid_config = *grid_config;
    let make_grid = || grid_config.grid();
    // A site-anchored DEM twin authors its own rocks and bakes rock features
    // into the far-field maps — the generated obstacle field on top is
    // redundant decoration that costs over a second per frame in views that
    // include it (thousands of collider+mesh rock entities across the DEM;
    // measured 0.7 → 32 FPS by toggling it off). Default it OFF here; the
    // procedural rover sandbox (no site anchor) keeps it, and
    // `SetSubsystemEnabled { name: "obstacle-field", on: true }` re-enables
    // it live for rover-testing on a twin.
    if let Some(mut toggles) = subsystems {
        if toggles.set("obstacle-field", false) {
            info!(
                "celestial takeover: obstacle-field subsystem defaulted OFF (site-anchored scene)"
            );
        } else {
            warn!(
                "celestial takeover: obstacle-field plugin did not register its subsystem toggle"
            );
        }
    }
    // NO HARDCODED PLANET IMAGERY.
    //
    // This used to `asset_server.load("lunco://textures/earth.png")` (and
    // moon.png) unconditionally. Those files are CACHE ARTIFACTS — produced by
    // the asset pipeline from a downloaded source, git-ignored, and absent on a
    // fresh checkout. A missing texture samples Bevy's white fallback, and the
    // Earth's `surface_color` was `[1,1,1]`, so the default experience was a
    // WHITE BALL where Earth should be: the engine asserting an asset it does
    // not ship and rendering nothing sensible when the assertion failed.
    //
    // So the body's own colour is the base state, not a degraded one. Imagery is
    // then just an ordinary authored look: bind a `UsdShade` Material to the body
    // prim and the existing USD → `ShaderLook` path picks it up, exactly as it
    // does for terrain layer maps and for any prop — see
    // [`adopt_authored_body_look`].

    // Body appearance is authored on the USD declarations and arrives as a
    // `ShaderLook`. This crate only carries that intent to the generated globe;
    // shader loading and binding stay at the render boundary.

    // `CelestialPlugin` installs `WorldShellPlugin` when the host has not
    // already done so. There is therefore exactly one storage hierarchy in
    // every context: production, headless, and tests all mount celestial grids
    // below the canonical `WorldGrid`. A second celestial-only root would make
    // frame transitions depend on which plugin happened to start first.
    let Ok(big_space_root) = q_world_grid.single() else {
        error!(
            "[celestial] canonical WorldGrid is missing or duplicated; refusing to spawn a second BigSpace hierarchy"
        );
        return;
    };

    // Resolve the built-in body catalog before spawning any derived entity.
    // The registry is the sole authority for identity and physical constants;
    // a partial hierarchy with guessed radii/GM/SOI is more dangerous than no
    // hierarchy because it looks valid while using inconsistent frames.
    let (Some(sun), Some(earth), Some(moon)) = (
        registry.get(lunco_celestial::ephemeris_id::SUN).cloned(),
        registry.get(lunco_celestial::ephemeris_id::EARTH).cloned(),
        registry.get(lunco_celestial::ephemeris_id::MOON).cloned(),
    ) else {
        error!(
            "[celestial] required Sun/Earth/Moon catalog entries are missing; refusing to build the celestial hierarchy"
        );
        return;
    };

    // ── Solar System Grid (inertial anchor) ────────────────────────────────
    //
    // CELL EDGE SETS RENDER PRECISION — NOT EXTENT. A grid's cell edge may look
    // like a free "scale" knob (bigger cells for bigger distances); it is not.
    // `LocalFloatingOrigin::translation` is an **f32** holding the floating
    // origin's offset within one cell of THIS grid, so it is bounded by
    // `maximum_distance_from_origin = edge/2 + switching_threshold`. When
    // big_space pushes the origin down the tree
    // (`local_origin::propagate_origin_to_child`) it rebuilds the origin's
    // position as `cells×edge` (exact f64) PLUS that f32. Re-splitting at the
    // child cannot recover bits the parent already dropped, so the COARSEST
    // grid in the chain sets the precision floor for its whole subtree.
    //
    // At the old `Grid::new(1e9, 1e8)` that f32 ranged to 6e8 m, where its ULP
    // is ~64 m — and the EMB grid below added ~4 m more. Everything under the
    // Moon (the surface underfoot, Earth, the orbit lines) therefore re-rounded
    // by tens of metres every frame the pin slid the tree: the "lunar surface
    // jitters / Earth jitters / orbits jump" report. Paused, the pin
    // early-returns, the origin's sub-cell offset never changes, and the frame
    // is pixel-identical — which is why a paused-clock test showed 0 px and hid
    // this for so long.
    //
    // Cells are `i64`, so small edges are free. Keep every celestial grid on
    // the same `WorldGridConfig` values as the root `WorldGrid`; otherwise a
    // coarser ancestor becomes the precision floor for its whole subtree.
    // With the default configuration, `max_distance` is 1100 m and the f32 ULP
    // at the split boundary is approximately 0.12 mm.
    // The solar hierarchy stays in its inertial frame. A site is mounted under
    // the matching body-fixed surface grid by `attach_site_scene_to_surface_grid`;
    // no celestial ancestor is re-posed to make that site the world origin.
    let solar_grid = commands
        .spawn((
            CelestialDerived,
            SolarSystemRoot,
            ReferenceFrame::EclipticJ2000 {
                center: lunco_celestial::ephemeris_id::SUN,
            },
            make_grid(),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
            InheritedVisibility::default(),
            Name::new("Solar Grid (Inertial)"),
            ChildOf(big_space_root),
        ))
        .id();

    // ── Sun (picking identity on Solar Grid, no grid of its own) ──────────
    //
    // Deliberately NOT tagged `SolarSystemRoot`: that marker names the one Solar
    // Grid entity, and the Sun is reached as a body (`CelestialBody`/ephemeris 10)
    // like any other.
    // The Sun is not a local surface. A real-radius sphere is therefore the
    // wrong render primitive here: its astronomical AABB intersects every
    // local shadow cascade, and a shadow-map implementation can legitimately
    // consider it before the render-only exclusion marker is extracted. The
    // directional light below is the Sun's illumination; sky/point
    // presentation owns any visible solar disc. Keep this entity as the
    // catalog/picking/gravity identity, but do not submit astronomical-scale
    // geometry to the local mesh renderer.
    let _sun_body = commands
        .spawn((
            sun.body_component(),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::Visible,
            InheritedVisibility::default(),
            Name::new("Sun Body"),
            // PICKING-ONLY COLLISION. The empty filter keeps the identity out of
            // Avian's contact graph while preserving body queries.
            Collider::sphere(sun.radius_m),
            CELESTIAL_PICKING_LAYERS,
            ChildOf(solar_grid),
        ))
        .id();

    // ── Sun Light: NOT SPAWNED HERE ────────────────────────────────────────
    //
    // The sun is scene content, composed from `lunco://lighting/sun.usda`: the
    // engine default is the weakest opinion on the scene's own `Sun` prim, not a
    // second entity. Composition resolves it before anything reaches the ECS, so
    // there is one prim and one light no matter what order things load in —
    // which matters here because this takeover is triggered by the site anchor
    // the scene load itself detects, i.e. it runs AFTER the scene's own light.
    // Spawning a sun here would be a second one, and since only the brightest is
    // steered from the ephemeris it would take the aim and leave the scene's own
    // sun frozen at its authored rotation.
    //
    // Physical/render lighting STATE is established here; the LIGHT is not.
    let sun = lunco_render::LunarSunShadow::for_profile(sun_profile);
    // Physical sun identity (illuminance / angular size) is environmental state.
    // A new celestial hierarchy starts with its physical lighting baseline.
    // Per-scene display exposure belongs to a composed `UsdGeomCamera`, which
    // is recreated with the scene; carrying the prior `LunarSun` resource here
    // would leak one scenario's grade into the next.
    let ls = lunco_environment::LunarSun {
        exposure_ev100: Some(sun_profile.camera_exposure_ev100),
        ..Default::default()
    };
    // Physical sun identity is environmental state. Camera exposure remains
    // authored by each `UsdGeomCamera`, so each scene keeps its own standard
    // ISO/shutter/f-stop calibration.
    commands.insert_resource(ls);
    // NOTE on shadow readability: the ~23-stop lunar range (128 klx direct
    // sun vs sub-lux earthshine) is not represented by a global ambient
    // resource. The terrain march owns its authored shadow-floor policy so
    // shadowed terrain keeps its relief while empty space stays black.
    commands.insert_resource(sun.shadow_map());

    // ── EMB Grid (inertial anchor for Earth-Moon system) ───────────────────
    let emb_grid = commands
        .spawn((
            EMBRoot,
            ReferenceFrame::EclipticJ2000 {
                center: lunco_celestial::ephemeris_id::EARTH_MOON_BARYCENTER,
            },
            // Same configured precision contract as the canonical WorldGrid.
            make_grid(),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
            InheritedVisibility::default(),
            Name::new("EMB Grid (Inertial)"),
            ChildOf(solar_grid),
        ))
        .id();

    // ── Earth body-fixed Grid (positioned by ephemeris, rotated by IAU) ────
    let earth_grid = commands
        .spawn((
            EarthRoot,
            ReferenceFrame::BodyFixed {
                body: lunco_celestial::ephemeris_id::EARTH,
            },
            // Same configured precision contract as the canonical WorldGrid.
            make_grid(),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
            InheritedVisibility::default(),
            Name::new("Earth Grid (Body Fixed)"),
            ChildOf(emb_grid),
        ))
        .id();

    // ── Earth Inertial Anchor (star-fixed frame at Earth) ──────────────────
    // Same position as the Earth Grid, NO rotation. The ephemeris system
    // positions both frames directly; the rotation stays IDENTITY forever. The
    // Observer Camera hangs here so the orbit view is actually star-fixed
    // (parented to the rotating Earth Grid it swung a 19,000 km circle once per
    // sidereal day — the whole point of `ReferenceFrame::EclipticJ2000`).
    let earth_inertial = commands
        .spawn((
            ReferenceFrame::EclipticJ2000 {
                center: lunco_celestial::ephemeris_id::EARTH,
            },
            // Same configured precision contract as every other celestial grid.
            make_grid(),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
            InheritedVisibility::default(),
            Name::new("Earth Inertial Anchor"),
            ChildOf(emb_grid),
        ))
        .id();

    // ── Earth Body (visual/physical centre in the rotating body-fixed Grid) ─
    // The body is a direct high-precision child of its body grid and therefore
    // carries CellCoord, just like every other grid-local spatial entity. Its
    // local Transform stays at identity; the parent body-fixed grid owns the
    // frame rotation and the cell owns the high-magnitude translation.
    let earth_body = commands
        .spawn((
            earth.body_component(),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::Visible,
            InheritedVisibility::default(),
            NoFrustumCulling,
            GravityProvider {
                model: Box::new(PointMassGravity { gm: earth.gm }),
            },
            // PICKING-ONLY GEOMETRY. The empty collision filter is what keeps this
            // collider out of Avian's physical contact graph; the absence of a
            // `RigidBody` alone is not sufficient because collider-only geometry is
            // still broad-phase indexed.
            // It remains in the spatial-query BVH for body picking. Vehicle/sensor rays
            // mask `CELESTIAL_COLLISION_LAYER` because a planet-sized sphere can contain
            // the whole local scene and otherwise returns a distance-0 hit.
            Collider::sphere(earth.radius_m),
            CELESTIAL_PICKING_LAYERS,
            Name::new("Earth Body (Rotating)"),
            ChildOf(earth_grid),
        ))
        .id();

    // ── Earth Surface Grid (edge=1e3 m, inside the rotating Earth Grid) ────
    let earth_surface_grid = commands
        .spawn((
            EarthSurfaceRoot,
            make_grid(),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
            InheritedVisibility::default(),
            Name::new("Earth Surface Grid"),
            ChildOf(earth_grid),
        ))
        .id();

    // Render-only marker frames follow the same WorldTime sample as the
    // physical hierarchy. They preserve the complete ephemeris ancestry: an
    // Earth child below the EMB must share the EMB epoch used by its parent.
    let emb_presentation_grid = commands
        .spawn((
            CelestialDerived,
            CelestialPresentationGrid {
                body: lunco_celestial::ephemeris_id::EARTH_MOON_BARYCENTER,
                body_fixed: false,
            },
            make_grid(),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
            InheritedVisibility::default(),
            Name::new("EMB Globe Presentation Grid"),
            ChildOf(solar_grid),
        ))
        .id();
    let _earth_presentation_inertial_grid = commands
        .spawn((
            CelestialDerived,
            CelestialPresentationGrid {
                body: lunco_celestial::ephemeris_id::EARTH,
                body_fixed: false,
            },
            make_grid(),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
            InheritedVisibility::default(),
            Name::new("Earth Globe Presentation Inertial Anchor"),
            ChildOf(emb_presentation_grid),
        ))
        .id();
    commands.spawn((
        CelestialDerived,
        CelestialPresentationGrid {
            body: lunco_celestial::ephemeris_id::EARTH,
            body_fixed: true,
        },
        make_grid(),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        InheritedVisibility::default(),
        Name::new("Earth Globe Presentation Grid"),
        ChildOf(emb_presentation_grid),
    ));
    // Earth terrain: camera-driven cube-sphere LOD. Globe tiles share the
    // physical Earth Surface Grid with site terrain and use the same `WorldTime`
    // pose as physics.
    // The authored `UsdShade` look supplies the base globe appearance. An
    // installed body-imagery dataset supplies its albedo map unless the scene
    // authors one, and `adopt_authored_body_look` carries both onto the tiles.
    commands.entity(earth_body).try_insert((
        crate::globe_lod::GlobeLod {
            radius_m: earth.radius_m,
            surface_grid: earth_surface_grid,
            look: earth_look,
            res: 32,
            max_lod: 8,
            lod_distance_factor: 2.0,
        },
        crate::globe_lod::GlobeTiles::default(),
    ));

    // ── Moon body-fixed Grid (positioned by ephemeris, rotated by IAU) ─────
    let moon_grid = commands
        .spawn((
            MoonRoot,
            ReferenceFrame::BodyFixed {
                body: lunco_celestial::ephemeris_id::MOON,
            },
            // Same configured precision contract as the canonical WorldGrid.
            make_grid(),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
            InheritedVisibility::default(),
            Name::new("Moon Grid (Body Fixed)"),
            ChildOf(emb_grid),
        ))
        .id();

    // ── Moon Inertial Anchor (star-fixed frame at Moon) ───────────────────
    // The Moon body grid carries the IAU spin for terrain/sites/vehicles.  An
    // orbit camera belongs in this co-located non-rotating sibling instead.
    commands.spawn((
        ReferenceFrame::EclipticJ2000 {
            center: lunco_celestial::ephemeris_id::MOON,
        },
        make_grid(),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        InheritedVisibility::default(),
        Name::new("Moon Inertial Anchor"),
        ChildOf(emb_grid),
    ));

    // ── Moon Body (visual/physical centre in the rotating body-fixed Grid) ─
    let moon_body = commands
        .spawn((
            moon.body_component(),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::Visible,
            InheritedVisibility::default(),
            NoFrustumCulling,
            GravityProvider {
                model: Box::new(PointMassGravity { gm: moon.gm }),
            },
            // PICKING-ONLY GEOMETRY. The empty collision filter is what keeps this
            // collider out of Avian's physical contact graph; the absence of a
            // `RigidBody` alone is not sufficient because collider-only geometry is
            // still broad-phase indexed.
            // It remains in the spatial-query BVH for body picking. Vehicle/sensor rays
            // mask `CELESTIAL_COLLISION_LAYER` because a planet-sized sphere can contain
            // the whole local scene and otherwise returns a distance-0 hit.
            Collider::sphere(moon.radius_m),
            CELESTIAL_PICKING_LAYERS,
            Name::new("Moon Body (Rotating)"),
            ChildOf(moon_grid),
        ))
        .id();

    // ── Moon Surface Grid (edge=1e3 m, inside the rotating Moon Grid) ──────
    let moon_surface_grid = commands
        .spawn((
            MoonSurfaceRoot,
            make_grid(),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
            InheritedVisibility::default(),
            Name::new("Moon Surface Grid"),
            ChildOf(moon_grid),
        ))
        .id();

    commands.spawn((
        CelestialDerived,
        CelestialPresentationGrid {
            body: lunco_celestial::ephemeris_id::MOON,
            body_fixed: true,
        },
        make_grid(),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        InheritedVisibility::default(),
        Name::new("Moon Globe Presentation Grid"),
        ChildOf(emb_presentation_grid),
    ));
    let _moon_presentation_inertial_grid = commands
        .spawn((
            CelestialDerived,
            CelestialPresentationGrid {
                body: lunco_celestial::ephemeris_id::MOON,
                body_fixed: false,
            },
            make_grid(),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
            InheritedVisibility::default(),
            Name::new("Moon Globe Presentation Inertial Anchor"),
            ChildOf(emb_presentation_grid),
        ))
        .id();
    // Moon terrain: camera-driven cube-sphere LOD (replaces the fixed 24-tile shell).
    commands.entity(moon_body).try_insert((
        crate::globe_lod::GlobeLod {
            radius_m: moon.radius_m,
            surface_grid: moon_surface_grid,
            look: moon_look,
            res: 32,
            max_lod: 8,
            lod_distance_factor: 2.0,
        },
        crate::globe_lod::GlobeTiles::default(),
    ));

    // ── Observer Camera (on Earth's INERTIAL ANCHOR, for the orbit view) ───
    // The camera must sit in a star-fixed frame, and the Earth Grid is NOT one:
    // it rotates with Earth (`body_rotation_system`). See `ReferenceFrame::EclipticJ2000`.
    // For surface views the camera uses SurfaceCamera, which recomputes
    // world-space rotation from LocalGravityField.
    let earth_radius_m = earth.radius_m;
    let earth_orbit_distance = earth_radius_m * 3.0;
    let cam_pos = DVec3::new(0.0, earth_orbit_distance * 0.4, earth_orbit_distance);
    let (cam_cell, cam_translation) = make_grid().translation_to_grid(cam_pos);
    let cam_direction = (-cam_pos).normalize().as_vec3();

    // The persistent OriginAnchor owns FloatingOrigin for both the observer
    // and sandbox cameras. Camera presentation and BigSpace origin ownership
    // are separate contracts.
    if config.spawn_observer_camera {
        commands.spawn((
            // The scene camera stated as INTENT: `lunco-render-bevy` attaches `Camera3d`,
            // the tonemapper and MSAA. Systems asking "which entity is the scene camera?"
            // filter on `With<SceneCamera>` — that question no longer costs a GPU stack.
            //
            // Tone mapping, MSAA, and the unauthored bloom look come from the
            // persisted Graphics profile. An authored LunCoEnvironment bloom value
            // is applied later as the scene-owned override. SMAA was already
            // dropped here — it blanks egui-composited viewports (the SMAA black-viewport
            // fix on main).
            // Grade + physical exposure from the ONE constructor every scene
            // camera uses (`lunco_render::scene_camera_look_with_profile`), paired with the
            // canonical sun illuminance (single source of truth —
            // lunco_environment::LunarSun).
            lunco_render::scene_camera_look_with_profile(ls.exposure_ev100, sun_profile),
            lunco_render::GraphicsCameraDefaults,
            Projection::Perspective(PerspectiveProjection {
                near: 1.0,
                far: 1.0e15,
                ..default()
            }),
            cam_cell,
            Transform::from_translation(cam_translation).looking_to(cam_direction, Vec3::Y),
            GlobalTransform::default(),
            lunco_embodiment_core::roles::Embodiment,
            lunco_control_core::IntentState::default(),
            input_map,
            lunco_control_core::IntentAnalogState::default(),
            Name::new("Observer Camera"),
            ChildOf(earth_inertial),
        )); // Star-fixed frame at Earth — NOT the rotating Earth Grid.
    } // config.spawn_observer_camera

    // ── Other Planets (simple entities on Solar Grid) ──────────────────────
    for body_desc in registry.bodies.iter() {
        if body_desc.ephemeris_id == lunco_celestial::ephemeris_id::SUN
            || body_desc.ephemeris_id == lunco_celestial::ephemeris_id::EARTH
            || body_desc.ephemeris_id == lunco_celestial::ephemeris_id::MOON
            || body_desc.ephemeris_id == lunco_celestial::ephemeris_id::EARTH_MOON_BARYCENTER
        {
            continue;
        }
        commands.spawn((
            body_desc.body_component(),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Mesh3d(
                meshes.add(
                    Sphere::new(body_desc.radius_m as f32)
                        .mesh()
                        .ico(2)
                        .unwrap(),
                ),
            ),
            PbrLook {
                base_color: LinearRgba::from(Color::srgb(0.5, 0.5, 0.5)),
                // `StandardMaterial`'s default (inherited via `..default()` before);
                // `PbrLook`'s default is 1.0, so state it or the planets go matte.
                perceptual_roughness: 0.5,
                ..default()
            },
            Name::new(format!("{} Body", body_desc.name)),
            // PICKING-ONLY GEOMETRY. The empty collision filter is what keeps this
            // collider out of Avian's physical contact graph; the absence of a
            // `RigidBody` alone is not sufficient because collider-only geometry is
            // still broad-phase indexed.
            // It remains in the spatial-query BVH for body picking. Vehicle/sensor rays
            // mask `CELESTIAL_COLLISION_LAYER` because a planet-sized sphere can contain
            // the whole local scene and otherwise returns a distance-0 hit.
            Collider::sphere(body_desc.radius_m),
            CELESTIAL_PICKING_LAYERS,
            ChildOf(solar_grid),
        ));
    }
}

/// Select the celestial gravity model for a site scene after the hierarchy is
/// present.
///
/// `UsdPhysicsScene:gravityDirection` is expressed in the stage frame. A site
/// scene is subsequently migrated under a body-fixed surface grid, whose
/// rotation is derived from the geodetic anchor. A stage-frame flat vector
/// therefore cannot remain the physical gravity vector after that migration:
/// it would pull a level pad sideways. Site scenes use the body's typed
/// surface-gravity model, which derives the direction and magnitude in the
/// same body-fixed frame as the terrain and rigid bodies. The authored physics
/// scene remains the flat-world default for scenes without a site anchor.
pub fn sync_site_gravity(
    q_site: Query<(), With<lunco_celestial::geo::SiteAnchor>>,
    mut gravity: ResMut<Gravity>,
) {
    if q_site.is_empty() || *gravity == Gravity::Surface {
        return;
    }
    *gravity = Gravity::Surface;
    info!(
        "celestial takeover: site scene selected body-fixed surface gravity; stage-frame PhysicsScene gravity is not valid after site alignment"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;

    #[test]
    fn celestial_picking_geometry_is_queryable_but_not_physical() {
        assert_ne!(
            CELESTIAL_PICKING_LAYERS.memberships.0 & lunco_core::CELESTIAL_COLLISION_LAYER,
            0,
        );
        assert_eq!(CELESTIAL_PICKING_LAYERS.filters.0, 0);
        let physical_geometry = CollisionLayers::from_bits(1, u32::MAX);
        assert!(!CELESTIAL_PICKING_LAYERS.interacts_with(physical_geometry));
    }

    #[test]
    fn site_gravity_uses_body_frame_after_stage_gravity_is_authored() {
        let mut app = App::new();
        app.insert_resource(Gravity::flat(1.62, DVec3::NEG_Y));
        app.world_mut().spawn(lunco_celestial::geo::SiteAnchor);
        app.add_systems(PostUpdate, sync_site_gravity);
        app.update();

        assert_eq!(app.world().resource::<Gravity>(), &Gravity::Surface);
    }
}
