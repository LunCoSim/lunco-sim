use avian3d::{
    math::{Quaternion as AvianQuaternion, Scalar, Vector},
    prelude::*,
};
use bevy::ecs::system::SystemParam;
use bevy::math::DVec3;
use bevy::prelude::*;

const STATIC_FRICTION_MAX_SLIP_SPEED_MPS: Scalar = 0.02;

/// This fixture intentionally preserves the high-resolution numerical
/// reproduction budget. It is not the production solver contract; the
/// luncosim path gets its eight substeps from `lunco_physics`.
const NUMERICAL_REPRODUCTION_SUBSTEPS: u32 = 32;

fn native_prismatic_drive(stiffness: Scalar, damping: Scalar, max_force: Scalar) -> LinearMotor {
    LinearMotor::new(MotorModel::ForceBased { stiffness, damping })
        .with_target_position(0.0)
        .with_max_force(max_force)
}

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
    app.insert_resource(SubstepCount(NUMERICAL_REPRODUCTION_SUBSTEPS));

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
fn four_raked_legs_do_not_create_unbounded_energy() {
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
    app.insert_resource(SubstepCount(NUMERICAL_REPRODUCTION_SUBSTEPS));

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
                Transform::from_translation(mount.as_vec3()).with_rotation(body_rotation),
                ChildOf(hull),
            ))
            .id();
        app.world_mut().spawn((
            PrismaticJoint::new(hull, leg)
                .with_local_anchor1(mount)
                .with_local_anchor2(DVec3::ZERO)
                .with_local_basis1(body_rotation_d)
                .with_local_basis2(AvianQuaternion::IDENTITY)
                .with_slider_axis(DVec3::Y)
                .with_limits(-0.8, 0.0)
                .with_motor(native_prismatic_drive(1_800_000.0, 90_000.0, 42_000.0)),
            JointCollisionDisabled,
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
        velocity.length() < 0.1,
        "native four-leg assembly became unstable: linear velocity {velocity:?}"
    );
    assert!(
        angular_velocity.length() < 0.05,
        "native four-leg assembly became unstable: angular velocity {angular_velocity:?}"
    );
}

/// The USD lander does not use a compound leg collider: each foot is its own
/// dynamic body on a spherical joint. Keep that topology in the native
/// reproduction as well. A compound collider can pass while the real
/// leg--ball--pad contact graph still injects energy.
#[test]
fn four_ball_jointed_pads_do_not_create_unbounded_energy() {
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
    app.insert_resource(SubstepCount(NUMERICAL_REPRODUCTION_SUBSTEPS));

    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(100.0, 0.1, 100.0),
        Friction {
            dynamic_coefficient: 1.0,
            static_coefficient: 1.0,
            combine_rule: CoefficientCombine::Min,
        },
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
                Mass(30.0),
                AngularInertia::new(Vec3::new(124.30, 0.0844, 124.30)),
                CenterOfMass::new(0.0, -3.525, 0.0),
                NoAutoMass,
                NoAutoAngularInertia,
                NoAutoCenterOfMass,
                Transform::from_xyz(mount.x as f32, (8.0 + mount.y) as f32, mount.z as f32)
                    .with_rotation(body_rotation),
            ))
            .id();
        let pad = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Mass(10.0),
                AngularInertia::new(Vec3::splat(10.0)),
                NoAutoMass,
                NoAutoAngularInertia,
                Collider::cylinder(0.455, 0.1),
                Friction {
                    dynamic_coefficient: 0.60,
                    static_coefficient: 0.65,
                    combine_rule: CoefficientCombine::Min,
                },
                ActiveCollisionHooks::MODIFY_CONTACTS,
                Transform::from_xyz(0.0, -7.2, 0.0).with_rotation(body_rotation.inverse()),
                ChildOf(leg),
            ))
            .id();
        app.world_mut().spawn((
            SphericalJoint::new(leg, pad)
                .with_local_anchor1(if axis.x > 0.0 {
                    DVec3::new(0.017101, -7.153015, 0.0)
                } else if axis.x < 0.0 {
                    DVec3::new(-0.017101, -7.153015, 0.0)
                } else if axis.z > 0.0 {
                    DVec3::new(0.0, -7.153015, 0.017101)
                } else {
                    DVec3::new(0.0, -7.153015, -0.017101)
                })
                .with_local_anchor2(DVec3::new(0.0, 0.05, 0.0)),
            JointCollisionDisabled,
            JointDamping {
                linear: 0.0,
                angular: 2.5,
            },
        ));
        app.world_mut().spawn((
            PrismaticJoint::new(hull, leg)
                .with_local_anchor1(mount)
                .with_local_anchor2(DVec3::ZERO)
                .with_local_basis1(body_rotation_d)
                .with_local_basis2(AvianQuaternion::IDENTITY)
                .with_slider_axis(DVec3::Y)
                .with_limits(-0.8, 0.0)
                .with_motor(native_prismatic_drive(1_800_000.0, 90_000.0, 42_000.0)),
            JointCollisionDisabled,
        ));
    }

    app.finish();
    app.cleanup();
    for _ in 0..1800 {
        app.update();
    }

    let velocity = app.world().get::<LinearVelocity>(hull).unwrap().0;
    let angular_velocity = app.world().get::<AngularVelocity>(hull).unwrap().0;
    println!("four-ball-pad settle: linear={velocity:?}, angular={angular_velocity:?}");
    assert!(
        velocity.length() < 0.1,
        "ball-jointed pad assembly became unstable: linear velocity {velocity:?}"
    );
    assert!(
        angular_velocity.length() < 0.25,
        "ball-jointed pad assembly became unstable: angular velocity {angular_velocity:?}"
    );
}

/// The production simulator installs the USD contact hook so authored
/// static/dynamic friction is honored. Keep that integration in the same
/// mechanical reproduction: the native assembly test above proves Avian's
/// solver, while this proves the production hook does not inject energy.
#[test]
fn four_raked_legs_with_usd_contact_hook_do_not_create_unbounded_energy() {
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
    app.insert_resource(SubstepCount(NUMERICAL_REPRODUCTION_SUBSTEPS));

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
                .with_limits(-0.8, 0.0)
                .with_motor(native_prismatic_drive(10_000.0, 6_500.0, 30_000.0)),
            JointCollisionDisabled,
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
        velocity.length() < 0.1,
        "USD contact hook made the native four-leg assembly unstable: linear velocity {velocity:?}"
    );
    assert!(
        angular_velocity.length() < 0.05,
        "USD contact hook made the native four-leg assembly unstable: angular velocity {angular_velocity:?}"
    );
}
