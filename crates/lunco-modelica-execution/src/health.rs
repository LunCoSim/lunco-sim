//! Windowed service cost and throughput of admitted Modelica worker completions.

use bevy::{platform::time::Instant, prelude::*};
use lunco_core::SceneResource;
use lunco_core_runtime::{ModelicaHealthSnapshot, SimTick};
use lunco_modelica_runtime::ModelicaStepDiagnostics;
use std::{collections::HashMap, time::Duration};

#[derive(Clone, Copy, PartialEq, Eq)]
struct CompletionCounter {
    session: u64,
    steps: u64,
    worker_ns: u64,
}

#[derive(Resource, Default)]
pub(super) struct ModelicaHealthWindow {
    started_at: Option<Instant>,
    counters: HashMap<Entity, CompletionCounter>,
    physics_tick: Option<u64>,
}

impl SceneResource for ModelicaHealthWindow {}

pub(super) fn modelica_health_due(
    diagnostics: Res<ModelicaStepDiagnostics>,
    window: Res<ModelicaHealthWindow>,
) -> bool {
    diagnostics.is_changed()
        || window.is_changed()
        || window
            .started_at
            .is_some_and(|start| start.elapsed() >= Duration::from_secs(1))
}

/// Publish the slowest instance's completed steps per real second and the
/// largest per-instance mean worker cost over the same one-second window.
/// Total worker service is normalized by the authoritative physics-tick delta.
/// Cumulative counters come from the sole validated receipt consumer.
pub(super) fn publish_modelica_health(
    diagnostics: Res<ModelicaStepDiagnostics>,
    mut window: ResMut<ModelicaHealthWindow>,
    mut health: ResMut<ModelicaHealthSnapshot>,
    physics_tick: Option<Res<SimTick>>,
) {
    let now = Instant::now();
    let participants = diagnostics
        .participants()
        .filter(|(_, sample)| sample.samples > 0)
        .count();
    if participants == 0 {
        if *health != ModelicaHealthSnapshot::default() {
            *health = ModelicaHealthSnapshot::default();
        }
        window.started_at = None;
        window.counters.clear();
        window.physics_tick = None;
        return;
    }
    let current_tick = physics_tick.as_ref().map(|tick| tick.0);
    let same_sessions = participants == window.counters.len()
        && diagnostics
            .participants()
            .filter(|(_, sample)| sample.samples > 0)
            .all(|(entity, counter)| {
                window.counters.get(&entity).is_some_and(|previous| {
                    previous.session == counter.session_id
                        && previous.steps <= counter.samples
                        && previous.worker_ns <= counter.total_worker_step_duration_ns
                })
            });
    let elapsed = window.started_at.map(|start| now.duration_since(start));
    if !same_sessions || elapsed.is_none() {
        *health = ModelicaHealthSnapshot {
            participants,
            ..Default::default()
        };
        window.capture(&diagnostics, now, current_tick);
        return;
    }
    let elapsed = elapsed.expect("the initialized window has a start instant");
    if elapsed < Duration::from_secs(1) {
        return;
    }
    let seconds = elapsed.as_secs_f64();
    let mut fps: Option<f64> = None;
    let mut worker_ms: Option<f64> = None;
    let mut worker_ns: u128 = 0;
    for (entity, counter) in diagnostics
        .participants()
        .filter(|(_, sample)| sample.samples > 0)
    {
        let previous = window.counters[&entity];
        let steps = counter.samples - previous.steps;
        let rate = steps as f64 / seconds;
        fps = Some(fps.map_or(rate, |current| current.min(rate)));
        let duration_ns = counter.total_worker_step_duration_ns - previous.worker_ns;
        worker_ns += u128::from(duration_ns);
        if steps > 0 {
            let mean_ms = duration_ns as f64 / steps as f64 / 1_000_000.0;
            worker_ms = Some(worker_ms.map_or(mean_ms, |current| current.max(mean_ms)));
        }
    }
    let physics_steps = current_tick.and_then(|tick| {
        window
            .physics_tick
            .and_then(|previous| tick.checked_sub(previous))
    });
    let worker_ms_per_physics_step = physics_steps
        .filter(|steps| *steps > 0)
        .map(|steps| worker_ns as f64 / steps as f64 / 1_000_000.0);
    *health = ModelicaHealthSnapshot {
        worker_ms_per_physics_step,
        sample_physics_steps: physics_steps,
        fps,
        worker_step_ms: worker_ms,
        participants,
        sample_wall_secs: Some(seconds),
    };
    window.capture(&diagnostics, now, current_tick);
}

impl ModelicaHealthWindow {
    fn capture(
        &mut self,
        diagnostics: &ModelicaStepDiagnostics,
        now: Instant,
        physics_tick: Option<u64>,
    ) {
        self.physics_tick = physics_tick;
        self.started_at = Some(now);
        self.counters.clear();
        self.counters.extend(
            diagnostics
                .participants()
                .filter(|(_, sample)| sample.samples > 0)
                .map(|(entity, sample)| {
                    (
                        entity,
                        CompletionCounter {
                            session: sample.session_id,
                            steps: sample.samples,
                            worker_ns: sample.total_worker_step_duration_ns,
                        },
                    )
                }),
        );
    }
}
