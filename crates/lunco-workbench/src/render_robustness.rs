//! Render-backend robustness: keep the app alive through transient GPU
//! validation errors, and steer Windows onto DX12.
//!
//! Motivated by two wgpu panics seen on Windows when *resizing the window*:
//!
//!   1. **Depth/color attachment size mismatch** (e.g. depth `(2560, 1600)`
//!      vs. color `(1548, 783)`) — a wgpu *validation* error. It's a one-frame
//!      skew: the surface is reconfigured to the new size before the camera's
//!      computed target size (and the depth texture sized from it) catches up.
//!   2. **`SurfaceAcquireSemaphores still in use`** — a hal Vulkan `panic!`.
//!      It cascades from (1): wgpu's *default* handler panics on the validation
//!      error, unwinding `render_system` mid-frame, so the acquired
//!      `SurfaceTexture` is never presented and its semaphore stays "in use"
//!      when the swapchain is torn down.
//!
//! Two independent, complementary mitigations:
//!
//! * [`preferred_wgpu_settings`] narrows the backend mask to DX12 on Windows so
//!   wgpu never selects the Vulkan adapter that exhibits both bugs. *Prevents*
//!   the races entirely on Windows; overridable via `WGPU_BACKEND`.
//! * [`install_wgpu_error_handler`] replaces wgpu's default panic-on-uncaptured
//!   -error with a logging handler. *Survives* a stray validation error on any
//!   platform/backend: validation errors do not lose the device, so the bad
//!   frame is dropped and the next (correctly-sized) frame renders. Because the
//!   render system no longer unwinds mid-frame, panic (2) is also avoided.

use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::{renderer::RenderDevice, settings::WgpuSettings, RenderApp, RenderStartup};

/// Base [`WgpuSettings`] with a platform-tuned backend preference.
///
/// Windows: default to DX12 (sidesteps the Vulkan resize panics) unless the
/// user set `WGPU_BACKEND` explicitly — that env var stays the escape hatch.
/// Every other platform keeps wgpu's defaults untouched.
pub fn preferred_wgpu_settings() -> WgpuSettings {
    #[allow(unused_mut)]
    let mut settings = WgpuSettings::default();
    #[cfg(target_os = "windows")]
    {
        if std::env::var_os("WGPU_BACKEND").is_none() {
            settings.backends = Some(bevy::render::settings::Backends::DX12);
        }
    }
    settings
}

/// Replace wgpu's default panic-on-uncaptured-error with a logging handler.
///
/// No-op when there is no [`RenderApp`] (headless tests / API-only servers).
pub(crate) fn install_wgpu_error_handler(app: &mut App) {
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app.add_systems(RenderStartup, set_error_handler);
}

/// Runs once in the render world (`RenderStartup`), where `RenderDevice` exists.
fn set_error_handler(device: Res<RenderDevice>) {
    use std::sync::atomic::{AtomicU64, Ordering};
    // A persistently-invalid pipeline (e.g. a material whose shader can't build)
    // raises the SAME validation error EVERY frame — thousands per second. Count
    // them so we log the first occurrence loudly + actionably and then throttle,
    // instead of flooding the log (which itself tanks the frame rate).
    let validation_hits = Arc::new(AtomicU64::new(0));
    device
        .wgpu_device()
        .on_uncaptured_error(Arc::new(move |err: wgpu::Error| match err {
            // Validation errors don't lose the device — the offending command
            // buffer is rejected and we continue. The Windows resize
            // depth/color mismatch lands here; dropping the frame is correct.
            wgpu::Error::Validation { description, .. } => {
                // SMAA without the `smaa_luts` cargo feature binds the area/search
                // LUT as the wrong texture dimension (D3 where D2 is expected),
                // so the "SMAA blending weight" bind group fails validation and
                // EVERY frame is dropped → permanently black viewport. That looked
                // for hours like a lighting/camera-activation bug. Promote it to a
                // loud, actionable error so it can never masquerade as black again.
                if description.contains("SMAA")
                    || (description.contains("dimension = D2")
                        && description.contains("D3"))
                {
                    error!(
                        "wgpu validation error in the SMAA pass — this binary spawns a \
                         camera with `Smaa` but is missing the bevy `smaa_luts` feature, \
                         so every frame is dropped (black viewport). Add `smaa_luts` to \
                         this binary's bevy features. Details: {description}"
                    );
                    return;
                }
                // Rate-limit the otherwise per-frame storm: shout once (pointing at
                // the usual culprit — a bad material shader), then log ~every 600th
                // repeat so a persistent failure stays visible without flooding.
                let n = validation_hits.fetch_add(1, Ordering::Relaxed);
                if n == 0 {
                    warn!(
                        "wgpu validation error (frame dropped, continuing): {description}. \
                         A persistent version of this usually means a material's render \
                         pipeline is invalid — check any `primvars:shaderPath` (a shader \
                         must be a whole shader with an `@fragment` entry, not a library). \
                         Identical errors are now rate-limited."
                    );
                } else if n.is_multiple_of(600) {
                    warn!(
                        "wgpu validation error persists ({} frames dropped): {description}",
                        n + 1
                    );
                }
            }
            other => error!("wgpu error: {other}"),
        }));
}
