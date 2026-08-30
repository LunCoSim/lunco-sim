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
//! - **both** GUI binaries add it (`lunco-luncosim` and `lunica`);
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
//! `CaptureScreenshot` and `CaptureFromCamera` are ordinary commands with ordinary
//! `#[on_command]` handlers. Their answers arrive late — the PNG does not exist until the GPU
//! hands a frame back — so both register as **deferred commands**
//! (`lunco_api::executor::register_deferred_command`) and answers on the request's
//! correlation id when the capture lands.
//!
//! That mechanism is generic and lives in the substrate; `lunco-api` does not know these
//! render-bound commands. A binary without this plugin never registers the types, so requests
//! resolve as ordinary `CommandNotFound` errors.

/// Update-schedule boundary for the offline recorder's aggregate visual
/// readiness consumer. Domain publishers that mirror scene/terrain/render
/// lifecycle state must run before this set so the gate reads the state produced
/// by the current projection frame rather than one frame of stale progress.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OfflineRecordingReadinessSet;

use std::io::Cursor;

use bevy::asset::RenderAssetUsages;
use bevy::image::TextureFormatPixelInfo;
use bevy::prelude::*;
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
use bevy::render::render_resource::{Extent3d, TextureDimension};
use bevy::render::renderer::RenderDevice;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use lunco_api::executor::{ApiResponseEvent, DeferredCommandAppExt, PendingApiRequest};
use lunco_api::schema::ApiResponse;
use lunco_core::{on_command, register_commands, Command, SceneViewport};
use lunco_render::SceneCamera;
use lunco_tools_bevy::{register_closure_tool, ToolResult};

/// **The one screenshot command.**
///
/// Declared HERE, next to the only implementation, so a binary with no render backend does
/// not advertise a command it cannot execute — `DiscoverSchema` (and hence the MCP tool list
/// and the generated command reference) only sees it when this plugin is added.
///
/// The reflected fields are the executable API contract used by the handler and generated
/// command schema.
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
        // holds the HTTP response open for the PNG instead of answering with a
        // provisional acknowledgement.
        app.register_deferred_command::<CaptureScreenshot>()
            .register_deferred_command::<CaptureFromCamera>()
            .add_observer(forward_texture_readback)
            .add_observer(deliver_screenshot);

        // Offline Frame-by-Frame Recording Mode
        app.init_resource::<lunco_core::KeepAwake>()
            .init_resource::<OfflineRenderReadiness>()
            .init_resource::<OfflineRecordingState>()
            .init_resource::<OfflineVideoSettings>()
            .init_resource::<OfflineVideoSink>()
            .add_observer(deliver_offline_frame)
            // The readiness gate. `Update` (not `Last`): it must run before
            // `drive_offline_clock`, which only acts once `state.active` is set —
            // so the shot begins on the same frame it was cleared to begin.
            .add_systems(
                Update,
                start_recording_when_scene_ready.in_set(OfflineRecordingReadinessSet),
            )
            .add_systems(Startup, arm_cli_recording)
            // One-shot contracts: `--record-frames <n>` bounds the take, and
            // `--offscreen` exits the process once the take is on disk.
            .add_systems(
                Update,
                (
                    abort_recording_on_runtime_fault,
                    stop_recording_at_limit,
                    exit_when_recording_drained,
                ),
            )
            // `Last`: the strategy written here is read by `TimeSystem` in `First`
            // next frame, so the decision is made after every other system has run.
            .add_systems(
                Last,
                (dispatch_pending_texture_readbacks, drive_offline_clock).chain(),
            );

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
        // Registers the observers AND the reflected types for both commands. Internal Rhai and
        // behaviour-tree triggers use correlation id 0 and remain fire-and-forget; HTTP calls
        // receive the actual file path or a correlated failure.
        register_all_commands(app);
    }
}

/// Convert the composition root's one-shot CLI request into the ordinary
/// recording command. Keeping this at the command boundary means API, Rhai, and
/// CLI recordings share directory validation, readiness, clock ownership, and
/// teardown semantics.
fn arm_cli_recording(request: Option<Res<OfflineRecordingRequest>>, mut commands: Commands) {
    let Some(request) = request else { return };
    commands.trigger(StartOfflineRecording {
        output_dir: request.output_dir.to_string_lossy().into_owned(),
        fps: request.fps.max(1),
    });
    commands.remove_resource::<OfflineRecordingRequest>();
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
    /// Answer a deferred save-to-file command after the GPU has written the PNG.
    completion_correlation_id: Option<u64>,
    save_path: Option<String>,
    region: Option<(u32, u32, u32, u32)>,
}

/// An API image-target capture requested during command dispatch.
///
/// Texture readback must be inserted in `Last`, after the main-world camera and render-target
/// state has been updated for the frame. The offline recorder already owns that boundary; API
/// captures use this pending component to reach the same boundary instead of sampling the
/// target before the camera has rendered it.
#[derive(Component, Debug, Clone)]
struct PendingTextureReadback {
    target: Handle<bevy::image::Image>,
    request: PendingCapture,
}

fn queue_texture_readback(
    commands: &mut Commands,
    target: Handle<bevy::image::Image>,
    request: PendingCapture,
) {
    commands.spawn(PendingTextureReadback { target, request });
}

/// Dispatch API image-target readbacks at the same schedule boundary as offline recording.
fn dispatch_pending_texture_readbacks(
    pending: Query<(Entity, &PendingTextureReadback)>,
    mut commands: Commands,
) {
    for (entity, pending) in &pending {
        commands.entity(entity).insert((
            Readback::texture(pending.target.clone()),
            pending.request.clone(),
        ));
        commands.entity(entity).remove::<PendingTextureReadback>();
    }
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
        let requested = if cmd.path.is_empty() {
            std::path::PathBuf::from(timestamped_name("screenshot"))
        } else {
            std::path::PathBuf::from(&cmd.path)
        };
        let path = match safe_screenshot_path(&requested) {
            Ok(path) => path,
            Err(error) => {
                commands.trigger(ApiResponseEvent {
                    correlation_id: pending_request.correlation_id,
                    response: ApiResponse::error(
                        lunco_api::schema::ApiErrorCode::InternalError,
                        error,
                    ),
                });
                return;
            }
        };

        // ANSWER NOW. A deferred command owes the caller EXACTLY ONE response on its
        // correlation id — the executor no longer sends one on its behalf. In save-to-file
        // mode the useful answer (the path) is known immediately and there is nothing to wait
        // for, so send it here rather than after the capture. Forgetting this is not a
        // cosmetic bug: the caller would hang until the HTTP timeout.
        commands.trigger(ApiResponseEvent {
            correlation_id: pending_request.correlation_id,
            response: ApiResponse::ok(serde_json::json!({ "path": path })),
        });

        PendingCapture {
            correlation_id: None,
            completion_correlation_id: None,
            save_path: Some(path),
            region,
        }
    } else {
        PendingCapture {
            correlation_id: Some(pending_request.correlation_id),
            completion_correlation_id: None,
            save_path: None,
            region,
        }
    };

    // Spawned HERE, next to the render-bound command implementation, so every binary that
    // installs this plugin uses the same capture and response path.
    if let Some(target) = capture_target {
        queue_texture_readback(&mut commands, target.0.clone(), request);
    } else {
        commands.spawn((Screenshot::primary_window(), request));
    }
}

