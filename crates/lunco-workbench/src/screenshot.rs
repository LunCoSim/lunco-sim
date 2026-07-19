//! Screenshot capture — the **render-bound half** of `CaptureScreenshot`.
//!
//! # Why it lives in the workbench
//!
//! Taking a picture needs `bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured}`,
//! which live in `bevy_render` → wgpu. That dependency used to sit inside `lunco-api`, behind
//! a `render` feature that was **on by default** — so staying render-free was the *non-default*
//! path, every consumer had to remember `default-features = false`, and **three forgot**
//! (`lunco-doc-bevy`, `lunco-celestial`/`lunco-tutorial`, `lunco-telemetry`). Each silently
//! re-linked a GPU stack into the `--no-ui` server. Cargo's feature unification makes that
//! invisible to code review; only `cargo tree` sees it. A feature you can forget is a trap
//! that fires forever, so `lunco-api` no longer has one: it **cannot** link a renderer.
//!
//! The GPU half belongs wherever "this binary can render" is already true for **every**
//! screenshot-taking binary — and that is this crate:
//!
//! - `lunco-workbench` already links `bevy_render` (it is the egui shell);
//! - **both** GUI binaries add it (`lunco-sandbox` and `lunica`);
//! - the headless server does **not** link it at all;
//! - it already owns app-level capabilities of exactly this kind (see `perf_hud`).
//!
//! Not `lunco-render-bevy`: **`lunica` takes screenshots and has no 3D renderer.** It links
//! `bevy_render` through egui but never adds `LuncoRenderPlugin`, so putting capture there
//! would silently kill the workbench's screenshots — which is what the MCP `capture_screenshot`
//! tool drives.
//!
//! # The seam
//!
//! `CaptureScreenshot` is an ORDINARY command with an ordinary `#[on_command]` handler. The
//! only unusual thing about it is that its answer arrives late — the PNG does not exist until
//! the GPU hands a frame back — so it registers as a **deferred command**
//! (`lunco_api::executor::register_deferred_command`) and answers on the request's
//! correlation id when the capture lands.
//!
//! That mechanism is generic and lives in the substrate; `lunco-api` does not know this
//! command exists. It used to: the executor special-cased the literal string
//! `"CaptureScreenshot"` and carried a bespoke `PendingScreenshotRequest` + a
//! `ScreenshotBackend` marker — the latter a hand-rolled second answer to "does this binary
//! have that command?", which the type registry already answers for every other command.
//! A binary without this plugin never registers the type, so the request resolves as an
//! ordinary `CommandNotFound` — the same way any other absent command does.

use std::io::Cursor;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use lunco_api::executor::{ApiResponseEvent, DeferredCommandAppExt, PendingApiRequest};
use lunco_api::schema::ApiResponse;
use lunco_core::{on_command, register_commands, Command, SceneViewport};
use lunco_tools_bevy::{register_closure_tool, ToolResult};

/// **The one screenshot command.**
///
/// Declared HERE, next to the only implementation, so a binary with no render backend does
/// not advertise a command it cannot execute — `DiscoverSchema` (and hence the MCP tool list
/// and the generated command reference) only sees it when this plugin is added.
///
/// The declared fields are the ones the handler actually reads — which they once were not.
/// The registered type was `CaptureScreenshot {}`, *no fields*, while the executor pulled
/// `save_to_file` / `path` / `region` straight out of the raw params. The schema that MCP
/// agents and `commands-reference.md` are generated from advertised a parameterless command.
#[Command(default)]
pub struct CaptureScreenshot {
    /// Write the PNG to `path` instead of returning the bytes to the caller.
    pub save_to_file: bool,
    /// Destination when `save_to_file`. Empty ⇒ a timestamped name in the cwd.
    pub path: String,
    /// Optional crop `[x, y, w, h]` in physical pixels, applied before save/encode. Empty ⇒
    /// the full frame. Cropping server-side lets a caller zoom into a panel without an
    /// external image tool.
    pub region: Vec<u32>,
}

/// Install the screenshot backend. Added by [`WorkbenchPlugin`](crate::WorkbenchPlugin), so
/// every binary with a workbench can take a picture — 3D or egui-only alike.
pub struct ScreenshotPlugin;

impl Plugin for ScreenshotPlugin {
    fn build(&self, app: &mut App) {
        // Registers the TYPE (so `DiscoverSchema` sees it, and so a binary without this
        // plugin cleanly reports "command not found") AND marks it deferred (so the executor
        // holds the HTTP response open for the PNG instead of answering `command_accepted`
        // and making the caller poll).
        app.register_deferred_command::<CaptureScreenshot>()
            .init_resource::<PendingCapture>()
            .add_observer(deliver_screenshot);

        // Offline Frame-by-Frame Recording Mode
        app.init_resource::<lunco_core::KeepAwake>()
            .init_resource::<OfflineRecordingState>()
            .add_observer(deliver_offline_frame)
            // The readiness gate. `Update` (not `Last`): it must run before
            // `drive_offline_clock`, which only acts once `state.active` is set —
            // so the shot begins on the same frame it was cleared to begin.
            .add_systems(Update, start_recording_when_scene_ready)
            // `Last`: the strategy written here is read by `TimeSystem` in `First`
            // next frame, so the decision is made after every other system has run.
            .add_systems(Last, drive_offline_clock);

        // `init_resource` first: the registry is shared by every plugin that
        // publishes a query, so whether it already exists depends on plugin
        // ORDER. Reaching straight for `resource_mut` made this plugin panic
        // whenever it was built before whichever plugin happened to insert the
        // registry. `init_resource` is idempotent and leaves an existing
        // registry (and its already-registered providers) untouched.
        app.init_resource::<lunco_api::queries::ApiQueryRegistry>();
        app.world_mut()
            .resource_mut::<lunco_api::queries::ApiQueryRegistry>()
            .register(GetOfflineRecordingStatusProvider);

        // The `science::take_photo` tool fires `CaptureFromCamera`, so it is advertised only
        // where that command actually exists.
        register_science_tools();
        // Registers the observers AND the reflected types for both commands — including
        // `CaptureFromCamera`, which is NOT deferred: it is fire-and-forget (a behaviour-tree
        // leaf or a rhai `photo()`, neither holding an HTTP response open).
        register_all_commands(app);
    }
}

