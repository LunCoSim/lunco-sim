// One-time scene bootstrap: spawning the Grid/Body/Surface hierarchy at
// startup. `set_parent_in_place` is fine here because no observers are
// registered against these archetypes yet, and the entities have default
// (CellCoord, Transform), so the lint's atomic-migration concern doesn't
// apply. See `lunco_core::attach::migrate_to_grid` for the runtime path.
#![allow(clippy::disallowed_methods)]

//! Sets up the big_space coordinate hierarchy for the solar system.
//!
//! ## Architecture: Rotating Grid + Body-fixed children
//!
//! **The GRID rotates. The Body does not.** `body_rotation_system`
//! (`systems.rs`) queries `(&mut Transform, &CelestialReferenceFrame)`, and
//! `CelestialReferenceFrame` lives on the **grids** — never on the body
//! entities, which sit at identity. Everything else in the crate is built on
//! that fact (`placement.rs` inverse-rotates inertial orbits INTO the grid;
//! `coords.rs`'s stored-chain test assumes a spinning grid), which is why it is
//! the grid that spins and not the body:
//!
//! 1. **Body Grid (ROTATING)** — carries `Grid` + `CelestialReferenceFrame`.
//!    Positioned by the ephemeris system, **and rotated** by
//!    `body_rotation_system` with the body's IAU rotation (`geo::body_rotation`).
//!    Its children are therefore **body-fixed**: terrain tiles, ground stations
//!    and surface ops inherit the spin for free, in high precision — which is
//!    exactly what big_space recommends ("place the planet and all objects on
//!    its surface in the same grid").
//!
//! 2. **Body Entity** — child of the Grid, identity transform. Carries
//!    `CelestialBody`, mesh, collider, `SOI`, `GravityProvider`.
//!
//! 3. **Inertial Anchor** — a NON-rotating sibling grid tracking the body's
//!    position but not its spin ([`InertialAnchor`]). This is where a
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
//!         │     │     └── Earth Surface Grid (surface sub-frame, body-fixed)
//!         │     │           └── terrain tiles + rovers + surface ops
//!         │     ├── Earth Inertial Anchor (position only, NO spin)
//!         │     │     └── Observer Camera  ← star-fixed
//!         │     └── Moon Grid (ROTATING: ephemeris + IAU spin)
//!         │           ├── Moon Body (mesh+collider, identity transform)
//!         │           └── Moon Surface Grid (surface sub-frame, body-fixed)
//!         │                 └── terrain tiles + rovers + surface ops
//!         └── Other planets (simple entities)
//! ```
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
//! Surface ops (rovers, avatars, terrain) live in a finer sub-Grid (edge=1e3 m,
//! ULP ≈ 60 µm at half-cell) under each body's rotating Grid. This keeps avian's
//! `Position` near zero in the rover's frame so f64 → f32 narrowing preserves
//! sub-mm precision on wheel raycasts even at body-radius distances from the
//! parent Grid's origin.
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

use bevy::prelude::*;
use big_space::prelude::*;
use avian3d::prelude::Collider;
use bevy::camera::visibility::NoFrustumCulling;
use crate::registry::{CelestialBodyRegistry, CelestialReferenceFrame, CelestialBody};
use crate::gravity::PointMassGravity;
use lunco_environment::GravityProvider;
use crate::soi::SOI;
use lunco_materials::{ParamValue, ShaderLook, TextureLayer};
use lunco_render::{PbrLook, SceneCamera};

