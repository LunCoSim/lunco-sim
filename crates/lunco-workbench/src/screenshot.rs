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
            .init_resource::<PendingScreenshotResponses>()
            // Answers deferred HTTP screenshots once their worker-side encode
            // lands — the encode itself never runs on the frame loop.
            .add_systems(Update, pump_screenshot_responses)
            .add_observer(deliver_screenshot);

        // Offline Frame-by-Frame Recording Mode
        app.init_resource::<lunco_core::KeepAwake>()
            .init_resource::<OfflineRecordingState>()
            .init_resource::<OfflineSaveQueue>()
            .init_resource::<OfflineVideoSink>()
            .add_observer(deliver_offline_frame)
            // Collects finished async frame saves. `Update`, unconditionally:
            // the queue must drain even after the recording deactivates.
            // The limit/exit systems are inert without their opt-in resources
            // (`OfflineRecordLimit` / `ExitAfterRecording`).
            .add_systems(
                Update,
                (pump_offline_saves, stop_recording_at_limit, exit_when_recording_drained),
            )
            // The readiness gate. `Update` (not `Last`): it must run before
            // `drive_offline_clock`, which only acts once `state.active` is set —
            // so the shot begins on the same frame it was cleared to begin.
            // The pacing enforcer is chained after it so a shot activated this
            // frame gets its knobs applied this frame, not next.
            .add_systems(
                Update,
                (start_recording_when_scene_ready, enforce_recording_pacing).chain(),
            )
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

