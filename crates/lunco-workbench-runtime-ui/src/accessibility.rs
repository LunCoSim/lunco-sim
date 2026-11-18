//! Keyboard and assistive activation share the exact callback lease used by pointer input.
//! Native Bevy accessibility owns geometry; labels come from authored text or aria_label.

use super::*;
use bevy::a11y::{AccessibilityNode, ActionRequest};
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::input_focus::{FocusCause, FocusedInput, InputFocus, InputFocusVisible};

/// Accessible command-menu entries retain the exact widget entity generation.
#[derive(Resource, Default)]
struct AccessibleControls(std::collections::HashMap<Entity, (String, bool)>);
impl lunco_core::SceneResource for AccessibleControls {}

#[derive(Event)]
pub(super) struct ActivateControl(pub(super) Entity);

fn surface_active(
    entity: Entity,
    roots: &Query<(&RuntimeUiSurface, &Visibility)>,
    parents: &Query<&ChildOf>,
) -> bool {
    let mut current = entity;
    loop {
        if let Ok((surface, visibility)) = roots.get(current) {
            return surface.interactive && *visibility == Visibility::Visible;
        }
        let Ok(parent) = parents.get(current) else {
            return false;
        };
        current = parent.parent();
    }
}

fn activate_control(
    event: On<ActivateControl>,
    controls: Query<(
        &RuntimeUiPressCallbacks,
        &InheritedVisibility,
        Has<bevy::ui::InteractionDisabled>,
    )>,
    roots: Query<(&RuntimeUiSurface, &Visibility)>,
    parents: Query<&ChildOf>,
    bindings: Res<FunctionBindings>,
    mut commands: Commands,
) {
    let Ok((callbacks, visible, disabled)) = controls.get(event.0) else {
        return;
    };
    if disabled || !visible.get() || !surface_active(event.0, &roots, &parents) {
        return;
    }
    for callback in &callbacks.0 {
        bindings.maybe_run(callback, event.0, &mut commands);
    }
}

fn key_activate(
    mut event: On<FocusedInput<KeyboardInput>>,
    mut contexts: Query<&mut bevy_egui::EguiContext, With<bevy_egui::PrimaryEguiContext>>,
    controls: Query<(), With<RuntimeUiPressCallbacks>>,
    mut commands: Commands,
) {
    let input = &event.input;
    if contexts
        .single_mut()
        .is_ok_and(|mut context| context.get_mut().egui_wants_keyboard_input())
    {
        return;
    }
    if input.repeat
        || input.state != ButtonState::Pressed
        || !matches!(input.key_code, KeyCode::Enter | KeyCode::Space)
        || !controls.contains(event.focused_entity)
    {
        return;
    }
    event.propagate(false);
    commands.trigger(ActivateControl(event.focused_entity));
}

fn assistive_activate(
    mut events: MessageReader<ActionRequest>,
    native_tree: Res<bevy::a11y::ManageAccessibilityUpdates>,
    controls: Query<(
        &RuntimeUiPressCallbacks,
        &InheritedVisibility,
        Has<bevy::ui::InteractionDisabled>,
    )>,
    roots: Query<(&RuntimeUiSurface, &Visibility)>,
    parents: Query<&ChildOf>,
    mut focus: ResMut<InputFocus>,
    mut commands: Commands,
) {
    if !native_tree.get() {
        events.clear();
        return;
    }
    for event in events.read() {
        let Some(entity) = Entity::try_from_bits(event.target_node.0) else {
            continue;
        };
        let Ok((_callbacks, visible, disabled)) = controls.get(entity) else {
            continue;
        };
        if disabled || !visible.get() || !surface_active(entity, &roots, &parents) {
            continue;
        }
        match event.action {
            accesskit::Action::Focus => focus.set(entity, FocusCause::Navigated),
            accesskit::Action::Click => commands.trigger(ActivateControl(entity)),
            _ => {}
        }
    }
}