/// Build the blueprint-grid [`ShaderLook`] for a celestial body's tiles: planet
/// imagery (the `Albedo` layer) tinted by `surface`, with the lat/long grid overlay
/// (`transition = 0`, the spherical mode of `blueprint.wgsl`).
///
/// Appearance **intent** only — `lunco-render-bevy` turns it into the real
/// `ShaderMaterial` (see `docs/architecture/render-decoupling.md`). Identical looks
/// share one material, so a body's whole tile set is still ONE material and one bind
/// group, exactly as the single hand-threaded handle used to guarantee.
fn blueprint_tile_look(
    texture: Handle<Image>,
    surface: [f32; 3],
    line: [f32; 3],
    subdivisions: [f32; 2],
    line_width: f32,
    roughness: f32,
) -> ShaderLook {
    ShaderLook::new("shaders/blueprint.wgsl")
        .with_texture(TextureLayer::Albedo, texture)
        .with("surface_color", ParamValue::Vec3(surface))
        .with("roughness", ParamValue::F32(roughness))
        .with("high_line_color", ParamValue::Vec3(line))
        .with("low_line_color", ParamValue::Vec3(line))
        .with("subdivisions", ParamValue::Vec2(subdivisions))
        .with("fade_range", ParamValue::Vec2([0.2, 0.6]))
        .with("line_width", ParamValue::F32(line_width))
        .with("transition", ParamValue::F32(0.0))
}

/// **The celestial ownership marker.** EVERY entity the celestial subsystem spawns in
/// Rust — grids, bodies, inertial anchors, orbit views, mission spacecraft — carries
/// this, and teardown despawns the whole set in one query
/// ([`teardown_celestial_when_undeclared`](crate::teardown_celestial_when_undeclared)).
///
/// This is the *architecture* that keeps scene reload correct: celestial content is
/// declared per scene (`CelestialBodyDecl`), and everything derived from that
/// declaration is owned by this marker, so "reload into a scene without a sky" tears
/// the sky down completely — no orbiting ghost bodies, no stale orbit lines, no sky
/// clock — without anyone maintaining a list of what to despawn. The invariant is
/// one line: *if the celestial subsystem spawns it, it carries `CelestialDerived`.*
#[derive(Component)]
pub struct CelestialDerived;

/// Marker for the solar system root grid (inertial, no rotation).
#[derive(Component)]
pub struct SolarSystemRoot;

/// Marker for the zero-translation grid between the `WorldGrid` and the Solar
/// Grid that carries the site-ENU `align` rotation — the only rotated joint
/// in the celestial chain, placed where the origin vector is near-zero so the
/// f32 quat costs sub-millimetres instead of the 15–20 km it cost on the
/// heliocentric Solar Grid (see the spawn-site comment).
#[derive(Component)]
pub struct SiteAlignGrid;

/// Marker for the Earth-Moon barycenter grid (genuinely inertial — the EMB is a
/// barycenter, so it has no IAU rotation model and `body_rotation_system` skips
/// it).
#[derive(Component)]
pub struct EMBRoot;

/// Marker for Earth's grid. **Rotating** (ephemeris position + IAU spin) — its
/// children are body-fixed. For a star-fixed frame at Earth use the
/// [`InertialAnchor`], not this.
#[derive(Component)]
pub struct EarthRoot;

/// Marker for the Moon's grid. **Rotating**, as [`EarthRoot`].
#[derive(Component)]
pub struct MoonRoot;

/// A grid that tracks a body's POSITION but never its rotation — a star-fixed
/// (inertial) frame co-located with the body.
///
/// `systems::sync_inertial_anchors` copies the body grid's `(CellCoord,
/// Transform.translation)` here each epoch and leaves `Transform.rotation` at
/// IDENTITY. That is the entire mechanism.
///
/// **Why it is a separate entity and not just "the body grid without the spin":**
/// the body grid must spin, because its children are surface features that have
/// to be carried by the body's rotation in high precision. An orbit camera needs
/// the opposite. Both frames are legitimate; they are different frames, so they
/// are different entities.
///
/// It deliberately does NOT carry `CelestialReferenceFrame` — that component is
/// what `body_rotation_system` rotates and what `placement` searches to find "the
/// grid for body N". A second entity answering that search would make the choice
/// of frame for every ground station and orbit **nondeterministic**.
#[derive(Component, Debug, Clone, Copy)]
pub struct InertialAnchor {
    /// NAIF id of the body whose position this anchor tracks.
    pub ephemeris_id: i32,
}

