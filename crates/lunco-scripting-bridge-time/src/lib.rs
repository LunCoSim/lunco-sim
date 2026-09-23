//! Simulation clock projections for scripting backends.
//!
//! This package owns the authoritative clock snapshot exposed to scripts.
//! Keeping it separate from both the neutral bridge mechanism and spatial
//! pose/navigation projections prevents unrelated hosts from acquiring the
//! physics and time-domain closure.

use bevy::prelude::*;
use lunco_physics::PhysicsTime;
use lunco_scripting_bridge_core::{ValueBuilder, execution_context, with_world};
use lunco_time::{
    CelestialTime, ClockRoot, Clocks, MissionClock, ResolvedDomains, SimulationPresentationTime,
    TimeDomain, TimeTransport, WorldTime,
};

/// `sim_tick()` — current admitted FixedUpdate tick. The caller must be inside
/// the simulation cycle; an out-of-cycle call fails only its current Rhai
/// invocation. A missing core tick remains a terminal clock-contract fault.
pub fn sim_tick() -> Result<i64, String> {
    require_simulation_context("sim_tick()")?;
    with_world(|world| {
        world
            .get_resource::<lunco_core_runtime::SimTick>()
            .map(|tick| Ok(tick.0 as i64))
            .unwrap_or_else(|| {
                report_clock_contract_fault(world, "sim-tick-missing", "SimTick is absent");
                Err("deterministic simulation clock is missing SimTick".to_owned())
            })
    })
    .ok_or_else(|| "sim_tick() requires an active script WorldScope".to_owned())?
}

/// `dt()` — fixed-step integration delta in seconds. The production clock
/// spine is mandatory; absence is a terminal contract fault, never a default.
pub fn dt() -> Result<f64, String> {
    require_simulation_context("dt()")?;
    with_world(|world| {
        let Some(time) = world.get_resource::<Time<bevy::time::Fixed>>() else {
            report_clock_contract_fault(world, "fixed-clock-missing", "Time<Fixed> is absent");
            return Err("deterministic simulation clock is missing Time<Fixed>".to_owned());
        };
        let delta = time.delta_secs_f64();
        if !delta.is_finite() || delta <= 0.0 {
            report_clock_contract_fault(
                world,
                "fixed-clock-invalid",
                format!("Time<Fixed>.delta must be finite and positive, got {delta:?}"),
            );
            return Err(format!(
                "deterministic simulation clock has invalid fixed delta {delta:?}"
            ));
        }
        Ok(delta)
    })
    .ok_or_else(|| "dt() requires an active script WorldScope".to_owned())?
}

/// `elapsed_seconds()` — deterministic simulation seconds derived from the
/// integer [`lunco_core_runtime::SimTick`], not Bevy's accumulated fixed-clock
/// bookkeeping. The core tick and fixed clock are mandatory; absence is a
/// terminal contract fault.
pub fn elapsed_seconds() -> Result<f64, String> {
    require_simulation_context("elapsed_seconds()")?;
    with_world(|world| {
        let Some(tick) = world
            .get_resource::<lunco_core_runtime::SimTick>()
            .map(|tick| tick.0)
        else {
            report_clock_contract_fault(world, "sim-tick-missing", "SimTick is absent");
            return Err("deterministic simulation clock is missing SimTick".to_owned());
        };
        let Some(time) = world.get_resource::<Time<bevy::time::Fixed>>() else {
            report_clock_contract_fault(world, "fixed-clock-missing", "Time<Fixed> is absent");
            return Err("deterministic simulation clock is missing Time<Fixed>".to_owned());
        };
        let dt = time.timestep().as_secs_f64();
        if !dt.is_finite() || dt <= 0.0 {
            report_clock_contract_fault(
                world,
                "fixed-clock-invalid",
                format!("Time<Fixed>.timestep must be finite and positive, got {dt:?}"),
            );
            return Err(format!(
                "deterministic simulation clock has invalid fixed timestep {dt:?}"
            ));
        }
        Ok(tick as f64 * dt)
    })
    .ok_or_else(|| "elapsed_seconds() requires an active script WorldScope".to_owned())?
}