/// What the in-flight capture should do when it lands. Set by the handler, read by the
/// `ScreenshotCaptured` observer.
#[derive(Resource, Default, Debug, Clone)]
struct PendingCapture {
    /// Answer the HTTP request on this id (raw-PNG mode). `None` ⇒ `save_to_file`, whose
    /// response was already sent.
    correlation_id: Option<u64>,
    save_path: Option<String>,
    region: Option<(u32, u32, u32, u32)>,
}

/// An ordinary command handler. It arms the capture and returns; the answer is sent by
/// [`deliver_screenshot`] once the GPU hands the frame back.
#[on_command(CaptureScreenshot)]
fn on_capture_screenshot(
    trigger: On<CaptureScreenshot>,
    pending_request: Res<PendingApiRequest>,
    mut pending: ResMut<PendingCapture>,
    mut commands: Commands,
) {
    let cmd = trigger.event();

    // A 4-element `[x, y, w, h]`, or nothing. A malformed region is ignored rather than
    // rejected — cropping is a convenience, and a bad crop should not cost you the frame.
    let region = match cmd.region.as_slice() {
        [x, y, w, h] => Some((*x, *y, *w, *h)),
        _ => None,
    };

    *pending = if cmd.save_to_file {
        // Empty ⇒ we pick a timestamped name. Reaching for a wall clock is not something the
        // render-free substrate should do, so that default lives here.
        let path = if cmd.path.is_empty() { timestamped_name() } else { cmd.path.clone() };

        // ANSWER NOW. A deferred command owes the caller EXACTLY ONE response on its
        // correlation id — the executor no longer sends one on its behalf. In save-to-file
        // mode the useful answer (the path) is known immediately and there is nothing to wait
        // for, so send it here rather than after the capture. Forgetting this is not a
        // cosmetic bug: the caller would hang until the HTTP timeout.
        commands.trigger(ApiResponseEvent {
            correlation_id: pending_request.correlation_id,
            response: ApiResponse::ok(serde_json::json!({ "path": path })),
        });

        PendingCapture { correlation_id: None, save_path: Some(path), region }
    } else {
        PendingCapture {
            correlation_id: Some(pending_request.correlation_id),
            save_path: None,
            region,
        }
    };

    // Spawned HERE, not by a domain-side observer. It used to be the latter, and that
    // observer only shipped in `lunco-avatar` — so binaries that didn't pull it in (lunica,
    // hello_workbench) never produced a screenshot at all: curl simply hung.
    commands.spawn(Screenshot::primary_window());
}

register_commands!(
    on_capture_screenshot,
    on_capture_from_camera,
    on_start_offline_recording,
    on_stop_offline_recording
);

/// The picture landed — crop, encode, and either save it or answer the deferred request.
fn deliver_screenshot(
    trigger: On<ScreenshotCaptured>,
    mut pending: ResMut<PendingCapture>,
    mut commands: Commands,
) {
    let event = trigger.event();
    let correlation_id = pending.correlation_id.take();
    let save_path = pending.save_path.take();
    let region = pending.region.take();

    let Ok(mut dyn_img) = event.image.clone().try_into_dynamic() else {
        error!("[screenshot] failed to convert the captured image");
        return;
    };

    // Crop to the requested region, clamped to the image bounds.
    if let Some((x, y, w, h)) = region {
        let (iw, ih) = (dyn_img.width(), dyn_img.height());
        if x < iw && y < ih && w > 0 && h > 0 {
            let cw = w.min(iw - x);
            let ch = h.min(ih - y);
            dyn_img = dyn_img.crop_imm(x, y, cw, ch);
        } else {
            error!(
                "[screenshot] region {:?} lies outside the {}x{} image — saving the full frame",
                region, iw, ih
            );
        }
    }

    if let Some(path) = save_path {
        // save_to_file mode — the response was already sent; just write the file.
        if let Err(e) = dyn_img.save(&path) {
            error!("[screenshot] failed to save to '{path}': {e}");
        }
    } else if let Some(cid) = correlation_id {
        // raw-PNG mode — encode and answer the deferred HTTP request.
        let mut png_bytes: Vec<u8> = Vec::new();
        if dyn_img
            .write_to(&mut Cursor::new(&mut png_bytes), image::ImageFormat::Png)
            .is_ok()
        {
            commands.trigger(ApiResponseEvent {
                response: ApiResponse::Screenshot { png_bytes },
                correlation_id: cid,
            });
        } else {
            error!("[screenshot] failed to encode the screenshot as PNG");
        }
    }
}

