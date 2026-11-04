//! Ordered control actions at the co-simulation input owner.

use super::*;
use lunco_command_contracts::SessionId;
use lunco_core_session::{InputFrame, PortControlAction, PortControlCommitted};

#[derive(Resource, Default)]
pub(super) struct FrameInputCache {
    entries: std::collections::HashMap<
        Entity,
        (
            u64,
            u64,
            Vec<(String, lunco_port_core::ports::ResolvedPort)>,
        ),
    >,
    identities_valid: bool,
    identities: std::collections::HashMap<GlobalEntityId, Option<Entity>>,
}
impl lunco_core::SceneResource for FrameInputCache {}

pub(super) fn invalidate_frame_identities<E: bevy::ecs::event::EventPattern>(
    _trigger: On<E>,
    mut cache: ResMut<FrameInputCache>,
) {
    cache.identities_valid = false;
}

fn stable_target_id(
    world: &mut World,
    target: Entity,
    operation: &str,
) -> Result<GlobalEntityId, String> {
    world.resource_scope(|world, mut cache: Mut<FrameInputCache>| {
        if !cache.identities_valid {
            cache.identities.clear();
            for entity in world.iter_entities() {
                if let Some(&gid) = entity.get::<GlobalEntityId>() {
                    cache
                        .identities
                        .entry(gid)
                        .and_modify(|entry| *entry = None)
                        .or_insert(Some(entity.id()));
                }
            }
            cache.identities_valid = true;
        }
        let gid = world
            .get::<GlobalEntityId>(target)
            .copied()
            .filter(|gid| gid.get() != 0)
            .ok_or_else(|| format!("{operation} requires a stable nonzero target identity"))?;
        if cache.identities.get(&gid) != Some(&Some(target)) {
            return Err(format!(
                "{operation} target identity {gid} does not resolve uniquely"
            ));
        }
        Ok(gid)
    })
}

pub(super) fn retire_frame_inputs(world: &mut bevy::ecs::world::DeferredWorld, entity: Entity) {
    if let Some(mut cache) = world.get_resource_mut::<FrameInputCache>() {
        cache.entries.remove(&entity);
    }
}

pub(super) fn retire_frame_identity(
    trigger: On<Remove<GlobalEntityId>>,
    mut cache: ResMut<FrameInputCache>,
) {
    cache.identities_valid = false;
    cache.entries.remove(&trigger.entity);
}

/// Resolve only the declared control vocabulary and explicitly controlled ports.
/// The native backend's unrelated writable state is not a replay baseline.
/// Name enumeration follows topology/hold membership or a new action endpoint;
/// continuous capture samples the cached readable control slots.
fn input_values(
    world: &mut World,
    registry: &lunco_port_core::ports::PortRegistry,
    target: Entity,
    action: &PortControlAction,
) -> Result<Vec<(String, f64)>, String> {
    let revision = world
        .resource::<lunco_port_core::ports::PortTopologyRevision>()
        .0;
    let membership = world.resource::<PortHolds>().membership_revision();
    let requested = match action {
        PortControlAction::Writes(writes) => writes.as_slice(),
        _ => &[],
    };
    world.resource_scope(|world, mut cache: Mut<FrameInputCache>| {
        let rebuild = cache
            .entries
            .get(&target)
            .is_none_or(|(observed, held, inputs)| {
                *observed != revision
                    || *held != membership
                    || requested.iter().any(|(name, _)| {
                        inputs
                            .binary_search_by(|(known, _)| known.cmp(name))
                            .is_err()
                    })
            });
        if rebuild {
            let mut names = cache
                .entries
                .get(&target)
                .into_iter()
                .flat_map(|(_, _, inputs)| inputs.iter().map(|(name, _)| name.clone()))
                .collect::<Vec<_>>();
            if let Some(inputs) = world.get::<lunco_port_core::InputPorts>(target) {
                names.extend(inputs.values.keys().map(str::to_owned));
            }
            if let Some(binding) = world.get::<lunco_control_core::ControlBinding>(target) {
                names.extend(binding.ports().map(str::to_owned));
            }
            names.extend(
                world
                    .resource::<PortHolds>()
                    .control_levels(target)
                    .into_iter()
                    .map(|(name, _)| name),
            );
            names.extend(requested.iter().map(|(name, _)| name.clone()));
            names.sort();
            names.dedup();
            let mut inputs = Vec::with_capacity(names.len());
            for name in names {
                let resolved = registry
                    .resolve_input_read(world, target, &name)
                    .ok_or_else(|| {
                        format!("prediction requires a readable control input owner for '{name}'")
                    })?;
                inputs.push((name, resolved));
            }
            cache.entries.insert(target, (revision, membership, inputs));
        }
        cache.entries[&target]
            .2
            .iter()
            .map(|(name, resolved)| {
                let value = registry
                    .read_resolved(world, target, resolved)
                    .filter(|value| value.is_finite())
                    .ok_or_else(|| format!("prediction input '{name}' has no finite sample"))?;
                Ok((name.clone(), value))
            })
            .collect()
    })
}