/// Forward a native texture readback through the same delivery event used by
/// window screenshots. Bevy's `Screenshot::image` temporarily replaces the
/// output attachment for an image target; the offscreen recorder must read the
/// camera's already-rendered target instead.
fn forward_texture_readback(
    trigger: On<ReadbackComplete>,
    readbacks: Query<&Readback>,
    images: Res<Assets<Image>>,
    mut commands: Commands,
) {
    let event = trigger.event();
    let Ok(Readback::Texture(handle)) = readbacks.get(event.entity) else {
        return;
    };
    let Some(source) = images.get(handle) else {
        warn!("[screenshot] texture readback source disappeared before completion");
        return;
    };
    let format = source.texture_descriptor.format;
    let Ok(pixel_size) = format.pixel_size() else {
        warn!("[screenshot] texture readback format {format:?} has no pixel size");
        return;
    };
    let width = source.width();
    let height = source.height();
    let row_bytes = width as usize * pixel_size;
    let aligned_row_bytes = RenderDevice::align_copy_bytes_per_row(row_bytes);
    let expected_bytes = aligned_row_bytes.saturating_mul(height as usize);
    if event.data.len() < expected_bytes {
        warn!(
            "[screenshot] texture readback returned {} bytes, expected at least {}",
            event.data.len(),
            expected_bytes
        );
        return;
    }

    let data = if aligned_row_bytes == row_bytes {
        event.data[..row_bytes * height as usize].to_vec()
    } else {
        event
            .data
            .chunks_exact(aligned_row_bytes)
            .take(height as usize)
            .flat_map(|row| &row[..row_bytes])
            .copied()
            .collect()
    };
    let image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        format,
        RenderAssetUsages::MAIN_WORLD,
    );

    // Match the Screenshot plugin lifecycle: the next First schedule removes
    // captured entities, preventing another readback after this completion.
    commands
        .entity(event.entity)
        .insert(bevy::render::view::screenshot::Captured);
    commands.trigger(ScreenshotCaptured {
        entity: event.entity,
        image,
    });
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
    requests: Query<&PendingCapture>,
    mut commands: Commands,
) {
    let event = trigger.event();
    // Not one of ours (an offline-recording frame) — bail out BEFORE the full-frame
    // clone+convert below, which costs ~8 MB per 1080p frame.
    let Ok(pending) = requests.get(event.entity) else {
        return;
    };
    let correlation_id = pending.correlation_id;
    let completion_correlation_id = pending.completion_correlation_id;
    let save_path = pending.save_path.clone();
    let region = pending.region;

    let Ok(mut dyn_img) = event.image.clone().try_into_dynamic() else {
        error!("[screenshot] failed to convert the captured image");
        if let Some(cid) = completion_correlation_id {
            commands.trigger(ApiResponseEvent {
                correlation_id: cid,
                response: ApiResponse::error(
                    lunco_api::schema::ApiErrorCode::InternalError,
                    "captured image could not be converted",
                ),
            });
        }
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
        if let Err(e) = save_png(std::path::Path::new(&path), &dyn_img) {
            error!("[screenshot] failed to save to '{path}': {e}");
            if let Some(cid) = completion_correlation_id {
                commands.trigger(ApiResponseEvent {
                    correlation_id: cid,
                    response: ApiResponse::error(
                        lunco_api::schema::ApiErrorCode::InternalError,
                        format!("failed to save screenshot to '{path}': {e}"),
                    ),
                });
            }
        } else if let Some(cid) = completion_correlation_id {
            commands.trigger(ApiResponseEvent {
                correlation_id: cid,
                response: ApiResponse::ok(serde_json::json!({ "path": path })),
            });
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
/// drops a no-param call — `photo()` in `control.rhai` sends `{}`. The default (`None`) means
/// capture the explicitly resolved active scene camera.
#[Command(default)]
pub struct CaptureFromCamera {
    /// Vessel whose unique mounted camera to capture from. `None` → the explicitly resolved
    /// active scene camera.
    pub target: Option<Entity>,
}

#[on_command(CaptureFromCamera)]
fn on_capture_from_camera(
    trigger: On<CaptureFromCamera>,
    viewport: Option<Res<SceneViewport>>,
    pending_request: Option<Res<PendingApiRequest>>,
    // `RenderTarget` is a separate component (see `camera_switch.rs`), not a field on
    // `Camera` — query it alongside so we know which window to capture.
    cameras: Query<
        (
            Entity,
            &Camera,
            &Camera3d,
            &bevy::camera::RenderTarget,
            Option<&Name>,
        ),
        With<SceneCamera>,
    >,
    children: Query<&Children>,
    mut commands: Commands,
) {
    let target = trigger.event().target;
    let completion_correlation_id = pending_request
        .as_ref()
        .and_then(|request| (request.correlation_id != 0).then_some(request.correlation_id));
    let path = match safe_screenshot_path(std::path::Path::new(&timestamped_name("photo"))) {
        Ok(path) => path,
        Err(message) => {
            report_capture_failure(&mut commands, message, completion_correlation_id);
            return;
        }
    };
    // The delivery, armed on whichever `Screenshot` entity is spawned below. Without it the
    // frame lands in `deliver_screenshot` with nothing pending and is silently dropped —
    // the instrument believes it photographed and recorded nothing.
    let request = PendingCapture {
        correlation_id: None,
        completion_correlation_id,
        save_path: Some(path),
        region: None,
    };
    let viewport_camera = viewport.as_deref().and_then(|v| v.active_camera);
    let active_camera = || {
        let mut active = cameras
            .iter()
            .filter(|(_, camera, _, _, _)| camera.is_active)
            .map(|(entity, ..)| entity);
        let Some(entity) = active.next() else {
            return None;
        };
        active.next().is_none().then_some(entity)
    };
    let camera_entity = match target {
        // A specific vessel → find a `Camera3d` among its descendants (its USD `def Camera`
        // mount), and name every candidate when the authored contract is ambiguous.
        Some(vessel) => {
            let candidates = find_descendant_cameras(vessel, &cameras, &children);
            match candidates.as_slice() {
                [only] => *only,
                [] => {
                    report_capture_failure(
                        &mut commands,
                        "target vessel has no mounted Camera3d descendants",
                        completion_correlation_id,
                    );
                    return;
                }
                many => {
                    let identities = many
                        .iter()
                        .filter_map(|entity| camera_identity(*entity, &cameras))
                        .collect::<Vec<_>>()
                        .join(", ");
                    report_capture_failure(
                        &mut commands,
                        format!(
                            "target vessel has ambiguous mounted Camera3d descendants: {identities}"
                        ),
                        completion_correlation_id,
                    );
                    return;
                }
            }
        }
        // No target → the camera selected by the viewport authority. Offscreen has no
        // window viewport, so its unique active SceneCamera is the explicit fallback.
        None => match viewport_camera.or_else(active_camera) {
            Some(entity) => entity,
            None => {
                report_capture_failure(
                    &mut commands,
                    "no active viewport camera is resolved",
                    completion_correlation_id,
                );
                return;
            }
        },
    };

    // Bevy's `Screenshot` captures a render TARGET (window/image), not a camera directly.
    let Ok((_, cam, _, rt, _)) = cameras.get(camera_entity) else {
        report_capture_failure(
            &mut commands,
            format!("resolved camera {camera_entity:?} is no longer realized"),
            completion_correlation_id,
        );
        return;
    };

    // Capturing a WINDOW captures whatever camera is actually drawing it — not necessarily
    // the camera we resolved. A vessel's mounted camera is usually INACTIVE (the operator is
    // flying the free camera), so capturing the window here would photograph the operator's
    // viewport and pass it off as the vessel's instrument data. Refuse instead.
    //
    // An inactive mounted camera is rejected because the command would otherwise need to
    // retarget that authored camera and steal a target from the operator. The offscreen
    // presentation camera is already active and uses the native texture-readback path below.
    if !cam.is_active {
        report_capture_failure(
            &mut commands,
            "resolved camera is not active; capture would photograph a different viewport",
            completion_correlation_id,
        );
        return;
    }

    let screenshot = match rt {
        bevy::camera::RenderTarget::Window(w) => match w {
            bevy::window::WindowRef::Primary => Screenshot::primary_window(),
            bevy::window::WindowRef::Entity(entity) => Screenshot::window(*entity),
        },
        // Capture the exact target selected by the camera. A primary-window substitute would
        // silently return a different camera's pixels, and would make offscreen captures fail
        // even though the selected camera is valid. Read the already-rendered image directly;
        // replacing the image's output attachment with Screenshot's intermediate target loses
        // HDR camera output on the windowless path.
        bevy::camera::RenderTarget::Image(image) => {
            queue_texture_readback(&mut commands, image.handle.clone(), request);
            return;
        }
        bevy::camera::RenderTarget::TextureView(texture_view) => {
            Screenshot::texture_view(*texture_view)
        }
        bevy::camera::RenderTarget::None { .. } => {
            report_capture_failure(
                &mut commands,
                "resolved camera has no capturable render target",
                completion_correlation_id,
            );
            return;
        }
    };
    commands.spawn((screenshot, request));
}

fn report_capture_failure(
    commands: &mut Commands,
    message: impl Into<String>,
    correlation_id: Option<u64>,
) {
    let message = message.into();
    warn!("[CaptureFromCamera] {message}");
    lunco_core::trigger_error(commands, "camera-capture-failed", message.clone());
    if let Some(correlation_id) = correlation_id {
        commands.trigger(ApiResponseEvent {
            correlation_id,
            response: ApiResponse::error(lunco_api::schema::ApiErrorCode::InternalError, message),
        });
    }
}

/// Walk `root`'s descendants and return a camera only when the mounted-camera contract is
/// unique. Descendant/entity order is not camera ownership.
fn find_descendant_cameras(
    root: Entity,
    cameras: &Query<
        (
            Entity,
            &Camera,
            &Camera3d,
            &bevy::camera::RenderTarget,
            Option<&Name>,
        ),
        With<SceneCamera>,
    >,
    children: &Query<&Children>,
) -> Vec<Entity> {
    let mut stack = vec![root];
    let mut found = Vec::new();
    while let Some(entity) = stack.pop() {
        if cameras.get(entity).is_ok() {
            found.push(entity);
        }
        if let Ok(kids) = children.get(entity) {
            stack.extend(kids.iter());
        }
    }
    found
}

fn camera_identity(
    entity: Entity,
    cameras: &Query<
        (
            Entity,
            &Camera,
            &Camera3d,
            &bevy::camera::RenderTarget,
            Option<&Name>,
        ),
        With<SceneCamera>,
    >,
) -> Option<String> {
    let Ok((_, _, _, _, name)) = cameras.get(entity) else {
        return None;
    };
    Some(name.map_or_else(|| format!("{entity:?}"), |name| name.as_str().to_string()))
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
            // from the window it renders to. An invalid or ambiguous camera contract reports
            // an error and produces no image.
            world.trigger(CaptureFromCamera {
                target: Some(vessel),
            });
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

/// Keep API-requested screenshot writes inside one explicit output root. The
/// root defaults to the process working directory so existing automation paths
/// such as `target/runtime-smoke.png` remain valid; deployments may set
/// `LUNCO_SCREENSHOT_ROOT` to a dedicated directory. Canonicalizing the parent
/// also rejects `..` traversal and an existing symlink that escapes the root.
#[cfg(not(target_arch = "wasm32"))]
fn safe_screenshot_path(requested: &std::path::Path) -> Result<String, String> {
    let root = std::env::var_os("LUNCO_SCREENSHOT_ROOT")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| "cannot determine screenshot output root".to_string())?;
    let root = lunco_storage::canonicalize_file_path(&root)
        .map_err(|error| format!("screenshot output root is unavailable: {error}"))?;
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    if candidate.extension().and_then(|ext| ext.to_str()) != Some("png") {
        return Err("screenshot path must end in .png".to_string());
    }
    let parent = candidate
        .parent()
        .ok_or_else(|| "screenshot path has no parent directory".to_string())?;
    let parent = lunco_storage::canonicalize_file_path(parent)
        .map_err(|error| format!("screenshot parent directory is unavailable: {error}"))?;
    if !parent.starts_with(&root) {
        return Err(format!(
            "screenshot path must stay under {}",
            root.display()
        ));
    }
    if matches!(
        lunco_storage::entry_kind_file_sync(&candidate),
        Ok(lunco_storage::StorageEntryKind::File) | Ok(lunco_storage::StorageEntryKind::Directory)
    ) {
        let resolved = lunco_storage::canonicalize_file_path(&candidate)
            .map_err(|error| format!("cannot resolve screenshot path: {error}"))?;
        if !resolved.starts_with(&root) {
            return Err("screenshot path resolves outside the output root".to_string());
        }
    }
    Ok(candidate.to_string_lossy().into_owned())
}

/// Encode a captured image as PNG and persist the bytes through the active
/// storage backend. `DynamicImage::save` takes a native path directly, which
/// would bypass storage and cannot work for browser-backed handles.
fn save_png(path: &std::path::Path, image: &image::DynamicImage) -> Result<(), String> {
    let mut png_bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png_bytes), image::ImageFormat::Png)
        .map_err(|error| format!("PNG encode failed: {error}"))?;
    lunco_storage::write_file_sync(path, &png_bytes)
        .map_err(|error| format!("storage write failed: {error}"))
}

#[cfg(target_arch = "wasm32")]
fn safe_screenshot_path(requested: &std::path::Path) -> Result<String, String> {
    // Browser saves are delegated to the browser/runtime; no native filesystem
    // path is writable from wasm.
    Ok(requested.to_string_lossy().into_owned())
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
    /// Cold-GPU warm-up fence for offscreen recording. No simulation time or
    /// output frame is consumed while this is active.
    pub render_warmup_until: Option<web_time::Instant>,
    /// Primary window present mode as it was before recording uncapped it,
    /// restored on stop.
    pub prev_present_mode: Option<bevy::window::PresentMode>,
    /// Virtual-time clamp as it was before recording. Normal interactive runs
    /// use a small catch-up cap; a declared 25 FPS take needs its full 40 ms
    /// frame delta or the film silently runs short.
    pub prev_virtual_max_delta: Option<std::time::Duration>,
    /// Encode straight to a video file via a spawned `ffmpeg` instead of a PNG
    /// sequence (destination named a video file — see [`output_is_video`]).
    /// Demoted back to a PNG sequence at activation if `ffmpeg` is not
    /// installed — a missing encoder must degrade, not crash.
    pub video: bool,
}

/// H.264 preset for direct-to-video capture.
///
/// Offline takes are intermediate footage: the marketing pipeline encodes the
/// assembled master again, so spending the capture interval on compression is
/// pure wall-clock cost. The preset changes compression effort, not pixels or
/// the CRF quality target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OfflineVideoPreset {
    /// Highest-throughput intermediate capture; the final master is encoded later.
    #[default]
    Ultrafast,
    /// Faster-than-default archival intermediate with more compact output.
    Veryfast,
    /// Quality-oriented archival intermediate; slower capture.
    Medium,
}