/// **Capture from a specific vessel's mounted camera** — the typed command behind the
/// `science::take_photo` instrument.
///
/// Lives HERE rather than in `lunco-avatar` (its domain home) for the same reason
/// [`CaptureScreenshot`] does: resolving a `Camera3d` and spawning a `Screenshot` is a
/// render-world readback, and `lunco-avatar` is render-free by construction. A binary with
/// no renderer therefore does not register this command *and* does not advertise the tool —
/// rather than advertising a `take_photo` that captures nothing.
///
/// `default`: `target` must have a reflect default or the executor's constructibility guard
/// drops a no-param call — `photo()` in `control.rhai` sends `{}`. The default (`None`) is
/// exactly the documented "capture from the active scene camera".
#[Command(default)]
pub struct CaptureFromCamera {
    /// Vessel whose mounted camera to capture from. `None` → the active scene camera,
    /// falling back to the primary window when none is bound.
    pub target: Option<Entity>,
}

#[on_command(CaptureFromCamera)]
fn on_capture_from_camera(
    trigger: On<CaptureFromCamera>,
    viewport: Option<Res<SceneViewport>>,
    // `RenderTarget` is a separate component (see `camera_switch.rs`), not a field on
    // `Camera` — query it alongside so we know which window to capture.
    cameras: Query<(&Camera, &Camera3d, &bevy::camera::RenderTarget)>,
    children: Query<&Children>,
    mut commands: Commands,
) {
    let target = trigger.event().target;
    let camera_entity = match target {
        // A specific vessel → find a `Camera3d` among its descendants (its USD `def Camera`
        // mount).
        Some(vessel) => find_descendant_camera(vessel, &cameras, &children),
        // No target → the active scene camera, else `None` (→ primary window).
        None => viewport.as_deref().and_then(|v| v.active_camera),
    };

    // Distinguish "explicit target requested but not found" (a vessel with no camera —
    // capturing the primary window would silently photograph the WRONG viewport, which for a
    // science instrument is worse than no data) from "no target requested" (the
    // active-camera/primary-window fallback is intended). The former warns + no-ops.
    let Some(camera_entity) = camera_entity else {
        if target.is_some() {
            warn!(
                "[CaptureFromCamera] target vessel has no Camera3d descendant; not capturing \
                 (would photograph the wrong viewport)"
            );
            return;
        }
        commands.spawn(Screenshot::primary_window());
        return;
    };

    // Bevy's `Screenshot` captures a render TARGET (window/image), not a camera directly.
    let Ok((cam, _, rt)) = cameras.get(camera_entity) else {
        commands.spawn(Screenshot::primary_window());
        return;
    };

    // Capturing a WINDOW captures whatever camera is actually drawing it — not necessarily
    // the camera we resolved. A vessel's mounted camera is usually INACTIVE (the operator is
    // flying the free camera), so capturing the window here would photograph the operator's
    // viewport and pass it off as the vessel's instrument data. Refuse instead.
    //
    // Making this capture an inactive mounted camera needs a render-to-image target
    // (`RenderTarget::Image` + `Screenshot::image`) so the camera renders its own view
    // off-screen regardless of what the window shows. Until then, an explicit vessel capture
    // only succeeds when its camera is live.
    if target.is_some() && !cam.is_active {
        warn!(
            "[CaptureFromCamera] target vessel's camera is not active; not capturing (a window \
             capture would photograph the operator's viewport, not the vessel's). Needs a \
             render-to-image target for inactive mounted cameras."
        );
        return;
    }

    let screenshot = match rt {
        bevy::camera::RenderTarget::Window(w) => match w {
            bevy::window::WindowRef::Primary => Screenshot::primary_window(),
            bevy::window::WindowRef::Entity(entity) => Screenshot::window(*entity),
        },
        // Image/texture-view targets aren't capturable via `Screenshot` (they'd
        // double-render); fall back to the primary window.
        _ => Screenshot::primary_window(),
    };
    commands.spawn(screenshot);
}

/// Walk `root`'s descendants (BFS) and return the first `Camera3d` — a vessel's mounted camera.
fn find_descendant_camera(
    root: Entity,
    cameras: &Query<(&Camera, &Camera3d, &bevy::camera::RenderTarget)>,
    children: &Query<&Children>,
) -> Option<Entity> {
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if cameras.get(entity).is_ok() {
            return Some(entity);
        }
        if let Ok(kids) = children.get(entity) {
            stack.extend(kids.iter());
        }
    }
    None
}

/// Register the science instrument tools into the global `lunco_tools` registry, so a
/// behaviour tree's `run_tool` leaf can fire them.
///
/// The closure IS the tool definition and triggers its typed command directly via
/// `world.trigger(...)` — no JSON, no reflection. Registered from [`ScreenshotPlugin`]
/// because the command it fires is implemented here.
fn register_science_tools() {
    register_closure_tool(
        "science::take_photo",
        vec!["take_photo/0".into()],
        |world, vessel, _gid, _args| {
            // The command's observer resolves the vessel's `Camera3d` descendant and captures
            // from the window it renders to. A vessel with no camera no-ops with a warn.
            world.trigger(CaptureFromCamera { target: Some(vessel) });
            ToolResult::Ok
        },
    );
}

