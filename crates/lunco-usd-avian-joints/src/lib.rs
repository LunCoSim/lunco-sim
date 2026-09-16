//! Generic Avian joint admission and lifecycle.
//!
//! This package owns the native constraint boundary shared by authored USD
//! joints and synthesized mechanisms. A caller supplies bodies and a typed
//! joint plan; the package filters the pair immediately, parks the constraint,
//! and admits it only after Avian has created the corresponding solver nodes.
//! The USD projector remains responsible for reading and validating authored
//! relationships, frames, limits, and drives.

use avian3d::dynamics::solver::islands::PhysicsIslands;
use avian3d::dynamics::solver::joint_graph::JointGraph;
use avian3d::physics_transform::{Position, Rotation};
use avian3d::prelude::*;
use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::schedule::common_conditions::any_with_component;
use bevy::ecs::system::SystemState;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use lunco_usd_avian_contracts::{AuthoredInitialVelocity, PendingJointAdmission};
use lunco_usd_avian_filters::filtered_pairs as collision_filters;

const JOINT_SEAT_EPS: f64 = 1.0e-3;
const JOINT_SEAT_ANGLE_EPS: f64 = 1.0e-3;
const JOINT_SEAT_ERROR_THRESHOLD: f64 = 0.1;

/// Retire Avian joint-graph edges before their ECS entities are removed.
///
/// Avian's component hooks normally maintain the graph. Scene teardown and
/// live detach need a stronger transaction, however: a recursive despawn can
/// remove a native joint, its collision marker, and its bodies in an order
/// that asks the island manager to unlink the same edge twice. Retiring the
/// edge while all bodies are still present makes subsequent component removal
/// harmless.
pub fn retire_joint_graph_edges(world: &mut World, entities: &[Entity]) {
    let mut graph_state: SystemState<(
        ResMut<PhysicsIslands>,
        ResMut<JointGraph>,
        Res<ContactGraph>,
        Query<
            &'static mut avian3d::dynamics::solver::islands::BodyIslandNode,
            Or<(With<Disabled>, Without<Disabled>)>,
        >,
    )> = SystemState::new(world);
    {
        let Ok((mut islands, mut joint_graph, contact_graph, mut body_islands)) =
            graph_state.get_mut(world)
        else {
            error!(
                "joint graph retirement blocked: Avian graph resources have conflicting access; keeping the topology intact"
            );
            return;
        };

        for &entity in entities {
            let Some(edge) = joint_graph.get(entity).cloned() else {
                continue;
            };
            let island_id = edge.island.island_id();
            if island_id != avian3d::dynamics::solver::islands::IslandId::PLACEHOLDER {
                let Some(island) = islands.get(island_id) else {
                    warn!(
                        "joint graph edge {:?} references missing island {:?}; edge is already outside the island list",
                        entity, island_id
                    );
                    joint_graph.remove_joint(entity);
                    continue;
                };
                if island.joint_count() > 0 {
                    let _ = islands.remove_joint(
                        edge.id,
                        &mut body_islands,
                        &contact_graph,
                        &mut joint_graph,
                    );
                } else {
                    warn!(
                        "joint graph edge {:?} has no island joint count; treating it as already retired",
                        entity
                    );
                }
            }
            joint_graph.remove_joint(entity);
        }
    }
    graph_state.apply(world);
}

/// A constructed native constraint that has not yet become an ECS component.
///
/// The inner value is private so callers can only hand a plan to
/// [`attach_joint`]. This keeps collision filtering and solver-island
/// admission coupled to every supported construction path.
pub struct JointSpec<J: Component + Clone> {
    joint: J,
    seat: Option<JointSeat>,
}

impl<J: Component + Clone> JointSpec<J> {
    fn new(joint: J) -> Self {
        Self { joint, seat: None }
    }

    fn with_seat(mut self, seat: JointSeat) -> Self {
        self.seat = Some(seat);
        self
    }
}

