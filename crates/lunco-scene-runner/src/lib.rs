//! `luncosim test` — headless, manually stepped scene+scenario runner.
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
//! `luncosim test` composes the **same app the server composes**
//! (`lunco-luncosim-runtime::build_headless_app_with_threads` plus
//! `LunCoSimHeadlessPlugin`), steps it by hand as fast as the CPU allows,
//! watches the scenario's telemetry verdict, and exits with a status code.
//!
//! ```text
//! cargo run -q -p lunco-luncosim --bin luncosim -j 4 -- \
//!     test --scene scenes/tests/drivetrain_parity.usda
//! echo $?   # 0 = PASS, 1 = FAIL, 2 = no verdict (hang / load failure)
//! ```
//!
//! ## Manual stepping without wall-clock pacing
//!
//! Two knobs, both essential:
//!
//! 1. **`TimeUpdateStrategy::ManualDuration(dt)`** — the clock no longer reads
//!    the wall. Every `app.update()` advances `Time<Virtual>` by exactly `dt`,
//!    which `Time<Fixed>` drains into one `FixedUpdate` tick when `--tick-hz`
//!    matches `lunco_core_runtime::FIXED_HZ`. This fixes the clock input and
//!    avoids realtime pacing; it does not by itself guarantee identical
//!    authoritative state across runs. The runner advances as fast as the CPU
//!    allows, without sleeping or vsync.
//!
//!    This is a manual stepping mode, independent of the live transport rate.
//!    It avoids realtime pacing and frame-time noise in the step sequence.
//!
//! 2. **A pinned Compute pool.** The default scene-test gate pins the Bevy
//!    `ComputeTaskPool` to one thread to control parallel physics scheduling.
//!    `--threads 0` uses Bevy's default task-pool allocation, matching the pool
//!    policy used by GUI `DefaultPlugins`. Neither setting establishes
//!    whole-simulation repeatability: IO and AsyncCompute remain separate pools,
//!    and GUI schedules, rendering, and input are not part of this headless
//!    runner.
//!
//! Manual stepping also means we do **not** call `App::run()`. The
//! `ScheduleRunnerPlugin` that `LunCoSimHeadlessPlugin` installs simply never
//! gets to drive anything — the loop at the bottom of `main` is the runner.
//!
//! ## How the verdict is read
//!
//! Via **telemetry**, not by scraping stdout.
//!
//! The scenario contract (`assets/scenarios/tests/*.rhai`) ends in
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
//! (`DRIVETRAIN_PARITY`, `ACKERMANN_CONTROLLER`, `SIX_INDEPENDENT_MOTION`,
//! …). The channel name is reported in the summary so it is
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
//! | 2    | readiness timed out, `--max-ticks` was exhausted with NO verdict, the app asked to exit before one, or the CLI was malformed |
//!
//! A hang is a FAILURE, not a pass. A scene whose scenario never reaches its
//! verdict (deadlocked wheel build, scene that never loaded, script that threw)
//! must not be able to go green by saying nothing.
//!
//! ## Rust/Rhai boundary
//!
//! This module is intentionally a small Rust harness, not a vehicle-specific
//! test implementation. Rust owns process lifecycle, app construction, asynchronous
//! asset/Modelica readiness, manual clock injection, barrier pumping,
//! telemetry capture, wall-time budgets, and exit codes. Those operations must
//! happen outside the Rhai VM: Rhai is scheduled only after a Bevy world exists,
//! and a script cannot safely construct or replace that world without creating a
//! second runtime owner. Twin-authored Rhai files own the assertions, component
//! decomposition, observations, and verdict payloads. A scene test therefore has
//! one generic Rust runner and many replaceable Rhai contracts; moving this file
//! into Rhai would weaken the fail-closed execution guarantees rather
//! than move runtime policy into the core.
//!
//! ## The 2x2 matrix: `--threads` × `--jitter`
//!
//! The runner exposes two diagnostic knobs: Compute pool width and the fixed
//! versus jittered dt sequence. A four-profile matrix can help isolate their
//! effects when each profile is run separately; the optional `--stress` pass
//! varies both at once and cannot identify which caused a failure. Matching
//! the GUI task-pool policy does not reproduce GUI schedules, rendering, or
//! input behavior.
//!
//! ```text
//!                jitter=0 (fixed dt)      jitter>0 (seeded variable dt)
//!   threads=1    default gate             vary dt only
//!   threads=0    vary pool only           combined stress profile
//! ```
//!
//! A seeded jitter profile repeats its dt sequence for a fixed seed, but does
//! not guarantee identical simulation results. Failures in one controlled
//! profile show sensitivity to that profile; they do not by themselves prove a
//! race or identify a specific subsystem.
//!
//! `--jitter` is a MODEL of realtime pacing, not realtime itself: the clock is
//! still `ManualDuration`, just re-set to a different value before each update,
//! drawn from a **seeded** PRNG (`--seed`). A fixed seed therefore repeats the
//! dt sequence. Other runtime inputs and asynchronous completion order are
//! outside that guarantee; jitter generation does not use wall time or system
//! randomness.
//!
//! Note that under `--jitter` one `app.update()` is no longer necessarily one
//! `FixedUpdate` tick: `Time<Fixed>` accumulates a varying delta and will drain
//! zero, one, or two ticks per update. That is the point — it is precisely the
//! catch-up behaviour a realtime frontend produces, and it is a prime suspect
//! for the blowup. The reported `ticks` counts completed `FixedUpdate` steps;
//! `updates` counts update iterations separately. This distinction is
//! load-bearing when an asynchronous Modelica participant holds the shared
//! barrier: update iterations may continue while simulation time is frozen.

use std::path::Path;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use lunco_cosim_core::UsdSourcedCosim;
use lunco_luncosim_simulation::LunCoSimHeadlessPlugin;
use lunco_modelica_runtime::ModelicaModel;
use lunco_telemetry_core::{TelemetryEvent, TelemetryValue};
use lunco_usd_document::document::UsdDocument;
use lunco_usd_sim_cosim::PendingModelicaSource;