/// `web_time`, not `std::time`: `std::time::SystemTime::now()` panics on wasm32 and trips the
/// `disallowed_methods` lint.
fn timestamped_name() -> String {
    let secs = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("screenshot_{secs}.png")
}

use bevy::time::TimeUpdateStrategy;

// ─── Offline Frame-by-Frame recording mode ───────────────────────────────────
//
// Recording touches three independent knobs. Each has exactly ONE writer — the
// bug this design exists to prevent is two systems writing the same knob and the
// last one each frame silently winning:
//
// * **How far virtual time advances** — `TimeUpdateStrategy`, written only by
//   `drive_offline_clock`. Exactly `1/fps` per captured frame, so the output is
//   locked to `fps` no matter how fast or slow the machine renders.
// * **Whether the app may sleep** — `WinitSettings`, written only by the pacer
//   (`lunco-modelica`'s `sim_focus_pace`). Recording states intent by holding a
//   `lunco_core::KeepAwake` token; it never writes the setting itself.
// * **How fast frames present** — `Window::present_mode`, written only here.
//   Uncapped while recording so rendering runs at max speed.
//
// Wall-clock rate and output frame rate are therefore fully decoupled: rendering
// faster changes only how long a capture takes, never what the video looks like.

/// State for the lock-step offline frame recording.
#[derive(Resource, Default, Debug, Clone)]
pub struct OfflineRecordingState {
    /// Whether the recording is active.
    pub active: bool,
    /// Sequential frame index.
    pub frame_index: u64,
    /// Destination directory.
    pub output_dir: std::path::PathBuf,
    /// Video target FPS (determines delta virtual time step per frame).
    pub fps: u32,
    /// Lock-step frame latch.
    pub is_waiting_for_frame: bool,
    /// Set by `deliver_offline_frame` when a frame lands; consumed by
    /// `drive_offline_clock` to schedule exactly one `1/fps` time step.
    pub frame_just_captured: bool,
    /// Primary window present mode as it was before recording uncapped it,
    /// restored on stop.
    pub prev_present_mode: Option<bevy::window::PresentMode>,
}

/// Command to start frame-by-frame recording.
#[Command(default)]
pub struct StartOfflineRecording {
    /// Target folder. Empty => 'recorded_frames' in the current working dir.
    pub output_dir: String,
    /// Video target FPS (default: 60).
    pub fps: u32,
}

/// Command to stop frame-by-frame recording.
#[Command(default)]
pub struct StopOfflineRecording {}

/// **Arms** the recorder; it does not start it.
///
/// Starting here is what the readiness gate exists to prevent: scene loading is
/// asynchronous (USD layers compose, meshes/materials resolve, textures stream,
/// pipelines compile), so a shot that began the instant it was asked captured its
/// opening frames from an unfinished scene — untextured placeholder geometry, a
/// missing DomeLight starfield, a texture that pops in three frames later. Those
/// frames are permanent: the recorder writes a fixed-rate sequence, and the only
/// remedy was re-recording the whole ~10 minute episode.
///
/// So this validates the destination, stashes the config in [`PendingShotStart`],
/// and returns. [`start_recording_when_scene_ready`] does the actual activation
/// once [`scene_visuals_ready`] says so (or the deadline passes).
///
/// **The clock is deliberately left alone here.** `TimeUpdateStrategy` has exactly
/// ONE writer — `drive_offline_clock` — and that is load-bearing (two writers once
/// produced a 3380-frame runaway). Waiting therefore happens with the clock in its
/// ordinary `Automatic` mode: the wait is real-time and consumes no recorded frames,
/// so the deterministic capture still begins at step 0 with N frames == N steps.
#[on_command(StartOfflineRecording)]
fn on_start_offline_recording(
    trigger: On<StartOfflineRecording>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let dir = if cmd.output_dir.is_empty() {
        std::env::current_dir().unwrap_or_default().join("recorded_frames")
    } else {
        std::path::PathBuf::from(&cmd.output_dir)
    };

    // Fail here rather than after the wait: an unwritable destination is the
    // caller's mistake and should be reported at the point of the request.
    if let Err(e) = std::fs::create_dir_all(&dir) {
        error!("[offline-record] failed to create output directory {}: {e}", dir.display());
        return;
    }

    info!(
        "[offline-record] armed for {} at {} FPS — waiting for the scene's visuals to load",
        dir.display(),
        cmd.fps.max(1)
    );
    commands.insert_resource(PendingShotStart {
        output_dir: dir,
        fps: cmd.fps.max(1),
        requested_at: web_time::Instant::now(),
        ready_streak: 0,
        ready_since: None,
        last_blocker: None,
    });
}