enum PreparedControlAction {
    Writes(Vec<lunco_port_core::ports::PreparedPortWrite>),
    ReleasePort(String),
    ReleaseControl,
}

fn prepare_action(
    world: &mut World,
    registry: &lunco_port_core::ports::PortRegistry,
    target: Entity,
    action: &PortControlAction,
) -> Result<PreparedControlAction, String> {
    match action {
        PortControlAction::Writes(writes) => {
            super::validate_port_input_target_state(world, target)
                .map_err(|(message, _)| message)?;
            super::validate_port_input_writes(world, registry, target, writes)
                .map(PreparedControlAction::Writes)
        }
        PortControlAction::ReleasePort(name) => {
            stable_target_id(world, target, "ReleasePort")?;
            if name.trim().is_empty() {
                return Err("ReleasePort input name must not be empty".into());
            }
            Ok(PreparedControlAction::ReleasePort(name.clone()))
        }
        PortControlAction::ReleaseControl => {
            stable_target_id(world, target, "ReleaseControl")?;
            Ok(PreparedControlAction::ReleaseControl)
        }
    }
}

fn commit_action(
    world: &mut World,
    registry: &lunco_port_core::ports::PortRegistry,
    target: Entity,
    action: PreparedControlAction,
    program: bool,
) -> Result<(), String> {
    match action {
        PreparedControlAction::Writes(writes) => {
            super::apply_validated_port_input_writes(world, registry, writes, program)
                .map_err(|(message, _)| message)
        }
        PreparedControlAction::ReleasePort(name) => {
            world.resource_mut::<PortHolds>().release(target, &name);
            Ok(())
        }
        PreparedControlAction::ReleaseControl => {
            world.resource_mut::<PortHolds>().clear_entity(target);
            Ok(())
        }
    }
}

/// Apply an admitted local action. Prediction and the wire observe only its
/// successfully committed receipt; requested input cannot advance their logs.
pub(super) fn commit_local_action(
    world: &mut World,
    registry: &lunco_port_core::ports::PortRegistry,
    target: Entity,
    action: PortControlAction,
    seq: u32,
    tick: u64,
    program: bool,
) -> Result<(), String> {
    let predicted = !program
        && world.get_resource::<lunco_core_session::NetworkRole>()
            == Some(&lunco_core_session::NetworkRole::Client)
        && world
            .get::<lunco_embodiment_core::roles::LocalEmbodiment>(target)
            .is_none();
    let prepared_action = prepare_action(world, registry, target, &action)?;
    let prepared_frame = if predicted {
        let gid = stable_target_id(world, target, "client control frame")?;
        let producer = world
            .get_resource::<lunco_core_session::LocalSession>()
            .ok_or("client control frame requires its session owner")?
            .0;
        if !world
            .get_resource::<lunco_core_session::SessionRegistry>()
            .is_some_and(|sessions| sessions.owns(producer, gid.get()))
        {
            return Err("client control frame requires ownership by its local session".into());
        }
        let log = world
            .get_resource::<lunco_core_session::OwnedInputLog>()
            .ok_or("client control frame requires its input log")?;
        let entry = log.0.get(&gid.get());
        let previous = entry.map_or(0, |entry| entry.next_seq);
        let seq = if seq == 0 {
            entry
                .map_or(0, |entry| entry.next_seq)
                .checked_add(1)
                .filter(|seq| *seq != u32::MAX)
                .ok_or("client control sequence exhausted")?
        } else {
            seq
        };
        if seq <= previous || seq == u32::MAX {
            return Err("client control sequence must increase".into());
        }
        let tick = if tick == 0 {
            world
                .get_resource::<lunco_core_runtime::SimTick>()
                .ok_or("client control frame requires SimTick")?
                .0
        } else {
            tick
        };
        let mut values = input_values(world, registry, target, &action)?;
        if let PortControlAction::Writes(writes) = &action {
            for (name, value) in writes {
                let index = values
                    .binary_search_by(|(known, _)| known.cmp(name))
                    .map_err(|_| {
                        format!("control input '{name}' is absent from its prepared capture")
                    })?;
                values[index].1 = *value;
            }
        }
        Some((gid.get(), producer, seq, tick, values))
    } else {
        None
    };
    commit_action(world, registry, target, prepared_action, program)?;
    if let Some((gid, producer, seq, tick, input_values)) = prepared_frame {
        let frame = InputFrame {
            seq,
            tick,
            producer,
            action,
            control_levels: world.resource::<PortHolds>().control_levels(target),
            input_values,
        };
        let mut log = world.resource_mut::<lunco_core_session::OwnedInputLog>();
        let entry = log.0.entry(gid).or_default();
        entry.next_seq = entry.next_seq.max(seq);
        if matches!(&frame.action, PortControlAction::Writes(writes) if writes.iter().any(|(_, value)| value.abs() > lunco_core_session::PREDICTION_INPUT_ACTIVITY_EPS))
        {
            entry.last_active_tick = tick;
        }
        entry.frames.push_back(frame.clone());
        while entry.frames.len() > lunco_core_session::MAX_PREDICTED_INPUT_FRAMES {
            entry.frames.pop_front();
        }
        drop(log);
        world.trigger(PortControlCommitted { target, frame });
    }
    Ok(())
}

