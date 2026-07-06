//! USD-authored connectivity (doc 43): `lunco:anchor:*` / `lunco:orbit:*` /
//! `lunco:comms:*` → celestial components → `update_comms_links` sight-lines →
//! `comms:*` ports. Runs on a fixed test ephemeris (Earth at the origin, Moon
//! 384 400 km along +X, epoch = J2000 so body rotation angles are zero) —
//! fully deterministic, no VSOP tables.

use bevy::asset::AssetApp;
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_celestial::comms::{CommsAntenna, CommsLinkState, CommsPlugin};
use lunco_celestial::ephemeris::{EphemerisProvider, EphemerisResource};
use lunco_celestial::geo::{GeodeticAnchor, SiteAnchor};
use lunco_celestial::kepler::KeplerOrbit;
use lunco_core::ports::PortRegistry;
use lunco_usd_bevy::{CanonicalStages, StageRecipe, UsdPrimPath, UsdStageAsset};
use lunco_usd_sim::celestial::insert_celestial_comms_components;
use openusd::sdf::Path as SdfPath;

/// Earth–Moon barycenter 1 AU along +X (clear of the Sun sphere at the
/// origin), Moon a further 384 400 km along +X. At J2000 all body rotation
/// angles are zero, so body-fixed == inertial — fully deterministic geometry.
struct TestEphemeris;

impl EphemerisProvider for TestEphemeris {
    fn position(&self, body_id: i32, _epoch_jd: f64) -> DVec3 {
        const AU: f64 = 1.495_978_707e11;
        match body_id {
            3 => DVec3::new(1.0, 0.0, 0.0),
            301 => DVec3::new(384_400.0e3 / AU, 0.0, 0.0),
            _ => DVec3::ZERO,
        }
    }
}