/// A joint that is parked until Avian has admitted both endpoint bodies.
#[derive(Component, Clone, Debug)]
pub struct PendingJoint<J: Component + Clone> {
    /// First jointed body.
    pub body0: Entity,
    /// Second jointed body.
    pub body1: Entity,
    /// Native constraint to install at admission.
    pub joint: J,
    seat: Option<JointSeat>,
}

/// The schedule set that installs parked native constraints.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct JointAdmission;

/// Plugin that owns native joint admission and solver-safe detach.
pub struct JointAttachPlugin;

impl Plugin for JointAttachPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(collision_filters::on_remove_joint_collision_pair);
        app.add_systems(Update, retire_requested_joints.before(JointAdmission));
        app.add_systems(
            Update,
            (
                admit_pending_joints::<RevoluteJoint>
                    .run_if(any_with_component::<PendingJoint<RevoluteJoint>>),
                admit_pending_joints::<PrismaticJoint>
                    .run_if(any_with_component::<PendingJoint<PrismaticJoint>>),
                admit_pending_joints::<FixedJoint>
                    .run_if(any_with_component::<PendingJoint<FixedJoint>>),
                admit_pending_joints::<SphericalJoint>
                    .run_if(any_with_component::<PendingJoint<SphericalJoint>>),
                admit_pending_joints::<DistanceJoint>
                    .run_if(any_with_component::<PendingJoint<DistanceJoint>>),
            )
                .in_set(JointAdmission),
        );
    }
}

/// Put a native joint behind the common collision-filter and admission gate.
pub fn attach_joint<J: Component + Clone>(
    commands: &mut Commands,
    joint_entity: Entity,
    body0: Entity,
    body1: Entity,
    joint: JointSpec<J>,
) {
    let JointSpec { joint, seat } = joint;
    collision_filters::filter_pair(commands, joint_entity, body0, body1);
    commands.entity(joint_entity).try_insert((
        collision_filters::JointCollisionPair { body0, body1 },
        PendingJoint {
            body0,
            body1,
            joint,
            seat,
        },
        PendingJointAdmission { body0, body1 },
        lunco_physics::PhysicsJointLink { body0, body1 },
        lunco_physics::PhysicsJointPending,
    ));
}

/// Install pending joints whose endpoint bodies are admitted to Avian's
/// solver graph. Static bodies are admitted by construction; a pair of two
/// static bodies is retained pending because it cannot constrain simulation.
pub fn admit_pending_joints<J: Component + Clone>(
    pending: Query<(Entity, &PendingJoint<J>)>,
    admitted: Query<(), With<avian3d::dynamics::solver::islands::BodyIslandNode>>,
    bodies: Query<&RigidBody>,
    mut q_pose: Query<(&mut Position, &mut Rotation)>,
    mut q_vel: Query<(&mut LinearVelocity, &mut AngularVelocity)>,
    q_authored_velocity: Query<&AuthoredInitialVelocity>,
    mut commands: Commands,
) {
    for (entity, p) in pending.iter() {
        let ready = |e: Entity| {
            admitted.contains(e) || bodies.get(e).map(RigidBody::is_static).unwrap_or(false)
        };
        if !ready(p.body0) || !ready(p.body1) {
            continue;
        }
        if !admitted.contains(p.body0) && !admitted.contains(p.body1) {
            continue;
        }
        if let Some(seat) = p.seat {
            seat_joint_bodies(
                "pending joint",
                p.body0,
                p.body1,
                seat,
                &mut q_pose,
                &mut q_vel,
                &q_authored_velocity,
                &mut commands,
            );
        }
        commands
            .entity(entity)
            .try_insert((p.joint.clone(), JointCollisionDisabled))
            .try_remove::<PendingJoint<J>>()
            .try_remove::<PendingJointAdmission>()
            .try_remove::<lunco_physics::PhysicsJointPending>();
    }
}