/// Buffer an authenticated network action without changing endpoint values or
/// holds. The per-vessel consumer commits the earliest action at a fixed tick.
pub(super) fn buffer_network_action(
    world: &mut World,
    registry: &lunco_port_core::ports::PortRegistry,
    target: Entity,
    action: PortControlAction,
    seq: u32,
    tick: u64,
    producer: SessionId,
) -> Result<(), String> {
    let gid = stable_target_id(world, target, "network control frame")?;
    if !world
        .resource::<lunco_core_session::SessionRegistry>()
        .owns(producer, gid.get())
    {
        return Err("network control frame is not owned by its authenticated producer".into());
    }
    match &action {
        PortControlAction::Writes(writes) => {
            super::validate_port_input_writes(world, registry, target, writes)?;
        }
        PortControlAction::ReleasePort(name) if name.trim().is_empty() => {
            return Err("ReleasePort input name must not be empty".into());
        }
        _ => {}
    }
    world
        .resource_mut::<lunco_core_session::BufferedClientInputs>()
        .push(
            gid.get(),
            InputFrame {
                seq,
                tick,
                producer,
                action,
                control_levels: Vec::new(),
                input_values: Vec::new(),
            },
        )
}

/// Commit exactly one authenticated network action through the same atomic
/// backend/hold owner used by local control commands.
pub fn apply_network_control_frame(
    world: &mut World,
    target: Entity,
    frame: &InputFrame,
) -> Result<(), String> {
    let gid = stable_target_id(world, target, "network control frame")?;
    if !world
        .resource::<lunco_core_session::SessionRegistry>()
        .owns(frame.producer, gid.get())
    {
        return Err("network input producer no longer controls this endpoint".into());
    }
    let registry = world
        .resource::<lunco_port_core::ports::PortRegistry>()
        .clone();
    let prepared = prepare_action(world, &registry, target, &frame.action)?;
    commit_action(world, &registry, target, prepared, false)
}

