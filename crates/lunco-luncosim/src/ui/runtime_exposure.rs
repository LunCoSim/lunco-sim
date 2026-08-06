//! Generic retained exposure boundary for runtime-authored HTML UI.
//!
//! This module is deliberately domain-free. Engine systems publish named,
//! already-sampled values into [`EngineExposures`]; HUI/Flair consume that
//! snapshot and own the retained tree, layout, and styling. A template does not
//! know whether a value came from a port, telemetry, physics, a script, or a
//! derived engine capability.
use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::prelude::*;
use bevy::render::{ExtractSchedule, MainWorld, Render, RenderApp, RenderSystems};
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, PrimaryEguiContext};
use bevy_flair::prelude::{InlineStyle, StyleSheet, Styled};
use bevy_hui::prelude::{
    CompileContextEvent, HtmlFunctions, HtmlNode, HtmlStyle, HtmlTemplate, TemplateProperties, UiId,
};
use lunco_core::exposure::EngineExposures;
use lunco_render::SceneCamera;
use lunco_workbench::{PanelId, PanelRects, ScenePickGate};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::io;

/// A semantic action emitted by an authored runtime surface.
///
/// HUI deliberately passes only the pressed element to a bound function. The
/// bridge parses the authored action into a closed semantic enum and turns that
/// callback into a typed event, so application code never needs to inspect HTML
/// ids or mutate simulation state from a template callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RuntimeUiActionKind {
    ViewSurface,
    ViewBodyMoon,
    ViewBodyEarth,
    DismissTerrainOverlay,
}

impl RuntimeUiActionKind {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "view.surface" => Ok(Self::ViewSurface),
            "view.body.moon" => Ok(Self::ViewBodyMoon),
            "view.body.earth" => Ok(Self::ViewBodyEarth),
            "overlay.terrain.dismiss" => Ok(Self::DismissTerrainOverlay),
            _ => Err(format!("unknown runtime UI action `{value}`")),
        }
    }
}

#[derive(Event, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeUiAction {
    /// Closed semantic action authored by the surface adapter.
    pub action: RuntimeUiActionKind,
    /// The retained HTML entity that emitted the action.
    pub source: Entity,
}

/// Bind one HUI callback name to a semantic runtime action.
pub(crate) fn register_action(
    functions: &mut HtmlFunctions,
    callback: impl Into<String>,
    action: RuntimeUiActionKind,
) {
    functions.register(
        callback,
        move |In(source): In<Entity>, mut commands: Commands| {
            commands.trigger(RuntimeUiAction { action, source });
        },
    );
}

/// Authored registration for runtime UI surfaces.
#[derive(Asset, Deserialize, TypePath, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeUiManifest {
    pub surfaces: Vec<RuntimeUiSurfaceDefinition>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeUiSurfaceDefinition {
    /// Stable identity used to validate and reconcile an authored surface.
    pub id: String,
    pub template: String,
    pub stylesheet: String,
    pub namespace: String,
    /// When true, offline capture waits until this authored surface is mounted,
    /// styled, positioned, and visible before frame zero is accepted.
    #[serde(default)]
    pub required_for_recording: bool,
    #[serde(default)]
    pub bindings: HashMap<String, RuntimeUiBindingDefinition>,
    #[serde(default)]
    pub actions: Vec<RuntimeUiActionDefinition>,
    #[serde(default)]
    pub visible_in_perspective: Option<String>,
    #[serde(default)]
    pub gate: Option<String>,
    #[serde(default)]
    pub interactive: bool,
    pub placement: RuntimeUiPlacementDefinition,
}

/// Authored mapping from an engine capability value to a template property.
/// The target key is the template property's name; the source is an engine
/// exposure property. `map` is optional for identity bindings and required for
/// authored state translations such as a boolean display or an active color.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeUiBindingDefinition {
    pub source: String,
    #[serde(default)]
    pub map: HashMap<String, String>,
}