/// Retire a requested joint before removing its native components and entity.
fn retire_requested_joints(world: &mut World) {
    let requested: Vec<(Entity, Option<bevy::ecs::component::ComponentId>)> = {
        let mut query = world.query_filtered::<(
            Entity,
            Option<&avian3d::dynamics::solver::joint_graph::JointComponentId>,
        ), With<lunco_physics::PhysicsJointDetachRequested>>();
        query
            .iter(world)
            .map(|(entity, id)| (entity, id.and_then(|id| id.id())))
            .collect()
    };
    if requested.is_empty() {
        return;
    }

    if world.get_resource::<PhysicsIslands>().is_none()
        || world.get_resource::<JointGraph>().is_none()
        || world.get_resource::<ContactGraph>().is_none()
    {
        error!(
            "DETACH_JOINT blocked: Avian physics resources are not installed; add PhysicsPlugins before JointAttachPlugin"
        );
        return;
    }

    let entities: Vec<Entity> = requested.iter().map(|(entity, _)| *entity).collect();
    retire_joint_graph_edges(world, &entities);

    for (entity, component_id) in requested {
        let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
            continue;
        };
        if let Some(component_id) = component_id {
            entity_mut.remove_by_id(component_id);
        }
        entity_mut
            .remove::<avian3d::dynamics::solver::joint_graph::JointComponentId>()
            .remove::<JointDisabled>()
            .remove::<PendingJoint<RevoluteJoint>>()
            .remove::<PendingJoint<PrismaticJoint>>()
            .remove::<PendingJoint<FixedJoint>>()
            .remove::<PendingJoint<SphericalJoint>>()
            .remove::<PendingJoint<DistanceJoint>>()
            .remove::<PendingJointAdmission>()
            .remove::<lunco_physics::PhysicsJointPending>()
            .remove::<lunco_physics::PhysicsJointDetachRequested>()
            .remove::<collision_filters::JointCollisionPair>();
        entity_mut.despawn();
        info!(
            "DETACH_JOINT: retired solver edge and despawned {:?}",
            entity
        );
    }
}

/// A plain weld between two bodies at their origins.
pub fn fixed_joint(body0: Entity, body1: Entity) -> JointSpec<FixedJoint> {
    JointSpec::new(FixedJoint::new(body0, body1))
}

/// Build the wheel hinge and return it to the common admission gate.
pub fn wheel_revolute_joint(
    chassis: Entity,
    wheel: Entity,
    mount_local: DVec3,
    axle: DVec3,
) -> JointSpec<RevoluteJoint> {
    JointSpec::new(
        RevoluteJoint::new(chassis, wheel)
            .with_local_anchor1(mount_local)
            .with_local_anchor2(DVec3::ZERO)
            .with_hinge_axis(axle),
    )
    .with_seat(JointSeat::new(
        JointSeatKind::Revolute,
        mount_local,
        DVec3::ZERO,
        DQuat::IDENTITY,
        DQuat::IDENTITY,
        axle,
    ))
}

/// Attach a USD-authored fixed joint after its body endpoints have resolved.
pub fn attach_fixed_joint(
    commands: &mut Commands,
    joint_entity: Entity,
    body0: Entity,
    body1: Entity,
    local_pos0: DVec3,
    local_pos1: DVec3,
    local_rot0: DQuat,
    local_rot1: DQuat,
) {
    let joint = FixedJoint::new(body0, body1)
        .with_local_anchor1(local_pos0)
        .with_local_anchor2(local_pos1)
        .with_local_basis1(local_rot0)
        .with_local_basis2(local_rot1);
    attach_joint(
        commands,
        joint_entity,
        body0,
        body1,
        JointSpec::new(joint).with_seat(JointSeat::new(
            JointSeatKind::Fixed,
            local_pos0,
            local_pos1,
            local_rot0,
            local_rot1,
            DVec3::ZERO,
        )),
    );
}