impl OfflineVideoPreset {
    /// Name accepted by ffmpeg's libx264 encoder.
    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::Ultrafast => "ultrafast",
            Self::Veryfast => "veryfast",
            Self::Medium => "medium",
        }
    }

    /// Parse the public CLI spelling.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ultrafast" => Ok(Self::Ultrafast),
            "veryfast" => Ok(Self::Veryfast),
            "medium" => Ok(Self::Medium),
            _ => Err(format!(
                "invalid record preset `{value}`; expected `ultrafast`, `veryfast`, or `medium`"
            )),
        }
    }
}

/// Process-wide encoder policy. The scene and shot sequencer do not own
/// compression settings; the recording application does.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct OfflineVideoSettings {
    /// H.264 compression-effort preset for direct video takes.
    pub preset: OfflineVideoPreset,
}

/// Command to start frame-by-frame recording.
#[Command(default)]
pub struct StartOfflineRecording {
    /// Target folder. Empty => 'recorded_frames' in the current working dir.
    pub output_dir: String,
    /// Video target FPS (default: 60).
    pub fps: u32,
}

/// Startup request used by one-shot CLI modes.
///
/// The application composition root can insert this before the screenshot
/// plugin is built. The plugin turns it into the same
/// [`StartOfflineRecording`] command used by the API and scenarios, so CLI
/// captures cannot bypass the scene-visual readiness gate.
#[derive(Resource, Debug, Clone)]
pub struct OfflineRecordingRequest {
    /// Target folder or video path.
    pub output_dir: std::path::PathBuf,
    /// Output frame rate.
    pub fps: u32,
}

