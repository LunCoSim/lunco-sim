//! `scene_test` — headless, physics-only, DETERMINISTIC scene+scenario runner.
//!
//! ## Why this exists
//!
//! A parity scene is a TEST, but until now the only way to run one was to boot
//! the whole GUI simulator and babysit it:
//!
//! ```text
//! timeout 300 cargo run -q -p lunco-sandbox --bin sandbox -j 2 -- \
//!     --scene scenes/sandbox/drivetrain_parity.usda
//! ```
//!
//! That opens a window, brings up wgpu, renders every frame, runs in REALTIME
//! (~25 s of wall clock for ~25 s of sim), never self-exits, and reports its
//! verdict only as a line in the log — so the harness needs an external
//! `timeout` and a human (or a grep) to decide pass/fail. It is also the reason
//! this crate grew a drawer of one-off probe binaries (`rover_turn`,
//! `rover_jitter`, `determinism_probe`): each one hand-built a headless world
//! because there was no way to headlessly run a REAL authored scene.
//!
//! `scene_test` is that missing thing: it composes the **same app the `--no-ui`
//! server composes** (`SandboxCorePlugin { headless: true }` +
//! `SandboxHeadlessPlugin` — literally the two plugins `run_with_mode(true)`
//! uses), steps it by hand as fast as the CPU allows, watches the scenario's
//! telemetry verdict, and exits with a status code.
//!
//! ```text
//! cargo run -q -p lunco-sandbox --bin scene_test -j 2 -- \
//!     --scene scenes/sandbox/drivetrain_parity.usda
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
//!    at `MAX_REALTIME_RATE = 8`; above that the physics tick FREEZES rather
//!    than going faster. Manual stepping sidesteps the rate limiter entirely
//!    because there is no "realtime" to be a multiple of.
//!
//! 2. **A single-threaded compute pool.** `determinism_probe` measured it:
//!    avian's parallel solver reorders island/contact work across threads, so a
//!    multi-threaded run is NOT run-to-run reproducible, while the same scene on
//!    one compute thread is bit-identical. A test runner that can't reproduce
//!    its own result is worthless, so we pin `compute` to 1 thread. (This is a
//!    deliberate divergence from how the GUI runs — see the note on
//!    `single_thread_compute_pool` below.)
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

use std::time::Duration;

use bevy::app::{PluginGroupBuilder, TaskPoolOptions, TaskPoolPlugin, TaskPoolThreadAssignmentPolicy};
use bevy::asset::{AssetMetaCheck, AssetPlugin};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use lunco_core::telemetry::{TelemetryEvent, TelemetryValue};
use lunco_sandbox::{SandboxCorePlugin, SandboxHeadlessPlugin};

/// Safety bound on the manual step loop. 20 000 ticks ≈ 333 s of simulated time
/// at 60 Hz — an order of magnitude more than any current parity scenario needs
/// (~25 s), so hitting it means something is genuinely stuck.
const DEFAULT_MAX_TICKS: u64 = 20_000;

#[derive(Clone)]
struct Cli {
    /// Asset-root-relative USD scene path, e.g. `scenes/sandbox/drivetrain_parity.usda`.
    /// Consumed by `SandboxCorePlugin`, which does its own `--scene` parse off
    /// `std::env::args()`; we parse it too only so we can REQUIRE it and print it.
    scene: String,
    max_ticks: u64,
    tick_hz: f64,
    /// Optional channel-name filter for the verdict (see module docs).
    verdict_channel: Option<String>,
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