fn sync_nodes(
    mut commands: Commands,
    mut catalogue: ResMut<AccessibleControls>,
    new_roots: Query<Entity, Added<RuntimeUiSurface>>,
    roots: Query<(&RuntimeUiSurface, &Visibility)>,
    parents: Query<&ChildOf>,
    changed_surfaces: Query<
        (),
        (
            With<RuntimeUiSurface>,
            Or<(Changed<RuntimeUiSurface>, Changed<Visibility>)>,
        ),
    >,
    controls: Query<(
        Entity,
        Ref<RuntimeUiPressCallbacks>,
        Option<&Tags>,
        Option<&AccessibilityNode>,
        Ref<InheritedVisibility>,
        Has<bevy::ui::InteractionDisabled>,
    )>,
    children: Query<&Children>,
    texts: Query<&Text>,
    changed_text: Query<(), (With<Text>, Changed<Text>)>,
    changed_tags: Query<(), (With<RuntimeUiPressCallbacks>, Changed<Tags>)>,
    changed_disabled: Query<
        (),
        (
            With<RuntimeUiPressCallbacks>,
            Added<bevy::ui::InteractionDisabled>,
        ),
    >,
    changed_hierarchy: Query<(), Or<(Changed<Children>, Changed<ChildOf>)>>,
    removed: (
        RemovedComponents<bevy::ui::InteractionDisabled>,
        RemovedComponents<RuntimeUiPressCallbacks>,
        RemovedComponents<Tags>,
        RemovedComponents<Children>,
    ),
) {
    let (mut removed_disabled, mut removed_callbacks, mut removed_tags, mut removed_children) =
        removed;
    for root in &new_roots {
        commands
            .entity(root)
            .try_insert(bevy::input_focus::tab_navigation::TabGroup::default());
    }
    for entity in removed_callbacks.read() {
        commands
            .entity(entity)
            .try_remove::<AccessibilityNode>()
            .try_remove::<bevy::input_focus::tab_navigation::TabIndex>();
        catalogue.0.remove(&entity);
    }
    catalogue
        .0
        .retain(|entity, _| controls.get(*entity).is_ok());
    let disabled_removed = removed_disabled.read().count() != 0;
    let tags_removed = removed_tags.read().count() != 0;
    let children_removed = removed_children.read().count() != 0;
    let content_changed = !changed_surfaces.is_empty()
        || !changed_text.is_empty()
        || !changed_tags.is_empty()
        || !changed_disabled.is_empty()
        || !changed_hierarchy.is_empty()
        || disabled_removed
        || tags_removed
        || children_removed;
    for (entity, callbacks, tags, node, visible, disabled) in &controls {
        let active = visible.get() && surface_active(entity, &roots, &parents);
        if node.is_some()
            && active
            && catalogue
                .0
                .get(&entity)
                .is_some_and(|(_, enabled)| *enabled == !disabled)
            && !callbacks.is_changed()
            && !visible.is_changed()
            && !content_changed
        {
            continue;
        }
        if !active {
            catalogue.0.remove(&entity);
            if node.is_some() {
                commands
                    .entity(entity)
                    .try_remove::<AccessibilityNode>()
                    .try_remove::<bevy::input_focus::tab_navigation::TabIndex>();
            }
            continue;
        }
        let label = tags
            .and_then(|tags| tags.get("aria_label"))
            .cloned()
            .unwrap_or_else(|| {
                std::iter::once(entity)
                    .chain(children.iter_descendants(entity))
                    .filter_map(|entity| texts.get(entity).ok().map(|text| text.0.clone()))
                    .collect::<Vec<_>>()
                    .join(" ")
            });
        let description = tags
            .and_then(|tags| tags.get("aria_description"))
            .filter(|description| !description.trim().is_empty());
        let menu_label = description.map_or_else(
            || label.clone(),
            |description| format!("{label} · {description}"),
        );
        let enabled = active && !disabled;
        let entry = (menu_label, enabled);
        if !active {
            catalogue.0.remove(&entity);
        } else if catalogue.0.get(&entity) != Some(&entry) {
            catalogue.0.insert(entity, entry);
        }
        let mut node = accesskit::Node::new(accesskit::Role::Button);
        if !label.trim().is_empty() {
            node.set_label(label);
        }
        if let Some(description) = description {
            node.set_description(description.clone());
        }
        node.add_action(accesskit::Action::Focus);
        node.add_action(accesskit::Action::Click);
        if !active {
            node.set_hidden();
        }
        if !enabled {
            node.set_disabled();
        }
        if enabled {
            commands
                .entity(entity)
                .try_insert(bevy::input_focus::tab_navigation::TabIndex(
                    tags.and_then(|tags| tags.get("ui_tab_index"))
                        .and_then(|value| value.parse::<i32>().ok())
                        .unwrap_or(0),
                ));
        } else {
            commands
                .entity(entity)
                .try_remove::<bevy::input_focus::tab_navigation::TabIndex>();
        }
        commands.entity(entity).try_insert(AccessibilityNode(node));
    }
}

