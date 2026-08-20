use avian3d::{
    math::{Quaternion as AvianQuaternion, Scalar, Vector},
    prelude::*,
};
use bevy::ecs::system::SystemParam;
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_cosim::PassivePrismaticSuspension;

const STATIC_FRICTION_MAX_SLIP_SPEED_MPS: Scalar = 0.02;

#[derive(SystemParam)]
struct StaticFrictionContactHook<'w, 's> {
    friction: Query<'w, 's, &'static Friction>,
    linear_velocity: Query<'w, 's, &'static LinearVelocity>,
    angular_velocity: Query<'w, 's, &'static AngularVelocity>,
    default_friction: Res<'w, DefaultFriction>,
}

impl CollisionHooks for StaticFrictionContactHook<'_, '_> {
    fn modify_contacts(&self, contacts: &mut ContactPair, _commands: &mut Commands) -> bool {
        let friction = |entity: Entity| {
            self.friction
                .get(entity)
                .copied()
                .unwrap_or(self.default_friction.0)
        };
        let combined = friction(contacts.collider1).combine(friction(contacts.collider2));
        let point_velocity = |body: Option<Entity>, anchor: Vector| {
            let Some(body) = body else {
                return Vector::ZERO;
            };
            let linear = self
                .linear_velocity
                .get(body)
                .map_or(Vector::ZERO, |velocity| velocity.0);
            let angular = self
                .angular_velocity
                .get(body)
                .map_or(Vector::ZERO, |velocity| velocity.0);
            linear + angular.cross(anchor)
        };

        for manifold in &mut contacts.manifolds {
            let max_tangent_speed = manifold
                .points
                .iter()
                .fold(0.0 as Scalar, |maximum, point| {
                    let relative = point_velocity(contacts.body2, point.anchor2)
                        - point_velocity(contacts.body1, point.anchor1);
                    let tangent = relative - manifold.normal * relative.dot(manifold.normal);
                    maximum.max(tangent.length())
                });
            manifold.friction = if max_tangent_speed <= STATIC_FRICTION_MAX_SLIP_SPEED_MPS {
                combined.static_coefficient
            } else {
                combined.dynamic_coefficient
            };
        }
        true
    }
}

#[test]
fn a_static_flat_friction_contact_does_not_self_accelerate() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        bevy::diagnostic::DiagnosticsPlugin,
        TransformPlugin,
        PhysicsPlugins::default(),
    ));
    app.init_asset::<Mesh>();
    app.insert_resource(Gravity(DVec3::new(0.0, -1.62, 0.0)));
    app.insert_resource(Time::<Fixed>::from_hz(60.0));
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / 60.0),
    ));
    app.insert_resource(SubstepCount(32));
    lunco_cosim::install_passive_prismatic_solver(&mut app);

    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(20.0, 0.5, 20.0),
        Friction::new(1.0),
        Transform::from_xyz(0.0, -0.25, 0.0),
    ));
    let cube = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 0.5, 1.0),
            Friction::new(0.6),
            Transform::from_xyz(0.0, 0.75, 0.0),
        ))
        .id();

    app.finish();
    app.cleanup();

    for _ in 0..600 {
        app.update();
    }

    let velocity = app.world().get::<LinearVelocity>(cube).unwrap().0;
    // XPBD's finite contact iterations leave micrometre-per-second-scale
    // roundoff in a long run. This is far below visible or physical slip; the
    // regression is for self-acceleration, not bitwise zero.
    assert!(
        velocity.length() < 1e-5,
        "static contact self-accelerated: {velocity:?}"
    );
}