/// Marker for Earth's surface sub-grid (edge=1e3 m).
///
/// Surface entities — rovers, avatars, terrain tiles, future surface ops —
/// live here so their `Transform.translation` stays small in `f32` and
/// inherits Earth's sidereal rotation via the parent Grid.
#[derive(Component)]
pub struct EarthSurfaceRoot;

/// Marker for Moon's surface sub-grid (edge=1e3 m). See [`EarthSurfaceRoot`].
#[derive(Component)]
pub struct MoonSurfaceRoot;

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
    mut meshes: ResMut<Assets<Mesh>>,
    asset_server: Res<AssetServer>,
    // The single world-shell root (WorldShellPlugin) to nest under, and any prior
    // FloatingOrigin holder (the shell's OriginAnchor) the Observer Camera claims.
    q_world_root: Query<Entity, With<lunco_core::WorldRoot>>,
    q_world_grid: Query<Entity, With<lunco_core::WorldGrid>>,
    q_prior_origins: Query<Entity, With<FloatingOrigin>>,
    q_prior_fallback_lights: Query<Entity, With<lunco_core::FallbackSceneLight>>,
    mut q_exposure: Query<&mut bevy::camera::Exposure>,
    subsystems: Option<ResMut<lunco_core::subsystems::SubsystemToggles>>,
) {
    // A site-anchored DEM twin authors its own rocks and bakes rock features
    // into the far-field maps — the generated obstacle field on top is
    // redundant decoration that costs over a second per frame in views that
    // include it (thousands of collider+mesh rock entities across the DEM;
    // measured 0.7 → 32 FPS by toggling it off). Default it OFF here; the
    // procedural rover sandbox (no site anchor) keeps it, and
    // `SetSubsystemEnabled { name: "obstacle-field", on: true }` re-enables
    // it live for rover-testing on a twin.
    if let Some(mut toggles) = subsystems {
        toggles.set("obstacle-field", false);
        info!("celestial takeover: obstacle-field subsystem defaulted OFF (site-anchored scene)");
    }
    // Earth/Moon textures load from the `cached_textures://` source on EVERY
    // platform — NOT baked into the binary. Desktop reads the cache dir; wasm
    // HTTP-fetches them same-origin (`cache_dir()` = ".cache" on wasm, so the
    // bevy HTTP reader resolves `<origin>/.cache/textures/<tex>`, staged there by
    // build_web.sh). A 4K Earth + Moon are tens of MB — far too large to embed.
    // REPEAT addressing on U is REQUIRED: the quadsphere tile UVs unwrap the
    // equirectangular longitude around each tile's centre so triangles never
    // interpolate backwards across the ±180° seam — which legitimately pushes
    // u outside [0,1] (a seam-centred face sits entirely in u ∈ [-0.5, 0]).
    // Under the default clamp-to-edge sampler that whole region sampled one
    // texel column: a face-wide horizontally-streaked "patch" on both globes.
    let seam_wrap = |s: &mut bevy::image::ImageLoaderSettings| {
        s.sampler = bevy::image::ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
            address_mode_u: bevy::image::ImageAddressMode::Repeat,
            ..bevy::image::ImageSamplerDescriptor::linear()
        });
    };
    let earth_texture: Handle<Image> =
        asset_server.load_with_settings("lunco://textures/earth.png", seam_wrap);
    let moon_texture: Handle<Image> =
        asset_server.load_with_settings("lunco://textures/moon.png", seam_wrap);

    // The blueprint grid shader is named by PATH in the `ShaderLook` (see
    // `blueprint_tile_look`) and loaded by the binder, so it still hot-reloads on
    // native and HTTP-fetches on web like every other shader — this crate just never
    // holds a `Handle<Shader>` (that type is `bevy_shader`, which pulls naga).

    // 1. Reuse the single world-shell hierarchy if present; otherwise
    //    (standalone celestial, no WorldShellPlugin) spawn our own root. This is
    //    the "collapse to one root" fix — in the full client the solar grids nest
    //    under the shell instead of creating a second, origin-less BigSpace.
    //    Prefer the shell's `WorldGrid` (a real `Grid`) over the bare `WorldRoot`
    //    (`BigSpace` only, NO `Grid`): the Solar Grid's `(CellCoord, Transform)`
    //    is interpreted in its PARENT grid, and the site-anchoring pin (doc 43)
    //    needs a parent grid for a high-precision heliocentric pose — under a
    //    grid-less parent the pin would fall back to raw f32, which quantizes in
    //    ~16 km steps at 1 AU (visible as the whole sky jumping/jittering).
    let big_space_root = q_world_grid
        .iter()
        .next()
        .or_else(|| q_world_root.iter().next())
        .unwrap_or_else(|| {
            // Standalone fallback (no WorldShellPlugin): a CANONICAL big_space
            // root — `BigSpace` + `Grid` on the same entity, NO `Transform`.
            // The high-precision pass only writes a root's GlobalTransform
            // when Grid and BigSpace share the entity; a bare `BigSpace`
            // leaves every child grid's pose to the f32 compat pass.
            commands
                .spawn((
                    BigSpace::default(),
                    Grid::new(2_000.0, 100.0),
                    GlobalTransform::default(),
                    Visibility::default(),
                    InheritedVisibility::default(),
                    Name::new("Celestial BigSpace Root (standalone)"),
                ))
                .id()
        });

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
    // Cells are `i64`, so small edges are free: 1 AU / 2 km ≈ 7.5e7 cells. Keep
    // every celestial grid at the same 2 km / 100 m as the root `WorldGrid` —
    // `max_distance` 1100 m, f32 ULP there ≈ 0.12 mm.
    // ── Site-Align Grid — the ONLY rotated joint in the celestial chain ────
    // Zero translation, zero cell; `anchor_solar_frame_to_site` writes the
    // site-ENU `align` rotation HERE, not on the Solar Grid. big_space's
    // origin propagation multiplies a grid's stored f32 rotation into the
    // origin's position vector at that node: on the Solar Grid that vector
    // is heliocentric (~1.5e11 m), so the f32 quat's ~1e-7 relative error
    // cost 15–20 km — the measured ULP staircase that made the globe judder
    // from the ground and the terrain judder from orbit ("the shadow on the
    // moon oscillates"). At THIS node the origin vector is near-zero (the
    // camera is within tens of km of the site), so the same f32 rotation
    // costs sub-millimetres, and the 1 AU offset below travels through the
    // EXACT i64 cells of the now identity-rotation Solar Grid.
    let align_grid = commands.spawn((
        SiteAlignGrid,
        // Subtree root: the entire body hierarchy chain-parents under this, so a
        // recursive despawn here tears down every grid, body, anchor and globe tile.
        CelestialDerived,
        Grid::new(2_000.0, 100.0),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        InheritedVisibility::default(),
        Name::new("Site Align Grid"),
    )).set_parent_in_place(big_space_root).id();

    let solar_grid = commands.spawn((
        SolarSystemRoot,
        CelestialReferenceFrame { ephemeris_id: 10 },
        Grid::new(2_000.0, 100.0),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        InheritedVisibility::default(),
        Name::new("Solar Grid (Inertial)"),
    )).set_parent_in_place(align_grid).id();

    // ── Sun (simple entity on Solar Grid, no grid of its own) ─────────────
    let _sun_body = commands.spawn((
        SolarSystemRoot,
        CelestialBody {
            name: "Sun".to_string(),
            ephemeris_id: 10,
            radius_m: 696_340.0e3,
        },
        SOI { radius_m: 1.0e13 },
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::Visible,
        InheritedVisibility::default(),
        // The sun's own visual sphere must NEVER cast shadows: it sits exactly
        // along the `DirectionalLight` direction, so as a caster it pancakes
        // into every cascade map and "eclipses" the whole scene — with the
        // celestial hierarchy enabled, every fragment within
        // `shadow_max_distance` rendered fully shadowed (the pitch-black
        // site-anchored surface), while terrain beyond cascade range lit fine.
        bevy::light::NotShadowCaster,
        Mesh3d(meshes.add(Sphere::new(696_340.0e3).mesh().ico(4).unwrap())),
        // `no_shadow_cast` mirrors the `NotShadowCaster` above and is NOT optional:
        // the binder's `Changed<PbrLook>` pass reconciles the marker from the look, so
        // a look that said `false` would STRIP the marker on the first frame and bring
        // back the sun-eclipses-everything bug the comment above describes.
        PbrLook {
            base_color: LinearRgba::BLACK,
            emissive: LinearRgba::from(Color::srgb(1.0, 0.9, 0.4)) * 5.0,
            // `StandardMaterial`'s default, which this spawn used to inherit via
            // `..default()`. `PbrLook`'s own default is 1.0 (regolith), so it must be
            // stated explicitly to keep the sun disc's shading identical.
            perceptual_roughness: 0.5,
            no_shadow_cast: true,
            ..default()
        },
        Name::new("Sun Body"),
        Collider::sphere(696_340.0e3),
    )).set_parent_in_place(solar_grid).id();

    // ── Sun Light ──────────────────────────────────────────────────────────
    // Tagged `FallbackSceneLight`: a scene that authors its own UsdLux
    // light (e.g. the moonbase Twin's `DistantLight`) replaces this default
    // sun — TWO simultaneous DirectionalLights double-light the scene and
    // make "which sun?" ambiguous for shadow systems.
    // Canonical lunar-sun shadows (cascade split + biases + 4096² atlas) from
    // the single source of truth — see `lunco_render::LunarSunShadow`. This
    // spawn used to omit the cascade config entirely, so it rendered with
    // Bevy's single-cascade default (wrong terrain self-shadow, clipped
    // low-sun streaks). Now it matches the sandbox + USD paths by construction.
    // REPLACE any pre-existing fallback sun (the sandbox binary spawns one at
    // startup, before the celestial hierarchy enables on site-anchor
    // detection). Two simultaneous shadow-casting suns double-light the scene
    // from conflicting directions.
    for e in q_prior_fallback_lights.iter() {
        commands.entity(e).despawn();
    }
    let sun = lunco_render::LunarSunShadow::default();
    // Physical sun identity (illuminance / angular size) is environmental state.
    let ls = lunco_environment::LunarSun::default();
    // Taking over the lighting rig means taking over the EXPOSURE with it:
    // this spawn replaces the sandbox's studio sun (10 klux, EV 9.7 — a
    // matched pair) with the calibrated 128 klux lunar sun. Cameras left at
    // studio EV under the real sun are +3.7 stops — "everything is
    // overexposed", surface and orbit alike. Write the resource (cameras
    // spawned later read it) AND the live cameras.
    commands.insert_resource(ls);
    for mut exposure in q_exposure.iter_mut() {
        exposure.ev100 = ls.exposure_ev100;
    }
    // NOTE on shadow readability: the ~23-stop lunar range (128 klx direct
    // sun vs sub-lux earthshine) is NOT handled here with a global ambient —
    // that lit the sky dome gray while the terrain march (which multiplies
    // the FINAL color) killed it on the very terrain it was meant to lift.
    // The fill lives in the march itself: `horizon_march.wgsl` floors sun
    // visibility at a few percent, so shadowed terrain keeps its relief and
    // space stays black.
    commands.insert_resource(sun.shadow_map());
    // Top-level entity, NOT a child of the Solar Grid: a `DirectionalLight`
    // only needs orientation (`update_sun_light_system` steers it in WORLD
    // axes), and parenting it into the solar hierarchy gives it a
    // heliocentric-magnitude (~1e11 m) GlobalTransform translation. Bevy
    // builds the cascade-shadow matrices from that transform in f32 — at that
    // magnitude they collapse into garbage that swallows the whole ground on
    // random frames (the site-anchored-scene lit/black strobe).
    commands.spawn((
        sun.directional_light(Color::WHITE, ls.illuminance_lux),
        sun.cascade_config(),
        lunco_core::SunAngularDiameter(ls.angular_diameter_deg),
        Transform::default(),
        GlobalTransform::default(),
        Name::new("Sun Light"),
        lunco_core::FallbackSceneLight,
    ));

    // ── EMB Grid (inertial anchor for Earth-Moon system) ───────────────────
    let emb_grid = commands.spawn((
        EMBRoot,
        CelestialReferenceFrame { ephemeris_id: 3 },
        // 2 km cells — see the Solar Grid note: cell edge is a PRECISION knob.
        Grid::new(2_000.0, 100.0),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        InheritedVisibility::default(),
        Name::new("EMB Grid (Inertial)"),
    )).set_parent_in_place(solar_grid).id();

    // ── Earth Inertial Grid (positioned by ephemeris) ──────────────────────
    let earth_grid = commands.spawn((
        EarthRoot,
        CelestialReferenceFrame { ephemeris_id: 399 },
        // 2 km cells — see the Solar Grid note: cell edge is a PRECISION knob.
        Grid::new(2_000.0, 100.0),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        InheritedVisibility::default(),
        Name::new("Earth Grid (Inertial)"),
    )).set_parent_in_place(emb_grid).id();

    // ── Earth Inertial Anchor (star-fixed frame at Earth) ──────────────────
    // Same position as the Earth Grid, NO rotation. `sync_inertial_anchors`
    // keeps the position in step; the rotation stays IDENTITY forever. The
    // Observer Camera hangs here so the orbit view is actually star-fixed
    // (parented to the rotating Earth Grid it swung a 19,000 km circle once per
    // sidereal day — the whole point of `InertialAnchor`).
    let earth_inertial = commands.spawn((
        InertialAnchor { ephemeris_id: 399 },
        // Same 2 km / 100 m as every other celestial grid — cell edge is a
        // PRECISION knob (see the Solar Grid note).
        Grid::new(2_000.0, 100.0),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        InheritedVisibility::default(),
        Name::new("Earth Inertial Anchor"),
    )).set_parent_in_place(emb_grid).id();

    // ── Earth Body (rotating child of Earth Grid) ─────────────────────────
    // Note: Body does NOT have CellCoord. It's a low-precision entity whose
    // GlobalTransform = Grid × local Transform. This allows rotation from
    // body_rotation_system to propagate to tile children via propagate_low_precision.
    // Position is handled by the parent Grid's ephemeris updates.
    let earth_gm = registry.bodies.iter()
        .find(|d| d.ephemeris_id == 399)
        .map(|d| d.gm)
        .unwrap_or(3.986e14);
    let earth_soi = registry.bodies.iter()
        .find(|d| d.ephemeris_id == 399)
        .and_then(|d| d.soi_radius_m)
        .unwrap_or(924e6);

    let earth_body = commands.spawn((
        CelestialBody {
            name: "Earth".to_string(),
            ephemeris_id: 399,
            radius_m: 6371.0e3,
        },
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::Visible,
        InheritedVisibility::default(),
        NoFrustumCulling,
        GravityProvider {
            model: Box::new(PointMassGravity { gm: earth_gm }),
        },
        SOI { radius_m: earth_soi },
        Collider::sphere(6371.0e3),
        Name::new("Earth Body (Rotating)"),
    )).set_parent_in_place(earth_grid).id();

    // ── Earth Surface Grid (edge=1e3 m, inside the rotating Earth Grid) ────
    let earth_surface_grid = commands.spawn((
        EarthSurfaceRoot,
        Grid::new(1_000.0, 100.0),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        InheritedVisibility::default(),
        Name::new("Earth Surface Grid"),
    )).set_parent_in_place(earth_grid).id();

    // Earth terrain: camera-driven cube-sphere LOD (replaces the old fixed 24-tile
    // shell). `update_globe_lod` streams tiles parented to the Earth Surface Grid.
    let earth_blueprint = blueprint_tile_look(
        earth_texture.clone(),
        [1.0, 1.0, 1.0], [0.0, 0.5, 1.0], [36.0, 18.0], 1.0, 0.5,
    );
    commands.entity(earth_body).insert((
        crate::globe_lod::GlobeLod {
            radius_m: 6371.0e3,
            surface_grid: earth_surface_grid,
            look: earth_blueprint,
            res: 32,
            max_lod: 8,
            lod_distance_factor: 2.0,
        },
        crate::globe_lod::GlobeTiles::default(),
    ));

    // ── Moon Inertial Grid (positioned by ephemeris) ───────────────────────
    let moon_grid = commands.spawn((
        MoonRoot,
        CelestialReferenceFrame { ephemeris_id: 301 },
        // 2 km cells — see the Solar Grid note: cell edge is a PRECISION knob.
        Grid::new(2_000.0, 100.0),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        InheritedVisibility::default(),
        Name::new("Moon Grid (Inertial)"),
    )).set_parent_in_place(emb_grid).id();

    // ── Moon Body (rotating child of Moon Grid) ────────────────────────────
    let moon_gm = registry.bodies.iter()
        .find(|d| d.ephemeris_id == 301)
        .map(|d| d.gm)
        .unwrap_or(4.904e12);
    let moon_soi = registry.bodies.iter()
        .find(|d| d.ephemeris_id == 301)
        .and_then(|d| d.soi_radius_m)
        .unwrap_or(66.1e6);

    let moon_body = commands.spawn((
        CelestialBody {
            name: "Moon".to_string(),
            ephemeris_id: 301,
            radius_m: 1737.0e3,
        },
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::Visible,
        InheritedVisibility::default(),
        NoFrustumCulling,
        GravityProvider {
            model: Box::new(PointMassGravity { gm: moon_gm }),
        },
        SOI { radius_m: moon_soi },
        Collider::sphere(1737.0e3),
        Name::new("Moon Body (Rotating)"),
    )).set_parent_in_place(moon_grid).id();

    // ── Moon Surface Grid (edge=1e3 m, inside the rotating Moon Grid) ──────
    let moon_surface_grid = commands.spawn((
        MoonSurfaceRoot,
        Grid::new(1_000.0, 100.0),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        InheritedVisibility::default(),
        Name::new("Moon Surface Grid"),
    )).set_parent_in_place(moon_grid).id();

    // Moon terrain: camera-driven cube-sphere LOD (replaces the fixed 24-tile shell).
    let moon_blueprint = blueprint_tile_look(
        moon_texture.clone(),
        [0.5, 0.5, 0.5], [0.6, 0.6, 0.6], [24.0, 12.0], 2.0, 0.9,
    );
    commands.entity(moon_body).insert((
        crate::globe_lod::GlobeLod {
            radius_m: 1737.0e3,
            surface_grid: moon_surface_grid,
            look: moon_blueprint,
            res: 32,
            max_lod: 8,
            lod_distance_factor: 2.0,
        },
        crate::globe_lod::GlobeTiles::default(),
    ));

    // ── Observer Camera (on Earth's INERTIAL ANCHOR, for the orbit view) ───
    // The camera must sit in a star-fixed frame, and the Earth Grid is NOT one:
    // it rotates with Earth (`body_rotation_system`). See `InertialAnchor`.
    // For surface views the camera uses SurfaceCamera, which recomputes
    // world-space rotation from LocalGravityField.
    let earth_radius_m = 6_371_000.0;
    let earth_orbit_distance = earth_radius_m * 3.0;
    let cam_pos = Vec3::new(0.0, earth_orbit_distance * 0.4, earth_orbit_distance);

    // Hosts that own their camera (sandbox avatar) keep their FloatingOrigin;
    // only the full-client Observer Camera claims it (doc 43).
    if config.spawn_observer_camera {
    // The Observer Camera is the intended view, so it holds the single
    // FloatingOrigin. Claim it from any prior holder (the shell's OriginAnchor)
    // so big_space never sees two origins (the "multiple floating origins →
    // resetting this big space" error — a known multi-crate hazard).
    for prior in q_prior_origins.iter() {
        commands.entity(prior).remove::<FloatingOrigin>();
    }

    commands.spawn((
        Camera::default(),
        // The scene camera stated as INTENT: `lunco-render-bevy` attaches `Camera3d`,
        // the tonemapper and MSAA. Systems asking "which entity is the scene camera?"
        // filter on `With<SceneCamera>` — that question no longer costs a GPU stack.
        //
        // BLOOM IS DELIBERATELY OFF. This spawn used to carry a tuned `Bloom`, but
        // `hdr` is set true NOWHERE in this repo (review finding `R4`), so that bloom
        // rendered NOTHING while still paying for its downsample/upsample chain.
        // Keeping it off is therefore what preserves today's actual output; turning it
        // on would be a visual change smuggled in by a decoupling pass. If someone
        // wants real bloom, that is a separate, deliberate decision:
        // `SceneCamera::default().with_bloom(..)` — which turns HDR on for you, because
        // bloom without HDR is exactly the bug `SceneCamera` exists to make
        // unrepresentable.
        //
        // Tonemapping stays `TonyMcMapface` (`ToneMap::default()`). SMAA was already
        // dropped here — it blanks egui-composited viewports (the SMAA black-viewport
        // fix on main).
        SceneCamera::default(),
        // Physical exposure paired with the canonical sun illuminance
        // (single source of truth — lunco_environment::LunarSun).
        bevy::camera::Exposure { ev100: lunco_environment::LunarSun::default().exposure_ev100 },
        Projection::Perspective(PerspectiveProjection {
            near: 1.0,
            far: 1.0e15,
            ..default()
        }),
        FloatingOrigin,
        CellCoord::default(),
        Transform::from_translation(cam_pos).looking_at(Vec3::ZERO, Vec3::Y),
        GlobalTransform::default(),
        lunco_core::Avatar,
        lunco_core::IntentState::default(),
        lunco_controller::get_avatar_input_map(),
        lunco_core::IntentAnalogState::default(),
        Name::new("Observer Camera"),
    )).set_parent_in_place(earth_inertial); // Star-fixed frame at Earth — NOT the rotating Earth Grid.
    } // config.spawn_observer_camera

    // ── Other Planets (simple entities on Solar Grid) ──────────────────────
    for body_desc in registry.bodies.iter() {
        if body_desc.ephemeris_id == 10 || body_desc.ephemeris_id == 399
            || body_desc.ephemeris_id == 301 || body_desc.ephemeris_id == 3
        {
            continue;
        }
        commands.spawn((
            CelestialBody {
                name: body_desc.name.clone(),
                ephemeris_id: body_desc.ephemeris_id,
                radius_m: body_desc.radius_m,
            },
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Mesh3d(meshes.add(Sphere::new(body_desc.radius_m as f32).mesh().ico(2).unwrap())),
            PbrLook {
                base_color: LinearRgba::from(Color::srgb(0.5, 0.5, 0.5)),
                // `StandardMaterial`'s default (inherited via `..default()` before);
                // `PbrLook`'s default is 1.0, so state it or the planets go matte.
                perceptual_roughness: 0.5,
                ..default()
            },
            Name::new(format!("{} Body", body_desc.name)),
            Collider::sphere(body_desc.radius_m),
        )).set_parent_in_place(solar_grid);
    }
}