    let mut i = 0;
    while i < args.len() {
        // Space-separated values, matching the `sandbox` binary's own `--scene`
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
    Ok(Cli { scene, max_ticks, tick_hz, verdict_channel })
}

fn usage() -> String {
    format!(
        "\
scene_test — run one authored USD scene + its scenario headless and deterministically.

USAGE:
    scene_test --scene <PATH> [--max-ticks N] [--tick-hz HZ] [--verdict-channel NAME]

    --scene PATH             REQUIRED. USD scene path relative to assets/, e.g.
                             scenes/sandbox/drivetrain_parity.usda
    --max-ticks N            Safety bound on simulated ticks (default {DEFAULT_MAX_TICKS}).
                             Exhausting it with no verdict exits 2.
    --tick-hz HZ             Manual clock step rate (default {hz}, = lunco_core::FIXED_HZ).
                             Keep it at FIXED_HZ for exactly one physics tick
                             per update.
    --verdict-channel NAME   Only accept a PASS/FAIL from this telemetry channel.
                             Default: the first PASS/FAIL payload on any channel.

EXIT CODES:
    0  scenario emitted PASS
    1  scenario emitted FAIL
    2  no verdict (max ticks exhausted, early app exit, or bad arguments)",
        hz = lunco_core::FIXED_HZ,
    )
}

/// Pin the compute task pool to ONE thread.
///
/// `determinism_probe` is the receipt: avian's parallel solver produces
/// run-to-run position drift on a multi-threaded pool and is bit-identical on
/// one. A regression test that cannot reproduce itself cannot bisect a
/// regression, so the runner trades throughput for reproducibility. The scenes
/// under test are single-rover, so the loss is small.
fn single_thread_compute_pool() -> TaskPoolPlugin {
    TaskPoolPlugin {
        task_pool_options: TaskPoolOptions {
            compute: TaskPoolThreadAssignmentPolicy {
                min_threads: 1,
                max_threads: 1,
                percent: 1.0,
                on_thread_spawn: None,
                on_thread_destroy: None,
            },
            ..default()
        },
    }
}

/// The base plugin group — a mirror of `lunco_sandbox`'s private
/// `default_plugins(headless: true, offscreen: false)`, which is not public.
///
/// Kept deliberately identical to the headless branch there, including the two
/// non-obvious parts:
///
/// * `.disable::<TransformPlugin>()` — `BigSpaceDefaultPlugins` (added by
///   `SandboxCorePlugin`) owns transform propagation in the f64 cell chain.
///   Leaving bevy's own propagation in place gives you a second whole-tree
///   writer fighting it.
/// * the `ui`-gated `RenderPlugin { backends: None }` — when this binary is
///   built with the crate's DEFAULT features, `bevy_render` IS linked (it comes
///   with the `ui` feature and a bin cannot unlink its own crate's deps), so
///   `DefaultPlugins` carries a `RenderPlugin` that must be told not to bring up
///   a GPU. No adapter is requested, no surface is created, no wgpu device
///   exists — the render sub-app is never built. Built with
///   `--no-default-features --features server` the whole block compiles away and
///   bevy_render is genuinely absent.
///
/// NOTE ON `MinimalPlugins`: the sibling probes (`rover_turn`,
/// `determinism_probe`) build their worlds in CODE, so `MinimalPlugins` + a
/// couple of `init_asset` calls suffices for them. This runner loads a REAL
/// authored scene, which needs the asset server rooted at `assets/`, the USD /
/// glTF / image / shader asset types, `bevy_input` (leafwing rides it),
/// `bevy_scene`, `bevy_state` and the window TYPES that the domain crates
/// reference. Reconstructing that set by hand is exactly the guesswork that
/// produces a binary which is "headless" and also subtly not the app. Starting
/// from the shipped headless configuration and subtracting is the safe
/// direction.
fn headless_plugins() -> PluginGroupBuilder {
    #[cfg(feature = "ui")]
    use bevy::render::settings::WgpuSettings;

    let group = DefaultPlugins
        // One compute thread — see `single_thread_compute_pool`. THE one place
        // this runner deliberately differs from the shipped headless server.
        .set(single_thread_compute_pool())
        .set(AssetPlugin {
            file_path: lunco_assets::assets_dir_abs().to_string_lossy().to_string(),
            // We ship no `.meta` sidecars; probing for them is a failed load per asset.
            meta_check: AssetMetaCheck::Never,
            ..default()
        })
        .set(bevy::log::LogPlugin {
            // Same filter as the app, so a failing run's log reads the same as a
            // GUI run's. The scenario's own `print` lines come through `info`.
            filter: "wgpu=error,naga=warn,cranelift=warn,cranelift_jit=warn,cranelift_codegen=warn,diffsol=warn,info".into(),
            ..default()
        });

    // No GPU. `backends: None` means bevy_render builds no render world at all.
    #[cfg(feature = "ui")]
    let group = group.set(bevy::render::RenderPlugin {
        render_creation: WgpuSettings { backends: None, ..default() }.into(),
        ..default()
    });

    // No window, and no winit event loop to want one.
    let group = group.set(WindowPlugin {
        primary_window: None,
        exit_condition: bevy::window::ExitCondition::DontExit,
        close_when_requested: false,
        ..default()
    });
    #[cfg(feature = "ui")]
    let group = group.disable::<bevy::winit::WinitPlugin>();

    group.build().disable::<TransformPlugin>()
}

/// Catch the scenario's verdict off the shared telemetry bus.
///
/// `emit(name, "PASS"|"FAIL")` in rhai lands here as a triggered
/// `TelemetryEvent` with a `TelemetryValue::String` payload. Anything else on
/// the bus (zone enters, `lander_touchdown`, sampled parameters) is ignored —
/// only a literal `PASS`/`FAIL` string is a verdict.
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
    info!("[scene_test] verdict received on channel {name}: {payload}");
    verdict.result = Some((name, passed));
}