/// Command to stop frame-by-frame recording.
#[Command(default)]
pub struct StopOfflineRecording {}

/// Does this recording destination mean "encode a video file" rather than
/// "write a PNG sequence into this directory"? Container choice is the
/// caller's; these are the ones ffmpeg infers an H.264-capable muxer for by
/// extension.
pub fn output_is_video(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("mp4" | "mkv" | "mov")
    )
}

/// When present, offline recording captures THIS image each frame instead of
/// the primary window — the offscreen (`--offscreen`) mode's render target.
/// Inserted by the binary that owns the mode (`SandboxOffscreenPlugin`); the
/// recorder itself stays target-agnostic.
#[derive(Resource)]
pub struct OfflineCaptureTarget(pub Handle<bevy::image::Image>);

/// Render-world acknowledgement for the active offscreen scene camera.
///
/// The main world can prove that USD projection and material assets exist, but
/// only the render world can prove that the camera's visibility pass submitted
/// a mesh phase item. The offscreen binary publishes this acknowledgement from
/// its render boundary; the readiness gate consumes it before activating the
/// deterministic recording clock.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct OfflineRenderReadiness {
    /// Main-world camera entity represented by the acknowledged render view.
    pub camera: Option<Entity>,
    /// Number of entities admitted to the view's visibility pass.
    pub visible_entities: usize,
    /// Number of submitted opaque phase items.
    pub opaque_items: usize,
    /// Number of submitted transparent phase items.
    pub transparent_items: usize,
    /// Whether every queued color-phase pipeline for the capture view is ready
    /// for the render pass.
    pub pipelines_ready: bool,
}

/// Stop the recording automatically once `frame_index` reaches this count —
/// the CLI `--record-frames <n>` one-shot contract. Routed through the SAME
/// `StopOfflineRecording` command a scenario would send, so the teardown and
/// status behaviour cannot diverge between the two.
#[derive(Resource)]
pub struct OfflineRecordLimit(pub u64);

/// Marker: exit the app once a recording has finished (recorder inactive with
/// frames on disk — the sink is synchronous, so inactive IS drained).
/// Inserted by one-shot modes (`--offscreen`); a windowed session never wants it.
#[derive(Resource)]
pub struct ExitAfterRecording;

/// A terminal engine fault stopped the current recording. Kept separate from
/// `OfflineRecordingState` so the drain path finalizes the encoder and still
/// returns a failing process status instead of mistaking an aborted take for a
/// successful one.
#[derive(Resource, Debug, Clone)]
pub struct OfflineRecordingFailure(pub String);

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

/// The live `ffmpeg` a video-mode recording is streaming into. `sink` is `None`
/// until the first frame delivers (the encoder needs the real capture
/// dimensions, which are only known then). SYNCHRONOUS by design: frames are
/// piped to `ffmpeg` inline from [`deliver_offline_frame`], exactly where the
/// PNG path writes its file — deliveries arrive strictly in order, one at a
/// time, so there is no writer thread, no channel and no reorder buffer. A slow
/// encoder back-pressures the pipe write, which holds the lock-step clock the
/// same way a slow PNG save does.
#[derive(Resource, Default)]
pub struct OfflineVideoSink {
    #[cfg(not(target_arch = "wasm32"))]
    sink: Option<VideoSink>,
}

