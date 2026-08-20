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
    extract_resource::{ExtractResource, ExtractResourcePlugin},
    init_gpu_resource,
    renderer::{RenderAdapterInfo, RenderDevice},
    settings::WgpuSettings,
    ExtractSchedule, MainWorld, Render, RenderApp, RenderStartup, RenderSystems,
};
use bevy_egui::{egui, EguiContexts};
use lunco_render::{
    estimate_shadow_allocation_bytes, GpuShadowAdapterLimit, RenderingQualitySettings,
    ShadowMapSuppressed, ShadowMapSuppressionReason,
};
use lunco_settings::AppSettingsExt;

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
}

/// Shared adapter-budget result. RenderStartup runs in the render world, while
/// the quality projection runs in the main world, so the result crosses the
/// existing ExtractSchedule boundary through this tiny atomic handle.
#[derive(Resource, Clone, Debug)]
struct ShadowAdapterLimitHandle {
    limit_bytes: Arc<AtomicU64>,
    revision: Arc<AtomicU64>,
}

impl Default for ShadowAdapterLimitHandle {
    fn default() -> Self {
        Self {
            limit_bytes: Arc::new(AtomicU64::new(GpuShadowAdapterLimit::default().limit_bytes)),
            revision: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl ShadowAdapterLimitHandle {
    fn publish(&self, limit_bytes: u64) {
        self.limit_bytes.store(limit_bytes, Ordering::Release);
        self.revision.fetch_add(1, Ordering::Release);
    }

    fn limit_bytes(&self) -> u64 {
        self.limit_bytes.load(Ordering::Acquire)
    }

    fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }
}

const INTEGRATED_SHADOW_ADAPTER_LIMIT_BYTES: u64 = 16 * 1024 * 1024;
const DISCRETE_SHADOW_BUDGET_BYTES: u64 = 128 * 1024 * 1024;
const CPU_SHADOW_BUDGET_BYTES: u64 = 8 * 1024 * 1024;
const UNKNOWN_SHADOW_BUDGET_BYTES: u64 = 32 * 1024 * 1024;

fn recommended_shadow_adapter_limit(info: &wgpu::AdapterInfo) -> u64 {
    match info.device_type {
        wgpu::DeviceType::IntegratedGpu => INTEGRATED_SHADOW_ADAPTER_LIMIT_BYTES,
        wgpu::DeviceType::DiscreteGpu => DISCRETE_SHADOW_BUDGET_BYTES,
        wgpu::DeviceType::Cpu => CPU_SHADOW_BUDGET_BYTES,
        wgpu::DeviceType::Other | wgpu::DeviceType::VirtualGpu => UNKNOWN_SHADOW_BUDGET_BYTES,
    }
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

/// Cross-world gate for the render schedule. Camera deactivation prevents view
/// extraction, but Bevy still runs the render graph and egui/pipeline systems
/// when another camera or a non-camera pass remains. This resource is extracted
/// into the render world and gates every render-stage set, so presentation and
/// GPU submission actually stop while the main simulation/API keeps running.
#[derive(Resource, Clone, Copy, Default, ExtractResource)]
pub(crate) struct PresentationState {
    pub stopped: bool,
}

fn presentation_is_active(state: Option<Res<PresentationState>>) -> bool {
    state.is_none_or(|state| !state.stopped)
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
    theme: Option<Res<lunco_theme::Theme>>,
) {
    let theme = theme
        .map(|theme| theme.clone())
        .unwrap_or_else(lunco_theme::Theme::dark);
    let (title, message, color, fill) = if let Some(gave_up) = gave_up {
        (
            "⚠  PRESENTATION STOPPED",
            gave_up.reason.clone(),
            theme.tokens.error,
            theme.tokens.alert_backdrop,
        )
    } else if let Some(warning) = warning {
        (
            "⚠  RENDERING DEGRADED",
            warning.message.clone(),
            theme.tokens.warning,
            theme.tokens.overlay_backdrop,
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
                .fill(fill)
                .corner_radius(10.0)
                .stroke(egui::Stroke::new(1.0, color.linear_multiply(0.75)))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_max_width(420.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new(title).color(color).strong());
                        ui.label(egui::RichText::new(message).color(theme.tokens.text));
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

/// Tracks the scene structure for shadow admission. A scene loads
/// asynchronously, so the policy is re-applied whenever another light entity
/// materialises, then remains dormant until the next teardown.
#[derive(Resource, Default)]
pub(crate) struct ShadowAdmissionState {
    light_count: Option<usize>,
    /// Number of currently enabled shadow casters. This changes when the
    /// recovery ladder sheds maps and changes back when an explicit re-arm
    /// restores them, even if the scene contains the same total lights.
    enabled_caster_count: Option<usize>,
    directional_map_size: Option<usize>,
    point_map_size: Option<usize>,
    directional_cascade_layers: Option<usize>,
    budget_bytes: Option<u64>,
    configuration_signature: Option<u64>,
    policy_signature: Option<u64>,
}

const FAILURE_QUIET_SECS: f64 = 0.5;

impl Ladder {
    /// Re-arm after an explicit user quality change. A re-arm is only valid
    /// after the shadow-specific mitigation; device loss and unrelated render
    /// failures remain terminal/persistent respectively.
    fn rearm(&mut self, total: u64) {
        if self.rung != Rung::ShadowMapsOff {
            return;
        }
        self.rung = Rung::Healthy;
        self.last_total = total;
        self.failing_since = None;
        self.last_failure_at = None;
        self.failure_kind = FailureKind::Other;
    }

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
    app.register_settings_section::<RenderingQualitySettings>();
    app.init_resource::<GpuShadowAdapterLimit>();

    if app.get_sub_app_mut(RenderApp).is_none() {
        return;
    }

    let health = Arc::new(RenderHealth::default());
    app.insert_resource(RenderHealthHandle(health.clone()));
    app.init_resource::<Ladder>();
    app.init_resource::<PresentationState>();
    app.add_plugins(ExtractResourcePlugin::<PresentationState>::default());
    app.add_systems(
        Update,
        (
            escalate_render_recovery,
            apply_render_quality.run_if(render_quality_changed),
        )
            .chain(),
    );
    app.init_resource::<ShadowAdmissionState>();
    // Shadow allocation happens during render extraction. The preflight must
    // observe the fully materialised scene in PostUpdate, after scene-load
    // commands apply but before the render sub-app extracts lights.
    app.add_systems(PostUpdate, apply_shadow_budget_policy);

    let adapter_limit = ShadowAdapterLimitHandle::default();
    app.insert_resource(adapter_limit.clone());
    let render_app = app.get_sub_app_mut(RenderApp).expect("checked above");
    render_app.insert_resource(RenderHealthHandle(health));
    render_app.insert_resource(adapter_limit);
    // Once the presentation decision is terminal, stop all render-stage work,
    // including `render_system`, whose only caller presents the swapchain.
    // Extraction may run one final time to propagate this resource; no render
    // schedule work is submitted after the gate becomes visible there.
    render_app.configure_sets(
        Render,
        (
            RenderSystems::ExtractCommands,
            RenderSystems::PrepareAssets,
            RenderSystems::PrepareMeshes,
            RenderSystems::CreateViews,
            RenderSystems::Specialize,
            RenderSystems::PrepareViews,
            RenderSystems::Queue,
            RenderSystems::PhaseSort,
            RenderSystems::Prepare,
            RenderSystems::Render,
            RenderSystems::Cleanup,
            RenderSystems::PostCleanup,
        )
            .run_if(presentation_is_active),
    );
    render_app.add_systems(
        RenderStartup,
        (set_error_handler, configure_shadow_adapter_limit).chain(),
    );
    // RenderStartup cannot borrow the simulation world directly. Publish the
    // adapter result on the normal extraction boundary so scene projectors see
    // the safe budget before asynchronous USD lights arrive.
    render_app.add_systems(ExtractSchedule, publish_shadow_adapter_limit);
    // This runs after Bevy has probed the adapter and initialized the resource.
    // It affects only the known-bad Quadro/Vulkan combination above.
    render_app.add_systems(
        RenderStartup,
        force_cpu_preprocessing.after(init_gpu_resource::<GpuPreprocessingSupport>),
    );
}

fn configure_shadow_adapter_limit(
    adapter: Res<RenderAdapterInfo>,
    budget: Res<ShadowAdapterLimitHandle>,
) {
    let limit_bytes = recommended_shadow_adapter_limit(&adapter.0);
    budget.publish(limit_bytes);
    info!(
        "rendering quality shadow budget: {} MiB for {} ({:?})",
        limit_bytes / (1024 * 1024),
        adapter.0.name,
        adapter.0.device_type,
    );
}

fn publish_shadow_adapter_limit(
    mut main_world: ResMut<MainWorld>,
    budget: Res<ShadowAdapterLimitHandle>,
    mut published_revision: Local<u64>,
) {
    let revision = budget.revision();
    if revision == *published_revision {
        return;
    }

    let value = GpuShadowAdapterLimit {
        limit_bytes: budget.limit_bytes(),
    };
    if let Some(mut current) = main_world.get_resource_mut::<GpuShadowAdapterLimit>() {
        *current = value;
    } else {
        main_world.insert_resource(value);
    }
    *published_revision = revision;
}

fn render_quality_changed(
    settings: Res<RenderingQualitySettings>,
    budget: Res<GpuShadowAdapterLimit>,
    directional_map: Res<bevy::light::DirectionalLightShadowMap>,
) -> bool {
    settings.is_changed() || budget.is_changed() || directional_map.is_changed()
}

fn shadow_configuration_signature(
    directionals: &[(Entity, usize)],
    points: &[Entity],
    spots: &[Entity],
) -> u64 {
    let mut signature = 0xcbf29ce484222325_u64;
    for (entity, cascades) in directionals {
        signature ^= entity.to_bits();
        signature = signature.wrapping_mul(0x100000001b3);
        signature ^= *cascades as u64;
        signature = signature.wrapping_mul(0x100000001b3);
    }
    for (entity, class) in points.iter().zip(std::iter::repeat(1_u64)) {
        signature ^= entity.to_bits();
        signature = signature.wrapping_mul(0x100000001b3);
        signature ^= class;
        signature = signature.wrapping_mul(0x100000001b3);
    }
    for (entity, class) in spots.iter().zip(std::iter::repeat(2_u64)) {
        signature ^= entity.to_bits();
        signature = signature.wrapping_mul(0x100000001b3);
        signature ^= class;
        signature = signature.wrapping_mul(0x100000001b3);
    }
    signature
}

fn shadow_policy_signature(profile: lunco_render::RenderQualityProfile) -> u64 {
    let mut signature = profile.directional_shadow_map_size as u64;
    signature = signature
        .wrapping_mul(0x100000001b3)
        .wrapping_add(profile.point_shadow_map_size as u64);
    signature = signature
        .wrapping_mul(0x100000001b3)
        .wrapping_add(profile.directional_cascades as u64);
    signature = signature
        .wrapping_mul(0x100000001b3)
        .wrapping_add(profile.max_directional_shadow_casters as u64);
    signature = signature
        .wrapping_mul(0x100000001b3)
        .wrapping_add(profile.max_point_shadow_casters as u64);
    signature = signature
        .wrapping_mul(0x100000001b3)
        .wrapping_add(profile.max_spot_shadow_casters as u64);
    signature = signature
        .wrapping_mul(0x100000001b3)
        .wrapping_add(profile.shadow_budget_bytes);
    signature = signature
        .wrapping_mul(0x100000001b3)
        .wrapping_add(profile.shadow_depth_bias.to_bits() as u64);
    signature
        .wrapping_mul(0x100000001b3)
        .wrapping_add(profile.shadow_normal_bias.to_bits() as u64)
}

/// Project the persisted quality choice onto the live shadow resources.
///
/// This is change-driven: settings, the adapter budget, or a newly authored
/// directional-light map must change before it runs. The selected settings are
/// applied exactly as authored. The adapter budget is only a safety ceiling for
/// caster admission; it never selects a lower preset or rewrites map/cascade
/// values. Changing a setting is also the explicit, safe re-arm after the
/// reactive error ladder has shed shadows.
fn apply_render_quality(
    settings: Res<RenderingQualitySettings>,
    budget: Res<GpuShadowAdapterLimit>,
    mut directional_map: ResMut<bevy::light::DirectionalLightShadowMap>,
    mut point_map: ResMut<bevy::light::PointLightShadowMap>,
    mut directional_lights: Query<(
        Entity,
        &mut bevy::light::DirectionalLight,
        &mut bevy::light::CascadeShadowConfig,
        Option<&ShadowMapSuppressed>,
    )>,
    mut point_lights: Query<(
        Entity,
        &mut bevy::light::PointLight,
        Option<&ShadowMapSuppressed>,
    )>,
    mut spot_lights: Query<(
        Entity,
        &mut bevy::light::SpotLight,
        Option<&ShadowMapSuppressed>,
    )>,
    mut ladder: ResMut<Ladder>,
    health: Option<Res<RenderHealthHandle>>,
    warning: Option<Res<RenderWarning>>,
    mut commands: Commands,
) {
    let explicit_rearm = settings.is_changed() && ladder.rung == Rung::ShadowMapsOff;
    if explicit_rearm {
        let total = health.as_ref().map_or(0, |handle| handle.0.total());
        ladder.rearm(total);
        restore_suppressed_shadow_maps(
            &mut commands,
            &mut directional_lights,
            &mut point_lights,
            &mut spot_lights,
        );
        // The warning was produced by the rung being re-armed. A subsequent
        // budget cap is reported in the Graphics settings row instead.
        commands.remove_resource::<RenderWarning>();
    }

    let profile = settings.profile();

    if let Err(reason) = settings.validate() {
        if warning.is_none() {
            commands.insert_resource(RenderWarning {
                message: format!(
                    "Invalid graphics shadow settings: {reason}. The requested settings were not applied until corrected."
                ),
            });
        }
        return;
    }

    if directional_map.size != profile.directional_shadow_map_size as usize {
        directional_map.size = profile.directional_shadow_map_size as usize;
    }
    if point_map.size != profile.point_shadow_map_size as usize {
        point_map.size = profile.point_shadow_map_size as usize;
    }

    for (_, _, mut config, _) in &mut directional_lights {
        if config.bounds.len() == profile.directional_cascades {
            continue;
        }
        let Some(maximum_distance) = config.bounds.last().copied() else {
            warn!("directional light has no authored cascade bounds; preserving its invalid shadow configuration");
            continue;
        };
        let maximum_distance = maximum_distance.max(config.minimum_distance + f32::EPSILON);
        let first_cascade_far_bound = config
            .bounds
            .first()
            .copied()
            .unwrap_or(maximum_distance)
            .clamp(config.minimum_distance + f32::EPSILON, maximum_distance);
        *config = bevy::light::CascadeShadowConfigBuilder {
            num_cascades: profile.directional_cascades,
            minimum_distance: config.minimum_distance,
            first_cascade_far_bound,
            maximum_distance,
            overlap_proportion: config.overlap_proportion,
        }
        .build();
    }

    if profile.shadow_budget_bytes > budget.limit_bytes && warning.is_none() {
        warn!(
            "configured shadow byte ceiling {} MiB exceeds the adapter safety ceiling of {} MiB; requested quality remains unchanged and caster admission will use the lower ceiling",
            profile.shadow_budget_bytes / (1024 * 1024),
            budget.limit_bytes / (1024 * 1024),
        );
    }
}

fn restore_suppressed_shadow_maps(
    commands: &mut Commands,
    directional_lights: &mut Query<(
        Entity,
        &mut bevy::light::DirectionalLight,
        &mut bevy::light::CascadeShadowConfig,
        Option<&ShadowMapSuppressed>,
    )>,
    point_lights: &mut Query<(
        Entity,
        &mut bevy::light::PointLight,
        Option<&ShadowMapSuppressed>,
    )>,
    spot_lights: &mut Query<(
        Entity,
        &mut bevy::light::SpotLight,
        Option<&ShadowMapSuppressed>,
    )>,
) {
    for (entity, mut light, _, suppressed) in directional_lights.iter_mut() {
        if let Some(suppressed) = suppressed {
            light.shadow_maps_enabled = suppressed.was_enabled;
            commands.entity(entity).remove::<ShadowMapSuppressed>();
        }
    }
    for (entity, mut light, suppressed) in point_lights.iter_mut() {
        if let Some(suppressed) = suppressed {
            light.shadow_maps_enabled = suppressed.was_enabled;
            commands.entity(entity).remove::<ShadowMapSuppressed>();
        }
    }
    for (entity, mut light, suppressed) in spot_lights.iter_mut() {
        if let Some(suppressed) = suppressed {
            light.shadow_maps_enabled = suppressed.was_enabled;
            commands.entity(entity).remove::<ShadowMapSuppressed>();
        }
    }
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
    // no access to the world, and adapter identity is exactly what triage needs.
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

/// Apply the explicitly configured shadow-admission policy before Bevy's PBR
/// render preparation allocates its depth textures. The configured byte ceiling
/// is combined with the adapter safety ceiling for admission only. The user's
/// map sizes and cascade count remain unchanged; only casters beyond the
/// explicitly configured class limits or byte ceiling are suppressed and
/// reported.
///
/// The selection is deliberately stable: directional casters are considered
/// first, then point lights, then spot lights, and each class is ordered by
/// Bevy's stable [`Entity`] key. There is no camera-distance heuristic whose
/// result can change merely because the viewer moved. The shared byte estimate
/// is the admission test for every individual caster, so the resulting set is
/// guaranteed to fit the published budget rather than merely fit three
/// unrelated per-class caps.
fn apply_shadow_budget_policy(
    mut state: ResMut<ShadowAdmissionState>,
    mut commands: Commands,
    settings: Res<RenderingQualitySettings>,
    budget: Res<GpuShadowAdapterLimit>,
    warning: Option<Res<RenderWarning>>,
    directional_shadow_map: Res<bevy::light::DirectionalLightShadowMap>,
    point_shadow_map: Res<bevy::light::PointLightShadowMap>,
    mut directionals: Query<(
        Entity,
        &mut bevy::light::DirectionalLight,
        &bevy::light::CascadeShadowConfig,
        Option<&ShadowMapSuppressed>,
    )>,
    mut points: Query<(
        Entity,
        &mut bevy::light::PointLight,
        Option<&ShadowMapSuppressed>,
    )>,
    mut spots: Query<(
        Entity,
        &mut bevy::light::SpotLight,
        Option<&ShadowMapSuppressed>,
    )>,
) {
    let profile = settings.profile();
    if settings.validate().is_err() {
        return;
    }
    let admission_budget = profile.shadow_budget_bytes.min(budget.limit_bytes);
    let policy_signature = shadow_policy_signature(profile);

    let admission_changed = state.policy_signature != Some(policy_signature)
        || state.budget_bytes != Some(admission_budget);
    if admission_changed {
        for (entity, mut light, _, suppressed) in directionals.iter_mut() {
            if suppressed.is_some_and(|s| s.reason == ShadowMapSuppressionReason::Budget) {
                light.shadow_maps_enabled = suppressed.is_some_and(|s| s.was_enabled);
                commands.entity(entity).remove::<ShadowMapSuppressed>();
            }
        }
        for (entity, mut light, suppressed) in points.iter_mut() {
            if suppressed.is_some_and(|s| s.reason == ShadowMapSuppressionReason::Budget) {
                light.shadow_maps_enabled = suppressed.is_some_and(|s| s.was_enabled);
                commands.entity(entity).remove::<ShadowMapSuppressed>();
            }
        }
        for (entity, mut light, suppressed) in spots.iter_mut() {
            if suppressed.is_some_and(|s| s.reason == ShadowMapSuppressionReason::Budget) {
                light.shadow_maps_enabled = suppressed.is_some_and(|s| s.was_enabled);
                commands.entity(entity).remove::<ShadowMapSuppressed>();
            }
        }
    }

    let mut directional_entities: Vec<(Entity, usize)> = directionals
        .iter_mut()
        .filter_map(|(entity, light, config, _)| {
            light
                .shadow_maps_enabled
                .then_some((entity, config.bounds.len().max(1)))
        })
        .collect();
    directional_entities.sort_by_key(|(entity, _)| *entity);

    let mut point_entities: Vec<Entity> = points
        .iter_mut()
        .filter_map(|(entity, light, _)| light.shadow_maps_enabled.then_some(entity))
        .collect();
    point_entities.sort();

    let mut spot_entities: Vec<Entity> = spots
        .iter_mut()
        .filter_map(|(entity, light, _)| light.shadow_maps_enabled.then_some(entity))
        .collect();
    spot_entities.sort();

    let light_count = directionals.iter().count() + points.iter().count() + spots.iter().count();
    let enabled_caster_count =
        directional_entities.len() + point_entities.len() + spot_entities.len();
    let configuration_signature =
        shadow_configuration_signature(&directional_entities, &point_entities, &spot_entities);
    if state.light_count == Some(light_count)
        && state.enabled_caster_count == Some(enabled_caster_count)
        && state.directional_map_size == Some(directional_shadow_map.size)
        && state.point_map_size == Some(point_shadow_map.size)
        && state.directional_cascade_layers
            == Some(
                directional_entities
                    .iter()
                    .map(|(_, cascades)| *cascades)
                    .sum(),
            )
        && state.budget_bytes == Some(admission_budget)
        && state
            .configuration_signature
            .is_some_and(|signature| signature == configuration_signature)
        && state.policy_signature == Some(policy_signature)
    {
        return;
    }

    if light_count == 0 {
        state.light_count = Some(0);
        state.enabled_caster_count = Some(0);
        state.directional_map_size = Some(directional_shadow_map.size);
        state.point_map_size = Some(point_shadow_map.size);
        state.directional_cascade_layers = Some(0);
        state.budget_bytes = Some(admission_budget);
        state.configuration_signature = Some(configuration_signature);
        state.policy_signature = Some(policy_signature);
        return;
    }

    let mut used_bytes = 0_u64;
    let mut kept_directionals = Vec::new();
    let mut kept_points = Vec::new();
    let mut kept_spots = Vec::new();

    for (index, (entity, cascades)) in directional_entities.iter().enumerate() {
        let cost = estimate_shadow_allocation_bytes(
            directional_shadow_map.size,
            point_shadow_map.size,
            *cascades,
            1,
            0,
            0,
        );
        let keep = index < profile.max_directional_shadow_casters
            && cost <= admission_budget.saturating_sub(used_bytes);
        if keep {
            used_bytes = used_bytes.saturating_add(cost);
            kept_directionals.push(*entity);
            if let Ok((_, _, _, suppressed)) = directionals.get_mut(*entity) {
                if suppressed.is_some_and(|s| s.reason == ShadowMapSuppressionReason::Budget) {
                    commands.entity(*entity).remove::<ShadowMapSuppressed>();
                }
            }
        } else if let Ok((_, mut light, _, suppressed)) = directionals.get_mut(*entity) {
            if suppressed.is_none() {
                commands.entity(*entity).try_insert(ShadowMapSuppressed {
                    was_enabled: true,
                    reason: ShadowMapSuppressionReason::Budget,
                });
            }
            light.shadow_maps_enabled = false;
        }
    }

    for (index, entity) in point_entities.iter().enumerate() {
        let cost = estimate_shadow_allocation_bytes(
            directional_shadow_map.size,
            point_shadow_map.size,
            0,
            0,
            1,
            0,
        );
        let keep = index < profile.max_point_shadow_casters
            && cost <= admission_budget.saturating_sub(used_bytes);
        if keep {
            used_bytes = used_bytes.saturating_add(cost);
            kept_points.push(*entity);
            if let Ok((_, _, suppressed)) = points.get_mut(*entity) {
                if suppressed.is_some_and(|s| s.reason == ShadowMapSuppressionReason::Budget) {
                    commands.entity(*entity).remove::<ShadowMapSuppressed>();
                }
            }
        } else if let Ok((_, mut light, suppressed)) = points.get_mut(*entity) {
            if suppressed.is_none() {
                commands.entity(*entity).try_insert(ShadowMapSuppressed {
                    was_enabled: true,
                    reason: ShadowMapSuppressionReason::Budget,
                });
            }
            light.shadow_maps_enabled = false;
        }
    }

    for (index, entity) in spot_entities.iter().enumerate() {
        let cost = estimate_shadow_allocation_bytes(
            directional_shadow_map.size,
            point_shadow_map.size,
            0,
            0,
            0,
            1,
        );
        let keep = index < profile.max_spot_shadow_casters
            && cost <= admission_budget.saturating_sub(used_bytes);
        if keep {
            used_bytes = used_bytes.saturating_add(cost);
            kept_spots.push(*entity);
            if let Ok((_, _, suppressed)) = spots.get_mut(*entity) {
                if suppressed.is_some_and(|s| s.reason == ShadowMapSuppressionReason::Budget) {
                    commands.entity(*entity).remove::<ShadowMapSuppressed>();
                }
            }
        } else if let Ok((_, mut light, suppressed)) = spots.get_mut(*entity) {
            if suppressed.is_none() {
                commands.entity(*entity).try_insert(ShadowMapSuppressed {
                    was_enabled: true,
                    reason: ShadowMapSuppressionReason::Budget,
                });
            }
            light.shadow_maps_enabled = false;
        }
    }

    let directional_layers = directional_entities
        .iter()
        .filter(|(entity, _)| kept_directionals.contains(entity))
        .map(|(_, cascades)| *cascades)
        .sum::<usize>();
    let estimated_mib = used_bytes as f64 / (1024.0 * 1024.0);
    let shed_count = enabled_caster_count
        .saturating_sub(kept_directionals.len() + kept_points.len() + kept_spots.len());
    warn!(
        "shadow budget: {} directional caster(s), {} cascade layer(s), {} point caster(s), {} spot caster(s), estimated allocation {:.1} MiB of {} MiB (directional {}px, point {}px)",
        kept_directionals.len(),
        directional_layers,
        kept_points.len(),
        kept_spots.len(),
        estimated_mib,
        admission_budget / (1024 * 1024),
        directional_shadow_map.size,
        point_shadow_map.size,
    );
    if shed_count > 0 && warning.is_none() {
        commands.insert_resource(RenderWarning {
            message: format!(
                "Shadow budget active: kept {} directional, {} point, and {} spot shadow caster(s) within {} MiB; {} caster(s) are intentionally disabled.",
                kept_directionals.len(),
                kept_points.len(),
                kept_spots.len(),
                admission_budget / (1024 * 1024),
                shed_count,
            ),
        });
    }

    // Cache the post-policy state, not the pre-policy observation. Otherwise a
    // scene with five authored casters (four allowed) would cache `5`; an
    // explicit re-arm would restore the fifth caster, still look like `5`, and
    // incorrectly take the early-return path above. Recording the effective
    // state makes a real re-arm visible without adding another ownership path.
    state.light_count = Some(light_count);
    state.enabled_caster_count =
        Some(kept_directionals.len() + kept_points.len() + kept_spots.len());
    state.directional_map_size = Some(directional_shadow_map.size);
    state.point_map_size = Some(point_shadow_map.size);
    state.directional_cascade_layers = Some(directional_layers);
    state.budget_bytes = Some(admission_budget);
    state.policy_signature = Some(policy_signature);
    state.configuration_signature = Some(shadow_configuration_signature(
        &kept_directionals
            .iter()
            .filter_map(|entity| {
                directional_entities
                    .iter()
                    .find(|(candidate, _)| candidate == entity)
                    .copied()
            })
            .collect::<Vec<_>>(),
        &kept_points,
        &kept_spots,
    ));
}

/// Scene teardown is the explicit re-arm boundary for presentation recovery.
/// A new scene gets fresh lights and cameras, so clearing the old ladder and
/// shadow-budget bookkeeping is safe. A lost device remains terminal because
/// no scene reload can recreate the adapter in-process.
pub(crate) fn reset_render_recovery(
    health: Res<RenderHealthHandle>,
    mut ladder: ResMut<Ladder>,
    mut budget: ResMut<ShadowAdmissionState>,
    mut presentation: ResMut<PresentationState>,
    mut commands: Commands,
    mut directional_lights: Query<(
        Entity,
        &mut bevy::light::DirectionalLight,
        &mut bevy::light::CascadeShadowConfig,
        Option<&ShadowMapSuppressed>,
    )>,
    mut point_lights: Query<(
        Entity,
        &mut bevy::light::PointLight,
        Option<&ShadowMapSuppressed>,
    )>,
    mut spot_lights: Query<(
        Entity,
        &mut bevy::light::SpotLight,
        Option<&ShadowMapSuppressed>,
    )>,
) {
    if health.0.device_lost() {
        return;
    }
    health.0.reset_for_scene();
    *ladder = Ladder::default();
    *budget = ShadowAdmissionState::default();
    restore_suppressed_shadow_maps(
        &mut commands,
        &mut directional_lights,
        &mut point_lights,
        &mut spot_lights,
    );
    // The render schedule is gated by this extracted state, not by camera
    // activation.  Clearing only the ladder would leave a successfully
    // reloaded scene permanently headless after the previous scene gave up.
    presentation.stopped = false;
    commands.remove_resource::<RenderWarning>();
    commands.remove_resource::<RenderGaveUp>();
}

/// Main-world escalation: read the shared tallies, advance the [`Ladder`], apply
/// whatever it decided, and publish the cross-world presentation gate.
///
/// In the main world rather than the render world because both remedies are
/// main-world state — `DirectionalLight::shadow_maps_enabled` and
/// `Camera::is_active` are extracted to the render world each frame. The
/// terminal presentation gate additionally stops the render schedule itself.
fn escalate_render_recovery(
    health: Res<RenderHealthHandle>,
    mut ladder: ResMut<Ladder>,
    time: Res<Time>,
    mut presentation: ResMut<PresentationState>,
    mut commands: Commands,
    warning: Option<Res<RenderWarning>>,
    mut dir: Query<(
        Entity,
        &mut bevy::light::DirectionalLight,
        Option<&ShadowMapSuppressed>,
    )>,
    mut point: Query<(
        Entity,
        &mut bevy::light::PointLight,
        Option<&ShadowMapSuppressed>,
    )>,
    mut spot: Query<(
        Entity,
        &mut bevy::light::SpotLight,
        Option<&ShadowMapSuppressed>,
    )>,
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
            for (entity, mut l, suppressed) in &mut dir {
                if suppressed.is_none() {
                    commands.entity(entity).try_insert(ShadowMapSuppressed {
                        was_enabled: l.shadow_maps_enabled,
                        reason: ShadowMapSuppressionReason::Recovery,
                    });
                }
                l.shadow_maps_enabled = false;
                n += 1;
            }
            for (entity, mut l, suppressed) in &mut point {
                if suppressed.is_none() {
                    commands.entity(entity).try_insert(ShadowMapSuppressed {
                        was_enabled: l.shadow_maps_enabled,
                        reason: ShadowMapSuppressionReason::Recovery,
                    });
                }
                l.shadow_maps_enabled = false;
                n += 1;
            }
            for (entity, mut l, suppressed) in &mut spot {
                if suppressed.is_none() {
                    commands.entity(entity).try_insert(ShadowMapSuppressed {
                        was_enabled: l.shadow_maps_enabled,
                        reason: ShadowMapSuppressionReason::Recovery,
                    });
                }
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
                message: "Rendering recovered with shadow maps disabled. Change Rendering quality in Settings to safely re-arm the shadow allocation.".to_string(),
            });
        }
        Action::GiveUp => {
            h.presentation_stopped.store(true, Ordering::Relaxed);
            presentation.stopped = true;
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

    #[derive(Resource, Default)]
    struct SubmittedFrames(u32);

    fn submit_frame(mut submitted: ResMut<SubmittedFrames>) {
        submitted.0 += 1;
    }

    #[test]
    fn terminal_presentation_gate_stops_submission_and_reload_rearms_it() {
        let mut schedule = Schedule::new(Render);
        schedule.configure_sets(RenderSystems::Render.run_if(presentation_is_active));
        schedule.add_systems(submit_frame.in_set(RenderSystems::Render));
        let mut world = World::new();
        world.init_resource::<SubmittedFrames>();
        world.insert_resource(PresentationState::default());

        schedule.run(&mut world);
        assert_eq!(world.resource::<SubmittedFrames>().0, 1);
        world.resource_mut::<PresentationState>().stopped = true;
        schedule.run(&mut world);
        assert_eq!(world.resource::<SubmittedFrames>().0, 1);

        let health = Arc::new(RenderHealth::default());
        health.presentation_stopped.store(true, Ordering::Relaxed);
        world.insert_resource(RenderHealthHandle(health.clone()));
        world.init_resource::<Ladder>();
        world.init_resource::<ShadowAdmissionState>();
        world.insert_resource(RenderGaveUp {
            reason: "test fault".into(),
        });
        world.insert_resource(RenderWarning {
            message: "test warning".into(),
        });
        let light = world
            .spawn((
                bevy::light::PointLight {
                    shadow_maps_enabled: false,
                    ..default()
                },
                ShadowMapSuppressed {
                    was_enabled: true,
                    reason: ShadowMapSuppressionReason::Recovery,
                },
            ))
            .id();
        let mut reset = Schedule::new(Update);
        reset.add_systems(reset_render_recovery);
        reset.run(&mut world);
        assert!(!world.resource::<PresentationState>().stopped);
        assert!(!health.presentation_stopped.load(Ordering::Relaxed));
        assert!(world.get_resource::<RenderGaveUp>().is_none());
        assert!(world.get_resource::<RenderWarning>().is_none());
        assert!(
            world
                .get::<bevy::light::PointLight>(light)
                .unwrap()
                .shadow_maps_enabled
        );
        assert!(world.get::<ShadowMapSuppressed>(light).is_none());
    }

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
    fn configured_shadow_budget_limits_casters_without_rewriting_quality() {
        let mut app = App::new();
        let mut settings = RenderingQualitySettings::default();
        settings.directional_shadow_map_size = 1024;
        settings.point_shadow_map_size = 512;
        settings.max_point_shadow_casters = 4;
        settings.shadow_budget_bytes = 16 * 1024 * 1024;
        app.insert_resource(settings);
        app.insert_resource(GpuShadowAdapterLimit {
            limit_bytes: 16 * 1024 * 1024,
        });
        app.init_resource::<ShadowAdmissionState>();
        app.insert_resource(bevy::light::DirectionalLightShadowMap { size: 1024 });
        app.insert_resource(bevy::light::PointLightShadowMap { size: 512 });
        app.add_systems(PostUpdate, apply_shadow_budget_policy);
        for _ in 0..5 {
            app.world_mut().spawn((bevy::light::PointLight {
                shadow_maps_enabled: true,
                ..default()
            },));
        }

        app.update();

        assert_eq!(
            app.world()
                .resource::<bevy::light::DirectionalLightShadowMap>()
                .size,
            1024
        );
        assert_eq!(
            app.world()
                .resource::<bevy::light::PointLightShadowMap>()
                .size,
            512
        );
        let world = app.world_mut();
        let mut query = world.query::<&bevy::light::PointLight>();
        let enabled = query
            .iter(world)
            .filter(|light| light.shadow_maps_enabled)
            .count();
        assert_eq!(enabled, 1);
        assert!(estimate_shadow_allocation_bytes(1024, 512, 0, 0, enabled, 0) <= 16 * 1024 * 1024);
        assert!(app.world().get_resource::<RenderWarning>().is_some());
    }

    #[test]
    fn configured_shadow_budget_rechecks_when_a_caster_is_rearmed() {
        let mut app = App::new();
        let mut settings = RenderingQualitySettings::default();
        settings.directional_shadow_map_size = 1024;
        settings.point_shadow_map_size = 512;
        settings.max_point_shadow_casters = 4;
        settings.shadow_budget_bytes = 16 * 1024 * 1024;
        app.insert_resource(settings);
        app.insert_resource(GpuShadowAdapterLimit {
            limit_bytes: 16 * 1024 * 1024,
        });
        app.init_resource::<ShadowAdmissionState>();
        app.insert_resource(bevy::light::DirectionalLightShadowMap { size: 1024 });
        app.insert_resource(bevy::light::PointLightShadowMap { size: 512 });
        app.add_systems(PostUpdate, apply_shadow_budget_policy);
        for _ in 0..2 {
            app.world_mut().spawn((bevy::light::PointLight {
                shadow_maps_enabled: true,
                ..default()
            },));
        }

        app.update();
        let disabled = {
            let mut query = app
                .world_mut()
                .query::<(Entity, &bevy::light::PointLight)>();
            query
                .iter(app.world())
                .find_map(|(entity, light)| (!light.shadow_maps_enabled).then_some(entity))
                .expect("budget must shed one caster")
        };
        app.world_mut()
            .get_mut::<bevy::light::PointLight>(disabled)
            .unwrap()
            .shadow_maps_enabled = true;

        app.update();

        let enabled = {
            let mut query = app.world_mut().query::<&bevy::light::PointLight>();
            query
                .iter(app.world())
                .filter(|light| light.shadow_maps_enabled)
                .count()
        };
        assert_eq!(enabled, 1);
    }

    #[test]
    fn adapter_limit_increase_rearms_only_budget_suppressed_casters() {
        let mut app = App::new();
        let mut settings = RenderingQualitySettings::default();
        settings.directional_shadow_map_size = 1024;
        settings.point_shadow_map_size = 512;
        settings.max_point_shadow_casters = 4;
        settings.shadow_budget_bytes = 64 * 1024 * 1024;
        app.insert_resource(settings);
        app.insert_resource(GpuShadowAdapterLimit {
            limit_bytes: 16 * 1024 * 1024,
        });
        app.init_resource::<ShadowAdmissionState>();
        app.insert_resource(bevy::light::DirectionalLightShadowMap { size: 1024 });
        app.insert_resource(bevy::light::PointLightShadowMap { size: 512 });
        app.add_systems(PostUpdate, apply_shadow_budget_policy);
        for _ in 0..4 {
            app.world_mut().spawn((bevy::light::PointLight {
                shadow_maps_enabled: true,
                ..default()
            },));
        }

        app.update();
        let initially_enabled = {
            let mut query = app.world_mut().query::<&bevy::light::PointLight>();
            query
                .iter(app.world())
                .filter(|light| light.shadow_maps_enabled)
                .count()
        };
        assert_eq!(initially_enabled, 1);

        app.world_mut()
            .resource_mut::<GpuShadowAdapterLimit>()
            .limit_bytes = 64 * 1024 * 1024;
        app.update();

        let enabled = {
            let mut query = app.world_mut().query::<&bevy::light::PointLight>();
            query
                .iter(app.world())
                .filter(|light| light.shadow_maps_enabled)
                .count()
        };
        assert_eq!(enabled, 4);
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

    #[test]
    fn an_explicit_quality_change_rearms_only_shadow_degradation() {
        let mut l = Ladder::default();
        assert_eq!(
            l.step(1, FailureKind::ShadowMap, false, 0.0),
            Some(Action::DisableShadowMaps)
        );
        l.rearm(1);
        assert_eq!(l.rung, Rung::Healthy);
        // The old error total is the new baseline; a clean frame does not
        // immediately trip the ladder again.
        assert_eq!(l.step(1, FailureKind::ShadowMap, false, 1.0), None);

        let mut persistent = Ladder::default();
        persistent.step(1, FailureKind::OutOfMemory, false, 0.0);
        persistent.rearm(1);
        assert_eq!(persistent.rung, Rung::PersistentFailure);
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