/// Attach a USD-authored prismatic joint after its body endpoints resolve.
pub fn attach_prismatic_joint(
    commands: &mut Commands,
    joint_entity: Entity,
    body0: Entity,
    body1: Entity,
    local_pos0: DVec3,
    local_pos1: DVec3,
    local_rot0: DQuat,
    local_rot1: DQuat,
    axis: DVec3,
    lower: f64,
    upper: f64,
    motor: Option<LinearMotor>,
) {
    let mut joint = PrismaticJoint::new(body0, body1)
        .with_local_anchor1(local_pos0)
        .with_local_anchor2(local_pos1)
        .with_local_basis1(local_rot0)
        .with_local_basis2(local_rot1)
        .with_slider_axis(axis)
        .with_limits(lower, upper);
    if let Some(motor) = motor {
        joint.motor = motor;
    }
    attach_joint(
        commands,
        joint_entity,
        body0,
        body1,
        JointSpec::new(joint).with_seat(JointSeat::new(
            JointSeatKind::Prismatic,
            local_pos0,
            local_pos1,
            local_rot0,
            local_rot1,
            axis,
        )),
    );
}

/// Attach a USD-authored revolute joint after its body endpoints resolve.
pub fn attach_revolute_joint(
    commands: &mut Commands,
    joint_entity: Entity,
    body0: Entity,
    body1: Entity,
    local_pos0: DVec3,
    local_pos1: DVec3,
    local_rot0: DQuat,
    local_rot1: DQuat,
    axis: DVec3,
    lower: f64,
    upper: f64,
    motor: Option<AngularMotor>,
) {
    let mut joint = RevoluteJoint::new(body0, body1)
        .with_local_anchor1(local_pos0)
        .with_local_anchor2(local_pos1)
        .with_local_basis1(local_rot0)
        .with_local_basis2(local_rot1)
        .with_hinge_axis(axis)
        .with_angle_limits(lower, upper);
    if let Some(motor) = motor {
        joint.motor = motor;
    }
    attach_joint(
        commands,
        joint_entity,
        body0,
        body1,
        JointSpec::new(joint).with_seat(JointSeat::new(
            JointSeatKind::Revolute,
            local_pos0,
            local_pos1,
            local_rot0,
            local_rot1,
            axis,
        )),
    );
}

/// Attach a USD-authored spherical joint after its body endpoints resolve.
pub fn attach_spherical_joint(
    commands: &mut Commands,
    joint_entity: Entity,
    body0: Entity,
    body1: Entity,
    local_pos0: DVec3,
    local_pos1: DVec3,
    local_rot0: DQuat,
    local_rot1: DQuat,
    axis: DVec3,
    swing_limit: Option<(f64, f64)>,
    twist_lower: f64,
    twist_upper: f64,
) {
    let mut joint = SphericalJoint::new(body0, body1)
        .with_local_anchor1(local_pos0)
        .with_local_anchor2(local_pos1)
        .with_local_basis1(local_rot0)
        .with_local_basis2(local_rot1)
        .with_twist_axis(axis);
    if let Some((a0, a1)) = swing_limit {
        let s = a0.abs().max(a1.abs());
        joint = joint.with_swing_limits(-s, s);
    }
    if twist_lower.is_finite() && twist_upper.is_finite() {
        joint = joint.with_twist_limits(twist_lower, twist_upper);
    }
    attach_joint(
        commands,
        joint_entity,
        body0,
        body1,
        JointSpec::new(joint).with_seat(JointSeat::new(
            JointSeatKind::Spherical,
            local_pos0,
            local_pos1,
            local_rot0,
            local_rot1,
            axis,
        )),
    );
}