/// Flip the armed recorder live. Split out of [`on_start_offline_recording`] so the
/// ready path and the timeout path cannot drift apart.
fn activate_recording(
    pending: &PendingShotStart,
    state: &mut OfflineRecordingState,
    keep_awake: &mut lunco_core::KeepAwake,
    windows: &mut Query<&mut Window, With<bevy::window::PrimaryWindow>>,
    commands: &mut Commands,
) {
    let dir = pending.output_dir.clone();
    state.active = true;
    state.frame_index = 0;
    state.output_dir = dir;
    state.fps = pending.fps;
    state.is_waiting_for_frame = false;
    // Enter the cycle on the "advance" phase so the very first captured frame is
    // rendered from a clock that has taken exactly one step, like every frame after it.
    state.frame_just_captured = true;

    // Ask to stay awake for the duration of the recording. An unattended capture
    // has no focused window, so the `reactive_low_power` throttle would otherwise
    // apply: the app sleeps between redraws and the lock-step advances only when
    // the reactive timer fires — measured at 2-10 s per frame against ~50 ms
    // awake, turning a ~1 minute episode into hours.
    //
    // This states intent and stops there; the pacer applies it. Writing
    // `WinitSettings` from here would be reverted on the very next frame anyway.
    keep_awake.acquire();
    info!("[offline-record] power saving disabled (KeepAwake acquired)");

    // Uncap the presentation rate for the same reason: recording wants frames as
    // fast as the machine can render them. Under `Fifo` (vsync) the render loop is
    // pinned to the display's refresh, and the lock-step spends two render frames
    // per captured frame — a hard ~30 captured FPS ceiling on a 60 Hz panel, for
    // output whose playback rate is `fps` regardless. Virtual time still advances
    // exactly `1/fps` per captured frame, so rendering faster changes only how long
    // the capture takes, never what the video looks like.
    if let Ok(mut window) = windows.single_mut() {
        state.prev_present_mode = Some(window.present_mode);
        window.present_mode = bevy::window::PresentMode::AutoNoVsync;
    }

    // Freeze time initially by setting manual duration to 0.
    // This allows guarded simulation systems to execute but see a 0 delta.
    // `drive_offline_clock` owns the strategy from the next frame onward.
    commands.insert_resource(TimeUpdateStrategy::ManualDuration(std::time::Duration::ZERO));

    // THE diagnostic line for "why does frame 0 look wrong?". It names the wait
    // duration and the last thing the gate was blocked on, so the next occurrence
    // is read off the log instead of guessed at.
    info!(
        "[offline-record] started recording to {} at {} FPS using TimeUpdateStrategy \
         (waited {:.2}s for scene visuals; last blocker: {})",
        state.output_dir.display(),
        state.fps,
        pending.requested_at.elapsed().as_secs_f32(),
        pending.last_blocker.as_deref().unwrap_or("none — ready on the first check"),
    );
}

#[on_command(StopOfflineRecording)]
fn on_stop_offline_recording(
    _trigger: On<StopOfflineRecording>,
    mut state: ResMut<OfflineRecordingState>,
    mut keep_awake: ResMut<lunco_core::KeepAwake>,
    mut windows: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
    mut commands: Commands,
) {
    // Disarm unconditionally: a scenario that gives up on a shot while the gate is
    // still waiting must not leave a `PendingShotStart` behind to fire into the
    // *next* shot's directory.
    commands.remove_resource::<PendingShotStart>();

    if state.active {
        state.active = false;
        state.is_waiting_for_frame = false;
        state.frame_just_captured = false;

        // Drop the wake request; the pacer restores the binary's idle policy.
        keep_awake.release();
        if let (Ok(mut window), Some(prev)) = (windows.single_mut(), state.prev_present_mode.take())
        {
            window.present_mode = prev;
        }
        // Restore automatic realtime ticking
        commands.insert_resource(TimeUpdateStrategy::Automatic);
        info!("[offline-record] stopped recording");
    }
}

// ─── The readiness gate ──────────────────────────────────────────────────────

/// A `StartOfflineRecording` that has been accepted but not yet started, because
/// [`scene_visuals_ready`] has not cleared it. Removed when the recorder activates
/// (either ready or timed out) and on `StopOfflineRecording`.
#[derive(Resource, Debug, Clone)]
struct PendingShotStart {
    output_dir: std::path::PathBuf,
    fps: u32,
    /// When the request arrived — drives both the settle timing and the timeout.
    requested_at: web_time::Instant,
    /// Consecutive frames for which every readiness clause held. See
    /// [`SETTLE_FRAMES`].
    ready_streak: u32,
    /// When the current ready streak began. See [`SETTLE_PERIOD`].
    ready_since: Option<web_time::Instant>,
    /// The most recent reason readiness was refused, kept for the start/timeout
    /// log line. Without it a timeout could only say "not ready", which is exactly
    /// the diagnosis a human needs and cannot reconstruct after the fact.
    last_blocker: Option<String>,
}

/// How long the gate will hold a shot before recording anyway.
///
/// It MUST give up. A scene that never becomes ready — an asset that fails to load,
/// a scene with no terrain when terrain was expected — would otherwise hang the shot
/// forever, and because the scenario polls `shot_frame()` (which reports `-1` while
/// we are armed) it would never reach its `StopOfflineRecording` either. The user
/// gets an empty episode with no explanation, which is strictly worse than a
/// slightly-early first frame plus a loud `warn!`. The campaign scripts have their
/// own outer timeout; this sits comfortably inside it.
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Consecutive ready frames required before the shot starts.
///
/// One frame is enough to prove the condition, but not to have *drawn* it. Bevy
/// queues a render pipeline the first time a material/mesh combination is actually
/// visible and compiles it on a later frame; a pipeline still compiling draws
/// nothing (or draws with a fallback). `PipelineCache` — the only authority on
/// "nothing left to compile" — lives in the render world, where no main-world
/// system can observe it, so there is no honest signal to wait on and these extra
/// frames are the substitute.
///
/// 5 = one frame to prove readiness + four to let the render world queue and
/// retire the pipelines for whatever became visible on that frame. The cost is
/// bounded and tiny (5 real frames, well under 100 ms) against the failure it
/// guards: a bad opening frame costs a ~10 minute re-record of the whole episode.
const SETTLE_FRAMES: u32 = 5;