fn require_simulation_context(function: &str) -> Result<(), String> {
    let context = execution_context();
    if context
        .route
        .is_some_and(|route| route.cycle == lunco_core::RuntimeCycle::Simulation)
        && context.clock == lunco_core::RuntimeClock::Simulation
    {
        return Ok(());
    }
    Err(format!(
        "{function} is available only in the simulation cycle (current route: {:?}, phase: {:?})",
        context.route, context.phase
    ))
}

/// Read the event's producer sequence when a producer stamp is attached;
/// otherwise use the current cycle's sequence. Discrete invocations have none.
pub fn logical_sequence() -> Option<u64> {
    let context = execution_context();
    context
        .producer
        .map(|producer| producer.sequence)
        .or(context.sequence)
}

/// Project the active typed execution context into a backend-native map.
pub fn execution_context_value<B: ValueBuilder>(b: &B) -> B::Value {
    let context = execution_context();
    let route = context.route;
    let entries = vec![
        (
            "scope".to_owned(),
            route.map_or_else(
                || b.unit(),
                |route| b.string(&format!("{:?}", route.scope).to_ascii_lowercase()),
            ),
        ),
        (
            "cycle".to_owned(),
            route.map_or_else(
                || b.unit(),
                |route| b.string(&format!("{:?}", route.cycle).to_ascii_lowercase()),
            ),
        ),
        (
            "phase".to_owned(),
            b.string(&format!("{:?}", context.phase).to_ascii_lowercase()),
        ),
        (
            "generation".to_owned(),
            optional_u64(b, route.map(|route| route.generation)),
        ),
        (
            "clock".to_owned(),
            b.string(&format!("{:?}", context.clock).to_ascii_lowercase()),
        ),
        ("sequence".to_owned(), optional_u64(b, context.sequence)),
        (
            "producer".to_owned(),
            context.producer.map_or_else(
                || b.unit(),
                |producer| {
                    b.map(vec![
                        (
                            "scope".to_owned(),
                            b.string(&format!("{:?}", producer.route.scope).to_ascii_lowercase()),
                        ),
                        (
                            "cycle".to_owned(),
                            b.string(&format!("{:?}", producer.route.cycle).to_ascii_lowercase()),
                        ),
                        ("generation".to_owned(), b.uint(producer.route.generation)),
                        ("sequence".to_owned(), b.uint(producer.sequence)),
                    ])
                },
            ),
        ),
        (
            "time_seconds".to_owned(),
            optional_float(b, context.time_seconds),
        ),
        (
            "delta_seconds".to_owned(),
            optional_float(b, context.delta_seconds),
        ),
    ];
    b.map(entries)
}

fn optional_u64<B: ValueBuilder>(b: &B, value: Option<u64>) -> B::Value {
    value.map(|value| b.uint(value)).unwrap_or_else(|| b.unit())
}

fn optional_float<B: ValueBuilder>(b: &B, value: Option<f64>) -> B::Value {
    value
        .map(|value| b.float(value))
        .unwrap_or_else(|| b.unit())
}

/// Surface a missing/invalid mandatory clock as a terminal simulation fault.
/// The scripting bridge never substitutes wall time or a nominal tick: a
/// production host that omitted the core time spine must stop loudly.
fn report_clock_contract_fault(world: &mut World, kind: &'static str, detail: impl Into<String>) {
    let detail = detail.into();
    if let Some(mut faults) = world.get_resource_mut::<lunco_core::RuntimeFaults>() {
        if faults.raise(kind, None, "scripting-clock", detail.clone()) {
            error!("[scripting] deterministic clock contract violated: {detail}");
        }
    } else {
        error!(
            "[scripting] deterministic clock contract violated: {detail} (RuntimeFaults missing)"
        );
    }
}

