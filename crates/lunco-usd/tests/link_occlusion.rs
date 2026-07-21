//! The connectivity scene contract: `comms_wall.usda` and the assets it uses
//! must author what the link kernel needs, through the REAL USD → ECS pipeline.
//!
//! The kernel's own geometry (range, masks, occlusion, the verdict hook, AOS/LOS,
//! cadence) is unit-tested in `lunco-celestial/src/link.rs` against a hand-built
//! world. What those tests CANNOT see is the seam this file guards: whether the
//! authored scene still produces the components the kernel solves over. Every gap
//! that shipped here was of that kind, not of the kernel kind —
//!
//! * `structures/comms_mast.usda` was named "the base's link home" and had no
//!   `lunco:linkNode` at all, so the base was not a node;
//! * three assets and a comment advertised `comms:*` ports that no code published;
//! * `props/wall.usda` had a collider and no `lunco:occluder`, so a wall between two
//!   antennas did not block a thing.
//!
//! A green kernel says nothing about any of that. These tests would have failed on
//! all three.

use bevy::asset::AssetPlugin;
use bevy::prelude::*;
use big_space::prelude::CellCoord;
use lunco_celestial::link::{LinkNode, LinkOccluder};
use lunco_usd_avian::*;
use lunco_usd_bevy::*;
use lunco_usd_sim::*;
use std::path::Path;

/// Compose a USD file and hand it to the Bevy pipeline as a canonical stage —
/// the same shape the other pipeline tests use (each integration test is its own
/// crate, so this helper is per-file by necessity).
fn add_canonical_from_file(app: &mut App, file_path: &Path) -> Handle<UsdStageAsset> {
    let handle = {
        let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
        stages.add(UsdStageAsset { recipe: None })
    };
    let stage = compose_file_to_stage(file_path)
        .unwrap_or_else(|e| panic!("Composition failed for {}: {e}", file_path.display()));
    let cstage = CanonicalStage::from_stage(stage, file_path.display().to_string());
    app.world_mut()
        .get_non_send_mut::<CanonicalStages>()
        .expect("CanonicalStages resource (UsdBevyPlugin)")
        .insert(handle.id(), cstage);
    handle
}

/// Compose a scene/asset through the same pipeline the app uses, and settle it.
fn load_through_bevy(file: &str, prim_path: &str) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<Image>();
    app.init_asset::<bevy::shader::Shader>();
    // No GPU here: mark headless so sim builds physics without waiting on a
    // render-only material (the `--no-ui` server stand-in).
    app.insert_resource(NoRenderVisuals);
    app.add_plugins((UsdBevyPlugin, UsdAvianPlugin, UsdSimPlugin));

    let handle = add_canonical_from_file(&mut app, &Path::new("../../assets/").join(file));
    app.world_mut().spawn((
        Name::new("TestRoot"),
        UsdPrimPath { stage_handle: handle, path: prim_path.to_string() },
        Transform::default(),
        CellCoord::default(),
        Visibility::Visible,
        InheritedVisibility::default(),
        ViewVisibility::default(),
    ));
    for _ in 0..10 {
        app.update();
    }
    app.world_mut().flush();
    app
}

/// The entity for a prim, addressed by its FULL path — the pipeline names entities
/// `/CommsWallTest/Mast`, and only the full path is unambiguous.
///
/// Not the leaf: this scene contains both `/CommsWallTest/Mast` (the base station)
/// and `/CommsWallTest/Rover/Comms/Mast` (the little cylinder holding the rover's
/// dish). A leaf match silently picked the wrong one — which is the same lesson the
/// kernel learned when it keyed nodes by `class`: names are not identities.
fn expect_path(app: &mut App, path: &str) -> Entity {
    let mut q = app.world_mut().query::<(Entity, &Name)>();
    q.iter(app.world())
        .find(|(_, n)| n.as_str() == path)
        .map(|(e, _)| e)
        .unwrap_or_else(|| panic!("no prim at '{path}' in the spawned scene"))
}

fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `lunco:occluder` on a prim ⇒ a `LinkOccluder` the kernel can test segments
/// against. Without this component the wall is scenery: it stops wheels and lets
/// radio through.
#[test]
fn wall_prop_authors_an_occluder() {
    let mut app = load_through_bevy("props/wall.usda", "/Wall");
    let body = expect_path(&mut app, "/Wall/Body");

    let occ = app
        .world()
        .get::<LinkOccluder>(body)
        .expect("props/wall.usda Body must carry lunco:occluder → LinkOccluder");

    // No authored `extent` ⇒ the unit-cube default, which the kernel scales by the
    // prim's own scale (8×4×1 ⇒ half-extents 4×2×0.5).
    assert_eq!(
        occ.half_extents,
        bevy::math::DVec3::splat(0.5),
        "a Cube with no authored extent must fall back to the unit cube"
    );
    let scale = app.world().get::<Transform>(body).expect("Body Transform").scale;
    let (_, half) = occ.box_for(scale);
    assert!(
        (half.x - 4.0).abs() < 1e-6 && (half.y - 2.0).abs() < 1e-6 && (half.z - 0.5).abs() < 1e-6,
        "the wall's occluding box must match its drawn geometry (4×2×0.5), got {half:?}"
    );

    // Occlusion and collision are SEPARATE facts (doc 49): the wall authors both,
    // and neither is derived from the other.
    assert!(
        app.world().get::<avian3d::prelude::Collider>(body).is_some(),
        "the wall still collides — occlusion did not replace its collider"
    );
}