/// Minimum wall-clock time every clause must hold before the shot starts.
///
/// The companion to [`SETTLE_FRAMES`], covering the same pipeline-warm-up hazard on
/// the other axis. Shader compilation happens off the main thread on a wall-clock
/// schedule, so a frame COUNT is the wrong unit for it on its own: once recording
/// is armed the present mode is uncapped and five frames can elapse in ~20 ms,
/// which is not enough for a compile to retire. This floor makes the window mean
/// "quiet for a while" rather than "quiet for a few frames on a machine that
/// happens to render fast".
///
/// 500 ms is invisible against a ~10 minute episode (6 shots ⇒ 3 s total), so it is
/// priced well below the failure it guards.
const SETTLE_PERIOD: std::time::Duration = std::time::Duration::from_millis(500);

/// [`StatusBus`](crate::status_bus::StatusBus) sources whose in-flight work makes the
/// scene un-presentable, for clause (3) of [`scene_visuals_ready`].
///
/// An ALLOWLIST, not "is anything busy at all". The bus is shared with work that has
/// nothing to do with what the camera sees — a Modelica compile, a document save, an
/// MCP request — and the MSL download in particular re-pushes progress every frame
/// from boot (see `status_bus::tests::push_progress_preserves_start_time_…`). Gating
/// on the whole bus would therefore stall every shot until [`READY_TIMEOUT`], adding
/// minutes to an episode and burying the timeout `warn!` under false positives.
///
/// Both entries are published by `lunco-sandbox`, which mirrors state this crate
/// cannot name onto the bus: terrain by `report_terrain_stream_status` (from
/// `lunco_terrain_surface::TerrainStreamStatus`) and scene by
/// `report_scene_spawn_status` (from `lunco_usd_sim::cosim::SceneLoadInFlight` +
/// `UsdAwaitingStage`). The entries are the SAME consts the publishers push under,
/// not copies of their spelling — see
/// [`TERRAIN_SOURCE`](crate::status_bus::TERRAIN_SOURCE) and
/// [`SCENE_SOURCE`](crate::status_bus::SCENE_SOURCE).
const VISUAL_BUSY_SOURCES: &[&str] = &[
    crate::status_bus::TERRAIN_SOURCE,
    crate::status_bus::SCENE_SOURCE,
];

/// **The one definition of "this scene is presentable".** Returns `None` when the
/// scene's visuals have finished loading, or `Some(reason)` naming what is still
/// outstanding — the reason string is what the start/timeout log lines report.
///
/// Three clauses, deliberately composed rather than left as scattered ad-hoc checks:
///
/// 1. **There is geometry to record at all.** A backstop for a scene that spawns
///    without ever setting the `SceneLoadInFlight` guard clause (3) reads: with no
///    meshes, clause (2) is vacuously true and the gate would fire on an empty
///    viewport. This mirrors the guard in `start_camera_paths_when_terrain_ready`
///    (`crates/lunco-sandbox/src/lib.rs:2264`), which likewise refuses to read an
///    empty query as "nothing to wait for".
/// 2. **Every mesh handle is loaded *with its dependencies*.** This is the direct
///    read for "meshes and the materials/textures hanging off them have resolved",
///    and it is what catches the untextured-placeholder opening frame.
/// 3. **No visual subsystem reports in-flight work on the [`StatusBus`]** (see
///    [`VISUAL_BUSY_SOURCES`]). This carries the weight of the condition, and is how
///    the gate shares — rather than re-implements — the existing definitions of
///    ready. `"scene"` is `SceneLoadInFlight`: prims are still spawning, the state
///    that produced the original half-loaded opening frame. `"terrain"` is
///    `TerrainStreamStatus`, the same read the status bar shows and the same one
///    `start_camera_paths_when_terrain_ready` gates camera paths on.
///
///    Going through the bus rather than the resources keeps `lunco-workbench` a
///    UI-shell crate: it cannot name `TerrainStreamStatus` or `SceneLoadInFlight`
///    without a terrain/USD dependency, and the established pattern is that
///    `lunco-sandbox` mirrors such state onto the bus. A future visual subsystem
///    joins by publishing progress and being listed in [`VISUAL_BUSY_SOURCES`].
fn scene_visuals_ready(
    meshes: &Query<&bevy::mesh::Mesh3d>,
    asset_server: &AssetServer,
    bus: Option<&crate::status_bus::StatusBus>,
) -> Option<String> {
    // (1) Nothing spawned yet — not "ready", just "empty".
    let total = meshes.iter().len();
    if total == 0 {
        return Some("no mesh entities in the scene yet (prims still spawning)".into());
    }

    // (2) Mesh assets and their dependency closure.
    //
    // `get_recursive_dependency_load_state` returns `None` for a handle the
    // AssetServer never issued — one built at runtime and handed to
    // `Assets<Mesh>::add`. MEASURED: the first cut of this used
    // `is_loaded_with_dependencies`, which is `false` for such handles, and every
    // shot of episode_02 reported "27/27 mesh assets still loading" for the full
    // 20 s timeout — because USD prims build their meshes procedurally, so NONE of
    // them are server-tracked. An untracked handle is already resident in
    // `Assets<Mesh>` by the time the component exists, so it is ready by
    // construction; only server-issued handles can be mid-flight.
    let unloaded = meshes
        .iter()
        .filter(|m| {
            asset_server
                .get_recursive_dependency_load_state(m.0.id())
                .is_some_and(|s| !s.is_loaded())
        })
        .count();
    if unloaded > 0 {
        return Some(format!("{unloaded}/{total} mesh assets still loading"));
    }

    // (3) Visual subsystems that report their own progress.
    if let Some(bus) = bus {
        let mut busy: Vec<String> = bus
            .entries_in(crate::status_bus::BusyScope::Global)
            .filter(|e| VISUAL_BUSY_SOURCES.contains(&e.source))
            .map(|e| format!("{}: {}", e.source, e.message))
            .collect();
        if !busy.is_empty() {
            // Stable order: `active_progress` is a HashMap, and an unstable blocker
            // string would make the log line jitter between frames.
            busy.sort();
            return Some(format!("status bus busy — {}", busy.join("; ")));
        }
    }

    None
}

