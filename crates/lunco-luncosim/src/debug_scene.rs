//! `luncosim test` — headless, physics-only, deterministic scene+scenario runner.
//!
//! ## Why this exists
//!
//! A parity scene is a TEST, but until now the only way to run one was to boot
//! the whole GUI simulator and babysit it:
//!
//! ```text
//! timeout 300 cargo run -q -p lunco-luncosim --bin luncosim -j 2 -- \
//!     --scene scenes/tests/drivetrain_parity.usda
//! ```
//!
//! That opens a window, brings up wgpu, renders every frame, runs in REALTIME
//! (~25 s of wall clock for ~25 s of sim), never self-exits, and reports its
//! verdict only as a line in the log — so the harness needs an external
//! `timeout` and a human (or a grep) to decide pass/fail. It is also the reason
//! this crate grew a drawer of one-off probes: each one hand-built a headless world
//! because there was no way to headlessly run a REAL authored scene.
//!
//! `luncosim test` composes the **same app the `--no-ui`
//! server composes** (`SandboxCorePlugin { headless: true }` +
//! `SandboxHeadlessPlugin` — literally the two plugins `run_with_mode(true)`
//! uses), steps it by hand as fast as the CPU allows, watches the scenario's
//! telemetry verdict, and exits with a status code.
//!
//! ```text
//! cargo run -q -p lunco-luncosim --bin luncosim -j 4 -- \
//!     test --scene scenes/tests/drivetrain_parity.usda
//! echo $?   # 0 = PASS, 1 = FAIL, 2 = no verdict (hang / load failure)
//! ```
//!
//! ## Why it is deterministic (and unbounded in rate)
//!
//! Two knobs, both essential:
//!
//! 1. **`TimeUpdateStrategy::ManualDuration(dt)`** — the clock no longer reads
//!    the wall. Every `app.update()` advances `Time<Virtual>` by exactly `dt`,
//!    which `Time<Fixed>` drains into exactly one `FixedUpdate` tick when
//!    `--tick-hz` matches `lunco_core::FIXED_HZ`. So the run is BOTH
//!    bit-reproducible (identical dt sequence every time, no frame-time noise
//!    leaking into the solver) and as fast as the CPU can go (no sleeping, no
//!    vsync, no realtime pacing).
//!
//!    This is *not* the same as asking the sim to warp. Realtime warp is capped
//!    at `MAX_REALTIME_RATE = 16`; above that the physics tick FREEZES rather
//!    than going faster. Manual stepping sidesteps the rate limiter entirely
//!    because there is no "realtime" to be a multiple of.
//!
//! 2. **A single-threaded compute pool.** The deterministic scene-test contract
//!    avian's parallel solver reorders island/contact work across threads, so a
//!    multi-threaded run is NOT run-to-run reproducible, while the same scene on
//!    one compute thread is bit-identical. A test runner that can't reproduce
//!    its own result is worthless, so we pin `compute` to 1 thread. (This is a
//!    deliberate divergence from how the GUI runs — see `pinned_compute_pool`
//!    and `--threads` below.)
//!
//! Manual stepping also means we do **not** call `App::run()`. The
//! `ScheduleRunnerPlugin` that `SandboxHeadlessPlugin` installs simply never
//! gets to drive anything — the loop at the bottom of `main` is the runner.
//!
//! ## How the verdict is read
//!
//! Via **telemetry**, not by scraping stdout.
//!
//! The scenario contract (`assets/scenarios/*_parity.rhai`) ends in
//! `emit("<CHANNEL>", "PASS" | "FAIL")`, and rhai's `emit` fires a real
//! `TelemetryEvent` on the shared bus (`bridge_core::emit` →
//! `world.trigger(TelemetryEvent { .. })`). An observer here catches it — a
//! typed, in-process, order-guaranteed signal. Log scraping would have meant
//! parsing the tracing output of another thread, tolerating format drift, and
//! racing the log writer; there is no reason to do that when the event is
//! already a first-class thing in the World.
//!
//! We match on the PAYLOAD (`TelemetryValue::String("PASS"/"FAIL")`), not on a
//! hardcoded channel name, because each scene names its own channel
//! (`DRIVETRAIN_PARITY`, `ACKERMANN_PARITY`, `SIX_INDEPENDENT_MOTION`,
//! `ALLOCATION_SPEC`, …). The channel name is reported in the summary so it is
//! never ambiguous WHICH check answered. `--verdict-channel <NAME>` pins it if a
//! scene ever emits two.
//!
//! First verdict wins: the parity scenarios have an early-abort path that emits
//! `FAIL` before the full comparison runs, and that is a genuine verdict.
//!
//! ## Exit codes
//!
//! | code | meaning |
//! |------|---------|
//! | 0    | a verdict arrived and it was `PASS` |
//! | 1    | a verdict arrived and it was `FAIL` |
//! | 2    | `--max-ticks` was exhausted with NO verdict, the app asked to exit before one, or the CLI was malformed |
//!
//! A hang is a FAILURE, not a pass. A scene whose scenario never reaches its
//! verdict (deadlocked wheel build, scene that never loaded, script that threw)
//! must not be able to go green by saying nothing.
//!
//! ## The 2x2 matrix: `--threads` × `--jitter`
//!
//! The two knobs above are exactly the two ways this runner differs from the
//! GUI luncosim, and a scene can pass here while failing there. That happened:
//! `scenes/tests/drivetrain_parity.usda` passes 8/8 under `luncosim test` and
//! blows up under the GUI (measured speed 4.5 → 120 → 846 m/s during the steer
//! phase, heading NaN). Two candidate causes, and "it passes headless" tells
//! you nothing about WHICH:
//!
//! ```text
//!                jitter=0 (fixed dt)      jitter>0 (variable dt)
//!   threads=1    the default gate         isolates DT SENSITIVITY
//!   threads=0/N  isolates THREAD ORDER    closest to the real GUI
//! ```
//!
//! Each axis isolates one thing, so read the matrix like this:
//!
//! * **Fails only when `--jitter > 0`** ⇒ a **dt-sensitivity bug**, not a
//!   threading bug. Something in the drivetrain integrates in a way that is not
//!   stable across a varying step — a per-frame delta divided by a stale `dt`,
//!   a spring/PD gain tuned for one step size, an explicit integration that
//!   goes unstable past a step threshold. A 4.5 → 846 m/s explosion is the
//!   signature of exactly this: a blowup, not numerical drift.
//! * **Fails only when `--threads` ≠ 1** ⇒ an **ordering/race bug**: avian's
//!   parallel solver visiting islands and contacts in a different order, or a
//!   system pair whose relative order is unconstrained.
//! * **Fails in both** ⇒ they are the same underlying fragility, surfaced two
//!   ways.
//! * **Passes in all four** ⇒ the GUI differs in a THIRD way not modelled here
//!   (rendering feedback, input, interpolation, a UI-only system) and the
//!   bisection has to continue outside this binary.
//!
//! `--jitter` is a MODEL of realtime pacing, not realtime itself: the clock is
//! still `ManualDuration`, just re-set to a different value before each update,
//! drawn from a **seeded** PRNG (`--seed`). So a jittered failure is still
//! exactly reproducible — same seed, same dt sequence, same blowup, every run.
//! A test you cannot re-run is not a test, which is why this deliberately does
//! not touch the wall clock or system randomness.
//!
//! Note that under `--jitter` one `app.update()` is no longer necessarily one
//! `FixedUpdate` tick: `Time<Fixed>` accumulates a varying delta and will drain
//! zero, one, or two ticks per update. That is the point — it is precisely the
//! catch-up behaviour a realtime frontend produces, and it is a prime suspect
//! for the blowup. The reported `ticks` counts UPDATES; `sim` is the summed
//! simulated time, which stays accurate because the jitter is symmetric.