impl OfflineVideoSink {
    /// Close the encoder's stdin (EOF ends the raw stream) and WAIT for
    /// `ffmpeg` to write the container trailer — when this returns, the file
    /// is complete on disk. No-op when no sink is live (PNG mode).
    fn finalize(&mut self, frames: u64) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(sink) = self.sink.take() {
            drop(sink.stdin);
            let mut child = sink.child;
            match child.wait() {
                Ok(status) if status.success() => info!(
                    "[offline-record] video finalized: {frames} frames encoded into {}",
                    sink.path.display()
                ),
                Ok(status) => error!(
                    "[offline-record] ffmpeg exited with {status} — {} may be incomplete",
                    sink.path.display()
                ),
                Err(e) => error!(
                    "[offline-record] waiting for ffmpeg failed: {e} — {} may be incomplete",
                    sink.path.display()
                ),
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = frames;
    }
}

/// A spawned `ffmpeg` encoding `rawvideo` piped into its stdin.
#[cfg(not(target_arch = "wasm32"))]
struct VideoSink {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    path: std::path::PathBuf,
    /// Dimensions the encoder was started with — a window resize mid-recording
    /// would corrupt the raw stream, so a mismatching frame aborts instead.
    width: u32,
    height: u32,
}

#[cfg(not(target_arch = "wasm32"))]
impl VideoSink {
    /// Spawn `ffmpeg` encoding raw RGBA frames from stdin into `path`.
    fn spawn(
        path: &std::path::Path,
        width: u32,
        height: u32,
        fps: u32,
        preset: OfflineVideoPreset,
    ) -> std::io::Result<Self> {
        let mut child = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "rawvideo", "-pix_fmt", "rgba"])
            .arg("-s")
            .arg(format!("{width}x{height}"))
            .arg("-r")
            .arg(fps.to_string())
            .args(["-i", "-"])
            .args([
                "-c:v",
                "libx264",
                "-preset",
                preset.ffmpeg_name(),
                "-crf",
                "16",
            ])
            // yuv420p for player compatibility; the crop keeps odd window
            // dimensions legal for it (4:2:0 needs even width/height).
            .args(["-vf", "crop=trunc(iw/2)*2:trunc(ih/2)*2"])
            .args(["-pix_fmt", "yuv420p"])
            .arg(path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin was piped above");
        Ok(Self {
            child,
            stdin,
            path: path.to_path_buf(),
            width,
            height,
        })
    }
}

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
/// once [`scene_visuals_ready`] says so.
///
/// **The clock is deliberately left alone here.** `TimeUpdateStrategy` has exactly
/// ONE writer — `drive_offline_clock` — and that is load-bearing (two writers once
/// produced a 3380-frame runaway). Waiting therefore happens with the clock in its
/// ordinary `Automatic` mode: the wait is real-time and consumes no recorded frames,
/// so the deterministic capture still begins at step 0 with N frames == N steps.
#[on_command(StartOfflineRecording)]
fn on_start_offline_recording(trigger: On<StartOfflineRecording>, mut commands: Commands) {
    let cmd = trigger.event();
    commands.remove_resource::<OfflineRecordingFailure>();
    let dir = if cmd.output_dir.is_empty() {
        std::env::current_dir()
            .unwrap_or_default()
            .join("recorded_frames")
    } else {
        std::path::PathBuf::from(&cmd.output_dir)
    };

    // Fail here rather than after the wait: an unwritable destination is the
    // caller's mistake and should be reported at the point of the request.
    // A video destination (`out.mp4`) needs its PARENT to exist — creating the
    // path itself would plant a directory where ffmpeg wants a file.
    let dir_to_create = if output_is_video(&dir) {
        dir.parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default()
    } else {
        dir.clone()
    };
    if !dir_to_create.as_os_str().is_empty() {
        if let Err(e) = lunco_storage::ensure_directory_sync(&dir_to_create) {
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
        last_blocker: None,
    });
}

