//! Render-backend robustness: keep the app alive through transient GPU
//! validation errors, degrade deliberately when they stop being transient, and
//! steer Windows away from the failing DX12 resize path.
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
//! # Surviving a bad frame is not the same as surviving a bad GPU
//!
//! Dropping the frame is right for (1), where the *next* frame is correctly
//! sized and renders. It is exactly wrong when the resource is permanently gone,
//! and a Windows tester found the difference the hard way: on an integrated
//! adapter the directional shadow map hit `Out of Memory`, and every subsequent
//! frame failed in `Texture::create_view` on the now-invalid
//! `directional_light_shadow_map_texture`. Nothing recreated it, so the app sat
//! in a **hot loop rendering nothing at ~339 fps** — 16 201 frames dropped and
//! still going when the log ended, burning CPU, GPU and battery, with no dialog,
//! no dump, and no clue for the tester. Converting a crash into a silent
//! infinite loop is worse than crashing.
//!
//! So a repeated error escalates through [`Ladder`], a three-rung ladder:
//!
//! | rung | when | what |
//! |---|---|---|
//! | `Healthy` | — | drop the bad frame, log rate-limited (transient skew) |
//! | `ShadowMapsOff` | first non-transient error | turn shadow maps off on every light — the reported failure IS the shadow atlas, and this releases it |
//! | `GaveUp` | still failing [`GIVE_UP_AFTER_SECS`] later, or device lost | deactivate every camera, log once, loudly |
//!
//! `GaveUp` stops the null loop rather than the process: the sim, the API and
//! the document model are all still healthy and worth keeping alive — it is only
//! *presentation* that is dead. Exiting would throw away a working session to
//! report a rendering fault.
//!
//! The ladder is a pure state machine precisely so it can be tested without a
//! GPU (see the tests at the bottom); the systems around it only apply what it
//! decides. The one thing no test here can prove is that disabling shadow maps
//! actually clears the driver's invalid-texture state — that needs the Windows
//! adapter that produced it.
//!
//! Two further mitigations, independent of the ladder:
//!
//! * [`preferred_wgpu_settings`] selects Vulkan on Windows. The DX12 backend
//!   can leave a swapchain permanently invalid after `ResizeBuffers` rejects a
//!   resize; `WGPU_BACKEND` remains the explicit override.
//! * [`install_wgpu_error_handler`] replaces wgpu's default panic-on-uncaptured
//!   -error with a logging handler, so the render system no longer unwinds
//!   mid-frame and panic (2) is avoided.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::{
    renderer::{RenderAdapterInfo, RenderDevice},
    settings::WgpuSettings,
    RenderApp, RenderStartup,
};

/// How long a failure must persist *after* shadow maps have been turned off
/// before presentation is abandoned.
///
/// Measured in wall-clock, not frames, deliberately: the failure mode this exists
/// for renders nothing and therefore runs *fast* (~339 fps was measured), so a
/// frame count would give wildly different grace periods to a wedged app and a
/// healthy one. Long enough that a slow shadow-atlas reallocation is not mistaken
/// for a wedge; short enough that nobody cooks a laptop waiting for it.
const GIVE_UP_AFTER_SECS: f64 = 5.0;

/// Base [`WgpuSettings`] with a platform-tuned backend preference.
///
/// Windows: default to Vulkan (sidesteps DX12 swapchain resize failures) unless the
/// user set `WGPU_BACKEND` explicitly — that env var stays the escape hatch.
/// Every other platform keeps wgpu's defaults untouched.
pub fn preferred_wgpu_settings() -> WgpuSettings {
    #[allow(unused_mut)]
    let mut settings = WgpuSettings::default();
    #[cfg(target_os = "windows")]
    {
        if std::env::var_os("WGPU_BACKEND").is_none() {
            settings.backends = Some(bevy::render::settings::Backends::VULKAN);
        }
    }
    settings
}

/// Error tallies shared between the wgpu callback (render thread, no `World`)
/// and the escalation system (main world).
///
/// Atomics rather than a channel: the callback fires potentially thousands of
/// times per second and must stay allocation-free, and the main world only ever
/// needs the *totals*, never the individual events.
#[derive(Debug, Default)]
pub struct RenderHealth {
    /// Every uncaptured error, of any kind. The ladder watches this one: what
    /// matters is "did the count move this frame", not which kind moved it.
    total: AtomicU64,
    /// Errors naming a shadow-map resource — the reported Windows failure.
    /// Recorded to make the log say *why* shadow maps were turned off.
    shadow: AtomicU64,
    /// Out-of-memory specifically. Distinguished because it is a resource
    /// exhaustion the app can act on, not a bug in what we submitted.
    oom: AtomicU64,
    /// The device is gone. Terminal — no rung of the ladder can recover it.
    device_lost: AtomicBool,
    /// Once presentation is abandoned, suppress the render-thread error storm.
    presentation_stopped: AtomicBool,
}