/// Safety bound on the manual step loop. 20 000 ticks ≈ 333 s of simulated time
/// at 60 Hz — an order of magnitude more than any current parity scenario needs
/// (~25 s), so hitting it means something is genuinely stuck.
const DEFAULT_MAX_TICKS: u64 = 20_000;

/// Wall-clock budget for scene materialization and asynchronous participant
/// readiness. Modelica source compilation and solver preparation are owned by
/// an async worker, so an update count is not a meaningful startup bound: the
/// same source can complete after different numbers of `app.update()` calls on
/// different machines. This is a liveness budget, not a performance target.
const DEFAULT_READINESS_TIMEOUT_SECS: u64 = 420;

/// Fixed default PRNG seed for `--jitter`. A CONSTANT, never a clock read: the
/// whole value of jitter-mode is that a failure it finds can be replayed.
const DEFAULT_SEED: u64 = 0x5EED_1EAF_C0FF_EE01;

#[derive(Clone)]
struct Cli {
    /// USD scene path. Accepts an asset-root-relative path such as
    /// `scenes/tests/drivetrain_parity.usda`, a cwd-relative path, or an
    /// absolute path into a custom Twin.
    /// Consumed by the application services, which resolve `--scene` through
    /// the same Twin-opening path as interactive loads. The runner parses it
    /// too so it can require the argument and print it.
    scene: String,
    max_ticks: u64,
    tick_hz: f64,
    /// Optional channel-name filter for the verdict (see module docs).
    verdict_channel: Option<String>,
    /// Optional Twin-owned SysML verification key. When set, the selected
    /// scene must be the registry mapping for this qualified name and the
    /// registry's verdict channel is used unless explicitly overridden.
    verification: Option<String>,
    /// Compute-pool threads. `1` pins one thread, `0` uses Bevy's default
    /// task-pool allocation (the same policy as GUI DefaultPlugins), `n>1` pins n.
    threads: usize,
    /// Fractional dt jitter in `[0, 1)`. `0.0` = the exact fixed step.
    jitter: f64,
    /// Seed for the jitter PRNG. Irrelevant when `jitter == 0.0`.
    seed: u64,
    /// Wall-clock budget for scene and participant readiness.
    readiness_timeout: Duration,
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

/// Fixed steps accumulated by this test process, independent of scene-local
/// clock resets during authored scene transitions.
#[derive(Resource, Default)]
struct SceneTestTickCount(u64);

fn count_scene_test_tick(
    mut count: ResMut<SceneTestTickCount>,
    virtual_time: Option<Res<Time<Virtual>>>,
) {
    let running = virtual_time
        .as_deref()
        .is_some_and(|time| !time.is_paused() && time.relative_speed_f64() > 0.0);
    if running {
        count.0 = count.0.saturating_add(1);
    }
}

fn parse_args() -> Result<Cli, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut scene: Option<String> = None;
    let component_mode = args.iter().any(|argument| argument == "test-component");
    let mut twin: Option<String> = None;
    let mut component: Option<String> = None;
    let mut max_ticks = DEFAULT_MAX_TICKS;
    let mut tick_hz = lunco_core_runtime::FIXED_HZ;
    let mut verdict_channel: Option<String> = None;
    let mut verification: Option<String> = None;
    let mut threads: usize = 1;
    let mut jitter = 0.0f64;
    let mut seed = DEFAULT_SEED;
    let mut readiness_timeout = Duration::from_secs(DEFAULT_READINESS_TIMEOUT_SECS);
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
            "--twin" => {
                twin = Some(need(i, "--twin")?);
                i += 2;
            }
            "--component" => {
                component = Some(need(i, "--component")?);
                i += 2;
            }
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
            "--verification" => {
                verification = Some(need(i, "--verification")?);
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
            "--readiness-timeout" => {
                let v = need(i, "--readiness-timeout")?;
                let seconds: u64 = v.parse().map_err(|_| {
                    format!(
                        "--readiness-timeout expects a positive integer seconds value, got {v:?}"
                    )
                })?;
                if seconds == 0 {
                    return Err("--readiness-timeout must be greater than zero".to_string());
                }
                readiness_timeout = Duration::from_secs(seconds);
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
            // Unknown args are ignored here because domain plugins parse their
            // own flags from `env::args()` (`--api`, `--host`, …).
            _ => i += 1,
        }
    }

    if component_mode {
        if scene.is_some() {
            return Err(
                "test-component selects the scene from the Twin manifest; omit --scene".to_owned(),
            );
        }
        let twin =
            twin.ok_or_else(|| format!("test-component requires --twin <PATH>\n\n{}", usage()))?;
        let component = component
            .ok_or_else(|| format!("test-component requires --component <NAME>\n\n{}", usage()))?;
        let selected = resolve_component_test(Path::new(&twin), &component)?;
        scene = Some(selected.scene);
        verification = Some(selected.verification);
        if verdict_channel.is_none() {
            verdict_channel = selected.verdict_channel;
        }
    }
    let scene = scene.ok_or_else(|| format!("--scene is required\n\n{}", usage()))?;
    Ok(Cli {
        scene,
        max_ticks,
        tick_hz,
        verdict_channel,
        verification,
        threads,
        jitter,
        seed,
        readiness_timeout,
        #[cfg(feature = "ui")]
        select_prim,
    })
}

/// Resolve one Twin component to its declared verification harness.
///
/// The manifest is the component index: it names the owned requirement source
/// and qualified verification, while that verification case names the Rhai
/// observer and USD fixture. The runner then executes the same deterministic
/// path as `test --scene`; no component-specific loading or assertion logic is
/// embedded in this CLI resolver.
struct ComponentTestSelection {
    scene: String,
    verification: String,
    verdict_channel: Option<String>,
}