/// Flip the armed recorder live. Split out of [`on_start_offline_recording`] so the
/// ready path cannot drift apart.
fn activate_recording(
    pending: &PendingShotStart,
    state: &mut OfflineRecordingState,
    keep_awake: &mut lunco_core::KeepAwake,
    windows: &mut Query<&mut Window, With<bevy::window::PrimaryWindow>>,
    commands: &mut Commands,
    offscreen: bool,
) {
    let dir = pending.output_dir.clone();
    state.active = true;
    state.frame_index = 0;
    state.video = output_is_video(&dir);
    state.output_dir = dir;
    state.fps = pending.fps;
    state.is_waiting_for_frame = false;
    // The presentation gate has already acknowledged a rendered UI frame. Capture
    // that state first; subsequent deliveries set this latch and advance virtual
    // time by exactly one frame before requesting the next capture.
    state.frame_just_captured = false;
    state.prev_virtual_max_delta = None;
    // A camera can be structurally ready before its first offscreen render has
    // completed pipeline compilation. Hold virtual time and frame numbering for
    // one render-second so frame 0 cannot become a clear-color race on a cold GPU.
    state.render_warmup_until =
        offscreen.then(|| web_time::Instant::now() + std::time::Duration::from_secs(1));

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
        if let Err(e) = lunco_storage::ensure_directory_sync(&fallback) {
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
    commands.insert_resource(TimeUpdateStrategy::ManualDuration(
        std::time::Duration::ZERO,
    ));

    // THE diagnostic line for "why does frame 0 look wrong?". It names the wait
    // duration and the last thing the gate was blocked on, so the next occurrence
    // is read off the log instead of guessed at.
    info!(
        "[offline-record] started recording to {} at {} FPS using TimeUpdateStrategy \
         (waited {:.2}s for scene visuals; last blocker: {})",
        state.output_dir.display(),
        state.fps,
        pending.requested_at.elapsed().as_secs_f32(),
        pending
            .last_blocker
            .as_deref()
            .unwrap_or("none — ready on the first check"),
    );
}

/// Wind an active recording down: release the wake token, restore the present
/// mode, and hand the clock back to `Automatic`. EVERY path that ends a recording
/// — the stop command and the failed-write abort alike — must run this; skipping
/// it leaves the last-written `ManualDuration(ZERO)` in place with nothing to
/// replace it, freezing virtual time until the process restarts.
fn teardown_recording(
    state: &mut OfflineRecordingState,
    keep_awake: &mut lunco_core::KeepAwake,
    windows: &mut Query<&mut Window, With<bevy::window::PrimaryWindow>>,
    virtual_time: &mut bevy::time::Time<bevy::time::Virtual>,
    video_sink: &mut OfflineVideoSink,
    commands: &mut Commands,
) {
    state.active = false;
    state.is_waiting_for_frame = false;
    state.frame_just_captured = false;
    state.render_warmup_until = None;

    // Video mode: EOF the encoder and wait for the container trailer. When
    // this returns the take is a complete, playable file — synchronous, like
    // every other part of this recorder.
    video_sink.finalize(state.frame_index);

    // Drop the wake request; the pacer restores the binary's idle policy.
    keep_awake.release();
    if let (Ok(mut window), Some(prev)) = (windows.single_mut(), state.prev_present_mode.take()) {
        window.present_mode = prev;
    }
    if let Some(prev) = state.prev_virtual_max_delta.take() {
        virtual_time.set_max_delta(prev);
    }
    // Restore automatic realtime ticking
    commands.insert_resource(TimeUpdateStrategy::Automatic);
}

#[on_command(StopOfflineRecording)]
fn on_stop_offline_recording(
    _trigger: On<StopOfflineRecording>,
    mut state: ResMut<OfflineRecordingState>,
    mut keep_awake: ResMut<lunco_core::KeepAwake>,
    mut windows: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
    mut virtual_time: ResMut<bevy::time::Time<bevy::time::Virtual>>,
    mut video_sink: ResMut<OfflineVideoSink>,
    mut commands: Commands,
) {
    // Disarm unconditionally: a scenario that gives up on a shot while the gate is
    // still waiting must not leave a `PendingShotStart` behind to fire into the
    // *next* shot's directory.
    commands.remove_resource::<PendingShotStart>();

    if state.active {
        teardown_recording(
            &mut state,
            &mut keep_awake,
            &mut windows,
            &mut virtual_time,
            &mut video_sink,
            &mut commands,
        );
        info!("[offline-record] stopped recording");
    }
}

// ─── The readiness gate ──────────────────────────────────────────────────────

/// A `StartOfflineRecording` that has been accepted but not yet started, because
/// [`scene_visuals_ready`] has not cleared it. Removed when the recorder activates
/// or on `StopOfflineRecording`.
#[derive(Resource, Debug, Clone)]
struct PendingShotStart {
    output_dir: std::path::PathBuf,
    fps: u32,
    /// When the request arrived — reported when the recorder activates.
    requested_at: web_time::Instant,
    /// The most recent reason readiness was refused, kept for the next wait log.
    last_blocker: Option<String>,
}

/// [`StatusBus`](crate::status_bus::StatusBus) sources whose in-flight work makes the
/// scene un-presentable, for clause (3) of [`scene_visuals_ready`].
///
/// An ALLOWLIST, not "is anything busy at all". The bus is shared with work that has
/// nothing to do with what the camera sees — a Modelica compile, a document save, an
/// MCP request — and the MSL download in particular re-pushes progress every frame
/// from boot (see `status_bus::tests::mirrored_progress_preserves_start_time_…`). Gating
/// on the whole bus would therefore stall every shot on unrelated work, adding
/// minutes to an episode and hiding the actual visual blocker.
///
/// These entries are published by `lunco-luncosim`, which mirrors state this crate
/// cannot name onto the bus: terrain by `report_terrain_stream_status` (from
/// `lunco_terrain_surface::TerrainStreamStatus`) and scene by
/// `report_scene_spawn_status` (from `lunco_usd_sim::cosim::SceneLoadInFlight` +
/// `UsdAwaitingStage`), plus Modelica participant state. The entries are the SAME
/// consts the publishers push under, not copies of their spelling — see
/// [`TERRAIN_SOURCE`](crate::status_bus::TERRAIN_SOURCE),
/// [`SCENE_SOURCE`](crate::status_bus::SCENE_SOURCE), and
/// [`MODELICA_SOURCE`](crate::status_bus::MODELICA_SOURCE).
const VISUAL_BUSY_SOURCES: &[&str] = &[
    crate::status_bus::TERRAIN_SOURCE,
    crate::status_bus::TERRAIN_BUILD_SOURCE,
    crate::status_bus::SCENE_SOURCE,
    crate::status_bus::DOME_SOURCE,
    crate::status_bus::MODELICA_SOURCE,
    crate::status_bus::RUNTIME_UI_SOURCE,
];

/// **The one definition of "this scene is presentable".** Returns `None` when the
/// scene's visuals have finished loading, or `Some(reason)` naming what is still
/// outstanding — the reason string is what the readiness log line reports.
///
/// Three clauses, deliberately composed rather than left as scattered ad-hoc checks:
///
/// 1. **There is geometry to record at all.** A backstop for a scene that spawns
///    without ever setting the `SceneLoadInFlight` guard clause (3) reads: with no
///    meshes, clause (2) is vacuously true and the gate would fire on an empty
///    viewport. This mirrors the guard in `start_camera_paths_when_terrain_ready`
///    (`crates/lunco-luncosim/src/lib.rs:2264`), which likewise refuses to read an
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
///    `start_camera_paths_when_terrain_ready` gates camera paths on. Optional
///    derived terrain products use a separate source and do not block
///    presentability.
///
///    Going through the bus rather than the resources keeps `lunco-workbench` a
///    UI-shell crate: it cannot name `TerrainStreamStatus` or `SceneLoadInFlight`
///    without a terrain/USD dependency, and the established pattern is that
///    `lunco-luncosim` mirrors such state onto the bus. A future visual subsystem
///    joins by publishing progress and being listed in [`VISUAL_BUSY_SOURCES`].
fn scene_visuals_ready(
    meshes: &Query<&bevy::mesh::Mesh3d>,
    asset_server: &AssetServer,
    bus: Option<&crate::status_bus::StatusBus>,
    cameras: &Query<(Entity, &Camera, &bevy::camera::RenderTarget)>,
    render_readiness: Option<&OfflineRenderReadiness>,
    offscreen: bool,
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

    // The offscreen target must have one active render camera before the first
    // readback. Physics admission is intentionally not part of this visual
    // gate: the armed recorder freezes virtual time, while USD/Avian admission
    // may need a live frame boundary to finish. `/api/ready` remains the
    // authoritative simulation-readiness contract.
    if offscreen {
        let active_image_cameras = cameras
            .iter()
            .filter(|(_, camera, target)| {
                camera.is_active
                    && matches!(
                        camera.output_mode,
                        bevy::camera::CameraOutputMode::Write { .. }
                    )
                    && matches!(target, bevy::camera::RenderTarget::Image(_))
            })
            .count();
        match active_image_cameras {
            0 => return Some("offscreen render target has no active render camera yet".into()),
            1 => {}
            count => {
                return Some(format!(
                    "offscreen render target has {count} active render cameras"
                ));
            }
        }

        let Some(readiness) = render_readiness else {
            return Some("offscreen render boundary has not acknowledged a scene camera".into());
        };
        let active_camera = cameras
            .iter()
            .find(|(_, camera, target)| {
                camera.is_active
                    && matches!(
                        camera.output_mode,
                        bevy::camera::CameraOutputMode::Write { .. }
                    )
                    && matches!(target, bevy::camera::RenderTarget::Image(_))
            })
            .map(|(entity, ..)| entity);
        if readiness.camera != active_camera {
            return Some("offscreen scene camera has not reached the render world yet".into());
        }
        if !readiness.pipelines_ready {
            return Some("offscreen render pipelines are still compiling".into());
        }
        if readiness.visible_entities == 0
            || readiness.opaque_items + readiness.transparent_items == 0
        {
            return Some(format!(
                "offscreen scene camera has no submitted mesh phase items (visible={}, opaque={}, transparent={})",
                readiness.visible_entities,
                readiness.opaque_items,
                readiness.transparent_items,
            ));
        }
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

/// Start an armed recording as soon as [`scene_visuals_ready`] clears it. The
/// readiness publishers own the lifecycle transitions; this system only
/// consumes their aggregate state. A broken scene remains armed and reports its
/// current blocker; the production scene-test gate owns timeout and failure
/// reporting rather than emitting an invalid recording.
fn start_recording_when_scene_ready(
    pending: Option<ResMut<PendingShotStart>>,
    mut state: ResMut<OfflineRecordingState>,
    mut keep_awake: ResMut<lunco_core::KeepAwake>,
    mut windows: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
    meshes: Query<&bevy::mesh::Mesh3d>,
    asset_server: Res<AssetServer>,
    cameras: Query<(Entity, &Camera, &bevy::camera::RenderTarget)>,
    viewport: Res<SceneViewport>,
    capture_target: Option<Res<OfflineCaptureTarget>>,
    render_readiness: Option<Res<OfflineRenderReadiness>>,
    // `Option`: the bus belongs to the workbench UI, which a headless/API-only
    // binary does not add. Absent simply means clause (3) has nothing to say.
    bus: Option<Res<crate::status_bus::StatusBus>>,
    mut commands: Commands,
) {
    let Some(mut pending) = pending else { return };

    let blocker = scene_visuals_ready(
        &meshes,
        &asset_server,
        bus.as_deref(),
        &cameras,
        render_readiness.as_deref(),
        capture_target.is_some(),
    );
    if let Some(reason) = blocker {
        if pending.last_blocker.as_deref() != Some(reason.as_str()) {
            pending.last_blocker = Some(reason.clone());
            debug!("[offline-record] waiting for scene visuals — {reason}");
        }
        return;
    }

    info!(
        "[offline-record] readiness snapshot: mesh_entities={} active_camera={:?} render={:?}",
        meshes.iter().len(),
        viewport.active_camera,
        render_readiness.as_deref(),
    );

    activate_recording(
        &pending,
        &mut state,
        &mut keep_awake,
        &mut windows,
        &mut commands,
        capture_target.is_some(),
    );
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
    // `--offscreen` mode: capture the offscreen render target, not the
    // (nonexistent) primary window.
    capture_target: Option<Res<OfflineCaptureTarget>>,
    // A live Modelica worker is a fixed-step barrier. Keep the recording clock
    // frozen after a capture until the worker releases the next physics step;
    // otherwise a slow worker produces duplicate frames at the same simulation
    // time and the captured sequence outruns its force state.
    coupling: Option<Res<lunco_core::SimulationBarrier>>,
    mut virtual_time: ResMut<bevy::time::Time<bevy::time::Virtual>>,
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
    // Safe against a simulation deadlock: the scenario script does not need to tick
    // during this window (it has already issued `shot_begin` and is polling
    // `shot_frame()`, which reports `-1` while armed). An incomplete scene stays
    // visibly armed until the caller stops it or the outer production gate reports
    // failure.
    if pending.is_some() && !state.active {
        commands.insert_resource(TimeUpdateStrategy::ManualDuration(
            std::time::Duration::ZERO,
        ));
        return;
    }

    if !state.active {
        return;
    }

    if let Some(until) = state.render_warmup_until {
        if web_time::Instant::now() < until {
            commands.insert_resource(TimeUpdateStrategy::ManualDuration(
                std::time::Duration::ZERO,
            ));
            return;
        }
        state.render_warmup_until = None;
        info!("[offline-record] offscreen render warm-up complete; capturing frame 0");
    }

    let frame_dur = std::time::Duration::from_secs_f64(1.0 / state.fps as f64);
    if state.prev_virtual_max_delta.is_none() {
        state.prev_virtual_max_delta = Some(virtual_time.max_delta());
    }
    // The simulator's normal 33 ms cap prevents interactive catch-up storms.
    // It must not clip the recorder's explicit frame duration: at 25 FPS that
    // would turn a nominal 40 s take into roughly 32 s of simulation.
    if virtual_time.max_delta() < frame_dur {
        virtual_time.set_max_delta(frame_dur);
    }

    if state.is_waiting_for_frame {
        // Capture in flight — hold the clock so the pending frame stays the one
        // that was rendered when it was requested.
        commands.insert_resource(TimeUpdateStrategy::ManualDuration(
            std::time::Duration::ZERO,
        ));
    } else if state.frame_just_captured {
        if coupling
            .as_deref()
            .is_some_and(|coupling| coupling.held && coupling.active_participants > 0)
        {
            commands.insert_resource(TimeUpdateStrategy::ManualDuration(
                std::time::Duration::ZERO,
            ));
            return;
        }
        // A frame landed: let the next frame advance by exactly one step.
        state.frame_just_captured = false;
        commands.insert_resource(TimeUpdateStrategy::ManualDuration(frame_dur));
    } else {
        // Time advanced this frame and the scene is rendered — capture it, then
        // hold the clock until the readback delivers.
        if let Some(target) = &capture_target {
            commands.spawn(Readback::texture(target.0.clone()));
        } else {
            commands.spawn(Screenshot::primary_window());
        }
        state.is_waiting_for_frame = true;
        commands.insert_resource(TimeUpdateStrategy::ManualDuration(
            std::time::Duration::ZERO,
        ));
    }
}

/// Observer for Bevy's ScreenshotCaptured event.
fn deliver_offline_frame(
    trigger: On<ScreenshotCaptured>,
    requests: Query<&PendingCapture>,
    mut state: ResMut<OfflineRecordingState>,
    mut keep_awake: ResMut<lunco_core::KeepAwake>,
    mut windows: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
    mut virtual_time: ResMut<bevy::time::Time<bevy::time::Virtual>>,
    mut video_sink: ResMut<OfflineVideoSink>,
    video_settings: Res<OfflineVideoSettings>,
    mut commands: Commands,
) {
    if !state.active || !state.is_waiting_for_frame {
        return;
    }

    let event = trigger.event();
    // A frame with a `PendingCapture` belongs to `deliver_screenshot` — an HTTP
    // screenshot or a `take_photo` taken mid-recording, not the sequence's frame.
    if requests.get(event.entity).is_ok() {
        return;
    }
    let Ok(dyn_img) = event.image.clone().try_into_dynamic() else {
        error!(
            "[offline-record] failed to convert image for frame {}",
            state.frame_index
        );
        state.is_waiting_for_frame = false;
        return;
    };

    // Deliver the frame synchronously to its sink. Either sink failing ABORTS
    // the recording: continuing would advance `frame_index` past a frame that
    // never landed, leaving a hole in the take. Nothing downstream notices — the
    // scenario keeps sequencing off `frame_index`, and the output silently
    // jumps. A disk that fills mid-capture is the ordinary way to hit this, so
    // fail loudly at the first bad frame rather than emit a corrupt take.
    #[cfg(not(target_arch = "wasm32"))]
    if state.video {
        // VIDEO SINK: pipe the raw RGBA straight into ffmpeg. Strictly inline
        // and in order — the write blocks if the encoder lags, which holds the
        // lock-step clock exactly like a slow PNG save.
        use std::io::Write;
        let rgba = dyn_img.to_rgba8();
        let (width, height) = rgba.dimensions();
        if video_sink.sink.is_none() {
            // First frame: only now are the real capture dimensions known.
            match VideoSink::spawn(
                &state.output_dir,
                width,
                height,
                state.fps.max(1),
                video_settings.preset,
            ) {
                Ok(sink) => {
                    info!(
                        "[offline-record] streaming {width}x{height} @ {} FPS into {} via ffmpeg (preset {})",
                        state.fps.max(1),
                        state.output_dir.display(),
                        video_settings.preset.ffmpeg_name(),
                    );
                    video_sink.sink = Some(sink);
                }
                Err(e) => {
                    error!("[offline-record] failed to start ffmpeg ({e}) — aborting recording");
                    teardown_recording(
                        &mut state,
                        &mut keep_awake,
                        &mut windows,
                        &mut virtual_time,
                        &mut video_sink,
                        &mut commands,
                    );
                    return;
                }
            }
        }
        let sink = video_sink.sink.as_mut().expect("spawned above");
        if (width, height) != (sink.width, sink.height) {
            error!(
                "[offline-record] capture size changed mid-recording ({}x{} -> {width}x{height}) \
                 — aborting; a raw video stream cannot change dimensions",
                sink.width, sink.height
            );
            teardown_recording(
                &mut state,
                &mut keep_awake,
                &mut windows,
                &mut virtual_time,
                &mut video_sink,
                &mut commands,
            );
            return;
        }
        if let Err(e) = sink.stdin.write_all(rgba.as_raw()) {
            error!(
                "[offline-record] ffmpeg pipe write failed at frame {} ({e}) — aborting recording",
                state.frame_index
            );
            teardown_recording(
                &mut state,
                &mut keep_awake,
                &mut windows,
                &mut virtual_time,
                &mut video_sink,
                &mut commands,
            );
            return;
        }
        trace!("[offline-record] encoded frame {}", state.frame_index);
    } else {
        // PNG SINK: one file per frame into the destination directory.
        let path = state
            .output_dir
            .join(format!("frame_{:06}.png", state.frame_index));
        if let Err(e) = save_png(&path, &dyn_img) {
            error!(
                "[offline-record] failed to save frame {} ({e}) — aborting recording to \
                 avoid a sequence with holes in it",
                state.frame_index
            );
            teardown_recording(
                &mut state,
                &mut keep_awake,
                &mut windows,
                &mut virtual_time,
                &mut video_sink,
                &mut commands,
            );
            return;
        }
        trace!("[offline-record] saved frame {}", state.frame_index);
    }
    // wasm never has a live video sink (`ffmpeg_available` is false there, so
    // activation always demotes to the PNG sequence).
    #[cfg(target_arch = "wasm32")]
    {
        let path = state
            .output_dir
            .join(format!("frame_{:06}.png", state.frame_index));
        if let Err(e) = save_png(&path, &dyn_img) {
            error!(
                "[offline-record] failed to save frame {} ({e}) — aborting recording to \
                 avoid a sequence with holes in it",
                state.frame_index
            );
            teardown_recording(
                &mut state,
                &mut keep_awake,
                &mut windows,
                &mut virtual_time,
                &mut video_sink,
                &mut commands,
            );
            return;
        }
        trace!("[offline-record] saved frame {}", state.frame_index);
    }

    state.frame_index += 1;
    state.is_waiting_for_frame = false;
    // `drive_offline_clock` owns `TimeUpdateStrategy`; just signal that a frame
    // landed so it can schedule the single `1/fps` step.
    state.frame_just_captured = true;
}

/// The CLI `--record-frames <n>` one-shot contract: once the recorder has
/// captured `n` frames, stop — through the SAME `StopOfflineRecording` command
/// a scenario would send, so the two stop paths cannot diverge. This is what
/// bounds an unattended capture; without it a CLI-armed recording spools frames
/// until the process is killed.
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

/// Stop a capture at the first engine-owned terminal fault. The physics/cosim
/// owners already hold their schedules; this boundary also makes the recorder's
/// result truthful and guarantees the encoder follows its normal finalization
/// path.
fn abort_recording_on_runtime_fault(
    state: Res<OfflineRecordingState>,
    faults: Option<Res<lunco_core::RuntimeFaults>>,
    mut commands: Commands,
    mut fired: Local<bool>,
) {
    if *fired || !state.active {
        return;
    }
    let Some(fault) = faults.and_then(|faults| faults.first.clone()) else {
        return;
    };
    error!(
        "[offline-record] aborting capture on {} for {}: {}",
        fault.kind, fault.subject, fault.detail
    );
    commands.insert_resource(OfflineRecordingFailure(format!(
        "{}: {} ({})",
        fault.kind, fault.subject, fault.detail
    )));
    commands.trigger(StopOfflineRecording {});
    *fired = true;
}

/// One-shot process contract (`--offscreen`): exit once the recording has
/// finished. Saves here are synchronous — a frame is on disk before
/// `frame_index` advances — so "recorder inactive, nothing armed, no readback
/// in flight, at least one frame captured" IS the drained condition. The
/// `frame_index > 0` clause keeps an `--offscreen` session launched for
/// API-driven work (no `--record-offline`) from exiting the moment it boots.
fn exit_when_recording_drained(
    exit_requested: Option<Res<ExitAfterRecording>>,
    state: Res<OfflineRecordingState>,
    failure: Option<Res<OfflineRecordingFailure>>,
    pending: Option<Res<PendingShotStart>>,
    mut exit: bevy::ecs::message::MessageWriter<bevy::app::AppExit>,
    mut fired: Local<bool>,
) {
    if exit_requested.is_none() || *fired {
        return;
    }
    if pending.is_some() || state.active || state.is_waiting_for_frame || state.frame_index == 0 {
        return;
    }
    if let Some(failure) = failure {
        error!(
            "[offline-record] recording drained after terminal failure ({} frames): {}",
            state.frame_index, failure.0
        );
        *fired = true;
        exit.write(bevy::app::AppExit::error());
        return;
    }
    info!(
        "[offline-record] recording drained ({} frames) — exiting (ExitAfterRecording)",
        state.frame_index
    );
    *fired = true;
    exit.write(bevy::app::AppExit::Success);
}

struct GetOfflineRecordingStatusProvider;
impl lunco_api::queries::ApiQueryProvider for GetOfflineRecordingStatusProvider {
    fn name(&self) -> &'static str {
        "GetOfflineRecordingStatus"
    }
    fn execute(
        &self,
        world: &World,
        _params: &serde_json::Value,
    ) -> lunco_api::schema::ApiResponse {
        let state = world.resource::<OfflineRecordingState>();
        lunco_api::schema::ApiResponse::ok(serde_json::json!({
            "active": state.active,
            "frame_index": state.frame_index,
            "is_waiting_for_frame": state.is_waiting_for_frame,
        }))
    }
}