/// What an in-flight capture should do when it lands. A component ON the `Screenshot`
/// entity it belongs to, so the correlation travels with the entity — concurrent captures
/// (a second HTTP request, a `take_photo`, an offline-recording frame) each deliver on
/// their own frame instead of consuming each other's.
#[derive(Component, Debug, Clone)]
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
    capture_target: Option<Res<OfflineCaptureTarget>>,
    mut commands: Commands,
) {
    let cmd = trigger.event();

    // A 4-element `[x, y, w, h]`, or nothing. A malformed region is ignored rather than
    // rejected — cropping is a convenience, and a bad crop should not cost you the frame.
    let region = match cmd.region.as_slice() {
        [x, y, w, h] => Some((*x, *y, *w, *h)),
        _ => None,
    };

    let request = if cmd.save_to_file {
        // Empty ⇒ we pick a timestamped name. Reaching for a wall clock is not something the
        // render-free substrate should do, so that default lives here.
        let path =
            if cmd.path.is_empty() { timestamped_name("screenshot") } else { cmd.path.clone() };

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
    //
    // Offscreen (`--offscreen`) has no window — "the screen" is the offscreen
    // render target, so API screenshots read that image.
    let shot = match capture_target.as_ref() {
        Some(target) => Screenshot::image(target.0.clone()),
        None => Screenshot::primary_window(),
    };
    commands.spawn((shot, request));
}

register_commands!(
    on_capture_screenshot,
    on_capture_from_camera,
    on_start_offline_recording,
    on_stop_offline_recording
);

/// The picture landed — hand it to a worker, which crops, encodes, and either
/// saves it or produces the bytes for the deferred HTTP answer. The main thread
/// does nothing per-pixel here: the image is STOLEN from the event (`mem::take`,
/// O(1)) — safe because the other observer, `deliver_offline_frame`, bails on
/// entities that carry a `PendingCapture` marker (this flow's), and this one
/// bails on everything else.
fn deliver_screenshot(
    mut trigger: On<ScreenshotCaptured>,
    requests: Query<&PendingCapture>,
    mut responses: ResMut<PendingScreenshotResponses>,
) {
    let Ok(pending) = requests.get(trigger.event().entity) else {
        return;
    };
    let correlation_id = pending.correlation_id;
    let save_path = pending.save_path.clone();
    let region = pending.region;
    let image = std::mem::take(&mut trigger.event_mut().image);

    let pool = bevy::tasks::AsyncComputeTaskPool::get();
    if let Some(path) = save_path {
        // save_to_file mode — the response was already sent; the write is
        // fire-and-forget (detach), errors surface in the log from the worker.
        pool.spawn(async move {
            if let Err(e) = encode_and_store(image, region, std::path::Path::new(&path)) {
                error!("[screenshot] failed to save to '{path}': {e}");
            }
        })
        .detach();
    } else if let Some(cid) = correlation_id {
        // raw-PNG mode — encode on the worker; the deferred HTTP request is
        // answered by `pump_screenshot_responses` when the bytes land.
        let task = pool
            .spawn(async move { encode_capture(image, region, image::ImageFormat::Png) });
        responses.0.push((cid, task));
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
    // The delivery, armed on whichever `Screenshot` entity is spawned below. Without it the
    // frame lands in `deliver_screenshot` with nothing pending and is silently dropped —
    // the instrument believes it photographed and recorded nothing.
    let request = PendingCapture {
        correlation_id: None,
        save_path: Some(timestamped_name("photo")),
        region: None,
    };
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
        commands.spawn((Screenshot::primary_window(), request));
        return;
    };

    // Bevy's `Screenshot` captures a render TARGET (window/image), not a camera directly.
    let Ok((cam, _, rt)) = cameras.get(camera_entity) else {
        commands.spawn((Screenshot::primary_window(), request));
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
    commands.spawn((screenshot, request));
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
fn timestamped_name(prefix: &str) -> String {
    let secs = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{prefix}_{secs}.png")
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
    /// Destination: a directory for a PNG sequence, or a video file when
    /// [`Self::video`] is set.
    pub output_dir: std::path::PathBuf,
    /// Encode straight to a video file via a spawned `ffmpeg` instead of a PNG
    /// sequence. Chosen by the destination's extension (see
    /// [`output_is_video`]); demoted back to a PNG sequence at activation if
    /// `ffmpeg` is not installed — a missing encoder must degrade, not crash.
    pub video: bool,
    /// Video target FPS (determines delta virtual time step per frame).
    pub fps: u32,
    /// Kept for status-payload compatibility; the pipelined capture cycle no
    /// longer waits on individual frames, so this stays `false`.
    pub is_waiting_for_frame: bool,
    /// The frame currently rendering carries a freshly-advanced sim state that
    /// has not been captured yet. Written by `drive_offline_clock` when it
    /// schedules a `1/fps` step; consumed by the same system one frame later to
    /// request exactly one capture of that state. `false` on the activation
    /// frame (whatever it rendered predates deterministic stepping) and after a
    /// back-pressure hold (a frozen frame re-renders an already-captured state).
    pub advanced_this_frame: bool,
    /// Capture requests whose GPU readback has not delivered yet. Bounded by
    /// [`MAX_OUTSTANDING_CAPTURES`] via back-pressure in `drive_offline_clock`.
    pub outstanding_captures: u32,
    /// Primary window present mode as it was before recording uncapped it,
    /// restored on stop. Doubles as the "uncap already applied" latch for
    /// [`enforce_recording_pacing`]: `None` while active means the override is
    /// still owed (e.g. CLI arming happened before a window existed).
    pub prev_present_mode: Option<bevy::window::PresentMode>,
}

impl OfflineRecordingState {
    /// The ONE way to construct an entering-recording state, called from
    /// [`activate_recording`] once the readiness gate clears. Every entry point
    /// funnels through that gate — the CLI `--record-offline` path arms via
    /// [`arm_recording_at_startup`] rather than building this struct itself,
    /// because when it hand-maintained its own field list it drifted (skipped
    /// the pacing setup → recorded through the power-save throttle at
    /// 2–10 s/frame) and, starting `active` at app construction, captured its
    /// opening frames before the scene had loaded (black frame 0).
    pub fn start(output_dir: std::path::PathBuf, fps: u32) -> Self {
        Self {
            active: true,
            frame_index: 0,
            video: output_is_video(&output_dir),
            output_dir,
            fps: fps.max(1),
            is_waiting_for_frame: false,
            // The activation frame rendered whatever real-time state preceded
            // the lock-step, so it must NOT be captured: the first capture
            // happens after the first scheduled `1/fps` step, so frame 0 of the
            // sequence is step 1 of the deterministic clock — same contract as
            // every frame after it.
            advanced_this_frame: false,
            outstanding_captures: 0,
            // `enforce_recording_pacing` fills this in when it applies the uncap.
            prev_present_mode: None,
        }
    }
}

/// Marker on the recorder's own `Screenshot` entities. Positive identification:
/// an HTTP `CaptureScreenshot` or a behaviour-tree `take_photo` taken
/// mid-recording spawns a `Screenshot` too, but without this component, so
/// `deliver_offline_frame` ignores it (and `deliver_screenshot` handles it via
/// `PendingCapture`, the mirror-image marker of the other flow).
///
/// Carrying the slot AND the destination in the component is what makes the
/// pipeline safe: readbacks may deliver out of order and may deliver after the
/// recording stopped (drain) or after the NEXT shot started — the frame still
/// lands at its own index in its own shot's directory, because nothing about it
/// is read from mutable recorder state at delivery time.
#[derive(Component)]
struct OfflineFrameCapture {
    /// Sequence slot this capture fills (`frame_{index:06}.png`).
    index: u64,
    /// Full destination path, resolved at request time.
    path: std::path::PathBuf,
    /// Request instant — readback latency probe for the debug log.
    requested_at: web_time::Instant,
}

/// Ceiling on capture requests whose GPU readback has not yet delivered.
///
/// Each outstanding capture pins a window-sized readback (~16 MB at 2560×1552),
/// so this bounds memory exactly the way [`MAX_IN_FLIGHT_SAVES`] does on the
/// save side. It is sized ABOVE the readback latency observed in practice
/// (~3–6 render frames) so the pipeline is normally governed by the save queue
/// and the render rate, not by this cap.
const MAX_OUTSTANDING_CAPTURES: u32 = 6;

/// Does this recording destination mean "encode a video file" rather than
/// "write a PNG sequence into this directory"? Container choice is the caller's;
/// these are the ones ffmpeg infers an H.264-capable muxer for by extension.
pub fn output_is_video(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
        Some("mp4" | "mkv" | "mov")
    )
}

/// Is a runnable `ffmpeg` on PATH? Probed once at activation so a machine
/// without it demotes the recording to a PNG sequence with a loud `warn!`
/// instead of failing on the first frame.
#[cfg(not(target_arch = "wasm32"))]
fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
/// No child processes on wasm — video mode always demotes there.
#[cfg(target_arch = "wasm32")]
fn ffmpeg_available() -> bool {
    false
}

/// When present, offline recording captures THIS image each frame instead of
/// the primary window — the offscreen (`--offscreen`) mode's render target.
/// Inserted by the binary that owns the mode (`SandboxOffscreenPlugin`); the
/// recorder itself stays target-agnostic.
#[derive(Resource)]
pub struct OfflineCaptureTarget(pub Handle<bevy::image::Image>);

/// Stop the recording automatically once `frame_index` reaches this count —
/// the CLI `--record-frames <n>` one-shot contract. Routed through the SAME
/// `StopOfflineRecording` command a scenario would send, so the drain,
/// finalization and status behaviour cannot diverge between the two.
#[derive(Resource)]
pub struct OfflineRecordLimit(pub u64);

/// Marker: exit the app once a recording has fully drained (frames delivered,
/// saves finished, video finalized). Inserted by one-shot modes (`--offscreen`);
/// a windowed session never wants it.
#[derive(Resource)]
pub struct ExitAfterRecording;

/// The live `ffmpeg` a video-mode recording is streaming into. `sink` is `None`
/// until the first frame delivers (the encoder needs the real capture
/// dimensions, which are only known then); `draining` holds the writer thread's
/// completion flag after finalization hand-off, so [`exit_when_recording_drained`]
/// can wait for the container trailer to actually hit disk.
#[derive(Resource, Default)]
pub struct OfflineVideoSink {
    #[cfg(not(target_arch = "wasm32"))]
    sink: Option<VideoSink>,
    #[cfg(not(target_arch = "wasm32"))]
    draining: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

/// How many raw frames may sit in the channel to the writer thread. Bounded so
/// an encoder slower than the render loop blocks the save workers, which fills
/// [`OfflineSaveQueue`], which holds the clock — the same back-pressure chain
/// as the PNG path, ending in a paused capture instead of unbounded memory
/// (raw frames are ~16 MB each at 2560×1552; 8 ≈ 128 MB ceiling).
#[cfg(not(target_arch = "wasm32"))]
const VIDEO_CHANNEL_DEPTH: usize = 8;

/// A spawned `ffmpeg` encoding `rawvideo` from stdin, fed by a dedicated writer
/// thread. The writer owns the ONLY ordering concern in the pipeline: PNG
/// frames are order-independent files, but a video stream is strictly
/// sequential, so deliveries (which may arrive out of order) are staged in a
/// reorder buffer and written contiguously from frame 0.
#[cfg(not(target_arch = "wasm32"))]
struct VideoSink {
    /// Save workers send `(frame_index, rgba8_bytes)` here.
    tx: std::sync::mpsc::SyncSender<(u64, Vec<u8>)>,
    /// First failure reported by the writer thread (pipe broke, ffmpeg died,
    /// missing frame at finalization). Read by [`pump_offline_saves`].
    error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// Set by the writer thread after `ffmpeg` exits — the file is complete.
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Dimensions the encoder was started with — a window resize mid-recording
    /// would corrupt the raw stream, so a mismatching frame aborts instead.
    width: u32,
    height: u32,
}

#[cfg(not(target_arch = "wasm32"))]
impl VideoSink {
    /// Spawn `ffmpeg` writing to `path` and the writer thread that feeds it.
    fn spawn(path: &std::path::Path, width: u32, height: u32, fps: u32) -> std::io::Result<Self> {
        let mut child = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "rawvideo", "-pixel_format", "rgba"])
            .arg("-video_size")
            .arg(format!("{width}x{height}"))
            .arg("-framerate")
            .arg(fps.to_string())
            .args(["-i", "-"])
            // veryfast: at cinematic resolutions the default preset encodes
            // slower than the capture produces frames, and the back-pressure
            // chain would pace the whole recording down to the encoder.
            .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "18"])
            // yuv420p for player compatibility; the crop keeps odd window
            // dimensions legal for it (4:2:0 needs even width/height).
            .args(["-vf", "crop=trunc(iw/2)*2:trunc(ih/2)*2"])
            .args(["-pix_fmt", "yuv420p"])
            .arg(path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;

        let (tx, rx) = std::sync::mpsc::sync_channel::<(u64, Vec<u8>)>(VIDEO_CHANNEL_DEPTH);
        let error: std::sync::Arc<std::sync::Mutex<Option<String>>> = Default::default();
        let done: std::sync::Arc<std::sync::atomic::AtomicBool> = Default::default();
        let err_slot = error.clone();
        let done_flag = done.clone();
        std::thread::Builder::new()
            .name("offline-video-writer".into())
            .spawn(move || {
                use std::io::Write;
                let mut stdin = child.stdin.take().expect("stdin was piped above");
                let fail = |m: String| {
                    *err_slot.lock().unwrap() = Some(m);
                };
                let mut reorder: std::collections::BTreeMap<u64, Vec<u8>> = Default::default();
                let mut next: u64 = 0;
                // Runs until every sender is dropped (finalization) or the first
                // write failure — exiting drops `rx`, which errors any blocked
                // sender, which aborts the recording via the task results.
                'recv: for (index, bytes) in rx {
                    reorder.insert(index, bytes);
                    while let Some(bytes) = reorder.remove(&next) {
                        if let Err(e) = stdin.write_all(&bytes) {
                            fail(format!("ffmpeg pipe write failed at frame {next}: {e}"));
                            break 'recv;
                        }
                        next += 1;
                    }
                }
                if !reorder.is_empty() {
                    fail(format!(
                        "video is missing frame {next}; {} delivered frame(s) after the gap \
                         were dropped",
                        reorder.len()
                    ));
                }
                // EOF tells ffmpeg to finish the file; wait for the trailer.
                drop(stdin);
                match child.wait() {
                    Ok(status) if status.success() => {
                        info!("[offline-record] video finalized: {next} frames encoded");
                    }
                    Ok(status) => fail(format!("ffmpeg exited with {status}")),
                    Err(e) => fail(format!("waiting for ffmpeg failed: {e}")),
                }
                done_flag.store(true, std::sync::atomic::Ordering::Release);
            })?;

        Ok(Self { tx, error, done, width, height })
    }
}