fn resolve_component_test(
    twin_path: &Path,
    component_name: &str,
) -> Result<ComponentTestSelection, String> {
    let mode = lunco_twin::TwinMode::open(twin_path).map_err(|error| {
        format!(
            "cannot open Twin for component `{component_name}` at `{}`: {error}",
            twin_path.display()
        )
    })?;
    let twin = match mode {
        lunco_twin::TwinMode::Twin(twin) | lunco_twin::TwinMode::Folder(twin) => twin,
        lunco_twin::TwinMode::Orphan(path) => {
            return Err(format!(
                "component tests require a Twin folder; `{}` is a standalone file",
                path.display()
            ));
        }
    };
    let selected = twin
        .component_verification(component_name)
        .map_err(|errors| format!("Twin component registry is invalid: {}", errors.join("; ")))?;
    let scene = twin.root.join(&selected.verification.scene);
    if !scene.is_file() {
        return Err(format!(
            "component `{component_name}` verification scene is missing: `{}`",
            scene.display()
        ));
    }
    Ok(ComponentTestSelection {
        scene: scene.to_string_lossy().into_owned(),
        verification: selected.component.verification,
        verdict_channel: selected.verification.verdict_channel,
    })
}

fn usage() -> String {
    format!(
        "\
    luncosim test — run one authored USD scene + its scenario headless with manual time stepping.

USAGE:
    luncosim test --scene <PATH> [--verification QUALIFIED_NAME]
               [--max-ticks N] [--tick-hz HZ] [--verdict-channel NAME]
               [--threads N] [--jitter FRAC] [--seed U64] [--readiness-timeout SECS]
    luncosim test-component --twin <PATH> --component <NAME>
               [--max-ticks N] [--tick-hz HZ] [--verdict-channel NAME]
               [--threads N] [--jitter FRAC] [--seed U64] [--readiness-timeout SECS]
    luncosim test --list

    --list                   Print the execution kind and every discovered test scene,
                             using the test Rhai program's literal TEST_KIND declaration.
    --scene PATH             REQUIRED. USD scene path. It may be relative to
                             assets/, relative to the current directory, or an
                             absolute path into a custom Twin.
    --max-ticks N            Safety bound on cumulative running fixed steps across scene
                             transitions (default {DEFAULT_MAX_TICKS}).
                             Exhausting it with no verdict exits 2.
    --tick-hz HZ             Manual clock step rate (default {hz}, = lunco_core_runtime::FIXED_HZ).
                             Keep it at FIXED_HZ for exactly one physics tick
                             per update.
    --verdict-channel NAME   Only accept a PASS/FAIL from this telemetry channel.
                             Default: the first PASS/FAIL payload on any channel.
    --verification NAME      Require the scene to match a Twin manifest's
                             qualified SysML verification mapping and use its
                             declared verdict channel by default.
    --twin PATH              Twin folder used by `test-component`.
    --component NAME         Stable `[[components]].name` selected by
                             `test-component`; its manifest verification owns
                             the scene and Rhai observer that are executed.

DIAGNOSTIC AXES (default settings match the gate profile):
    --threads N              Compute-pool threads (default 1).
                               1  pin one Compute thread — the gate profile.
                               0  use Bevy's default task-pool allocation,
                                  also used by GUI DefaultPlugins.
                               N  pin N compute threads.
                             This changes Compute width only; IO and
                             AsyncCompute retain their Bevy assignments.
    --jitter FRAC            Fractional dt jitter in [0.0, 1.0) (default 0.0).
                             0.0 keeps the exact fixed step. Above 0, each
                             update advances by a seeded pseudo-random dt in
                             [(1-FRAC)*base, (1+FRAC)*base], approximating
                             variable frame pacing. This does not reproduce
                             GUI-only behavior or prove a dt-sensitivity root
                             cause by itself. A fixed seed repeats the dt
                             sequence, not necessarily the simulation outcome.
    --seed U64               Seed for the jitter PRNG (default {seed}).
                             Same seed => same jitter dt sequence.
    --readiness-timeout SECS Wall-clock budget for scene materialization and
                             asynchronous Modelica/physics readiness (default
                             {readiness_timeout}s). A timeout is a no-verdict
                             failure; it is independent of simulated max-ticks.

EXIT CODES:
    0  scenario emitted PASS
    1  scenario emitted FAIL
    2  no verdict (max ticks exhausted, early app exit, or bad arguments)",
        hz = lunco_core_runtime::FIXED_HZ,
        seed = DEFAULT_SEED,
        readiness_timeout = DEFAULT_READINESS_TIMEOUT_SECS,
    )
}

/// Resolve and validate a Twin-owned verification selection before constructing
/// the application.  The scene runner still executes the authored Rhai
/// observer; this guard proves that the requested qualified SysML case,
/// component ownership, scene/script mapping, and verdict channel agree in
/// the Twin manifest.
fn apply_verification_selection(cli: &mut Cli) -> Result<(), String> {
    let Some(requested) = cli.verification.as_deref() else {
        return Ok(());
    };
    if requested.trim().is_empty() {
        return Err("--verification requires a non-empty qualified SysML name".to_owned());
    }

    let given = Path::new(&cli.scene);
    let absolute = if given.is_absolute() {
        given.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot resolve scene directory: {error}"))?
            .join(given)
    };
    let absolute = absolute
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize scene `{}`: {error}", cli.scene))?;

    let mut candidate = absolute.parent();
    while let Some(root) = candidate {
        if root.join(lunco_twin::MANIFEST_FILENAME).is_file() {
            let mode = lunco_twin::TwinMode::open(root)
                .map_err(|error| format!("cannot open Twin at {}: {error}", root.display()))?;
            let twin = match mode {
                lunco_twin::TwinMode::Twin(twin) | lunco_twin::TwinMode::Folder(twin) => twin,
                lunco_twin::TwinMode::Orphan(_) => {
                    return Err(format!("scene `{}` is not inside a Twin folder", cli.scene));
                }
            };
            let canonical_root = twin
                .root
                .canonicalize()
                .map_err(|error| format!("cannot canonicalize Twin root: {error}"))?;
            let relative = absolute.strip_prefix(&canonical_root).map_err(|_| {
                format!(
                    "scene `{}` is outside Twin root `{}`",
                    cli.scene,
                    canonical_root.display()
                )
            })?;
            let mut structural = twin.verification_registry_errors();
            structural.extend(twin.component_registry_errors());
            if !structural.is_empty() {
                return Err(format!(
                    "Twin verification/component registry is invalid: {}",
                    structural.join("; ")
                ));
            }
            let case = twin.verification_case(requested).ok_or_else(|| {
                format!(
                    "Twin `{}` has no mapping for SysML verification `{requested}`",
                    twin.manifest
                        .as_ref()
                        .map(|manifest| manifest.name.as_str())
                        .unwrap_or("<folder>")
                )
            })?;
            if case.scene != relative {
                return Err(format!(
                    "verification `{requested}` maps to `{}`, not scene `{}`",
                    case.scene.display(),
                    relative.display()
                ));
            }
            if let Some(expected) = case.verdict_channel.as_deref() {
                if let Some(actual) = cli.verdict_channel.as_deref() {
                    if actual != expected {
                        return Err(format!(
                            "verification `{requested}` requires verdict channel `{expected}`, got `{actual}`"
                        ));
                    }
                } else {
                    cli.verdict_channel = Some(expected.to_owned());
                }
            }
            return Ok(());
        }
        candidate = root.parent();
    }
    Err(format!(
        "--verification requires a Twin manifest enclosing scene `{}`",
        cli.scene
    ))
}

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
        With<lunco_usd_avian_contracts::PendingUsdJoint>,
        With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::RevoluteJoint>>,
        With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::PrismaticJoint>>,
        With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::FixedJoint>>,
        With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::SphericalJoint>>,
        With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::DistanceJoint>>,
    )>>();
    q_pending.iter(world).next().is_none()
}