/// Authored callback-to-command mapping. HUI knows only the callback name;
/// the runtime bridge emits the semantic action without a widget-specific Rust
/// registration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeUiActionDefinition {
    pub callback: String,
    pub action: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RuntimeUiPlacementDefinition {
    Viewport,
    DockPanel {
        panel: PanelId,
        #[serde(default)]
        inset: f32,
    },
    Window {
        anchor: RuntimeUiWindowAnchor,
        #[serde(default)]
        offset: [f32; 2],
        width: f32,
        height: f32,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RuntimeUiWindowAnchor {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl RuntimeUiManifest {
    /// Validate the authored runtime UI contract before it reaches HUI or the
    /// semantic action bridge. Invalid manifests are rejected as data errors,
    /// so a typo cannot silently create an inert or globally overwritten UI.
    pub(crate) fn validate(&self) -> Result<(), String> {
        let mut ids = HashSet::new();
        let mut namespaces = HashSet::new();
        let mut callbacks = HashSet::new();

        for surface in &self.surfaces {
            require_non_empty("surface id", &surface.id)?;
            require_non_empty("surface namespace", &surface.namespace)?;
            require_asset_path("surface template", &surface.template)?;
            require_asset_path("surface stylesheet", &surface.stylesheet)?;
            if !ids.insert(surface.id.as_str()) {
                return Err(format!("duplicate runtime UI surface id `{}`", surface.id));
            }
            if !namespaces.insert(surface.namespace.as_str()) {
                return Err(format!(
                    "duplicate runtime UI namespace `{}`",
                    surface.namespace
                ));
            }
            if let Some(perspective) = &surface.visible_in_perspective {
                require_non_empty("visible_in_perspective", perspective)?;
            }
            if let Some(gate) = &surface.gate {
                require_non_empty("surface gate", gate)?;
            }
            validate_placement(&surface.placement)?;

            for (target, binding) in &surface.bindings {
                require_non_empty("binding target", target)?;
                require_non_empty("binding source", &binding.source)?;
                for (source_value, rendered_value) in &binding.map {
                    require_non_empty("binding map key", source_value)?;
                    require_non_empty("binding map value", rendered_value)?;
                }
            }
            for action in &surface.actions {
                require_non_empty("action callback", &action.callback)?;
                require_non_empty("action name", &action.action)?;
                if !callbacks.insert(action.callback.as_str()) {
                    return Err(format!(
                        "duplicate runtime UI callback `{}`",
                        action.callback
                    ));
                }
                RuntimeUiActionKind::parse(&action.action)?;
            }
        }
        Ok(())
    }
}

fn require_non_empty(label: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{label} must not be empty"))
    } else {
        Ok(())
    }
}

fn require_asset_path(label: &str, value: &str) -> Result<(), String> {
    require_non_empty(label, value)?;
    if value.contains('\\')
        || value.contains(':')
        || value.starts_with('/')
        || value.split('/').any(|part| part.is_empty() || part == "..")
    {
        return Err(format!("{label} must be a relative asset path: `{value}`"));
    }
    Ok(())
}

fn validate_placement(placement: &RuntimeUiPlacementDefinition) -> Result<(), String> {
    match placement {
        RuntimeUiPlacementDefinition::Viewport => Ok(()),
        RuntimeUiPlacementDefinition::DockPanel { panel, inset } => {
            require_non_empty("dock panel", panel.as_str())?;
            if !inset.is_finite() || *inset < 0.0 {
                Err("dock panel inset must be finite and non-negative".to_string())
            } else {
                Ok(())
            }
        }
        RuntimeUiPlacementDefinition::Window {
            offset,
            width,
            height,
            ..
        } => {
            if offset.iter().any(|value| !value.is_finite()) {
                return Err("window offset must contain finite values".to_string());
            }
            if !width.is_finite() || *width <= 0.0 || !height.is_finite() || *height <= 0.0 {
                return Err("window width and height must be finite and positive".to_string());
            }
            Ok(())
        }
    }
}

#[derive(Default, TypePath)]
pub(crate) struct RuntimeUiManifestLoader;

impl AssetLoader for RuntimeUiManifestLoader {
    type Asset = RuntimeUiManifest;
    type Settings = ();
    type Error = serde_json::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|error| serde_json::Error::io(error))?;
        let manifest: RuntimeUiManifest = serde_json::from_slice(&bytes)?;
        manifest.validate().map_err(|error| {
            serde_json::Error::io(io::Error::new(io::ErrorKind::InvalidData, error))
        })?;
        Ok(manifest)
    }

    fn extensions(&self) -> &[&str] {
        &["json"]
    }
}

pub(crate) struct RuntimeUiManifestPlugin;

impl Plugin for RuntimeUiManifestPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<RuntimeUiManifest>()
            .init_asset_loader::<RuntimeUiManifestLoader>();
    }
}

#[derive(Resource)]
pub(crate) struct RuntimeUiManifestState {
    handle: Handle<RuntimeUiManifest>,
    applied: Option<AssetId<RuntimeUiManifest>>,
    rebuild_pending: bool,
}

/// Acknowledgement from the render extraction boundary. Main-world layout is
/// not enough to arm offline capture: the render world must have extracted at
/// least one visible UI node for every required surface at the current
/// exposure revision. This is the presentation event the recorder consumes
/// indirectly through [`StatusBus`].
#[derive(Resource, Clone, Copy, Debug, Default)]
pub(crate) struct RuntimeUiRenderState {
    pub extracted_revision: u64,
    pub extracted_surface_count: u32,
}

/// Render-world handoff for the presentation acknowledgement. The extraction
/// phase fills `extracted_revision`; the render schedule promotes it only after
/// the UI render set has submitted the frame. The next extraction copies that
/// submitted revision back to the simulation world.
#[derive(Resource, Clone, Copy, Debug, Default)]
struct RuntimeUiRenderAck {
    extracted_revision: u64,
    extracted_surface_count: u32,
    submitted_revision: u64,
    submitted_surface_count: u32,
}

/// Generic named visibility gates supplied by the host application.
#[derive(Resource, Debug, Default)]
pub(crate) struct RuntimeUiGates {
    values: HashMap<String, bool>,
}

impl RuntimeUiGates {
    pub(crate) fn set(&mut self, name: impl Into<String>, value: bool) {
        self.values.insert(name.into(), value);
    }

