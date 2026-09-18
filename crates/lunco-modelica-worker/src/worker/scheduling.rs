//! Native worker command scheduling.

use super::{CompileWork, cmd_entity, cmd_session, is_squashable, result_ok};
use bevy::prelude::Entity;
use crossbeam_channel::Sender;
use lunco_modelica_runtime::{ModelicaCommand, ModelicaResult};
use std::collections::{HashMap, VecDeque};

/// M8 — the worker's two-lane scheduler, on ONE thread.
///
/// The `SimulationSession`s are `!Send` and the rumoca `Session` is owned by
/// the same thread, so a genuine compile-thread/step-thread split would have
/// to move one of them across threads — not available. The alternative the
/// architecture does support is PRIORITY scheduling: commands are queued into
/// two lanes and every runnable Step is processed before the next queued
/// compile-shaped command, so a slow compile (or a 10-60 s `LoadSourceRoot`)
/// delays other live models' Steps by at most the command currently executing,
/// never by the whole queue.
///
/// Lanes:
/// * **step lane** — `Step` for an entity with no queued compile-lane command.
///   A step is also held here while that entity's asynchronous solve
///   preparation is in flight.
/// * **compile lane** — everything else (`Compile`, `UpdateParameters`,
///   `Reset`, `Despawn`, `LoadSourceRoot`), strictly FIFO, one per round.
///
/// **The per-entity ordering guarantee is preserved**: a `Step` whose entity
/// has any command pending in the compile lane is appended to the compile lane
/// instead (at its arrival position), so "Compile then Step sees the compiled
/// model" (`source_roots.rs` relies on the compile lane's FIFO for
/// source-root admission → Compile the same way) still holds command-by-command for
/// each entity. Only OTHER entities' steps jump the queue.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn enqueue_command(
    cmd: ModelicaCommand,
    compile_lane: &mut VecDeque<ModelicaCommand>,
    step_lane: &mut VecDeque<ModelicaCommand>,
    tx: &Sender<ModelicaResult>,
) {
    let is_step = matches!(cmd, ModelicaCommand::Step { .. });
    if is_step {
        let entity = cmd_entity(&cmd);
        let blocked = compile_lane.iter().any(|c| cmd_entity(c) == entity);
        if !blocked {
            step_lane.push_back(cmd);
            return;
        }
        // Fall through: this entity has pending compile-lane work, so its Step
        // takes the FIFO slot behind it. (`is_squashable` is false for Step,
        // so the squash below never collapses it.)
    }
    // The setpoint squash, unchanged in meaning: consecutive
    // Compile/UpdateParameters for the same entity+session collapse to the
    // latest, acking the dropped one (see `is_squashable`).
    if let Some(last) = compile_lane.back_mut() {
        if is_squashable(last, &cmd) && cmd_session(last) == cmd_session(&cmd) {
            let _ = tx.send(result_ok(cmd_entity(last), cmd_session(last)));
            *last = cmd;
            return;
        }
    }
    compile_lane.push_back(cmd);
}

/// After the compile lane's front command has been taken for execution, hoist
/// every deferred `Step` that is no longer behind compile-lane work for its
/// entity back into the step lane (in order), so it runs next round instead of
/// trickling out one-per-round behind unrelated compiles.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn promote_unblocked_steps(
    compile_lane: &mut VecDeque<ModelicaCommand>,
    step_lane: &mut VecDeque<ModelicaCommand>,
) {
    let mut i = 0;
    while i < compile_lane.len() {
        if matches!(compile_lane[i], ModelicaCommand::Step { .. }) {
            let entity = cmd_entity(&compile_lane[i]);
            let blocked = compile_lane.iter().take(i).any(|c| cmd_entity(c) == entity);
            if !blocked {
                let cmd = compile_lane.remove(i).expect("index checked");
                step_lane.push_back(cmd);
                continue;
            }
        }
        i += 1;
    }
}

/// Return the entities whose immutable solve preparation has been submitted
/// but whose live stepper has not yet been committed by the worker thread.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn pending_preparation_entities(
    pending_compile_works: &HashMap<u64, CompileWork>,
) -> std::collections::HashSet<Entity> {
    pending_compile_works
        .values()
        .filter(|work| !work.cancelled)
        .map(|work| work.entity)
        .collect()
}

