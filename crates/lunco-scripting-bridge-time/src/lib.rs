//! Simulation clock projections for scripting backends.
//!
//! This package owns the authoritative clock snapshot exposed to scripts.
//! Keeping it separate from both the neutral bridge mechanism and spatial
//! pose/navigation projections prevents unrelated hosts from acquiring the
//! physics and time-domain closure.

use bevy::prelude::*;
use lunco_physics::PhysicsTime;
use lunco_scripting_bridge_core::{ValueBuilder, with_world};
use lunco_time::{Clocks, MissionClock, ResolvedDomains, TimeTransport, WorldTime};

/// `sim_tick()` — current admitted FixedUpdate tick. A missing core tick is a
/// terminal clock-contract fault; `-1` is returned only as an explicit invalid
/// sentinel so the scripting callback cannot invent a valid tick.
pub fn sim_tick() -> i64 {
    with_world(|world| {
        world
            .get_resource::<lunco_core_runtime::SimTick>()
            .map(|tick| tick.0 as i64)
            .unwrap_or_else(|| {
                report_clock_contract_fault(world, "sim-tick-missing", "SimTick is absent");
                -1
            })
    })
    .unwrap_or_else(|| {
        error!("[scripting] deterministic clock contract violated: sim_tick() called outside a WorldScope");
        -1
    })
}

/// `dt()` — fixed-step integration delta in seconds. The production clock
/// spine is mandatory; absence is a terminal contract fault, never a default.
pub fn dt() -> f64 {
    with_world(|world| {
        let Some(time) = world.get_resource::<Time<bevy::time::Fixed>>() else {
            report_clock_contract_fault(world, "fixed-clock-missing", "Time<Fixed> is absent");
            return f64::NAN;
        };
        let delta = time.delta_secs_f64();
        if !delta.is_finite() || delta <= 0.0 {
            report_clock_contract_fault(
                world,
                "fixed-clock-invalid",
                format!("Time<Fixed>.delta must be finite and positive, got {delta:?}"),
            );
            return f64::NAN;
        }
        delta
    })
    .unwrap_or_else(|| {
        error!(
            "[scripting] deterministic clock contract violated: dt() called outside a WorldScope"
        );
        f64::NAN
    })
}

/// `elapsed_seconds()` — deterministic simulation seconds derived from the
/// integer [`lunco_core_runtime::SimTick`], not Bevy's accumulated fixed-clock
/// bookkeeping. The core tick and fixed clock are mandatory; absence is a
/// terminal contract fault.
pub fn elapsed_seconds() -> f64 {
    with_world(|world| {
        let Some(tick) = world.get_resource::<lunco_core_runtime::SimTick>().map(|tick| tick.0) else {
            report_clock_contract_fault(world, "sim-tick-missing", "SimTick is absent");
            return f64::NAN;
        };
        let Some(time) = world.get_resource::<Time<bevy::time::Fixed>>() else {
            report_clock_contract_fault(world, "fixed-clock-missing", "Time<Fixed> is absent");
            return f64::NAN;
        };
        let dt = time.timestep().as_secs_f64();
        if !dt.is_finite() || dt <= 0.0 {
            report_clock_contract_fault(
                world,
                "fixed-clock-invalid",
                format!("Time<Fixed>.timestep must be finite and positive, got {dt:?}"),
            );
            return f64::NAN;
        }
        tick as f64 * dt
    })
    .unwrap_or_else(|| {
        error!(
            "[scripting] deterministic clock contract violated: elapsed_seconds() called outside a WorldScope"
        );
        f64::NAN
    })
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
        let mission = world
            .get_resource::<MissionClock>()
            .copied()
            .unwrap_or_default();
        let transport = world
            .get_resource::<TimeTransport>()
            .copied()
            .unwrap_or_default();
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
        if let (Some(clocks), Some(resolved)) = (
            world.get_resource::<Clocks>(),
            world.get_resource::<ResolvedDomains>(),
        ) {
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
