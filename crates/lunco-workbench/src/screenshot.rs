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

register_commands!(on_capture_screenshot, on_capture_from_camera);

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