/// Whether the authored physical scene has crossed its deferred admission
/// boundary without requiring any Modelica solver time.  The scene-test runner
/// uses this while compiled programs are intentionally paused: physics must be
/// seeded from one coherent pose before the first controller step, otherwise a
/// worker that happens to answer one update earlier can tilt the lander before
/// the other participants are even live.
fn physics_admission_ready(world: &mut World) -> bool {
    let readiness_clear = world
        .get_resource::<lunco_readiness::ReadinessState>()
        .is_none_or(|state| !state.world_hold && state.held_entities.is_empty());
    if !readiness_clear {
        return false;
    }

    let mut q_pending = world.query_filtered::<(), Or<(
        With<lunco_usd_avian_contracts::ShouldBeDynamic>,
        With<lunco_core::PhysicsStatePending>,
        With<lunco_usd_avian_contracts::PendingUsdJoint>,
        With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::RevoluteJoint>>,
        With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::PrismaticJoint>>,
        With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::FixedJoint>>,
        With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::SphericalJoint>>,
        With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::DistanceJoint>>,
    )>>();
    q_pending.iter(world).next().is_none()
}

fn set_scene_test_startup_hold(app: &mut App, held: bool) -> Result<(), &'static str> {
    let Some(mut holds) = app
        .world_mut()
        .get_resource_mut::<lunco_physics::PhysicsHolds>()
    else {
        return Err("physics hold resource is not installed");
    };
    holds.set(lunco_physics::PhysicsHolds::SCENE_TEST_STARTUP, held);
    Ok(())
}

/// Pause every compiled Modelica participant at the scene-test startup fence.
/// This is runner policy, not a production vehicle rule: it keeps asynchronous
/// worker completion from becoming a hidden source of initial-condition drift.
fn pause_modelica_participants(world: &mut World) {
    let mut q = world.query_filtered::<&mut ModelicaModel, With<UsdSourcedCosim>>();
    for mut model in q.iter_mut(world) {
        model.paused = true;
        model.is_stepping = false;
        model.in_flight_step = None;
    }
    // No worker request is allowed to remain in flight across the startup
    // fence. Clear the projected barrier together with those model states;
    // otherwise the time spine sees a stale `held` bit, pauses Time<Virtual>,
    // and the first prime request is correctly (but permanently) gated out.
    if let Some(mut barrier) = world.get_resource_mut::<lunco_core_runtime::SimulationBarrier>() {
        barrier.held = false;
        barrier.worst_lag_secs = 0.0;
        barrier.worst_entity = None;
    }
}

/// Release the startup fence in one authored batch once physics admission is
/// complete. Every live solver then sees the same first fixed tick.
fn resume_modelica_participants(world: &mut World) {
    let mut q = world.query_filtered::<&mut ModelicaModel, With<UsdSourcedCosim>>();
    for mut model in q.iter_mut(world) {
        if model.last_error.is_none() && model.is_compiled {
            model.paused = false;
        }
    }
}

/// Wait for every USD-backed solver's first fixed-step exchange and for the
/// shared causal barrier to clear. A compile snapshot is not enough: it contains
/// algebraic defaults, but the first sensor/actuator sample is produced
/// asynchronously. The generic barrier also covers causal participants from
/// adapters beyond USD without duplicating their entity list in this runner.
fn modelica_exchanges_ready(world: &mut World) -> bool {
    let sources_ready = {
        let mut q = world.query_filtered::<&ModelicaModel, With<UsdSourcedCosim>>();
        q.iter(world).all(|model| {
            model.last_error.is_some()
                || (model.is_compiled
                    && !model.is_compiling
                    && !model.paused
                    && !model.is_stepping
                    && model.current_time > 0.0
                    && !model.variables.is_empty())
        })
    };
    if !sources_ready {
        return false;
    }

    // The per-source snapshot above covers USD-backed participants. The shared
    // barrier is the generic owner for every causal participant, including
    // models that enter through another adapter. Do not open the scene-test
    // clock while any such result is still in flight.
    let expected_terminal_fault = world
        .get_resource::<lunco_core::RuntimeFaults>()
        .is_some_and(lunco_core::RuntimeFaults::active)
        && world
            .get_resource::<ExpectedRuntimeFaults>()
            .is_some_and(|expected| !expected.0.is_empty());
    expected_terminal_fault
        || world
            .get_resource::<lunco_core_runtime::SimulationBarrier>()
            .is_some_and(|barrier| !barrier.held)
}