/// Frame saves still being encoded/written by the async task pool.
///
/// A separate resource rather than a field on [`OfflineRecordingState`] because
/// `Task` is neither `Clone` nor meaningfully `Default`-able, and because the
/// queue must keep draining after the state deactivates (stop or abort): a
/// spawned save owns its pixels and its path and always runs to completion, so
/// stopping a recording never truncates the tail of the sequence.
///
/// This queue is what takes the save OFF the per-frame critical path. The
/// synchronous design paid `convert + encode + write` (measured ~70 ms at
/// 2560×1552) inside the clock-freeze window, in series with the render; with
/// the queue, the freeze clears the moment the captured image is handed to a
/// worker, and the next frame renders while the previous one deflates.
/// Determinism is untouched — the capture *head* (one `1/fps` step per captured
/// frame, in order) is exactly as before; only the save *tail* is concurrent.
#[derive(Resource, Default)]
pub struct OfflineSaveQueue {
    /// Each task resolves to the frame index it saved, or an error string.
    tasks: Vec<bevy::tasks::Task<Result<u64, String>>>,
}

/// Ceiling on concurrently in-flight frame saves.
///
/// The bound is what makes image-buffered saving safe: the unbounded version of
/// this idea is exactly the historical OOM (`docs/offline-recording.md`) — raw
/// frames are ~16 MB at 2560×1552 and the GPU can capture faster than a disk
/// can deflate+write, so an unbounded queue grows without limit on any long
/// shot. When the queue is full, `drive_offline_clock` simply holds the clock
/// and waits (the pre-queue behaviour, applied only under pressure), turning
/// overload into back-pressure instead of memory growth. 6 slots ≈ 96 MB peak,
/// and at the measured ~70 ms per save it caps sustained capture throughput at
/// ~85 fps — above what the render loop delivers, so in practice it only bites
/// when the disk stalls.
const MAX_IN_FLIGHT_SAVES: usize = 6;