/// The landing assembly is four independent raked sliders sharing one hull.
/// This is deliberately a native Avian reproduction: no USD loader, floating
/// origin, Modelica plant, or recording code is involved.  If this test drifts,
/// the error is in the mechanism/contact contract itself.
#[test]
fn four_raked_passive_legs_do_not_create_horizontal_energy() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        TransformPlugin,
        PhysicsPlugins::default(),
    ));
    app.init_asset::<Mesh>();
    app.insert_resource(Gravity(Vector::new(0.0, -1.6248896, 0.0)));
    app.insert_resource(Time::<Fixed>::from_hz(60.0));
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / 60.0),
    ));
    app.insert_resource(SubstepCount(32));
    lunco_cosim::install_passive_prismatic_solver(&mut app);

    let ground_friction = Friction {
        dynamic_coefficient: 1.0,
        static_coefficient: 1.0,
        combine_rule: CoefficientCombine::Min,
    };
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(100.0, 0.1, 100.0),
        ground_friction,
        Transform::from_xyz(0.0, -0.05, 0.0),
    ));

    let hull = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Mass(4000.0),
            AngularInertia::new(Vec3::new(5905.0, 6250.0, 5905.0)),
            CenterOfMass::ZERO,
            NoAutoMass,
            NoAutoAngularInertia,
            NoAutoCenterOfMass,
            Collider::cuboid(2.5, 1.5, 2.5),
            Transform::from_xyz(0.0, 8.0, 0.0),
        ))
        .id();

    let angle = 20.0_f64.to_radians();
    let sin = angle.sin();
    let cos = angle.cos();
    let leg_specs = [
        (DVec3::new(sin, -cos, 0.0), DVec3::new(2.519, 1.388, 0.0)),
        (DVec3::new(-sin, -cos, 0.0), DVec3::new(-2.519, 1.388, 0.0)),
        (DVec3::new(0.0, -cos, sin), DVec3::new(0.0, 1.388, 2.519)),
        (DVec3::new(0.0, -cos, -sin), DVec3::new(0.0, 1.388, -2.519)),
    ];

    for (axis, mount) in leg_specs {
        let body_rotation = if axis.z.abs() > 0.0 {
            Quat::from_rotation_x(if axis.z > 0.0 {
                -angle as f32
            } else {
                angle as f32
            })
        } else {
            Quat::from_rotation_z(if axis.x > 0.0 {
                angle as f32
            } else {
                -angle as f32
            })
        };
        let body_rotation_d = AvianQuaternion::from_xyzw(
            body_rotation.x as f64,
            body_rotation.y as f64,
            body_rotation.z as f64,
            body_rotation.w as f64,
        );
        let leg = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Mass(40.0),
                AngularInertia::new(Vec3::new(226.3688, 1.46454, 226.3688)),
                CenterOfMass::new(0.0, -4.44375, 0.0),
                NoAutoMass,
                NoAutoAngularInertia,
                NoAutoCenterOfMass,
                Collider::compound(vec![(
                    DVec3::new(0.0, -7.2, 0.0),
                    body_rotation_d.inverse(),
                    Collider::cuboid(0.455, 0.05, 0.455),
                )]),
                Friction {
                    dynamic_coefficient: 0.60,
                    static_coefficient: 0.65,
                    combine_rule: CoefficientCombine::Min,
                },
                Transform::from_xyz(mount.x as f32, (8.0 + mount.y) as f32, mount.z as f32)
                    .with_rotation(body_rotation),
            ))
            .id();
        app.world_mut().spawn((
            PrismaticJoint::new(hull, leg)
                .with_local_anchor1(mount)
                .with_local_anchor2(DVec3::ZERO)
                .with_local_basis1(body_rotation_d)
                .with_local_basis2(AvianQuaternion::IDENTITY)
                .with_slider_axis(DVec3::Y)
                .with_limits(-0.8, 0.0),
            JointCollisionDisabled,
            PassivePrismaticSuspension {
                body1: hull,
                body2: leg,
                rest_position: 0.0,
                plastic_position: 0.0,
                spring_k: 10_000.0,
                damping_c: 6_500.0,
                yield_force: 8_000.0,
                max_force: 30_000.0,
                reaction_force: 0.0,
            },
        ));
    }

    app.finish();
    app.cleanup();
    for _ in 0..1800 {
        app.update();
    }

    let velocity = app.world().get::<LinearVelocity>(hull).unwrap().0;
    let angular_velocity = app.world().get::<AngularVelocity>(hull).unwrap().0;
    println!("four-leg native settle: linear={velocity:?}, angular={angular_velocity:?}");
    assert!(
        velocity.length() < 0.03,
        "native four-leg assembly drifted: linear velocity {velocity:?}"
    );
    assert!(
        angular_velocity.length() < 0.01,
        "native four-leg assembly rotated: angular velocity {angular_velocity:?}"
    );
}