impl RenderHealth {
    fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
    fn device_lost(&self) -> bool {
        self.device_lost.load(Ordering::Relaxed)
    }
}

/// Main-world handle to the shared [`RenderHealth`].
#[derive(Resource, Clone, Debug)]
pub struct RenderHealthHandle(pub Arc<RenderHealth>);

/// Present once presentation has been abandoned, carrying why.
///
/// A resource rather than only a log line so the API surface and any surviving
/// UI can report the state — by the time this exists nothing can be *drawn* to
/// say it.
#[derive(Resource, Clone, Debug)]
pub struct RenderGaveUp {
    /// Human-readable reason presentation was abandoned.
    pub reason: String,
}

/// Which rung of the degradation ladder we are on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Rung {
    #[default]
    Healthy,
    ShadowMapsOff,
    GaveUp,
}

/// What the ladder decided to do this frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Action {
    DisableShadowMaps,
    GiveUp,
}

/// The escalation state machine — pure, so it is testable without a GPU.
#[derive(Resource, Debug, Default)]
pub(crate) struct Ladder {
    pub(crate) rung: Rung,
    /// Error total at the previous evaluation, to detect "did it fail again".
    last_total: u64,
    /// When the current unbroken run of failing frames began, on the current
    /// rung. Cleared the moment a frame renders without a new error, so a
    /// transient burst can never accumulate its way to `GaveUp`.
    failing_since: Option<f64>,
}

impl Ladder {
    /// Advance one evaluation. `now` is monotonic seconds; `total` is
    /// [`RenderHealth::total`].
    pub(crate) fn step(&mut self, total: u64, device_lost: bool, now: f64) -> Option<Action> {
        // Device loss short-circuits every rung: the shadow-map fallback cannot
        // help when there is no device to render with.
        if device_lost {
            if self.rung == Rung::GaveUp {
                return None;
            }
            self.rung = Rung::GaveUp;
            return Some(Action::GiveUp);
        }

        let failed_again = total > self.last_total;
        self.last_total = total;

        if !failed_again {
            // A clean frame ends the run. This is what keeps the one-frame
            // resize skew — the case this module was originally written for —
            // from ever escalating.
            self.failing_since = None;
            return None;
        }

        let since = *self.failing_since.get_or_insert(now);

        match self.rung {
            // First non-transient failure: shed the shadow atlas immediately.
            // Not after N frames — the reported failure was already permanent on
            // frame 1, and every frame spent deciding is a frame rendered wrong.
            Rung::Healthy => {
                self.rung = Rung::ShadowMapsOff;
                // Restart the clock: persistence is now measured against the
                // mitigation, not against the original fault.
                self.failing_since = Some(now);
                Some(Action::DisableShadowMaps)
            }
            Rung::ShadowMapsOff if now - since >= GIVE_UP_AFTER_SECS => {
                self.rung = Rung::GaveUp;
                Some(Action::GiveUp)
            }
            Rung::ShadowMapsOff | Rung::GaveUp => None,
        }
    }
}

/// Install the error handler, the device-lost callback and the escalation ladder.
///
/// No-op when there is no [`RenderApp`] (headless tests / API-only servers).
pub(crate) fn install_wgpu_error_handler(app: &mut App) {
    if app.get_sub_app_mut(RenderApp).is_none() {
        return;
    }

    let health = Arc::new(RenderHealth::default());
    app.insert_resource(RenderHealthHandle(health.clone()));
    app.init_resource::<Ladder>();
    app.add_systems(Update, escalate_render_recovery);

    let render_app = app.get_sub_app_mut(RenderApp).expect("checked above");
    render_app.insert_resource(RenderHealthHandle(health));
    render_app.add_systems(RenderStartup, set_error_handler);
}

/// Render the full `source` chain of a wgpu error.
///
/// `Error::OutOfMemory` carries NO description — only a source — so the previous
/// `error!("wgpu error: {other}")` reduced the single most actionable failure in
/// the Windows report to four words. The chain is where the allocation that
/// failed is actually named.
fn describe(err: &wgpu::Error) -> String {
    use std::error::Error as _;
    let mut out = match err {
        wgpu::Error::OutOfMemory { .. } => "out of memory".to_string(),
        wgpu::Error::Validation { description, .. } => description.clone(),
        wgpu::Error::Internal { description, .. } => description.clone(),
    };
    let mut src = err.source();
    while let Some(e) = src {
        out.push_str("\n  caused by: ");
        out.push_str(&e.to_string());
        src = e.source();
    }
    out
}

