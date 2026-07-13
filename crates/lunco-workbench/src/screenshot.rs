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
use lunco_core::{on_command, register_commands, Command};

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

register_commands!(on_capture_screenshot);

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

/// `web_time`, not `std::time`: `std::time::SystemTime::now()` panics on wasm32 and trips the
/// `disallowed_methods` lint.
fn timestamped_name() -> String {
    let secs = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("screenshot_{secs}.png")
}