/// Attach a USD-authored distance joint after its body endpoints resolve.
pub fn attach_distance_joint(
    commands: &mut Commands,
    joint_entity: Entity,
    body0: Entity,
    body1: Entity,
    local_pos0: DVec3,
    local_pos1: DVec3,
    lower: f64,
    upper: f64,
) {
    let min = if lower.is_finite() {
        lower.max(0.0)
    } else {
        0.0
    };
    let max = if upper.is_finite() && upper >= 0.0 {
        upper.max(min)
    } else {
        f64::INFINITY
    };
    let joint = DistanceJoint::new(body0, body1)
        .with_local_anchor1(local_pos0)
        .with_local_anchor2(local_pos1)
        .with_limits(min, max);
    attach_joint(
        commands,
        joint_entity,
        body0,
        body1,
        JointSpec::new(joint).with_seat(JointSeat::new(
            JointSeatKind::Distance,
            local_pos0,
            local_pos1,
            DQuat::IDENTITY,
            DQuat::IDENTITY,
            DVec3::ZERO,
        )),
    );
}

#[derive(Clone, Copy, Debug)]
enum JointSeatKind {
    Fixed,
    Prismatic,
    Revolute,
    Spherical,
    Distance,
}

#[derive(Clone, Copy, Debug)]
struct JointSeat {
    local_pos0: DVec3,
    local_pos1: DVec3,
    local_rot0: DQuat,
    local_rot1: DQuat,
    axis: DVec3,
    kind: JointSeatKind,
}

impl JointSeat {
    fn new(
        kind: JointSeatKind,
        local_pos0: DVec3,
        local_pos1: DVec3,
        local_rot0: DQuat,
        local_rot1: DQuat,
        axis: DVec3,
    ) -> Self {
        Self {
            local_pos0,
            local_pos1,
            local_rot0,
            local_rot1,
            axis,
            kind,
        }
    }
}

fn seated_body1_velocity(
    body0_position: DVec3,
    body1_position: DVec3,
    anchor_world: DVec3,
    body0_linear: DVec3,
    body0_angular: DVec3,
    body1_linear: DVec3,
    body1_angular: DVec3,
    free_linear_axis_world: Option<DVec3>,
    free_angular_axis_world: Option<DVec3>,
    all_angular_free: bool,
    preserve_authored_free_rates: bool,
) -> (DVec3, DVec3) {
    let body0_anchor_offset = anchor_world - body0_position;
    let body1_anchor_offset = anchor_world - body1_position;
    let body0_anchor_velocity = body0_linear + body0_angular.cross(body0_anchor_offset);
    let free_linear_rate = free_linear_axis_world
        .filter(|_| preserve_authored_free_rates)
        .map(|axis| {
            let body1_anchor_velocity = body1_linear + body1_angular.cross(body1_anchor_offset);
            (body1_anchor_velocity - body0_anchor_velocity).dot(axis)
        })
        .unwrap_or(0.0);
    let target_angular = if !preserve_authored_free_rates {
        body0_angular
    } else if all_angular_free {
        body1_angular
    } else if let Some(axis) = free_angular_axis_world {
        body0_angular + axis * (body1_angular - body0_angular).dot(axis)
    } else {
        body0_angular
    };
    let target_anchor_velocity = free_linear_axis_world
        .map(|axis| body0_anchor_velocity + axis * free_linear_rate)
        .unwrap_or(body0_anchor_velocity);
    let target_linear = target_anchor_velocity - target_angular.cross(body1_anchor_offset);
    (target_linear, target_angular)
}

