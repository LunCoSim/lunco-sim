//! Generic retained exposure boundary for runtime-authored HTML UI.
//!
//! This module is deliberately domain-free. Engine systems publish named,
//! already-sampled values into [`EngineExposures`]; HUI/Flair consume that
//! snapshot and own the retained tree, layout, and styling. A template does not
//! know whether a value came from a port, telemetry, physics, a script, or a
//! derived engine capability.
use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, PrimaryEguiContext};
use bevy_flair::prelude::{InlineStyle, StyleSheet, Styled};
use bevy_hui::prelude::{
    CompileContextEvent, HtmlFunctions, HtmlNode, HtmlStyle, HtmlTemplate, TemplateProperties, UiId,
};
use lunco_core::exposure::EngineExposures;
use lunco_workbench::{PanelId, PanelRects, ScenePickGate};
use serde::Deserialize;
use std::collections::HashMap;

/// A semantic action emitted by an authored runtime surface.
///
/// HUI deliberately passes only the pressed element to a bound function. The
/// bridge closes over the authored action name and turns that callback into a
/// typed event, so application code never needs to inspect HTML ids or mutate
/// simulation state from a template callback.
#[derive(Event, Clone, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeUiAction {
    /// Stable action name authored by the surface adapter.
    pub action: String,
    /// The retained HTML entity that emitted the action.
    pub source: Entity,
}

/// Bind one HUI callback name to a semantic runtime action.
pub(crate) fn register_action(
    functions: &mut HtmlFunctions,
    callback: &'static str,
    action: &'static str,
) {
    let action = action.to_owned();
    functions.register(
        callback,
        move |In(source): In<Entity>, mut commands: Commands| {
            commands.trigger(RuntimeUiAction {
                action: action.clone(),
                source,
            });
        },
    );
}

/// Authored registration for runtime UI surfaces.
#[derive(Asset, Deserialize, TypePath, Debug, Clone)]
pub(crate) struct RuntimeUiManifest {
    pub surfaces: Vec<RuntimeUiSurfaceDefinition>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RuntimeUiSurfaceDefinition {
    pub id: String,
    pub template: String,
    pub stylesheet: String,
    pub namespace: String,
    #[serde(default)]
    pub visible_in_perspective: Option<String>,
    #[serde(default)]
    pub gate: Option<String>,
    #[serde(default)]
    pub interactive: bool,
    pub placement: RuntimeUiPlacementDefinition,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
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
        serde_json::from_slice(&bytes)
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
    visible_in_perspective: Option<String>,
    gate: Option<String>,
    interactive: bool,
    placement: RuntimeUiPlacement,
    applied_revision: u64,
    applied_placement: Option<ResolvedRuntimeUiPlacement>,
    input_rect: Option<egui::Rect>,
}

impl RuntimeUiSurface {
    fn from_definition(definition: &RuntimeUiSurfaceDefinition) -> Self {
        Self {
            namespace: definition.namespace.clone(),
            visible_in_perspective: definition.visible_in_perspective.clone(),
            gate: definition.gate.clone(),
            interactive: definition.interactive,
            placement: (&definition.placement).into(),
            applied_revision: 0,
            applied_placement: None,
            input_rect: None,
        }
    }
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

    for root in &roots {
        commands.entity(root).despawn_related::<Children>();
        commands.entity(root).despawn();
    }
    for definition in &manifest.surfaces {
        let template: Handle<HtmlTemplate> = server.load(definition.template.clone());
        let stylesheet: Handle<StyleSheet> = server.load(definition.stylesheet.clone());
        commands.spawn((
            Node::default(),
            HtmlNode(template),
            Styled::new(stylesheet),
            InlineStyle::default(),
            RuntimeUiSurface::from_definition(definition),
            Name::new(format!("runtime-ui:{}", definition.id)),
            Visibility::Hidden,
        ));
    }
    state.applied = Some(state.handle.id());
    state.rebuild_pending = true;
}

/// Keep retained HTML surfaces on the same window-targeting camera as egui.
pub(crate) fn bind_runtime_ui_to_camera(
    mut commands: Commands,
    cameras: Query<Entity, With<PrimaryEguiContext>>,
    roots: Query<(Entity, Option<&UiTargetCamera>), With<RuntimeUiSurface>>,
) {
    let Some(camera) = cameras.iter().next() else {
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
    ids: Query<(Entity, &UiId), (With<Node>, Without<Name>)>,
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
    nodes: Query<(Entity, &HtmlStyle)>,
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
        &mut TemplateProperties,
        &InlineStyle,
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

    for (entity, mut surface, mut node, mut visibility, mut properties, existing_style, target) in
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

        let mut properties_changed = false;
        let mut style = existing_style.clone();
        let mut style_changed = false;
        for (name, value) in &exposure.properties {
            let rendered = value.render();
            if properties.get(name).map(String::as_str) != Some(rendered.as_str()) {
                properties.set(name, &rendered);
                properties_changed = true;
            }
            let css_name = format!("--ui-{}", name.replace('_', "-"));
            if style.get(&css_name) != Some(rendered.as_str()) {
                style.set(css_name, rendered);
                style_changed = true;
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
    let scale = target
        .map(ComputedUiRenderTargetInfo::scale_factor)
        .or_else(|| window.map(Window::scale_factor))?;
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let physical_size = target
        .map(ComputedUiRenderTargetInfo::physical_size)
        .or_else(|| {
            window.map(|window| {
                UVec2::new(
                    window.resolution.physical_width(),
                    window.resolution.physical_height(),
                )
            })
        })?;
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

        assert_eq!(manifest.surfaces.len(), 3);
        assert!(matches!(
            manifest.surfaces[0].placement,
            RuntimeUiPlacementDefinition::Viewport
        ));
        assert!(manifest.surfaces[2].interactive);
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
}