fn show_focus(
    mut contexts: Query<&mut bevy_egui::EguiContext, With<bevy_egui::PrimaryEguiContext>>,
    focus: Res<InputFocus>,
    visible: Res<InputFocusVisible>,
    controls: Query<
        (&ComputedNode, &UiGlobalTransform, &InheritedVisibility),
        With<RuntimeUiPressCallbacks>,
    >,
    theme: Res<lunco_theme::Theme>,
) {
    if !visible.0 {
        return;
    }
    let Some(entity) = focus.get() else {
        return;
    };
    let Ok((node, transform, inherited)) = controls.get(entity) else {
        return;
    };
    if !inherited.get() {
        return;
    }
    let Some(rect) = runtime_ui_input_rect(node, transform) else {
        return;
    };
    let Ok(mut context) = contexts.single_mut() else {
        return;
    };
    let ctx = context.get_mut();
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("runtime_ui_keyboard_focus"),
    ))
    .rect_stroke(
        rect.expand(2.0),
        theme.rounding.button,
        egui::Stroke::new(2.0, theme.tokens.accent),
        egui::StrokeKind::Outside,
    );
}

pub(super) fn install(app: &mut App) {
    app.init_scene_resource::<AccessibleControls>()
        .add_observer(activate_control)
        .init_resource::<lunco_workbench_core::WorkbenchMenuRegistry>();
    app.world_mut()
        .resource_mut::<lunco_workbench_core::WorkbenchMenuRegistry>()
        .register_custom_menu("Controls", |ui, ctx| {
            let mut controls: Vec<_> = ctx
                .resource::<AccessibleControls>()
                .map(|catalogue| {
                    catalogue
                        .0
                        .iter()
                        .filter(|(_, (label, _))| !label.trim().is_empty())
                        .map(|(entity, (label, enabled))| (*entity, label.clone(), *enabled))
                        .collect()
                })
                .unwrap_or_default();
            controls.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            if controls.is_empty() {
                ui.weak("No authored controls are visible.");
            }
            for (entity, label, enabled) in controls {
                if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                    ctx.trigger(ActivateControl(entity));
                    ui.close();
                }
            }
        });
    app.init_resource::<bevy::a11y::ManageAccessibilityUpdates>()
        .add_message::<ActionRequest>()
        .add_observer(key_activate)
        .add_systems(
            Update,
            (
                assistive_activate,
                sync_nodes.after(register_runtime_ui_input_regions),
                show_focus,
            )
                .in_set(lunco_core::RuntimeCycleSet::Ui),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct Invocations(Vec<Entity>);

    // This tests the generic callback/entity-generation seam without authored assets.
    #[test]
    fn accessible_callbacks_require_a_live_enabled_surface_lease() {
        let mut app = App::new();
        app.init_resource::<FunctionBindings>()
            .init_resource::<Invocations>()
            .init_resource::<AccessibleControls>()
            .add_observer(activate_control)
            .add_systems(Update, sync_nodes);
        let callback = app.world_mut().register_system(
            |In(entity): In<Entity>, mut calls: ResMut<Invocations>| {
                calls.0.push(entity);
            },
        );
        app.world_mut()
            .resource_mut::<FunctionBindings>()
            .register("activate", callback);
        let definition: RuntimeUiSurfaceDefinition = serde_json::from_str(r#"{
            "id":"control", "template":"control.html", "stylesheet":"control.css", "namespace":"control", "interactive":true, "placement":{"mode":"viewport"}
        }"#).unwrap();
        let root = app
            .world_mut()
            .spawn((
                RuntimeUiSurface::from_definition(
                    &definition,
                    Handle::default(),
                    Handle::default(),
                    None,
                )
                .unwrap(),
                Visibility::Visible,
            ))
            .id();
        let control = app
            .world_mut()
            .spawn((
                RuntimeUiPressCallbacks(vec!["activate".into()]),
                InheritedVisibility::VISIBLE,
                ChildOf(root),
            ))
            .id();
        app.update();
        assert!(
            app.world()
                .resource::<AccessibleControls>()
                .0
                .contains_key(&control)
        );
        app.world_mut()
            .resource_mut::<AccessibleControls>()
            .0
            .clear();
        app.update();
        assert!(
            app.world()
                .resource::<AccessibleControls>()
                .0
                .contains_key(&control),
            "surviving application controls republish after scene-cache retirement"
        );
        app.world_mut().trigger(ActivateControl(control));
        app.world_mut().flush();
        assert_eq!(app.world().resource::<Invocations>().0, [control]);
        app.world_mut()
            .entity_mut(control)
            .insert(bevy::ui::InteractionDisabled);
        app.world_mut().trigger(ActivateControl(control));
        app.world_mut().flush();
        app.world_mut()
            .entity_mut(control)
            .remove::<bevy::ui::InteractionDisabled>();
        *app.world_mut().get_mut::<Visibility>(root).unwrap() = Visibility::Hidden;
        app.world_mut().trigger(ActivateControl(control));
        app.world_mut().flush();
        *app.world_mut().get_mut::<Visibility>(root).unwrap() = Visibility::Visible;
        app.world_mut()
            .get_mut::<RuntimeUiSurface>(root)
            .unwrap()
            .interactive = false;
        app.world_mut().trigger(ActivateControl(control));
        app.world_mut().flush();
        app.world_mut()
            .get_mut::<RuntimeUiSurface>(root)
            .unwrap()
            .interactive = true;
        let mut tags = Tags::default();
        tags.insert("ui_tab_index".into(), "7".into());
        tags.insert("aria_label".into(), "Action".into());
        app.world_mut().entity_mut(control).insert(tags);
        app.update();
        assert_eq!(
            app.world()
                .get::<bevy::input_focus::tab_navigation::TabIndex>(control)
                .unwrap()
                .0,
            7
        );
        assert_eq!(
            app.world().resource::<AccessibleControls>().0[&control].0,
            "Action"
        );
        app.world_mut().entity_mut(control).remove::<Tags>();
        app.update();
        assert_eq!(
            app.world().resource::<AccessibleControls>().0[&control].0,
            ""
        );
        let label = app
            .world_mut()
            .spawn((Text::new("Child label"), ChildOf(control)))
            .id();
        app.update();
        assert_eq!(
            app.world().resource::<AccessibleControls>().0[&control].0,
            "Child label"
        );
        app.world_mut().entity_mut(label).remove::<ChildOf>();
        app.update();
        assert_eq!(
            app.world().resource::<AccessibleControls>().0[&control].0,
            ""
        );
        app.world_mut().entity_mut(control).remove::<ChildOf>();
        app.update();
        assert!(
            !app.world()
                .resource::<AccessibleControls>()
                .0
                .contains_key(&control)
        );
        assert!(app.world().get::<AccessibilityNode>(control).is_none());
        assert!(
            app.world()
                .get::<bevy::input_focus::tab_navigation::TabIndex>(control)
                .is_none()
        );
        app.world_mut().trigger(ActivateControl(control));
        app.world_mut().flush();
        app.world_mut().entity_mut(control).insert(ChildOf(root));
        app.update();
        assert!(
            app.world()
                .resource::<AccessibleControls>()
                .0
                .contains_key(&control)
        );
        app.world_mut()
            .entity_mut(control)
            .remove::<RuntimeUiPressCallbacks>();
        app.update();
        assert!(
            !app.world()
                .resource::<AccessibleControls>()
                .0
                .contains_key(&control)
        );
        assert!(app.world().get::<AccessibilityNode>(control).is_none());
        assert!(
            app.world()
                .get::<bevy::input_focus::tab_navigation::TabIndex>(control)
                .is_none()
        );
        app.world_mut().entity_mut(control).despawn();
        app.world_mut().trigger(ActivateControl(control));
        app.world_mut().flush();
        assert_eq!(
            app.world().resource::<Invocations>().0,
            [control],
            "disabled, hidden, noninteractive and retired leases cannot invoke a callback"
        );
    }
}
