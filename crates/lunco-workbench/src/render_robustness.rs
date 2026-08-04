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
//! So a repeated error escalates through [`Ladder`], a resource-aware ladder:
//!
//! | rung | when | what |
//! |---|---|---|
//! | `Healthy` | — | drop the bad frame, log rate-limited (transient skew) |
//! | `ShadowMapsOff` | first shadow-map failure | turn shadow maps off on every light — this releases the named atlas |
//! | `PersistentFailure` | first non-shadow failure | preserve scene state and wait for a bounded give-up decision |
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

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::{
    batching::gpu_preprocessing::{GpuPreprocessingMode, GpuPreprocessingSupport},
    init_gpu_resource,
    renderer::{RenderAdapterInfo, RenderDevice},
    settings::WgpuSettings,
    RenderApp, RenderStartup,
};
use bevy_egui::{egui, EguiContexts};

const INTEGRATED_DIRECTIONAL_SHADOW_SIZE: usize = 1024;
const INTEGRATED_POINT_SHADOW_SIZE: usize = 512;
const INTEGRATED_DIRECTIONAL_CASTERS: usize = 1;
const INTEGRATED_DIRECTIONAL_CASCADES: usize = 2;
const INTEGRATED_POINT_CASTERS: usize = 4;
const INTEGRATED_SPOT_CASTERS: usize = 4;

/// How long a failure must persist after the applicable recovery decision
/// before presentation is abandoned.
///
/// Measured in wall-clock, not frames, deliberately: the failure mode this exists
/// for renders nothing and therefore runs *fast* (~339 fps was measured), so a
/// frame count would give wildly different grace periods to a wedged app and a
/// healthy one. Long enough that a slow shadow-atlas reallocation is not mistaken
/// for a wedge; short enough that nobody cooks a laptop waiting for it.
const GIVE_UP_AFTER_SECS: f64 = 5.0;

/// NVIDIA's PCI vendor ID.
const NVIDIA_VENDOR_ID: u32 = 0x10DE;
/// The Quadro K2100M (Kepler GK107) device ID reported by the failing laptop.
const QUADRO_K2100M_DEVICE_ID: u32 = 0x11FC;

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
            // `WgpuSettings::default()` chose its release validation flags while
            // every native backend (including DX12) was still enabled. DX12
            // needs indirect-call validation, but Vulkan does not; leaving it
            // on makes wgpu create an internal compute pipeline during device
            // creation. Some older Vulkan drivers lose the device at exactly
            // that pipeline, before our render-health recovery can start.
            //
            // Keep an explicit diagnostic override intact. This mirrors Bevy's
            // release default for a Vulkan-only backend while retaining debug
            // builds' full validation surface.
            #[cfg(not(debug_assertions))]
            if std::env::var_os("WGPU_VALIDATION_INDIRECT_CALL").is_none() {
                settings
                    .instance_flags
                    .remove(bevy::render::settings::InstanceFlags::VALIDATION_INDIRECT_CALL);
            }
        }
    }
    settings
}

/// Whether this adapter needs Bevy's CPU preprocessing path.
///
/// The K2100M's Vulkan driver accepts the feature probes for GPU preprocessing,
/// then loses the device while the first scene's materials are prepared. This is
/// intentionally a PCI-ID match rather than a broad NVIDIA rule: newer NVIDIA
/// adapters are known to use the faster path successfully.
fn requires_cpu_preprocessing(info: &wgpu::AdapterInfo) -> bool {
    requires_cpu_preprocessing_for(info.backend, info.vendor, info.device)
}

fn requires_cpu_preprocessing_for(backend: wgpu::Backend, vendor: u32, device: u32) -> bool {
    cfg!(target_os = "windows")
        && backend == wgpu::Backend::Vulkan
        && vendor == NVIDIA_VENDOR_ID
        && device == QUADRO_K2100M_DEVICE_ID
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
    /// Last failure class observed by the callback. This keeps recovery aligned
    /// with the resource that actually failed instead of guessing from totals.
    last_failure_kind: AtomicU8,
    /// Set by render startup so the main world can apply the integrated-adapter
    /// shadow budget before the first scene reaches the render graph.
    integrated_adapter: AtomicBool,
}