/// Start an armed recording once [`scene_visuals_ready`] clears it for
/// [`SETTLE_FRAMES`] consecutive frames — or once [`READY_TIMEOUT`] expires,
/// whichever comes first.
///
/// Timing out records anyway, loudly. See [`READY_TIMEOUT`] for why silently never
/// recording is the worse failure.
fn start_recording_when_scene_ready(
    pending: Option<ResMut<PendingShotStart>>,
    mut state: ResMut<OfflineRecordingState>,
    mut keep_awake: ResMut<lunco_core::KeepAwake>,
    mut windows: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
    meshes: Query<&bevy::mesh::Mesh3d>,
    asset_server: Res<AssetServer>,
    // `Option`: the bus belongs to the workbench UI, which a headless/API-only
    // binary does not add. Absent simply means clause (3) has nothing to say.
    bus: Option<Res<crate::status_bus::StatusBus>>,
    mut commands: Commands,
) {
    let Some(mut pending) = pending else { return };

    let blocker = scene_visuals_ready(&meshes, &asset_server, bus.as_deref());
    match &blocker {
        Some(reason) => {
            // Any refusal restarts the streak: the settle window must be
            // CONSECUTIVE, otherwise a texture that pops in late could land inside
            // the window and still be missing from frame 0.
            pending.ready_streak = 0;
            pending.ready_since = None;
            if pending.last_blocker.as_deref() != Some(reason.as_str()) {
                pending.last_blocker = Some(reason.clone());
                debug!("[offline-record] waiting for scene visuals — {reason}");
            }
        }
        None => {
            pending.ready_streak += 1;
            pending.ready_since.get_or_insert_with(web_time::Instant::now);
        }
    }

    // BOTH guards must pass — they cover different hazards (pipeline warm-up vs.
    // asynchronous spawn lulls). See [`SETTLE_FRAMES`] / [`SETTLE_PERIOD`].
    let settled = pending.ready_streak >= SETTLE_FRAMES
        && pending.ready_since.is_some_and(|t| t.elapsed() >= SETTLE_PERIOD);
    let timed_out = pending.requested_at.elapsed() >= READY_TIMEOUT;
    if !settled && !timed_out {
        return;
    }

    if timed_out && blocker.is_some() {
        warn!(
            "[offline-record] scene visuals were still not ready after {:.1}s — recording \
             anyway so the episode is not silently empty. Still waiting on: {}. Expect the \
             opening frames of this shot to show an unfinished scene.",
            pending.requested_at.elapsed().as_secs_f32(),
            blocker.as_deref().unwrap_or("unknown"),
        );
    }

    activate_recording(&pending, &mut state, &mut keep_awake, &mut windows, &mut commands);
    commands.remove_resource::<PendingShotStart>();
}