/// The mast's whole purpose is to lift an antenna clear of the ground, so its link
/// node must be the `Antenna` child at dish height — not the root at y = 0, which
/// would take the terrain's line of sight rather than the dish's.
#[test]
fn comms_mast_is_a_link_node_at_dish_height() {
    let mut app = load_through_bevy("structures/comms_mast.usda", "/CommsMast");
    let antenna = expect_path(&mut app, "/CommsMast/Antenna");

    let node = app
        .world()
        .get::<LinkNode>(antenna)
        .expect("comms_mast.usda must have a link node — it is 'the base's link home'");
    assert_eq!(node.class.as_deref(), Some("base"));

    let tf = app.world().get::<Transform>(antenna).expect("Antenna Transform");
    assert!(
        tf.translation.y > 10.0,
        "the node must sit at the dish (y > 10), not at the mast's base: y = {}",
        tf.translation.y
    );
}

/// The occlusion scene, end to end: both endpoints are nodes, the wall between them
/// is an occluder, and the geometry is arranged so that ONLY occlusion can explain a
/// dropped link.
#[test]
fn comms_wall_scene_authors_two_nodes_and_a_wall_between_them() {
    let mut app = load_through_bevy("scenes/tests/comms_wall.usda", "/CommsWallTest");

    // The rover's antenna and the mast's antenna are both endpoints, with distinct
    // roles — so `can_reach(rover, "base")` has something to resolve.
    let mut q = app.world_mut().query::<(&LinkNode, &Name)>();
    let mut classes: Vec<String> = q
        .iter(app.world())
        .filter_map(|(n, _)| n.class.clone())
        .collect();
    classes.sort();
    assert_eq!(
        classes,
        vec!["base".to_string(), "base_clear".to_string(), "rover".to_string()],
        "the scene must author the two endpoints the lesson talks about, plus the \
         CONTROL mast (`base_clear`) whose sight-line misses the wall — without it \
         'the link is down' is equally true of a kernel that connects nothing"
    );

    // Exactly one occluder: the wall.
    let mut q_occ = app.world_mut().query::<(&LinkOccluder, &Name)>();
    let occluders: Vec<String> = q_occ
        .iter(app.world())
        .map(|(_, n)| leaf(n.as_str()).to_string())
        .collect();
    assert_eq!(
        occluders.len(),
        1,
        "one occluder, so a dropped link has exactly one possible cause: {occluders:?}"
    );

    // The wall must actually INTERSECT the sight-line — not merely sit between the
    // two nodes in z.
    //
    // This assertion exists because the weaker one (`mast.z < wall.z < rover.z`)
    // passed on a scene that demonstrated nothing: the mast lifts its antenna to
    // y = 10.6 while the rover's is at y = 0.84, so over a 40 m baseline the link
    // crossed the wall plane at y ≈ 5.66 and sailed over a 4 m wall. Running the
    // scene showed `can_reach == true`, which reads exactly like a broken occluder
    // and was in fact a correctly-placed wall of the wrong height. Ordering is not
    // occlusion; check the geometry the kernel actually tests.
    let body = expect_path(&mut app, "/CommsWallTest/Wall/Body");
    let rover_ant = expect_path(&mut app, "/CommsWallTest/Rover/Comms");
    let mast_ant = expect_path(&mut app, "/CommsWallTest/Mast/Antenna");

    // Scene-local positions: every prim here is a direct child chain off the root
    // with no rotation, so composing translations is exact.
    let world_y_z = |leaf: Entity, app: &App| -> (f32, f32) {
        let mut e = leaf;
        let (mut y, mut z) = (0.0f32, 0.0f32);
        loop {
            if let Some(t) = app.world().get::<Transform>(e) {
                y += t.translation.y;
                z += t.translation.z;
            }
            match app.world().get::<ChildOf>(e) {
                Some(p) => e = p.parent(),
                None => break,
            }
        }
        (y, z)
    };
    let (ay, az) = world_y_z(rover_ant, &app);
    let (by, bz) = world_y_z(mast_ant, &app);
    let (wy, wz) = world_y_z(body, &app);

    // The wall is between them at all.
    assert!(
        (bz < wz && wz < az) || (az < wz && wz < bz),
        "wall must sit between the antennas: mast z={bz}, wall z={wz}, rover z={az}"
    );

    // Where the sight-line crosses the wall's plane…
    let t = (az - wz) / (az - bz);
    let y_at_wall = ay + t * (by - ay);

    // …must be inside the box the kernel builds (half-extent × the Body's scale).
    let occ = app.world().get::<LinkOccluder>(body).expect("wall Body is an occluder");
    let scale = app.world().get::<Transform>(body).expect("Body Transform").scale;
    let (_, half) = occ.box_for(scale);
    let (lo, hi) = (wy as f64 - half.y, wy as f64 + half.y);
    assert!(
        (y_at_wall as f64) > lo && (y_at_wall as f64) < hi,
        "the sight-line crosses the wall plane at y={y_at_wall:.2}, but the wall's box \
         spans y ∈ [{lo:.2}, {hi:.2}] — the link would pass over/under it and the scene \
         would prove nothing"
    );
}

/// The rover's antenna component is a link node wherever it is mounted — the fact the
/// `comms_wall_test` and SS3 scenes both lean on.
#[test]
fn rover_antenna_is_a_link_node() {
    let mut app = load_through_bevy("vessels/rovers/skid_rover.usda", "/SkidRover");
    let comms = expect_path(&mut app, "/SkidRover/Comms");
    assert!(
        app.world().get::<LinkNode>(comms).is_some(),
        "the rover's Comms prim must be a link endpoint"
    );
}