use std::time::Duration;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use crate::SandboxHeadlessPlugin;
use lunco_core::telemetry::{TelemetryEvent, TelemetryValue};
use lunco_modelica::ModelicaModel;
use lunco_usd_sim::cosim::{PendingModelicaSource, UsdSourcedCosim};

/// Safety bound on the manual step loop. 20 000 ticks ≈ 333 s of simulated time
/// at 60 Hz — an order of magnitude more than any current parity scenario needs
/// (~25 s), so hitting it means something is genuinely stuck.
const DEFAULT_MAX_TICKS: u64 = 20_000;

/// Fixed default PRNG seed for `--jitter`. A CONSTANT, never a clock read: the
/// whole value of jitter-mode is that a failure it finds can be replayed.
const DEFAULT_SEED: u64 = 0x5EED_1EAF_C0FF_EE01;

#[derive(Clone)]
struct Cli {
    /// USD scene path. Accepts an asset-root-relative path such as
    /// `scenes/tests/drivetrain_parity.usda`, a cwd-relative path, or an
    /// absolute path into a custom Twin.
    /// Consumed by `SandboxCorePlugin`, which does its own `--scene` parse off
    /// `std::env::args()`; we parse it too only so we can REQUIRE it and print it.
    scene: String,
    max_ticks: u64,
    tick_hz: f64,
    /// Optional channel-name filter for the verdict (see module docs).
    verdict_channel: Option<String>,
    /// Compute-pool threads. `1` = the reproducible default, `0` = leave bevy's
    /// default multi-threaded pool alone (what the GUI runs), `n>1` = pin n.
    threads: usize,
    /// Fractional dt jitter in `[0, 1)`. `0.0` = the exact fixed step.
    jitter: f64,
    /// Seed for the jitter PRNG. Irrelevant when `jitter == 0.0`.
    seed: u64,
    /// Optional prim path to select and measure selection AABB bounds for.
    #[cfg(feature = "ui")]
    select_prim: Option<String>,
}

/// `xorshift64*` — a seeded, dependency-free PRNG.
///
/// Deliberately NOT `rand`: this needs three lines of arithmetic, and pulling a
/// crate in would mean the dt sequence depends on a version bump. Deliberately
/// not the system RNG or the clock either — see the module docs. The multiplier
/// and shift triple are Vigna's; the statistical quality only has to be good/// enough to look like frame-pacing noise.
struct Xorshift64Star(u64);

