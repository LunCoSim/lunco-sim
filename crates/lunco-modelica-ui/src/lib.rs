//! Modelica workbench UI and application facade.
//!
//! The compiler, document runtime, worker, and simulation session live in
//! lunco_modelica_core. This package owns only the egui/workbench
//! integration and the lunica application-facing bundle.

pub use lunco_modelica_core::*;

#[cfg(feature = "ui")]
pub mod ui;

#[cfg(feature = "ui")]
use bevy::prelude::*;
#[cfg(feature = "ui")]
use lunco_modelica_core::ModelicaCorePlugin as CoreModelicaPlugin;

#[cfg(feature = "ui")]
/// UI configuration for the Modelica workbench.
#[derive(Resource, Clone, Debug)]
pub struct ModelicaUiConfig {
    /// Include the landing-page Welcome panel.
    pub include_welcome_panel: bool,
}

#[cfg(feature = "ui")]
impl Default for ModelicaUiConfig {
    fn default() -> Self {
        Self {
            include_welcome_panel: true,
        }
    }
}

#[cfg(feature = "ui")]
/// The Modelica workbench integration: core runtime, source-root admission,
/// UI panels, document presentation, and the shared visualization plugin.
#[derive(Default)]
pub struct ModelicaPlugin;

#[cfg(feature = "ui")]
impl Plugin for ModelicaPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<CoreModelicaPlugin>() {
            app.add_plugins(CoreModelicaPlugin);
        }
        if !app.is_plugin_added::<lunco_viz::LuncoVizPlugin>() {
            app.add_plugins(lunco_viz::LuncoVizPlugin);
        }
        #[cfg(feature = "scripting")]
        if !app.is_plugin_added::<lunco_scripting::LunCoScriptingPlugin>() {
            app.add_plugins(lunco_scripting::LunCoScriptingPlugin);
        }

        app.insert_resource(source_roots::SourceRootRegistry::build());
        app.add_systems(Update, source_roots::load_twin_source_roots);
        app.add_plugins(ui::ModelicaUiPlugin);
        app.add_plugins(lunco_doc_bevy::ViewSyncPlugin);

        app.init_resource::<lunco_twin::DocumentKindRegistry>();
        app.world_mut()
            .resource_mut::<lunco_twin::DocumentKindRegistry>()
            .register(
                lunco_twin::DocumentKindId::new("modelica"),
                lunco_twin::DocumentKindMeta {
                    display_name: "Modelica Model".into(),
                    extensions: vec!["mo"],
                    can_create_new: true,
                    default_filename: Some("NewModel.mo"),
                    uri_scheme: Some("modelica"),
                    manifest_section: Some("modelica"),
                },
            );
        pretty::set_options(pretty::PrettyOptions::tabs());

        app.init_resource::<FrameTimeProbe>();
        app.add_systems(First, frame_time_probe_start);
        app.add_systems(PreUpdate, frame_time_probe_pre_update_end);
        app.add_systems(Update, (sim_focus_pace, frame_time_probe_update_end));
        app.add_systems(PostUpdate, frame_time_probe_post_update_end);
        app.add_systems(Last, frame_time_probe_end);
    }
}

#[cfg(feature = "ui")]
/// One-call bundle for standalone lunica and embedded Modelica workspaces.
#[derive(Default)]
pub struct ModelicaWorkbenchPlugin {
    /// Workbench onboarding and welcome-panel configuration.
    pub config: ModelicaUiConfig,
}

#[cfg(feature = "ui")]
impl Plugin for ModelicaWorkbenchPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_workbench::WorkbenchPlugin>() {
            app.add_plugins(lunco_workbench::WorkbenchPlugin);
        }
        app.insert_resource(self.config.clone());
        app.add_plugins(ModelicaPlugin);

        #[cfg(target_arch = "wasm32")]
        {
            app.add_plugins(ui::wasm_clipboard::WasmClipboardPlugin);
            app.add_plugins(ui::wasm_autosave::WasmAutosavePlugin);
        }

        app.add_plugins(ui::model_share::ModelSharePlugin);

        #[cfg(target_arch = "wasm32")]
        lunco_modelica_core::worker_transport::register_worker_url("./worker/worker_bootstrap.js");
    }
}