/// The production simulator installs the USD contact hook so authored
/// static/dynamic friction is honored. Keep that integration in the same
/// mechanical reproduction: the native assembly test above proves Avian's
/// solver, while this proves the production hook does not inject energy.
#[test]
fn four_raked_passive_legs_with_usd_contact_hook_do_not_create_horizontal_energy() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        TransformPlugin,
        PhysicsPlugins::default().with_collision_hooks::<StaticFrictionContactHook>(),
    ));
    app.init_asset::<Mesh>();
    app.insert_resource(Gravity(Vector::new(0.0, -1.6248896, 0.0)));
    app.insert_resource(Time::<Fixed>::from_hz(60.0));
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / 60.0),
    ));
    app.insert_resource(SubstepCount(32));
    lunco_cosim::install_passive_prismatic_solver(&mut app);

    let ground_friction = Friction {
        dynamic_coefficient: 1.0,
        static_coefficient: 1.0,
        combine_rule: CoefficientCombine::Min,
    };
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(100.0, 0.1, 100.0),
        ground_friction,
        ActiveCollisionHooks::MODIFY_CONTACTS,
        Transform::from_xyz(0.0, -0.05, 0.0),
    ));

    let hull = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Mass(4000.0),
            AngularInertia::new(Vec3::new(5905.0, 6250.0, 5905.0)),
            CenterOfMass::ZERO,
            NoAutoMass,
            NoAutoAngularInertia,
            NoAutoCenterOfMass,
            Collider::cuboid(2.5, 1.5, 2.5),
            Transform::from_xyz(0.0, 8.0, 0.0),
        ))
        .id();

    let angle = 20.0_f64.to_radians();
    let sin = angle.sin();
    let cos = angle.cos();
    let leg_specs = [
        (DVec3::new(sin, -cos, 0.0), DVec3::new(2.519, 1.388, 0.0)),
        (DVec3::new(-sin, -cos, 0.0), DVec3::new(-2.519, 1.388, 0.0)),
        (DVec3::new(0.0, -cos, sin), DVec3::new(0.0, 1.388, 2.519)),
        (DVec3::new(0.0, -cos, -sin), DVec3::new(0.0, 1.388, -2.519)),
    ];

    for (axis, mount) in leg_specs {
        let body_rotation = if axis.z.abs() > 0.0 {
            Quat::from_rotation_x(if axis.z > 0.0 {
                -angle as f32
            } else {
                angle as f32
            })
        } else {
            Quat::from_rotation_z(if axis.x > 0.0 {
                angle as f32
            } else {
                -angle as f32
            })
        };
        let body_rotation_d = AvianQuaternion::from_xyzw(
            body_rotation.x as f64,
            body_rotation.y as f64,
            body_rotation.z as f64,
            body_rotation.w as f64,
        );
        let leg = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Mass(40.0),
                AngularInertia::new(Vec3::new(226.3688, 1.46454, 226.3688)),
                CenterOfMass::new(0.0, -4.44375, 0.0),
                NoAutoMass,
                NoAutoAngularInertia,
                NoAutoCenterOfMass,
                Collider::compound(vec![(
                    DVec3::new(0.0, -7.2, 0.0),
                    body_rotation_d.inverse(),
                    Collider::cuboid(0.455, 0.05, 0.455),
                )]),
                Friction {
                    dynamic_coefficient: 0.60,
                    static_coefficient: 0.65,
                    combine_rule: CoefficientCombine::Min,
                },
                ActiveCollisionHooks::MODIFY_CONTACTS,
                Transform::from_xyz(mount.x as f32, (8.0 + mount.y) as f32, mount.z as f32)
                    .with_rotation(body_rotation),
            ))
            .id();
        app.world_mut().spawn((
            PrismaticJoint::new(hull, leg)
                .with_local_anchor1(mount)
                .with_local_anchor2(DVec3::ZERO)
                .with_local_basis1(body_rotation_d)
                .with_local_basis2(AvianQuaternion::IDENTITY)
                .with_slider_axis(DVec3::Y)
                .with_limits(-0.8, 0.0),
            JointCollisionDisabled,
            PassivePrismaticSuspension {
                body1: hull,
                body2: leg,
                rest_position: 0.0,
                plastic_position: 0.0,
                spring_k: 10_000.0,
                damping_c: 6_500.0,
                yield_force: 8_000.0,
                max_force: 30_000.0,
                reaction_force: 0.0,
            },
        ));
    }

    app.finish();
    app.cleanup();
    for _ in 0..1800 {
        app.update();
    }

    let velocity = app.world().get::<LinearVelocity>(hull).unwrap().0;
    let angular_velocity = app.world().get::<AngularVelocity>(hull).unwrap().0;
    println!("four-leg native USD-hook settle: linear={velocity:?}, angular={angular_velocity:?}");
    assert!(
        velocity.length() < 0.03,
        "USD contact hook made the native four-leg assembly drift: linear velocity {velocity:?}"
    );
    assert!(
        angular_velocity.length() < 0.01,
        "USD contact hook made the native four-leg assembly rotate: angular velocity {angular_velocity:?}"
    );
}