fn main() -> std::process::ExitCode {
    let cli = match parse_args() {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            return std::process::ExitCode::from(2);
        }
    };

    let dt = Duration::from_secs_f64(1.0 / cli.tick_hz);

    // THE app, built by the same function the GUI and the headless server use
    // (`lunco_sandbox::build_sim_app`) — asset sources first (they must precede
    // `AssetPlugin`, which snapshots the source registry), then the engine plugins,
    // then `SandboxCorePlugin`.
    //
    // Re-assembling that prelude here is exactly the mistake this runner made on its
    // first draft: it hand-mirrored the plugin list, omitted the asset-source
    // registration, and aborted with `Res<TwinRoots> failed validation: Resource does
    // not exist` — a resource that ships WITH the `twin://` scheme it belongs to. A
    // test runner that assembles its own lookalike app stops testing the real one.
    let mut app = lunco_sandbox::build_sim_app(true, false);
    app.add_plugins(SandboxHeadlessPlugin);

    // ── Determinism, installed AFTER the core plugin so it wins ──────────────
    //
    // `SandboxCorePlugin` inserts a `Time<Virtual>` with `max_delta = 33 ms` —
    // a JITTER cap for a realtime GUI (it stops one slow frame breeding catch-up
    // ticks). Under manual stepping there is no jitter to cap, and the cap would
    // silently swallow steps for any `--tick-hz` below ~30. Re-insert with a cap
    // just above our own step so it can never clamp us.
    let mut virtual_time = Time::<Virtual>::default();
    virtual_time.set_max_delta(dt * 2);
    app.insert_resource(virtual_time);
    // The fixed clock must match the manual step, or one `app.update()` is not
    // one physics tick and the "ticks" in `--max-ticks` stop meaning anything.
    app.insert_resource(Time::<Fixed>::from_hz(cli.tick_hz));
    // THE determinism knob: the clock stops reading the wall (see module docs).
    app.insert_resource(TimeUpdateStrategy::ManualDuration(dt));

    app.insert_resource(Verdict {
        result: None,
        want_channel: cli.verdict_channel.clone(),
    });
    app.add_observer(catch_verdict);

    app.finish();
    app.cleanup();

    let mut ticks = 0u64;
    let mut early_exit = false;
    while ticks < cli.max_ticks {
        app.update();
        ticks += 1;
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

    let sim_seconds = ticks as f64 / cli.tick_hz;
    match app.world().resource::<Verdict>().result.clone() {
        Some((channel, true)) => {
            println!(
                "scene_test PASS  scene={}  channel={channel}  ticks={ticks}  sim={sim_seconds:.2}s",
                cli.scene
            );
            std::process::ExitCode::SUCCESS
        }
        Some((channel, false)) => {
            println!(
                "scene_test FAIL  scene={}  channel={channel}  ticks={ticks}  sim={sim_seconds:.2}s",
                cli.scene
            );
            std::process::ExitCode::from(1)
        }
        None => {
            let why = if early_exit {
                "app exited before the scenario reported (scene load failure?)"
            } else {
                "max-ticks exhausted with no verdict (scenario never finished — treated as a failure)"
            };
            println!(
                "scene_test NO-VERDICT  scene={}  ticks={ticks}  sim={sim_seconds:.2}s  — {why}",
                cli.scene
            );
            std::process::ExitCode::from(2)
        }
    }
}
