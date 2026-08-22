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
//! | `GaveUp` | still failing [`GIVE_UP_AFTER_SECS`] later, or device lost | deactivate every camera, log once, loudly |
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
    ShadowMapSuppressed, ShadowMapSuppressionReason, ShadowRangeAuthorship,
};
use lunco_settings::AppSettingsExt;

/// How long a failure must persist after the first observed error before
/// presentation is abandoned.
///
/// Measured in wall-clock, not frames, deliberately: the failure mode this exists
/// for renders nothing and therefore runs *fast* (~339 fps was measured), so a
/// frame count would give wildly different grace periods to a wedged app and a
/// healthy one. Long enough that a slow shadow-atlas reallocation is not mistaken
/// for a wedge; short enough that nobody cooks a laptop waiting for it.
const GIVE_UP_AFTER_SECS: f64 = 5.0;

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
    /// Conservative pre-extraction estimate for the shadow resources admitted
    /// by the explicit graphics settings. The wgpu callback cannot inspect the
    /// ECS world, so the policy publishes this here for actionable OOM logs.
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
struct RenderCapabilities {
    ready: bool,
    max_texture_dimension_2d: u32,
    max_texture_array_layers: u32,
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
    PersistentFailure,
    GaveUp,
}

/// What the ladder decided to do this frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Action {
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
    /// Number of currently enabled shadow casters. This changes when an
    /// explicit caster-limit setting changes, even if the scene contains the
    /// same total lights.
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
                .is_some_and(|last| now - last >= FAILURE_QUIET_SECS)
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
            Rung::PersistentFailure if now - since >= GIVE_UP_AFTER_SECS => {
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
fn validate_profile_for_capabilities(
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

#[derive(Clone, Debug)]
struct ShadowCasterCandidate {
    entity: Entity,
    cascade_layers: usize,
    /// Canonical identity used for admission ordering. It is independent of
    /// ECS allocation order; the entity is retained only for the query target
    /// and cache invalidation.
    key: String,
}

fn shadow_caster_key(
    entity: Entity,
    global_id: Option<&lunco_core::GlobalEntityId>,
    provenance: Option<&lunco_core::Provenance>,
    name: Option<&Name>,
) -> String {
    if let Some(id) = global_id {
        return format!("global:{:016x}", id.get());
    }
    if let Some(id) = provenance.and_then(lunco_core::identity::derive_id) {
        return format!("provenance:{id:016x}");
    }
    if let Some(name) = name {
        return format!("name:{}", name.as_str());
    }
    // Anonymous local lights have no semantic identity to order by. This is
    // only a final tie-break for entities that are otherwise indistinguishable;
    // authored/content lights reach one of the canonical keys above.
    format!("anonymous:{:016x}", entity.to_bits())
}

fn sort_shadow_casters(casters: &mut [ShadowCasterCandidate]) {
    casters.sort_by(|a, b| a.key.cmp(&b.key).then_with(|| a.entity.cmp(&b.entity)));
}

fn shadow_configuration_signature(
    directionals: &[ShadowCasterCandidate],
    points: &[ShadowCasterCandidate],
    spots: &[ShadowCasterCandidate],
) -> u64 {
    let mut signature = 0xcbf29ce484222325_u64;
    for candidate in directionals {
        signature ^= candidate.entity.to_bits();
        signature = signature.wrapping_mul(0x100000001b3);
        signature ^= candidate.cascade_layers as u64;
        signature = signature.wrapping_mul(0x100000001b3);
        for byte in candidate.key.as_bytes() {
            signature ^= u64::from(*byte);
            signature = signature.wrapping_mul(0x100000001b3);
        }
    }
    for candidate in points {
        signature ^= candidate.entity.to_bits();
        signature = signature.wrapping_mul(0x100000001b3);
        signature ^= 1;
        signature = signature.wrapping_mul(0x100000001b3);
        for byte in candidate.key.as_bytes() {
            signature ^= u64::from(*byte);
            signature = signature.wrapping_mul(0x100000001b3);
        }
    }
    for candidate in spots {
        signature ^= candidate.entity.to_bits();
        signature = signature.wrapping_mul(0x100000001b3);
        signature ^= 2;
        signature = signature.wrapping_mul(0x100000001b3);
        for byte in candidate.key.as_bytes() {
            signature ^= u64::from(*byte);
            signature = signature.wrapping_mul(0x100000001b3);
        }
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
                message: format!(
                    "Invalid graphics shadow settings: {reason}. The requested settings were not applied until corrected."
                ),
            });
            }
            return;
        }
    };

    if let Some(capabilities) = capabilities {
        if !capabilities.ready {
            return;
        }
        if let Err(reason) = validate_profile_for_capabilities(profile, &capabilities) {
            if warning.is_none() {
                commands.insert_resource(RenderWarning {
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
            warn!("directional light has no cascade bounds; preserving its invalid shadow configuration");
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
                profile.shadow_minimum_distance,
                first_cascade_far_bound,
                maximum_distance,
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

fn restore_suppressed_shadow_maps(
    commands: &mut Commands,
    directional_lights: &mut Query<(
        Entity,
        &mut bevy::light::DirectionalLight,
        &mut bevy::light::CascadeShadowConfig,
        Option<&ShadowMapSuppressed>,
        Option<&ShadowRangeAuthorship>,
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
    for (entity, mut light, _, suppressed, _) in directional_lights.iter_mut() {
        if let Some(suppressed) = suppressed {
            light.shadow_maps_enabled =
                restored_shadow_map_value(light.shadow_maps_enabled, suppressed);
            commands.entity(entity).remove::<ShadowMapSuppressed>();
        }
    }
    for (entity, mut light, suppressed) in point_lights.iter_mut() {
        if let Some(suppressed) = suppressed {
            light.shadow_maps_enabled =
                restored_shadow_map_value(light.shadow_maps_enabled, suppressed);
            commands.entity(entity).remove::<ShadowMapSuppressed>();
        }
    }
    for (entity, mut light, suppressed) in spot_lights.iter_mut() {
        if let Some(suppressed) = suppressed {
            light.shadow_maps_enabled =
                restored_shadow_map_value(light.shadow_maps_enabled, suppressed);
            commands.entity(entity).remove::<ShadowMapSuppressed>();
        }
    }
}

fn restored_shadow_map_value(current: bool, suppressed: &ShadowMapSuppressed) -> bool {
    if current == suppressed.last_applied_enabled {
        suppressed.restore_enabled
    } else {
        current
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

/// Apply the explicitly configured shadow-caster limits before Bevy's PBR
/// render preparation allocates its depth textures. The user's map sizes,
/// cascade count, and byte ceiling remain unchanged. Only casters beyond the
/// explicitly configured per-class limits are suppressed and reported; the
/// byte ceiling is validated as a constraint on those settings and never
/// causes an additional quality downgrade.
///
/// The selection is deliberately stable: directional casters are considered
/// first, then point lights, then spot lights, and each class is ordered by the
/// existing canonical entity identity (`GlobalEntityId`, provenance-derived
/// identity, or its authored `Name`). Anonymous local lights use an ECS key
/// only as a final tie-break because they have no semantic identity. There is
/// no camera-distance heuristic whose result can change merely because the
/// viewer moved. The shared byte estimate is the admission test for the complete
/// configured caster set, so the resulting set is guaranteed to fit the published
/// logical allocation ceiling rather than merely fit three unrelated per-class
/// caps. The estimate does not account for unrelated GPU allocations or driver
/// overhead; adapter limits are validated separately before settings apply.
fn apply_shadow_caster_policy(
    mut state: ResMut<ShadowAdmissionState>,
    mut commands: Commands,
    settings: Res<RenderingQualitySettings>,
    warning: Option<Res<RenderWarning>>,
    health: Option<Res<RenderHealthHandle>>,
    directional_shadow_map: Res<bevy::light::DirectionalLightShadowMap>,
    point_shadow_map: Res<bevy::light::PointLightShadowMap>,
    mut directionals: Query<(
        Entity,
        &mut bevy::light::DirectionalLight,
        &bevy::light::CascadeShadowConfig,
        Option<&ShadowMapSuppressed>,
        Option<&lunco_core::GlobalEntityId>,
        Option<&lunco_core::Provenance>,
        Option<&Name>,
    )>,
    mut points: Query<(
        Entity,
        &mut bevy::light::PointLight,
        Option<&ShadowMapSuppressed>,
        Option<&lunco_core::GlobalEntityId>,
        Option<&lunco_core::Provenance>,
        Option<&Name>,
    )>,
    mut spots: Query<(
        Entity,
        &mut bevy::light::SpotLight,
        Option<&ShadowMapSuppressed>,
        Option<&lunco_core::GlobalEntityId>,
        Option<&lunco_core::Provenance>,
        Option<&Name>,
    )>,
) {
    let profile = match settings.validated_profile() {
        Ok(profile) => profile,
        Err(reason) => {
            if warning.is_none() {
                commands.insert_resource(RenderWarning {
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
    let policy_signature = shadow_policy_signature(profile);

    let admission_changed = state.policy_signature != Some(policy_signature)
        || state.budget_bytes != Some(admission_budget);
    if admission_changed {
        for (entity, mut light, _, suppressed, _, _, _) in directionals.iter_mut() {
            if suppressed.is_some_and(|s| s.reason == ShadowMapSuppressionReason::ConfiguredLimit) {
                if let Some(suppressed) = suppressed {
                    light.shadow_maps_enabled =
                        restored_shadow_map_value(light.shadow_maps_enabled, suppressed);
                }
                commands.entity(entity).remove::<ShadowMapSuppressed>();
            }
        }
        for (entity, mut light, suppressed, _, _, _) in points.iter_mut() {
            if suppressed.is_some_and(|s| s.reason == ShadowMapSuppressionReason::ConfiguredLimit) {
                if let Some(suppressed) = suppressed {
                    light.shadow_maps_enabled =
                        restored_shadow_map_value(light.shadow_maps_enabled, suppressed);
                }
                commands.entity(entity).remove::<ShadowMapSuppressed>();
            }
        }
        for (entity, mut light, suppressed, _, _, _) in spots.iter_mut() {
            if suppressed.is_some_and(|s| s.reason == ShadowMapSuppressionReason::ConfiguredLimit) {
                if let Some(suppressed) = suppressed {
                    light.shadow_maps_enabled =
                        restored_shadow_map_value(light.shadow_maps_enabled, suppressed);
                }
                commands.entity(entity).remove::<ShadowMapSuppressed>();
            }
        }
    }

    let mut directional_entities: Vec<ShadowCasterCandidate> = directionals
        .iter_mut()
        .filter_map(|(entity, light, config, _, global_id, provenance, name)| {
            light.shadow_maps_enabled.then_some(ShadowCasterCandidate {
                entity,
                cascade_layers: config.bounds.len().max(1),
                key: shadow_caster_key(entity, global_id, provenance, name),
            })
        })
        .collect();
    sort_shadow_casters(&mut directional_entities);

    let mut point_entities: Vec<ShadowCasterCandidate> = points
        .iter_mut()
        .filter_map(|(entity, light, _, global_id, provenance, name)| {
            light.shadow_maps_enabled.then_some(ShadowCasterCandidate {
                entity,
                cascade_layers: 1,
                key: shadow_caster_key(entity, global_id, provenance, name),
            })
        })
        .collect();
    sort_shadow_casters(&mut point_entities);

    let mut spot_entities: Vec<ShadowCasterCandidate> = spots
        .iter_mut()
        .filter_map(|(entity, light, _, global_id, provenance, name)| {
            light.shadow_maps_enabled.then_some(ShadowCasterCandidate {
                entity,
                cascade_layers: 1,
                key: shadow_caster_key(entity, global_id, provenance, name),
            })
        })
        .collect();
    sort_shadow_casters(&mut spot_entities);

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
                    .map(|candidate| candidate.cascade_layers)
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
        if let Some(health) = health.as_ref() {
            health.0.shadow_estimated_bytes.store(0, Ordering::Relaxed);
        }
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

    // Validate the effective configured caster set before mutating any light.
    // `settings.validate()` proves the ceiling covers the profile's nominal
    // cascade count, but an authored malformed cascade configuration may still
    // have more bounds than the profile could ever request. That is an invalid
    // effective scene, not permission to silently shed another caster or lower
    // quality. Leave the scene untouched and require an explicit correction.
    let required_bytes = directional_entities
        .iter()
        .take(profile.max_directional_shadow_casters)
        .map(|candidate| {
            estimate_shadow_allocation_bytes(
                directional_shadow_map.size,
                point_shadow_map.size,
                candidate.cascade_layers,
                1,
                0,
                0,
            )
        })
        .chain(
            point_entities
                .iter()
                .take(profile.max_point_shadow_casters)
                .map(|_| {
                    estimate_shadow_allocation_bytes(
                        directional_shadow_map.size,
                        point_shadow_map.size,
                        0,
                        0,
                        1,
                        0,
                    )
                }),
        )
        .chain(
            spot_entities
                .iter()
                .take(profile.max_spot_shadow_casters)
                .map(|_| {
                    estimate_shadow_allocation_bytes(
                        directional_shadow_map.size,
                        point_shadow_map.size,
                        0,
                        0,
                        0,
                        1,
                    )
                }),
        )
        .fold(0_u64, u64::saturating_add);
    if required_bytes > admission_budget {
        if let Some(health) = health.as_ref() {
            health
                .0
                .shadow_estimated_bytes
                .store(required_bytes, Ordering::Relaxed);
        }
        if warning.is_none() {
            commands.insert_resource(RenderWarning {
                message: format!(
                    "Configured shadow caster set requires {} bytes, above the explicit byte ceiling of {} bytes; no caster or quality changes were applied.",
                    required_bytes, admission_budget
                ),
            });
        }
        state.light_count = Some(light_count);
        state.enabled_caster_count = Some(enabled_caster_count);
        state.directional_map_size = Some(directional_shadow_map.size);
        state.point_map_size = Some(point_shadow_map.size);
        state.directional_cascade_layers = Some(
            directional_entities
                .iter()
                .map(|candidate| candidate.cascade_layers)
                .sum(),
        );
        state.budget_bytes = Some(admission_budget);
        state.configuration_signature = Some(configuration_signature);
        state.policy_signature = Some(policy_signature);
        return;
    }

    let mut used_bytes = 0_u64;
    let mut kept_directionals = Vec::new();
    let mut kept_points = Vec::new();
    let mut kept_spots = Vec::new();

    for (index, candidate) in directional_entities.iter().enumerate() {
        let cost = estimate_shadow_allocation_bytes(
            directional_shadow_map.size,
            point_shadow_map.size,
            candidate.cascade_layers,
            1,
            0,
            0,
        );
        let keep = index < profile.max_directional_shadow_casters;
        if keep {
            used_bytes = used_bytes.saturating_add(cost);
            kept_directionals.push(candidate.entity);
            if let Ok((_, _, _, suppressed, _, _, _)) = directionals.get_mut(candidate.entity) {
                if suppressed
                    .is_some_and(|s| s.reason == ShadowMapSuppressionReason::ConfiguredLimit)
                {
                    commands
                        .entity(candidate.entity)
                        .remove::<ShadowMapSuppressed>();
                }
            }
        } else if let Ok((_, mut light, _, suppressed, _, _, _)) =
            directionals.get_mut(candidate.entity)
        {
            if suppressed.is_none() {
                commands
                    .entity(candidate.entity)
                    .try_insert(ShadowMapSuppressed {
                        restore_enabled: true,
                        last_applied_enabled: false,
                        reason: ShadowMapSuppressionReason::ConfiguredLimit,
                    });
            }
            light.shadow_maps_enabled = false;
        }
    }

    for (index, candidate) in point_entities.iter().enumerate() {
        let cost = estimate_shadow_allocation_bytes(
            directional_shadow_map.size,
            point_shadow_map.size,
            0,
            0,
            1,
            0,
        );
        let keep = index < profile.max_point_shadow_casters;
        if keep {
            used_bytes = used_bytes.saturating_add(cost);
            kept_points.push(candidate.entity);
            if let Ok((_, _, suppressed, _, _, _)) = points.get_mut(candidate.entity) {
                if suppressed
                    .is_some_and(|s| s.reason == ShadowMapSuppressionReason::ConfiguredLimit)
                {
                    commands
                        .entity(candidate.entity)
                        .remove::<ShadowMapSuppressed>();
                }
            }
        } else if let Ok((_, mut light, suppressed, _, _, _)) = points.get_mut(candidate.entity) {
            if suppressed.is_none() {
                commands
                    .entity(candidate.entity)
                    .try_insert(ShadowMapSuppressed {
                        restore_enabled: true,
                        last_applied_enabled: false,
                        reason: ShadowMapSuppressionReason::ConfiguredLimit,
                    });
            }
            light.shadow_maps_enabled = false;
        }
    }

    for (index, candidate) in spot_entities.iter().enumerate() {
        let cost = estimate_shadow_allocation_bytes(
            directional_shadow_map.size,
            point_shadow_map.size,
            0,
            0,
            0,
            1,
        );
        let keep = index < profile.max_spot_shadow_casters;
        if keep {
            used_bytes = used_bytes.saturating_add(cost);
            kept_spots.push(candidate.entity);
            if let Ok((_, _, suppressed, _, _, _)) = spots.get_mut(candidate.entity) {
                if suppressed
                    .is_some_and(|s| s.reason == ShadowMapSuppressionReason::ConfiguredLimit)
                {
                    commands
                        .entity(candidate.entity)
                        .remove::<ShadowMapSuppressed>();
                }
            }
        } else if let Ok((_, mut light, suppressed, _, _, _)) = spots.get_mut(candidate.entity) {
            if suppressed.is_none() {
                commands
                    .entity(candidate.entity)
                    .try_insert(ShadowMapSuppressed {
                        restore_enabled: true,
                        last_applied_enabled: false,
                        reason: ShadowMapSuppressionReason::ConfiguredLimit,
                    });
            }
            light.shadow_maps_enabled = false;
        }
    }

    let directional_layers = directional_entities
        .iter()
        .filter(|candidate| kept_directionals.contains(&candidate.entity))
        .map(|candidate| candidate.cascade_layers)
        .sum::<usize>();
    if let Some(health) = health.as_ref() {
        health
            .0
            .shadow_estimated_bytes
            .store(used_bytes, Ordering::Relaxed);
    }
    let estimated_mib = used_bytes as f64 / (1024.0 * 1024.0);
    let limit_shed_count = enabled_caster_count
        .saturating_sub(kept_directionals.len() + kept_points.len() + kept_spots.len());
    warn!(
        "shadow allocation: {} directional caster(s), {} cascade layer(s), {} point caster(s), {} spot caster(s), estimated allocation {} bytes ({:.1} MiB) of configured ceiling {} bytes (directional {}px, point {}px)",
        kept_directionals.len(),
        directional_layers,
        kept_points.len(),
        kept_spots.len(),
        used_bytes,
        estimated_mib,
        admission_budget,
        directional_shadow_map.size,
        point_shadow_map.size,
    );
    if limit_shed_count > 0 && warning.is_none() {
        commands.insert_resource(RenderWarning {
            message: format!(
                "Configured shadow caster limits: kept {} directional, {} point, and {} spot shadow caster(s); {} caster(s) are disabled by the Graphics settings.",
                kept_directionals.len(),
                kept_points.len(),
                kept_spots.len(),
                limit_shed_count,
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
                    .find(|candidate| candidate.entity == *entity)
                    .cloned()
            })
            .collect::<Vec<_>>(),
        &point_entities
            .iter()
            .filter(|candidate| kept_points.contains(&candidate.entity))
            .cloned()
            .collect::<Vec<_>>(),
        &spot_entities
            .iter()
            .filter(|candidate| kept_spots.contains(&candidate.entity))
            .cloned()
            .collect::<Vec<_>>(),
    ));
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
    mut directional_lights: Query<(
        Entity,
        &mut bevy::light::DirectionalLight,
        &mut bevy::light::CascadeShadowConfig,
        Option<&ShadowMapSuppressed>,
        Option<&ShadowRangeAuthorship>,
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

/// The ladder is the whole point of this module and none of it needs a GPU: it
/// is a decision about *when* to degrade, driven by two numbers.
///
/// The failure it guards against cannot be reproduced in CI — it needs the
/// Windows integrated adapter that ran out of memory — so the escalation policy
/// is tested directly instead, against the transcript in the tester's report.
#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn suppression_restore_preserves_a_later_explicit_light_change() {
        let suppressed = ShadowMapSuppressed {
            restore_enabled: true,
            last_applied_enabled: false,
            reason: ShadowMapSuppressionReason::ConfiguredLimit,
        };
        assert!(restored_shadow_map_value(false, &suppressed));
        assert!(
            restored_shadow_map_value(true, &suppressed),
            "a later owner that enabled the light must not be overwritten"
        );

        let explicitly_disabled = ShadowMapSuppressed {
            restore_enabled: false,
            ..suppressed
        };
        assert!(!restored_shadow_map_value(false, &explicitly_disabled));
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
            message: "test warning".into(),
        });
        let light = world
            .spawn((
                bevy::light::PointLight {
                    shadow_maps_enabled: false,
                    ..default()
                },
                ShadowMapSuppressed {
                    restore_enabled: true,
                    last_applied_enabled: false,
                    reason: ShadowMapSuppressionReason::ConfiguredLimit,
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
    fn configured_shadow_caster_limits_do_not_rewrite_quality() {
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
        app.init_resource::<ShadowAdmissionState>();
        let health = Arc::new(RenderHealth::default());
        app.insert_resource(RenderHealthHandle(health.clone()));
        app.insert_resource(bevy::light::DirectionalLightShadowMap { size: 1024 });
        app.insert_resource(bevy::light::PointLightShadowMap { size: 512 });
        app.add_systems(PostUpdate, apply_shadow_caster_policy);
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
        assert_eq!(
            health.shadow_estimated_bytes.load(Ordering::Relaxed),
            estimate_shadow_allocation_bytes(1024, 512, 0, 0, enabled, 0)
        );
        assert_eq!(
            health.shadow_budget_bytes.load(Ordering::Relaxed),
            16 * 1024 * 1024
        );
        assert!(app.world().get_resource::<RenderWarning>().is_some());
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
        let warning = app.world().resource::<RenderWarning>();
        assert!(warning
            .message
            .contains("no caster or quality changes were applied"));
    }

    #[test]
    fn shadow_caster_admission_orders_by_canonical_identity_not_spawn_order() {
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
        app.init_resource::<ShadowAdmissionState>();
        app.insert_resource(bevy::light::DirectionalLightShadowMap { size: 1024 });
        app.insert_resource(bevy::light::PointLightShadowMap { size: 512 });
        app.add_systems(PostUpdate, apply_shadow_caster_policy);

        // The first entity has the lower ECS allocation key but the higher
        // canonical identity. The later-spawned entity must win admission.
        let spawned_first = app
            .world_mut()
            .spawn((
                bevy::light::PointLight {
                    shadow_maps_enabled: true,
                    ..default()
                },
                lunco_core::GlobalEntityId::from_raw(20),
                Name::new("ZetaLamp"),
            ))
            .id();
        let spawned_second = app
            .world_mut()
            .spawn((
                bevy::light::PointLight {
                    shadow_maps_enabled: true,
                    ..default()
                },
                lunco_core::GlobalEntityId::from_raw(10),
                Name::new("AlphaLamp"),
            ))
            .id();

        app.update();

        assert!(
            !app.world()
                .get::<bevy::light::PointLight>(spawned_first)
                .unwrap()
                .shadow_maps_enabled
        );
        assert!(
            app.world()
                .get::<bevy::light::PointLight>(spawned_second)
                .unwrap()
                .shadow_maps_enabled
        );
    }

    #[test]
    fn configured_shadow_limit_rechecks_when_a_caster_is_rearmed() {
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
        app.init_resource::<ShadowAdmissionState>();
        app.insert_resource(bevy::light::DirectionalLightShadowMap { size: 1024 });
        app.insert_resource(bevy::light::PointLightShadowMap { size: 512 });
        app.add_systems(PostUpdate, apply_shadow_caster_policy);
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
    fn user_caster_limit_increase_rearms_only_limit_suppressed_casters() {
        let mut app = App::new();
        let settings = RenderingQualitySettings {
            directional_shadow_map_size: 1024,
            point_shadow_map_size: 512,
            max_directional_shadow_casters: 0,
            max_point_shadow_casters: 1,
            max_spot_shadow_casters: 0,
            shadow_budget_bytes: 64 * 1024 * 1024,
            ..Default::default()
        };
        app.insert_resource(settings);
        app.init_resource::<ShadowAdmissionState>();
        app.insert_resource(bevy::light::DirectionalLightShadowMap { size: 1024 });
        app.insert_resource(bevy::light::PointLightShadowMap { size: 512 });
        app.add_systems(PostUpdate, apply_shadow_caster_policy);
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
            .resource_mut::<RenderingQualitySettings>()
            .max_point_shadow_casters = 4;
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

    /// The 339-fps null loop. Errors keep coming without a quality fallback,
    /// so presentation is abandoned — once, and only after the grace period.
    #[test]
    fn persistent_failure_gives_up_after_the_grace_period() {
        let mut l = Ladder::default();
        assert_eq!(l.step(1, FailureKind::ShadowMap, false, 0.0), None);

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
