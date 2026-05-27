//! Generic engineering workbench for testing any Modelica model.

use bevy::prelude::*;
use bevy_egui::EguiPlugin;
use lunco_modelica::ModelicaPlugin;

fn main() {
    // Cap rayon's global pool to leave headroom for Bevy's renderer.
    //
    // History: when projection + ast_refresh still ran on rayon, the
    // unconfigured pool grabbed `num_cpus - 1` threads and starved
    // the renderer's pipelined extract — every Add/Move edit froze
    // the UI for 1.5–2.5 s. Hard cap at 2 fixed it.
    //
    // After the SyntaxCache refactor (commits TBD), projection +
    // ast_refresh both run on Bevy's `AsyncComputeTaskPool`, NOT on
    // rayon. The only remaining rayon caller is rumoca's
    // `parse_files_parallel`, which fires once at compile-time MSL
    // preload and again per file load — short bursts, not background
    // work that races the renderer. A cap of 2 there made first-
    // compile MSL preload 8× slower than CLI (~64 s vs 8 s wall;
    // worse under contention).
    //
    // New policy: leave 2 cores for Bevy (renderer + main), give the
    // rest to rumoca. On a 16-core machine that's 14 threads — close
    // to CLI parity. On low-core machines (≤4) we still cap at 2
    // because the original starvation problem dominates there.
    let n_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let rayon_threads = if n_cpus <= 4 { 2 } else { n_cpus.saturating_sub(2) };
    let rayon_init = rayon::ThreadPoolBuilder::new()
        .num_threads(rayon_threads)
        .build_global();
    match rayon_init {
        Ok(()) => eprintln!(
            "[lunica] rayon global pool capped at {rayon_threads} threads (of {n_cpus} CPUs)"
        ),
        Err(e) => eprintln!(
            "[lunica] WARN: rayon already initialised, our cap LOST: {e}"
        ),
    }

    // Mirror `LunCoApiConfig::from_args` so the title can advertise
    // the listening port — automation drives the workbench via this
    // port, having it visible in the title bar avoids confusion when
    // multiple instances run side-by-side (e.g. user on 3000 + a
    // sandboxed test on 3001).
    let api_port: Option<u16> = {
        let args: Vec<String> = std::env::args().collect();
        let mut port = None;
        for i in 0..args.len() {
            if args[i] == "--api" {
                port = Some(3000);
                if i + 1 < args.len() {
                    if let Ok(p) = args[i + 1].parse::<u16>() {
                        port = Some(p);
                    }
                }
                break;
            }
        }
        port
    };
    let window_title = match api_port {
        Some(p) => format!("Lunica — Listening on {p}"),
        None => "Lunica".to_string(),
    };

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                resolution: bevy::window::WindowResolution::new(1600, 1000),
                position: WindowPosition::Centered(MonitorSelection::Primary),
                ..lunco_workbench::merged_titlebar_window(window_title)
            }),
            // Route the OS X-button through the in-app save-prompt
            // flow (`lifecycle::on_window_close_requested`). Default
            // `true` would close the window immediately on X click,
            // skipping the dirty-doc Save dialogs.
            close_when_requested: false,
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        // Vello-backed diagram canvas — TBD.
        //
        // The pipeline (lunco-canvas's DiagramRenderer trait,
        // EguiRenderer + VelloRenderer backends, per-tab offscreen
        // render targets in `lunco_modelica::ui::vello_canvas`) is
        // landed and renders all MSL geometry primitives. Re-enable
        // by un-commenting the two `add_plugins` lines below once
        // the text-rendering issue (bevy_vello 0.13.1 entities
        // don't appear in offscreen `RenderTarget::Image`) is
        // resolved upstream or worked around. The egui canvas
        // remains the production paint path until then.
        // .add_plugins(bevy_vello::VelloPlugin::default())
        // .add_plugins(lunco_modelica::ui::vello_canvas::VelloCanvasPlugin)
        .add_plugins(lunco_workbench::WorkbenchPlugin)
        .add_plugins(ModelicaPlugin)
        .add_systems(Startup, setup_sandbox);

    #[cfg(feature = "lunco-api")]
    app.add_plugins(lunco_api::LunCoApiPlugin::default());

    // Reactive — repaint on input, animation hints, or external
    // wakes. The HTTP bridge wakes the loop via EventLoopProxy when
    // a request arrives (see lunco-api), so dropping continuous mode
    // doesn't make the API "hang" the way it used to. Big quality-
    // of-life win: focused-but-idle goes from 90+ fps GPU burn to
    // ~0%, and unfocused windows stop spinning fans.
    use bevy::winit::{UpdateMode, WinitSettings};
    app.insert_resource(WinitSettings {
        // Focused: Continuous lets vsync (Fifo present) act as the
        // pacer, so each Update lands on a real vblank. We tried
        // Reactive(1/60s) first to be conservative, but the
        // independent 16.67ms timer drifts against the actual 60Hz
        // refresh — they fall out of phase every ~13 frames, present
        // misses a vsync, and `present_frames` stalls for 16-32ms.
        // That's the 5Hz spike train tracy showed.
        // Unfocused: ReactiveLowPower(1s) keeps fans quiet when the
        // window is in the background. The Modelica plugin overrides
        // this to Continuous while a sim is running (see
        // lunco_modelica::sim_focus).
        focused_mode: UpdateMode::Continuous,
        unfocused_mode: UpdateMode::reactive_low_power(std::time::Duration::from_secs(1)),
    });

    // Cap how much catchup `FixedUpdate` does after a slow frame.
    // Bevy default: a 250ms hitch breeds 15 fixed ticks the next
    // frame — which makes that frame slow too, breeding the next.
    // The cascade is exactly what was producing the 5Hz spike train
    // in the perf HUD. Capping `Time<Virtual>` to 33ms ≈ 2 fixed
    // ticks: residual real time is *dropped* instead of compounded.
    // (`Time<Fixed>` reads its delta from Virtual, so this
    // transitively caps the catchup loop.) Same fix as
    // `sandbox.rs`.
    let mut virtual_time = Time::<Virtual>::default();
    virtual_time.set_max_delta(std::time::Duration::from_millis(33));
    app.insert_resource(virtual_time);

    // Physics fixed timestep: 60 Hz. Modelica stepping runs in
    // FixedUpdate so the worker receives a predictable per-tick dt.
    // Matches the Avian / lunco-cosim convention; the worker hands
    // `time.delta_secs_f64()` straight to `stepper.step()`.
    app.insert_resource(Time::<Fixed>::from_hz(60.0));

    app.run();
}

fn setup_sandbox(mut commands: Commands) {
    // Start empty: the user lands on the Welcome tab, opens whatever
    // they need via Package Browser / Twin / Ctrl+N. Auto-loading
    // Battery was a debug convenience that confused new users —
    // `cargo run` would show a random model with no explanation.
    //
    // `PrimaryEguiContext` pins egui's window-side rendering to *this*
    // camera. Without the explicit marker, adding a second `Camera2d`
    // (e.g. the vello-spike's offscreen camera) makes bevy_egui's
    // auto-context-pick ambiguous and the workbench chrome silently
    // stops rendering to the window — only the offscreen vello content
    // shows up in screenshots.
    commands.spawn((Camera2d, bevy_egui::PrimaryEguiContext));
}

// Phase-0 spike test scene removed; Phase 1 lives in
// `lunco_modelica::ui::vello_canvas`.