/// Runs once in the render world (`RenderStartup`), where `RenderDevice` exists.
fn set_error_handler(
    device: Res<RenderDevice>,
    adapter: Res<RenderAdapterInfo>,
    health: Res<RenderHealthHandle>,
) {
    // Captured once, by value: the callbacks below run on the render thread with
    // no access to the world, and adapter identity is exactly what triage needs
    // (the reported failures were specific to an integrated adapter).
    let info = &adapter.0;
    let adapter_desc = format!(
        "{} ({:?}, backend {:?}, driver {} {})",
        info.name, info.device_type, info.backend, info.driver, info.driver_info
    );

    // ── Device lost ─────────────────────────────────────────────────────────
    //
    // Windows Issue 3: the tester saw only "Caught DeviceLost error: Unknown
    // Device is lost", which names neither the adapter nor the driver and is
    // untriageable. wgpu's safe API does not expose DX12's
    // `GetDeviceRemovedReason`; reaching it means an unsafe hal downcast behind
    // a DX12-only cfg, and we default Windows to Vulkan anyway, so DX12 is only
    // reachable via an explicit `WGPU_BACKEND` override. Log everything the safe
    // API does give, which is already far more than the tester got.
    {
        let health = health.0.clone();
        let adapter_desc = adapter_desc.clone();
        device
            .wgpu_device()
            .set_device_lost_callback(move |reason, message| {
                if health.presentation_stopped.load(Ordering::Relaxed) {
                    return;
                }
                health.device_lost.store(true, Ordering::Relaxed);
                health.total.fetch_add(1, Ordering::Relaxed);
                error!(
                    "GPU device lost ({reason:?}) on {adapter_desc}: {message}. \
                     Rendering cannot be recovered in-process — presentation will stop. \
                     If this is reproducible, re-run with `WGPU_BACKEND=dx12` (or `vulkan`) \
                     to establish whether it is backend-specific."
                );
            });
    }

    // ── Uncaptured errors ───────────────────────────────────────────────────
    let health = health.0.clone();
    let validation_hits = Arc::new(AtomicU64::new(0));
    device
        .wgpu_device()
        .on_uncaptured_error(Arc::new(move |err: wgpu::Error| {
            if health.presentation_stopped.load(Ordering::Relaxed) {
                return;
            }
            let desc = describe(&err);
            health.total.fetch_add(1, Ordering::Relaxed);
            // Matched on the resource LABEL, which is what wgpu actually gives
            // us; the reported labels were `directional_light_shadow_map_texture`
            // and `directional_light_shadow_map_array_texture_view`.
            if desc.contains("shadow_map") {
                health.shadow.fetch_add(1, Ordering::Relaxed);
            }
            if matches!(err, wgpu::Error::OutOfMemory { .. }) {
                health.oom.fetch_add(1, Ordering::Relaxed);
            }

            match err {
                // Validation errors don't lose the device — the offending command
                // buffer is rejected and we continue. The Windows resize
                // depth/color mismatch lands here; dropping the frame is correct.
                wgpu::Error::Validation { .. } => {
                    // SMAA without the `smaa_luts` cargo feature binds the area/search
                    // LUT as the wrong texture dimension (D3 where D2 is expected),
                    // so the "SMAA blending weight" bind group fails validation and
                    // EVERY frame is dropped → permanently black viewport. That looked
                    // for hours like a lighting/camera-activation bug. Promote it to a
                    // loud, actionable error so it can never masquerade as black again.
                    if desc.contains("SMAA")
                        || (desc.contains("dimension = D2") && desc.contains("D3"))
                    {
                        error!(
                            "wgpu validation error in the SMAA pass — this binary spawns a \
                             camera with `Smaa` but is missing the bevy `smaa_luts` feature, \
                             so every frame is dropped (black viewport). Add `smaa_luts` to \
                             this binary's bevy features. Details: {desc}"
                        );
                        return;
                    }
                    // Rate-limit the otherwise per-frame storm: shout once (pointing at
                    // the usual culprit — a bad material shader), then log ~every 600th
                    // repeat so a persistent failure stays visible without flooding.
                    let n = validation_hits.fetch_add(1, Ordering::Relaxed);
                    if n == 0 {
                        warn!(
                            "wgpu validation error (frame dropped, continuing): {desc}. \
                             A persistent version of this usually means a material's render \
                             pipeline is invalid — check any `primvars:shaderPath` (a shader \
                             must be a whole shader with an `@fragment` entry, not a library). \
                             Identical errors are now rate-limited."
                        );
                    } else if n.is_multiple_of(600) {
                        warn!(
                            "wgpu validation error persists ({} frames dropped): {desc}",
                            n + 1
                        );
                    }
                }
                // OOM on a shared-memory adapter is the reported root cause, and
                // it is NOT a droppable frame: the resource that failed to
                // allocate stays invalid for every frame after. Say so once, with
                // the adapter, and let the ladder degrade.
                wgpu::Error::OutOfMemory { .. } => {
                    error!("wgpu out of memory on {adapter_desc}: {desc}");
                }
                wgpu::Error::Internal { .. } => {
                    error!("wgpu internal error on {adapter_desc}: {desc}");
                }
            }
        }));
}