/// Convert a captured [`Image`] to encoded bytes — the CPU-heavy tail shared by
/// EVERY capture consumer (recorder frames, `save_to_file` screenshots, raw-PNG
/// HTTP answers). Runs on task-pool workers, never on the frame loop.
///
/// `region` crops first (clamped; an out-of-bounds region keeps the full frame,
/// loudly, matching the historical screenshot behaviour). `format` follows the
/// destination — [`image::ImageFormat::from_path`] for files, PNG for wire.
fn encode_capture(
    image: bevy::image::Image,
    region: Option<(u32, u32, u32, u32)>,
    format: image::ImageFormat,
) -> Result<Vec<u8>, String> {
    let mut dyn_img = image
        .try_into_dynamic()
        .map_err(|e| format!("convert failed: {e}"))?;
    if let Some((x, y, w, h)) = region {
        let (iw, ih) = (dyn_img.width(), dyn_img.height());
        if x < iw && y < ih && w > 0 && h > 0 {
            dyn_img = dyn_img.crop_imm(x, y, w.min(iw - x), h.min(ih - y));
        } else {
            error!(
                "[screenshot] region {region:?} lies outside the {iw}x{ih} image — \
                 keeping the full frame"
            );
        }
    }
    let mut bytes = Vec::new();
    dyn_img
        .write_to(&mut Cursor::new(&mut bytes), format)
        .map_err(|e| format!("encode failed: {e}"))?;
    Ok(bytes)
}

/// [`encode_capture`] + write THROUGH `lunco_storage` — the storage API is the
/// write path for every file this codebase produces (and the only one that
/// exists on wasm/OPFS); the `DynamicImage::save` calls this replaces were
/// holes in that rule. Format follows the path's extension, PNG when unnamed.
fn encode_and_store(
    image: bevy::image::Image,
    region: Option<(u32, u32, u32, u32)>,
    path: &std::path::Path,
) -> Result<(), String> {
    let format = image::ImageFormat::from_path(path).unwrap_or(image::ImageFormat::Png);
    let bytes = encode_capture(image, region, format)?;
    lunco_storage::write_file_sync(path, &bytes).map_err(|e| format!("write failed: {e}"))
}

/// Raw-PNG HTTP screenshots whose encode is still on a worker. Polled by
/// [`pump_screenshot_responses`], which triggers the deferred `ApiResponseEvent`
/// on the main world once the bytes are ready — the one step a worker cannot do.
#[derive(Resource, Default)]
struct PendingScreenshotResponses(Vec<(u64, bevy::tasks::Task<Result<Vec<u8>, String>>)>);