/// Sole owner of `TimeUpdateStrategy` while recording, and the only place that
/// requests a capture.
///
/// Runs in `Last` so it writes the strategy that Bevy's `TimeSystem` (in `First`)
/// will read at the top of the NEXT frame. That ordering is what makes the
/// lock-step deterministic: exactly one strategy write per frame, decided after
/// every other system — including the `deliver_offline_frame` observer — has run.
///
/// A second writer of `TimeUpdateStrategy` breaks this outright: whichever system
/// runs later in the frame wins, and re-freezing to ZERO after a step is scheduled
/// means virtual time never advances, `FixedUpdate` never runs, and a scenario
/// script sequencing the shots is starved — it can never reach its
/// `StopOfflineRecording`, so recording spools frames until the process is killed.
///
/// The cycle alternates two render frames per captured frame:
///   1. **advance** — clock steps `1/fps`, sim and `FixedUpdate` run, scene renders.
///   2. **capture** — request the screenshot, clock frozen until it lands.
///
/// Freezing while a capture is in flight is what keeps a slow (multi-frame)
/// readback from advancing time more than once per saved frame.
fn drive_offline_clock(
    mut state: ResMut<OfflineRecordingState>,
    // Armed-but-not-started is a clock phase like any other, so it is owned HERE.
    // Freezing from the readiness gate instead would make that gate a SECOND writer
    // of `TimeUpdateStrategy`, which is the exact failure this system's doc warns
    // about.
    pending: Option<Res<PendingShotStart>>,
    mut commands: Commands,
) {
    // PHASE 0 — armed, waiting for the scene. Freeze virtual time.
    //
    // MEASURED: without this, two runs of episode_02 differed at EVERY frame of
    // EVERY shot starting at frame 0 (viewport-crop RMSE 0.019-0.030, well clear of
    // the perf-HUD text burnt into the frame). The readiness wait is a REAL-TIME
    // window of variable length — 0.81 s vs 1.69 s for shot_01, 6.48 s vs 0.51 s for
    // shot_02 across two runs — and `Time<Virtual>` ran throughout it. The camera
    // path is a curve evaluated on the sim clock, and animated beats release the
    // physics hold, so a longer wait meant a differently-framed, differently-posed
    // frame 0 and therefore a different film.
    //
    // Freezing here restores the contract: the wait costs real time only, and the
    // captured sequence starts from the state the scene was in when the shot was
    // asked for, no matter how long the assets took.
    //
    // Safe against deadlock in both directions: the scenario script does not need to
    // tick during this window (it has already issued `shot_begin` and is polling
    // `shot_frame()`, which reports `-1` while armed), and [`READY_TIMEOUT`] is
    // measured on `web_time::Instant` — real time — so a scene that never becomes
    // ready still starts rather than freezing the app forever.
    if pending.is_some() && !state.active {
        commands.insert_resource(TimeUpdateStrategy::ManualDuration(
            std::time::Duration::ZERO,
        ));
        return;
    }

    if !state.active {
        return;
    }

    let frame_dur = std::time::Duration::from_secs_f64(1.0 / state.fps as f64);

    if state.is_waiting_for_frame {
        // Capture in flight — hold the clock so the pending frame stays the one
        // that was rendered when it was requested.
        commands.insert_resource(TimeUpdateStrategy::ManualDuration(
            std::time::Duration::ZERO,
        ));
    } else if state.frame_just_captured {
        // A frame landed: let the next frame advance by exactly one step.
        state.frame_just_captured = false;
        commands.insert_resource(TimeUpdateStrategy::ManualDuration(frame_dur));
    } else {
        // Time advanced this frame and the scene is rendered — capture it, then
        // hold the clock until the readback delivers.
        commands.spawn(Screenshot::primary_window());
        state.is_waiting_for_frame = true;
        commands.insert_resource(TimeUpdateStrategy::ManualDuration(
            std::time::Duration::ZERO,
        ));
    }
}

/// Observer for Bevy's ScreenshotCaptured event.
fn deliver_offline_frame(
    trigger: On<ScreenshotCaptured>,
    mut state: ResMut<OfflineRecordingState>,
) {
    if !state.active || !state.is_waiting_for_frame {
        return;
    }

    let event = trigger.event();
    let Ok(dyn_img) = event.image.clone().try_into_dynamic() else {
        error!("[offline-record] failed to convert image for frame {}", state.frame_index);
        state.is_waiting_for_frame = false;
        return;
    };

    let path = state.output_dir.join(format!("frame_{:06}.png", state.frame_index));

    // Save the image synchronously to disk. A failed write ABORTS the recording:
    // continuing would advance `frame_index` past a frame that never landed, leaving
    // a hole in the sequence. Nothing downstream notices — the scenario keeps
    // sequencing off `frame_index`, and the encoder happily renders the remaining
    // files as a continuous shot that silently jumps. A disk that fills mid-capture
    // is the ordinary way to hit this, so fail loudly at the first bad frame rather
    // than emit a corrupt take.
    if let Err(e) = dyn_img.save(&path) {
        error!(
            "[offline-record] failed to save frame {} ({e}) — aborting recording to \
             avoid a sequence with holes in it",
            state.frame_index
        );
        state.active = false;
        state.is_waiting_for_frame = false;
        state.frame_just_captured = false;
        return;
    }
    trace!("[offline-record] saved frame {}", state.frame_index);

    state.frame_index += 1;
    state.is_waiting_for_frame = false;
    // `drive_offline_clock` owns `TimeUpdateStrategy`; just signal that a frame
    // landed so it can schedule the single `1/fps` step.
    state.frame_just_captured = true;
}

struct GetOfflineRecordingStatusProvider;
impl lunco_api::queries::ApiQueryProvider for GetOfflineRecordingStatusProvider {
    fn name(&self) -> &'static str {
        "GetOfflineRecordingStatus"
    }
    fn execute(&self, world: &mut World, _params: &serde_json::Value) -> lunco_api::schema::ApiResponse {
        let state = world.resource::<OfflineRecordingState>();
        lunco_api::schema::ApiResponse::ok(serde_json::json!({
            "active": state.active,
            "frame_index": state.frame_index,
            "is_waiting_for_frame": state.is_waiting_for_frame,
        }))
    }
}

