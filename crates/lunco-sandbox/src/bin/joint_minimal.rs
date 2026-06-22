//! Avian joint **showcase** + gizmo playground.
//!
//! All five Avian joint types laid out in a row, each connecting a
//! kinematic anchor to a dynamic slave so you can drag either body
//! with the transform gizmo and watch the constraint do its thing.
//! Plus one "freely rotating in air" cube — kinematic body with an
//! `AngularVelocity`, no joint, no gravity.
//!
//! ```text
//! Layout (top-down, Y is up — anchors at y=5, slaves at y=5, z=+2):
//!
//!   FREE_ROT             FixedJoint  RevoluteJoint  PrismaticJoint  DistanceJoint  SphericalJoint
//!     (kine                ●              ●               ●               ●               ●
//!      spinning)           |              |               |               |               |
//!                          ○              ○               ○               ○               ○
//!                       (slave)         (slave)         (slave)         (slave)         (slave)
//!
//!   x = 0 (back)        x=-16          x=-8            x=0             x=8             x=16
//! ```
//!
//! Run: `cargo run --bin joint_minimal -- --api 3001`
//!
//! - `MoveEntity` API command teleports any body, with the
//!   `JustMovedKinematic` velocity-pulse pattern so joint-coupled
//!   bodies follow.
//! - Click a body in the window, drag with the gizmo — same pulse
//!   + zero behavior.

