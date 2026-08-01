//! Generic retained exposure boundary for runtime-authored HTML UI.
//!
//! This module is deliberately domain-free. Engine systems publish named,
//! already-sampled values into [`EngineExposures`]; HUI/Flair consume that
//! snapshot and own the retained tree, layout, and styling. A template does not
//! know whether a value came from a port, telemetry, physics, a script, or a
//! derived engine capability.
use bevy::prelude::*;
use bevy_egui::PrimaryEguiContext;
use bevy_flair::prelude::{InlineStyle, StyleSheet, Styled};
use bevy_hui::prelude::{
    CompileContextEvent, HtmlNode, HtmlStyle, HtmlTemplate, TemplateProperties, UiId,
};
use lunco_core::exposure::EngineExposures;

/// Root marker for a retained runtime-authored HTML surface. The namespace is
/// the only contract between an engine exposure producer and a template.
#[derive(Component, Debug)]
pub(crate) struct RuntimeUiSurface {
    namespace: String,
    visible_in_perspective: Option<lunco_workbench::PerspectiveId>,
    applied_revision: u64,
}

impl RuntimeUiSurface {
    pub(crate) fn new(
        namespace: &str,
        visible_in_perspective: Option<lunco_workbench::PerspectiveId>,
    ) -> Self {
        Self {
            namespace: namespace.to_owned(),
            visible_in_perspective,
            applied_revision: 0,
        }
    }
}

/// Spawn one runtime-authored template on the stable egui host camera.
pub(crate) fn spawn_html_surface(
    commands: &mut Commands,
    server: &AssetServer,
    template_path: &str,
    stylesheet_path: &str,
    namespace: &str,
    visible_in_perspective: Option<lunco_workbench::PerspectiveId>,
) {
    let template: Handle<HtmlTemplate> = server.load(template_path.to_owned());
    let stylesheet: Handle<StyleSheet> = server.load(stylesheet_path.to_owned());

    commands.spawn((
        Node::default(),
        HtmlNode(template),
        Styled::new(stylesheet),
        InlineStyle::default(),
        RuntimeUiSurface::new(namespace, visible_in_perspective),
        Visibility::Hidden,
    ));
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
    layout: Option<Res<lunco_workbench::WorkbenchLayout>>,
    mut roots: Query<(
        Entity,
        &mut RuntimeUiSurface,
        &mut Visibility,
        &mut TemplateProperties,
        &InlineStyle,
    )>,
) {
    let layout_changed = layout.as_ref().is_some_and(|layout| layout.is_changed());
    for (entity, mut surface, mut visibility, mut properties, existing_style) in &mut roots {
        if surface.applied_revision == exposures.revision && !layout_changed {
            continue;
        }

        let Some(exposure) = exposures.surfaces.get(&surface.namespace) else {
            *visibility = Visibility::Hidden;
            surface.applied_revision = exposures.revision;
            continue;
        };

        let perspective_visible = surface.visible_in_perspective.is_none_or(|required| {
            layout
                .as_ref()
                .is_some_and(|layout| layout.active_perspective() == Some(required))
        });
        *visibility = if exposure.visible && perspective_visible {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };

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