fn seat_joint_bodies(
    label: &str,
    body0: Entity,
    body1: Entity,
    seat: JointSeat,
    q_pose: &mut Query<(&mut Position, &mut Rotation)>,
    q_vel: &mut Query<(&mut LinearVelocity, &mut AngularVelocity)>,
    q_authored_velocity: &Query<&AuthoredInitialVelocity>,
    commands: &mut Commands,
) {
    let pose0 = q_pose
        .get(body0)
        .ok()
        .map(|(position, rotation)| (position.0, rotation.0));
    let pose1 = q_pose
        .get(body1)
        .ok()
        .map(|(position, rotation)| (position.0, rotation.0));
    let (Some((p0, r0)), Some((p1, r1))) = (pose0, pose1) else {
        return;
    };

    let locks_rotation = matches!(seat.kind, JointSeatKind::Fixed | JointSeatKind::Prismatic);
    let r1_target = r0 * seat.local_rot0 * seat.local_rot1.inverse();
    let angle = if locks_rotation {
        r1.angle_between(r1_target)
    } else {
        0.0
    };
    let r1_seated = if locks_rotation { r1_target } else { r1 };
    let anchor0_world = p0 + r0 * seat.local_pos0;
    let anchor1_world = p1 + r1_seated * seat.local_pos1;
    let delta = anchor0_world - anchor1_world;
    let p1_seated = p1 + delta;
    let seat_pos = delta.length() > JOINT_SEAT_EPS;
    let seat_rot = angle > JOINT_SEAT_ANGLE_EPS;

    if seat_pos || seat_rot {
        let worst = delta.length().max(angle);
        let detail = format!(
            "[usd-avian] joint {label} starts violated by {:.3} m / {:.3} rad — seating body1 {:?} onto the authored joint frame",
            delta.length(),
            angle,
            body1,
        );
        if worst > JOINT_SEAT_ERROR_THRESHOLD {
            error!("{detail}");
        } else {
            warn!("{detail}");
        }
        if let Ok((mut position, mut rotation)) = q_pose.get_mut(body1) {
            if seat_rot {
                rotation.0 = r1_target;
            }
            if seat_pos {
                position.0 += delta;
            }
        }
    }

    let seats_anchor_velocity = matches!(
        seat.kind,
        JointSeatKind::Fixed
            | JointSeatKind::Prismatic
            | JointSeatKind::Revolute
            | JointSeatKind::Spherical
    );
    if !seats_anchor_velocity {
        return;
    }

    let authored0 = q_authored_velocity.get(body0).ok().copied();
    let authored1 = q_authored_velocity.get(body1).ok().copied();
    let Some((lin0, ang0)) = q_vel.get(body0).ok().map(|(linear, angular)| {
        (
            authored0
                .and_then(|velocity| velocity.linear)
                .unwrap_or(linear.0),
            authored0
                .and_then(|velocity| velocity.angular)
                .unwrap_or(angular.0),
        )
    }) else {
        return;
    };
    let Some((lin1, ang1)) = q_vel.get(body1).ok().map(|(linear, angular)| {
        (
            authored1
                .and_then(|velocity| velocity.linear)
                .unwrap_or(linear.0),
            authored1
                .and_then(|velocity| velocity.angular)
                .unwrap_or(angular.0),
        )
    }) else {
        return;
    };

    let joint_axis_world = (r0 * seat.local_rot0 * seat.axis).normalize_or_zero();
    let free_linear_axis_world =
        matches!(seat.kind, JointSeatKind::Prismatic).then_some(joint_axis_world);
    let free_angular_axis_world =
        matches!(seat.kind, JointSeatKind::Revolute).then_some(joint_axis_world);
    let all_angular_free = matches!(seat.kind, JointSeatKind::Spherical);
    let (target_lin, target_ang) = seated_body1_velocity(
        p0,
        p1_seated,
        anchor0_world,
        lin0,
        ang0,
        lin1,
        ang1,
        free_linear_axis_world,
        free_angular_axis_world,
        all_angular_free,
        authored1.is_some_and(|velocity| velocity.linear.is_some() || velocity.angular.is_some()),
    );
    if (lin1 - target_lin).length() > JOINT_SEAT_EPS
        || (ang1 - target_ang).length() > JOINT_SEAT_ANGLE_EPS
    {
        if let Ok((mut linear, mut angular)) = q_vel.get_mut(body1) {
            linear.0 = target_lin;
            angular.0 = target_ang;
        }
        commands
            .entity(body1)
            .try_remove::<AuthoredInitialVelocity>();
    }
}

