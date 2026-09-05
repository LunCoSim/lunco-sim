//! Render-backend robustness: keep the app alive through transient GPU
//! validation errors and stop presentation deliberately when they stop being
//! transient.
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
//! So a repeated error escalates through [`Ladder`]:
//!
//! | rung | when | what |
//! |---|---|---|
//! | `Healthy` | — | drop the bad frame, log rate-limited (transient skew) |
//! | `PersistentFailure` | first render failure | preserve the authored and configured scene state and wait for a bounded give-up decision |
//! | `GaveUp` | still failing for the configured presentation grace period, or device lost | deactivate every camera, log once, loudly |
//!
//! `GaveUp` stops the null loop rather than the process: the sim, the API and
//! the document model are all still healthy and worth keeping alive — it is only
//! *presentation* that is dead. Exiting would throw away a working session to
//! report a rendering fault.
//!
//! The ladder is a pure state machine precisely so it can be tested without a
//! GPU (see the tests at the bottom); the systems around it only apply what it
//! decides. No quality setting is changed by this ladder. The user can choose
//! a different explicit graphics configuration, or reload the scene, after a
//! presentation failure.
//!
//! The one further mitigation, independent of the ladder, is
//! [`install_wgpu_error_handler`], which replaces wgpu's default panic-on-uncaptured
//!   -error with a logging handler, so the render system no longer unwinds
//!   mid-frame and panic (2) is avoided.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::{
    extract_resource::{ExtractResource, ExtractResourcePlugin},
    renderer::{RenderAdapterInfo, RenderDevice},
    Render, RenderApp, RenderStartup, RenderSystems,
};
use bevy_egui::{egui, EguiContexts};
use lunco_render::{
    estimate_shadow_allocation_bytes, LightGraphicsDefaults, RenderingQualitySettings,
    ShadowRangeAuthorship,
};
use lunco_settings::AppSettingsExt;

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
    /// Recorded to make the log identify the failed resource class.
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
    /// Conservative pre-extraction estimate for the currently enabled shadow
    /// resources. The wgpu callback cannot inspect the ECS world, so the render
    /// boundary publishes this here for actionable OOM logs.
    shadow_estimated_bytes: AtomicU64,
    /// The explicit graphics ceiling paired with `shadow_estimated_bytes`.
    shadow_budget_bytes: AtomicU64,
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
        self.shadow_estimated_bytes.store(0, Ordering::Relaxed);
        self.shadow_budget_bytes.store(0, Ordering::Relaxed);
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

/// Values discovered from the actual render device. They are published from
/// the render world because the main world must not guess an adapter's limits.
#[derive(Debug, Default)]
struct RenderCapabilityShared {
    ready: AtomicBool,
    max_texture_dimension_2d: AtomicU32,
    max_texture_array_layers: AtomicU32,
}

/// Main/render-world bridge for the adapter limits used by graphics-settings
/// admission. A settings request is held until these values are known; it is
/// never replaced with a lower preset while waiting for them.
#[derive(Resource, Clone, Debug)]
pub(crate) struct RenderCapabilitiesHandle(Arc<RenderCapabilityShared>);

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RenderCapabilities {
    ready: bool,
    max_texture_dimension_2d: u32,
    max_texture_array_layers: u32,
}

impl RenderCapabilities {
    pub(crate) fn is_ready(self) -> bool {
        self.ready
    }

    /// Return every shadow-map size accepted by the live adapter and the
    /// persisted graphics-settings contract.
    pub(crate) fn supported_shadow_map_sizes(self) -> Option<Vec<u32>> {
        if !self.ready || self.max_texture_dimension_2d == 0 {
            return None;
        }

        let mut sizes = Vec::new();
        let mut size: u32 = 1;
        loop {
            sizes.push(size);
            let Some(next) = size.checked_mul(2) else {
                break;
            };
            if next > self.max_texture_dimension_2d {
                break;
            }
            size = next;
        }
        Some(sizes)
    }
}

fn publish_render_capabilities(
    device: Res<RenderDevice>,
    capabilities: Res<RenderCapabilitiesHandle>,
) {
    let limits = device.limits();
    capabilities
        .0
        .max_texture_dimension_2d
        .store(limits.max_texture_dimension_2d, Ordering::Release);
    capabilities
        .0
        .max_texture_array_layers
        .store(limits.max_texture_array_layers, Ordering::Release);
    capabilities.0.ready.store(true, Ordering::Release);
}

fn poll_render_capabilities(
    capabilities: Res<RenderCapabilitiesHandle>,
    mut state: ResMut<RenderCapabilities>,
) {
    let next = RenderCapabilities {
        ready: capabilities.0.ready.load(Ordering::Acquire),
        max_texture_dimension_2d: capabilities
            .0
            .max_texture_dimension_2d
            .load(Ordering::Acquire),
        max_texture_array_layers: capabilities
            .0
            .max_texture_array_layers
            .load(Ordering::Acquire),
    };
    if *state != next {
        *state = next;
    }
}

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

/// Why a persistent presentation warning is being shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderWarningKind {
    /// The requested graphics configuration is invalid or unsupported by the
    /// current adapter and therefore was not applied.
    GraphicsSettings,
    /// The renderer is receiving repeated GPU failures.
    RuntimeFailure,
}

impl RenderWarningKind {
    fn title(self) -> &'static str {
        match self {
            Self::GraphicsSettings => "GRAPHICS SETTINGS",
            Self::RuntimeFailure => "RENDERING DEGRADED",
        }
    }
}

/// Persistent presentation warning shown while the simulation is still alive.
#[derive(Resource, Clone, Debug)]
pub struct RenderWarning {
    /// The owner/category of the condition shown in the workbench overlay.
    pub kind: RenderWarningKind,
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
    let (icon, title, message, color, fill) = if let Some(gave_up) = gave_up {
        (
            crate::UiIcon::Error,
            "PRESENTATION STOPPED",
            gave_up.reason.clone(),
            theme.tokens.error,
            theme.tokens.alert_backdrop,
        )
    } else if let Some(warning) = warning {
        (
            crate::UiIcon::Warning,
            warning.kind.title(),
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
                        ui.horizontal(|ui| {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                            crate::paint_icon(ui.painter(), icon, rect, color);
                            ui.label(egui::RichText::new(title).color(color).strong());
                        });
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
    PersistentFailure,
    GaveUp,
}

/// What the ladder decided to do this frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Action {
    GiveUp,
}

/// The escalation state machine — pure, so it is testable without a GPU.
#[derive(Resource, Debug)]
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
    /// Configured quiet period for deciding that callbacks have stopped.
    failure_quiet_period_secs: f64,
    /// Configured wall-clock grace period before presentation is stopped.
    failure_give_up_after_secs: f64,
}

impl Default for Ladder {
    fn default() -> Self {
        let profile = RenderingQualitySettings::default().profile();
        Self {
            rung: Rung::default(),
            last_total: 0,
            failing_since: None,
            last_failure_at: None,
            failure_kind: FailureKind::default(),
            failure_quiet_period_secs: profile.render_failure_quiet_period_secs,
            failure_give_up_after_secs: profile.render_failure_give_up_after_secs,
        }
    }
}