impl Xorshift64Star {
    fn new(seed: u64) -> Self {
        // State must never be zero, or the generator is stuck at zero forever.
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    /// Next value in `[0, 1)`, taking the high 53 bits (an f64's mantissa).
    fn next_unit(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        let v = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (v >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Next dt, uniform in `[(1-frac)*base, (1+frac)*base]`.
    fn next_dt(&mut self, base: Duration, frac: f64) -> Duration {
        if frac <= 0.0 {
            return base;
        }
        let scale = 1.0 - frac + 2.0 * frac * self.next_unit();
        Duration::from_secs_f64(base.as_secs_f64() * scale)
    }
}

/// The verdict, filled in by the telemetry observer. `None` until a scenario
/// speaks.
#[derive(Resource, Default)]
struct Verdict {
    /// `Some((channel, passed))` once the first PASS/FAIL payload lands.
    result: Option<(String, bool)>,
    /// Set from the CLI so the observer can filter by channel.
    want_channel: Option<String>,
}

fn parse_args() -> Result<Cli, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut scene: Option<String> = None;
    let mut max_ticks = DEFAULT_MAX_TICKS;
    let mut tick_hz = lunco_core::FIXED_HZ;
    let mut verdict_channel: Option<String> = None;
    let mut threads: usize = 1;
    let mut jitter = 0.0f64;
    let mut seed = DEFAULT_SEED;
    #[cfg(feature = "ui")]
    let mut select_prim: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        // Space-separated values, matching the `luncosim` binary's own `--scene`
        // convention (NOT `--key=value` like the ad-hoc probes) so a scene path
        // reads the same in both invocations.
        let need = |i: usize, flag: &str| -> Result<String, String> {
            args.get(i + 1)
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match args[i].as_str() {
            "--scene" => {
                scene = Some(need(i, "--scene")?);
                i += 2;
            }
            "--max-ticks" => {
                let v = need(i, "--max-ticks")?;
                max_ticks = v
                    .parse()
                    .map_err(|_| format!("--max-ticks expects an integer, got {v:?}"))?;
                i += 2;
            }
            "--tick-hz" => {
                let v = need(i, "--tick-hz")?;
                tick_hz = v
                    .parse()
                    .map_err(|_| format!("--tick-hz expects a number, got {v:?}"))?;
                if tick_hz <= 0.0 || !tick_hz.is_finite() {
                    return Err("--tick-hz must be a positive, finite number".to_string());
                }
                i += 2;
            }
            "--verdict-channel" => {
                verdict_channel = Some(need(i, "--verdict-channel")?);
                i += 2;
            }
            "--threads" => {
                let v = need(i, "--threads")?;
                threads = v
                    .parse()
                    .map_err(|_| format!("--threads expects a non-negative integer, got {v:?}"))?;
                i += 2;
            }
            "--jitter" => {
                let v = need(i, "--jitter")?;
                jitter = v
                    .parse()
                    .map_err(|_| format!("--jitter expects a number, got {v:?}"))?;
                // >= 1.0 would admit a zero or negative dt, which is not
                // "variable pacing" but a broken clock.
                if !(0.0..1.0).contains(&jitter) {
                    return Err("--jitter must be in [0.0, 1.0)".to_string());
                }
                i += 2;
            }
            "--seed" => {
                let v = need(i, "--seed")?;
                seed = v
                    .parse()
                    .map_err(|_| format!("--seed expects an unsigned integer, got {v:?}"))?;
                i += 2;
            }
            #[cfg(feature = "ui")]
            "--select-prim" => {
                select_prim = Some(need(i, "--select-prim")?);
                i += 2;
            }
            // Answer help and leave with a SUCCESS code — asking for usage is
            // not a test failure, and a `2` here would poison a wrapper script.
            "-h" | "--help" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            // Unknown args are IGNORED rather than rejected: `SandboxCorePlugin`
            // and the domain plugins parse their own flags off `env::args()`
            // (`--api`, `--host`, …), and this binary must not veto them.
            _ => i += 1,
        }
    }

    let scene = scene.ok_or_else(|| format!("--scene is required\n\n{}", usage()))?;
    Ok(Cli {
        scene,
        max_ticks,
        tick_hz,
        verdict_channel,
        threads,
        jitter,
        seed,
        #[cfg(feature = "ui")]
        select_prim,
    })
}

fn usage() -> String {
    format!(
        "\
    luncosim test — run one authored USD scene + its scenario headless and deterministically.

USAGE:
    luncosim test --scene <PATH> [--max-ticks N] [--tick-hz HZ] [--verdict-channel NAME]
               [--threads N] [--jitter FRAC] [--seed U64]

    --scene PATH             REQUIRED. USD scene path. It may be relative to
                             assets/, relative to the current directory, or an
                             absolute path into a custom Twin.
    --max-ticks N            Safety bound on simulated ticks (default {DEFAULT_MAX_TICKS}).
                             Exhausting it with no verdict exits 2.
    --tick-hz HZ             Manual clock step rate (default {hz}, = lunco_core::FIXED_HZ).
                             Keep it at FIXED_HZ for exactly one physics tick
                             per update.
    --verdict-channel NAME   Only accept a PASS/FAIL from this telemetry channel.
                             Default: the first PASS/FAIL payload on any channel.

DIAGNOSTIC AXES (defaults reproduce the deterministic gate exactly):
    --threads N              Compute-pool threads (default 1).
                               1  pin one thread — reproducible, the gate.
                               0  DO NOT override the pool: bevy's default
                                  multi-threaded sizing, i.e. what the GUI runs.
                               N  pin N compute threads.
                             Non-1 values are NOT run-to-run reproducible
                             (avian's parallel solver reorders island work) —
                             use them to isolate ordering bugs, not to gate.
    --jitter FRAC            Fractional dt jitter in [0.0, 1.0) (default 0.0).
                             0.0 keeps the exact fixed step. Above 0, each
                             update advances by a seeded pseudo-random dt in
                             [(1-FRAC)*base, (1+FRAC)*base], MIMICKING the
                             variable frame pacing of the realtime GUI so that
                             GUI-only failures can be reproduced headlessly.
                             A scene that PASSES at jitter=0 and FAILS at
                             jitter>0 has a DT-SENSITIVITY bug, not a threading
                             bug. Still fully reproducible for a given --seed.
    --seed U64               Seed for the jitter PRNG (default {seed}).
                             Same seed => same dt sequence => same outcome.

EXIT CODES:
    0  scenario emitted PASS
    1  scenario emitted FAIL
    2  no verdict (max ticks exhausted, early app exit, or bad arguments)",
        hz = lunco_core::FIXED_HZ,
        seed = DEFAULT_SEED,
    )
}

/// Pin the compute task pool to exactly `threads` threads.
///
/// The deterministic headless test runner uses one compute worker: a regression
/// gate must reproduce its own result before it can meaningfully diagnose a
/// scenario failure. The scenes under test are single-rover, so the throughput
/// trade-off is small.
///
/// `threads == 0` is handled by the CALLER, which skips this override entirely
/// rather than asking for a zero-sized pool.
/// Catch the scenario's verdict off the shared telemetry bus.
///
/// `emit(name, "PASS"|"FAIL")` in rhai lands here as a triggered
/// `TelemetryEvent` with a `TelemetryValue::String` payload. Anything else on
/// the bus (zone enters, `lander_touchdown`, sampled parameters) is ignored —
/// only a literal `PASS`/`FAIL` string is a verdict.
/// Ports the scenario declared it EXPECTS to dangle, via `expect_fault(port)`.
///
/// A fixture scene can be deliberately malformed — `lint_selftest` authors wires that
/// target ports no program declares, because that malformation is the thing the lint
/// policy is being tested against. Without a way to say so, the never-landed gate reads a
/// fixture exactly like a broken rover.
///
/// This is a two-way assertion, not a mute: an expected fault that never occurs fails the
/// scene too. A fixture that quietly stops being malformed is a broken fixture, and that
/// is precisely the failure a mute would hide.
#[derive(Resource, Default)]
struct ExpectedFaults(std::collections::BTreeSet<String>);

/// Runtime-fault kinds a negative scene explicitly asserts.
#[derive(Resource, Default)]
struct ExpectedRuntimeFaults(std::collections::BTreeSet<String>);

/// `expect_fault(port)` in rhai lands here.
fn catch_expected_fault(trigger: On<TelemetryEvent>, mut expected: ResMut<ExpectedFaults>) {
    let evt = trigger.event();
    if evt.name != "EXPECT_FAULT" {
        return;
    }
    let TelemetryValue::String(port) = &evt.data else {
        return;
    };
    expected.0.insert(port.clone());
}

/// `expect_runtime_fault(kind)` in rhai lands here.
fn catch_expected_runtime_fault(
    trigger: On<TelemetryEvent>,
    mut expected: ResMut<ExpectedRuntimeFaults>,
) {
    let evt = trigger.event();
    if evt.name != "EXPECT_RUNTIME_FAULT" {
        return;
    }
    let TelemetryValue::String(kind) = &evt.data else {
        return;
    };
    expected.0.insert(kind.clone());
}

fn catch_verdict(trigger: On<TelemetryEvent>, mut verdict: ResMut<Verdict>) {
    if verdict.result.is_some() {
        return; // First verdict wins — the early-abort FAIL is a real verdict.
    }
    let evt = trigger.event();
    if let Some(want) = &verdict.want_channel {
        if &evt.name != want {
            return;
        }
    }
    let TelemetryValue::String(payload) = &evt.data else {
        return;
    };
    let passed = match payload.as_str() {
        "PASS" => true,
        "FAIL" => false,
        _ => return,
    };
    let name = evt.name.clone();
    info!("[luncosim test] verdict received on channel {name}: {payload}");
    verdict.result = Some((name, passed));
}

/// Whether every Modelica source in the composed scene has reached a terminal
/// compile state. The source marker is included because the `ModelicaModel`
/// component is created only after the async asset has loaded.
fn modelica_sources_terminal(world: &mut World) -> bool {
    let pending = world
        .query_filtered::<(), (With<UsdSourcedCosim>, With<PendingModelicaSource>)>()
        .iter(world)
        .next()
        .is_none();
    if !pending {
        return false;
    }

    let mut q = world.query_filtered::<&ModelicaModel, With<UsdSourcedCosim>>();
    q.iter(world)
        .all(|model| !model.is_compiling && (model.is_compiled || model.last_error.is_some()))
}

/// True once every Modelica participant has compiled (or reached a terminal
/// error), and all authored USD joints have crossed
/// both deferred admission stages.
///
/// A terminal Modelica error is not an initialized solver and must keep physics
/// fail-closed in the normal application. The scene-test runner still advances
/// a fixture containing deliberate invalid programs so its scenario can issue
/// the real lint/failure verdict; any non-terminal compile or admission wait
/// continues to block the runner.
fn participants_ready(world: &mut World) -> bool {
    let models_ready = {
        let mut q = world.query_filtered::<&ModelicaModel, With<UsdSourcedCosim>>();
        q.iter(world).all(|model| {
            model.last_error.is_some()
                || (model.last_error.is_none()
                    && model.is_compiled
                    && !model.paused
                    && !model.variables.is_empty())
        })
    };
    if !models_ready {
        return false;
    }

    let has_terminal_model_error = {
        let mut q = world.query_filtered::<&ModelicaModel, With<UsdSourcedCosim>>();
        q.iter(world).any(|model| model.last_error.is_some())
    };
    let only_terminal_model_failures = has_terminal_model_error
        && world
            .get_resource::<lunco_readiness::ReadinessRegistry>()
            .is_some_and(|registry| {
                registry
                    .pending()
                    .next()
                    .is_some_and(|first| first.kind == lunco_readiness::kinds::PROGRAM_FAILED)
                    && registry
                        .pending()
                        .all(|item| item.kind == lunco_readiness::kinds::PROGRAM_FAILED)
            });
    if world
        .get_resource::<lunco_readiness::ReadinessState>()
        .is_some_and(|state| {
            (state.world_hold && !only_terminal_model_failures) || !state.held_entities.is_empty()
        })
    {
        return false;
    }

    let mut q_pending = world.query_filtered::<(), Or<(
        With<lunco_usd_avian::PendingUsdJoint>,
        With<lunco_usd_avian::PendingJoint<avian3d::prelude::RevoluteJoint>>,
        With<lunco_usd_avian::PendingJoint<avian3d::prelude::PrismaticJoint>>,
        With<lunco_usd_avian::PendingJoint<avian3d::prelude::FixedJoint>>,
        With<lunco_usd_avian::PendingJoint<avian3d::prelude::SphericalJoint>>,
        With<lunco_usd_avian::PendingJoint<avian3d::prelude::DistanceJoint>>,
    )>>();
    q_pending.iter(world).next().is_none()
}

/// Explain a bounded readiness failure with the live state that kept the gate
/// closed. A silent no-verdict is not actionable: the scene runner must name
/// the model, terminal error, pause state, and readiness hold that blocked the
/// scenario so the owning subsystem can be fixed.
fn log_participant_readiness_blockers(world: &mut World) {
    let mut models = world.query_filtered::<
        (
            Entity,
            &ModelicaModel,
            Option<&lunco_cosim::SimComponent>,
        ),
        With<UsdSourcedCosim>,
    >();
    for (entity, model, component) in models.iter(world) {
        let status = component
            .map(|component| format!("{:?}", component.status))
            .unwrap_or_else(|| "no SimComponent".into());
        warn!(
            "[test] participant blocker entity={entity:?} model={} compiled={} compiling={} paused={} current_time={:.6} variables={} status={} error={}",
            model.model_name,
            model.is_compiled,
            model.is_compiling,
            model.paused,
            model.current_time,
            model.variables.len(),
            status,
            model.last_error.as_deref().unwrap_or("none"),
        );
    }
    if let Some(state) = world.get_resource::<lunco_readiness::ReadinessState>() {
        warn!(
            "[test] readiness blocker world_hold={} held_entities={:?}",
            state.world_hold, state.held_entities
        );
    }
    if let Some(registry) = world.get_resource::<lunco_readiness::ReadinessRegistry>() {
        for item in registry.pending() {
            warn!(
                "[test] readiness pending kind={} subject={:?} label={} elapsed_ticks={} action={:?}",
                item.kind, item.subject, item.label, item.elapsed_ticks, item.action
            );
        }
    }
    let mut pending_joints = world
        .query_filtered::<(Entity, Option<&lunco_usd_bevy::UsdPrimPath>), Or<(
            With<lunco_usd_avian::PendingUsdJoint>,
            With<lunco_usd_avian::PendingJoint<avian3d::prelude::RevoluteJoint>>,
            With<lunco_usd_avian::PendingJoint<avian3d::prelude::PrismaticJoint>>,
            With<lunco_usd_avian::PendingJoint<avian3d::prelude::FixedJoint>>,
            With<lunco_usd_avian::PendingJoint<avian3d::prelude::SphericalJoint>>,
            With<lunco_usd_avian::PendingJoint<avian3d::prelude::DistanceJoint>>,
        )>>();
    for (entity, path) in pending_joints.iter(world) {
        warn!(
            "[test] pending joint entity={entity:?} path={}",
            path.map(|path| path.path.as_str())
                .unwrap_or("<no USD path>"),
        );
    }
    let mut pending_usd = world.query_filtered::<
        (Entity, &lunco_usd_avian::PendingUsdJoint),
        With<lunco_usd_avian::PendingUsdJoint>,
    >();
    let describe_target = |path: &str| {
        let found = world.iter_entities().find(|entity| {
            entity
                .get::<lunco_usd_bevy::UsdPrimPath>()
                .is_some_and(|value| value.path == path)
        });
        let Some(entity) = found else {
            return format!("{path}:<missing>");
        };
        format!(
            "{path}:{:?}(position={},disabled={},shadow_seeded={})",
            entity.id(),
            entity.get::<avian3d::prelude::Position>().is_some(),
            entity
                .get::<avian3d::prelude::RigidBodyDisabled>()
                .is_some(),
            entity
                .get::<lunco_usd_avian::big_space_bridge::BridgeShadow>()
                .is_some_and(|shadow| shadow.is_seeded()),
        )
    };
    for (entity, pending) in pending_usd.iter(world) {
        warn!(
            "[test] unresolved USD joint entity={entity:?} type={} body0={} body1={}",
            pending.joint_type,
            describe_target(&pending.body0_path),
            describe_target(&pending.body1_path),
        );
    }
    let mut pending_admissions = world.query_filtered::<
        (Entity, &lunco_usd_avian::PendingJointAdmission),
        With<lunco_usd_avian::PendingJointAdmission>,
    >();
    for (entity, pending) in pending_admissions.iter(world) {
        let describe_body = |body: Entity| {
            format!(
                "{body:?}(rb={:?},island={},disabled={})",
                world.get::<avian3d::prelude::RigidBody>(body),
                world
                    .get::<avian3d::dynamics::solver::islands::BodyIslandNode>(body)
                    .is_some(),
                world
                    .get::<avian3d::prelude::RigidBodyDisabled>(body)
                    .is_some(),
            )
        };
        warn!(
            "[test] joint admission entity={entity:?} body0={} body1={}",
            describe_body(pending.body0),
            describe_body(pending.body1),
        );
    }
}

pub fn run() -> u8 {
    // BEFORE the `App` exists, because building it registers settings sections and
    // that is what loads (and installs the flush for) `settings.json`.
    //
    // A harness must not write the developer's real settings. `is_test_binary()`
    // cannot see this one — a `[[bin]]` lands next to the real app, not in
    // `deps/` — so it says so out loud. This is not defensive tidiness: the
    // `CelestialCadenceSettings::EXACT` inserted below used to persist, and every
    // later run of the *luncosim* then loaded tolerance 0° and solved the whole
    // celestial tree every frame.
    #[cfg(feature = "ui")]
    lunco_settings::use_ephemeral_settings();

    let cli = match parse_args() {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            return 2;
        }
    };

    let dt = Duration::from_secs_f64(1.0 / cli.tick_hz);

    // THE app the GUI and the headless server run, assembled exactly as
    // `lunco_luncosim::build_sim_app(true, false)` does it — asset sources first
    // (they MUST precede `AssetPlugin`, which snapshots the source registry), then
    // the engine plugin group, then `SandboxCorePlugin`.
    //
    // Re-assembling that prelude here is exactly the mistake this runner made on its
    // first draft: it hand-mirrored the plugin list, omitted the asset-source
    // registration, and aborted with `Res<TwinRoots> failed validation: Resource does
    // not exist` — a resource that ships WITH the `twin://` scheme it belongs to. A
    // test runner that assembles its own lookalike app stops testing the real one.
    //
    // We inline the three lines of `build_sim_app` instead of calling it for ONE
    // reason: `TaskPoolPlugin` is configured at PluginGroup-build time and cannot be
    // reconfigured by a plugin added afterwards, so `--threads` has to reach into the
    // group. Every part still comes from the shipped public helpers
    // (`register_lunco_asset_sources`, `default_plugins`), so nothing is guessed —
    // the ONLY divergence from `build_sim_app` is the compute-pool override below.
    let mut app =
        crate::build_sim_app_with_threads(true, false, (cli.threads > 0).then_some(cli.threads));
    app.add_plugins(SandboxHeadlessPlugin::default());

    // ── Determinism, installed AFTER the core plugin so it wins ──────────────
    //
    // `SandboxCorePlugin` inserts a `Time<Virtual>` with `max_delta = 33 ms` —
    // a JITTER cap for a realtime GUI (it stops one slow frame breeding catch-up
    // ticks). Under manual stepping there is no jitter to cap, and the cap would
    // silently swallow steps for any `--tick-hz` below ~30. Re-insert with a cap
    // just above our own step so it can never clamp us.
    // The cap must clear the LARGEST step we will ever ask for, which under
    // `--jitter` is `(1 + jitter) * dt` — a cap below that would clamp exactly the
    // long frames we are trying to reproduce and quietly defang the experiment.
    let max_dt = Duration::from_secs_f64(dt.as_secs_f64() * (1.0 + cli.jitter));
    let mut virtual_time = Time::<Virtual>::default();
    virtual_time.set_max_delta(max_dt * 2);
    app.insert_resource(virtual_time);
    // The fixed clock must match the manual step, or one `app.update()` is not
    // one physics tick and the "ticks" in `--max-ticks` stop meaning anything.
    // (Under `--jitter` that one-to-one relation is INTENTIONALLY broken — the
    // fixed accumulator drains 0, 1 or 2 ticks per update, as it does in the GUI.)
    app.insert_resource(Time::<Fixed>::from_hz(cli.tick_hz));
    // THE determinism knob: the clock stops reading the wall (see module docs).
    // With `--jitter` this resource is re-set before each update; it is still
    // `ManualDuration`, so the wall clock never enters the run either way.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(dt));
    // The other determinism knob: solve the celestial tree EVERY tick, so a
    // developer's persisted cadence tolerance can never change a verdict. The
    // production default trades ≤0.01° of celestial angle for ~10 ms/frame;
    // a test that asserts a sun angle or a shadow must not inherit that trade.
    app.insert_resource(lunco_celestial::cadence::CelestialCadenceSettings::EXACT);

    app.insert_resource(Verdict {
        result: None,
        want_channel: cli.verdict_channel.clone(),
    });
    app.add_observer(catch_verdict);
    app.init_resource::<ExpectedFaults>();
    app.add_observer(catch_expected_fault);
    app.init_resource::<ExpectedRuntimeFaults>();
    app.add_observer(catch_expected_runtime_fault);

    app.finish();
    app.cleanup();

    // A scene test starts its scenario only after the scene's asynchronous
    // participants are live. The scripting plugin owns this lifecycle gate;
    // the runner only sets its explicit test-time policy.
    if let Some(mut gate) = app
        .world_mut()
        .get_resource_mut::<lunco_scripting::scenario::ScenarioExecutionGate>()
    {
        gate.enabled = false;
    }
    // A scene test is allowed to contain deliberate negative Modelica fixtures
    // (the linter self-test does). Use the readiness system's explicit policy
    // input for that test contract; otherwise a terminal compile error correctly
    // holds the production world and also prevents the fixture's Rhai verdict
    // from ever running. This changes no production policy and does not bypass
    // active compilation or unresolved physics admission.
    if let Some(mut settings) = app
        .world_mut()
        .get_resource_mut::<lunco_readiness::ReadinessSettings>()
    {
        settings.ignore_failed_models = true;
    }

    // ── Scene-readiness freeze ────────────────────────────────────────────────
    //
    // The kinematic→dynamic activation gate (`GroundColliderPending`) clears
    // when every USD prim has been examined by the DEM bridge, which in turn
    // waits for each prim's stage asset to land in the asset store. The LAST
    // asset landing is an IO-pool event on the WALL clock: it completes after
    // a different UPDATE count on every run, so rovers begin falling at
    // "tick 29±1" and the whole run diverges from there — the settle is a
    // spring-drop whose landing phase is chaotically sensitive to the release
    // tick, and a wheelie/obstacle collision later amplifies it into different
    // verdicts (measured: same `--seed`, same binary, same 6006 ticks, Easy
    // tier flips at a different station every run).
    //
    // Freeze the sim clock while the scene finishes loading: run `Update`
    // (which pumps the asset server and the bridge) with a ZERO step so the
    // fixed clock never advances, and start the real clock only once the whole
    // WALL-CLOCK-CONTINGENT half of the spawn pipeline is done — the load
    // materialized every prim (`SceneLoadInFlight` gone again) and every prim
    // processed into sim components (`UsdSimProcessed`), and the ground gate
    // cleared (a DEM terrain has at least requested its collider). Everything
    // that remains — Avian body admission, joint/differential resolution, the
    // dynamic activation and its two-update hold — is a deterministic tick
    // count once the world is fully materialized, because none of it reads the
    // wall clock anymore. "Sim tick 1" then always means "the spawn pipeline
    // is done", and the release tick depends only on scene content, never on
    // load scheduling.
    //
    // Bounded: a scene whose gate never clears is reported as a readiness
    // failure below. Starting scenario time in that state would make a test
    // verdict depend on wall-clock asset or compiler scheduling.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
    let load_waits = {
        const MAX_LOAD_WAIT_UPDATES: u32 = 60 * 60;
        let mut waits = 0u32;
        while waits < MAX_LOAD_WAIT_UPDATES {
            app.update();
            waits += 1;
            std::thread::yield_now();
            // The startup `LoadScene` runs its first step on the very first
            // update; the stage asset and every referenced vessel stage land on
            // the wall clock after that. Until prims exist every readiness query
            // below is VACUOUSLY true, so the wait must first see the load
            // actually materialize — `SceneLoadInFlight` gone again AND at least
            // one `UsdPrimPath` in the world.
            let load_done = app
                .world()
                .get_resource::<lunco_usd_sim::cosim::SceneLoadInFlight>()
                .is_none();
            let load_finished = load_done
                && app
                    .world_mut()
                    .query_filtered::<(), With<lunco_usd::UsdPrimPath>>()
                    .iter(app.world())
                    .next()
                    .is_some();
            // Every USD sim prim processed into its sim components.
            let all_processed = app
                .world_mut()
                .query_filtered::<(), (
                    With<lunco_usd::UsdPrimPath>,
                    Without<lunco_usd::UsdSimProcessed>,
                )>()
                .iter(app.world())
                .next()
                .is_none();
            let gate_clear = !app.world().resource::<lunco_usd::GroundColliderPending>().0;
            if load_finished
                && all_processed
                && gate_clear
                && modelica_sources_terminal(app.world_mut())
            {
                break;
            }
            if app.should_exit().is_some() {
                break;
            }
        }
        waits
    };
    println!("[test] scene-readiness freeze held {load_waits} updates");
    app.insert_resource(TimeUpdateStrategy::ManualDuration(dt));

    // Modelica compilation is asynchronous, and a successful compile still
    // needs one live fixed-step exchange before readiness can release the
    // body's physics hold. Run that exchange with scenario execution disabled;
    // otherwise `on_start` would consume its authored settling interval while
    // its joints are still parked and its force ports do not yet exist.
    const MAX_PARTICIPANT_WAIT_UPDATES: u32 = 60 * 60;
    let participant_waits = {
        let mut waits = 0u32;
        while waits < MAX_PARTICIPANT_WAIT_UPDATES {
            if participants_ready(app.world_mut()) {
                break;
            }
            app.update();
            waits += 1;
            std::thread::yield_now();
            if app.should_exit().is_some() {
                break;
            }
        }
        waits
    };
    let participants_are_ready = participants_ready(app.world_mut());
    println!("[test] participant-readiness warmup held {participant_waits} updates");
    if !participants_are_ready {
        log_participant_readiness_blockers(app.world_mut());
        println!(
            "luncosim test NO-VERDICT  scene={}  — participant readiness did not complete \
             before the bounded warmup",
            cli.scene
        );
        return 2;
    }
    if let Some(mut gate) = app
        .world_mut()
        .get_resource_mut::<lunco_scripting::scenario::ScenarioExecutionGate>()
    {
        gate.enabled = true;
    }

    let mut ticks = 0u64;
    let mut early_exit = false;
    let mut sim_seconds = 0.0f64;
    let mut rng = Xorshift64Star::new(cli.seed);
    while ticks < cli.max_ticks {
        // jitter == 0 short-circuits to `dt` bit-for-bit and never advances the
        // PRNG, so the default path is byte-identical to the pre-jitter runner.
        let step = rng.next_dt(dt, cli.jitter);
        if cli.jitter > 0.0 {
            app.insert_resource(TimeUpdateStrategy::ManualDuration(step));
        }
        app.update();
        ticks += 1;
        sim_seconds += step.as_secs_f64();

        // A declared negative fixture reaches its terminal boundary instead of
        // emitting an ordinary scenario verdict. Stop at that boundary so the
        // test proves the fault is raised and the process remains rebootable;
        // it does not burn the remaining max-ticks against a held simulation.
        if app
            .world()
            .get_resource::<lunco_core::RuntimeFaults>()
            .is_some_and(|faults| faults.active())
            && !app.world().resource::<ExpectedRuntimeFaults>().0.is_empty()
        {
            break;
        }

        #[cfg(feature = "ui")]
        if ticks == 10 {
            if let Some(ref target_prim) = cli.select_prim {
                use lunco_luncosim_edit::selection::{compute_selection_aabb, Selected};
                use lunco_usd_bevy::UsdPrimPath;

                let target_ent = {
                    let mut q = app.world_mut().query::<(Entity, &UsdPrimPath)>();
                    q.iter(app.world()).find_map(|(e, p)| {
                        if p.path == *target_prim {
                            Some(e)
                        } else {
                            None
                        }
                    })
                };

                if let Some(e) = target_ent {
                    app.world_mut().entity_mut(e).insert(Selected);
                    let Some(body_transform) = app.world().get::<GlobalTransform>(e).copied()
                    else {
                        warn!(
                            "[selection-aabb] Prim '{target_prim}' ({e:?}) has no body transform"
                        );
                        continue;
                    };
                    let mut state_aabb =
                        app.world_mut()
                            .query_filtered::<(&GlobalTransform, &bevy::camera::primitives::Aabb), (
                                With<Mesh3d>,
                                Without<lunco_celestial::TrajectoryMeshMarker>,
                                Without<lunco_core::programs::ProgramDriverId>,
                                Without<lunco_core::NoSelectionBounds>,
                            )>();
                    let mut state_children = app.world_mut().query::<&Children>();
                    let mut state_skip = app.world_mut().query_filtered::<(), Or<(
                        With<big_space::prelude::Grid>,
                        With<big_space::prelude::CellCoord>,
                        With<lunco_celestial::TrajectoryMeshMarker>,
                        With<lunco_core::programs::ProgramDriverId>,
                        With<lunco_core::NoSelectionBounds>,
                    )>>();
                    let mut queue = Vec::new();
                    if let Some((min, max)) = compute_selection_aabb(
                        e,
                        &body_transform,
                        &state_aabb.query(app.world()),
                        &state_children.query(app.world()),
                        &state_skip.query(app.world()),
                        &mut queue,
                    ) {
                        let size = max - min;
                        let center = (min + max) * 0.5;
                        info!(
                            "[selection-aabb] Prim '{target_prim}' ({e:?}) bounds: min={min} max={max} center={center} size={size} max_extent={:.3}m",
                            size.max_element()
                        );
                    } else {
                        warn!("[selection-aabb] Prim '{target_prim}' ({e:?}) has no mesh AABB");
                    }
                } else {
                    warn!("[selection-aabb] Target prim '{target_prim}' not found in stage");
                }
            }
        }
        if app.world().resource::<Verdict>().result.is_some() {
            break;
        }
        // Something asked the app to quit before a verdict — e.g.
        // `startup_scene_failguard` firing because `--scene` never loaded. That
        // is a failure to produce a verdict, not a pass.
        if app.should_exit().is_some() {
            early_exit = true;
            break;
        }
    }

    // The run CONFIGURATION, on the same line as the result. A green line that
    // does not say which cell of the threads×jitter matrix produced it cannot be
    // attributed, and an unattributable result is not evidence of anything.
    let threads_desc = if cli.threads == 0 {
        "default(multi)".to_string()
    } else {
        cli.threads.to_string()
    };
    let cfg = format!(
        "threads={threads_desc}  jitter={:.3}  seed={}  tick_hz={:.4}",
        cli.jitter, cli.seed, cli.tick_hz
    );

    // A PASS cannot stand while the engine is still reporting a terminal
    // runtime fault. This is a separate causal boundary from wiring: a
    // rejected client-prediction loop, non-finite force, or escaped body must
    // be reported as that fault, not disguised as a dangling connection.
    let expected_runtime = app.world().resource::<ExpectedRuntimeFaults>().0.clone();
    if let Some(fault) = app
        .world()
        .get_resource::<lunco_core::RuntimeFaults>()
        .and_then(|faults| faults.first.as_ref())
    {
        if expected_runtime.contains(fault.kind) {
            println!(
                "luncosim test PASS  scene={}  expected terminal runtime fault kind={} subject={}  ticks={ticks}  sim={sim_seconds:.2}s  {cfg}",
                cli.scene, fault.kind, fault.subject
            );
            return 0;
        }
        println!(
            "luncosim test FAIL  scene={}  ticks={ticks}  sim={sim_seconds:.2}s  {cfg}",
            cli.scene
        );
        println!(
            "  terminal runtime fault kind={} subject={} detail={}",
            fault.kind, fault.subject, fault.detail
        );
        return 1;
    }

    if !expected_runtime.is_empty() {
        println!(
            "luncosim test FAIL  scene={}  ticks={ticks}  sim={sim_seconds:.2}s  {cfg}",
            cli.scene
        );
        println!(
            "  the scenario declared expect_runtime_fault({}) but no terminal runtime fault was raised",
            expected_runtime.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        return 1;
    }

    // A PASS cannot stand while the engine is still reporting broken wires.
    //
    // `CosimDiagnostics.faults` is the substrate's OWN account of which
    // connections failed to write, recorded when it happened rather than
    // sampled now. `broken` is the wrong field for a gate: propagation is
    // CHANGE-DRIVEN, so a wire that dropped its value at load is not retried
    // on a quiet tick and the live set reads empty long before the verdict.
    // Only genuine missing-port faults are recorded (`has_port_surface`), so a
    // structural endpoint or an algebraic-loop topology diagnostic never
    // reaches here.
    // Nothing is inferred and no log is scraped; the harness reads the
    // diagnostic the propagation master already publishes.
    //
    // This exists because a dropped wire is invisible in every way that
    // matters. `rocker_bogie` passes 9/9 while its antenna yaw joint never
    // attaches, so `inputs:angle` lands nowhere and the dish never tracks
    // Earth. The Modelica drive law drops `drive_left`/`drive_right` and the
    // vessel is simply never actuated. Both emit one `warn!` into a log no
    // test reads. A wire that does not land is an authoring error, and an
    // authoring error must fail the gate that is supposed to be watching.
    let expected = app.world().resource::<ExpectedFaults>().0.clone();
    let faults: Vec<(String, String)> = app
        .world()
        .get_resource::<lunco_cosim::diagnostics::CosimDiagnostics>()
        .map(|d| {
            let mut v: Vec<(String, String)> = d
                .faults
                .values()
                .map(|b| (b.port.clone(), format!("`{}` on {:?}", b.port, b.entity)))
                .collect();
            // A HashMap has no order and the gate's output is compared between runs.
            v.sort();
            v
        })
        .unwrap_or_default();

    // Undeclared faults fail the scene. Declared ones are the fixture working.
    let broken: Vec<String> = faults
        .iter()
        // A self-coupled plant (Avian state → Modelica → Avian force) is a
        // valid explicit causal co-simulation topology, not a missing-port
        // fault. Causal exchange is handled by the fixed-step master; acausal
        // equations belong to a backend island.
        .filter(|(port, _)| !expected.contains(port))
        .map(|(_, label)| label.clone())
        .collect();
    // …and a declared fault that never happened fails it too: the fixture stopped
    // reproducing the defect it exists to reproduce.
    let missing: Vec<String> = expected
        .iter()
        .filter(|port| !faults.iter().any(|(p, _)| &p == port))
        .cloned()
        .collect();
    if !missing.is_empty() {
        println!(
            "luncosim test FAIL  scene={}  ticks={ticks}  sim={sim_seconds:.2}s  {cfg}",
            cli.scene
        );
        println!(
            "  the scenario declared expect_fault({}) but no such connection ever \
             dangled — the fixture is no longer reproducing what it asserts",
            missing.join(", ")
        );
        return 1;
    }

    match app.world().resource::<Verdict>().result.clone() {
        Some((channel, true)) if !broken.is_empty() => {
            println!(
                "luncosim test FAIL  scene={}  channel={channel}  ticks={ticks}  sim={sim_seconds:.2}s  {cfg}",
                cli.scene
            );
            println!(
                "  the scenario reported PASS, but {} connection(s) never landed: {}",
                broken.len(),
                broken.join(", ")
            );
            println!(
                "  a wire that targets a port the endpoint does not expose is an authoring \
                 error — the subsystem it feeds is dead, whatever the scenario measured"
            );
            1
        }
        Some((channel, true)) => {
            println!(
                "luncosim test PASS  scene={}  channel={channel}  ticks={ticks}  sim={sim_seconds:.2}s  {cfg}",
                cli.scene
            );
            0
        }
        Some((channel, false)) => {
            println!(
                "luncosim test FAIL  scene={}  channel={channel}  ticks={ticks}  sim={sim_seconds:.2}s  {cfg}",
                cli.scene
            );
            1
        }
        None => {
            let why = if early_exit {
                "app exited before the scenario reported (scene load failure?)"
            } else {
                "max-ticks exhausted with no verdict (scenario never finished — treated as a failure)"
            };
            println!(
                "luncosim test NO-VERDICT  scene={}  ticks={ticks}  sim={sim_seconds:.2}s  {cfg}  — {why}",
                cli.scene
            );
            2
        }
    }
}