    fn allows(&self, name: &str) -> bool {
        self.values.get(name).copied().unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum RuntimeUiPlacement {
    Viewport,
    DockPanel {
        panel: PanelId,
        inset: f32,
    },
    Window {
        anchor: RuntimeUiWindowAnchor,
        offset: Vec2,
        width: f32,
        height: f32,
    },
}

impl From<&RuntimeUiPlacementDefinition> for RuntimeUiPlacement {
    fn from(value: &RuntimeUiPlacementDefinition) -> Self {
        match value {
            RuntimeUiPlacementDefinition::Viewport => Self::Viewport,
            RuntimeUiPlacementDefinition::DockPanel { panel, inset } => Self::DockPanel {
                panel: *panel,
                inset: *inset,
            },
            RuntimeUiPlacementDefinition::Window {
                anchor,
                offset,
                width,
                height,
            } => Self::Window {
                anchor: *anchor,
                offset: Vec2::from_array(*offset),
                width: *width,
                height: *height,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ResolvedRuntimeUiPlacement {
    rect: egui::Rect,
}

/// Root marker for a retained runtime-authored HTML surface. The namespace is
/// the only contract between an engine exposure producer and a template.
#[derive(Component, Debug)]
pub(crate) struct RuntimeUiSurface {
    namespace: String,
    required_for_recording: bool,
    template: Handle<HtmlTemplate>,
    stylesheet: Handle<StyleSheet>,
    bindings: HashMap<String, RuntimeUiBindingDefinition>,
    visible_in_perspective: Option<String>,
    gate: Option<String>,
    interactive: bool,
    mounted: bool,
    /// Set only after the retained tree has a camera target, a computed
    /// non-zero render target, a computed layout, and the current exposure
    /// revision projected into HUI. This is a presentation lifecycle state,
    /// not a frame counter.
    presentation_ready: bool,
    placement: RuntimeUiPlacement,
    applied_revision: u64,
    applied_placement: Option<ResolvedRuntimeUiPlacement>,
    input_rect: Option<egui::Rect>,
}

impl RuntimeUiSurface {
    fn from_definition(
        definition: &RuntimeUiSurfaceDefinition,
        template: Handle<HtmlTemplate>,
        stylesheet: Handle<StyleSheet>,
    ) -> Self {
        Self {
            namespace: definition.namespace.clone(),
            required_for_recording: definition.required_for_recording,
            template,
            stylesheet,
            bindings: definition.bindings.clone(),
            visible_in_perspective: definition.visible_in_perspective.clone(),
            gate: definition.gate.clone(),
            interactive: definition.interactive,
            mounted: false,
            presentation_ready: false,
            placement: (&definition.placement).into(),
            applied_revision: 0,
            applied_placement: None,
            input_rect: None,
        }
    }
}

/// Mirror the authored recording contract onto the workbench status bus. This
/// remains generic: a surface is required only when its namespace has a live
/// exposure, so unrelated scenes do not wait for a HUD they do not author.
///
/// Readiness is evaluated at the end of Bevy's UI lifecycle. The root must have
/// passed target-camera propagation and layout, and the exposure revision must
/// have been applied. The recorder can therefore consume one semantic state
/// transition instead of guessing how many render passes a retained UI needs.
pub(crate) fn report_runtime_ui_readiness(
    exposures: Res<EngineExposures>,
    mut roots: Query<(
        &mut RuntimeUiSurface,
        &Visibility,
        Option<&TemplateProperties>,
        Option<&UiTargetCamera>,
        Option<&ComputedUiTargetCamera>,
        Option<&ComputedUiRenderTargetInfo>,
        Option<&ComputedNode>,
    )>,
    render_state: Option<Res<RuntimeUiRenderState>>,
    bus: Option<ResMut<lunco_workbench::status_bus::StatusBus>>,
) {
    let Some(mut bus) = bus else { return };

    let mut required = 0usize;
    let mut ready = 0usize;
    for (
        mut surface,
        visibility,
        properties,
        target_camera,
        computed_camera,
        render_target,
        computed_node,
    ) in &mut roots
    {
        let exposure_visible = exposures
            .surfaces
            .get(&surface.namespace)
            .is_some_and(|exposure| exposure.visible);
        if !surface.required_for_recording || !exposure_visible {
            surface.presentation_ready = false;
            continue;
        }
        required += 1;
        let target_ready = target_camera.is_some()
            && computed_camera.is_some_and(|camera| camera.get().is_some())
            && render_target.is_some_and(|target| {
                let size = target.physical_size();
                size.x > 0 && size.y > 0
            });
        let layout_ready = computed_node.is_some_and(|node| {
            node.size.x.is_finite()
                && node.size.y.is_finite()
                && node.size.x > 0.0
                && node.size.y > 0.0
        });
        let surface_ready = surface.mounted
            && surface.applied_placement.is_some()
            && matches!(*visibility, Visibility::Visible)
            && properties.is_some()
            && surface.applied_revision == exposures.revision
            && target_ready
            && layout_ready;
        if surface_ready {
            ready += 1;
        }
        surface.presentation_ready = surface_ready;
    }

    let render_ready = required == 0
        || render_state.is_some_and(|state| {
            state.extracted_revision == exposures.revision
                && state.extracted_surface_count == required as u32
        });
    if required == 0 || (ready == required && render_ready) {
        bus.remove_progress(lunco_workbench::status_bus::RUNTIME_UI_SOURCE);
    } else {
        let message = if ready < required {
            format!("mounting recorded UI surfaces {ready}/{required}")
        } else {
            "waiting for recorded UI render extraction".to_string()
        };
        bus.set_progress(
            lunco_workbench::status_bus::RUNTIME_UI_SOURCE,
            message,
            ready as u64,
            required as u64,
        );
    }
}

/// Observe the actual Bevy UI extraction boundary. `ExtractedUiNodes` is
/// populated only after target-camera propagation, layout, HUI/Flair styling,
/// and the UI extraction systems have all run. A positive acknowledgement here
/// lets the main-world readiness gate arm capture without a warm-up frame or a
/// discarded probe.
fn acknowledge_runtime_ui_render_extraction(
    mut main_world: ResMut<MainWorld>,
    extracted_nodes: Res<bevy::ui_render::ExtractedUiNodes>,
    mut render_ack: ResMut<RuntimeUiRenderAck>,
) {
    let visible_namespaces: HashSet<String> = main_world
        .get_resource::<EngineExposures>()
        .map(|exposures| {
            exposures
                .surfaces
                .iter()
                .filter(|(_, exposure)| exposure.visible)
                .map(|(namespace, _)| namespace.clone())
                .collect()
        })
        .unwrap_or_default();
    let required_roots: Vec<Entity> = {
        let mut roots = main_world.query::<(Entity, &RuntimeUiSurface)>();
        roots
            .iter(&main_world)
            .filter(|(_, surface)| {
                surface.required_for_recording && visible_namespaces.contains(&surface.namespace)
            })
            .map(|(entity, _)| entity)
            .collect()
    };

    let presentation_ready_roots = {
        let mut roots = main_world.query::<(Entity, &RuntimeUiSurface)>();
        roots
            .iter(&main_world)
            .filter(|(_, surface)| {
                surface.required_for_recording
                    && visible_namespaces.contains(&surface.namespace)
                    && surface.presentation_ready
            })
            .count()
    };

    let mut extracted_roots = HashSet::new();
    let mut parents = main_world.query::<&ChildOf>();
    for node in &extracted_nodes.uinodes {
        let mut current = node.main_entity.id();
        loop {
            if required_roots.contains(&current) {
                extracted_roots.insert(current);
                break;
            }
            let Ok(child_of) = parents.get(&main_world, current) else {
                break;
            };
            current = child_of.parent();
        }
    }

    let all_extracted = presentation_ready_roots == required_roots.len()
        && required_roots
            .iter()
            .all(|root| extracted_roots.contains(root));
    render_ack.extracted_revision = if all_extracted {
        main_world
            .get_resource::<EngineExposures>()
            .map_or(0, |exposures| exposures.revision)
    } else {
        0
    };
    render_ack.extracted_surface_count = if all_extracted {
        required_roots.len() as u32
    } else {
        0
    };
}

/// Copy the previous render submission acknowledgement into the simulation
/// world. This runs in extraction before this frame's UI nodes are built, so a
/// recorder start can only observe a completed prior presentation.
fn publish_runtime_ui_render_ack(
    mut main_world: ResMut<MainWorld>,
    render_ack: Res<RuntimeUiRenderAck>,
) {
    if let Some(mut state) = main_world.get_resource_mut::<RuntimeUiRenderState>() {
        state.extracted_revision = render_ack.submitted_revision;
        state.extracted_surface_count = render_ack.submitted_surface_count;
    }
}

/// Mark the UI revision submitted once Bevy's render schedule has completed its
/// render set. This is the handoff that replaces the old warm-up/probe logic.
fn acknowledge_runtime_ui_render_submission(mut render_ack: ResMut<RuntimeUiRenderAck>) {
    render_ack.submitted_revision = render_ack.extracted_revision;
    render_ack.submitted_surface_count = render_ack.extracted_surface_count;
}

/// Install the cross-world presentation acknowledgement after Bevy has
/// extracted all authored UI nodes for the current render frame.
pub(crate) fn install_runtime_ui_render_readiness(app: &mut App) {
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app.init_resource::<RuntimeUiRenderAck>();
    render_app.add_systems(
        ExtractSchedule,
        publish_runtime_ui_render_ack.before(bevy::ui_render::RenderUiSystems::ExtractCameraViews),
    );
    render_app.add_systems(
        ExtractSchedule,
        acknowledge_runtime_ui_render_extraction
            .after(bevy::ui_render::RenderUiSystems::ExtractDebug),
    );
    render_app.add_systems(
        Render,
        acknowledge_runtime_ui_render_submission.after(RenderSystems::Render),
    );
}

/// Load the authored surface manifest. The asset watcher can replace it while
/// the application is running; `sync_runtime_ui_manifest` then rebuilds only
/// the registered surface roots.
pub(crate) fn load_runtime_ui_manifest(mut commands: Commands, server: Res<AssetServer>) {
    commands.insert_resource(RuntimeUiManifestState {
        handle: server.load("ui/runtime_surfaces.json"),
        applied: None,
        rebuild_pending: false,
    });
}

pub(crate) fn sync_runtime_ui_manifest(
    mut commands: Commands,
    manifests: Res<Assets<RuntimeUiManifest>>,
    mut events: MessageReader<AssetEvent<RuntimeUiManifest>>,
    mut state: ResMut<RuntimeUiManifestState>,
    roots: Query<Entity, With<RuntimeUiSurface>>,
    server: Res<AssetServer>,
    mut functions: HtmlFunctions,
) {
    let changed = state.applied.is_none()
        || events.read().any(|event| {
            matches!(
                event,
                AssetEvent::Added { id }
                    | AssetEvent::Modified { id }
                    | AssetEvent::LoadedWithDependencies { id }
                    if *id == state.handle.id()
            )
        });
    if !changed {
        return;
    }
    let Some(manifest) = manifests.get(&state.handle) else {
        return;
    };
    if let Err(error) = manifest.validate() {
        error!("runtime UI manifest rejected: {error}");
        return;
    }

    for surface in &manifest.surfaces {
        for action in &surface.actions {
            let Ok(action_kind) = RuntimeUiActionKind::parse(&action.action) else {
                // `validate` above already checked this. Keep this branch
                // explicit so a future programmatic manifest cannot bypass
                // the closed action boundary.
                error!("runtime UI action rejected: `{}`", action.action);
                return;
            };
            register_action(&mut functions, action.callback.clone(), action_kind);
        }
    }

    for root in &roots {
        commands.entity(root).despawn_related::<Children>();
        commands.entity(root).despawn();
    }
    for definition in &manifest.surfaces {
        let template: Handle<HtmlTemplate> = server.load(definition.template.clone());
        let stylesheet: Handle<StyleSheet> = server.load(definition.stylesheet.clone());
        commands.spawn((
            Node::default(),
            RuntimeUiSurface::from_definition(definition, template, stylesheet),
            Visibility::Hidden,
        ));
    }
    state.applied = Some(state.handle.id());
    state.rebuild_pending = true;
}

/// Attach HUI/Flair only to surfaces that can currently be shown. Runtime
/// manifests may describe many optional overlays, but hidden trees still incur
/// HUI compilation, CSS matching, and Bevy UI layout cost if they are mounted.
/// The marker root remains alive so an exposure can activate it later without
/// rebuilding the manifest.
pub(crate) fn mount_runtime_ui_surfaces(
    mut commands: Commands,
    manifest_state: Res<RuntimeUiManifestState>,
    exposures: Res<EngineExposures>,
    layout: Option<Res<lunco_workbench::WorkbenchLayout>>,
    gates: Option<Res<RuntimeUiGates>>,
    rects: Option<Res<PanelRects>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut roots: Query<(
        Entity,
        &mut RuntimeUiSurface,
        Option<&ComputedUiRenderTargetInfo>,
    )>,
) {
    if manifest_state.rebuild_pending {
        return;
    }

    let window = windows.iter().next();
    for (entity, mut surface, target) in &mut roots {
        if surface.mounted {
            continue;
        }
        let Some(exposure) = exposures.surfaces.get(&surface.namespace) else {
            continue;
        };
        if !exposure.visible
            || !runtime_ui_is_allowed(&surface, layout.as_deref(), gates.as_deref())
            || resolve_placement(&surface.placement, rects.as_deref(), target, window).is_none()
        {
            continue;
        }

        commands.entity(entity).insert((
            HtmlNode(surface.template.clone()),
            Styled::new(surface.stylesheet.clone()),
            InlineStyle::default(),
        ));
        surface.mounted = true;
    }
}

fn runtime_ui_is_allowed(
    surface: &RuntimeUiSurface,
    layout: Option<&lunco_workbench::WorkbenchLayout>,
    gates: Option<&RuntimeUiGates>,
) -> bool {
    let perspective_visible = surface
        .visible_in_perspective
        .as_deref()
        .is_none_or(|required| {
            layout.is_some_and(|layout| {
                layout
                    .active_perspective()
                    .is_some_and(|active| active.as_str() == required)
            })
        });
    let gate_visible = surface
        .gate
        .as_deref()
        .is_none_or(|gate| gates.is_some_and(|gates| gates.allows(gate)));
    perspective_visible && gate_visible
}

/// Keep retained HTML surfaces on the presentation camera.
///
/// Windowed runs have a `PrimaryEguiContext` camera. Windowless recording does
/// not create a window or egui host, so the same authored surface is bound to
/// the active authored `SceneCamera` instead. This is what makes a runtime HUD
/// part of the captured render target rather than editor chrome.
pub(crate) fn bind_runtime_ui_to_camera(
    mut commands: Commands,
    cameras: Query<(Entity, &Camera, Has<PrimaryEguiContext>, Has<SceneCamera>)>,
    roots: Query<(Entity, Option<&UiTargetCamera>), With<RuntimeUiSurface>>,
) {
    let camera = cameras
        .iter()
        .find(|(_, _, is_egui, _)| *is_egui)
        .or_else(|| cameras.iter().find(|(_, _, _, is_scene)| *is_scene))
        .map(|(entity, _, _, _)| entity);
    let Some(camera) = camera else {
        return;
    };

    for (entity, target) in &roots {
        if target.is_none_or(|target| target.entity() != camera) {
            commands.entity(entity).insert(UiTargetCamera(camera));
        }
    }
}

/// Bridge HUI's stable IDs to Bevy names, which is the selector identity used
/// by Flair. This is shared by all runtime-authored templates.
pub(crate) fn attach_runtime_ui_names(
    mut commands: Commands,
    ids: Query<(Entity, &UiId), (With<Node>, Or<(Added<UiId>, Changed<UiId>)>)>,
) {
    for (entity, id) in &ids {
        commands.entity(entity).insert(Name::new(id.id().clone()));
    }
}

/// HUI's inline-style cache is not the style authority for runtime surfaces;
/// Flair's stylesheet is. Remove only HUI style components below a runtime
/// surface, leaving unrelated HUI templates untouched.
pub(crate) fn hand_runtime_ui_styling_to_flair(
    mut commands: Commands,
    roots: Query<Entity, With<RuntimeUiSurface>>,
    nodes: Query<(Entity, &HtmlStyle), Or<(Added<HtmlStyle>, Changed<HtmlStyle>)>>,
    parents: Query<&ChildOf>,
) {
    for (entity, _) in &nodes {
        let mut current = entity;
        let mut belongs_to_runtime_surface = false;
        loop {
            if roots.get(current).is_ok() {
                belongs_to_runtime_surface = true;
                break;
            }
            let Ok(child_of) = parents.get(current) else {
                break;
            };
            current = child_of.parent();
        }

        if belongs_to_runtime_surface {
            commands.entity(entity).remove::<HtmlStyle>();
        }
    }
}

/// Apply only changed exposure snapshots to retained HUI properties and Flair
/// custom properties. Parsing, entity creation, and style writes therefore do
/// not run on idle render frames.
pub(crate) fn apply_runtime_ui_exposures(
    mut commands: Commands,
    exposures: Res<EngineExposures>,
    mut manifest_state: ResMut<RuntimeUiManifestState>,
    layout: Option<Res<lunco_workbench::WorkbenchLayout>>,
    gates: Option<Res<RuntimeUiGates>>,
    rects: Option<Res<PanelRects>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut roots: Query<(
        Entity,
        &mut RuntimeUiSurface,
        &mut Node,
        &mut Visibility,
        Option<&mut TemplateProperties>,
        Option<&InlineStyle>,
        Option<&ComputedUiRenderTargetInfo>,
    )>,
) {
    // Manifest replacement despawns the previous retained roots and queues new
    // ones. Bevy applies those commands after the schedule; skip this one
    // presentation pass so no system queues a style command for a root that is
    // already scheduled for despawn.
    if manifest_state.rebuild_pending {
        manifest_state.rebuild_pending = false;
        return;
    }

    let layout_changed = layout.as_ref().is_some_and(|layout| layout.is_changed());
    let gates_changed = gates.as_ref().is_some_and(|gates| gates.is_changed());
    let window = windows.iter().next();

    for (entity, mut surface, mut node, mut visibility, properties, existing_style, target) in
        &mut roots
    {
        let placement = resolve_placement(&surface.placement, rects.as_deref(), target, window);
        let placement_changed = surface.applied_placement != placement;
        if surface.applied_revision == exposures.revision
            && !layout_changed
            && !gates_changed
            && !placement_changed
        {
            continue;
        }

        if let Some(placement) = placement {
            if placement_changed {
                apply_placement(&mut node, placement.rect);
            }
            surface.input_rect =
                (surface.interactive && placement.rect.is_positive()).then_some(placement.rect);
        } else {
            surface.input_rect = None;
        }
        surface.applied_placement = placement;

        let Some(exposure) = exposures.surfaces.get(&surface.namespace) else {
            *visibility = Visibility::Hidden;
            surface.applied_revision = exposures.revision;
            continue;
        };

        let perspective_visible = surface
            .visible_in_perspective
            .as_ref()
            .is_none_or(|required| {
                layout.as_ref().is_some_and(|layout| {
                    layout
                        .active_perspective()
                        .is_some_and(|active| active.as_str() == required)
                })
            });
        let gate_visible = surface
            .gate
            .as_deref()
            .is_none_or(|gate| gates.as_ref().is_some_and(|gates| gates.allows(gate)));
        *visibility =
            if exposure.visible && perspective_visible && gate_visible && placement.is_some() {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
        if !matches!(*visibility, Visibility::Visible) {
            surface.input_rect = None;
        }

        let Some(mut properties) = properties else {
            // The root is intentionally unmounted until its exposure becomes
            // visible. Do not advance the exposure revision before HUI has
            // supplied TemplateProperties, or the first mounted frame would
            // skip its initial projection.
            continue;
        };

        let mut properties_changed = false;
        let mut style = existing_style.cloned().unwrap_or_default();
        let mut style_changed = false;
        if surface.bindings.is_empty() {
            for (name, value) in &exposure.properties {
                apply_runtime_ui_property(
                    name,
                    value.render(),
                    &mut properties,
                    &mut style,
                    &mut properties_changed,
                    &mut style_changed,
                );
            }
        } else {
            for (target, binding) in &surface.bindings {
                let Some(value) = exposure.properties.get(&binding.source) else {
                    continue;
                };
                let source_value = value.render();
                let rendered = if binding.map.is_empty() {
                    source_value
                } else {
                    let Some(mapped) = binding.map.get(&source_value) else {
                        continue;
                    };
                    mapped.clone()
                };
                apply_runtime_ui_property(
                    target,
                    rendered,
                    &mut properties,
                    &mut style,
                    &mut properties_changed,
                    &mut style_changed,
                );
            }
        }

        if style_changed {
            commands.entity(entity).insert(style);
        }
        if properties_changed {
            commands.trigger(CompileContextEvent { entity });
        }
        surface.applied_revision = exposures.revision;
    }
}

/// Apply the manifest rectangle after HUI has finished replacing the template
/// root's [`Node`] and Flair has applied authored CSS.
///
/// This is change-detected on the root node. It runs for the initial HUI build
/// (and for an actual resize/style rebuild), then goes dormant; there is no
/// per-frame geometry write. The manifest owns the outer rectangle and the
/// stylesheet owns the contents of that rectangle.
pub(crate) fn apply_runtime_ui_placement_after_style(
    rects: Option<Res<PanelRects>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut roots: Query<
        (
            &mut RuntimeUiSurface,
            &mut Node,
            Option<&ComputedUiRenderTargetInfo>,
        ),
        Changed<Node>,
    >,
) {
    let window = windows.iter().next();
    for (mut surface, mut node, target) in &mut roots {
        let placement = resolve_placement(&surface.placement, rects.as_deref(), target, window);
        if surface.applied_placement != placement {
            surface.applied_placement = placement;
            surface.input_rect = if surface.interactive {
                placement
                    .filter(|placement| placement.rect.is_positive())
                    .map(|placement| placement.rect)
            } else {
                None
            };
        }
        let Some(placement) = placement else {
            continue;
        };
        let rect = placement.rect;
        let already_applied = node.position_type == PositionType::Absolute
            && node.left == Val::Px(rect.min.x)
            && node.top == Val::Px(rect.min.y)
            && node.width == Val::Px(rect.width())
            && node.height == Val::Px(rect.height());
        if !already_applied {
            apply_placement(&mut node, rect);
        }
    }
}

fn apply_runtime_ui_property(
    name: &str,
    rendered: String,
    properties: &mut TemplateProperties,
    style: &mut InlineStyle,
    properties_changed: &mut bool,
    style_changed: &mut bool,
) {
    // A capability is only projected when the authored template declares the
    // property. This prevents raw engine facts from becoming write-only UI
    // state and keeps the manifest the reader boundary.
    if properties.get(name).is_none() {
        return;
    }
    if properties.get(name).map(String::as_str) != Some(rendered.as_str()) {
        properties.set(name, &rendered);
        *properties_changed = true;
    }
    let css_name = format!("--ui-{}", name.replace('_', "-"));
    if rendered.trim().is_empty() {
        // Empty custom-property values are not a valid Flair token stream.
        // Keep the text property authoritative (an empty text value is valid),
        // but remove the inline CSS override so the authored stylesheet value
        // becomes effective again. This is an invariant at the presentation
        // boundary, not a replacement value or a producer-side fallback.
        if style.get(&css_name).is_some() {
            style.remove(&css_name);
            *style_changed = true;
        }
    } else if style.get(&css_name) != Some(rendered.as_str()) {
        style.set(css_name, rendered);
        *style_changed = true;
    }
}

/// Add the visible interactive runtime surfaces to the existing workbench
/// scene/chrome gate. This is intentionally a bridge into `ScenePickGate`, not
/// a second hit-test implementation: egui dock cards and runtime UI cards are
/// resolved by the same press latch and scene ownership state machine.
pub(crate) fn register_runtime_ui_input_regions(
    roots: Query<(&RuntimeUiSurface, &Visibility)>,
    mut gate: ResMut<ScenePickGate>,
) {
    for (surface, visibility) in &roots {
        if matches!(*visibility, Visibility::Visible) && surface.interactive {
            if let Some(rect) = surface.input_rect {
                gate.record_chrome_panel(rect, rect);
            }
        }
    }
}

fn resolve_placement(
    placement: &RuntimeUiPlacement,
    rects: Option<&PanelRects>,
    target: Option<&ComputedUiRenderTargetInfo>,
    window: Option<&Window>,
) -> Option<ResolvedRuntimeUiPlacement> {
    // The UI target component is propagated before the camera has a live
    // viewport on the first frame, so its default is a valid-looking 1.0 scale
    // paired with a 0x0 physical size. Do not let that placeholder shadow the
    // already-valid window dimensions; doing so makes a top-center surface
    // resolve to a negative x coordinate until the first resize event.
    let target_dimensions = target.and_then(|target| {
        let scale = target.scale_factor();
        let physical_size = target.physical_size();
        (scale.is_finite() && scale > 0.0 && physical_size.x > 0 && physical_size.y > 0)
            .then_some((scale, physical_size))
    });
    let (scale, physical_size) = target_dimensions
        .or_else(|| window.map(|window| (window.scale_factor(), window.physical_size())))?;
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    if physical_size.x == 0 || physical_size.y == 0 {
        return None;
    }
    let window_size = physical_size.as_vec2() / scale;
    let egui_vec2 = |value: Vec2| egui::vec2(value.x, value.y);
    let egui_pos2 = |value: Vec2| egui::pos2(value.x, value.y);

    match placement {
        RuntimeUiPlacement::Viewport => Some(ResolvedRuntimeUiPlacement {
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui_vec2(window_size)),
        }),
        RuntimeUiPlacement::DockPanel { panel, inset } => {
            let physical = rects?.get(*panel)?;
            let inset = inset.max(0.0);
            let origin = physical.origin.as_vec2() / scale + Vec2::splat(inset);
            let size = (physical.size.as_vec2() / scale - Vec2::splat(inset * 2.0)).max(Vec2::ONE);
            Some(ResolvedRuntimeUiPlacement {
                rect: egui::Rect::from_min_size(egui_pos2(origin), egui_vec2(size)),
            })
        }
        RuntimeUiPlacement::Window {
            anchor,
            offset,
            width,
            height,
        } => {
            let size = Vec2::new(width.max(1.0), height.max(1.0));
            let base = match anchor {
                RuntimeUiWindowAnchor::TopLeft => Vec2::ZERO,
                RuntimeUiWindowAnchor::TopCenter => Vec2::new((window_size.x - size.x) * 0.5, 0.0),
                RuntimeUiWindowAnchor::TopRight => Vec2::new(window_size.x - size.x, 0.0),
                RuntimeUiWindowAnchor::BottomLeft => Vec2::new(0.0, window_size.y - size.y),
                RuntimeUiWindowAnchor::BottomCenter => {
                    Vec2::new((window_size.x - size.x) * 0.5, window_size.y - size.y)
                }
                RuntimeUiWindowAnchor::BottomRight => {
                    Vec2::new(window_size.x - size.x, window_size.y - size.y)
                }
            };
            Some(ResolvedRuntimeUiPlacement {
                rect: egui::Rect::from_min_size(egui_pos2(base + *offset), egui_vec2(size)),
            })
        }
    }
}

fn apply_placement(node: &mut Node, rect: egui::Rect) {
    node.position_type = PositionType::Absolute;
    node.left = Val::Px(rect.min.x);
    node.top = Val::Px(rect.min.y);
    node.width = Val::Px(rect.width());
    node.height = Val::Px(rect.height());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_accepts_viewport_dock_and_window_surfaces() {
        let manifest: RuntimeUiManifest = serde_json::from_str(
            r#"{
                "surfaces": [
                    {
                        "id": "viewport",
                        "template": "ui/a.html",
                        "stylesheet": "ui/a.css",
                        "namespace": "a",
                        "placement": {"mode": "viewport"}
                    },
                    {
                        "id": "dock",
                        "template": "ui/b.html",
                        "stylesheet": "ui/b.css",
                        "namespace": "b",
                        "placement": {
                            "mode": "dock_panel",
                            "panel": "right_inspector",
                            "inset": 6.0
                        }
                    },
                    {
                        "id": "window",
                        "template": "ui/c.html",
                        "stylesheet": "ui/c.css",
                        "namespace": "c",
                        "interactive": true,
                        "placement": {
                            "mode": "window",
                            "anchor": "bottom_right",
                            "offset": [-4.0, -8.0],
                            "width": 240.0,
                            "height": 80.0
                        }
                    }
                ]
            }"#,
        )
        .expect("manifest should parse");

        manifest.validate().expect("manifest should validate");
        assert_eq!(manifest.surfaces.len(), 3);
        assert!(matches!(
            manifest.surfaces[0].placement,
            RuntimeUiPlacementDefinition::Viewport
        ));
        assert!(manifest.surfaces[2].interactive);
    }

    #[test]
    fn shipped_manifest_validates() {
        let manifest: RuntimeUiManifest =
            serde_json::from_str(include_str!("../../../../assets/ui/runtime_surfaces.json"))
                .expect("shipped runtime UI manifest should parse");
        manifest
            .validate()
            .expect("shipped runtime UI manifest should validate");
    }

    #[test]
    fn window_placement_uses_logical_points_and_anchor_offset() {
        let window = Window::default();
        let placement = RuntimeUiPlacement::Window {
            anchor: RuntimeUiWindowAnchor::BottomRight,
            offset: Vec2::new(-4.0, -8.0),
            width: 240.0,
            height: 80.0,
        };
        let resolved = resolve_placement(&placement, None, None, Some(&window))
            .expect("default window has a render size");

        assert_eq!(resolved.rect.width(), 240.0);
        assert_eq!(resolved.rect.height(), 80.0);
        assert!(resolved.rect.max.x <= window.width());
        assert!(resolved.rect.max.y <= window.height());
    }

    #[test]
    fn zero_sized_target_uses_live_window_dimensions() {
        let window = Window::default();
        let placement = RuntimeUiPlacement::Window {
            anchor: RuntimeUiWindowAnchor::TopCenter,
            offset: Vec2::new(0.0, 50.0),
            width: 350.0,
            height: 58.0,
        };
        let resolved = resolve_placement(
            &placement,
            None,
            Some(&ComputedUiRenderTargetInfo::default()),
            Some(&window),
        )
        .expect("the initialized window is a valid fallback target");

        assert_eq!(resolved.rect.min, egui::pos2(465.0, 50.0));
        assert_eq!(resolved.rect.size(), egui::vec2(350.0, 58.0));
    }

    #[test]
    fn dock_placement_uses_authoritative_panel_rect() {
        let mut rects = PanelRects::default();
        let panel = PanelId("right_inspector");
        rects.record(
            panel,
            lunco_workbench::PanelRect {
                origin: UVec2::new(800, 100),
                size: UVec2::new(400, 600),
            },
        );
        let window = Window::default();
        let placement = RuntimeUiPlacement::DockPanel { panel, inset: 10.0 };
        let resolved = resolve_placement(&placement, Some(&rects), None, Some(&window))
            .expect("recorded dock panel should resolve");

        assert_eq!(resolved.rect.min, egui::pos2(810.0, 110.0));
        assert_eq!(resolved.rect.size(), egui::vec2(380.0, 580.0));
    }

    #[test]
    fn unknown_gate_is_closed_until_authored_by_the_host() {
        let mut gates = RuntimeUiGates::default();
        assert!(!gates.allows("not-yet-published"));
        gates.set("not-yet-published", true);
        assert!(gates.allows("not-yet-published"));
    }

    #[test]
    fn manifest_rejects_unknown_fields() {
        let error = serde_json::from_str::<RuntimeUiManifest>(
            r#"{
                "surfaces": [{
                    "id": "surface",
                    "template": "ui/a.html",
                    "stylesheet": "ui/a.css",
                    "namespace": "surface",
                    "unexpected": true,
                    "placement": {"mode": "viewport"}
                }]
            }"#,
        )
        .expect_err("unknown manifest fields must be rejected");
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn manifest_validation_rejects_duplicate_identity_and_unknown_action() {
        let manifest: RuntimeUiManifest = serde_json::from_str(
            r#"{
                "surfaces": [
                    {
                        "id": "same",
                        "template": "ui/a.html",
                        "stylesheet": "ui/a.css",
                        "namespace": "one",
                        "placement": {"mode": "viewport"}
                    },
                    {
                        "id": "same",
                        "template": "ui/b.html",
                        "stylesheet": "ui/b.css",
                        "namespace": "two",
                        "actions": [{"callback": "button", "action": "not.allowed"}],
                        "placement": {"mode": "viewport"}
                    }
                ]
            }"#,
        )
        .expect("JSON shape should parse");
        let error = manifest.validate().expect_err("identity must be unique");
        assert!(error.contains("duplicate runtime UI surface id"));

        let unknown_action: RuntimeUiManifest = serde_json::from_str(
            r#"{
                "surfaces": [{
                    "id": "action-surface",
                    "template": "ui/a.html",
                    "stylesheet": "ui/a.css",
                    "namespace": "action-surface",
                    "actions": [{"callback": "button", "action": "not.allowed"}],
                    "placement": {"mode": "viewport"}
                }]
            }"#,
        )
        .expect("JSON shape should parse");
        let error = unknown_action
            .validate()
            .expect_err("unsupported action must be rejected");
        assert!(error.contains("unknown runtime UI action"));
    }

    #[test]
    fn manifest_validation_rejects_unsafe_asset_path_and_bad_geometry() {
        let manifest: RuntimeUiManifest = serde_json::from_str(
            r#"{
                "surfaces": [{
                    "id": "surface",
                    "template": "../outside.html",
                    "stylesheet": "ui/a.css",
                    "namespace": "surface",
                    "placement": {
                        "mode": "window",
                        "anchor": "top_left",
                        "width": 0.0,
                        "height": 10.0
                    }
                }]
            }"#,
        )
        .expect("JSON shape should parse");
        let error = manifest
            .validate()
            .expect_err("unsafe path must be rejected");
        assert!(error.contains("relative asset path"));
    }

    #[test]
    fn empty_exposure_text_does_not_reach_flair_as_css() {
        let mut properties = TemplateProperties::default().with("value", "previous");
        let mut style = InlineStyle::default();
        style.set("--ui-value", "previous");
        let mut properties_changed = false;
        let mut style_changed = false;

        apply_runtime_ui_property(
            "value",
            String::new(),
            &mut properties,
            &mut style,
            &mut properties_changed,
            &mut style_changed,
        );

        assert!(properties_changed);
        assert!(style_changed);
        assert_eq!(properties.get("value").map(String::as_str), Some(""));
        assert_eq!(style.get("--ui-value"), None);
    }
}