/// Scene: site anchored on the Moon's NEAR side (lon 180 faces Earth at +X
/// with the Moon further along +X → local up points back at Earth), a mast
/// antenna in the scene, a relay on a circular equatorial lunar orbit parked
/// between Moon and Earth (mean anomaly 180 → the −X side of the Moon), and
/// an Earth ground station at lat/lon 0 — directly under the Moon.
const SCENE: &str = r#"#usda 1.0
(
    defaultPrim = "Site"
)
def Xform "Site"
{
    double lunco:anchor:lat = 0
    double lunco:anchor:lon = 180
    double lunco:anchor:height = 0
    int lunco:anchor:body = 301

    def Xform "Mast"
    {
        bool lunco:comms:antenna = true
        double3 xformOp:translate = (0, 2, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }

    def Xform "RelaySat"
    {
        bool lunco:comms:antenna = true
        int lunco:orbit:body = 301
        double lunco:orbit:semiMajorAxisM = 3000000
        double lunco:orbit:eccentricity = 0
        double lunco:orbit:inclinationDeg = 0
        double lunco:orbit:meanAnomalyDeg = 180
    }

    def Xform "EarthStation"
    {
        bool lunco:comms:antenna = true
        double lunco:comms:minElevationDeg = 5
        double lunco:anchor:lat = 0
        double lunco:anchor:lon = 0
        double lunco:anchor:height = 0
        int lunco:anchor:body = 399
    }
}
"#;

fn build_app() -> (App, Vec<(Entity, &'static str)>) {
    let mut app = App::new();
    app.add_plugins(bevy::asset::AssetPlugin::default())
        .init_asset::<UsdStageAsset>()
        .init_non_send_resource::<CanonicalStages>()
        .add_plugins(CommsPlugin);

    // Fixed epoch + deterministic ephemeris.
    app.insert_resource(lunco_time::WorldTime {
        epoch_jd: lunco_time::J2000_JD,
        ..Default::default()
    });
    app.insert_resource(EphemerisResource {
        provider: std::sync::Arc::new(TestEphemeris),
    });

    let recipe = StageRecipe::from_source("comms.usda", SCENE);
    let handle = app
        .world_mut()
        .resource_mut::<Assets<UsdStageAsset>>()
        .add(UsdStageAsset { recipe: Some(recipe.clone()) });
    let id = handle.id();
    app.world_mut()
        .non_send_resource_mut::<CanonicalStages>()
        .get_or_build(id, &recipe)
        .expect("test scene builds");

    // Spawn prim entities and run the USD→component bridge exactly as
    // `process_usd_sim_prim_read` does.
    let prims = [
        ("/Site", "Site"),
        ("/Site/Mast", "Mast"),
        ("/Site/RelaySat", "RelaySat"),
        ("/Site/EarthStation", "EarthStation"),
    ];
    let mut entities = Vec::new();
    for (path, name) in prims {
        let e = app
            .world_mut()
            .spawn((
                UsdPrimPath { stage_handle: handle.clone(), path: path.into() },
                Name::new(name),
                Transform::default(),
            ))
            .id();
        entities.push((e, name));
    }
    {
        let world = app.world_mut();
        let stages = world
            .remove_non_send_resource::<CanonicalStages>()
            .expect("stages");
        {
            let view_stage = stages.get(id).expect("stage");
            let view = view_stage.view();
            let mut queue = bevy::ecs::world::CommandQueue::default();
            let mut commands = Commands::new(&mut queue, world);
            for (e, _) in &entities {
                let path = world.get::<UsdPrimPath>(*e).unwrap().path.clone();
                let sdf = SdfPath::new(&path).unwrap();
                insert_celestial_comms_components(&view, *e, &path, &sdf, &mut commands);
            }
            drop(commands);
            queue.apply(world);
        }
        world.insert_non_send_resource(stages);
    }
    // Mast local height (site frame maps local Y=2 up).
    let mast = entities.iter().find(|(_, n)| *n == "Mast").unwrap().0;
    app.world_mut().get_mut::<Transform>(mast).unwrap().translation.y = 2.0;

    (app, entities)
}

fn entity(entities: &[(Entity, &str)], name: &str) -> Entity {
    entities.iter().find(|(_, n)| *n == name).unwrap().0
}

#[test]
fn usd_attrs_bridge_to_components_and_links_connect() {
    let (mut app, entities) = build_app();
    app.update(); // update_comms_links runs (first run: last_jd 0 → computes)

    let site = entity(&entities, "Site");
    let mast = entity(&entities, "Mast");
    let sat = entity(&entities, "RelaySat");
    let earth = entity(&entities, "EarthStation");

    // Bridge: components derived from the authored attrs.
    let world = app.world();
    let site_anchor = world.get::<GeodeticAnchor>(site).expect("site anchor");
    assert_eq!(site_anchor.body, 301);
    assert_eq!(site_anchor.geodetic.lon_deg, 180.0);
    assert!(world.get::<SiteAnchor>(site).is_some(), "root prim anchor = site");
    assert!(world.get::<CommsAntenna>(mast).is_some());
    let orbit = world.get::<KeplerOrbit>(sat).expect("orbit from lunco:orbit:*");
    assert_eq!(orbit.body, 301);
    assert_eq!(orbit.elements.semi_major_axis_m, 3_000_000.0);
    let earth_anchor = world.get::<GeodeticAnchor>(earth).expect("earth anchor");
    assert_eq!(earth_anchor.body, 399);
    assert!(world.get::<SiteAnchor>(earth).is_none(), "nested prim is not the site");

    // Links: near-side site — everything sees everything.
    let mast_state = world.get::<CommsLinkState>(mast).expect("mast link state");
    let peer = |state: &CommsLinkState, name: &str| -> bool {
        state.peers.iter().find(|p| p.peer == name).expect(name).connected
    };
    assert!(peer(mast_state, "RelaySat"), "mast sees the relay: {mast_state:?}");
    assert!(peer(mast_state, "EarthStation"), "near-side mast sees Earth directly");
    assert_eq!(mast_state.earth_hops, Some(1), "one hop to the Earth station");
    let earth_state = world.get::<CommsLinkState>(earth).unwrap();
    assert_eq!(earth_state.earth_hops, Some(0), "the Earth station IS the route root");

    // Ports: published through the registry backend.
    let registry = world.resource::<PortRegistry>().clone();
    assert_eq!(
        registry.read_output_port(world, mast, "comms:route_earth:connected"),
        Some(1.0)
    );
    assert_eq!(
        registry.read_output_port(world, mast, "comms:relaysat:connected"),
        Some(1.0)
    );
    let range = registry
        .read_output_port(world, mast, "comms:earthstation:range_m")
        .expect("range port");
    assert!(
        (range - 384_400.0e3).abs() < 30_000.0e3,
        "mast↔Earth range ≈ the Earth–Moon distance, got {range}"
    );
    let ports = registry.entity_ports(world, mast);
    assert!(
        ports.iter().any(|p| p.name == "comms:relaysat:elevation_deg"),
        "surface antenna lists elevation ports: {ports:?}"
    );
}

/// Moving the site around the limb (lon 90 → local up points −Z, away from
/// the Earth line) kills the direct mast↔Earth link (below horizon), while a
/// relay parked overhead on the −Z side still sees BOTH endpoints: the route
/// becomes mast → sat → Earth = 2 hops.
#[test]
fn limb_site_routes_through_the_relay() {
    let (mut app, entities) = build_app();
    let site = entity(&entities, "Site");
    app.update();

    // Site on the −Z limb (up = −Z); relay directly overhead at 20 000 km
    // (mean anomaly 90° on the equatorial circle → Moon + (0, 0, −a)), which
    // is also high above the Earth–Moon line for the ground station.
    {
        let mut anchor = app.world_mut().get_mut::<GeodeticAnchor>(site).unwrap();
        anchor.geodetic.lon_deg = 90.0;
    }
    let sat = entity(&entities, "RelaySat");
    {
        let mut orbit = app.world_mut().get_mut::<KeplerOrbit>(sat).unwrap();
        orbit.elements.semi_major_axis_m = 20_000.0e3;
        orbit.elements.mean_anomaly_deg = 90.0;
    }
    app.update();

    let world = app.world();
    let mast = entity(&entities, "Mast");
    let state = world.get::<CommsLinkState>(mast).expect("state");
    let earth_link = state.peers.iter().find(|p| p.peer == "EarthStation").unwrap();
    assert!(
        !earth_link.connected,
        "limb mast must NOT see Earth directly: {earth_link:?}"
    );
    let sat_link = state.peers.iter().find(|p| p.peer == "RelaySat").unwrap();
    assert!(sat_link.connected, "limb mast sees the overhead relay: {sat_link:?}");
    assert_eq!(
        state.earth_hops,
        Some(2),
        "route = mast → relay → Earth station: {state:?}"
    );
}