/// Read the complete simulation clock snapshot exposed to every scripting
/// backend. The integer [`lunco_core_runtime::SimTick`] is the deterministic master;
/// all other simulation-domain values are derived or projected from it.
///
/// The mandatory `SimTick` + `Time<Fixed>` + `Time<Virtual>` spine is never
/// substituted: a missing/invalid spine raises `RuntimeFaults` and sets
/// `clock_contract_ok` false. Optional physics and domain clocks remain
/// explicitly represented in the returned map.
pub fn clock_snapshot<B: ValueBuilder>(b: &B) -> B::Value {
    with_world(|world| {
        // Copy the mandatory spine before raising a fault. A fault is a
        // mutable resource write; retaining `&SimTick`/`&Time` borrows while
        // reporting it would make the contract checker itself fail to
        // compile. The snapshot below is therefore a value-level view of the
        // clocks, not a set of live ECS references.
        let tick = world
            .get_resource::<lunco_core_runtime::SimTick>()
            .map(|value| value.0);
        let fixed_snapshot = world.get_resource::<Time<bevy::time::Fixed>>().map(|time| {
            (
                time.delta_secs_f64(),
                time.elapsed_secs_f64(),
                time.timestep().as_secs_f64(),
            )
        });
        let virtual_snapshot = world.get_resource::<Time<Virtual>>().map(|time| {
            (
                time.delta_secs_f64(),
                time.elapsed_secs_f64(),
                time.relative_speed_f64(),
                time.is_paused(),
            )
        });
        let mut clock_contract_error = String::new();
        if tick.is_none() {
            clock_contract_error.push_str("SimTick is absent; ");
        }
        if fixed_snapshot.is_none() {
            clock_contract_error.push_str("Time<Fixed> is absent; ");
        }
        if virtual_snapshot.is_none() {
            clock_contract_error.push_str("Time<Virtual> is absent; ");
        }
        if !clock_contract_error.is_empty() {
            report_clock_contract_fault(
                world,
                "simulation-clock-missing",
                clock_contract_error.clone(),
            );
        }
        let tick = tick.unwrap_or(0);
        let fixed_dt = fixed_snapshot.map_or(f64::NAN, |(_, _, timestep)| timestep);
        let admitted_sim_elapsed = (tick as f64) * fixed_dt;
        let real_snapshot = world
            .get_resource::<Time<Real>>()
            .map(|time| (time.delta_secs_f64(), time.elapsed_secs_f64()));
        let physics_snapshot = world
            .get_resource::<Time<lunco_physics::Physics>>()
            .map(|time| {
                (
                    time.delta_secs_f64(),
                    time.elapsed_secs_f64(),
                    time.is_paused(),
                )
            });
        let physics_contract = world
            .get_resource::<lunco_physics::PhysicsDeterminism>()
            .copied();
        let physics_contract_error = if physics_contract.is_none() {
            let error = "PhysicsDeterminism is absent; physics admission is not enforceable";
            report_clock_contract_fault(world, "physics-determinism-missing", error.to_owned());
            error
        } else {
            ""
        };
        let world_time = world
            .get_resource::<WorldTime>()
            .copied()
            .unwrap_or_default();
        let presentation_time = world.get_resource::<SimulationPresentationTime>().copied();
        let mission = world
            .get_resource::<MissionClock>()
            .copied()
            .unwrap_or_default();
        let transport = world
            .get_resource::<TimeTransport>()
            .copied()
            .unwrap_or_default();
        let celestial_time = world.get_resource::<CelestialTime>().copied();
        let clocks = world.get_resource::<Clocks>().copied();
        let celestial_domain = clocks.and_then(|clocks| {
            world
                .get::<TimeDomain>(clocks.celestial)
                .copied()
                .map(|domain| (clocks, domain))
        });
        let celestial_parent = celestial_domain.map(|(clocks, domain)| match domain.parent {
            Some(parent) if parent == clocks.sim => "sim",
            Some(parent) if parent == clocks.real => "real",
            Some(_) => "other",
            None => match world.get::<ClockRoot>(clocks.celestial) {
                Some(ClockRoot::Epoch) => "epoch",
                Some(ClockRoot::Tick) => "sim",
                Some(ClockRoot::Wall) => "real",
                None => "unknown",
            },
        });
        let barrier = world
            .get_resource::<lunco_core_runtime::SimulationBarrier>()
            .copied()
            .unwrap_or_default();

        let running = virtual_snapshot.is_some_and(|(_, _, rate, paused)| !paused && rate > 0.0);
        let mut entries = vec![
            ("sim_tick".to_owned(), b.int(tick as i64)),
            (
                "admitted_sim_elapsed_s".to_owned(),
                b.float(admitted_sim_elapsed),
            ),
            ("fixed_dt_s".to_owned(), b.float(fixed_dt)),
            (
                "fixed_elapsed_s".to_owned(),
                b.float(fixed_snapshot.map_or(f64::NAN, |(_, elapsed, _)| elapsed)),
            ),
            (
                "virtual_dt_s".to_owned(),
                b.float(virtual_snapshot.map_or(f64::NAN, |(delta, _, _, _)| delta)),
            ),
            (
                "virtual_elapsed_s".to_owned(),
                b.float(virtual_snapshot.map_or(f64::NAN, |(_, elapsed, _, _)| elapsed)),
            ),
            (
                "virtual_rate".to_owned(),
                b.float(virtual_snapshot.map_or(f64::NAN, |(_, _, rate, _)| rate)),
            ),
            (
                "virtual_paused".to_owned(),
                b.bool(virtual_snapshot.is_some_and(|(_, _, _, paused)| paused)),
            ),
            ("simulation_is_running".to_owned(), b.bool(running)),
            (
                "wall_dt_s".to_owned(),
                b.float(real_snapshot.map_or(0.0, |(delta, _)| delta)),
            ),
            (
                "wall_elapsed_s".to_owned(),
                b.float(real_snapshot.map_or(0.0, |(_, elapsed)| elapsed)),
            ),
            (
                "physics_dt_s".to_owned(),
                b.float(physics_snapshot.map_or(0.0, |(delta, _, _)| delta)),
            ),
            (
                "physics_elapsed_s".to_owned(),
                b.float(physics_snapshot.map_or(0.0, |(_, elapsed, _)| elapsed)),
            ),
            (
                "physics_paused".to_owned(),
                b.bool(physics_snapshot.is_some_and(|(_, _, paused)| paused)),
            ),
            (
                "physics_deterministic".to_owned(),
                b.bool(physics_contract.is_some_and(|contract| contract.deterministic)),
            ),
            (
                "physics_compute_threads".to_owned(),
                b.int(
                    physics_contract
                        .and_then(|contract| contract.compute_threads)
                        .map_or(-1, |value| value as i64),
                ),
            ),
            (
                "physics_contract_ok".to_owned(),
                b.bool(physics_contract.is_some_and(|contract| contract.deterministic)),
            ),
            (
                "physics_contract_error".to_owned(),
                b.string(physics_contract_error),
            ),
            ("world_sim_s".to_owned(), b.float(world_time.sim_secs)),
            ("world_met_s".to_owned(), b.float(world_time.met_secs)),
            ("epoch_jd".to_owned(), b.float(world_time.epoch_jd)),
            (
                "celestial_time_available".to_owned(),
                b.bool(celestial_time.is_some()),
            ),
            (
                "celestial_epoch_jd".to_owned(),
                celestial_time.map_or_else(|| b.unit(), |time| b.float(time.epoch_jd)),
            ),
            (
                "celestial_delta_s".to_owned(),
                celestial_time.map_or_else(|| b.unit(), |time| b.float(time.delta_secs)),
            ),
            (
                "celestial_rate".to_owned(),
                celestial_domain.map_or_else(|| b.unit(), |(_, domain)| b.float(domain.scale)),
            ),
            (
                "celestial_parent".to_owned(),
                celestial_parent.map_or_else(|| b.unit(), |parent| b.string(parent)),
            ),
            (
                "presentation_time_available".to_owned(),
                b.bool(presentation_time.is_some()),
            ),
            (
                "presentation_sim_s".to_owned(),
                presentation_time.map_or_else(|| b.unit(), |time| b.float(time.sim_secs)),
            ),
            (
                "presentation_epoch_jd".to_owned(),
                presentation_time.map_or_else(|| b.unit(), |time| b.float(time.epoch_jd)),
            ),
            (
                "presentation_interpolation".to_owned(),
                presentation_time.map_or_else(|| b.unit(), |time| b.float(time.interpolation)),
            ),
            (
                "mission_tick0".to_owned(),
                b.int(mission.mission_tick0 as i64),
            ),
            (
                "mission_epoch0_jd".to_owned(),
                b.float(mission.mission_epoch0_jd),
            ),
            (
                "transport_playing".to_owned(),
                b.bool(transport.is_running()),
            ),
            ("transport_rate".to_owned(), b.float(transport.rate)),
            ("barrier_held".to_owned(), b.bool(barrier.held)),
            (
                "barrier_active_participants".to_owned(),
                b.int(barrier.active_participants as i64),
            ),
            (
                "barrier_shared_clock_participants".to_owned(),
                b.int(barrier.shared_clock_participants as i64),
            ),
            (
                "barrier_worst_lag_s".to_owned(),
                b.float(barrier.worst_lag_secs),
            ),
            ("wall_time_deterministic".to_owned(), b.bool(false)),
            ("deterministic_master".to_owned(), b.string("sim_tick")),
            (
                "clock_contract_ok".to_owned(),
                b.bool(clock_contract_error.is_empty()),
            ),
            (
                "clock_contract_error".to_owned(),
                b.string(&clock_contract_error),
            ),
        ];

        let mut domains = Vec::new();
        if let (Some(clocks), Some(resolved)) =
            (clocks.as_ref(), world.get_resource::<ResolvedDomains>())
        {
            for (name, entity) in [
                ("real", clocks.real),
                ("sim", clocks.sim),
                ("interaction", clocks.interaction),
                ("celestial", clocks.celestial),
            ] {
                if let Some(sample) = resolved.sample(entity) {
                    domains.push(b.map(vec![
                        ("name".to_owned(), b.string(name)),
                        ("t_s".to_owned(), b.float(sample.t)),
                        ("dt_s".to_owned(), b.float(sample.dt)),
                    ]));
                }
            }
        }
        entries.push(("domains".to_owned(), b.array(domains)));
        b.map(entries)
    })
    .unwrap_or_else(|| b.map(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_core::{
        RuntimeClock, RuntimeCycle, RuntimeExecutionContext, RuntimePhase, RuntimeProducerStamp,
        RuntimeRoute, RuntimeScope,
    };
    use lunco_scripting_bridge_core::WorldScope;

    #[test]
    fn fixed_clock_helpers_reject_application_context_without_faulting_simulation() {
        let mut world = World::new();
        world.init_resource::<lunco_core::RuntimeFaults>();
        world.insert_resource(lunco_core_runtime::SimTick(12));
        let context = RuntimeExecutionContext {
            route: Some(RuntimeRoute {
                scope: RuntimeScope::Application,
                cycle: RuntimeCycle::Repl,
                generation: 0,
            }),
            phase: RuntimePhase::Evaluation,
            clock: RuntimeClock::Application,
            time_seconds: Some(2.0),
            delta_seconds: Some(0.1),
            sequence: Some(4),
            producer: None,
        };
        let _scope = WorldScope::enter(&mut world, context);

        assert!(
            sim_tick()
                .unwrap_err()
                .contains("only in the simulation cycle")
        );
        assert!(dt().unwrap_err().contains("only in the simulation cycle"));
        assert!(
            elapsed_seconds()
                .unwrap_err()
                .contains("only in the simulation cycle")
        );
        assert!(!world.resource::<lunco_core::RuntimeFaults>().active());
    }

    #[test]
    fn fixed_clock_helpers_read_simulation_context() {
        let mut world = World::new();
        world.insert_resource(lunco_core_runtime::SimTick(12));
        let mut fixed = Time::<bevy::time::Fixed>::default();
        fixed.set_timestep_seconds(0.25);
        fixed.advance_by(std::time::Duration::from_millis(250));
        world.insert_resource(fixed);
        world.init_resource::<lunco_core::RuntimeFaults>();
        let context = RuntimeExecutionContext {
            route: Some(RuntimeRoute::twin(RuntimeCycle::Simulation, 3)),
            phase: RuntimePhase::Behavior,
            clock: RuntimeClock::Simulation,
            time_seconds: Some(3.0),
            delta_seconds: Some(0.25),
            sequence: Some(12),
            producer: None,
        };
        let _scope = WorldScope::enter(&mut world, context);

        assert_eq!(sim_tick().unwrap(), 12);
        assert!((dt().unwrap() - 0.25).abs() < f64::EPSILON);
        assert!((elapsed_seconds().unwrap() - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn event_producer_sequence_takes_precedence_over_consumer_sequence() {
        let mut world = World::new();
        let context = RuntimeExecutionContext {
            route: Some(RuntimeRoute::application(RuntimeCycle::Repl)),
            phase: RuntimePhase::Event,
            clock: RuntimeClock::Application,
            time_seconds: Some(9.0),
            delta_seconds: Some(0.1),
            sequence: Some(90),
            producer: Some(RuntimeProducerStamp::simulation(3, 89)),
        };
        let _scope = WorldScope::enter(&mut world, context);

        assert_eq!(logical_sequence(), Some(89));
    }
}