/// Tracks the scene structure for shadow-resource diagnostics. A scene loads
/// asynchronously, so the policy is re-applied whenever another light entity
/// materialises, then remains dormant until the next teardown.
#[derive(Resource, Default)]
pub(crate) struct ShadowAdmissionState {
    light_count: Option<usize>,
    enabled_directional_casters: Option<usize>,
    enabled_point_casters: Option<usize>,
    enabled_spot_casters: Option<usize>,
    directional_map_size: Option<usize>,
    point_map_size: Option<usize>,
    directional_cascade_layers: Option<usize>,
    budget_bytes: Option<u64>,
    policy_signature: Option<u64>,
    configured_limit_status_active: bool,
}

impl Ladder {
    fn set_recovery_policy(&mut self, profile: lunco_render::RenderQualityProfile) {
        self.failure_quiet_period_secs = profile.render_failure_quiet_period_secs;
        self.failure_give_up_after_secs = profile.render_failure_give_up_after_secs;
    }

    fn reset_state(&mut self) {
        self.rung = Rung::Healthy;
        self.last_total = 0;
        self.failing_since = None;
        self.last_failure_at = None;
        self.failure_kind = FailureKind::Other;
    }

    /// Re-arm after an explicit user quality change. Device loss remains
    /// terminal because changing a setting cannot recreate the device.
    fn rearm(&mut self, total: u64, device_lost: bool) {
        if device_lost || self.rung != Rung::PersistentFailure {
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
        // Device loss short-circuits every rung: no quality setting can help
        // when there is no device to render with.
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
                .is_some_and(|last| now - last >= self.failure_quiet_period_secs)
            {
                self.failing_since = None;
                self.last_failure_at = None;
                self.failure_kind = FailureKind::Other;
            }
            return None;
        }

        self.last_failure_at = Some(now);
        self.failure_kind = kind;
        let since = *self.failing_since.get_or_insert(now);

        match self.rung {
            Rung::Healthy => {
                self.rung = Rung::PersistentFailure;
                self.failing_since = Some(now);
                None
            }
            Rung::PersistentFailure if now - since >= self.failure_give_up_after_secs => {
                self.rung = Rung::GaveUp;
                Some(Action::GiveUp)
            }
            Rung::PersistentFailure | Rung::GaveUp => None,
        }
    }
}

/// Install the error handler, the device-lost callback and the escalation ladder.
///
/// No-op when there is no [`RenderApp`] (headless tests / API-only servers).
pub(crate) fn install_wgpu_error_handler(app: &mut App) {
    app.register_settings_section::<RenderingQualitySettings>();

    if app.get_sub_app_mut(RenderApp).is_none() {
        return;
    }

    let health = Arc::new(RenderHealth::default());
    app.insert_resource(RenderHealthHandle(health.clone()));
    let capabilities = Arc::new(RenderCapabilityShared::default());
    app.insert_resource(RenderCapabilitiesHandle(capabilities.clone()));
    app.init_resource::<RenderCapabilities>();
    app.init_resource::<Ladder>();
    app.init_resource::<PresentationState>();
    app.add_plugins(ExtractResourcePlugin::<PresentationState>::default());
    app.add_systems(
        Update,
        (
            poll_render_capabilities,
            escalate_render_recovery,
            apply_render_quality.run_if(render_quality_changed),
        )
            .chain(),
    );
    app.init_resource::<ShadowAdmissionState>();
    // Shadow allocation happens during render extraction. The preflight must
    // observe the fully materialised scene in PostUpdate, after scene-load
    // commands apply but before the render sub-app extracts lights.
    app.add_systems(PostUpdate, apply_shadow_caster_policy);

    let render_app = app.get_sub_app_mut(RenderApp).expect("checked above");
    render_app.insert_resource(RenderHealthHandle(health));
    render_app.insert_resource(RenderCapabilitiesHandle(capabilities));
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
        (set_error_handler, publish_render_capabilities).chain(),
    );
}

fn render_quality_changed(
    settings: Res<RenderingQualitySettings>,
    capabilities: Option<Res<RenderCapabilities>>,
    directional_map: Res<bevy::light::DirectionalLightShadowMap>,
    point_map: Res<bevy::light::PointLightShadowMap>,
    directionals: Query<(), Added<bevy::light::DirectionalLight>>,
    points: Query<(), Added<bevy::light::PointLight>>,
    spots: Query<(), Added<bevy::light::SpotLight>>,
    rects: Query<(), Added<bevy::light::RectLight>>,
) -> bool {
    settings.is_changed()
        || capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.is_changed())
        || directional_map.is_changed()
        || point_map.is_changed()
        || !directionals.is_empty()
        || !points.is_empty()
        || !spots.is_empty()
        || !rects.is_empty()
}