#[cfg(test)]
mod joint_velocity_tests {
    use super::seated_body1_velocity;
    use bevy::math::DVec3;

    #[test]
    fn prismatic_child_inherits_parent_motion_but_keeps_slider_rate_free() {
        let parent_velocity = DVec3::new(0.6, -0.25, 0.3);
        let slider_axis = DVec3::new(0.34202014, -0.93969262, 0.0);
        let (linear, angular) = seated_body1_velocity(
            DVec3::ZERO,
            DVec3::new(2.5, -4.0, 0.0),
            DVec3::new(2.5, 0.0, 0.0),
            parent_velocity,
            DVec3::ZERO,
            DVec3::ZERO,
            DVec3::ZERO,
            Some(slider_axis),
            None,
            false,
            false,
        );

        assert!((linear - parent_velocity).length() < 1.0e-6);
        assert_eq!(angular, DVec3::ZERO);
    }

    #[test]
    fn prismatic_child_preserves_only_an_authored_slider_rate() {
        let parent_velocity = DVec3::new(0.6, -0.25, 0.3);
        let slider_axis = DVec3::new(0.34202014, -0.93969262, 0.0);
        let child_velocity = parent_velocity + slider_axis * 1.75;
        let (linear, angular) = seated_body1_velocity(
            DVec3::ZERO,
            DVec3::new(2.5, -4.0, 0.0),
            DVec3::new(2.5, 0.0, 0.0),
            parent_velocity,
            DVec3::ZERO,
            child_velocity,
            DVec3::ZERO,
            Some(slider_axis),
            None,
            false,
            true,
        );

        assert!((linear - child_velocity).length() < 1.0e-6);
        assert_eq!(angular, DVec3::ZERO);
    }

    #[test]
    fn spherical_child_inherits_rigid_motion_when_rates_are_unauthored() {
        let parent_linear = DVec3::new(0.8, -2.6, 1.5);
        let parent_angular = DVec3::new(0.1, -0.2, 0.3);
        let child_position = DVec3::new(2.5, -5.0, 0.0);
        let anchor = DVec3::new(2.5, -4.9, 0.0);
        let (linear, angular) = seated_body1_velocity(
            DVec3::ZERO,
            child_position,
            anchor,
            parent_linear,
            parent_angular,
            DVec3::ZERO,
            DVec3::ZERO,
            None,
            None,
            true,
            false,
        );

        let expected_anchor_velocity = parent_linear + parent_angular.cross(anchor);
        let actual_anchor_velocity = linear + angular.cross(anchor - child_position);
        assert!((actual_anchor_velocity - expected_anchor_velocity).length() < 1.0e-6);
        assert_eq!(angular, parent_angular);
    }

    #[test]
    fn revolute_child_preserves_only_authored_hinge_rate() {
        let hinge = DVec3::Y;
        let parent_angular = DVec3::new(0.1, -0.2, 0.3);
        let child_angular = parent_angular + hinge * 1.75 + DVec3::X * 4.0;
        let (linear, angular) = seated_body1_velocity(
            DVec3::ZERO,
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(0.5, 0.0, 0.0),
            DVec3::ZERO,
            parent_angular,
            DVec3::ZERO,
            child_angular,
            None,
            Some(hinge),
            false,
            true,
        );

        assert!(((angular - parent_angular).dot(hinge) - 1.75).abs() < 1.0e-6);
        assert!((angular.x - parent_angular.x).abs() < 1.0e-6);
        let body0_anchor_velocity = parent_angular.cross(DVec3::new(0.5, 0.0, 0.0));
        let body1_anchor_velocity = linear + angular.cross(DVec3::new(-0.5, 0.0, 0.0));
        assert!((body1_anchor_velocity - body0_anchor_velocity).length() < 1.0e-6);
    }
}