/// Restore a recorded controller snapshot atomically before replaying its action.
/// Future controller levels are removed; authored-program holds are preserved
/// except where the recorded explicit release addresses them.
pub fn replay_control_frame(
    world: &mut World,
    target: Entity,
    frame: &InputFrame,
) -> Result<(), String> {
    let gid = stable_target_id(world, target, "replayed control frame")?;
    if !world
        .resource::<lunco_core_session::SessionRegistry>()
        .owns(frame.producer, gid.get())
    {
        return Err("replayed input producer no longer controls this endpoint".into());
    }
    let registry = world
        .resource::<lunco_port_core::ports::PortRegistry>()
        .clone();
    super::validate_port_input_target_state(world, target).map_err(|(message, _)| message)?;
    let current_inputs = input_values(world, &registry, target, &frame.action)?;
    if current_inputs
        .iter()
        .map(|(name, _)| name)
        .ne(frame.input_values.iter().map(|(name, _)| name))
    {
        return Err("replayed control frame belongs to a different input surface".into());
    }
    // Validate the complete snapshot and held intents before changing either.
    if !frame.control_levels.is_empty() {
        super::validate_port_input_writes(world, &registry, target, &frame.control_levels)?;
    }
    let prepared = if frame.input_values.is_empty() {
        Vec::new()
    } else {
        super::validate_port_input_writes(world, &registry, target, &frame.input_values)?
    };
    super::commit_prepared_port_input_writes(
        world,
        &registry,
        prepared,
        false,
        Some((target, &frame.control_levels)),
    )
    .map_err(|(message, _)| message)?;
    match &frame.action {
        PortControlAction::ReleasePort(name) => {
            world.resource_mut::<PortHolds>().release(target, name);
        }
        PortControlAction::ReleaseControl => world.resource_mut::<PortHolds>().clear_entity(target),
        PortControlAction::Writes(_) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acknowledged_control_frames_keep_their_sequence_fence() {
        let mut world = World::new();
        world.init_resource::<PortHolds>();
        world.init_resource::<CosimDiagnostics>();
        world.init_resource::<FrameInputCache>();
        world.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        world.init_resource::<lunco_core_session::OwnedInputLog>();
        world.init_resource::<lunco_core_session::SessionRegistry>();
        world.insert_resource(lunco_core_session::NetworkRole::Client);
        world.insert_resource(lunco_core_session::LocalSession(SessionId(7)));
        world.insert_resource(lunco_core_runtime::SimTick(10));
        let target = world
            .spawn((
                GlobalEntityId::from_raw(20),
                lunco_port_core::InputPorts::new(&["throttle"]),
            ))
            .id();
        world
            .resource_mut::<lunco_core_session::SessionRegistry>()
            .claim(SessionId(7), 20)
            .unwrap();
        let mut registry = lunco_port_core::ports::PortRegistry::default();
        crate::ports::register_builtin_port_backends(&mut registry);
        world.insert_resource(registry.clone());
        commit_local_action(
            &mut world,
            &registry,
            target,
            PortControlAction::Writes(vec![("throttle".into(), 0.25)]),
            0,
            0,
            false,
        )
        .unwrap();
        world
            .resource_mut::<lunco_core_session::OwnedInputLog>()
            .0
            .get_mut(&20)
            .unwrap()
            .frames
            .clear();
        let holds = world.resource::<PortHolds>().snapshot();
        for action in [
            PortControlAction::Writes(vec![("throttle".into(), 0.9)]),
            PortControlAction::ReleasePort("throttle".into()),
            PortControlAction::ReleaseControl,
        ] {
            let error = commit_local_action(&mut world, &registry, target, action, 1, 0, false)
                .unwrap_err();
            assert!(error.contains("sequence must increase"), "{error}");
            assert_eq!(world.resource::<PortHolds>().snapshot(), holds);
            assert_eq!(
                world
                    .get::<lunco_port_core::InputPorts>(target)
                    .unwrap()
                    .cmd("throttle"),
                0.25
            );
            let log = &world.resource::<lunco_core_session::OwnedInputLog>().0[&20];
            assert!(log.frames.is_empty());
            assert_eq!(log.next_seq, 1);
        }
        commit_local_action(
            &mut world,
            &registry,
            target,
            PortControlAction::Writes(vec![("throttle".into(), 0.5)]),
            0,
            0,
            false,
        )
        .unwrap();
        let log = &world.resource::<lunco_core_session::OwnedInputLog>().0[&20];
        assert_eq!(log.next_seq, 2);
        assert_eq!(log.frames.len(), 1);
        assert_eq!(log.frames[0].seq, 2);
        assert_eq!(
            world
                .get::<lunco_port_core::InputPorts>(target)
                .unwrap()
                .cmd("throttle"),
            0.5
        );
    }

    #[test]
    fn committed_control_frames_restore_partial_inputs_and_release_atomically() {
        let mut world = World::new();
        world.init_resource::<PortHolds>();
        world.init_resource::<CosimDiagnostics>();
        world.init_resource::<FrameInputCache>();
        world.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        world.init_resource::<lunco_core_session::OwnedInputLog>();
        world.init_resource::<lunco_core_session::SessionRegistry>();
        world.insert_resource(lunco_core_session::NetworkRole::Client);
        world.insert_resource(lunco_core_session::LocalSession(SessionId(7)));
        world.insert_resource(lunco_core_runtime::SimTick(10));
        let target = world
            .spawn((
                GlobalEntityId::from_raw(20),
                lunco_port_core::InputPorts::new(&["throttle", "steer"]),
                avian3d::prelude::RigidBody::Kinematic,
                avian3d::prelude::Position(bevy::math::DVec3::new(1.0, 2.0, 3.0)),
                avian3d::prelude::LinearVelocity(Vec3::new(4.0, 5.0, 6.0)),
            ))
            .id();
        world
            .resource_mut::<lunco_core_session::SessionRegistry>()
            .claim(SessionId(7), 20)
            .unwrap();
        let mut registry = lunco_port_core::ports::PortRegistry::default();
        crate::ports::register_builtin_port_backends(&mut registry);
        world.insert_resource(registry.clone());
        commit_local_action(
            &mut world,
            &registry,
            target,
            PortControlAction::Writes(vec![("throttle".into(), 0.25)]),
            0,
            0,
            false,
        )
        .unwrap();
        commit_local_action(
            &mut world,
            &registry,
            target,
            PortControlAction::Writes(vec![("steer".into(), 0.5)]),
            0,
            0,
            false,
        )
        .unwrap();
        assert!(
            commit_local_action(
                &mut world,
                &registry,
                target,
                PortControlAction::Writes(vec![("throttle".into(), 0.9), ("missing".into(), 1.0)]),
                0,
                0,
                false
            )
            .is_err()
        );
        assert_eq!(
            world.resource::<lunco_core_session::OwnedInputLog>().0[&20]
                .frames
                .len(),
            2
        );
        assert_eq!(
            world
                .get::<lunco_port_core::InputPorts>(target)
                .unwrap()
                .cmd("throttle"),
            0.25
        );
        commit_local_action(
            &mut world,
            &registry,
            target,
            PortControlAction::ReleasePort("throttle".into()),
            0,
            0,
            false,
        )
        .unwrap();
        let frames = world.resource::<lunco_core_session::OwnedInputLog>().0[&20]
            .frames
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            frames.iter().map(|frame| frame.seq).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        let authoritative_position = bevy::math::DVec3::new(70.0, 80.0, 90.0);
        let authoritative_velocity = Vec3::new(9.0, 8.0, 7.0);
        world
            .get_mut::<avian3d::prelude::Position>(target)
            .unwrap()
            .0 = authoritative_position;
        world
            .get_mut::<avian3d::prelude::LinearVelocity>(target)
            .unwrap()
            .0 = authoritative_velocity;
        assert_eq!(
            frames[0]
                .input_values
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["steer", "throttle"]
        );
        replay_control_frame(&mut world, target, &frames[0]).unwrap();
        assert_eq!(
            world.get::<avian3d::prelude::Position>(target).unwrap().0,
            authoritative_position,
            "control replay must not replace authoritative pose"
        );
        assert_eq!(
            world
                .get::<avian3d::prelude::LinearVelocity>(target)
                .unwrap()
                .0,
            authoritative_velocity,
            "control replay must not replace authoritative velocity"
        );
        assert_eq!(
            world
                .get::<lunco_port_core::InputPorts>(target)
                .unwrap()
                .cmd("steer"),
            0.0,
            "a future partial input cannot leak into replay"
        );
        assert_eq!(world.resource::<PortHolds>().get(target, "steer"), None);
        assert_eq!(
            world.resource::<PortHolds>().get(target, "throttle"),
            Some(0.25)
        );
        replay_control_frame(&mut world, target, &frames[1]).unwrap();
        assert_eq!(
            world
                .get::<lunco_port_core::InputPorts>(target)
                .unwrap()
                .cmd("steer"),
            0.5
        );
        replay_control_frame(&mut world, target, &frames[2]).unwrap();
        assert_eq!(world.resource::<PortHolds>().get(target, "throttle"), None);
        assert_eq!(
            world.resource::<PortHolds>().get(target, "steer"),
            Some(0.5)
        );
        assert_eq!(
            world
                .get::<lunco_port_core::InputPorts>(target)
                .unwrap()
                .cmd("throttle"),
            0.25,
            "release preserves the endpoint until propagation"
        );
        let before = world.resource::<PortHolds>().snapshot();
        let mut invalid = frames[0].clone();
        invalid.input_values.push(("missing".into(), 1.0));
        assert!(replay_control_frame(&mut world, target, &invalid).is_err());
        assert_eq!(world.resource::<PortHolds>().snapshot(), before);
        assert_eq!(
            world
                .get::<lunco_port_core::InputPorts>(target)
                .unwrap()
                .cmd("steer"),
            0.5
        );
    }
}