/// Validate settings against the limits that Bevy's PBR light pipeline and
/// the current adapter will actually use. Bevy otherwise truncates light
/// lists/layers in the render world, which would silently change an explicit
/// graphics request or allocate an unsupported point-shadow texture.
pub(crate) fn validate_profile_for_capabilities(
    profile: lunco_render::RenderQualityProfile,
    capabilities: &RenderCapabilities,
) -> Result<(), String> {
    if !capabilities.ready {
        return Err("render-device capabilities are not available yet".to_string());
    }
    if profile.directional_cascades > bevy::pbr::MAX_CASCADES_PER_LIGHT {
        return Err(format!(
            "directional cascade count {} exceeds Bevy's limit of {}",
            profile.directional_cascades,
            bevy::pbr::MAX_CASCADES_PER_LIGHT
        ));
    }
    if profile.max_directional_shadow_casters > bevy::pbr::MAX_DIRECTIONAL_LIGHTS {
        return Err(format!(
            "directional shadow caster limit {} exceeds Bevy's limit of {}",
            profile.max_directional_shadow_casters,
            bevy::pbr::MAX_DIRECTIONAL_LIGHTS
        ));
    }
    if profile.directional_shadow_map_size > capabilities.max_texture_dimension_2d {
        return Err(format!(
            "directional shadow-map size {} exceeds the adapter limit of {}",
            profile.directional_shadow_map_size, capabilities.max_texture_dimension_2d
        ));
    }
    if profile.point_shadow_map_size > capabilities.max_texture_dimension_2d {
        return Err(format!(
            "point shadow-map size {} exceeds the adapter limit of {}",
            profile.point_shadow_map_size, capabilities.max_texture_dimension_2d
        ));
    }

    let max_layers = usize::try_from(capabilities.max_texture_array_layers)
        .map_err(|_| "adapter texture-array layer limit is not representable".to_string())?;
    let point_layers = profile
        .max_point_shadow_casters
        .checked_mul(6)
        .ok_or_else(|| "point shadow caster layer count overflows".to_string())?;
    if point_layers > max_layers {
        return Err(format!(
            "point shadow caster limit requires {} texture-array layers, but the adapter supports {}",
            point_layers, max_layers
        ));
    }
    let directional_layers = profile
        .max_directional_shadow_casters
        .checked_mul(profile.directional_cascades)
        .and_then(|layers| layers.checked_add(profile.max_spot_shadow_casters))
        .ok_or_else(|| "directional shadow layer count overflows".to_string())?;
    if directional_layers > max_layers {
        return Err(format!(
            "directional and spot shadow limits require {} texture-array layers, but the adapter supports {}",
            directional_layers, max_layers
        ));
    }
    Ok(())
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
/// This is change-driven: settings or a newly authored directional-light map
/// must change before it runs. The selected settings are applied exactly as
/// authored. Changing a setting is also the explicit re-arm for a pending
/// persistent-failure timer.
fn apply_render_quality(
    settings: Res<RenderingQualitySettings>,
    capabilities: Option<Res<RenderCapabilities>>,
    mut directional_map: ResMut<bevy::light::DirectionalLightShadowMap>,
    mut point_map: ResMut<bevy::light::PointLightShadowMap>,
    mut directional_lights: Query<(
        &mut bevy::light::DirectionalLight,
        &mut bevy::light::CascadeShadowConfig,
        Option<&ShadowRangeAuthorship>,
        Option<&LightGraphicsDefaults>,
    )>,
    mut point_lights: Query<(&mut bevy::light::PointLight, Option<&LightGraphicsDefaults>)>,
    mut spot_lights: Query<(&mut bevy::light::SpotLight, Option<&LightGraphicsDefaults>)>,
    mut rect_lights: Query<(&mut bevy::light::RectLight, Option<&LightGraphicsDefaults>)>,
    mut ladder: ResMut<Ladder>,
    health: Option<Res<RenderHealthHandle>>,
    warning: Option<Res<RenderWarning>>,
    mut commands: Commands,
) {
    if settings.is_changed() {
        let total = health.as_ref().map_or(0, |handle| handle.0.total());
        let device_lost = health.as_ref().is_some_and(|handle| handle.0.device_lost());
        ladder.rearm(total, device_lost);
        commands.remove_resource::<RenderWarning>();
    }

    let profile = match settings.validated_profile() {
        Ok(profile) => profile,
        Err(reason) => {
            if warning.is_none() {
                commands.insert_resource(RenderWarning {
                    kind: RenderWarningKind::GraphicsSettings,
                    message: format!(
                        "Invalid graphics shadow settings: {reason}. The requested settings were not applied until corrected."
                    ),
                });
            }
            return;
        }
    };
    ladder.set_recovery_policy(profile);

    if let Some(capabilities) = capabilities {
        if !capabilities.ready {
            return;
        }
        if let Err(reason) = validate_profile_for_capabilities(profile, &capabilities) {
            if warning.is_none() {
                commands.insert_resource(RenderWarning {
                    kind: RenderWarningKind::GraphicsSettings,
                    message: format!(
                        "Unsupported graphics shadow settings: {reason}. The requested settings were not applied."
                    ),
                });
            }
            return;
        }
    }

    if directional_map.size != profile.directional_shadow_map_size as usize {
        directional_map.size = profile.directional_shadow_map_size as usize;
    }
    if point_map.size != profile.point_shadow_map_size as usize {
        point_map.size = profile.point_shadow_map_size as usize;
    }

    for (mut light, mut config, authored_ranges, defaults) in &mut directional_lights {
        if let Some(defaults) = defaults {
            if defaults.intensity_uses_graphics_default {
                light.illuminance =
                    profile.distant_light_default_illuminance * defaults.intensity_scale;
            }
        }
        light.shadow_depth_bias = profile.shadow_depth_bias;
        light.shadow_normal_bias = profile.shadow_normal_bias;

        let Some(current_maximum_distance) = config.bounds.last().copied() else {
            warn!(
                "directional light has no cascade bounds; preserving its invalid shadow configuration"
            );
            continue;
        };
        let current_first_cascade_far_bound = config
            .bounds
            .first()
            .copied()
            .unwrap_or(current_maximum_distance);
        let authored_ranges = authored_ranges.copied().unwrap_or_default();
        let maximum_distance = if authored_ranges.maximum_distance {
            current_maximum_distance
        } else {
            profile.shadow_maximum_distance
        };
        let first_cascade_far_bound = if authored_ranges.first_cascade_far_bound {
            current_first_cascade_far_bound
        } else {
            profile.shadow_first_cascade_far_bound
        };
        if maximum_distance <= profile.shadow_minimum_distance
            || (profile.directional_cascades > 1
                && first_cascade_far_bound <= profile.shadow_minimum_distance)
            || first_cascade_far_bound > maximum_distance
        {
            warn!(
                "graphics shadow defaults cannot be applied to directional light: minimum={}, first={}, maximum={}; preserving its current cascade configuration",
                profile.shadow_minimum_distance, first_cascade_far_bound, maximum_distance,
            );
            continue;
        }
        *config = bevy::light::CascadeShadowConfigBuilder {
            num_cascades: profile.directional_cascades,
            minimum_distance: profile.shadow_minimum_distance,
            first_cascade_far_bound,
            maximum_distance,
            overlap_proportion: profile.shadow_cascade_overlap,
        }
        .build();
    }

    for (mut light, defaults) in &mut point_lights {
        if let Some(defaults) = defaults {
            if defaults.intensity_uses_graphics_default {
                light.intensity = profile.local_light_default_intensity * defaults.intensity_scale;
            }
            if defaults.range_uses_graphics_default {
                light.range = profile.local_light_default_range;
            }
        }
        light.shadow_depth_bias = profile.shadow_depth_bias;
        light.shadow_normal_bias = profile.shadow_normal_bias;
        light.shadow_map_near_z = profile.local_shadow_map_near_z;
    }

    for (mut light, defaults) in &mut spot_lights {
        if let Some(defaults) = defaults {
            if defaults.intensity_uses_graphics_default {
                light.intensity = profile.local_light_default_intensity * defaults.intensity_scale;
            }
            if defaults.range_uses_graphics_default {
                light.range = profile.local_light_default_range;
            }
        }
        light.shadow_depth_bias = profile.shadow_depth_bias;
        light.shadow_normal_bias = profile.shadow_normal_bias;
        light.shadow_map_near_z = profile.local_shadow_map_near_z;
    }

    for (mut light, defaults) in &mut rect_lights {
        if let Some(defaults) = defaults {
            if defaults.intensity_uses_graphics_default {
                light.intensity = profile.rect_light_default_intensity * defaults.intensity_scale;
            }
            if defaults.range_uses_graphics_default {
                light.range = profile.local_light_default_range;
            }
        }
    }
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
    // a DX12-only cfg. Log everything the safe API does give, which is already
    // far more than the tester got.
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
                             pipeline is invalid — check the bound Shader's \
                             `info:wgsl:sourceAsset` (it must name a whole shader with an \
                             `@fragment` entry, not a library). Identical errors are now \
                             rate-limited."
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
                    let estimated_bytes = health.shadow_estimated_bytes.load(Ordering::Relaxed);
                    let budget_bytes = health.shadow_budget_bytes.load(Ordering::Relaxed);
                    error!(
                        "wgpu out of memory on {adapter_desc}: {desc}; configured shadow estimate={} bytes ({} MiB), explicit byte ceiling={} bytes ({} MiB)",
                        estimated_bytes,
                        estimated_bytes / (1024 * 1024),
                        budget_bytes,
                        budget_bytes / (1024 * 1024),
                    );
                }
                wgpu::Error::Internal { .. } => {
                    error!("wgpu internal error on {adapter_desc}: {desc}");
                }
            }
        }));
}

fn shadow_hook_int(value: u64) -> lunco_hooks::HookValue {
    lunco_hooks::HookValue::Int(value.min(i64::MAX as u64) as i64)
}