/// Move only steps that are safe to execute into the current scheduling round.
/// Steps for entities with an in-flight preparation stay queued; executing one
/// early would violate the Compile -> Step lifecycle contract.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn take_runnable_steps(
    step_lane: &mut VecDeque<ModelicaCommand>,
    pending_entities: &std::collections::HashSet<Entity>,
) -> Vec<ModelicaCommand> {
    let mut runnable = Vec::with_capacity(step_lane.len());
    let mut blocked = VecDeque::new();
    for command in step_lane.drain(..) {
        if pending_entities.contains(&cmd_entity(&command)) {
            blocked.push_back(command);
        } else {
            runnable.push(command);
        }
    }
    *step_lane = blocked;
    runnable
}

/// Take the next compile-lane command only when its ordering prerequisites are
/// satisfied. The lane remains FIFO: a blocked front command is not bypassed
/// by a later command, preserving command order while allowing unrelated
/// Steps to continue in the step lane.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn take_runnable_compile_command(
    compile_lane: &mut VecDeque<ModelicaCommand>,
    pending_entities: &std::collections::HashSet<Entity>,
    preparation_pending: bool,
    preparation_capacity_available: bool,
) -> Option<ModelicaCommand> {
    let command = compile_lane.front()?;
    let runnable = match command {
        ModelicaCommand::Compile { entity, .. } => {
            preparation_capacity_available && !pending_entities.contains(entity)
        }
        ModelicaCommand::UpdateParameters { entity, .. }
        | ModelicaCommand::Reset { entity, .. } => !pending_entities.contains(entity),
        ModelicaCommand::Despawn { .. } => true,
        ModelicaCommand::LoadSourceRoot { .. } => !preparation_pending,
        ModelicaCommand::Step { entity, .. } => !pending_entities.contains(entity),
    };
    runnable.then(|| compile_lane.pop_front().expect("front command exists"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::Receiver;
    use lunco_modelica_runtime::LoadSourceRootPayload;

    fn ent(n: u32) -> Entity {
        Entity::from_raw_u32(n).expect("valid test entity index")
    }

    fn step(e: Entity) -> ModelicaCommand {
        ModelicaCommand::Step {
            entity: e,
            session_id: 1,
            step_id: 1,
            start_time: 0.0,
            stop_time: 0.016,
            model_name: "M".into(),
            inputs: Vec::new(),
            dt: 0.016,
        }
    }

    fn compile(e: Entity, session_id: u64) -> ModelicaCommand {
        ModelicaCommand::Compile {
            entity: e,
            session_id,
            model_name: "M".into(),
            source: String::new(),
            realtime_safe: false,
            doc_uri: "doc.mo".into(),
            extra_sources: Vec::new(),
            parameter_overrides: Vec::new(),
            stream: None,
        }
    }

    fn load_root() -> ModelicaCommand {
        ModelicaCommand::LoadSourceRoot {
            id: "Modelica".into(),
            payload: LoadSourceRootPayload::InMemory {
                label: "t".into(),
                files: Vec::new(),
            },
        }
    }

    struct Lanes {
        compile: VecDeque<ModelicaCommand>,
        step: VecDeque<ModelicaCommand>,
        tx: Sender<ModelicaResult>,
        rx: Receiver<ModelicaResult>,
    }

    impl Lanes {
        fn new() -> Self {
            let (tx, rx) = crossbeam_channel::unbounded();
            Self {
                compile: VecDeque::new(),
                step: VecDeque::new(),
                tx,
                rx,
            }
        }

        fn push(&mut self, cmd: ModelicaCommand) {
            enqueue_command(cmd, &mut self.compile, &mut self.step, &self.tx);
        }
    }

    #[test]
    fn unrelated_step_jumps_queued_compile() {
        let mut l = Lanes::new();
        l.push(load_root());
        l.push(compile(ent(1), 2));
        l.push(step(ent(2)));
        assert_eq!(l.step.len(), 1, "entity 2's Step must take the step lane");
        assert_eq!(l.compile.len(), 2);
    }

    #[test]
    fn step_behind_own_entitys_compile_does_not_jump() {
        let mut l = Lanes::new();
        l.push(compile(ent(1), 2));
        l.push(step(ent(1)));
        assert!(
            l.step.is_empty(),
            "entity 1's Step must wait for its compile"
        );
        assert_eq!(l.compile.len(), 2);
        assert!(matches!(l.compile[0], ModelicaCommand::Compile { .. }));
        assert!(matches!(l.compile[1], ModelicaCommand::Step { .. }));
    }

    #[test]
    fn load_source_root_does_not_block_live_steps() {
        let mut l = Lanes::new();
        l.push(load_root());
        l.push(step(ent(3)));
        assert_eq!(l.step.len(), 1);
    }

    #[test]
    fn deferred_step_promoted_after_its_compile_is_taken() {
        let mut l = Lanes::new();
        l.push(compile(ent(1), 2));
        l.push(compile(ent(9), 2));
        l.push(step(ent(1)));
        assert!(l.step.is_empty());

        let front = l.compile.pop_front().expect("front compile");
        assert_eq!(cmd_entity(&front), ent(1));
        promote_unblocked_steps(&mut l.compile, &mut l.step);
        assert_eq!(l.step.len(), 1);
        assert_eq!(l.compile.len(), 1);
    }

    #[test]
    fn compile_lane_still_squashes_setpoints() {
        let mut l = Lanes::new();
        l.push(compile(ent(1), 2));
        l.push(compile(ent(1), 2));
        assert_eq!(l.compile.len(), 1, "second Compile replaced the first");
        let ack = l.rx.try_recv().expect("dropped command must be acked");
        assert_eq!(ack.session_id, 2);
        l.push(step(ent(1)));
        l.push(step(ent(1)));
        assert_eq!(l.compile.len(), 3);
        assert!(l.rx.try_recv().is_err(), "no synthetic ack for a Step");
    }

    #[test]
    fn in_flight_preparation_holds_only_its_entity_steps() {
        let mut steps = VecDeque::from([step(ent(1)), step(ent(2))]);
        let pending = std::collections::HashSet::from([ent(1)]);

        let runnable = take_runnable_steps(&mut steps, &pending);

        assert_eq!(runnable.len(), 1);
        assert_eq!(cmd_entity(&runnable[0]), ent(2));
        assert_eq!(steps.len(), 1);
        assert_eq!(cmd_entity(steps.front().expect("blocked step")), ent(1));
    }

    #[test]
    fn in_flight_preparation_preserves_compile_lane_order() {
        let mut compile_lane = VecDeque::from([compile(ent(1), 2), compile(ent(2), 3)]);
        let pending = std::collections::HashSet::from([ent(1)]);

        assert!(take_runnable_compile_command(&mut compile_lane, &pending, true, true).is_none());
        assert_eq!(compile_lane.len(), 2);

        let pending = std::collections::HashSet::new();
        let command = take_runnable_compile_command(&mut compile_lane, &pending, false, true)
            .expect("front compile becomes runnable after preparation commits");
        assert_eq!(cmd_entity(&command), ent(1));
    }

    #[test]
    fn source_root_waits_for_all_in_flight_preparations() {
        let mut compile_lane = VecDeque::from([load_root()]);
        let pending = std::collections::HashSet::from([ent(1)]);

        assert!(take_runnable_compile_command(&mut compile_lane, &pending, true, true).is_none());

        let pending = std::collections::HashSet::new();
        assert!(matches!(
            take_runnable_compile_command(&mut compile_lane, &pending, false, true),
            Some(ModelicaCommand::LoadSourceRoot { .. })
        ));
    }

    #[test]
    fn despawn_can_cancel_an_in_flight_entity() {
        let entity = ent(1);
        let mut compile_lane = VecDeque::from([ModelicaCommand::Despawn { entity }]);
        let pending = std::collections::HashSet::from([entity]);

        assert!(matches!(
            take_runnable_compile_command(&mut compile_lane, &pending, true, false),
            Some(ModelicaCommand::Despawn { entity: candidate }) if candidate == entity
        ));
    }
}