use bevy::prelude::*;
use avian3d::prelude::*;
use avian3d::physics_transform::Position;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        // `PhysicsPlugins::default()` includes the joint constraint
        // solver automatically when the `xpbd_joints` feature is on
        // (see workspace `Cargo.toml`).
        .add_plugins(PhysicsPlugins::default())
        .add_plugins(lunco_core::LunCoCorePlugin)
        .add_plugins(lunco_api::LunCoApiPlugin::new(lunco_api::LunCoApiConfig::from_args()))
        .add_plugins(lunco_sandbox_edit::SandboxEditPlugin)
        .insert_resource(SubstepCount(8))
        .add_systems(Startup, setup)
        .add_systems(Update, log_positions)
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    use bevy::math::DVec3;

    // Camera positioned to view the row of stations.
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 12.0, 22.0).looking_at(Vec3::new(0.0, 5.0, 0.0), Vec3::Y),
    ));

    commands.spawn((
        DirectionalLight { illuminance: 8000.0, shadows_enabled: false, ..default() },
        Transform::default().looking_at(Vec3::new(-1.0, -2.5, -1.5), Vec3::Y),
    ));

    // Ground.
    let ground_mesh = meshes.add(Cuboid::new(80.0, 0.5, 40.0));
    let ground_mat = mats.add(Color::srgb(0.40, 0.40, 0.45));
    commands.spawn((
        Mesh3d(ground_mesh),
        MeshMaterial3d(ground_mat),
        Transform::from_xyz(0.0, -0.25, 0.0),
        RigidBody::Static,
        Collider::cuboid(80.0, 0.5, 40.0),
        Name::new("Ground"),
    ));

    let cube_mesh = meshes.add(Cuboid::from_length(1.0));
    let anchor_mat = mats.add(Color::srgb(0.85, 0.85, 0.90));   // light grey
    let slave_mats = [
        mats.add(Color::srgb(0.95, 0.30, 0.30)),  // red — Fixed
        mats.add(Color::srgb(0.95, 0.65, 0.20)),  // orange — Revolute
        mats.add(Color::srgb(0.95, 0.90, 0.20)),  // yellow — Prismatic
        mats.add(Color::srgb(0.30, 0.85, 0.30)),  // green — Distance
        mats.add(Color::srgb(0.30, 0.55, 0.95)),  // blue — Spherical
    ];

    // === FREE-ROTATION DEMO (no joint) ===
    // Kinematic body with constant AngularVelocity — proves a body
    // can be pinned in space yet still rotate. This is the Avian
    // pattern: kinematic state is driven by *velocity*, not by
    // teleporting Position each frame.
    commands.spawn((
        Mesh3d(cube_mesh.clone()),
        MeshMaterial3d(mats.add(Color::srgb(0.85, 0.55, 0.85))),
        Transform::from_xyz(-22.0, 7.0, 0.0).with_scale(Vec3::splat(1.5)),
        RigidBody::Kinematic,
        AngularVelocity(DVec3::new(0.5, 1.0, 0.3)),
        Collider::cuboid(1.0, 1.0, 1.0),
        Name::new("FreeRotator"),
    ));

    // === Five joint stations ===
    let station_x = [-16.0_f32, -8.0, 0.0, 8.0, 16.0];
    let station_names = ["Fixed", "Revolute", "Prismatic", "Distance", "Spherical"];

    for (i, (&x, &name)) in station_x.iter().zip(station_names.iter()).enumerate() {
        // Anchor — kinematic cube at y=5, doesn't fall.
        let anchor = commands.spawn((
            Mesh3d(cube_mesh.clone()),
            MeshMaterial3d(anchor_mat.clone()),
            Transform::from_xyz(x, 5.0, 0.0),
            RigidBody::Kinematic,
            Collider::cuboid(1.0, 1.0, 1.0),
            Mass(1.0),
            Name::new(format!("Anchor_{}", name)),
        )).id();

        // Slave — dynamic cube 2m forward of anchor.
        let slave = commands.spawn((
            Mesh3d(cube_mesh.clone()),
            MeshMaterial3d(slave_mats[i].clone()),
            Transform::from_xyz(x, 5.0, 2.0),
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            // Full mass properties (Mass + AngularInertia + CenterOfMass)
            // — Avian's joint solver needs the inertia tensor to compute
            // correct impulses, not just scalar mass.
            MassPropertiesBundle::from_shape(&Cuboid::from_length(1.0), 1.0),
            Name::new(format!("Slave_{}", name)),
        )).id();

        // Joint anchors meet at world (x, 5, 1) — midpoint between
        // the two cubes along z.
        let local_a = DVec3::new(0.0, 0.0, 1.0);   // +Z half-distance from anchor
        let local_s = DVec3::new(0.0, 0.0, -1.0);  // -Z half-distance from slave

        match i {
            0 => {
                // FixedJoint — locks all 6 DOF. Slave moves rigidly
                // with the anchor.
                commands.spawn((
                    FixedJoint::new(anchor, slave)
                        .with_local_anchor1(local_a)
                        .with_local_anchor2(local_s),
                    Name::new("Joint_Fixed"),
                ));
            }
            1 => {
                // RevoluteJoint — single rotation axis (around Y).
                // Slave can swing/rotate around the joint point about
                // the vertical axis but the position stays attached.
                commands.spawn((
                    RevoluteJoint::new(anchor, slave)
                        .with_local_anchor1(local_a)
                        .with_local_anchor2(local_s)
                        .with_hinge_axis(DVec3::Y),
                    Name::new("Joint_Revolute"),
                ));
            }
            2 => {
                // PrismaticJoint — single translation axis (Z).
                // Slave can slide toward/away from anchor along Z
                // (within limits) but no rotation, no other axes.
                commands.spawn((
                    PrismaticJoint::new(anchor, slave)
                        .with_local_anchor1(local_a)
                        .with_local_anchor2(local_s)
                        .with_slider_axis(DVec3::Z)
                        .with_limits(-1.5, 1.5),
                    Name::new("Joint_Prismatic"),
                ));
            }
            3 => {
                // DistanceJoint — keep two bodies at a fixed distance
                // (2 m here). Slave can rotate freely around the
                // anchor like a tethered ball.
                commands.spawn((
                    DistanceJoint::new(anchor, slave)
                        .with_local_anchor1(DVec3::ZERO)
                        .with_local_anchor2(DVec3::ZERO)
                        .with_limits(2.0, 2.0),
                    Name::new("Joint_Distance"),
                ));
            }
            4 => {
                // SphericalJoint — ball joint. Locks translation
                // (anchors stay at the joint point) but rotation is
                // unconstrained on all three axes. Slave dangles
                // and swings freely.
                commands.spawn((
                    SphericalJoint::new(anchor, slave)
                        .with_local_anchor1(local_a)
                        .with_local_anchor2(local_s),
                    Name::new("Joint_Spherical"),
                ));
            }
            _ => unreachable!(),
        }
    }

    info!("Joint showcase up. Drag any cube with the gizmo to test constraint propagation.");
}

/// Logs slave positions every 1.0s so you can watch joint behavior
/// from the terminal without polling QueryEntity.
fn log_positions(
    time: Res<Time>,
    mut last: Local<f32>,
    q: Query<(&Name, &Position)>,
) {
    let now = time.elapsed_secs();
    if now - *last < 1.0 { return; }
    *last = now;
    for (name, pos) in q.iter() {
        let n = name.as_str();
        if n.starts_with("Slave_") || n == "FreeRotator" {
            info!("[{:.1}s] {} pos=({:.2}, {:.2}, {:.2})", now, n, pos.0.x, pos.0.y, pos.0.z);
        }
    }
}