fn publish_shadow_facts(
    exposures: Option<&mut lunco_core::exposure::EngineExposures>,
    profile: lunco_render::RenderQualityProfile,
    light_count: usize,
    directional_casters: usize,
    point_casters: usize,
    spot_casters: usize,
    directional_cascade_layers: usize,
    required_bytes: u64,
    budget_bytes: u64,
) {
    let Some(exposures) = exposures else {
        return;
    };
    let mut writer = exposures.writer("render-shadow");
    writer.visible(true);
    writer.property("valid", true);
    writer.property("light_count", light_count as f64);
    writer.property("directional_casters", directional_casters as f64);
    writer.property("point_casters", point_casters as f64);
    writer.property("spot_casters", spot_casters as f64);
    writer.property(
        "directional_cascade_layers",
        directional_cascade_layers as f64,
    );
    writer.property(
        "directional_map_size",
        profile.directional_shadow_map_size as f64,
    );
    writer.property("point_map_size", profile.point_shadow_map_size as f64);
    writer.property("estimated_bytes", required_bytes as f64);
    writer.property("budget_bytes", budget_bytes as f64);
    writer.property(
        "max_directional_shadow_casters",
        profile.max_directional_shadow_casters as f64,
    );
    writer.property(
        "max_point_shadow_casters",
        profile.max_point_shadow_casters as f64,
    );
    writer.property(
        "max_spot_shadow_casters",
        profile.max_spot_shadow_casters as f64,
    );
}

fn shadow_quality_policy_warning(
    profile: lunco_render::RenderQualityProfile,
    directional_casters: usize,
    point_casters: usize,
    spot_casters: usize,
    required_bytes: u64,
    budget_bytes: u64,
) -> Option<String> {
    let facts = lunco_hooks::HookValue::map([
        (
            "directional_casters",
            shadow_hook_int(directional_casters as u64),
        ),
        ("point_casters", shadow_hook_int(point_casters as u64)),
        ("spot_casters", shadow_hook_int(spot_casters as u64)),
        (
            "max_directional_shadow_casters",
            shadow_hook_int(profile.max_directional_shadow_casters as u64),
        ),
        (
            "max_point_shadow_casters",
            shadow_hook_int(profile.max_point_shadow_casters as u64),
        ),
        (
            "max_spot_shadow_casters",
            shadow_hook_int(profile.max_spot_shadow_casters as u64),
        ),
        ("estimated_bytes", shadow_hook_int(required_bytes)),
        ("budget_bytes", shadow_hook_int(budget_bytes)),
    ]);
    match lunco_hooks::invoke(lunco_core::session::RENDER_SHADOW_QUALITY_HOOK, &[facts]) {
        None | Some(Ok(lunco_hooks::HookValue::Unit)) => None,
        Some(Ok(lunco_hooks::HookValue::Str(message))) if !message.is_empty() => Some(message),
        Some(Ok(lunco_hooks::HookValue::Str(_))) => None,
        Some(Ok(value)) => {
            warn!("render shadow-quality policy returned unsupported value: {value:?}");
            None
        }
        Some(Err(error)) => {
            warn!("render shadow-quality policy failed: {error}");
            None
        }
    }
}

/// Report shadow-resource pressure before Bevy's PBR render preparation
/// allocates its depth textures. The user's map sizes, cascade count, byte
/// ceiling, and every authored `shadow_maps_enabled` value remain unchanged.
/// This system is diagnostic only: an explicit graphics limit that cannot
/// accommodate the live scene produces one StatusBus warning, never a selected
/// subset of lights or a renderer-owned shadow omission.
///
/// The estimate does not account for unrelated GPU allocations or driver
/// overhead; adapter limits are validated separately before settings apply.
fn apply_shadow_caster_policy(
    mut state: ResMut<ShadowAdmissionState>,
    mut commands: Commands,
    settings: Res<RenderingQualitySettings>,
    warning: Option<Res<RenderWarning>>,
    status_bus: Option<ResMut<crate::status_bus::StatusBus>>,
    exposures: Option<ResMut<lunco_core::exposure::EngineExposures>>,
    health: Option<Res<RenderHealthHandle>>,
    directional_shadow_map: Res<bevy::light::DirectionalLightShadowMap>,
    point_shadow_map: Res<bevy::light::PointLightShadowMap>,
    directionals: Query<(
        &bevy::light::DirectionalLight,
        &bevy::light::CascadeShadowConfig,
    )>,
    points: Query<&bevy::light::PointLight>,
    spots: Query<&bevy::light::SpotLight>,
) {
    let mut exposures = exposures;
    let profile = match settings.validated_profile() {
        Ok(profile) => profile,
        Err(reason) => {
            state.configured_limit_status_active = false;
            if let Some(mut exposures) = exposures {
                let mut writer = exposures.writer("render-shadow");
                writer.visible(false);
                writer.property("valid", false);
            }
            if warning.is_none() {
                commands.insert_resource(RenderWarning {
                    kind: RenderWarningKind::GraphicsSettings,
                    message: format!(
                        "Invalid graphics shadow settings: {reason}. Shadow caster limits were not changed."
                    ),
                });
            }
            return;
        }
    };
    let admission_budget = profile.shadow_budget_bytes;
    if let Some(health) = health.as_ref() {
        health
            .0
            .shadow_budget_bytes
            .store(admission_budget, Ordering::Relaxed);
    }
    let policy_signature = shadow_policy_signature(profile)
        .wrapping_mul(31)
        .wrapping_add(lunco_hooks::generation());

    let light_count = directionals.iter().count() + points.iter().count() + spots.iter().count();
    let enabled_directional_casters = directionals
        .iter()
        .filter(|(light, _)| light.shadow_maps_enabled)
        .count();
    let enabled_point_casters = points
        .iter()
        .filter(|light| light.shadow_maps_enabled)
        .count();
    let enabled_spot_casters = spots
        .iter()
        .filter(|light| light.shadow_maps_enabled)
        .count();
    let directional_cascade_layers = directionals
        .iter()
        .filter(|(light, _)| light.shadow_maps_enabled)
        .map(|(_, config)| config.bounds.len().max(1))
        .sum::<usize>();
    let required_bytes = estimate_shadow_allocation_bytes(
        directional_shadow_map.size,
        point_shadow_map.size,
        1,
        directional_cascade_layers,
        enabled_point_casters,
        enabled_spot_casters,
    );
    publish_shadow_facts(
        exposures.as_deref_mut(),
        profile,
        light_count,
        enabled_directional_casters,
        enabled_point_casters,
        enabled_spot_casters,
        directional_cascade_layers,
        required_bytes,
        admission_budget,
    );

    let configuration_changed = state.light_count != Some(light_count)
        || state.enabled_directional_casters != Some(enabled_directional_casters)
        || state.enabled_point_casters != Some(enabled_point_casters)
        || state.enabled_spot_casters != Some(enabled_spot_casters)
        || state.directional_map_size != Some(directional_shadow_map.size)
        || state.point_map_size != Some(point_shadow_map.size)
        || state.directional_cascade_layers != Some(directional_cascade_layers)
        || state.budget_bytes != Some(admission_budget)
        || state.policy_signature != Some(policy_signature);
    if !configuration_changed {
        return;
    }
    state.configured_limit_status_active = false;

    if let Some(health) = health.as_ref() {
        health
            .0
            .shadow_estimated_bytes
            .store(required_bytes, Ordering::Relaxed);
    }

    let policy_warning = shadow_quality_policy_warning(
        profile,
        enabled_directional_casters,
        enabled_point_casters,
        enabled_spot_casters,
        required_bytes,
        admission_budget,
    );
    if let Some(mut status_bus) = status_bus {
        if let Some(message) = policy_warning {
            if !state.configured_limit_status_active {
                status_bus.push(
                    crate::status_bus::RENDER_SOURCE,
                    crate::status_bus::StatusLevel::Warn,
                    message,
                );
            }
            state.configured_limit_status_active = true;
        } else {
            state.configured_limit_status_active = false;
        }
    }

    let estimated_mib = required_bytes as f64 / (1024.0 * 1024.0);
    warn!(
        "shadow allocation: {} directional caster(s), {} cascade layer(s), {} point caster(s), {} spot caster(s), estimated allocation {} bytes ({:.1} MiB) of configured ceiling {} bytes (directional {}px, point {}px); authored state preserved",
        enabled_directional_casters,
        directional_cascade_layers,
        enabled_point_casters,
        enabled_spot_casters,
        required_bytes,
        estimated_mib,
        admission_budget,
        directional_shadow_map.size,
        point_shadow_map.size,
    );

    state.light_count = Some(light_count);
    state.enabled_directional_casters = Some(enabled_directional_casters);
    state.enabled_point_casters = Some(enabled_point_casters);
    state.enabled_spot_casters = Some(enabled_spot_casters);
    state.directional_map_size = Some(directional_shadow_map.size);
    state.point_map_size = Some(point_shadow_map.size);
    state.directional_cascade_layers = Some(directional_cascade_layers);
    state.budget_bytes = Some(admission_budget);
    state.policy_signature = Some(policy_signature);
}