/// Main-world escalation: read the shared tallies, advance the [`Ladder`], apply
/// whatever it decided.
///
/// In the main world rather than the render world because both remedies are
/// main-world state — `DirectionalLight::shadow_maps_enabled` and
/// `Camera::is_active` are extracted to the render world each frame, so setting
/// them here is what actually stops the work being submitted.
fn escalate_render_recovery(
    health: Res<RenderHealthHandle>,
    mut ladder: ResMut<Ladder>,
    time: Res<Time>,
    mut commands: Commands,
    mut dir: Query<&mut bevy::light::DirectionalLight>,
    mut point: Query<&mut bevy::light::PointLight>,
    mut spot: Query<&mut bevy::light::SpotLight>,
    mut cameras: Query<&mut bevy::camera::Camera>,
) {
    let h = &health.0;
    let Some(action) = ladder.step(h.total(), h.device_lost(), time.elapsed_secs_f64()) else {
        return;
    };

    match action {
        Action::DisableShadowMaps => {
            let mut n = 0;
            for mut l in &mut dir {
                l.shadow_maps_enabled = false;
                n += 1;
            }
            for mut l in &mut point {
                l.shadow_maps_enabled = false;
                n += 1;
            }
            for mut l in &mut spot {
                l.shadow_maps_enabled = false;
                n += 1;
            }
            let shadow = h.shadow.load(Ordering::Relaxed);
            let oom = h.oom.load(Ordering::Relaxed);
            warn!(
                "GPU errors are not clearing ({shadow} naming a shadow map, {oom} out-of-memory) \
                 — disabling shadow maps on {n} light(s) to release the shadow atlas and keep \
                 rendering. Shadows are off for the rest of this session; reload after closing \
                 some scene content to get them back. If the errors continue, presentation will \
                 stop in {GIVE_UP_AFTER_SECS:.0}s."
            );
        }
        Action::GiveUp => {
            h.presentation_stopped.store(true, Ordering::Relaxed);
            let mut n = 0;
            for mut c in &mut cameras {
                c.is_active = false;
                n += 1;
            }
            let reason = if h.device_lost() {
                "the GPU device was lost".to_string()
            } else {
                format!(
                    "GPU errors persisted for {GIVE_UP_AFTER_SECS:.0}s after shadow maps \
                     were disabled ({} total, {} out-of-memory)",
                    h.total(),
                    h.oom.load(Ordering::Relaxed)
                )
            };
            error!(
                "PRESENTATION STOPPED: {reason}. Deactivated {n} camera(s) rather than keep \
                 spinning on frames that cannot succeed — an app rendering nothing at full \
                 speed burns CPU, GPU and battery and reports nothing. The simulation, the \
                 API and any open documents are UNAFFECTED and still running; only display \
                 has stopped. Save your work and restart to render again."
            );
            commands.insert_resource(RenderGaveUp { reason });
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::preferred_wgpu_settings;
    use bevy::render::settings::Backends;

    #[test]
    fn windows_defaults_to_vulkan_when_not_overridden() {
        // Cargo test inherits the environment, so this test only describes the
        // default path. An explicit `WGPU_BACKEND` is intentionally allowed to
        // select another backend for driver diagnostics.
        if std::env::var_os("WGPU_BACKEND").is_none() {
            assert_eq!(preferred_wgpu_settings().backends, Some(Backends::VULKAN));
        }
    }
}

/// The ladder is the whole point of this module and none of it needs a GPU: it
/// is a decision about *when* to degrade, driven by two numbers.
///
/// The failure it guards against cannot be reproduced in CI — it needs the
/// Windows integrated adapter that ran out of memory — so the escalation policy
/// is tested directly instead, against the transcript in the tester's report.
#[cfg(test)]
mod tests {
    use super::*;

    /// The original reason this module exists: a one-frame depth/color size skew
    /// during a window resize. Presentation must survive it — the run of failing
    /// frames has length one, so the clock resets and no amount of elapsed time
    /// can turn it into a give-up.
    ///
    /// Note this DOES cost the session its shadow maps, which for a resize skew
    /// is a real over-reaction. It is the deliberate trade: the ladder cannot
    /// tell a permanent invalid-texture from a one-frame skew at the moment it
    /// must decide, and shedding shadows is recoverable where a 339-fps null loop
    /// is not.
    #[test]
    fn a_transient_error_never_gives_up() {
        let mut l = Ladder::default();
        l.step(1, false, 0.0);
        for i in 0..1000 {
            // No new errors: healthy frames, arbitrarily far into the future.
            assert_eq!(l.step(1, false, i as f64), None);
        }
        assert_eq!(l.rung, Rung::ShadowMapsOff);
        assert!(
            l.failing_since.is_none(),
            "a clean frame must reset the clock"
        );
    }

    /// Shadow maps come off on the FIRST non-transient error, not after a
    /// countdown. The reported failure was already permanent on frame one.
    #[test]
    fn first_failure_sheds_shadow_maps() {
        let mut l = Ladder::default();
        assert_eq!(l.step(1, false, 0.0), Some(Action::DisableShadowMaps));
        assert_eq!(l.rung, Rung::ShadowMapsOff);
    }

    /// The 339-fps null loop. Errors keep coming after the mitigation, so
    /// presentation is abandoned — once, and only after the grace period.
    #[test]
    fn persistent_failure_gives_up_after_the_grace_period() {
        let mut l = Ladder::default();
        assert_eq!(l.step(1, false, 0.0), Some(Action::DisableShadowMaps));

        // Still failing, but inside the grace period: hold.
        let mut total = 1;
        let mut t = 0.0;
        while t + 0.5 < GIVE_UP_AFTER_SECS - 0.1 {
            t += 0.5;
            total += 1;
            assert_eq!(l.step(total, false, t), None, "gave up too early at t={t}");
        }

        // Past it: give up exactly once.
        total += 1;
        assert_eq!(
            l.step(total, false, GIVE_UP_AFTER_SECS + 0.01),
            Some(Action::GiveUp)
        );
        total += 1;
        assert_eq!(
            l.step(total, false, 99.0),
            None,
            "give up is not repeatable"
        );
        assert_eq!(l.rung, Rung::GaveUp);
    }

    /// The grace period is measured from the MITIGATION, not from the first
    /// error — otherwise a fallback applied at t=4.9s would be judged a failure
    /// 0.1s later, having had no chance to work.
    #[test]
    fn grace_period_starts_at_the_mitigation() {
        let mut l = Ladder::default();
        assert_eq!(l.step(1, false, 100.0), Some(Action::DisableShadowMaps));
        // 4s after the mitigation — not yet.
        assert_eq!(l.step(2, false, 104.0), None);
        // 5s after the mitigation.
        assert_eq!(l.step(3, false, 105.0), Some(Action::GiveUp));
    }

    /// A recovery that works must be permanent: once frames render again the
    /// ladder holds at `ShadowMapsOff` and never escalates.
    #[test]
    fn a_successful_mitigation_never_escalates() {
        let mut l = Ladder::default();
        l.step(1, false, 0.0);
        // Shadow maps off; frames now render. Total never moves again.
        for i in 0..100 {
            assert_eq!(l.step(1, false, i as f64 * 10.0), None);
        }
        assert_eq!(l.rung, Rung::ShadowMapsOff);
    }

    /// Device loss skips the ladder entirely — no rung of it can recover a
    /// device that is gone, so degrading shadows first would only delay the
    /// report by the grace period.
    #[test]
    fn device_lost_gives_up_immediately() {
        let mut l = Ladder::default();
        assert_eq!(l.step(0, true, 0.0), Some(Action::GiveUp));
        assert_eq!(l.rung, Rung::GaveUp);
        assert_eq!(l.step(0, true, 0.1), None, "give up is not repeatable");
    }

    /// Device loss after a shadow-map fallback still terminates immediately,
    /// rather than waiting out a grace period that cannot help.
    #[test]
    fn device_lost_from_a_degraded_rung_is_still_immediate() {
        let mut l = Ladder::default();
        l.step(1, false, 0.0);
        assert_eq!(l.rung, Rung::ShadowMapsOff);
        assert_eq!(l.step(2, true, 0.5), Some(Action::GiveUp));
    }
}