/// Answer deferred HTTP screenshot requests whose worker-side encode finished.
/// Failure logs and drops the request (the API layer's watchdog times the
/// held response out), matching the synchronous behaviour this replaced.
fn pump_screenshot_responses(
    mut pending: ResMut<PendingScreenshotResponses>,
    mut commands: Commands,
) {
    if pending.0.is_empty() {
        return;
    }
    use bevy::tasks::futures_lite::future;
    pending.0.retain_mut(|(cid, task)| {
        match future::block_on(future::poll_once(&mut *task)) {
            None => true,
            Some(Ok(png_bytes)) => {
                commands.trigger(ApiResponseEvent {
                    response: ApiResponse::Screenshot { png_bytes },
                    correlation_id: *cid,
                });
                false
            }
            Some(Err(e)) => {
                error!("[screenshot] failed to produce the HTTP screenshot ({e})");
                false
            }
        }
    });
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
    // A video destination (`out.mp4`) needs its PARENT to exist — creating the
    // path itself would plant a directory where ffmpeg wants a file.
    let dir_to_create = if output_is_video(&dir) {
        dir.parent().map(std::path::Path::to_path_buf).unwrap_or_default()
    } else {
        dir.clone()
    };
    if !dir_to_create.as_os_str().is_empty() {
        if let Err(e) = std::fs::create_dir_all(&dir_to_create) {
            error!(
                "[offline-record] failed to create output directory {}: {e}",
                dir_to_create.display()
            );
            return;
        }
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
/// ready path and the timeout path cannot drift apart. The pacing knobs
/// (`KeepAwake`, present mode) are NOT touched here — [`enforce_recording_pacing`]
/// applies them from the `active` flag, so every entry point gets them.
fn activate_recording(
    pending: &PendingShotStart,
    state: &mut OfflineRecordingState,
    commands: &mut Commands,
) {
    *state = OfflineRecordingState::start(pending.output_dir.clone(), pending.fps);

    // Video mode needs a runnable `ffmpeg`. Probe NOW, not at the first frame:
    // a missing encoder must demote the recording to a PNG sequence with a loud
    // warning — never crash the shot, and never fail after frames were taken.
    if state.video && !ffmpeg_available() {
        let fallback = state.output_dir.with_extension("frames");
        warn!(
            "[offline-record] {} requested a video but ffmpeg is not installed — \
             falling back to a PNG sequence in {} (install ffmpeg for direct video \
             recording)",
            state.output_dir.display(),
            fallback.display(),
        );
        if let Err(e) = std::fs::create_dir_all(&fallback) {
            error!(
                "[offline-record] fallback directory {} could not be created ({e}) — \
                 aborting the shot",
                fallback.display()
            );
            state.active = false;
            return;
        }
        state.output_dir = fallback;
        state.video = false;
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

/// Wind an active recording down and hand the clock back to `Automatic`. EVERY
/// path that ends a recording — the stop command and the failed-write abort alike
/// — must run this; skipping it leaves the last-written `ManualDuration(ZERO)` in
/// place with nothing to replace it, freezing virtual time until the process
/// restarts. The wake token and present mode are restored by
/// [`enforce_recording_pacing`] when it observes `active` drop.
fn teardown_recording(state: &mut OfflineRecordingState, commands: &mut Commands) {
    state.active = false;
    state.is_waiting_for_frame = false;
    state.advanced_this_frame = false;

    // Restore automatic realtime ticking
    commands.insert_resource(TimeUpdateStrategy::Automatic);
}

#[on_command(StopOfflineRecording)]
fn on_stop_offline_recording(
    _trigger: On<StopOfflineRecording>,
    mut state: ResMut<OfflineRecordingState>,
    mut commands: Commands,
) {
    // Disarm unconditionally: a scenario that gives up on a shot while the gate is
    // still waiting must not leave a `PendingShotStart` behind to fire into the
    // *next* shot's directory.
    commands.remove_resource::<PendingShotStart>();

    if state.active {
        teardown_recording(&mut state, &mut commands);
        info!("[offline-record] stopped recording");
    }
}

/// Sole applier of the two recording pacing knobs — the [`lunco_core::KeepAwake`]
/// token and the present-mode uncap — driven off `state.active` instead of being
/// called from the activation/teardown paths.
///
/// Reacting to the flag is what covers BOTH entry points with one piece of code.
/// The `StartOfflineRecording` command path and the CLI `--record-offline` direct
/// insert each used to (not) do this setup by hand, and they drifted: the CLI
/// path skipped both knobs, so an unfocused CLI capture recorded through the
/// `reactive_low_power` throttle — measured at 2–10 s per frame against ~50 ms
/// awake, turning a ~1 minute episode into hours — and under `Fifo` (vsync) the
/// render loop stayed pinned to the display refresh, a hard ~30 captured-FPS
/// ceiling on a 60 Hz panel. Any future entry point inherits the knobs for free.
///
/// * **KeepAwake** states intent and stops there; the pacer (`sim_focus_pace` in
///   `lunco-modelica`, the sole `WinitSettings` writer) applies it. Acquired on
///   the rising edge of `active`, released on the falling edge, so the token
///   stays balanced no matter which path started or ended the recording.
/// * **Present mode** wants frames as fast as the machine can render them:
///   virtual time still advances exactly `1/fps` per captured frame, so
///   rendering faster changes only how long the capture takes, never what the
///   video looks like. Applied whenever `active` holds and no previous mode is
///   stashed — a retry loop, because CLI arming happens at app construction,
///   before the primary window exists. Restored from the stash once `active`
///   drops.
fn enforce_recording_pacing(
    mut state: ResMut<OfflineRecordingState>,
    mut keep_awake: ResMut<lunco_core::KeepAwake>,
    mut windows: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
    mut was_active: Local<bool>,
) {
    if state.active != *was_active {
        *was_active = state.active;
        if state.active {
            keep_awake.acquire();
            info!("[offline-record] power saving disabled (KeepAwake acquired)");
        } else {
            // Drop the wake request; the pacer restores the binary's idle policy.
            keep_awake.release();
        }
    }

    if state.active {
        if state.prev_present_mode.is_none() {
            if let Ok(mut window) = windows.single_mut() {
                // Only vsynced modes are overridden. A window already presenting
                // uncapped (`--no-vsync` ⇒ Mailbox, or networked builds) must be
                // left alone: replacing a known-good Mailbox with AutoNoVsync
                // lets the driver re-negotiate, and on compositors where the
                // "NoVsync" chain falls back to Fifo that SLOWS the capture.
                use bevy::window::PresentMode;
                match window.present_mode {
                    PresentMode::Mailbox | PresentMode::Immediate | PresentMode::AutoNoVsync => {}
                    prev => {
                        state.prev_present_mode = Some(prev);
                        window.present_mode = PresentMode::AutoNoVsync;
                        info!(
                            "[offline-record] present mode uncapped to AutoNoVsync (was {prev:?})"
                        );
                    }
                }
            }
        }
    } else if state.prev_present_mode.is_some() {
        if let Ok(mut window) = windows.single_mut() {
            window.present_mode = state.prev_present_mode.take().unwrap();
        }
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

/// Arm a recording at app construction — the CLI `--record-offline` path.
///
/// Arms, never starts: this inserts the same [`PendingShotStart`] the
/// `StartOfflineRecording` command does, so the CLI shot waits on the SAME
/// readiness gate and gets the same pacing setup. The CLI used to insert an
/// `active` [`OfflineRecordingState`] directly, which meant capture began at
/// frame 0 of the process — its opening frames were permanently black (nothing
/// loaded yet), the exact defect [`scene_visuals_ready`] exists to prevent.
/// [`READY_TIMEOUT`] counting from construction is correct here: it caps the
/// whole boot-plus-load wait, and a scene that never becomes ready still
/// records (loudly) rather than never starting.
pub fn arm_recording_at_startup(
    app: &mut bevy::app::App,
    output_dir: std::path::PathBuf,
    fps: u32,
) {
    app.insert_resource(PendingShotStart {
        output_dir,
        fps: fps.max(1),
        requested_at: web_time::Instant::now(),
        ready_streak: 0,
        ready_since: None,
        last_blocker: None,
    });
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

    activate_recording(&pending, &mut state, &mut commands);
    commands.remove_resource::<PendingShotStart>();
}

/// Sole owner of `TimeUpdateStrategy` while recording, and the only place that
/// requests a capture.
///
/// Runs in `Last` so it writes the strategy that Bevy's `TimeSystem` (in `First`)
/// will read at the top of the NEXT frame. That ordering is what makes the
/// lock-step deterministic: exactly one strategy write per frame, decided after
/// every other system has run.
///
/// A second writer of `TimeUpdateStrategy` breaks this outright: whichever system
/// runs later in the frame wins, and re-freezing to ZERO after a step is scheduled
/// means virtual time never advances, `FixedUpdate` never runs, and a scenario
/// script sequencing the shots is starved — it can never reach its
/// `StopOfflineRecording`, so recording spools frames until the process is killed.
///
/// The cycle is PIPELINED — one captured frame per render frame at steady state:
/// every `Last`, the state the frame just rendered (if it carried a fresh `1/fps`
/// step) gets a capture request, and the next step is scheduled immediately —
/// without waiting for the readback. Waiting was the old design, and it was the
/// recording's dominant cost: a GPU readback is ~50–60 ms of latency during which
/// the pixels CANNOT change (the snapshot happens at request time), so freezing
/// the clock until it delivered bought nothing and capped capture at ~9 fps.
///
/// Determinism is the same contract as the serial design, enforced by ordering
/// within this one system: a scheduled step is ALWAYS captured before the next
/// step is scheduled (capture-before-advance below), every capture carries its
/// slot index and destination in its own [`OfflineFrameCapture`], and in-flight
/// readbacks/saves are bounded by [`MAX_OUTSTANDING_CAPTURES`] /
/// [`MAX_IN_FLIGHT_SAVES`] back-pressure — under pressure the clock holds, which
/// re-renders an already-captured state and captures nothing, never skips.
fn drive_offline_clock(
    mut state: ResMut<OfflineRecordingState>,
    // Armed-but-not-started is a clock phase like any other, so it is owned HERE.
    // Freezing from the readiness gate instead would make that gate a SECOND writer
    // of `TimeUpdateStrategy`, which is the exact failure this system's doc warns
    // about.
    pending: Option<Res<PendingShotStart>>,
    save_queue: Res<OfflineSaveQueue>,
    capture_target: Option<Res<OfflineCaptureTarget>>,
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

    // CAPTURE FIRST, unconditionally. If this frame rendered a fresh step, that
    // step already happened — skipping its capture under back-pressure would
    // leave a 2/fps jump between adjacent files, the silent-hole class of bug
    // the failure policy aborts recordings over. Pressure may only pause
    // *advancing*, never drop a step that was taken.
    if state.advanced_this_frame {
        state.advanced_this_frame = false;
        // Offscreen mode captures the render-target image; windowed mode the
        // primary window. Same pipeline either side of this one expression.
        let shot = match capture_target.as_ref() {
            Some(target) => Screenshot::image(target.0.clone()),
            None => Screenshot::primary_window(),
        };
        commands.spawn((
            shot,
            OfflineFrameCapture {
                index: state.frame_index,
                path: state
                    .output_dir
                    .join(format!("frame_{:06}.png", state.frame_index)),
                requested_at: web_time::Instant::now(),
            },
        ));
        state.frame_index += 1;
        state.outstanding_captures += 1;
    }

    if state.outstanding_captures >= MAX_OUTSTANDING_CAPTURES
        || save_queue.tasks.len() >= MAX_IN_FLIGHT_SAVES
    {
        // Readback or save side is saturated — hold the clock until a slot
        // frees. The held frame re-renders an already-captured state, so the
        // capture branch above stays idle; nothing is skipped, nothing grows.
        commands.insert_resource(TimeUpdateStrategy::ManualDuration(
            std::time::Duration::ZERO,
        ));
    } else {
        // Schedule the next deterministic step; next `Last` captures it.
        state.advanced_this_frame = true;
        commands.insert_resource(TimeUpdateStrategy::ManualDuration(frame_dur));
    }
}

/// Observer for Bevy's ScreenshotCaptured event.
fn deliver_offline_frame(
    mut trigger: On<ScreenshotCaptured>,
    captures: Query<&OfflineFrameCapture>,
    mut state: ResMut<OfflineRecordingState>,
    mut queue: ResMut<OfflineSaveQueue>,
    mut video_sink: ResMut<OfflineVideoSink>,
    mut commands: Commands,
) {
    // Positive identification via the marker — see [`OfflineFrameCapture`]. Not
    // gated on `state.active`: a delivery arriving after stop belongs to a step
    // taken while active, and the take's tail must land (drain), even if the
    // next shot is already recording into a different directory.
    let Ok(capture) = captures.get(trigger.event().entity) else {
        return;
    };
    let frame_idx = capture.index;
    let path = capture.path.clone();
    let readback = capture.requested_at.elapsed().as_secs_f32() * 1000.0;
    state.outstanding_captures = state.outstanding_captures.saturating_sub(1);

    // STEAL the image, don't clone it: `mem::take` moves the ~16 MB buffer out
    // of the event in O(1), replacing it with `Image::default()`. Cloning here
    // was the last per-frame main-thread cost worth naming (~10-20 ms at
    // 2560×1552). Safe because this event's only other observer,
    // `deliver_screenshot`, bails on the missing `PendingCapture` before it
    // touches the image — and our entities never carry that component.
    let image = std::mem::take(&mut trigger.event_mut().image);

    // Hand the frame to a worker immediately: convert+encode+write (~70 ms
    // measured at 2560×1552) never touches the render loop's critical path.
    //
    // Failure policy is drain-and-abort, surfaced in [`pump_offline_saves`] up
    // to [`MAX_IN_FLIGHT_SAVES`] frames late: the recording aborts loudly and
    // the log names the lost frame, but frames already handed to workers still
    // land. The sequence can therefore end a few frames after the failed index —
    // never with a silent mid-sequence hole, which is what the abort exists to
    // prevent (a disk filling mid-capture is the ordinary trigger, and the video
    // encoder would happily splice across a gap).
    let pool = bevy::tasks::AsyncComputeTaskPool::get();
    if state.video {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // The encoder starts on the FIRST delivered frame — only then are
            // the real capture dimensions known. `ffmpeg_available` was probed
            // at activation, so a spawn failure here is an abort, not a demote.
            if video_sink.sink.is_none() {
                match VideoSink::spawn(
                    &state.output_dir,
                    image.width(),
                    image.height(),
                    state.fps,
                ) {
                    Ok(sink) => {
                        info!(
                            "[offline-record] streaming {}x{} @ {} FPS into {} via ffmpeg",
                            image.width(),
                            image.height(),
                            state.fps,
                            state.output_dir.display(),
                        );
                        video_sink.sink = Some(sink);
                    }
                    Err(e) => {
                        error!(
                            "[offline-record] failed to start ffmpeg ({e}) — aborting recording"
                        );
                        teardown_recording(&mut state, &mut commands);
                        return;
                    }
                }
            }
            let sink = video_sink.sink.as_ref().expect("filled above");
            if (sink.width, sink.height) != (image.width(), image.height()) {
                error!(
                    "[offline-record] window resized mid-recording ({}x{} → {}x{}) — a raw \
                     video stream cannot change size, aborting",
                    sink.width,
                    sink.height,
                    image.width(),
                    image.height(),
                );
                teardown_recording(&mut state, &mut commands);
                return;
            }
            // The worker converts to tightly-packed RGBA and sends into the
            // writer thread's bounded channel; a full channel blocks the worker,
            // which fills the queue, which holds the clock (back-pressure).
            let tx = sink.tx.clone();
            let task = pool.spawn(async move {
                let rgba = image
                    .try_into_dynamic()
                    .map_err(|e| format!("frame {frame_idx} convert failed: {e}"))?
                    .into_rgba8()
                    .into_raw();
                tx.send((frame_idx, rgba))
                    .map_err(|_| format!("frame {frame_idx}: the video writer terminated"))?;
                Ok(frame_idx)
            });
            queue.tasks.push(task);
        }
        #[cfg(target_arch = "wasm32")]
        {
            // Activation demotes video mode on wasm (no child processes); an
            // active video state here is a logic error, not a user condition.
            let _ = &mut video_sink;
            error!("[offline-record] video mode is unavailable on wasm — aborting");
            teardown_recording(&mut state, &mut commands);
            return;
        }
    } else {
        // PNG-sequence mode: encode + write through `lunco_storage` (see
        // [`encode_and_store`]). Default PNG compression, on purpose:
        // `CompressionType::Fast` was tried and MEASURED (2026-07-19, ~390-frame
        // A/B at 2560×1552) — 65 ms vs 77.5 ms save, SLOWER at the same
        // ~4 MB/frame. If save cost matters again the lever is more overlap,
        // not a cheaper deflate.
        let task = pool.spawn(async move {
            encode_and_store(image, None, &path)
                .map_err(|e| format!("frame {frame_idx}: {e}"))?;
            Ok(frame_idx)
        });
        queue.tasks.push(task);
    }

    debug!(
        "[offline-record] frame {}: readback={:.1}ms outstanding={} saves_in_flight={}",
        frame_idx,
        readback,
        state.outstanding_captures,
        queue.tasks.len(),
    );
}

/// Collect finished async frame saves; on the FIRST failure, abort the recording.
///
/// The abort arrives up to [`MAX_IN_FLIGHT_SAVES`] frames after the write that
/// failed — the price of taking the save off the critical path. Policy is
/// drain-and-abort: stop capturing (teardown), let already-spawned saves finish
/// (nothing is cancelled — the queue keeps draining while inactive), and keep
/// the failed frame's index in the log so the take's usable range is knowable.
/// The alternative — discarding the queue — would throw away frames captured
/// before the failure that would still land fine.
fn pump_offline_saves(
    mut queue: ResMut<OfflineSaveQueue>,
    mut state: ResMut<OfflineRecordingState>,
    mut video_sink: ResMut<OfflineVideoSink>,
    mut commands: Commands,
) {
    use bevy::tasks::futures_lite::future;
    let mut first_failure: Option<String> = None;
    queue.tasks.retain_mut(|task| {
        match future::block_on(future::poll_once(&mut *task)) {
            None => true, // still saving
            Some(Ok(_frame)) => false,
            Some(Err(e)) => {
                if first_failure.is_none() {
                    first_failure = Some(e);
                }
                false
            }
        }
    });
    if let Some(e) = first_failure {
        error!(
            "[offline-record] async frame save failed ({e}) — aborting recording; \
             the sequence is usable up to the frame named above, later frames may \
             be missing"
        );
        if state.active {
            teardown_recording(&mut state, &mut commands);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        // Surface writer-thread failures (broken pipe, ffmpeg died) on the main
        // world, where the recording can actually be aborted.
        if let Some(sink) = video_sink.sink.as_ref() {
            let failure = sink.error.lock().unwrap().take();
            if let Some(e) = failure {
                error!("[offline-record] video sink failed ({e}) — aborting recording");
                if state.active {
                    teardown_recording(&mut state, &mut commands);
                }
            }
        }
        // Finalize: recording over, every readback delivered, every worker done.
        // Dropping the sink drops the last sender; the writer thread flushes its
        // reorder buffer, closes ffmpeg's stdin and waits the trailer out,
        // logging completion from its own thread.
        if video_sink.sink.is_some()
            && !state.active
            && state.outstanding_captures == 0
            && queue.tasks.is_empty()
        {
            info!("[offline-record] all frames handed to the encoder — finalizing video");
            if let Some(sink) = video_sink.sink.take() {
                // Keep the writer's completion flag so a one-shot run can wait
                // for the container trailer before exiting the process.
                video_sink.draining = Some(sink.done.clone());
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = &mut video_sink;
}

/// One-shot length contract: stop after `--record-frames` frames, through the
/// SAME `StopOfflineRecording` command a scenario would send — so drain,
/// finalization and status behave identically however a shot ends.
fn stop_recording_at_limit(
    state: Res<OfflineRecordingState>,
    limit: Option<Res<OfflineRecordLimit>>,
    mut commands: Commands,
) {
    let Some(limit) = limit else { return };
    if state.active && state.frame_index >= limit.0 {
        info!(
            "[offline-record] reached the requested {} frames — stopping",
            limit.0
        );
        commands.trigger(StopOfflineRecording {});
    }
}

/// One-shot process contract (`--offscreen`): exit once the recording has fully
/// drained — recorder inactive, no readback in flight, no save on a worker, and
/// (video mode) the container trailer written. Without every one of those
/// clauses the exit truncates the take; with them the process end IS the
/// "recording is on disk" signal a script can wait on.
fn exit_when_recording_drained(
    exit_requested: Option<Res<ExitAfterRecording>>,
    state: Res<OfflineRecordingState>,
    pending: Option<Res<PendingShotStart>>,
    queue: Res<OfflineSaveQueue>,
    video_sink: Res<OfflineVideoSink>,
    mut exit: bevy::ecs::message::MessageWriter<bevy::app::AppExit>,
    mut fired: Local<bool>,
) {
    if exit_requested.is_none() || *fired {
        return;
    }
    // Not before the shot even started: armed (gate waiting) or active means
    // in progress. And a take must have actually CAPTURED something — without
    // `frame_index > 0` an `--offscreen` session launched for API-driven work
    // (no `--record-offline`) exits the moment it boots, every condition
    // vacuously true.
    if pending.is_some() || state.active || state.frame_index == 0 {
        return;
    }
    if state.outstanding_captures > 0 || !queue.tasks.is_empty() {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if video_sink.sink.is_some() {
            return;
        }
        if let Some(done) = video_sink.draining.as_ref() {
            if !done.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = &video_sink;
    *fired = true;
    info!("[offline-record] recording drained — exiting (one-shot mode)");
    exit.write(bevy::app::AppExit::Success);
}

struct GetOfflineRecordingStatusProvider;
impl lunco_api::queries::ApiQueryProvider for GetOfflineRecordingStatusProvider {
    fn name(&self) -> &'static str {
        "GetOfflineRecordingStatus"
    }
    fn execute(&self, world: &mut World, _params: &serde_json::Value) -> lunco_api::schema::ApiResponse {
        let pending_saves = world.resource::<OfflineSaveQueue>().tasks.len();
        let state = world.resource::<OfflineRecordingState>();
        lunco_api::schema::ApiResponse::ok(serde_json::json!({
            "active": state.active,
            "frame_index": state.frame_index,
            "is_waiting_for_frame": state.is_waiting_for_frame,
            // Encoding straight to a video file (ffmpeg) vs a PNG sequence.
            "video": state.video,
            // Captures whose GPU readback hasn't delivered yet.
            "outstanding_captures": state.outstanding_captures,
            // Frames still deflating/writing on workers. `active == false` with
            // a non-zero count means a stopped shot is draining its tail.
            "pending_saves": pending_saves,
        }))
    }
}