/// Scene teardown is the explicit re-arm boundary for presentation recovery.
/// A new scene gets fresh lights and cameras, so clearing the old ladder and
/// shadow-admission bookkeeping is safe. A lost device remains terminal because
/// no scene reload can recreate the adapter in-process.
pub(crate) fn reset_render_recovery(
    health: Res<RenderHealthHandle>,
    mut ladder: ResMut<Ladder>,
    mut budget: ResMut<ShadowAdmissionState>,
    mut presentation: ResMut<PresentationState>,
    mut commands: Commands,
) {
    if health.0.device_lost() {
        return;
    }
    health.0.reset_for_scene();
    ladder.reset_state();
    *budget = ShadowAdmissionState::default();
    // The render schedule is gated by this extracted state, not by camera
    // activation.  Clearing only the ladder would leave a successfully
    // reloaded scene permanently headless after the previous scene gave up.
    presentation.stopped = false;
    commands.remove_resource::<RenderWarning>();
    commands.remove_resource::<RenderGaveUp>();
}

/// Main-world escalation: read the shared tallies, advance the [`Ladder`], and
/// publish the cross-world presentation gate when the fault persists.
///
/// In the main world because `Camera::is_active` is extracted to the render
/// world each frame. The terminal presentation gate additionally stops the
/// render schedule itself.
fn escalate_render_recovery(
    health: Res<RenderHealthHandle>,
    mut ladder: ResMut<Ladder>,
    time: Res<Time>,
    mut presentation: ResMut<PresentationState>,
    mut commands: Commands,
    warning: Option<Res<RenderWarning>>,
    mut cameras: Query<&mut bevy::camera::Camera>,
) {
    let h = &health.0;
    let kind = h.failure_kind();
    let action = ladder.step(h.total(), kind, h.device_lost(), time.elapsed_secs_f64());

    if action.is_none() && ladder.rung == Rung::PersistentFailure && warning.is_none() {
        commands.insert_resource(RenderWarning {
            kind: RenderWarningKind::RuntimeFailure,
            message: format!(
                "Rendering is failing because of {}. No automatic quality fallback was applied; presentation will stop if it does not recover.",
                kind.label()
            ),
        });
    }

    let Some(action) = action else {
        return;
    };

    match action {
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
                    "{} persisted for {:.0}s ({} total, {} shadow-map, {} out-of-memory)",
                    kind.label(),
                    ladder.failure_give_up_after_secs,
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
                kind: RenderWarningKind::RuntimeFailure,
                message: format!(
                    "Presentation stopped: {reason}. Simulation and API remain available; restart the window to restore rendering."
                ),
            });
            commands.insert_resource(RenderGaveUp { reason });
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

    #[test]
    fn warning_titles_distinguish_invalid_configuration_from_render_failure() {
        assert_eq!(
            RenderWarningKind::GraphicsSettings.title(),
            "GRAPHICS SETTINGS"
        );
        assert_eq!(
            RenderWarningKind::RuntimeFailure.title(),
            "RENDERING DEGRADED"
        );
    }

    fn default_give_up_after_secs() -> f64 {
        RenderingQualitySettings::default().render_failure_give_up_after_secs
    }

    struct TestShadowQualityPolicy;

    impl lunco_hooks::ScriptHook for TestShadowQualityPolicy {
        fn invoke(
            &self,
            _args: &[lunco_hooks::HookValue],
        ) -> Result<lunco_hooks::HookValue, lunco_hooks::HookError> {
            Ok(lunco_hooks::HookValue::str("policy warning"))
        }
    }

    struct ShadowQualityHookGuard;

    impl Drop for ShadowQualityHookGuard {
        fn drop(&mut self) {
            lunco_hooks::unregister(lunco_core::session::RENDER_SHADOW_QUALITY_HOOK);
        }
    }

    fn install_test_shadow_quality_policy() -> ShadowQualityHookGuard {
        lunco_hooks::register(lunco_hooks::RegisteredHook {
            id: lunco_core::session::RENDER_SHADOW_QUALITY_HOOK.into(),
            backend: "test".into(),
            deterministic: false,
            hook: Arc::new(TestShadowQualityPolicy),
        });
        ShadowQualityHookGuard
    }

    fn capabilities(
        max_texture_dimension_2d: u32,
        max_texture_array_layers: u32,
    ) -> RenderCapabilities {
        RenderCapabilities {
            ready: true,
            max_texture_dimension_2d,
            max_texture_array_layers,
        }
    }

    #[test]
    fn shadow_map_choices_are_adapter_bounded_powers_of_two() {
        assert_eq!(
            capabilities(4095, 2048).supported_shadow_map_sizes(),
            Some(vec![1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048])
        );
        assert_eq!(
            RenderCapabilities::default().supported_shadow_map_sizes(),
            None
        );
    }

    #[test]
    fn shadow_settings_reject_bevy_shader_limits() {
        let mut profile = RenderingQualitySettings::default().profile();
        profile.directional_cascades = bevy::pbr::MAX_CASCADES_PER_LIGHT + 1;
        assert!(
            validate_profile_for_capabilities(profile, &capabilities(4096, 2048))
                .unwrap_err()
                .contains("cascade count")
        );

        profile = RenderingQualitySettings::default().profile();
        profile.max_directional_shadow_casters = bevy::pbr::MAX_DIRECTIONAL_LIGHTS + 1;
        assert!(
            validate_profile_for_capabilities(profile, &capabilities(4096, 2048))
                .unwrap_err()
                .contains("directional shadow caster limit")
        );
    }

    #[test]
    fn shadow_settings_reject_adapter_texture_limits() {
        let mut profile = RenderingQualitySettings::default().profile();
        profile.directional_shadow_map_size = 8192;
        assert!(
            validate_profile_for_capabilities(profile, &capabilities(4096, 2048))
                .unwrap_err()
                .contains("directional shadow-map size")
        );

        profile = RenderingQualitySettings::default().profile();
        profile.max_point_shadow_casters = 2;
        assert!(
            validate_profile_for_capabilities(profile, &capabilities(4096, 6))
                .unwrap_err()
                .contains("point shadow caster limit")
        );

        profile = RenderingQualitySettings::default().profile();
        profile.max_directional_shadow_casters = 2;
        profile.directional_cascades = 2;
        profile.max_spot_shadow_casters = 1;
        profile.max_point_shadow_casters = 0;
        assert!(
            validate_profile_for_capabilities(profile, &capabilities(4096, 4))
                .unwrap_err()
                .contains("directional and spot shadow limits")
        );
    }

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
            kind: RenderWarningKind::RuntimeFailure,
            message: "test warning".into(),
        });
        let mut reset = Schedule::new(Update);
        reset.add_systems(reset_render_recovery);
        reset.run(&mut world);
        assert!(!world.resource::<PresentationState>().stopped);
        assert!(!health.presentation_stopped.load(Ordering::Relaxed));
        assert!(world.get_resource::<RenderGaveUp>().is_none());
        assert!(world.get_resource::<RenderWarning>().is_none());
    }

    #[test]
    fn quality_settings_apply_to_existing_and_late_lights() {
        let mut app = App::new();
        app.insert_resource(RenderingQualitySettings::default());
        app.insert_resource(bevy::light::DirectionalLightShadowMap { size: 1024 });
        app.insert_resource(bevy::light::PointLightShadowMap { size: 1024 });
        app.init_resource::<Ladder>();
        app.add_systems(Update, apply_render_quality.run_if(render_quality_changed));

        let directional = app
            .world_mut()
            .spawn((
                bevy::light::DirectionalLight::default(),
                bevy::light::CascadeShadowConfig::default(),
            ))
            .id();
        let point = app
            .world_mut()
            .spawn(bevy::light::PointLight::default())
            .id();
        let spot = app
            .world_mut()
            .spawn(bevy::light::SpotLight::default())
            .id();

        app.update();
        app.world_mut()
            .resource_mut::<RenderingQualitySettings>()
            .shadow_depth_bias = 0.37;
        app.world_mut()
            .resource_mut::<RenderingQualitySettings>()
            .shadow_normal_bias = 7.25;
        app.world_mut()
            .resource_mut::<RenderingQualitySettings>()
            .directional_cascades = 3;
        app.update();

        let directional_light = app
            .world()
            .get::<bevy::light::DirectionalLight>(directional)
            .unwrap();
        assert_eq!(directional_light.shadow_depth_bias, 0.37);
        assert_eq!(directional_light.shadow_normal_bias, 7.25);
        let point_light = app.world().get::<bevy::light::PointLight>(point).unwrap();
        assert_eq!(point_light.shadow_depth_bias, 0.37);
        assert_eq!(point_light.shadow_normal_bias, 7.25);
        let spot_light = app.world().get::<bevy::light::SpotLight>(spot).unwrap();
        assert_eq!(spot_light.shadow_depth_bias, 0.37);
        assert_eq!(spot_light.shadow_normal_bias, 7.25);

        let late_directional = app
            .world_mut()
            .spawn((
                bevy::light::DirectionalLight::default(),
                bevy::light::CascadeShadowConfig::default(),
            ))
            .id();
        let late_point = app
            .world_mut()
            .spawn(bevy::light::PointLight::default())
            .id();
        let late_spot = app
            .world_mut()
            .spawn(bevy::light::SpotLight::default())
            .id();
        let late_rect = app
            .world_mut()
            .spawn((
                bevy::light::RectLight::default(),
                LightGraphicsDefaults {
                    intensity_uses_graphics_default: true,
                    intensity_scale: 1.0,
                    range_uses_graphics_default: true,
                },
            ))
            .id();
        app.update();

        let late_directional_light = app
            .world()
            .get::<bevy::light::DirectionalLight>(late_directional)
            .unwrap();
        assert_eq!(late_directional_light.shadow_depth_bias, 0.37);
        assert_eq!(late_directional_light.shadow_normal_bias, 7.25);
        assert_eq!(
            app.world()
                .get::<bevy::light::CascadeShadowConfig>(late_directional)
                .unwrap()
                .bounds
                .len(),
            3
        );
        let late_point_light = app
            .world()
            .get::<bevy::light::PointLight>(late_point)
            .unwrap();
        assert_eq!(late_point_light.shadow_depth_bias, 0.37);
        assert_eq!(late_point_light.shadow_normal_bias, 7.25);
        let late_spot_light = app
            .world()
            .get::<bevy::light::SpotLight>(late_spot)
            .unwrap();
        assert_eq!(late_spot_light.shadow_depth_bias, 0.37);
        assert_eq!(late_spot_light.shadow_normal_bias, 7.25);
        let late_rect_light = app
            .world()
            .get::<bevy::light::RectLight>(late_rect)
            .unwrap();
        assert_eq!(late_rect_light.intensity, 10_000.0);
        assert_eq!(late_rect_light.range, 30.0);
    }

    #[test]
    fn graphics_light_defaults_update_live_without_overwriting_authored_values() {
        let mut app = App::new();
        app.insert_resource(RenderingQualitySettings::default());
        app.insert_resource(bevy::light::DirectionalLightShadowMap { size: 1024 });
        app.insert_resource(bevy::light::PointLightShadowMap { size: 1024 });
        app.init_resource::<Ladder>();
        app.add_systems(Update, apply_render_quality.run_if(render_quality_changed));

        let distant = app
            .world_mut()
            .spawn((
                bevy::light::DirectionalLight {
                    illuminance: 999.0,
                    ..default()
                },
                bevy::light::CascadeShadowConfig::default(),
                LightGraphicsDefaults {
                    intensity_uses_graphics_default: true,
                    intensity_scale: 2.0,
                    range_uses_graphics_default: false,
                },
            ))
            .id();
        let authored_distant = app
            .world_mut()
            .spawn((
                bevy::light::DirectionalLight {
                    illuminance: 777.0,
                    ..default()
                },
                bevy::light::CascadeShadowConfig::default(),
                LightGraphicsDefaults {
                    intensity_uses_graphics_default: false,
                    intensity_scale: 1.0,
                    range_uses_graphics_default: false,
                },
            ))
            .id();
        let point = app
            .world_mut()
            .spawn((
                bevy::light::PointLight {
                    intensity: 999.0,
                    range: 99.0,
                    ..default()
                },
                LightGraphicsDefaults {
                    intensity_uses_graphics_default: true,
                    intensity_scale: 1.5,
                    range_uses_graphics_default: true,
                },
            ))
            .id();
        let authored_point = app
            .world_mut()
            .spawn((
                bevy::light::PointLight {
                    intensity: 777.0,
                    range: 88.0,
                    ..default()
                },
                LightGraphicsDefaults {
                    intensity_uses_graphics_default: false,
                    intensity_scale: 1.0,
                    range_uses_graphics_default: false,
                },
            ))
            .id();
        let spot = app
            .world_mut()
            .spawn((
                bevy::light::SpotLight {
                    intensity: 999.0,
                    range: 99.0,
                    ..default()
                },
                LightGraphicsDefaults {
                    intensity_uses_graphics_default: true,
                    intensity_scale: 2.0,
                    range_uses_graphics_default: true,
                },
            ))
            .id();
        let rect = app
            .world_mut()
            .spawn((
                bevy::light::RectLight {
                    intensity: 999.0,
                    range: 99.0,
                    ..default()
                },
                LightGraphicsDefaults {
                    intensity_uses_graphics_default: true,
                    intensity_scale: 0.5,
                    range_uses_graphics_default: true,
                },
            ))
            .id();

        app.update();
        let mut settings = app.world_mut().resource_mut::<RenderingQualitySettings>();
        settings.distant_light_default_illuminance = 42_000.0;
        settings.local_light_default_intensity = 10.0;
        settings.rect_light_default_intensity = 20.0;
        settings.local_light_default_range = 7.0;
        app.update();

        assert_eq!(
            app.world()
                .get::<bevy::light::DirectionalLight>(distant)
                .unwrap()
                .illuminance,
            84_000.0
        );
        assert_eq!(
            app.world()
                .get::<bevy::light::DirectionalLight>(authored_distant)
                .unwrap()
                .illuminance,
            777.0
        );
        let point_light = app.world().get::<bevy::light::PointLight>(point).unwrap();
        assert_eq!(point_light.intensity, 15.0);
        assert_eq!(point_light.range, 7.0);
        let authored_point_light = app
            .world()
            .get::<bevy::light::PointLight>(authored_point)
            .unwrap();
        assert_eq!(authored_point_light.intensity, 777.0);
        assert_eq!(authored_point_light.range, 88.0);
        let spot_light = app.world().get::<bevy::light::SpotLight>(spot).unwrap();
        assert_eq!(spot_light.intensity, 20.0);
        assert_eq!(spot_light.range, 7.0);
        let rect_light = app.world().get::<bevy::light::RectLight>(rect).unwrap();
        assert_eq!(rect_light.intensity, 10.0);
        assert_eq!(rect_light.range, 7.0);
    }

    #[test]
    fn quality_range_defaults_update_without_overwriting_authored_ranges() {
        let mut app = App::new();
        app.insert_resource(RenderingQualitySettings::default());
        app.insert_resource(bevy::light::DirectionalLightShadowMap { size: 1024 });
        app.insert_resource(bevy::light::PointLightShadowMap { size: 1024 });
        app.init_resource::<Ladder>();
        app.add_systems(Update, apply_render_quality.run_if(render_quality_changed));

        let unauthored = app
            .world_mut()
            .spawn((
                bevy::light::DirectionalLight::default(),
                bevy::light::CascadeShadowConfig::default(),
            ))
            .id();
        let authored = app
            .world_mut()
            .spawn((
                bevy::light::DirectionalLight::default(),
                bevy::light::CascadeShadowConfigBuilder {
                    num_cascades: 2,
                    minimum_distance: 0.1,
                    first_cascade_far_bound: 30.0,
                    maximum_distance: 700.0,
                    overlap_proportion: 0.05,
                }
                .build(),
                ShadowRangeAuthorship {
                    first_cascade_far_bound: true,
                    maximum_distance: true,
                },
            ))
            .id();

        app.update();
        let mut settings = app.world_mut().resource_mut::<RenderingQualitySettings>();
        settings.directional_cascades = 3;
        settings.shadow_minimum_distance = 0.5;
        settings.shadow_first_cascade_far_bound = 25.0;
        settings.shadow_maximum_distance = 800.0;
        settings.shadow_cascade_overlap = 0.2;
        app.update();

        let unauthored_config = app
            .world()
            .get::<bevy::light::CascadeShadowConfig>(unauthored)
            .unwrap();
        assert_eq!(unauthored_config.bounds.len(), 3);
        assert_eq!(unauthored_config.minimum_distance, 0.5);
        assert_eq!(unauthored_config.bounds.first().copied(), Some(25.0));
        assert!((unauthored_config.bounds.last().copied().unwrap() - 800.0).abs() < 1e-3);
        assert_eq!(unauthored_config.overlap_proportion, 0.2);

        let authored_config = app
            .world()
            .get::<bevy::light::CascadeShadowConfig>(authored)
            .unwrap();
        assert_eq!(authored_config.bounds.len(), 3);
        assert_eq!(authored_config.minimum_distance, 0.5);
        assert_eq!(authored_config.bounds.first().copied(), Some(30.0));
        assert!((authored_config.bounds.last().copied().unwrap() - 700.0).abs() < 1e-3);
        assert_eq!(authored_config.overlap_proportion, 0.2);
    }

    /// The original reason this module exists: a one-frame depth/color size skew
    /// during a window resize. Presentation must survive it — the run of failing
    /// frames has length one, so the clock resets and no amount of elapsed time
    /// can turn it into a give-up.
    ///
    /// No quality setting is changed for the one-frame skew. The clean frame
    /// resets the persistence clock, while the authored shadow intent remains
    /// intact.
    #[test]
    fn a_transient_error_never_gives_up() {
        let mut l = Ladder::default();
        l.step(1, FailureKind::ShadowMap, false, 0.0);
        for i in 0..1000 {
            // No new errors: healthy frames, arbitrarily far into the future.
            assert_eq!(l.step(1, FailureKind::ShadowMap, false, i as f64), None);
        }
        assert_eq!(l.rung, Rung::PersistentFailure);
        assert!(
            l.failing_since.is_none(),
            "a clean frame must reset the clock"
        );
    }

    /// The first error records a persistent-failure state but does not alter
    /// any quality setting.
    #[test]
    fn first_failure_does_not_change_quality() {
        let mut l = Ladder::default();
        assert_eq!(l.step(1, FailureKind::ShadowMap, false, 0.0), None);
        assert_eq!(l.rung, Rung::PersistentFailure);
    }

    #[test]
    fn configured_shadow_caster_limits_preserve_authored_lights_and_deduplicate_warning() {
        let _policy = install_test_shadow_quality_policy();
        let mut app = App::new();
        let settings = RenderingQualitySettings {
            directional_shadow_map_size: 1024,
            point_shadow_map_size: 512,
            max_directional_shadow_casters: 0,
            max_point_shadow_casters: 1,
            max_spot_shadow_casters: 0,
            shadow_budget_bytes: 16 * 1024 * 1024,
            ..Default::default()
        };
        app.insert_resource(settings);
        app.insert_resource(crate::status_bus::StatusBus::default());
        app.insert_resource(lunco_core::exposure::EngineExposures::default());
        app.init_resource::<ShadowAdmissionState>();
        let health = Arc::new(RenderHealth::default());
        app.insert_resource(RenderHealthHandle(health.clone()));
        app.insert_resource(bevy::light::DirectionalLightShadowMap { size: 1024 });
        app.insert_resource(bevy::light::PointLightShadowMap { size: 512 });
        app.add_systems(PostUpdate, apply_shadow_caster_policy);
        for _ in 0..5 {
            app.world_mut().spawn((bevy::light::PointLight {
                intensity: 321.0,
                range: 37.0,
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
        assert_eq!(enabled, 5);
        let mut point_lights = world.query::<&bevy::light::PointLight>();
        assert!(point_lights
            .iter(world)
            .all(|light| light.intensity == 321.0 && light.range == 37.0));
        assert!(estimate_shadow_allocation_bytes(1024, 512, 0, 0, enabled, 0) > 16 * 1024 * 1024);
        assert_eq!(
            health.shadow_estimated_bytes.load(Ordering::Relaxed),
            estimate_shadow_allocation_bytes(1024, 512, 0, 0, 5, 0)
        );
        assert_eq!(
            health.shadow_budget_bytes.load(Ordering::Relaxed),
            16 * 1024 * 1024
        );
        let status_bus = app.world().resource::<crate::status_bus::StatusBus>();
        assert_eq!(
            status_bus
                .history()
                .filter(|event| event.source == crate::status_bus::RENDER_SOURCE)
                .count(),
            1
        );
        assert!(status_bus
            .history()
            .any(|event| event.message.contains("policy warning")));

        let exposures = app
            .world()
            .resource::<lunco_core::exposure::EngineExposures>();
        assert_eq!(
            exposures
                .surfaces
                .get("render-shadow")
                .and_then(|surface| surface.properties.get("point_casters")),
            Some(&lunco_core::exposure::ExposureValue::Number(5.0))
        );

        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::status_bus::StatusBus>()
                .history()
                .filter(|event| event.source == crate::status_bus::RENDER_SOURCE)
                .count(),
            1,
            "an unchanged shadow-limit condition must not spam status history"
        );
    }

    #[test]
    fn invalid_byte_ceiling_does_not_shed_casters() {
        let mut app = App::new();
        let settings = RenderingQualitySettings {
            directional_shadow_map_size: 1024,
            point_shadow_map_size: 512,
            max_directional_shadow_casters: 0,
            max_point_shadow_casters: 1,
            max_spot_shadow_casters: 0,
            shadow_budget_bytes: 1,
            ..Default::default()
        };
        app.insert_resource(settings);
        app.init_resource::<ShadowAdmissionState>();
        let health = Arc::new(RenderHealth::default());
        app.insert_resource(RenderHealthHandle(health.clone()));
        app.insert_resource(bevy::light::DirectionalLightShadowMap { size: 1024 });
        app.insert_resource(bevy::light::PointLightShadowMap { size: 512 });
        app.add_systems(PostUpdate, apply_shadow_caster_policy);
        app.world_mut().spawn(bevy::light::PointLight {
            shadow_maps_enabled: true,
            ..default()
        });

        app.update();

        assert_eq!(health.shadow_budget_bytes.load(Ordering::Relaxed), 0);
        assert_eq!(health.shadow_estimated_bytes.load(Ordering::Relaxed), 0);
        let world = app.world_mut();
        let mut query = world.query::<&bevy::light::PointLight>();
        assert!(query.single(world).unwrap().shadow_maps_enabled);
        assert!(app.world().get_resource::<RenderWarning>().is_some());
    }

    #[test]
    fn effective_cascade_overflow_does_not_shed_or_lower_quality() {
        let mut app = App::new();
        app.insert_resource(RenderingQualitySettings::default());
        app.init_resource::<ShadowAdmissionState>();
        let health = Arc::new(RenderHealth::default());
        app.insert_resource(RenderHealthHandle(health.clone()));
        app.insert_resource(lunco_core::exposure::EngineExposures::default());
        app.insert_resource(bevy::light::DirectionalLightShadowMap { size: 1024 });
        app.insert_resource(bevy::light::PointLightShadowMap { size: 1024 });
        app.add_systems(PostUpdate, apply_shadow_caster_policy);

        let light = app
            .world_mut()
            .spawn((
                bevy::light::DirectionalLight {
                    shadow_maps_enabled: true,
                    ..default()
                },
                bevy::light::CascadeShadowConfig {
                    bounds: (0..128).map(|index| index as f32 + 1.0).collect(),
                    overlap_proportion: 0.2,
                    minimum_distance: 0.1,
                },
            ))
            .id();

        app.update();

        assert!(
            app.world()
                .get::<bevy::light::DirectionalLight>(light)
                .unwrap()
                .shadow_maps_enabled,
            "an effective scene overflow must not be hidden by disabling the caster"
        );
        let required = health.shadow_estimated_bytes.load(Ordering::Relaxed);
        assert!(
            required > RenderingQualitySettings::default().shadow_budget_bytes,
            "the fixture must exceed the explicit byte ceiling"
        );
        let exposures = app
            .world()
            .resource::<lunco_core::exposure::EngineExposures>();
        assert_eq!(
            exposures
                .surfaces
                .get("render-shadow")
                .and_then(|surface| surface.properties.get("directional_cascade_layers")),
            Some(&lunco_core::exposure::ExposureValue::Number(128.0))
        );
    }

    /// The 339-fps null loop. Errors keep coming without a quality fallback,
    /// so presentation is abandoned — once, and only after the grace period.
    #[test]
    fn persistent_failure_gives_up_after_the_grace_period() {
        let mut l = Ladder::default();
        assert_eq!(l.step(1, FailureKind::ShadowMap, false, 0.0), None);

        // Still failing, but inside the grace period: hold.
        let mut total = 1;
        let mut t = 0.0;
        while t + 0.5 < default_give_up_after_secs() - 0.1 {
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
                default_give_up_after_secs() + 0.01
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

    /// The grace period is measured from the first persistent failure because
    /// no automatic mitigation is applied.
    #[test]
    fn grace_period_starts_at_the_first_failure() {
        let mut l = Ladder::default();
        assert_eq!(l.step(1, FailureKind::ShadowMap, false, 100.0), None);
        // 4s after the first failure — not yet.
        assert_eq!(l.step(2, FailureKind::ShadowMap, false, 104.0), None);
        // 5s after the mitigation.
        assert_eq!(
            l.step(3, FailureKind::ShadowMap, false, 105.0),
            Some(Action::GiveUp)
        );
    }

    /// A clean frame resets the persistence clock without silently re-arming
    /// or changing any quality setting.
    #[test]
    fn a_clean_frame_resets_the_persistence_clock() {
        let mut l = Ladder::default();
        l.step(1, FailureKind::ShadowMap, false, 0.0);
        // Frames now render. Total never moves again.
        for i in 0..100 {
            assert_eq!(
                l.step(1, FailureKind::ShadowMap, false, i as f64 * 10.0),
                None
            );
        }
        assert_eq!(l.rung, Rung::PersistentFailure);
    }

    #[test]
    fn an_explicit_quality_change_rearms_pending_failures() {
        let mut l = Ladder::default();
        assert_eq!(l.step(1, FailureKind::ShadowMap, false, 0.0), None);
        l.rearm(1, false);
        assert_eq!(l.rung, Rung::Healthy);
        // The old error total is the new baseline; a clean frame does not
        // immediately trip the ladder again.
        assert_eq!(l.step(1, FailureKind::ShadowMap, false, 1.0), None);

        let mut persistent = Ladder::default();
        persistent.step(1, FailureKind::OutOfMemory, false, 0.0);
        persistent.rearm(1, false);
        assert_eq!(persistent.rung, Rung::Healthy);
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

    /// Device loss after a pending failure still terminates immediately,
    /// rather than waiting out a grace period that cannot help.
    #[test]
    fn device_lost_from_a_degraded_rung_is_still_immediate() {
        let mut l = Ladder::default();
        l.step(1, FailureKind::ShadowMap, false, 0.0);
        assert_eq!(l.rung, Rung::PersistentFailure);
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