impl RenderHealth {
    fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
    fn device_lost(&self) -> bool {
        self.device_lost.load(Ordering::Relaxed)
    }
    fn failure_kind(&self) -> FailureKind {
        FailureKind::from_u8(self.last_failure_kind.load(Ordering::Relaxed))
    }

    fn reset_for_scene(&self) {
        self.total.store(0, Ordering::Relaxed);
        self.shadow.store(0, Ordering::Relaxed);
        self.oom.store(0, Ordering::Relaxed);
        self.presentation_stopped.store(false, Ordering::Relaxed);
        self.last_failure_kind
            .store(FailureKind::Other as u8, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum FailureKind {
    #[default]
    Other = 0,
    ShadowMap = 1,
    OutOfMemory = 2,
}

impl FailureKind {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::ShadowMap,
            2 => Self::OutOfMemory,
            _ => Self::Other,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Other => "a render validation/internal fault",
            Self::ShadowMap => "a shadow-map allocation/validation fault",
            Self::OutOfMemory => "out of memory",
        }
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

/// Persistent presentation warning shown while the simulation is still alive.
#[derive(Resource, Clone, Debug)]
pub struct RenderWarning {
    /// Human-readable warning shown in the workbench overlay.
    pub message: String,
}

/// Keep a render-health decision visible in the surviving workbench UI.
pub(crate) fn draw_render_recovery_banner(
    mut egui_ctx: EguiContexts,
    warning: Option<Res<RenderWarning>>,
    gave_up: Option<Res<RenderGaveUp>>,
) {
    let (title, message, color) = if let Some(gave_up) = gave_up {
        (
            "⚠  PRESENTATION STOPPED",
            gave_up.reason.clone(),
            egui::Color32::from_rgb(255, 120, 120),
        )
    } else if let Some(warning) = warning {
        (
            "⚠  RENDERING DEGRADED",
            warning.message.clone(),
            egui::Color32::from_rgb(255, 210, 110),
        )
    } else {
        return;
    };

    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("lunco_render_recovery"))
        .order(egui::Order::Foreground)
        .interactable(false)
        .fixed_pos(egui::pos2(screen.center().x - 210.0, screen.top() + 12.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(42, 24, 18, 238))
                .corner_radius(10.0)
                .stroke(egui::Stroke::new(1.0, color.linear_multiply(0.75)))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_max_width(420.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new(title).color(color).strong());
                        ui.label(egui::RichText::new(message).color(egui::Color32::WHITE));
                    });
                });
        });
}

/// Which rung of the degradation ladder we are on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Rung {
    #[default]
    Healthy,
    ShadowMapsOff,
    PersistentFailure,
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
    /// rung.
    failing_since: Option<f64>,
    /// Last time a new failure was observed. A short callback gap does not make
    /// a wedged render path look healthy merely because no callback arrived.
    last_failure_at: Option<f64>,
    failure_kind: FailureKind,
}

/// Tracks the scene structure for the integrated-adapter preflight. A scene
/// loads asynchronously, so the budget is re-applied whenever another light
/// entity materialises, then remains dormant until the next teardown.
#[derive(Resource, Default)]
pub(crate) struct ShadowBudgetState {
    light_count: Option<usize>,
}

const FAILURE_QUIET_SECS: f64 = 0.5;

impl Ladder {
    /// Advance one evaluation. `now` is monotonic seconds; `total` is
    /// [`RenderHealth::total`].
    fn step(
        &mut self,
        total: u64,
        kind: FailureKind,
        device_lost: bool,
        now: f64,
    ) -> Option<Action> {
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
            // A callback gap shorter than the quiet threshold is not evidence
            // that a failed resource recovered. This protects the grace clock
            // from render-thread scheduling jitter while still clearing a real
            // one-frame resize skew.
            if self
                .last_failure_at
                .is_some_and(|last| now - last >= FAILURE_QUIET_SECS)
            {
                self.failing_since = None;
                self.last_failure_at = None;
                self.failure_kind = FailureKind::Other;
            }
            return None;
        }

        self.last_failure_at = Some(now);
        let kind_changed = self.failure_kind != kind;
        self.failure_kind = kind;
        let since = *self.failing_since.get_or_insert(now);