#[cfg(feature = "ui")]
fn sim_focus_pace(
    settings: Option<ResMut<bevy::winit::WinitSettings>>,
    pending: Option<Res<experiments_runner::PendingHandles>>,
    models: Query<&worker::ModelicaModel>,
    keep_awake: Option<Res<lunco_core::KeepAwake>>,
    mut idle: Local<Option<bevy::winit::UpdateMode>>,
) {
    let Some(mut settings) = settings else { return };
    if idle.is_none() {
        *idle = Some(settings.unfocused_mode);
    }
    let sim_active = pending.map(|p| !p.0.is_empty()).unwrap_or(false)
        || models.iter().any(|m| !m.paused && m.is_compiled)
        || keep_awake.map(|k| k.wanted()).unwrap_or(false);
    let desired = if sim_active {
        bevy::winit::UpdateMode::Continuous
    } else {
        idle.expect("snapshot set above")
    };
    if settings.unfocused_mode != desired {
        settings.unfocused_mode = desired;
    }
}

#[cfg(feature = "ui")]
fn frame_time_probe_start(mut probe: ResMut<FrameTimeProbe>) {
    let now = web_time::Instant::now();
    probe.frame_start = Some(now);
    probe.pre_update_start = Some(now);
}

#[cfg(feature = "ui")]
fn frame_time_probe_pre_update_end(mut probe: ResMut<FrameTimeProbe>) {
    let now = web_time::Instant::now();
    probe.pre_update_ms = probe
        .pre_update_start
        .map(|t| now.duration_since(t).as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    probe.update_start = Some(now);
}

#[cfg(feature = "ui")]
fn frame_time_probe_update_end(mut probe: ResMut<FrameTimeProbe>) {
    let now = web_time::Instant::now();
    probe.update_ms = probe
        .update_start
        .map(|t| now.duration_since(t).as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    probe.post_update_start = Some(now);
}

#[cfg(feature = "ui")]
fn frame_time_probe_post_update_end(mut probe: ResMut<FrameTimeProbe>) {
    let now = web_time::Instant::now();
    probe.post_update_ms = probe
        .post_update_start
        .map(|t| now.duration_since(t).as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    probe.last_start = Some(now);
}

#[cfg(feature = "ui")]
fn frame_time_probe_end(mut probe: ResMut<FrameTimeProbe>) {
    let now = web_time::Instant::now();
    let last_ms = probe
        .last_start
        .map(|t| now.duration_since(t).as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    let Some(start) = probe.frame_start else {
        return;
    };
    let dt_ms = now.duration_since(start).as_secs_f64() * 1000.0;
    let in_window = probe
        .last_edit
        .map(|t| t.elapsed().as_secs_f64() < 5.0)
        .unwrap_or(false);
    let probe_enabled = std::env::var_os("LUNCO_FRAME_PROBE").is_some();
    if probe_enabled && (dt_ms > 30.0 || in_window) {
        bevy::log::info!(
            "[FrameTimeProbe] total={dt_ms:.0}ms pre={:.0} update={:.0} post={:.0} last={last_ms:.0}{}",
            probe.pre_update_ms,
            probe.update_ms,
            probe.post_update_ms,
            if in_window { " (post-edit window)" } else { "" }
        );
    }
    probe.frame_start = None;
}

#[cfg(feature = "ui")]
/// Shared frame-time diagnostic used by the Modelica editor to mark edits.
#[derive(Resource, Default)]
pub struct FrameTimeProbe {
    frame_start: Option<web_time::Instant>,
    pre_update_start: Option<web_time::Instant>,
    update_start: Option<web_time::Instant>,
    post_update_start: Option<web_time::Instant>,
    last_start: Option<web_time::Instant>,
    pre_update_ms: f64,
    update_ms: f64,
    post_update_ms: f64,
    last_edit: Option<web_time::Instant>,
}

#[cfg(feature = "ui")]
/// Mark the next few frames as following a document edit.
pub fn frame_time_probe_stamp_edit(world: &mut World) {
    if let Some(mut probe) = world.get_resource_mut::<FrameTimeProbe>() {
        probe.last_edit = Some(web_time::Instant::now());
    }
}

/// UI-side painter for Modelica annotation graphics.
#[cfg(feature = "ui")]
pub use ui::icon_paint;