/// Explain a bounded readiness failure with the live state that kept the gate
/// closed. A silent no-verdict is not actionable: the scene runner must name
/// the model, terminal error, pause state, and readiness hold that blocked the
/// scenario so the owning subsystem can be fixed.
fn log_participant_readiness_blockers(world: &mut World) {
    let mut models = world.query::<(
        Entity,
        &ModelicaModel,
        Option<&lunco_cosim_core::SimComponent>,
    )>();
    for (entity, model, component) in models.iter(world) {
        let status = component
            .map(|component| format!("{:?}", component.status))
            .unwrap_or_else(|| "no SimComponent".into());
        warn!(
            "[test] participant blocker entity={entity:?} model={} compiled={} compiling={} paused={} stepping={} current_time={:.6} target_time={:.6} variables={} status={} error={}",
            model.model_name,
            model.is_compiled,
            model.is_compiling,
            model.paused,
            model.is_stepping,
            model.current_time,
            model.target_time,
            model.variables.len(),
            status,
            model.last_error.as_deref().unwrap_or("none"),
        );
    }
    if let Some(barrier) = world.get_resource::<lunco_core_runtime::SimulationBarrier>() {
        warn!(
            "[test] simulation barrier held={} active_participants={} shared_clock_participants={} worst_lag_secs={:.6} worst_entity={:?}",
            barrier.held,
            barrier.active_participants,
            barrier.shared_clock_participants,
            barrier.worst_lag_secs,
            barrier.worst_entity,
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
        .query_filtered::<(Entity, Option<&lunco_usd_bevy_scene::UsdPrimPath>), Or<(
            With<lunco_usd_avian_contracts::PendingUsdJoint>,
            With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::RevoluteJoint>>,
            With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::PrismaticJoint>>,
            With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::FixedJoint>>,
            With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::SphericalJoint>>,
            With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::DistanceJoint>>,
        )>>();
    for (entity, path) in pending_joints.iter(world) {
        warn!(
            "[test] pending joint entity={entity:?} path={}",
            path.map(|path| path.path.as_str())
                .unwrap_or("<no USD path>"),
        );
    }
    let mut pending_usd = world.query_filtered::<
        (Entity, &lunco_usd_avian_contracts::PendingUsdJoint),
        With<lunco_usd_avian_contracts::PendingUsdJoint>,
    >();
    let describe_target = |path: &str| {
        let found = world.iter_entities().find(|entity| {
            entity
                .get::<lunco_usd_bevy_scene::UsdPrimPath>()
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
                .get::<lunco_usd_avian_core::BridgeShadow>()
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
        (Entity, &lunco_usd_avian_contracts::PendingJointAdmission),
        With<lunco_usd_avian_contracts::PendingJointAdmission>,
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

/// Explain a scene-materialization timeout with the live projection state.
/// The scene gate spans the USD load, structural projection, simulation projection,
/// terrain build, and Modelica source lifecycle; reporting only the final
/// boolean would hide which owner failed to publish its completion marker.
fn log_scene_readiness_blockers(world: &mut World) {
    let load_in_flight = world
        .get_resource::<lunco_usd_bevy_runtime_core::scene::SceneLoadInFlight>()
        .is_some();
    let ground_pending = world
        .get_resource::<lunco_usd_sim_core::GroundColliderPending>()
        .is_some_and(|pending| pending.0);
    let mut prims = world.query::<(
        &lunco_usd_bevy_scene::UsdPrimPath,
        Has<lunco_usd_bevy_scene::UsdSceneProjected>,
        Has<lunco_usd_sim_core::UsdSimProcessed>,
    )>();
    let mut prim_count = 0usize;
    let mut visual_count = 0usize;
    let mut unprocessed = Vec::new();
    for (path, visual_synced, sim_processed) in prims.iter(world) {
        prim_count += 1;
        visual_count += usize::from(visual_synced);
        if !sim_processed && unprocessed.len() < 32 {
            unprocessed.push(format!("{} (visual_synced={visual_synced})", path.path));
        }
    }
    let terrain_requests = world
        .query_filtered::<(), With<lunco_terrain_surface::DemTerrainRequest>>()
        .iter(world)
        .count();
    warn!(
        "[test] scene blocker load_in_flight={} prims={} visual_synced={} unprocessed_sample={:?} ground_pending={} terrain_requests={}",
        load_in_flight, prim_count, visual_count, unprocessed, ground_pending, terrain_requests,
    );
}

fn dirty_authored_scene_document(world: &World) -> Option<String> {
    let registry = world.get_resource::<lunco_doc_bevy::DocumentRegistry<UsdDocument>>()?;
    registry.ids().find_map(|doc| {
        let host = registry.host(doc)?;
        let document = host.document();
        if !document.is_dirty() {
            return None;
        }
        let origin = document.origin().canonical_path()?;
        Some(format!(
            "doc={} origin={} generation={} base_revision={} runtime_revision={}",
            doc.raw(),
            origin.display(),
            host.generation(),
            document.base_revision(),
            document.runtime_revision(),
        ))
    })
}

/// Keep the runner-owned lifecycle closed while scene participants warm up.
/// The normal runtime opens scenarios automatically when all initial readiness
/// holds clear; a scene test must inspect its clean authored baseline before
/// its scenario can submit an edit.
fn hold_scenarios_closed(app: &mut App) {
    if let Some(mut gate) = app
        .world_mut()
        .get_resource_mut::<lunco_scripting::scenario::ScenarioExecutionGate>()
    {
        gate.enabled = false;
    }
    if let Some(mut arm) = app
        .world_mut()
        .get_resource_mut::<lunco_scripting::scenario::ScenarioReadinessArm>()
    {
        arm.0 = false;
    }
}

/// A barrier raised inside one FixedUpdate must consume no residual fixed
/// overstep from that render-frame burst. Otherwise a following zero-duration
/// readiness update can run another Rhai/physics tick after the participant has
/// declared the shared clock held. The time spine performs the same operation
/// for an explicit transport command; the headless runner applies it at this
/// generic barrier boundary as well.
fn discard_fixed_overstep_while_barrier_held(app: &mut App) {
    let held = app
        .world()
        .get_resource::<lunco_core_runtime::SimulationBarrier>()
        .is_some_and(|barrier| barrier.held);
    if !held {
        return;
    }
    if let Some(mut fixed) = app.world_mut().get_resource_mut::<Time<Fixed>>() {
        lunco_time::discard_fixed_overstep(&mut fixed);
    }
}

/// Re-project the authoritative transport after a co-simulation warm-up.
///
/// The final participant response can clear `SimulationBarrier` during the
/// last `app.update()` in the readiness loop.  In that case the loop observes
/// readiness and exits before the next `PreUpdate` transport projection,
/// leaving `Time<Virtual>` paused even though `TimeTransport` is playing.  The
/// first authored fixed tick would then never arrive (and a scenario that uses
/// `elapsed_seconds()` would wait forever).  Apply the same generic transport
/// projection at this lifecycle boundary; an authored pause remains a pause,
/// while a playing transport is admitted without consuming a fixed overstep.
fn reproject_test_transport(app: &mut App) {
    let transport = *app.world().resource::<lunco_time::TimeTransport>();
    let causal_hold = app
        .world()
        .get_resource::<lunco_core_runtime::SimulationProgress>()
        .is_some_and(|progress| progress.is_held())
        || app
            .world()
            .get_resource::<lunco_core_runtime::SimulationBarrier>()
            .is_some_and(|barrier| barrier.held);
    if let Some(mut virtual_time) = app.world_mut().get_resource_mut::<Time<Virtual>>() {
        lunco_time::project_transport_state(&transport, &mut virtual_time, None, causal_hold);
    }
}

/// Run the production authored-scene command and return its process exit code.
pub fn run() -> u8 {
    if std::env::args().any(|argument| argument == "--list") {
        return list_scene_tests();
    }

    // Every production scene-test process is throwaway by design. Mark this
    // before app construction so runtime persistence owners cannot restore or
    // write a developer's Twin overlay during the run.
    lunco_twin::request_isolated_run();

    // BEFORE the `App` exists, because building it registers settings sections and
    // that is what loads (and installs the flush for) `settings.json`.
    //
    // A harness must not write the developer's real settings. `is_test_binary()`
    // cannot see this one — a `[[bin]]` lands next to the real app, not in
    // `deps/` — so it says so out loud. The scene-test cadence override must not
    // persist into the developer's later *luncosim* runs, where the normal
    // cadence policy owns the frame cost.
    #[cfg(feature = "ui")]
    lunco_settings::use_ephemeral_settings();

    let mut cli = match parse_args() {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            return 2;
        }
    };
    if let Err(error) = apply_verification_selection(&mut cli) {
        eprintln!("verification selection failed: {error}");
        return 2;
    }

    let dt = Duration::from_secs_f64(1.0 / cli.tick_hz);

    // The app the GUI and the headless server run, assembled exactly as
    // `lunco_luncosim_runtime::build_headless_app_with_threads` does it — asset sources first
    // (they MUST precede `AssetPlugin`, which snapshots the source registry), then
    // the engine plugin group, then `LunCoSimHeadlessPlugin` from the
    // simulation composition package.
    //
    // The core builder receives the compute-pool override at plugin-group build
    // time, keeping scene tests on the same production composition as the server.
    let mut app = lunco_luncosim_runtime::build_headless_app_with_scene(
        (cli.threads > 0).then_some(cli.threads),
        Some(cli.scene.clone()),
    );
    app.add_plugins(LunCoSimHeadlessPlugin::default());
    // The component runner has already resolved the manifest-selected scene.
    // Re-assert that resolved value at the application boundary immediately
    // before startup schedules are built. This keeps the runner independent of
    // process argv and makes the scene hand-off explicit for every test mode.
    app.insert_resource(lunco_luncosim_services::ScenePath(Some(cli.scene.clone())));

    // ── Determinism, installed AFTER the core plugin so it wins ──────────────
    //
    // `LunCoSimHeadlessPlugin` inserts a `Time<Virtual>` with `max_delta = 33 ms` —
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
    app.insert_resource(SceneTestTickCount::default());
    app.add_systems(
        FixedUpdate,
        count_scene_test_tick
            .in_set(lunco_core::RuntimeCycleSet::Simulation)
            .after(lunco_core_runtime::SimTickSet),
    );
    // THE determinism knob: the clock stops reading the wall (see module docs).
    // With `--jitter` this resource is re-set before each update; it is still
    // `ManualDuration`, so the wall clock never enters the run either way.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(dt));
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
    hold_scenarios_closed(&mut app);
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
    // Freeze the sim clock while the scene finishes structural loading: run `Update`
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
    // Readiness is asynchronous wall-clock work (asset IO plus Modelica source
    // compilation/solver preparation). A fixed number of updates makes the
    // result depend on how quickly this process is scheduled, and is therefore
    // not a valid liveness contract. One budget covers both startup phases so a
    // slow scene cannot receive two independent extensions.
    let readiness_started = Instant::now();
    let load_waits = {
        let mut waits = 0u32;
        while readiness_started.elapsed() < cli.readiness_timeout {
            hold_scenarios_closed(&mut app);
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
                .get_resource::<lunco_usd_bevy_runtime_core::scene::SceneLoadInFlight>()
                .is_none();
            let load_finished = load_done
                && app
                    .world_mut()
                    .query_filtered::<(), With<lunco_usd_bevy_scene::UsdPrimPath>>()
                    .iter(app.world())
                    .next()
                    .is_some();
            // Every USD sim prim processed into its sim components.
            let all_processed = app
                .world_mut()
                .query_filtered::<(), (
                    With<lunco_usd_bevy_scene::UsdPrimPath>,
                    Without<lunco_usd_sim_core::UsdSimProcessed>,
                )>()
                .iter(app.world())
                .next()
                .is_none();
            let gate_clear = !app
                .world()
                .resource::<lunco_usd_sim_core::GroundColliderPending>()
                .0;
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
    let scene_readiness_elapsed = readiness_started.elapsed();
    println!(
        "[test] scene-readiness freeze held {load_waits} updates ({:.1}s wall)",
        scene_readiness_elapsed.as_secs_f64()
    );

    let scene_ready = app
        .world()
        .get_resource::<lunco_usd_bevy_runtime_core::scene::SceneLoadInFlight>()
        .is_none()
        && app
            .world_mut()
            .query_filtered::<(), With<lunco_usd_bevy_scene::UsdPrimPath>>()
            .iter(app.world())
            .next()
            .is_some()
        && app
            .world_mut()
            .query_filtered::<(), (
                With<lunco_usd_bevy_scene::UsdPrimPath>,
                Without<lunco_usd_sim_core::UsdSimProcessed>,
            )>()
            .iter(app.world())
            .next()
            .is_none()
        && !app
            .world()
            .resource::<lunco_usd_sim_core::GroundColliderPending>()
            .0
        && modelica_sources_terminal(app.world_mut());
    if !scene_ready {
        log_scene_readiness_blockers(app.world_mut());
        println!(
            "luncosim test NO-VERDICT  scene={}  — scene materialization did not complete \
             within the {:.1}s readiness timeout",
            cli.scene,
            cli.readiness_timeout.as_secs_f64()
        );
        return 2;
    }
    // All source programs are terminal now, but physics admission still needs
    // fixed ticks to seed poses and validate support. Freeze the compiled
    // Modelica participants while that happens. Otherwise whichever worker
    // response arrives first can advance one controller against a still-held
    // body, making the first thrust vector (and the eventual landing attitude)
    // depend on wall-clock scheduling.
    pause_modelica_participants(app.world_mut());
    if let Err(error) = set_scene_test_startup_hold(&mut app, true) {
        eprintln!("luncosim test NO-VERDICT  scene={}  — {error}", cli.scene);
        return 2;
    }
    app.insert_resource(TimeUpdateStrategy::ManualDuration(dt));

    let admission_waits = {
        let mut waits = 0u32;
        while readiness_started.elapsed() < cli.readiness_timeout {
            if physics_admission_ready(app.world_mut()) {
                break;
            }
            hold_scenarios_closed(&mut app);
            app.update();
            waits += 1;
            std::thread::yield_now();
            if app.should_exit().is_some() {
                break;
            }
        }
        waits
    };
    let admission_is_ready = physics_admission_ready(app.world_mut());
    println!(
        "[test] physics-admission warmup held {admission_waits} updates ({:.1}s wall)",
        readiness_started.elapsed().as_secs_f64()
    );
    if !admission_is_ready {
        log_participant_readiness_blockers(app.world_mut());
        println!(
            "luncosim test NO-VERDICT  scene={}  — physics admission did not complete \
             before the {:.1}s readiness timeout",
            cli.scene,
            cli.readiness_timeout.as_secs_f64()
        );
        return 2;
    }

    // Release all compiled participants together while the runner-owned
    // startup hold keeps their first successful exchange out of the physical
    // initial condition. This reason is independent from the readiness policy
    // hold, which may clear during the final body-admission update.
    resume_modelica_participants(app.world_mut());
    let participant_waits = {
        let mut waits = 0u32;
        while readiness_started.elapsed() < cli.readiness_timeout {
            // Worker responses land in Update, after this frame's
            // PreUpdate readiness projection. Wait for both the first exchange
            // and its cleared readiness state before leaving admission.
            if modelica_exchanges_ready(app.world_mut()) && participants_ready(app.world_mut()) {
                break;
            }
            // Advance only when no causal result is in flight. Once the first
            // communication point raises the barrier, pump worker responses at
            // zero duration; when it clears, one more fixed tick may be needed
            // to reach another participant's first communication point. This
            // keeps readiness latency out of the physical initial condition
            // while still allowing models with different periods to prime.
            let barrier_held = app
                .world()
                .get_resource::<lunco_core_runtime::SimulationBarrier>()
                .is_some_and(|barrier| barrier.held);
            app.insert_resource(TimeUpdateStrategy::ManualDuration(if barrier_held {
                Duration::ZERO
            } else {
                dt
            }));
            hold_scenarios_closed(&mut app);
            app.update();
            waits += 1;
            discard_fixed_overstep_while_barrier_held(&mut app);
            std::thread::yield_now();
            if app.should_exit().is_some() {
                break;
            }
        }
        waits
    };
    let participants_are_ready =
        modelica_exchanges_ready(app.world_mut()) && participants_ready(app.world_mut());
    if participants_are_ready {
        if let Err(error) = set_scene_test_startup_hold(&mut app, false) {
            eprintln!("luncosim test NO-VERDICT  scene={}  — {error}", cli.scene);
            return 2;
        }
        // The final worker response may have released the coupling barrier in
        // the same update that made the participant set ready.  Do not wait for
        // another render frame to project the playing transport: the test clock
        // must be live before the scenario gate opens below.
        reproject_test_transport(&mut app);
        // The final participant iteration may have used a zero manual duration
        // while the barrier was held.  The response can clear that barrier in
        // the same update that makes the set ready, so restore the authored
        // fixed step before entering the test loop.
        app.insert_resource(TimeUpdateStrategy::ManualDuration(dt));
    }
    println!(
        "[test] participant-readiness warmup held {participant_waits} updates ({:.1}s wall)",
        readiness_started.elapsed().as_secs_f64()
    );
    if !participants_are_ready {
        log_participant_readiness_blockers(app.world_mut());
        println!(
            "luncosim test NO-VERDICT  scene={}  — participant readiness did not complete \
             before the {:.1}s readiness timeout",
            cli.scene,
            cli.readiness_timeout.as_secs_f64()
        );
        return 2;
    }

    if let Some(dirty) = dirty_authored_scene_document(app.world()) {
        println!(
            "luncosim test FAIL scene={} — authored document was dirty before scenario start: {dirty}",
            cli.scene
        );
        return 1;
    }

    // The scene and its asynchronous participants are ready now. Install the
    // manual scene-test clock policy only for the authored simulation: the
    // production cadence policy must remain active while wall-clock asset IO,
    // USD projection, and Modelica preparation are being pumped above. EXACT
    // solves the celestial tree every update, which is correct for verdicts but
    // needlessly blocks readiness when applied to the zero-time load phase.
    app.insert_resource(lunco_celestial_spatial::cadence::CelestialCadenceSettings::EXACT);
    if let Some(mut gate) = app
        .world_mut()
        .get_resource_mut::<lunco_scripting::scenario::ScenarioExecutionGate>()
    {
        gate.enabled = true;
    }
    // Count completed, clock-admitted fixed steps in a runner-owned resource.
    // This process-level horizon survives authored scene transitions, which
    // intentionally reset scene-local simulation time. Counting `app.update()`
    // calls would include barrier waits and make the same physical horizon
    // depend on worker scheduling.
    app.world_mut().resource_mut::<SceneTestTickCount>().0 = 0;
    let mut ticks = 0u64;
    let mut updates = 0u64;
    let mut early_exit = false;
    let mut sim_seconds = 0.0f64;
    let mut rng = Xorshift64Star::new(cli.seed);
    // A coupled Modelica step is asynchronous by design. Give it bounded
    // update iterations to release the barrier, while keeping `max_ticks` a
    // true fixed-step horizon. This avoids a false NO-VERDICT at the exact
    // moment the final step request is dispatched.
    let max_updates = cli.max_ticks.saturating_mul(32).max(cli.max_ticks).max(1);
    while ticks < cli.max_ticks && updates < max_updates {
        // jitter == 0 short-circuits to `dt` bit-for-bit and never advances the
        // PRNG, so the default path is byte-identical to the pre-jitter runner.
        let step = rng.next_dt(dt, cli.jitter);
        if cli.jitter > 0.0 {
            app.insert_resource(TimeUpdateStrategy::ManualDuration(step));
        }
        app.update();
        updates += 1;
        ticks = app.world().resource::<SceneTestTickCount>().0;
        sim_seconds = ticks as f64 / cli.tick_hz;

        // Let an in-flight Modelica worker make progress before the next
        // zero-delta barrier update. This is a scheduling yield only; no wall
        // duration enters either clock or the authored physics state.
        if app
            .world()
            .get_resource::<lunco_core_runtime::SimulationBarrier>()
            .is_some_and(|barrier| barrier.held)
        {
            discard_fixed_overstep_while_barrier_held(&mut app);
            std::thread::yield_now();
        }

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
                use lunco_luncosim_edit_ui::selection::{Selected, compute_selection_aabb};
                use lunco_usd_bevy_scene::UsdPrimPath;

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
                                Without<lunco_core::programs::ProgramDriverId>,
                                Without<lunco_core::NoSelectionBounds>,
                            )>();
                    let mut state_children = app.world_mut().query::<&Children>();
                    let mut state_skip = app.world_mut().query_filtered::<(), Or<(
                        With<big_space::prelude::Grid>,
                        With<big_space::prelude::CellCoord>,
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
    // does not say which threads×jitter profile produced it cannot be
    // attributed, and an unattributable result is not evidence of anything.
    let threads_desc = if cli.threads == 0 {
        "default".to_string()
    } else {
        cli.threads.to_string()
    };
    let cfg = format!(
        "threads={threads_desc}  jitter={:.3}  seed={}  tick_hz={:.4}",
        cli.jitter, cli.seed, cli.tick_hz
    );

    // A PASS cannot stand while the engine is still reporting a terminal
    // runtime fault. This is a separate causal boundary from wiring: a
    // rejected client-prediction loop, non-finite force, or an explicitly
    // world-pausing escape policy must be reported as that fault, not disguised
    // as a dangling connection. The default object-scoped escape action is
    // recoverable and keeps the rest of the solver available to this verdict.
    let expected_runtime = app.world().resource::<ExpectedRuntimeFaults>().0.clone();
    if let Some(fault) = app
        .world()
        .get_resource::<lunco_core::RuntimeFaults>()
        .and_then(|faults| faults.first.as_ref())
    {
        if expected_runtime.contains(fault.kind) {
            println!(
                "luncosim test PASS  scene={}  expected terminal runtime fault kind={} subject={}  ticks={ticks}  updates={updates}  sim={sim_seconds:.2}s  {cfg}",
                cli.scene, fault.kind, fault.subject
            );
            return 0;
        }
        println!(
            "luncosim test FAIL  scene={}  ticks={ticks}  updates={updates}  sim={sim_seconds:.2}s  {cfg}",
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
            "luncosim test FAIL  scene={}  ticks={ticks}  updates={updates}  sim={sim_seconds:.2}s  {cfg}",
            cli.scene
        );
        println!(
            "  the scenario declared expect_runtime_fault({}) but no terminal runtime fault was raised",
            expected_runtime
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
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
        .get_resource::<lunco_cosim_core::CosimDiagnostics>()
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
            "luncosim test FAIL  scene={}  ticks={ticks}  updates={updates}  sim={sim_seconds:.2}s  {cfg}",
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
                "luncosim test FAIL  scene={}  channel={channel}  ticks={ticks}  updates={updates}  sim={sim_seconds:.2}s  {cfg}",
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
                "luncosim test PASS  scene={}  channel={channel}  ticks={ticks}  updates={updates}  sim={sim_seconds:.2}s  {cfg}",
                cli.scene
            );
            0
        }
        Some((channel, false)) => {
            println!(
                "luncosim test FAIL  scene={}  channel={channel}  ticks={ticks}  updates={updates}  sim={sim_seconds:.2}s  {cfg}",
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
                "luncosim test NO-VERDICT  scene={}  ticks={ticks}  updates={updates}  sim={sim_seconds:.2}s  {cfg}  — {why}",
                cli.scene
            );
            2
        }
    }
}

/// Print the authoritative scene-test catalog without constructing Bevy or a
/// renderer. The shell gates consume this as `KIND<TAB>assets-relative-scene`.
fn list_scene_tests() -> u8 {
    let scenes_dir = lunco_assets_core::engine_scene_tests_root();
    let assets_root = lunco_assets_core::assets_dir_abs();
    let tests = match lunco_scene_validation::test_discovery::discover_scene_tests(&scenes_dir) {
        Ok(tests) => tests,
        Err(error) => {
            eprintln!("scene test discovery failed: {error}");
            return 2;
        }
    };

    for test in tests {
        let scene = test
            .scene_path
            .strip_prefix(&assets_root)
            .expect("discovered scene is below assets root")
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        println!("{}\t{scene}", test.kind.as_str());
    }
    0
}