        match self.rung {
            // Only the resource named by the error gets this fallback. OOM and
            // unrelated validation faults must not toggle every light in the
            // scene; their bounded outcome is a visible give-up state.
            Rung::Healthy if kind == FailureKind::ShadowMap => {
                self.rung = Rung::ShadowMapsOff;
                // Restart the clock: persistence is now measured against the
                // mitigation, not against the original fault.
                self.failing_since = Some(now);
                Some(Action::DisableShadowMaps)
            }
            Rung::Healthy => {
                self.rung = Rung::PersistentFailure;
                self.failing_since = Some(now);
                None
            }
            Rung::ShadowMapsOff if kind_changed => {
                self.rung = Rung::PersistentFailure;
                self.failing_since = Some(now);
                None
            }
            Rung::ShadowMapsOff | Rung::PersistentFailure if now - since >= GIVE_UP_AFTER_SECS => {
                self.rung = Rung::GaveUp;
                Some(Action::GiveUp)
            }
            Rung::ShadowMapsOff | Rung::PersistentFailure | Rung::GaveUp => None,
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
    app.init_resource::<ShadowBudgetState>();
    // Shadow allocation happens during render extraction. The preflight must
    // observe the fully materialised scene in PostUpdate, after scene-load
    // commands apply but before the render sub-app extracts lights.
    app.add_systems(PostUpdate, apply_integrated_shadow_budget);
    app.add_systems(Update, escalate_render_recovery);

    let render_app = app.get_sub_app_mut(RenderApp).expect("checked above");
    render_app.insert_resource(RenderHealthHandle(health));
    render_app.add_systems(RenderStartup, set_error_handler);
    // This runs after Bevy has probed the adapter and initialized the resource.
    // It affects only the known-bad Quadro/Vulkan combination above.
    render_app.add_systems(
        RenderStartup,
        force_cpu_preprocessing.after(init_gpu_resource::<GpuPreprocessingSupport>),
    );
}

/// Override an optimistic feature probe for the one older adapter on which the
/// first GPU-preprocessing/material frame loses the device.
fn force_cpu_preprocessing(
    adapter: Res<RenderAdapterInfo>,
    mut support: ResMut<GpuPreprocessingSupport>,
) {
    let info = &adapter.0;
    if !requires_cpu_preprocessing(info) || support.max_supported_mode == GpuPreprocessingMode::None
    {
        return;
    }

    support.max_supported_mode = GpuPreprocessingMode::None;
    warn!(
        "{} (Vulkan, PCI {:04X}:{:04X}) reports GPU preprocessing support but is a known \
         device-loss path; using CPU preprocessing for this session",
        info.name, info.vendor, info.device
    );
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
    health.0.integrated_adapter.store(
        info.device_type == wgpu::DeviceType::IntegratedGpu,
        Ordering::Relaxed,
    );
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
                health
                    .last_failure_kind
                    .store(FailureKind::Other as u8, Ordering::Relaxed);
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
            let kind = if matches!(err, wgpu::Error::OutOfMemory { .. }) {
                FailureKind::OutOfMemory
            } else if desc.contains("shadow_map") {
                FailureKind::ShadowMap
            } else {
                FailureKind::Other
            };
            // Once an OOM has occurred, later validation fallout is a symptom
            // of that exhausted allocation, not a new shader diagnosis.
            if kind == FailureKind::OutOfMemory || health.oom.load(Ordering::Relaxed) == 0 {
                health
                    .last_failure_kind
                    .store(kind as u8, Ordering::Relaxed);
            }
            if kind == FailureKind::ShadowMap {
                health.shadow.fetch_add(1, Ordering::Relaxed);
            }
            if kind == FailureKind::OutOfMemory {
                health.oom.fetch_add(1, Ordering::Relaxed);
            }

            match err {
                // Validation errors don't lose the device — the offending command
                // buffer is rejected and we continue. The Windows resize
                // depth/color mismatch lands here; dropping the frame is correct.
                wgpu::Error::Validation { .. } => {
                    if health.oom.load(Ordering::Relaxed) > 0 {
                        let n = validation_hits.fetch_add(1, Ordering::Relaxed);
                        if n == 0 {
                            warn!(
                                "wgpu validation errors follow an out-of-memory failure; \
                                 suppressing shader hints while the render ladder handles \
                                 resource exhaustion: {desc}"
                            );
                        }
                        return;
                    }
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

/// Apply a conservative, deterministic shadow budget before Bevy's PBR render
/// preparation allocates its depth textures. Integrated adapters share memory
/// with the system and are the class that produced the observed directional
/// atlas OOM; shedding quality up front keeps that failure out of the error
/// ladder entirely.
fn apply_integrated_shadow_budget(
    health: Res<RenderHealthHandle>,
    mut state: ResMut<ShadowBudgetState>,
    mut commands: Commands,
    warning: Option<Res<RenderWarning>>,
    mut directional_shadow_map: ResMut<bevy::light::DirectionalLightShadowMap>,
    mut point_shadow_map: ResMut<bevy::light::PointLightShadowMap>,
    cameras: Query<(&bevy::camera::Camera, &GlobalTransform), With<bevy::camera::Camera3d>>,
    mut directionals: Query<(
        Entity,
        &mut bevy::light::DirectionalLight,
        &mut bevy::light::CascadeShadowConfig,
    )>,
    mut points: Query<(Entity, &GlobalTransform, &mut bevy::light::PointLight)>,
    mut spots: Query<(Entity, &GlobalTransform, &mut bevy::light::SpotLight)>,
) {
    if !health.0.integrated_adapter.load(Ordering::Relaxed) {
        return;
    }
    let Some(camera_position) = cameras
        .iter()
        .find(|(camera, _)| camera.is_active)
        .map(|(_, transform)| transform.translation())
    else {
        return;
    };

    let light_count = directionals.iter().count() + points.iter().count() + spots.iter().count();
    if light_count == 0 || state.light_count == Some(light_count) {
        return;
    }
    state.light_count = Some(light_count);

    directional_shadow_map.size = directional_shadow_map
        .size
        .min(INTEGRATED_DIRECTIONAL_SHADOW_SIZE);
    point_shadow_map.size = point_shadow_map.size.min(INTEGRATED_POINT_SHADOW_SIZE);

    let mut directional_entities: Vec<Entity> = directionals
        .iter_mut()
        .filter_map(|(entity, light, _)| light.shadow_maps_enabled.then_some(entity))
        .collect();
    directional_entities.sort();
    for entity in directional_entities
        .iter()
        .skip(INTEGRATED_DIRECTIONAL_CASTERS)
        .copied()
    {
        if let Ok((_, mut light, _)) = directionals.get_mut(entity) {
            light.shadow_maps_enabled = false;
        }
    }
    let directional_shed = directional_entities.len() > INTEGRATED_DIRECTIONAL_CASTERS;
    let directional_layers = directional_entities
        .first()
        .and_then(|entity| directionals.get_mut(*entity).ok())
        .map(|(_, _, mut config)| {
            config.bounds.truncate(INTEGRATED_DIRECTIONAL_CASCADES);
            config.bounds.len()
        })
        .unwrap_or(0);

    let mut point_entities: Vec<(f32, Entity)> = points
        .iter_mut()
        .filter_map(|(entity, transform, light)| {
            light.shadow_maps_enabled.then_some((
                transform.translation().distance_squared(camera_position),
                entity,
            ))
        })
        .collect();
    point_entities.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    for (_, entity) in point_entities.iter().skip(INTEGRATED_POINT_CASTERS) {
        if let Ok((_, _, mut light)) = points.get_mut(*entity) {
            light.shadow_maps_enabled = false;
        }
    }
    let point_shed = point_entities.len() > INTEGRATED_POINT_CASTERS;

    let mut spot_entities: Vec<(f32, Entity)> = spots
        .iter_mut()
        .filter_map(|(entity, transform, light)| {
            light.shadow_maps_enabled.then_some((
                transform.translation().distance_squared(camera_position),
                entity,
            ))
        })
        .collect();
    spot_entities.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    for (_, entity) in spot_entities.iter().skip(INTEGRATED_SPOT_CASTERS) {
        if let Ok((_, _, mut light)) = spots.get_mut(*entity) {
            light.shadow_maps_enabled = false;
        }
    }
    let spot_shed = spot_entities.len() > INTEGRATED_SPOT_CASTERS;

    let directional_bytes = (directional_shadow_map.size as u64)
        .saturating_mul(directional_shadow_map.size as u64)
        .saturating_mul(directional_layers as u64)
        .saturating_mul(4);
    let point_bytes = (point_shadow_map.size as u64)
        .saturating_mul(point_shadow_map.size as u64)
        .saturating_mul(point_entities.len().min(INTEGRATED_POINT_CASTERS) as u64)
        .saturating_mul(6)
        .saturating_mul(4);
    let spot_bytes = (directional_shadow_map.size as u64)
        .saturating_mul(directional_shadow_map.size as u64)
        .saturating_mul(spot_entities.len().min(INTEGRATED_SPOT_CASTERS) as u64)
        .saturating_mul(4);
    let estimated_mib = (directional_bytes + point_bytes + spot_bytes) as f64 / (1024.0 * 1024.0);
    warn!(
        "integrated GPU shadow budget: {} directional caster(s), {} cascade layer(s), {} point caster(s), {} spot caster(s), estimated depth allocation {:.1} MiB (directional {}px, point {}px)",
        directional_entities.len().min(INTEGRATED_DIRECTIONAL_CASTERS),
        directional_layers,
        point_entities.len().min(INTEGRATED_POINT_CASTERS),
        spot_entities.len().min(INTEGRATED_SPOT_CASTERS),
        estimated_mib,
        directional_shadow_map.size,
        point_shadow_map.size,
    );
    if (directional_shed || point_shed || spot_shed) && warning.is_none() {
        commands.insert_resource(RenderWarning {
            message: format!(
                "Integrated GPU shadow budget active: kept {} directional, {} point, and {} spot shadow caster(s); some shadows are intentionally disabled.",
                directional_entities.len().min(INTEGRATED_DIRECTIONAL_CASTERS),
                point_entities.len().min(INTEGRATED_POINT_CASTERS),
                spot_entities.len().min(INTEGRATED_SPOT_CASTERS),
            ),
        });
    }
}

/// Scene teardown is the explicit re-arm boundary for presentation recovery.
/// A new scene gets fresh lights and cameras, so clearing the old ladder and
/// shadow-budget bookkeeping is safe. A lost device remains terminal because
/// no scene reload can recreate the adapter in-process.
pub(crate) fn reset_render_recovery(
    health: Res<RenderHealthHandle>,
    mut ladder: ResMut<Ladder>,
    mut budget: ResMut<ShadowBudgetState>,
    mut commands: Commands,
) {
    if health.0.device_lost() {
        return;
    }
    health.0.reset_for_scene();
    *ladder = Ladder::default();
    budget.light_count = None;
    commands.remove_resource::<RenderWarning>();
    commands.remove_resource::<RenderGaveUp>();
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
    warning: Option<Res<RenderWarning>>,
    mut dir: Query<&mut bevy::light::DirectionalLight>,
    mut point: Query<&mut bevy::light::PointLight>,
    mut spot: Query<&mut bevy::light::SpotLight>,
    mut cameras: Query<&mut bevy::camera::Camera>,
) {
    let h = &health.0;
    let kind = h.failure_kind();
    let action = ladder.step(h.total(), kind, h.device_lost(), time.elapsed_secs_f64());

    if action.is_none() && ladder.rung == Rung::PersistentFailure && warning.is_none() {
        commands.insert_resource(RenderWarning {
            message: format!(
                "Rendering is failing because of {}. No shadow fallback was applied; presentation will stop if it does not recover.",
                kind.label()
            ),
        });
    }

    let Some(action) = action else {
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
            warn!(
                "GPU errors are not clearing ({shadow} naming a shadow map) \
                 — disabling shadow maps on {n} light(s) to release the shadow atlas and keep \
                 rendering. A scene reload re-arms the policy after closing \
                 content; a lost GPU remains terminal. If the errors continue, \
                 presentation will stop in {GIVE_UP_AFTER_SECS:.0}s."
            );
            commands.insert_resource(RenderWarning {
                message: "Rendering recovered with shadow maps disabled. The simulation and API are still running.".to_string(),
            });
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
                    "{} persisted for {GIVE_UP_AFTER_SECS:.0}s ({} total, {} shadow-map, {} out-of-memory)",
                    kind.label(),
                    h.total(),
                    h.shadow.load(Ordering::Relaxed),
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
            commands.insert_resource(RenderWarning {
                message: format!(
                    "Presentation stopped: {reason}. Simulation and API remain available; restart the window to restore rendering."
                ),
            });
            commands.insert_resource(RenderGaveUp { reason });
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::{
        preferred_wgpu_settings, requires_cpu_preprocessing_for, NVIDIA_VENDOR_ID,
        QUADRO_K2100M_DEVICE_ID,
    };
    use bevy::render::settings::{Backends, InstanceFlags};

    #[test]
    fn quadro_k2100m_vulkan_uses_cpu_preprocessing_only() {
        assert!(requires_cpu_preprocessing_for(
            wgpu::Backend::Vulkan,
            NVIDIA_VENDOR_ID,
            QUADRO_K2100M_DEVICE_ID,
        ));
        assert!(!requires_cpu_preprocessing_for(
            wgpu::Backend::Dx12,
            NVIDIA_VENDOR_ID,
            QUADRO_K2100M_DEVICE_ID,
        ));
        assert!(!requires_cpu_preprocessing_for(
            wgpu::Backend::Vulkan,
            NVIDIA_VENDOR_ID,
            0x0001,
        ));
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn release_windows_defaults_to_vulkan_without_indirect_call_validation() {
        // Cargo test inherits the environment, so this test only describes the
        // package default. Explicit backend/validation overrides remain
        // available for driver diagnostics.
        if std::env::var_os("WGPU_BACKEND").is_none()
            && std::env::var_os("WGPU_VALIDATION_INDIRECT_CALL").is_none()
        {
            let settings = preferred_wgpu_settings();
            assert_eq!(settings.backends, Some(Backends::VULKAN));
            assert!(!settings
                .instance_flags
                .contains(InstanceFlags::VALIDATION_INDIRECT_CALL));
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_windows_keeps_indirect_call_validation() {
        // Debug builds deliberately retain wgpu's complete validation surface.
        // This is diagnostic instrumentation, not a different renderer path.
        if std::env::var_os("WGPU_BACKEND").is_none()
            && std::env::var_os("WGPU_VALIDATION_INDIRECT_CALL").is_none()
        {
            let settings = preferred_wgpu_settings();
            assert_eq!(settings.backends, Some(Backends::VULKAN));
            assert!(settings
                .instance_flags
                .contains(InstanceFlags::VALIDATION_INDIRECT_CALL));
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
        l.step(1, FailureKind::ShadowMap, false, 0.0);
        for i in 0..1000 {
            // No new errors: healthy frames, arbitrarily far into the future.
            assert_eq!(l.step(1, FailureKind::ShadowMap, false, i as f64), None);
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
        assert_eq!(
            l.step(1, FailureKind::ShadowMap, false, 0.0),
            Some(Action::DisableShadowMaps)
        );
        assert_eq!(l.rung, Rung::ShadowMapsOff);
    }

    #[test]
    fn integrated_shadow_budget_limits_casters_and_map_sizes() {
        let health = Arc::new(RenderHealth::default());
        health.integrated_adapter.store(true, Ordering::Relaxed);

        let mut app = App::new();
        app.insert_resource(RenderHealthHandle(health));
        app.init_resource::<ShadowBudgetState>();
        app.init_resource::<bevy::light::DirectionalLightShadowMap>();
        app.init_resource::<bevy::light::PointLightShadowMap>();
        app.add_systems(PostUpdate, apply_integrated_shadow_budget);
        app.world_mut().spawn((
            bevy::camera::Camera3d::default(),
            GlobalTransform::default(),
        ));
        for index in 0..(INTEGRATED_POINT_CASTERS + 2) {
            app.world_mut().spawn((
                bevy::light::PointLight {
                    shadow_maps_enabled: true,
                    ..default()
                },
                GlobalTransform::from_xyz(index as f32, 0.0, 0.0),
            ));
        }

        app.update();

        assert_eq!(
            app.world()
                .resource::<bevy::light::DirectionalLightShadowMap>()
                .size,
            INTEGRATED_DIRECTIONAL_SHADOW_SIZE
        );
        assert_eq!(
            app.world()
                .resource::<bevy::light::PointLightShadowMap>()
                .size,
            INTEGRATED_POINT_SHADOW_SIZE
        );
        let world = app.world_mut();
        let mut query = world.query::<&bevy::light::PointLight>();
        let enabled = query
            .iter(&world)
            .filter(|light| light.shadow_maps_enabled)
            .count();
        assert_eq!(enabled, INTEGRATED_POINT_CASTERS);
        assert!(app.world().get_resource::<RenderWarning>().is_some());
    }

    /// The 339-fps null loop. Errors keep coming after the mitigation, so
    /// presentation is abandoned — once, and only after the grace period.
    #[test]
    fn persistent_failure_gives_up_after_the_grace_period() {
        let mut l = Ladder::default();
        assert_eq!(
            l.step(1, FailureKind::ShadowMap, false, 0.0),
            Some(Action::DisableShadowMaps)
        );

        // Still failing, but inside the grace period: hold.
        let mut total = 1;
        let mut t = 0.0;
        while t + 0.5 < GIVE_UP_AFTER_SECS - 0.1 {
            t += 0.5;
            total += 1;
            assert_eq!(
                l.step(total, FailureKind::ShadowMap, false, t),
                None,
                "gave up too early at t={t}"
            );
        }

        // Past it: give up exactly once.
        total += 1;
        assert_eq!(
            l.step(
                total,
                FailureKind::ShadowMap,
                false,
                GIVE_UP_AFTER_SECS + 0.01
            ),
            Some(Action::GiveUp)
        );
        total += 1;
        assert_eq!(
            l.step(total, FailureKind::ShadowMap, false, 99.0),
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
        assert_eq!(
            l.step(1, FailureKind::ShadowMap, false, 100.0),
            Some(Action::DisableShadowMaps)
        );
        // 4s after the mitigation — not yet.
        assert_eq!(l.step(2, FailureKind::ShadowMap, false, 104.0), None);
        // 5s after the mitigation.
        assert_eq!(
            l.step(3, FailureKind::ShadowMap, false, 105.0),
            Some(Action::GiveUp)
        );
    }

    /// A recovery that works must be permanent: once frames render again the
    /// ladder holds at `ShadowMapsOff` and never escalates.
    #[test]
    fn a_successful_mitigation_never_escalates() {
        let mut l = Ladder::default();
        l.step(1, FailureKind::ShadowMap, false, 0.0);
        // Shadow maps off; frames now render. Total never moves again.
        for i in 0..100 {
            assert_eq!(
                l.step(1, FailureKind::ShadowMap, false, i as f64 * 10.0),
                None
            );
        }
        assert_eq!(l.rung, Rung::ShadowMapsOff);
    }

    /// Device loss skips the ladder entirely — no rung of it can recover a
    /// device that is gone, so degrading shadows first would only delay the
    /// report by the grace period.
    #[test]
    fn device_lost_gives_up_immediately() {
        let mut l = Ladder::default();
        assert_eq!(
            l.step(0, FailureKind::Other, true, 0.0),
            Some(Action::GiveUp)
        );
        assert_eq!(l.rung, Rung::GaveUp);
        assert_eq!(
            l.step(0, FailureKind::Other, true, 0.1),
            None,
            "give up is not repeatable"
        );
    }

    /// Device loss after a shadow-map fallback still terminates immediately,
    /// rather than waiting out a grace period that cannot help.
    #[test]
    fn device_lost_from_a_degraded_rung_is_still_immediate() {
        let mut l = Ladder::default();
        l.step(1, FailureKind::ShadowMap, false, 0.0);
        assert_eq!(l.rung, Rung::ShadowMapsOff);
        assert_eq!(
            l.step(2, FailureKind::Other, true, 0.5),
            Some(Action::GiveUp)
        );
    }

    #[test]
    fn oom_does_not_disable_shadow_maps() {
        let mut l = Ladder::default();
        assert_eq!(l.step(1, FailureKind::OutOfMemory, false, 0.0), None);
        assert_eq!(l.rung, Rung::PersistentFailure);
    }

    #[test]
    fn a_short_callback_gap_does_not_reset_persistent_failure_clock() {
        let mut l = Ladder::default();
        assert_eq!(l.step(1, FailureKind::OutOfMemory, false, 0.0), None);
        assert_eq!(l.step(1, FailureKind::OutOfMemory, false, 0.25), None);
        assert_eq!(l.step(2, FailureKind::OutOfMemory, false, 0.75), None);
        assert_eq!(
            l.step(3, FailureKind::OutOfMemory, false, 5.01),
            Some(Action::GiveUp)
        );
    }
}
