//! Generic retained exposure boundary for runtime-authored HTML UI.
//!
//! This crate is deliberately domain-free. Engine systems publish named,
//! already-sampled values into [`EngineExposures`]; HUI/Flair consume that
//! snapshot and own the retained tree, layout, and styling. A template does not
//! know whether a value came from a port, telemetry, physics, a script, or a
//! derived engine capability.
//!
//! Twin-authored forms use the same retained HUI tree: put `tag:ui_form` and
//! `tag:ui_field` on an `<input type="text">` or `<input type="number">`.
//! Legacy `<text>` fields can opt in with `tag:ui_input="text|number"`.
//! Use `tag:ui_value` for labels projected from the form state,
//! `tag:ui_status="true"` for status text, and
//! `tag:ui_scroll_area="vertical"` on a scrollable `<node>`. A button tagged
//! with `ui_form` receives the current form snapshot as `parameters.fields`
//! alongside its authored action tags. Values remain strings here so each Twin
//! can own its validation and domain rules. Input fields can set an integer
//! `tag:ui_tab_index`; the runtime adds them to the surface's keyboard tab order.
//! Use `tag:class` for Flair CSS classes; the runtime maps it to Flair's
//! `ClassList` component. A draggable window moves only when a pointer drag
//! starts on a descendant tagged `tag:ui_drag_handle="true"`.
use bevy::ecs::entity::EntityHashSet;
use bevy::picking::Pickable;
use bevy::picking::events::{PointerClick, PointerDrag};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::render::{ExtractSchedule, MainWorld, Render, RenderApp, RenderSystems};
use bevy::text::{EditableText, EditableTextFilter, EditableTextSystems};
use bevy::ui::picking_backend::{UiPickingCamera, UiPickingSettings};
use bevy::window::PrimaryWindow;
use bevy_egui::{PrimaryEguiContext, egui};
use bevy_flair::prelude::{ClassList, InlineStyle, StyleSheet, Styled};
use bevy_hui::prelude::{
    CompileContextEvent, FunctionBindings, HtmlFunctions, HtmlNode, HtmlStyle, HtmlTemplate,
    OnUiPress, Tags, TemplateProperties, UiId,
};
use lunco_assets_runtime::{JsonAssetContent, JsonAssetScopeChanged};
use lunco_command_contracts::{Ack, OpId};
use lunco_core::AppSceneExt;
use lunco_exposure_core::{EngineExposures, ExposureValue};
use lunco_hooks::HookValue;
use lunco_render::SceneCamera;
use lunco_viewport_core::SceneViewport;
use lunco_workbench_core::scene_pick::ScenePickGate;
use lunco_workbench_core::viewport::PanelRects;
use lunco_workbench_core::{PanelId, WorkbenchSnapshot};
use lunco_workbench_state::{RuntimeSurfaceLayout, RuntimeSurfaceLayouts};
use lunco_workspace::{TwinClosed, TwinId, WorkspaceResource};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};

const RUNTIME_UI_MANIFEST_KIND: &str = "lunco.runtime-ui-manifest.v1";

lunco_hooks::declare_hook! {
    id: "runtime.ui.recording",
    owner: "lunco-workbench-runtime-ui",
    description: "Select visible runtime UI surface ids for an active recording.",
    signature: [facts: Map],
    output: ArrayOfString,
    deterministic: false,
    required: false,
    installable: true,
}

/// A semantic action emitted by an authored runtime surface.
///
/// The runtime transports the authored identifier without interpreting its
/// domain meaning. The host or Rhai policy owns the resulting command/event.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct RuntimeUiAction {
    /// Owner captured when input emitted the action, before deferred delivery.
    pub scope: RuntimeUiActionScope,
    /// Twin-authored semantic action identifier.
    pub action: String,
    /// Typed parameter map attached by the authored node's tags. The runtime
    /// transports this generic value without assigning domain meaning.
    pub parameters: HookValue,
    /// HUI node that emitted the action.
    pub source: Entity,
}

/// Values and operator status for a Twin-authored form embedded in HUI markup.
/// Forms are keyed by the Twin that published them so a delayed action cannot
/// read values from a different active Twin.
#[derive(Resource, Default)]
pub struct RuntimeUiForms {
    owner_twin_id: Option<u64>,
    forms: BTreeMap<String, RuntimeUiFormState>,
    published_collections: HashSet<(String, String)>,
}

#[derive(Default)]
struct RuntimeUiFormState {
    values: BTreeMap<String, String>,
    status: String,
    needs_projection: bool,
}

#[derive(Component, Clone, Debug)]
struct RuntimeUiTextInput {
    owner_twin_id: u64,
    form_id: String,
    field_id: String,
}

#[derive(Component)]
struct RuntimeUiScrollArea;

/// Set or patch a Twin-authored HTML form's current values. Values are strings
/// at the form boundary; Twin policy owns domain parsing and validation.
#[lunco_core::Command(default)]
pub struct SetRuntimeUiFormValues {
    pub form_id: String,
    pub values: BTreeMap<String, String>,
}

/// Replace one authored keyed collection with rows from the active Twin's
/// presentation policy. The destination must be declared by the active
/// runtime UI manifest; the command cannot publish arbitrary exposures.
#[lunco_core::Command(default)]
pub struct SetRuntimeUiCollectionData {
    pub namespace: String,
    pub source: String,
    pub items: Vec<BTreeMap<String, String>>,
}

/// Set the inline status text for a Twin-authored HTML form.
#[lunco_core::Command(default)]
pub struct SetRuntimeUiFormStatus {
    pub form_id: String,
    pub status: String,
}

/// Read Twin-scoped form values for runtime integration tests and diagnostics.
#[lunco_core::Command(default)]
pub struct InspectRuntimeUiForm {
    pub form_id: String,
}

/// Origin lifetime of semantic UI input. A deferred event cannot acquire the
/// next Twin or scene merely because its handler runs after a reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeUiActionScope {
    /// Workspace Twin active when input occurred, including no active Twin.
    pub owner_twin_id: Option<u64>,
    /// Optional scene target, independent of the UI's execution cadence.
    pub lifetime: lunco_core::InteractionLifetime,
}

impl RuntimeUiActionScope {
    /// Capture the authoritative workspace and scene publications at input time.
    pub fn capture(
        workspace: Option<&lunco_workspace::WorkspaceResource>,
        scene: Option<&lunco_core::SceneTransitionCoordinator>,
    ) -> Self {
        Self {
            owner_twin_id: workspace.and_then(|w| w.active_twin).map(|id| id.raw()),
            lifetime: lunco_core::InteractionLifetime::capture_scene(scene),
        }
    }

    /// Capture an application/Twin/provider action without a scene target.
    pub fn capture_owner(workspace: Option<&lunco_workspace::WorkspaceResource>) -> Self {
        Self {
            owner_twin_id: workspace.and_then(|w| w.active_twin).map(|id| id.raw()),
            lifetime: lunco_core::InteractionLifetime::Owner,
        }
    }

    /// Reject stale input before interpreting any semantic action.
    pub fn validate(
        &self,
        workspace: Option<&lunco_workspace::WorkspaceResource>,
        scene: Option<&lunco_core::SceneTransitionCoordinator>,
    ) -> Result<(), String> {
        if self.owner_twin_id != workspace.and_then(|w| w.active_twin).map(|id| id.raw()) {
            return Err("UI action belongs to a retired Twin".into());
        }
        self.lifetime.validate(scene)
    }
}

fn authored_action_scope(
    world: &bevy::ecs::world::DeferredWorld,
    source: Entity,
) -> Option<RuntimeUiActionScope> {
    let mut scope = RuntimeUiActionScope::capture(
        world.get_resource::<lunco_workspace::WorkspaceResource>(),
        world.get_resource::<lunco_core::SceneTransitionCoordinator>(),
    );
    let mut entity = source;
    loop {
        if let Some(surface) = world.get::<RuntimeUiSurface>(entity) {
            if world
                .get_resource::<EngineExposures>()
                .is_some_and(|exposures| exposures.is_application_surface(&surface.namespace))
            {
                scope.lifetime = lunco_core::InteractionLifetime::Owner;
            }
            return Some(scope);
        }
        let parent = world.get::<ChildOf>(entity)?;
        entity = parent.parent();
    }
}

#[derive(Message, Clone, Copy, Debug)]
struct RuntimeUiSurfaceDragged {
    entity: Entity,
    delta: Vec2,
}

#[derive(Message, Clone, Copy, Debug)]
struct RuntimeUiSurfaceReset {
    entity: Entity,
}

fn emit_runtime_ui_surface_drag(
    mut trigger: On<PointerDrag>,
    mut drags: MessageWriter<RuntimeUiSurfaceDragged>,
    roots: Query<(Entity, &RuntimeUiSurface)>,
    parents: Query<&ChildOf>,
    tags: Query<&Tags>,
) {
    if trigger.button != PointerButton::Primary {
        return;
    }
    // This observer is scoped to the surface root. During event bubbling,
    // `trigger.entity` is that root; preserve the original HUI hit target to
    // determine whether the drag began on the authored handle.
    let root = trigger.entity;
    let hit = trigger.trigger().original_event_target;
    if !roots.get(root).is_ok_and(|(_, surface)| surface.draggable)
        || !is_runtime_ui_drag_handle(hit, root, &parents, &tags)
    {
        return;
    }
    drags.write(RuntimeUiSurfaceDragged {
        entity: root,
        delta: trigger.delta,
    });
    trigger.propagate(false);
}

/// Only authored drag handles move a window surface. Without this boundary,
/// pointer drags bubble from every HUI child and make ordinary form controls
/// move the entire panel while the user tries to edit or activate them.
fn is_runtime_ui_drag_handle(
    hit: Entity,
    root: Entity,
    parents: &Query<&ChildOf>,
    tags: &Query<&Tags>,
) -> bool {
    let mut current = hit;
    for _ in 0..64 {
        if current == root {
            return false;
        }
        if tags
            .get(current)
            .ok()
            .and_then(|tags| tags.tags().get("ui_drag_handle"))
            .is_some_and(|value| value == "true")
        {
            return true;
        }
        let Ok(parent) = parents.get(current) else {
            return false;
        };
        current = parent.parent();
    }
    false
}

/// Reset a draggable runtime surface from a primary-button double click.
///
/// This is global because the picked entity is usually a retained HUI child.
/// Other global click observers may intentionally stop propagation at that leaf,
/// so a root-scoped observer cannot reliably receive the event. The authored
/// surface root remains the only entity that owns the reset message; this
/// traversal only resolves that owner from the picked child.
fn emit_runtime_ui_surface_reset(
    mut trigger: On<PointerClick>,
    roots: Query<(Entity, &RuntimeUiSurface)>,
    parents: Query<&ChildOf>,
    tags: Query<&Tags>,
    mut resets: MessageWriter<RuntimeUiSurfaceReset>,
) {
    if trigger.button != PointerButton::Primary || trigger.count < 2 {
        return;
    }
    let hit = trigger.trigger().original_event_target;
    let Some(entity) = runtime_ui_surface_ancestor(hit, &roots, &parents) else {
        return;
    };
    let Ok((_, surface)) = roots.get(entity) else {
        return;
    };
    if !surface.draggable || !is_runtime_ui_drag_handle(hit, entity, &parents, &tags) {
        return;
    }
    resets.write(RuntimeUiSurfaceReset { entity });
    trigger.propagate(false);
}

/// Bind one HUI callback name to a semantic runtime action.
fn register_action(functions: &mut HtmlFunctions, callback: impl Into<String>, action: String) {
    functions.register(
        callback,
        move |In(source): In<Entity>, mut world: bevy::ecs::world::DeferredWorld| {
            let Some(scope) = authored_action_scope(&world, source) else {
                return;
            };
            world.trigger(RuntimeUiAction {
                scope,
                action: action.clone(),
                source,
                parameters: HookValue::Map(Vec::new()),
            });
        },
    );
}

/// Bind the HUI `tag:action` convention once. The action value is a
/// dynamic template property, so Twin/Rhai can choose it without adding a
/// manifest callback or a Rust enum arm.
fn register_dynamic_action(functions: &mut HtmlFunctions) {
    functions.register(
        "runtime_ui_authored_action",
        |In(source): In<Entity>, mut world: bevy::ecs::world::DeferredWorld| {
            let Some(tags) = world.get::<Tags>(source) else {
                return;
            };
            let action = tags.tags().get("action").cloned().unwrap_or_default();
            let Some(scope) = authored_action_scope(&world, source) else {
                return;
            };
            let form_id = tags.tags().get("ui_form").map(String::as_str);
            let form_values = form_id.and_then(|form_id| {
                world
                    .get_resource::<RuntimeUiForms>()
                    .filter(|forms| forms.owner_twin_id == scope.owner_twin_id)
                    .and_then(|forms| forms.forms.get(form_id))
                    .map(|form| &form.values)
            });
            let parameters = runtime_ui_action_parameters(tags, form_values);
            world.trigger(RuntimeUiAction {
                scope,
                action,
                source,
                parameters,
            });
        },
    );
}

/// Bridge HUI's namespaced HTML class tag to Flair's retained class list.
/// Plain `class="..."` is parsed as a Flair style attribute by HUI, so
/// runtime-authored templates use `tag:class="..."` for CSS selectors.
fn sync_runtime_ui_html_classes(
    mut commands: Commands,
    nodes: Query<(Entity, &Tags), Changed<Tags>>,
) {
    for (entity, tags) in &nodes {
        if let Some(classes) = tags.tags().get("class") {
            commands.entity(entity).insert(ClassList::new(classes));
        } else {
            // HUI can despawn a template node after this query but before the
            // deferred class sync is applied. Treat that teardown race as a
            // normal no-op instead of logging a stale-entity warning.
            commands.entity(entity).try_remove::<ClassList>();
        }
    }
}

fn runtime_ui_action_parameters(
    tags: &Tags,
    form_values: Option<&BTreeMap<String, String>>,
) -> HookValue {
    let mut fields = tags
        .tags()
        .iter()
        .filter(|(key, _)| key.as_str() != "action")
        .map(|(key, value)| (key.clone(), HookValue::str(value.clone())))
        .collect::<Vec<_>>();
    if let Some(form_values) = form_values {
        fields.push((
            "fields".to_owned(),
            HookValue::map(
                form_values
                    .iter()
                    .map(|(key, value)| (key.clone(), HookValue::str(value.clone()))),
            ),
        ));
    }
    HookValue::Map(fields)
}

#[lunco_core::on_command(SetRuntimeUiFormValues)]
fn on_set_runtime_ui_form_values(
    trigger: On<SetRuntimeUiFormValues>,
    workspace: Option<Res<WorkspaceResource>>,
    mut forms: ResMut<RuntimeUiForms>,
    mut exposures: Option<ResMut<EngineExposures>>,
) -> Result<Ack, String> {
    let owner = workspace
        .as_deref()
        .and_then(|workspace| workspace.active_twin)
        .map(TwinId::raw)
        .ok_or_else(|| "SetRuntimeUiFormValues requires an active Twin".to_owned())?;
    let event = trigger.event();
    if event.form_id.trim().is_empty() || event.form_id.len() > 64 {
        return Err("form_id must contain 1 to 64 characters".to_owned());
    }
    if event.values.len() > 64 {
        return Err("a runtime HTML form may contain at most 64 values".to_owned());
    }
    for (field, value) in &event.values {
        if field.trim().is_empty() || field.len() > 64 {
            return Err("form field ids must contain 1 to 64 characters".to_owned());
        }
        if value.len() > 2048 {
            return Err(format!("value for `{field}` exceeds 2048 characters"));
        }
    }
    if forms.owner_twin_id != Some(owner) {
        clear_runtime_ui_collections(&mut forms, exposures.as_deref_mut());
        forms.forms.clear();
        forms.owner_twin_id = Some(owner);
    }
    let form = forms.forms.entry(event.form_id.clone()).or_default();
    form.values.extend(event.values.clone());
    form.needs_projection = true;
    Ok(Ack::new(OpId::new()))
}

#[lunco_core::on_command(SetRuntimeUiCollectionData)]
fn on_set_runtime_ui_collection_data(
    trigger: On<SetRuntimeUiCollectionData>,
    workspace: Option<Res<WorkspaceResource>>,
    surfaces: Query<&RuntimeUiSurface>,
    manifest_state: Option<Res<RuntimeUiManifestState>>,
    mut forms: ResMut<RuntimeUiForms>,
    exposures: Option<ResMut<EngineExposures>>,
) -> Result<Ack, String> {
    let active_twin = workspace
        .as_deref()
        .and_then(|workspace| workspace.active_twin)
        .ok_or_else(|| "SetRuntimeUiCollectionData requires an active Twin".to_owned())?;
    let owner = active_twin.raw();
    let event = trigger.event();
    if event.namespace.trim().is_empty() || event.namespace.len() > 96 {
        return Err("namespace must contain 1 to 96 characters".to_owned());
    }
    if event.source.trim().is_empty() || event.source.len() > 64 {
        return Err("source must contain 1 to 64 characters".to_owned());
    }
    // Validate against authored manifest data when available. Surface roots
    // mount asynchronously after Twin startup, so commands from on_start must
    // not depend on HUI having spawned the presentation tree yet.
    let collection_in_manifest = manifest_state
        .as_deref()
        .map(|state| state.find_collection(Some(active_twin), &event.namespace, &event.source))
        .transpose()?
        .flatten();
    let collection_key = match collection_in_manifest {
        Some(collection_key) => collection_key,
        None => surfaces.iter().find_map(|surface| {
            (surface.namespace == event.namespace)
                .then(|| {
                    surface
                        .collections
                        .iter()
                        .find(|collection| collection.source == event.source)
                })
                .flatten()
                .map(|collection| collection.key.clone())
        }),
    };
    let Some(collection_key) = collection_key else {
        return Err(format!(
            "`{}` is not a declared collection source on runtime surface `{}`",
            event.source, event.namespace
        ));
    };
    let rows = runtime_ui_collection_value(&collection_key, &event.items)?;
    let Some(mut exposures) = exposures else {
        return Err("EngineExposures is not available for runtime collection data".to_owned());
    };
    if forms.owner_twin_id != Some(owner) {
        clear_runtime_ui_collections(&mut forms, Some(&mut exposures));
        forms.forms.clear();
        forms.owner_twin_id = Some(owner);
    }
    exposures
        .writer(&event.namespace)
        .property(&event.source, rows);
    forms
        .published_collections
        .insert((event.namespace.clone(), event.source.clone()));
    Ok(Ack::new(OpId::new()))
}

fn runtime_ui_collection_value(
    key_field: &str,
    items: &[BTreeMap<String, String>],
) -> Result<ExposureValue, String> {
    const MAX_ITEMS: usize = 128;
    const MAX_FIELDS_PER_ITEM: usize = 32;
    const MAX_TOTAL_BYTES: usize = 262_144;
    if items.len() > MAX_ITEMS {
        return Err(format!(
            "a runtime UI collection may contain at most {MAX_ITEMS} rows"
        ));
    }
    let mut seen = HashSet::with_capacity(items.len());
    let mut total_bytes = 0usize;
    let mut rows = Vec::with_capacity(items.len());
    for (row_index, item) in items.iter().enumerate() {
        if item.is_empty() || item.len() > MAX_FIELDS_PER_ITEM {
            return Err(format!(
                "collection row {} must contain 1 to {MAX_FIELDS_PER_ITEM} fields",
                row_index + 1
            ));
        }
        let Some(key) = item.get(key_field).filter(|key| !key.trim().is_empty()) else {
            return Err(format!(
                "collection row {} is missing its non-empty `{key_field}` key",
                row_index + 1
            ));
        };
        if !seen.insert(key.as_str()) {
            return Err(format!("collection key `{key}` appears more than once"));
        }
        let mut fields = Vec::with_capacity(item.len());
        for (name, value) in item {
            if name.trim().is_empty() || name.len() > 64 {
                return Err(format!(
                    "collection row {} contains an invalid field name",
                    row_index + 1
                ));
            }
            if value.len() > 2048 {
                return Err(format!("collection value `{name}` exceeds 2048 characters"));
            }
            total_bytes = total_bytes
                .saturating_add(name.len())
                .saturating_add(value.len());
            if total_bytes > MAX_TOTAL_BYTES {
                return Err(format!("collection data exceeds {MAX_TOTAL_BYTES} bytes"));
            }
            fields.push((name.clone(), ExposureValue::Text(value.clone())));
        }
        rows.push(ExposureValue::Map(fields));
    }
    Ok(ExposureValue::Array(rows))
}

fn clear_runtime_ui_collections(
    forms: &mut RuntimeUiForms,
    exposures: Option<&mut EngineExposures>,
) {
    let Some(exposures) = exposures else {
        forms.published_collections.clear();
        return;
    };
    for (namespace, source) in forms.published_collections.drain() {
        if exposures.surfaces.contains_key(&namespace) {
            exposures.writer(&namespace).remove_property(&source);
        }
    }
}

#[lunco_core::on_command(SetRuntimeUiFormStatus)]
fn on_set_runtime_ui_form_status(
    trigger: On<SetRuntimeUiFormStatus>,
    workspace: Option<Res<WorkspaceResource>>,
    mut forms: ResMut<RuntimeUiForms>,
) -> Result<Ack, String> {
    let owner = workspace
        .as_deref()
        .and_then(|workspace| workspace.active_twin)
        .map(TwinId::raw)
        .ok_or_else(|| "SetRuntimeUiFormStatus requires an active Twin".to_owned())?;
    if forms.owner_twin_id != Some(owner) {
        return Err("SetRuntimeUiFormStatus does not own the active Twin form".to_owned());
    }
    let form = forms
        .forms
        .get_mut(&trigger.event().form_id)
        .ok_or_else(|| "the requested runtime HTML form is not initialized".to_owned())?;
    form.status = trigger.event().status.chars().take(500).collect();
    Ok(Ack::new(OpId::new()))
}

#[lunco_core::on_command(InspectRuntimeUiForm)]
fn on_inspect_runtime_ui_form(
    trigger: On<InspectRuntimeUiForm>,
    workspace: Option<Res<WorkspaceResource>>,
    forms: Res<RuntimeUiForms>,
) -> Result<Ack, String> {
    let owner = workspace
        .as_deref()
        .and_then(|workspace| workspace.active_twin)
        .map(TwinId::raw)
        .ok_or_else(|| "InspectRuntimeUiForm requires an active Twin".to_owned())?;
    if forms.owner_twin_id != Some(owner) {
        return Err("the active Twin has not initialized this HTML form".to_owned());
    }
    let form = forms
        .forms
        .get(&trigger.event().form_id)
        .ok_or_else(|| "the requested runtime HTML form is not initialized".to_owned())?;
    Ok(Ack::with_data(
        OpId::new(),
        lunco_hooks::HookValue::map([
            ("form_id", HookValue::str(trigger.event().form_id.clone())),
            ("field_count", HookValue::UInt(form.values.len() as u64)),
            ("status", HookValue::str(form.status.clone())),
            ("owner_twin_id", HookValue::UInt(owner)),
            (
                "fields",
                HookValue::map(
                    form.values
                        .iter()
                        .map(|(key, value)| (key.clone(), HookValue::str(value.clone()))),
                ),
            ),
        ]),
    ))
}

fn on_runtime_ui_form_owner_closed(
    trigger: On<TwinClosed>,
    mut forms: ResMut<RuntimeUiForms>,
    mut exposures: Option<ResMut<EngineExposures>>,
) {
    if forms.owner_twin_id == Some(trigger.event().twin.raw()) {
        clear_runtime_ui_collections(&mut forms, exposures.as_deref_mut());
        *forms = RuntimeUiForms::default();
    }
}

fn attach_runtime_ui_text_inputs(
    mut commands: Commands,
    candidates: Query<
        (Entity, &Tags, &Text, Option<&TemplateProperties>),
        Without<RuntimeUiTextInput>,
    >,
    roots: Query<(Entity, &RuntimeUiSurface)>,
    parents: Query<&ChildOf>,
    workspace: Option<Res<WorkspaceResource>>,
    forms: Res<RuntimeUiForms>,
) {
    let Some(owner) = workspace
        .as_deref()
        .and_then(|workspace| workspace.active_twin)
        .map(TwinId::raw)
    else {
        return;
    };
    if forms.owner_twin_id != Some(owner) {
        return;
    }
    for (entity, tags, text, properties) in &candidates {
        let tags = tags.tags();
        let (Some(form_id), Some(field_id)) = (tags.get("ui_form"), tags.get("ui_field")) else {
            continue;
        };
        let kind = tags.get("ui_input").map(String::as_str).or_else(|| {
            properties.and_then(|properties| properties.get("type").map(String::as_str))
        });
        let Some(kind) = kind else {
            continue;
        };
        if form_id.is_empty() || field_id.is_empty() || !matches!(kind, "text" | "number") {
            continue;
        }
        let Some(surface_root) = runtime_ui_surface_ancestor(entity, &roots, &parents) else {
            continue;
        };
        let Ok((_, surface)) = roots.get(surface_root) else {
            continue;
        };
        if !surface.mounted || !forms.forms.contains_key(form_id) {
            continue;
        }
        let initial = forms.forms[form_id]
            .values
            .get(field_id)
            .cloned()
            .or_else(|| properties.and_then(|properties| properties.get("value").cloned()))
            .unwrap_or_else(|| text.0.clone());
        let mut editable = EditableText::new(initial);
        editable.max_characters = tags
            .get("ui_max_chars")
            .and_then(|value| value.parse::<usize>().ok())
            .or_else(|| match kind {
                "text" => Some(256),
                "number" => Some(32),
                _ => None,
            });
        let mut entity_commands = commands.entity(entity);
        entity_commands.insert((
            RuntimeUiTextInput {
                owner_twin_id: owner,
                form_id: form_id.clone(),
                field_id: field_id.clone(),
            },
            bevy_ui_widgets::TextInput,
            bevy_ui_widgets::SelectAllOnFocus,
            editable,
            bevy::text::TextCursorStyle {
                color: Color::srgb(0.44, 0.85, 0.77),
                ..default()
            },
            Pickable::default(),
        ));
        let tab_index = tags
            .get("ui_tab_index")
            .and_then(|value| value.parse::<i32>().ok());
        if let Some(tab_index) = tab_index {
            entity_commands.insert(bevy::input_focus::tab_navigation::TabIndex(tab_index));
        }
        if kind == "number" {
            entity_commands.insert(EditableTextFilter::new(|character| {
                character.is_ascii_digit() || matches!(character, '.' | '-')
            }));
        }
        drop(entity_commands);
        if tab_index.is_some() {
            commands
                .entity(surface_root)
                .insert(bevy::input_focus::tab_navigation::TabGroup::default());
        }
    }
}

fn attach_runtime_ui_scroll_areas(
    mut commands: Commands,
    candidates: Query<(Entity, &Tags), (With<Node>, Without<RuntimeUiScrollArea>)>,
    roots: Query<(Entity, &RuntimeUiSurface)>,
    parents: Query<&ChildOf>,
) {
    for (entity, tags) in &candidates {
        if tags.tags().get("ui_scroll_area").map(String::as_str) != Some("vertical") {
            continue;
        }
        let Some(surface_root) = runtime_ui_surface_ancestor(entity, &roots, &parents) else {
            continue;
        };
        if !roots
            .get(surface_root)
            .is_ok_and(|(_, surface)| surface.mounted)
        {
            continue;
        }
        commands.entity(entity).insert((
            RuntimeUiScrollArea,
            bevy_ui_widgets::ScrollArea,
            Pickable::default(),
        ));
    }
}

fn enable_runtime_ui_scroll_area_overflow(mut areas: Query<&mut Node, With<RuntimeUiScrollArea>>) {
    for mut node in &mut areas {
        node.overflow.y = bevy::ui::OverflowAxis::Scroll;
    }
}

fn apply_runtime_ui_form_values(
    mut forms: ResMut<RuntimeUiForms>,
    mut inputs: Query<(&RuntimeUiTextInput, &mut EditableText)>,
    workspace: Option<Res<WorkspaceResource>>,
) {
    let Some(owner) = forms.owner_twin_id else {
        return;
    };
    if workspace
        .as_deref()
        .and_then(|workspace| workspace.active_twin)
        .map(TwinId::raw)
        != Some(owner)
    {
        return;
    }
    for (form_id, form) in &mut forms.forms {
        if !form.needs_projection {
            continue;
        }
        for (input, mut editable) in &mut inputs {
            if input.owner_twin_id != owner || &input.form_id != form_id {
                continue;
            }
            if let Some(value) = form.values.get(&input.field_id) {
                editable.editor_mut().set_text(value);
            }
        }
        form.needs_projection = false;
    }
}

fn store_runtime_ui_form_input_changes(
    mut forms: ResMut<RuntimeUiForms>,
    inputs: Query<(&RuntimeUiTextInput, &EditableText), Changed<EditableText>>,
    workspace: Option<Res<WorkspaceResource>>,
) {
    let Some(owner) = forms.owner_twin_id else {
        return;
    };
    if workspace
        .as_deref()
        .and_then(|workspace| workspace.active_twin)
        .map(TwinId::raw)
        != Some(owner)
    {
        return;
    }
    for (input, editable) in &inputs {
        if input.owner_twin_id != owner {
            continue;
        }
        let Some(form) = forms.forms.get_mut(&input.form_id) else {
            continue;
        };
        form.values
            .insert(input.field_id.clone(), editable.value().to_string());
    }
}

fn update_runtime_ui_form_text(
    forms: Res<RuntimeUiForms>,
    mut texts: Query<(&Tags, &mut Text)>,
    workspace: Option<Res<WorkspaceResource>>,
) {
    let Some(owner) = forms.owner_twin_id else {
        return;
    };
    if workspace
        .as_deref()
        .and_then(|workspace| workspace.active_twin)
        .map(TwinId::raw)
        != Some(owner)
    {
        return;
    }
    for (tags, mut text) in &mut texts {
        let tags = tags.tags();
        let Some(form) = tags.get("ui_form").and_then(|id| forms.forms.get(id)) else {
            continue;
        };
        if tags.get("ui_status").map(String::as_str) == Some("true") {
            if text.0 != form.status {
                text.0.clone_from(&form.status);
            }
        } else if let Some(field) = tags.get("ui_value") {
            if let Some(value) = form.values.get(field)
                && text.0 != *value
            {
                text.0.clone_from(value);
            }
        }
    }
}

/// Authored registration for runtime UI surfaces.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RuntimeUiManifest {
    #[serde(default, rename = "kind")]
    _kind: String,
    pub surfaces: Vec<RuntimeUiSurfaceDefinition>,
}

/// One authored perspective or a bounded set, using the same field and gate
/// for interactive rendering and capture readiness. Omission means unrestricted.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RuntimeUiPerspectiveFilter {
    One(String),
    Many(Vec<String>),
}

impl RuntimeUiPerspectiveFilter {
    fn validates(&self) -> Result<(), String> {
        let values: &[String] = match self {
            Self::One(value) => std::slice::from_ref(value),
            Self::Many(values) => values,
        };
        if values.is_empty() || values.len() > 32 {
            return Err("visible_in_perspective must contain 1–32 perspective ids".into());
        }
        let mut seen = HashSet::new();
        for value in values {
            require_non_empty("visible_in_perspective", value)?;
            if !seen.insert(value) {
                return Err("visible_in_perspective repeats an id".into());
            }
        }
        Ok(())
    }

    fn allows(&self, layout: Option<&WorkbenchSnapshot>) -> bool {
        let Some(active) = layout.and_then(WorkbenchSnapshot::active_perspective) else {
            return false;
        };
        match self {
            Self::One(value) => active.as_str() == value,
            Self::Many(values) => values.iter().any(|value| active.as_str() == value),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeUiSurfaceDefinition {
    /// Stable identity used to validate and reconcile an authored surface.
    pub id: String,
    pub template: String,
    pub stylesheet: String,
    pub namespace: String,
    #[serde(default)]
    pub bindings: HashMap<String, RuntimeUiBindingDefinition>,
    #[serde(default)]
    pub actions: Vec<RuntimeUiActionDefinition>,
    /// Generic ordered collection hosts. The source value is an exposure
    /// array of typed records; the runtime only reconciles rows by key.
    #[serde(default)]
    pub collections: Vec<RuntimeUiCollectionDefinition>,
    /// Generic option dropdowns. Each dropdown reads an exposure array of
    /// typed records and uses authored field names for identity, display, and
    /// action dispatch. The runtime does not know what the options represent.
    #[serde(default)]
    pub dropdowns: Vec<RuntimeUiDropdownDefinition>,
    #[serde(default)]
    pub visible_in_perspective: Option<RuntimeUiPerspectiveFilter>,
    #[serde(default)]
    pub gate: Option<String>,
    /// Optional boolean Twin setting that controls whether this surface mounts.
    #[serde(default)]
    pub setting: Option<String>,
    /// Default for the optional Twin setting when the key is absent. The
    /// authored surface declares this policy; Rust has no per-setting field.
    #[serde(default)]
    pub setting_default: bool,
    #[serde(default)]
    pub interactive: bool,
    /// A modal surface owns the full window while visible and may provide an
    /// authored semantic dismissal action for Escape.
    #[serde(default)]
    pub modal: bool,
    #[serde(default)]
    pub dismiss_action: Option<String>,
    /// Whether the surface root can be moved with a primary-button drag.
    /// Only window surfaces may opt in; viewport and dock-panel roots own a
    /// larger layout boundary and cannot be moved as a whole.
    #[serde(default)]
    pub draggable: bool,
    pub placement: RuntimeUiPlacementDefinition,
}

/// Authored mapping from an engine capability value to a template property.
/// The target key is the template property's name; the source is an engine
/// exposure property. `map` is optional for identity bindings and required for
/// authored state translations such as a boolean display or an active color.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeUiBindingDefinition {
    pub source: String,
    /// Project this authored presentation value into a CSS custom property.
    /// Text-only template values must not enter the CSS token stream.
    #[serde(default)]
    pub css: bool,
    #[serde(default)]
    pub map: HashMap<String, String>,
}

/// Authored callback-to-command mapping. HUI knows only the callback name;
/// the runtime bridge emits the semantic action without a widget-specific Rust
/// registration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeUiActionDefinition {
    pub callback: String,
    pub action: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeUiCollectionDefinition {
    pub source: String,
    pub container: String,
    pub template: String,
    pub key: String,
    /// Row fields explicitly admitted to Flair's CSS custom-property stream.
    #[serde(default)]
    pub css: Vec<String>,
    /// Optional authored vertical scrollbar track id, sharing this collection's scroll position.
    #[serde(default)]
    pub scrollbar: Option<String>,
}

/// Generic dropdown mechanics for an authored runtime surface.
///
/// `width_source` and `max_height_source` name text or numeric exposure
/// properties. Rhai owns those values, so a Twin can size a dropdown without a
/// Rust constant or a camera-specific branch. The HTML surface may bind the
/// same properties to CSS custom properties for its trigger styling.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeUiDropdownDefinition {
    pub id: String,
    pub trigger_action: String,
    pub source: String,
    pub key: String,
    pub label: String,
    pub action: String,
    pub width_source: String,
    pub max_height_source: String,
    #[serde(default)]
    pub selected_source: Option<String>,
    /// Optional record field for group headings in authored row order.
    #[serde(default)]
    pub group: Option<String>,
    /// Enables a local text filter over row labels, keys and groups.
    #[serde(default)]
    pub search_hint: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeUiPlacementDefinition {
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
pub enum RuntimeUiWindowAnchor {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl RuntimeUiManifest {
    pub fn dropdown_key_for_action(&self, action: &str) -> Option<String> {
        self.surfaces.iter().find_map(|surface| {
            surface
                .dropdowns
                .iter()
                .find(|dropdown| dropdown.trigger_action == action)
                .map(|dropdown| runtime_ui_dropdown_key(&surface.id, &dropdown.id))
        })
    }

    pub fn dropdown_for_key(
        &self,
        key: &str,
    ) -> Option<(&RuntimeUiSurfaceDefinition, &RuntimeUiDropdownDefinition)> {
        self.surfaces.iter().find_map(|surface| {
            surface.dropdowns.iter().find_map(|dropdown| {
                (runtime_ui_dropdown_key(&surface.id, &dropdown.id) == key)
                    .then_some((surface, dropdown))
            })
        })
    }

    /// Validate the authored runtime UI contract before it reaches HUI or the
    /// semantic action bridge. Invalid manifests are rejected as data errors,
    /// so a typo cannot silently create an inert or globally overwritten UI.
    fn validate(&self) -> Result<(), String> {
        let mut ids = HashSet::new();
        let mut namespaces = HashSet::new();
        let mut callbacks = HashSet::new();
        let mut dropdown_triggers = HashSet::new();

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
                perspective.validates()?;
            }
            if let Some(gate) = &surface.gate {
                require_non_empty("surface gate", gate)?;
            }
            if let Some(setting) = &surface.setting {
                lunco_twin::TwinManifest::validate_setting_key(setting)?;
            }
            if surface.draggable
                && !matches!(
                    surface.placement,
                    RuntimeUiPlacementDefinition::Window { .. }
                )
            {
                return Err(format!(
                    "draggable runtime UI surface `{}` must use window placement",
                    surface.id
                ));
            }
            if surface.modal && !surface.interactive {
                return Err(format!(
                    "modal runtime UI surface `{}` must be interactive",
                    surface.id
                ));
            }
            if surface.modal
                && !matches!(&surface.placement, RuntimeUiPlacementDefinition::Viewport)
            {
                return Err(format!(
                    "modal runtime UI surface `{}` must use viewport placement",
                    surface.id
                ));
            }
            if let Some(action) = &surface.dismiss_action {
                require_non_empty("modal dismiss action", action)?;
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
            let mut collection_sources = HashSet::new();
            let mut collection_hosts = HashSet::new();
            for collection in &surface.collections {
                require_non_empty("collection source", &collection.source)?;
                require_non_empty("collection container", &collection.container)?;
                require_non_empty("collection template", &collection.template)?;
                require_non_empty("collection key", &collection.key)?;
                require_asset_path("collection template", &collection.template)?;
                if let Some(scrollbar) = &collection.scrollbar {
                    require_non_empty("collection scrollbar", scrollbar)?;
                }
                if !collection_sources.insert(collection.source.as_str()) {
                    return Err(format!(
                        "duplicate runtime UI collection source `{}` on `{}`",
                        collection.source, surface.id
                    ));
                }
                if !collection_hosts.insert(collection.container.as_str()) {
                    return Err(format!(
                        "duplicate runtime UI collection container `{}` on `{}`",
                        collection.container, surface.id
                    ));
                }
            }
            let mut dropdown_ids = HashSet::new();
            let mut dropdown_sources = HashSet::new();
            for dropdown in &surface.dropdowns {
                require_non_empty("dropdown id", &dropdown.id)?;
                require_non_empty("dropdown trigger action", &dropdown.trigger_action)?;
                require_non_empty("dropdown source", &dropdown.source)?;
                require_non_empty("dropdown key", &dropdown.key)?;
                require_non_empty("dropdown label", &dropdown.label)?;
                require_non_empty("dropdown action", &dropdown.action)?;
                require_non_empty("dropdown width source", &dropdown.width_source)?;
                require_non_empty("dropdown max height source", &dropdown.max_height_source)?;
                if !dropdown_ids.insert(dropdown.id.as_str()) {
                    return Err(format!(
                        "duplicate runtime UI dropdown id `{}` on `{}`",
                        dropdown.id, surface.id
                    ));
                }
                if !dropdown_triggers.insert(dropdown.trigger_action.as_str()) {
                    return Err(format!(
                        "duplicate runtime UI dropdown trigger `{}` on `{}`",
                        dropdown.trigger_action, surface.id
                    ));
                }
                if !dropdown_sources.insert(dropdown.source.as_str()) {
                    return Err(format!(
                        "duplicate runtime UI dropdown source `{}` on `{}`",
                        dropdown.source, surface.id
                    ));
                }
                if let Some(selected_source) = &dropdown.selected_source {
                    require_non_empty("dropdown selected source", selected_source)?;
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
            }
        }
        Ok(())
    }
}

fn runtime_ui_dropdown_key(surface_id: &str, dropdown_id: &str) -> String {
    format!("{surface_id}::{dropdown_id}")
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
    if lunco_assets_core::parse_lunco_uri(value).is_some()
        || lunco_assets_core::parse_twin_uri(value).is_some()
    {
        lunco_assets_core::asset_path::load_asset_path(value, None, None, None)
            .map_err(|error| format!("{label}: {error}"))?;
        return Ok(());
    }
    if lunco_assets_path::relative_path(value).is_none() {
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

struct RuntimeUiManifestPlugin;

impl Plugin for RuntimeUiManifestPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_assets_runtime::TextAssetPlugin>() {
            app.add_plugins(lunco_assets_runtime::TextAssetPlugin);
        }
        app.add_message::<RuntimeUiSurfaceDragged>()
            .add_message::<RuntimeUiSurfaceReset>()
            .add_observer(emit_runtime_ui_surface_reset);
    }
}

/// Generic HUI/Flair runtime-surface presentation plugin.
///
/// Hosts provide exposure values, workbench snapshots, Twin workspace state,
/// named gates, and capture mode; this crate owns manifest loading, retained
/// tree lifecycle, placement, styling, input regions, and render readiness.
pub struct RuntimeUiPlugin;

impl Plugin for RuntimeUiPlugin {
    fn build(&self, app: &mut App) {
        // Bevy UI nodes block scene picks by default, including decorative
        // retained nodes. Require explicit targets so transparent UI layout
        // does not intercept the scene pointer.
        app.init_resource::<UiPickingSettings>();
        app.world_mut()
            .resource_mut::<UiPickingSettings>()
            .require_markers = true;
        if !app.is_plugin_added::<bevy_ui_widgets::ScrollbarPlugin>() {
            app.add_plugins(bevy_ui_widgets::ScrollbarPlugin);
        }
        if !app.is_plugin_added::<bevy_ui_widgets::ScrollAreaPlugin>() {
            app.add_plugins(bevy_ui_widgets::ScrollAreaPlugin);
        }
        if !app.is_plugin_added::<bevy_ui_widgets::TextInputPlugin>() {
            app.add_plugins(bevy_ui_widgets::TextInputPlugin);
        }
        if !app.is_plugin_added::<bevy::input_focus::InputFocusPlugin>() {
            app.add_plugins(bevy::input_focus::InputFocusPlugin);
        }
        if !app.is_plugin_added::<bevy::input_focus::tab_navigation::TabNavigationPlugin>() {
            app.add_plugins(bevy::input_focus::tab_navigation::TabNavigationPlugin);
        }
        if !app.is_plugin_added::<bevy::input_focus::InputDispatchPlugin>() {
            app.add_plugins(bevy::input_focus::InputDispatchPlugin);
        }
        if !app.is_plugin_added::<bevy::input_focus::pointer_focus::PointerFocusPlugin>() {
            app.add_plugins(bevy::input_focus::pointer_focus::PointerFocusPlugin);
        }
        app.add_plugins((
            bevy_hui::HuiPlugin,
            bevy_flair::FlairPlugin,
            RuntimeUiManifestPlugin,
        ));
        // HUI treats unsupported HTML tags as custom elements. Bind the
        // standard `<input>` tag to Bevy UI text nodes so its `type`, `value`,
        // `id`, and runtime form tags can use the same retained HTML pipeline.
        app.world_mut()
            .resource_mut::<bevy_hui::prelude::ComponentBindings>()
            .register("input", |mut entity| {
                entity.insert((Node::default(), Text::default()));
            });
        app.add_observer(dispatch_runtime_ui_press)
            .init_resource::<RuntimeSurfaceLayouts>()
            .init_resource::<RuntimeUiRenderState>()
            .init_resource::<RuntimeUiPresentationGeneration>()
            .init_resource::<RuntimeUiForms>()
            .init_resource::<RuntimeUiRecordingContract>()
            .init_resource::<RuntimeUiCaptureState>()
            .init_resource::<RuntimeUiGates>()
            .init_resource::<RuntimeUiCursorMirror>()
            .add_systems(
                PreUpdate,
                (
                    mirror_runtime_ui_cursor.before(bevy::ui::UiSystems::Focus),
                    restore_runtime_ui_cursor.after(bevy::ui::UiSystems::Focus),
                ),
            )
            // Asset-scope events publish validated source snapshots. Scene teardown
            // clears resolved presentation while open source scopes remain reusable.
            .init_scene_resource::<RuntimeUiManifestState>()
            .add_observer(admit_runtime_ui_manifest)
            .add_observer(retire_runtime_ui_manifest)
            .add_observer(on_runtime_ui_form_owner_closed)
            .add_systems(
                Update,
                (
                    attach_runtime_ui_text_inputs
                        .after(bevy_hui::HuiSystems::Build)
                        .after(apply_runtime_ui_exposures),
                    attach_runtime_ui_scroll_areas
                        .after(bevy_hui::HuiSystems::Build)
                        .after(apply_runtime_ui_exposures),
                    apply_runtime_ui_form_values
                        .after(attach_runtime_ui_text_inputs)
                        .before(bevy_hui::HuiSystems::Style),
                    update_runtime_ui_form_text
                        .after(attach_runtime_ui_text_inputs)
                        .before(bevy_hui::HuiSystems::Style),
                    sync_runtime_ui_html_classes
                        .after(bevy_hui::HuiSystems::Build)
                        .before(bevy_hui::HuiSystems::Style),
                ),
            )
            .add_systems(
                PostUpdate,
                store_runtime_ui_form_input_changes.after(EditableTextSystems),
            )
            .add_systems(
                Update,
                (
                    sync_runtime_ui_manifest,
                    mount_runtime_ui_surfaces
                        .after(sync_runtime_ui_manifest)
                        .before(bevy_hui::HuiSystems::Build),
                    bind_runtime_ui_to_camera.after(sync_runtime_ui_manifest),
                    apply_runtime_ui_exposures
                        .after(sync_runtime_ui_manifest)
                        .after(bevy_hui::HuiSystems::Build)
                        .before(bevy_hui::HuiSystems::Style),
                    apply_runtime_ui_surface_interactions
                        .after(apply_runtime_ui_exposures)
                        .before(bevy_hui::HuiSystems::Style),
                    reconcile_runtime_ui_collections
                        .after(apply_runtime_ui_exposures)
                        .after(bevy_hui::HuiSystems::Build)
                        .before(bevy_hui::HuiSystems::Style),
                    register_runtime_ui_input_regions.after(apply_runtime_ui_exposures),
                    sync_runtime_ui_scrollbar_theme,
                    dismiss_runtime_ui,
                )
                    .in_set(lunco_core::RuntimeCycleSet::Ui),
            )
            .add_systems(
                PostUpdate,
                (
                    enable_runtime_ui_scroll_area_overflow
                        .after(bevy_flair::style::StyleSystems::ApplyComputedProperties)
                        .before(bevy::ui::UiSystems::Content),
                    apply_runtime_ui_placement_after_style
                        .after(bevy_flair::style::StyleSystems::ApplyComputedProperties)
                        .after(bevy::ui::UiSystems::Propagate)
                        .before(bevy::ui::UiSystems::Content),
                    update_runtime_ui_recording_contract
                        .after(apply_runtime_ui_placement_after_style),
                    report_runtime_ui_readiness
                        .after(update_runtime_ui_recording_contract)
                        .after(apply_runtime_ui_placement_after_style)
                        .after(bevy::ui::UiSystems::PostLayout),
                )
                    .in_set(lunco_core::RuntimeCycleSet::Ui),
            );
        install_runtime_ui_style_handoff(app);
        install_runtime_ui_render_readiness(app);
        register_runtime_ui_form_commands(app);
    }
}

fn install_runtime_ui_style_handoff(app: &mut App) {
    app.add_systems(
        Update,
        (attach_runtime_ui_names, hand_runtime_ui_styling_to_flair)
            .after(sync_runtime_ui_manifest)
            .after(bevy_hui::HuiSystems::Build)
            .after(apply_runtime_ui_exposures)
            .after(apply_runtime_ui_surface_interactions)
            .after(reconcile_runtime_ui_collections)
            .before(bevy_hui::HuiSystems::Style)
            .in_set(lunco_core::RuntimeCycleSet::Ui),
    );
}

#[derive(Resource, Default)]
pub struct RuntimeUiManifestState {
    // Application source survives scene teardown; Twin sources survive restart
    // and are retired by exact TwinClosed identity.
    scopes: HashMap<Option<TwinId>, Result<Option<RuntimeUiManifest>, String>>,
    revision: u64,
    selected: Option<TwinId>,
    manifest: Option<RuntimeUiManifest>,
    error: Option<String>,
    applied: Option<u64>,
    rebuild_pending: bool,
}
impl lunco_core::SceneResource for RuntimeUiManifestState {
    fn clear_scene(&mut self) {
        self.manifest = None;
        self.applied = None;
        self.error = None;
        self.rebuild_pending = true;
    }
}

impl RuntimeUiManifestState {
    pub fn manifest(&self) -> Option<&RuntimeUiManifest> {
        self.manifest.as_ref()
    }

    fn find_collection(
        &self,
        selected: Option<TwinId>,
        namespace: &str,
        source: &str,
    ) -> Result<Option<Option<String>>, String> {
        if self.selected == selected
            && let Some(manifest) = &self.manifest
        {
            return Ok(Some(
                manifest
                    .surfaces
                    .iter()
                    .find(|surface| surface.namespace == namespace)
                    .and_then(|surface| {
                        surface
                            .collections
                            .iter()
                            .find(|collection| collection.source == source)
                    })
                    .map(|collection| collection.key.clone()),
            ));
        }

        // Before the first manifest reconciliation, the per-scope source
        // cache is already the authoritative authored declaration. Merge it
        // with the same Twin-over-application identity rule used by the live
        // surface builder.
        let mut surfaces = HashMap::new();
        let mut scope_loaded = false;
        if let Some(application) = self.scopes.get(&None) {
            scope_loaded = true;
            if let Some(manifest) = application.as_ref().map_err(Clone::clone)? {
                for surface in &manifest.surfaces {
                    surfaces.insert(surface.id.clone(), surface.clone());
                }
            }
        }
        if let Some(twin) = selected.and_then(|id| self.scopes.get(&Some(id))) {
            scope_loaded = true;
            if let Some(manifest) = twin.as_ref().map_err(Clone::clone)? {
                for surface in &manifest.surfaces {
                    surfaces.insert(surface.id.clone(), surface.clone());
                }
            }
        }
        if !scope_loaded {
            return Ok(None);
        }
        Ok(Some(
            surfaces
                .values()
                .find(|surface| surface.namespace == namespace)
                .and_then(|surface| {
                    surface
                        .collections
                        .iter()
                        .find(|collection| collection.source == source)
                })
                .map(|collection| collection.key.clone()),
        ))
    }

    pub fn dropdown_key_for_action(&self, action: &str) -> Option<String> {
        self.manifest()?.dropdown_key_for_action(action)
    }
}

/// Twin-authored capture contract for runtime surfaces.
///
/// The global UI manifest describes reusable presentation assets only. The
/// active Twin's `runtime.ui.recording` policy returns stable surface IDs for
/// the current capture. The recorder consumes the readiness status below; it
/// does not know what a surface represents.
#[derive(Resource, Debug, Default)]
struct RuntimeUiRecordingContract {
    required_namespaces: HashSet<String>,
    manifest_id: Option<u64>,
    exposure_revision: u64,
    hook_generation: u64,
    recording_active: bool,
    error: Option<String>,
}

/// Host-provided capture mode for the generic runtime UI layer.
///
/// The host mirrors its recorder's active state into this resource. Keeping the
/// recorder type out of this crate prevents the reusable UI mechanism from
/// depending on screenshot/video implementation details.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeUiCaptureState {
    /// Whether the host is currently preparing an offline capture.
    pub active: bool,
}

/// Acknowledgement from the render extraction boundary. Main-world layout is
/// not enough to arm offline capture: the render world must have extracted at
/// least one visible UI node for every required surface in the current
/// presentation generation. This is the presentation event the recorder
/// consumes indirectly through [`StatusBus`].
#[derive(Resource, Clone, Copy, Debug, Default)]
struct RuntimeUiRenderState {
    pub extracted_generation: u64,
    pub extracted_surface_count: u32,
}

/// Monotonic identity for a required runtime UI presentation lifecycle.
///
/// Exposure revisions describe changing values inside an already-mounted
/// retained tree. They are deliberately not used as a render acknowledgement
/// key because a live surface can update every frame. This generation changes
/// only when a required surface becomes visible and ready, so a new render
/// submission can be matched to the presentation that needs to be captured.
#[derive(Resource, Debug, Default)]
struct RuntimeUiPresentationGeneration {
    next: u64,
}

impl RuntimeUiPresentationGeneration {
    fn start_if_visible(&mut self, surface: &mut RuntimeUiSurface, visible: bool) {
        if visible && !surface.presentation_visible {
            self.next = self
                .next
                .checked_add(1)
                .expect("runtime UI presentation generation exhausted");
            surface.presentation_generation = self.next;
        }
        surface.presentation_visible = visible;
    }
}

/// Render-world handoff for the presentation acknowledgement. The extraction
/// phase fills `extracted_generation`; the render schedule promotes it only
/// after the UI render set has submitted the frame. The next extraction copies
/// that submitted generation back to the simulation world.
#[derive(Resource, Debug, Default)]
struct RuntimeUiRenderAck {
    extracted_generation: u64,
    extracted_surface_count: u32,
    submitted_generation: u64,
    submitted_surface_count: u32,
    visible_exposure_revision: u64,
    visible_namespaces: HashSet<String>,
}

/// Generic named visibility gates supplied by the host application.
#[derive(Resource, Debug, Default)]
pub struct RuntimeUiGates {
    values: HashMap<String, bool>,
}

impl RuntimeUiGates {
    pub fn set(&mut self, name: impl Into<String>, value: bool) {
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
pub struct RuntimeUiSurface {
    layout_id: String,
    namespace: String,
    template: Handle<HtmlTemplate>,
    stylesheet: Handle<StyleSheet>,
    bindings: HashMap<String, RuntimeUiBindingDefinition>,
    collections: Vec<RuntimeUiCollection>,
    visible_in_perspective: Option<RuntimeUiPerspectiveFilter>,
    gate: Option<String>,
    setting: Option<String>,
    setting_default: bool,
    interactive: bool,
    modal: bool,
    dismiss_action: Option<String>,
    draggable: bool,
    mounted: bool,
    /// For a surface required by offline recording, set after the retained tree
    /// has a camera target, a computed non-zero render target, a computed
    /// layout, and an exposure snapshot projected into HUI. This is a
    /// presentation lifecycle state, not a frame counter: later exposure
    /// revisions update the mounted tree in place and must not hide it while
    /// live values change. Interactive surfaces do not wait for this
    /// render-world acknowledgement.
    presentation_ready: bool,
    /// Whether the required surface was visible in the last readiness pass.
    /// A visibility transition starts a new render acknowledgement lifecycle,
    /// including when a surface is shown again after a perspective or gate
    /// change.
    presentation_visible: bool,
    presentation_generation: u64,
    placement: RuntimeUiPlacement,
    /// The retained tree has consumed a real exposure snapshot. Its initial
    /// readiness must not race later global exposure revisions in PostUpdate.
    has_projected_exposure: bool,
    applied_revision: u64,
    applied_placement: Option<ResolvedRuntimeUiPlacement>,
}

/// A domain image provider has not yet published the exact image requested by
/// an authored runtime node. Recording must wait for this descendant too.
#[derive(Component, Debug)]
pub struct RuntimeUiImagePending(pub String);

impl RuntimeUiSurface {
    /// Whether this retained surface covers scene labels in its layout rectangle.
    /// Modal roots cover their viewport. Other viewport roots are transparent
    /// containers; their visible controls cover labels individually.
    pub fn occludes_scene_labels(&self) -> bool {
        self.modal || !matches!(self.placement, RuntimeUiPlacement::Viewport)
    }
    fn from_definition(
        definition: &RuntimeUiSurfaceDefinition,
        template: Handle<HtmlTemplate>,
        stylesheet: Handle<StyleSheet>,
        server: Option<&AssetServer>,
    ) -> Result<Self, String> {
        Ok(Self {
            layout_id: definition.id.clone(),
            namespace: definition.namespace.clone(),
            template,
            stylesheet,
            bindings: definition.bindings.clone(),
            collections: definition
                .collections
                .iter()
                .map(|collection| {
                    let template = match server {
                        Some(server) => server.load(
                            lunco_assets_core::asset_path::load_asset_path(
                                &collection.template,
                                None,
                                None,
                                None,
                            )
                            .map_err(|error| error.to_string())?,
                        ),
                        None => Handle::default(),
                    };
                    Ok(RuntimeUiCollection {
                        source: collection.source.clone(),
                        container: collection.container.clone(),
                        template,
                        key: collection.key.clone(),
                        css: collection.css.clone(),
                        scrollbar: collection.scrollbar.clone(),
                    })
                })
                .collect::<Result<_, String>>()?,
            visible_in_perspective: definition.visible_in_perspective.clone(),
            gate: definition.gate.clone(),
            setting: definition.setting.clone(),
            setting_default: definition.setting_default,
            interactive: definition.interactive,
            modal: definition.modal,
            dismiss_action: definition.dismiss_action.clone(),
            draggable: definition.draggable,
            mounted: false,
            presentation_ready: false,
            presentation_visible: false,
            presentation_generation: 0,
            placement: (&definition.placement).into(),
            has_projected_exposure: false,
            applied_revision: 0,
            applied_placement: None,
        })
    }

    /// Return the exposure namespace owned by this surface.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Return whether HUI has mounted the retained surface tree.
    pub fn is_mounted(&self) -> bool {
        self.mounted
    }

    /// Return the last resolved outer rectangle, if placement has completed.
    pub fn applied_rect(&self) -> Option<egui::Rect> {
        self.applied_placement.map(|placement| placement.rect)
    }
}

#[derive(Debug)]
struct RuntimeUiCollection {
    source: String,
    container: String,
    template: Handle<HtmlTemplate>,
    key: String,
    css: Vec<String>,
    scrollbar: Option<String>,
}

#[derive(Component)]
struct RuntimeUiScrollbarThumb;

#[derive(Component, Debug)]
struct RuntimeUiCollectionRow {
    host: Entity,
    key: String,
    property_names: Vec<String>,
}

#[derive(Component)]
struct RuntimeUiCollectionHost;

/// HUI's compiler requires its style context for dynamic attributes. Retain
/// that context outside the active style system so Flair owns rendered styles.
#[derive(Component)]
struct RuntimeUiTemplateStyle(HtmlStyle);

/// Bevy UI reads Window cursor state during focus. Keep this transient mirror
/// within that phase so winit's Last-stage synchronization sees native state.
#[derive(Resource, Default)]
struct RuntimeUiCursorMirror(Option<(Entity, Option<bevy::math::DVec2>)>);

fn mirror_runtime_ui_cursor(
    mut events: MessageReader<bevy::window::CursorMoved>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    mut mirror: ResMut<RuntimeUiCursorMirror>,
) {
    let Ok((entity, mut window)) = windows.single_mut() else {
        return;
    };
    let Some(event) = events.read().filter(|event| event.window == entity).last() else {
        return;
    };
    mirror.0 = Some((
        entity,
        window
            .physical_cursor_position()
            .map(|position| position.as_dvec2()),
    ));
    window.set_cursor_position(Some(event.position));
}

fn restore_runtime_ui_cursor(
    mut windows: Query<&mut Window>,
    mut mirror: ResMut<RuntimeUiCursorMirror>,
) {
    if let Some((entity, position)) = mirror.0.take() {
        if let Ok(mut window) = windows.get_mut(entity) {
            window.set_physical_cursor_position(position);
        }
    }
}

fn compile_runtime_ui_context(world: &mut World, root: Entity) {
    let mut nodes = vec![root];
    let mut index = 0;
    while index < nodes.len() {
        if let Some(children) = world.get::<Children>(nodes[index]) {
            nodes.extend(children.iter());
        }
        index += 1;
    }
    for entity in &nodes {
        if world.get::<HtmlStyle>(*entity).is_none() {
            if let Some(style) = world.get::<RuntimeUiTemplateStyle>(*entity) {
                let style = style.0.clone();
                world.entity_mut(*entity).insert(style);
            }
        }
    }
    world.trigger(CompileContextEvent { entity: root });
    world.flush();
    for entity in nodes {
        if let Some(style) = world.get::<HtmlStyle>(entity).cloned() {
            world
                .entity_mut(entity)
                .insert(RuntimeUiTemplateStyle(style))
                .remove::<HtmlStyle>();
        }
    }
}

/// Resolve the active Twin's typed capture contract once per exposure/policy
/// revision. A normal interactive session has no contract; when the recorder
/// becomes active the Twin policy may select any currently visible surfaces by
/// stable authored ID.
fn update_runtime_ui_recording_contract(
    time: Res<Time<Real>>,
    exposures: Res<EngineExposures>,
    manifest_state: Res<RuntimeUiManifestState>,
    recording: Option<Res<RuntimeUiCaptureState>>,
    layout: Option<Res<WorkbenchSnapshot>>,
    gates: Option<Res<RuntimeUiGates>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut contract: ResMut<RuntimeUiRecordingContract>,
) {
    let recording_active = recording.is_some_and(|state| state.active);
    let manifest_id = manifest_state.applied;
    let hook_generation = lunco_hooks::generation();
    if contract.manifest_id == manifest_id
        && contract.exposure_revision == exposures.revision
        && contract.hook_generation == hook_generation
        && contract.recording_active == recording_active
        && !manifest_state.is_changed()
        && !layout.as_ref().is_some_and(|layout| layout.is_changed())
        && !gates.as_ref().is_some_and(|gates| gates.is_changed())
        && !workspace
            .as_ref()
            .is_some_and(|workspace| workspace.is_changed())
    {
        return;
    }

    contract.manifest_id = manifest_id;
    contract.exposure_revision = exposures.revision;
    contract.hook_generation = hook_generation;
    contract.recording_active = recording_active;
    contract.required_namespaces.clear();
    contract.error = None;
    if !recording_active {
        return;
    }

    let Some(manifest) = manifest_state.manifest() else {
        return;
    };
    let all_surface_ids = manifest
        .surfaces
        .iter()
        .map(|surface| HookValue::str(surface.id.clone()))
        .collect();
    let visible_surface_ids = manifest
        .surfaces
        .iter()
        .filter(|surface| {
            capture_surface_is_visible(
                surface,
                &exposures,
                layout.as_deref(),
                gates.as_deref(),
                workspace.as_deref(),
            )
        })
        .map(|surface| HookValue::str(surface.id.clone()))
        .collect();
    let facts = HookValue::map([
        ("all_surface_ids", HookValue::Array(all_surface_ids)),
        ("visible_surface_ids", HookValue::Array(visible_surface_ids)),
        ("recording_active", HookValue::Bool(true)),
    ]);

    let result = lunco_hooks::invoke_with_context(
        "runtime.ui.recording",
        &[facts],
        runtime_ui_recording_context(&time),
    );
    let Some(result) = result else {
        return;
    };
    let ids = match result {
        Ok(HookValue::Array(ids)) => ids,
        Ok(value) => {
            contract.error = Some(format!(
                "recording policy returned {value:?}; expected an array of surface IDs"
            ));
            return;
        }
        Err(error) => {
            contract.error = Some(format!("recording policy failed: {error}"));
            return;
        }
    };

    let mut selected = HashSet::new();
    for id in ids {
        let Some(id) = id.as_str() else {
            contract.error = Some("recording policy returned a non-string surface ID".into());
            contract.required_namespaces.clear();
            return;
        };
        let Some(surface) = manifest.surfaces.iter().find(|surface| surface.id == id) else {
            contract.error = Some(format!("recording policy selected unknown surface `{id}`"));
            contract.required_namespaces.clear();
            return;
        };
        if !capture_surface_is_visible(
            surface,
            &exposures,
            layout.as_deref(),
            gates.as_deref(),
            workspace.as_deref(),
        ) {
            contract.error = Some(format!("recording policy selected hidden surface `{id}`"));
            contract.required_namespaces.clear();
            return;
        }
        if !selected.insert(id.to_owned()) {
            contract.error = Some(format!("recording policy selected `{id}` more than once"));
            contract.required_namespaces.clear();
            return;
        }
        contract
            .required_namespaces
            .insert(surface.namespace.clone());
    }
}

fn runtime_ui_recording_context(time: &Time<Real>) -> lunco_core::RuntimeExecutionContext {
    lunco_core::RuntimeExecutionContext {
        target_twin_id: None,
        route: Some(lunco_core::RuntimeRoute::application(
            lunco_core::RuntimeCycle::Ui,
        )),
        phase: lunco_core::RuntimePhase::Preparation,
        clock: lunco_core::RuntimeClock::Application,
        time_seconds: Some(time.elapsed_secs_f64()),
        delta_seconds: Some(time.delta_secs_f64()),
        sequence: None,
        producer: None,
    }
}

/// Mirror the Twin-owned recording contract onto the workbench status bus.
///
/// Readiness is evaluated at the end of Bevy's UI lifecycle. The root must have
/// passed target-camera propagation and layout, and the retained presentation
/// must have been submitted by the render world. The recorder can therefore
/// consume one semantic state transition instead of guessing how many render
/// passes a retained UI needs.
fn report_runtime_ui_readiness(
    exposures: Res<EngineExposures>,
    contract: Res<RuntimeUiRecordingContract>,
    mut presentation_generation: ResMut<RuntimeUiPresentationGeneration>,
    mut roots: Query<(
        Entity,
        &mut RuntimeUiSurface,
        &Visibility,
        Option<&TemplateProperties>,
        Option<&UiTargetCamera>,
        Option<&ComputedUiTargetCamera>,
        Option<&ComputedUiRenderTargetInfo>,
        Option<&ComputedNode>,
    )>,
    pending_images: Query<(Entity, &RuntimeUiImagePending)>,
    parents: Query<&ChildOf>,
    render_state: Option<Res<RuntimeUiRenderState>>,
    bus: Option<ResMut<lunco_status_core::status_bus::StatusBus>>,
) {
    let Some(mut bus) = bus else { return };

    if let Some(error) = &contract.error {
        bus.push(
            lunco_status_core::status_bus::RUNTIME_UI_SOURCE,
            lunco_status_core::status_bus::StatusLevel::Error,
            format!("runtime UI recording policy rejected: {error}"),
        );
        bus.remove_progress(lunco_status_core::status_bus::RUNTIME_UI_SOURCE);
        return;
    }

    let mut required = 0usize;
    let mut ready = 0usize;
    let mut current_generation = 0u64;
    let mut pending_details = Vec::new();
    let mut image_waits = HashMap::<Entity, Vec<String>>::new();
    for (entity, pending) in &pending_images {
        let mut current = entity;
        loop {
            if roots.contains(current) {
                image_waits
                    .entry(current)
                    .or_default()
                    .push(pending.0.clone());
                break;
            }
            let Ok(parent) = parents.get(current) else {
                break;
            };
            current = parent.parent();
        }
    }
    for (
        entity,
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
        let capture_required = contract.required_namespaces.contains(&surface.namespace);
        if !capture_required || !exposure_visible {
            surface.presentation_ready = false;
            surface.presentation_visible = false;
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
        let initial_presentation_ready = surface.mounted
            && surface.applied_placement.is_some()
            && properties.is_some()
            && surface.has_projected_exposure
            && target_ready
            && layout_ready;
        surface.presentation_ready = !image_waits.contains_key(&entity)
            && (surface.presentation_ready || initial_presentation_ready);
        let surface_ready =
            surface.presentation_ready && matches!(*visibility, Visibility::Visible);
        if !surface_ready {
            if let Some(images) = image_waits.get(&entity) {
                pending_details.push(format!(
                    "{}: waiting for images {images:?}",
                    surface.layout_id
                ));
            }
            pending_details.push(format!(
                "{}: mounted={} placement={} properties={} projected={} target={} layout={}",
                surface.layout_id,
                surface.mounted,
                surface.applied_placement.is_some(),
                properties.is_some(),
                surface.has_projected_exposure,
                target_ready,
                layout_ready,
            ));
        }
        presentation_generation.start_if_visible(&mut surface, surface_ready);
        if surface_ready {
            ready += 1;
            current_generation = current_generation.max(surface.presentation_generation);
        }
    }

    let render_ready = required == 0
        || render_state.is_some_and(|state| {
            state.extracted_generation == current_generation
                && state.extracted_surface_count == required as u32
        });
    if required == 0 || (ready == required && render_ready) {
        bus.remove_progress(lunco_status_core::status_bus::RUNTIME_UI_SOURCE);
    } else {
        let message = if ready < required {
            format!(
                "mounting recorded UI surfaces {ready}/{required}: {}",
                pending_details.join("; ")
            )
        } else {
            "waiting for recorded UI render extraction".to_string()
        };
        bus.set_progress(
            lunco_status_core::status_bus::RUNTIME_UI_SOURCE,
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
    // EngineExposures already owns the invalidation boundary for visibility;
    // do not clone every visible namespace on every render extraction.
    let exposure_revision = main_world
        .get_resource::<EngineExposures>()
        .map_or(0, |exposures| exposures.revision);
    if render_ack.visible_exposure_revision != exposure_revision {
        render_ack.visible_namespaces = main_world
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
        render_ack.visible_exposure_revision = exposure_revision;
    }
    let required_namespaces = main_world
        .get_resource::<RuntimeUiRecordingContract>()
        .map(|contract| contract.required_namespaces.clone())
        .unwrap_or_default();

    let (required_roots, presentation_ready_roots, presentation_generation) = {
        let mut roots = main_world.query::<(Entity, &RuntimeUiSurface)>();
        let mut required_roots = EntityHashSet::default();
        let mut presentation_ready_roots = 0;
        let mut presentation_generation = 0;
        for (entity, surface) in roots.iter(&main_world) {
            if required_namespaces.contains(&surface.namespace)
                && render_ack.visible_namespaces.contains(&surface.namespace)
            {
                required_roots.insert(entity);
                if surface.presentation_ready && surface.presentation_visible {
                    presentation_ready_roots += 1;
                    presentation_generation =
                        presentation_generation.max(surface.presentation_generation);
                }
            }
        }
        (
            required_roots,
            presentation_ready_roots,
            presentation_generation,
        )
    };

    // The acknowledgement exists only for authored surfaces that participate
    // in offline capture. Normal runtime HUDs are visible without this
    // cross-world proof, and scenes without an active recording surface have
    // no presentation contract to validate. Avoid traversing every extracted
    // UI node in those steady-state frames.
    if required_roots.is_empty() {
        render_ack.extracted_generation = 0;
        render_ack.extracted_surface_count = 0;
        return;
    }

    let mut extracted_roots = EntityHashSet::default();
    let mut parents = main_world.query::<&ChildOf>();
    for (main_entity, _) in &extracted_nodes.uinodes {
        let mut current = main_entity.id();
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
        && extracted_roots.len() == required_roots.len();
    render_ack.extracted_generation = if all_extracted {
        presentation_generation
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
        state.extracted_generation = render_ack.submitted_generation;
        state.extracted_surface_count = render_ack.submitted_surface_count;
    }
}

/// Mark the UI presentation generation submitted once Bevy's render schedule
/// has completed its render set.
fn acknowledge_runtime_ui_render_submission(mut render_ack: ResMut<RuntimeUiRenderAck>) {
    render_ack.submitted_generation = render_ack.extracted_generation;
    render_ack.submitted_surface_count = render_ack.extracted_surface_count;
}

/// Install the cross-world presentation acknowledgement after Bevy has
/// extracted all authored UI nodes for the current render frame.
fn install_runtime_ui_render_readiness(app: &mut App) {
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

fn spawn_runtime_ui_surface(
    commands: &mut Commands,
    definition: &RuntimeUiSurfaceDefinition,
    server: &AssetServer,
) {
    let admitted = (|| -> Result<_, String> {
        let template = server.load(
            lunco_assets_core::asset_path::load_asset_path(&definition.template, None, None, None)
                .map_err(|error| error.to_string())?,
        );
        let stylesheet = server.load(
            lunco_assets_core::asset_path::load_asset_path(
                &definition.stylesheet,
                None,
                None,
                None,
            )
            .map_err(|error| error.to_string())?,
        );
        RuntimeUiSurface::from_definition(definition, template, stylesheet, Some(server))
    })();
    let surface = match admitted {
        Ok(surface) => surface,
        Err(error) => {
            lunco_core::trigger_runtime_error(commands, "ui-asset-address-invalid", error);
            return;
        }
    };
    let mut entity = commands.spawn((Node::default(), surface, Visibility::Hidden));
    if definition.draggable {
        entity.observe(emit_runtime_ui_surface_drag);
    }
}

fn admit_runtime_ui_manifest(
    event: On<JsonAssetScopeChanged>,
    workspace: Option<Res<WorkspaceResource>>,
    mut state: ResMut<RuntimeUiManifestState>,
) {
    let scope = event.event();
    if let Some(id) = scope.twin_id {
        if !workspace.as_ref().is_some_and(|w| w.twin(id).is_some()) {
            return;
        }
    }
    let result = (|| -> Result<Option<RuntimeUiManifest>, String> {
        let mut selected = None;
        for asset in &scope.assets {
            let JsonAssetContent::Source(Ok(text)) = &asset.content else {
                continue;
            };
            // Other JSON domains report their own invalid source. Only this
            // kind belongs to the runtime-surface owner.
            let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
                continue;
            };
            if value.get("kind").and_then(|v| v.as_str()) != Some(RUNTIME_UI_MANIFEST_KIND) {
                continue;
            }
            let Some(mut manifest) = parse_runtime_ui_manifest(&asset.asset_uri, text)? else {
                continue;
            };
            if selected.is_some() {
                return Err(format!(
                    "{} contains multiple {RUNTIME_UI_MANIFEST_KIND} documents",
                    scope.asset_root_uri
                ));
            }
            for surface in &mut manifest.surfaces {
                for path in [&mut surface.template, &mut surface.stylesheet] {
                    if !path.contains("://") {
                        *path = format!("{}{path}", scope.asset_root_uri);
                    }
                }
                for collection in &mut surface.collections {
                    if !collection.template.contains("://") {
                        collection.template =
                            format!("{}{}", scope.asset_root_uri, collection.template);
                    }
                }
            }
            selected = Some(manifest);
        }
        if scope.twin_id.is_none() && selected.is_none() {
            return Err(format!(
                "{} contains no {RUNTIME_UI_MANIFEST_KIND} document",
                scope.asset_root_uri
            ));
        }
        Ok(selected)
    })();
    state.scopes.insert(scope.twin_id, result);
    state.revision += 1;
}

fn retire_runtime_ui_manifest(event: On<TwinClosed>, mut state: ResMut<RuntimeUiManifestState>) {
    state.scopes.remove(&Some(event.twin));
    state.revision += 1;
    if state.selected == Some(event.twin) {
        state.manifest = None;
        state.applied = None;
    }
}

fn sync_runtime_ui_manifest(
    mut commands: Commands,
    workspace: Option<Res<WorkspaceResource>>,
    mut state: ResMut<RuntimeUiManifestState>,
    roots: Query<Entity, With<RuntimeUiSurface>>,
    server: Res<AssetServer>,
    mut functions: HtmlFunctions,
    mut surface_layouts: ResMut<RuntimeSurfaceLayouts>,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
) {
    let selected = workspace.as_ref().and_then(|w| w.active_twin);
    if state.applied == Some(state.revision) && state.selected == selected {
        return;
    }
    let Some(application) = state.scopes.get(&None) else {
        return;
    };
    let resolved = (|| -> Result<RuntimeUiManifest, String> {
        let mut manifest = application
            .as_ref()
            .map_err(Clone::clone)?
            .clone()
            .ok_or_else(|| "application UI manifest is missing".to_string())?;
        if let Some(twin) = selected.and_then(|id| state.scopes.get(&Some(id))) {
            if let Some(twin) = twin.as_ref().map_err(Clone::clone)? {
                for surface in &twin.surfaces {
                    if let Some(existing) =
                        manifest.surfaces.iter_mut().find(|s| s.id == surface.id)
                    {
                        *existing = surface.clone();
                    } else {
                        manifest.surfaces.push(surface.clone());
                    }
                }
            }
        }
        manifest.validate()?;
        Ok(manifest)
    })();
    state.selected = selected;
    state.applied = Some(state.revision);
    state.rebuild_pending = true;
    for root in &roots {
        commands.entity(root).try_despawn();
    }
    let manifest = match resolved {
        Ok(manifest) => manifest,
        Err(error) => {
            state.manifest = None;
            state.error = Some(error.clone());
            faults.raise(
                "runtime-ui-manifest-invalid",
                None,
                "active runtime UI manifest",
                &error,
            );
            lunco_core::trigger_runtime_error(&mut commands, "runtime-ui-manifest-invalid", error);
            return;
        }
    };
    state.manifest = Some(manifest.clone());
    state.error = None;
    register_dynamic_action(&mut functions);
    let ids = manifest
        .surfaces
        .iter()
        .map(|surface| surface.id.clone())
        .collect::<HashSet<_>>();
    surface_layouts.retain_ids(&ids);
    for surface in &manifest.surfaces {
        for action in &surface.actions {
            register_action(
                &mut functions,
                action.callback.clone(),
                action.action.clone(),
            );
        }
        spawn_runtime_ui_surface(&mut commands, surface, &server);
    }
}

fn parse_runtime_ui_manifest(
    path: &str,
    source: &str,
) -> Result<Option<RuntimeUiManifest>, String> {
    let value = serde_json::from_str::<serde_json::Value>(source)
        .map_err(|error| format!("{path}: invalid JSON: {error}"))?;
    if value.get("kind").and_then(serde_json::Value::as_str) != Some(RUNTIME_UI_MANIFEST_KIND) {
        return Ok(None);
    }
    let manifest = serde_json::from_value::<RuntimeUiManifest>(value)
        .map_err(|error| format!("{path}: invalid runtime UI manifest: {error}"))?;
    manifest
        .validate()
        .map_err(|error| format!("{path}: {error}"))?;
    Ok(Some(manifest))
}

/// Attach HUI/Flair only to surfaces that can currently be shown. Runtime
/// manifests may describe many optional overlays, but hidden trees still incur
/// HUI compilation, CSS matching, and Bevy UI layout cost if they are mounted.
/// A hidden surface has no presentation root. When its exposure becomes
/// visible again, this system creates a fresh root from the authoritative
/// manifest before HUI builds it. This keeps HUI's private build state and all
/// template-owned presentation components within one lifecycle.
pub fn mount_runtime_ui_surfaces(
    mut commands: Commands,
    manifest_state: Res<RuntimeUiManifestState>,
    server: Res<AssetServer>,
    exposures: Res<EngineExposures>,
    layout: Option<Res<WorkbenchSnapshot>>,
    gates: Option<Res<RuntimeUiGates>>,
    surface_layouts: Res<RuntimeSurfaceLayouts>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
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

    let Some(manifest) = manifest_state.manifest() else {
        return;
    };
    let existing_namespaces: HashSet<String> = roots
        .iter()
        .map(|(_, surface, _)| surface.namespace.clone())
        .collect();
    for definition in &manifest.surfaces {
        if existing_namespaces.contains(&definition.namespace)
            || !exposures
                .surfaces
                .get(&definition.namespace)
                .is_some_and(|exposure| exposure.visible)
        {
            continue;
        }
        spawn_runtime_ui_surface(&mut commands, definition, &server);
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
            || !runtime_ui_is_allowed(
                &surface,
                layout.as_deref(),
                gates.as_deref(),
                workspace.as_deref(),
            )
            || resolve_surface_placement(
                &surface,
                &surface_layouts,
                rects.as_deref(),
                target,
                window,
            )
            .is_none()
        {
            continue;
        }

        commands.entity(entity).try_insert((
            HtmlNode(surface.template.clone()),
            Styled::new(surface.stylesheet.clone()),
            InlineStyle::default(),
        ));
        surface.mounted = true;
    }
}

fn runtime_ui_is_allowed(
    surface: &RuntimeUiSurface,
    layout: Option<&WorkbenchSnapshot>,
    gates: Option<&RuntimeUiGates>,
    workspace: Option<&lunco_workspace::WorkspaceResource>,
) -> bool {
    runtime_ui_visibility_allows(
        surface.visible_in_perspective.as_ref(),
        surface.gate.as_deref(),
        surface.setting.as_deref(),
        surface.setting_default,
        layout,
        gates,
        workspace,
    )
}

/// One authored visibility predicate for mounting, retained rendering and the
/// recording hook's candidate facts. Eligibility deliberately does not depend
/// on mounting or render acknowledgement: required visible surfaces must still
/// pass those later readiness fences before capture.
fn runtime_ui_visibility_allows(
    perspective: Option<&RuntimeUiPerspectiveFilter>,
    gate: Option<&str>,
    setting: Option<&str>,
    setting_default: bool,
    layout: Option<&WorkbenchSnapshot>,
    gates: Option<&RuntimeUiGates>,
    workspace: Option<&lunco_workspace::WorkspaceResource>,
) -> bool {
    let perspective_visible = perspective.is_none_or(|required| required.allows(layout));
    let gate_visible = gate.is_none_or(|gate| gates.is_some_and(|gates| gates.allows(gate)));
    let setting_visible =
        setting.is_none_or(|key| active_twin_setting_is_true(workspace, key, setting_default));
    perspective_visible && gate_visible && setting_visible
}

fn capture_surface_is_visible(
    surface: &RuntimeUiSurfaceDefinition,
    exposures: &EngineExposures,
    layout: Option<&WorkbenchSnapshot>,
    gates: Option<&RuntimeUiGates>,
    workspace: Option<&lunco_workspace::WorkspaceResource>,
) -> bool {
    exposures
        .surfaces
        .get(&surface.namespace)
        .is_some_and(|exposure| exposure.visible)
        && runtime_ui_visibility_allows(
            surface.visible_in_perspective.as_ref(),
            surface.gate.as_deref(),
            surface.setting.as_deref(),
            surface.setting_default,
            layout,
            gates,
            workspace,
        )
}

fn active_twin_setting_is_true(
    workspace: Option<&lunco_workspace::WorkspaceResource>,
    key: &str,
    default: bool,
) -> bool {
    let Some(workspace) = workspace else {
        return default;
    };
    let Some(twin_id) = workspace.active_twin else {
        return default;
    };
    match workspace
        .twin(twin_id)
        .and_then(|twin| twin.manifest.as_ref())
        .and_then(|manifest| manifest.setting(key))
    {
        Some(lunco_twin::TwinSettingValue::Bool(value)) => *value,
        Some(_) => false,
        None => default,
    }
}

/// Keep retained HTML surfaces on the presentation camera.
///
/// Windowed runs have a `PrimaryEguiContext` camera. Windowless recording does
/// not create a window or egui host, so the same authored surface is bound to
/// the active authored `SceneCamera` instead. This is what makes a runtime HUD
/// part of the captured render target rather than editor chrome.
fn bind_runtime_ui_to_camera(
    mut commands: Commands,
    viewport: Option<Res<SceneViewport>>,
    cameras: Query<(Entity, &Camera, Has<PrimaryEguiContext>, Has<SceneCamera>)>,
    ui_picking_cameras: Query<(), With<UiPickingCamera>>,
    roots: Query<(Entity, Option<&UiTargetCamera>), With<RuntimeUiSurface>>,
) {
    // Windowed runtime UI is owned by the single egui host. Windowless
    // recording has no host and therefore uses the viewport's explicitly
    // resolved authored scene camera. Neither branch selects by ECS order.
    let mut egui_cameras = cameras
        .iter()
        .filter(|(_, _, is_egui, _)| *is_egui)
        .map(|(entity, _, _, _)| entity);
    let egui_camera = match (egui_cameras.next(), egui_cameras.next()) {
        (None, None) => None,
        (Some(camera), None) => Some(camera),
        (Some(_), Some(_)) => {
            error!("runtime UI binding refused: multiple PrimaryEguiContext cameras");
            return;
        }
        (None, Some(_)) => unreachable!("an iterator cannot yield a second item without a first"),
    };
    let camera = egui_camera.or_else(|| {
        let entity = viewport.as_deref()?.active_camera?;
        cameras
            .get(entity)
            .ok()
            .filter(|(_, camera, _, is_scene)| *is_scene && camera.is_active)
            .map(|(entity, _, _, _)| entity)
    });
    let Some(camera) = camera else {
        return;
    };
    if ui_picking_cameras.get(camera).is_err() {
        commands.entity(camera).insert(UiPickingCamera);
    }

    for (entity, target) in &roots {
        if target.is_none_or(|target| target.entity() != camera) {
            commands.entity(entity).try_insert(UiTargetCamera(camera));
        }
    }
}

/// Bridge HUI's stable IDs to Bevy names, which is the selector identity used
/// by Flair. This is shared by all runtime-authored templates.
fn attach_runtime_ui_names(
    mut commands: Commands,
    ids: Query<(Entity, &UiId), (With<Node>, Or<(Added<UiId>, Changed<UiId>)>)>,
) {
    for (entity, id) in &ids {
        commands
            .entity(entity)
            .try_insert(Name::new(id.id().clone()));
    }
}

/// HUI's inline-style cache is not the style authority for runtime surfaces;
/// Flair's stylesheet is. Remove only HUI style components below a runtime
/// surface, leaving unrelated HUI templates untouched.
fn hand_runtime_ui_styling_to_flair(
    mut commands: Commands,
    roots: Query<Entity, With<RuntimeUiSurface>>,
    nodes: Query<(Entity, &HtmlStyle), Or<(Added<HtmlStyle>, Changed<HtmlStyle>)>>,
    parents: Query<&ChildOf>,
) {
    for (entity, style) in &nodes {
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
            if let Some(pickable) = style.computed.pickable {
                commands.entity(entity).try_insert(pickable);
            }
            commands
                .entity(entity)
                .insert(RuntimeUiTemplateStyle(style.clone()))
                .remove::<HtmlStyle>();
        }
    }
}

/// Apply only changed exposure snapshots to retained HUI properties and Flair
/// custom properties. Parsing, entity creation, and style writes therefore do
/// not run on idle render frames.
fn apply_runtime_ui_exposures(
    mut commands: Commands,
    exposures: Res<EngineExposures>,
    mut manifest_state: ResMut<RuntimeUiManifestState>,
    layout: Option<Res<WorkbenchSnapshot>>,
    gates: Option<Res<RuntimeUiGates>>,
    surface_layouts: Res<RuntimeSurfaceLayouts>,
    recording_contract: Option<Res<RuntimeUiRecordingContract>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    rects: Option<Res<PanelRects>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    theme: Option<Res<lunco_theme::Theme>>,
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
    let surface_layouts_changed = surface_layouts.is_changed();
    let workspace_changed = workspace
        .as_ref()
        .is_some_and(|workspace| workspace.is_changed());
    let theme_changed = theme.as_ref().is_some_and(|theme| theme.is_changed());
    let window = windows.iter().next();

    for (entity, mut surface, mut node, mut visibility, properties, existing_style, target) in
        &mut roots
    {
        let placement =
            resolve_surface_placement(&surface, &surface_layouts, rects.as_deref(), target, window);
        let placement_changed = surface.applied_placement != placement;
        let exposure = exposures.surfaces.get(&surface.namespace);
        let should_be_visible = exposure.is_some_and(|exposure| exposure.visible)
            && runtime_ui_is_allowed(
                &surface,
                layout.as_deref(),
                gates.as_deref(),
                workspace.as_deref(),
            )
            && placement.is_some();
        // Only recording surfaces need to wait for the cross-world render
        // acknowledgement. Ordinary runtime UI remains a live presentation
        // surface once its exposure, placement, and authored visibility rules
        // allow it. `presentation_ready` is intentionally maintained only for
        // the recording contract; applying it to every surface leaves normal
        // HUDs hidden forever because `report_runtime_ui_readiness` does not
        // arm non-recording surfaces.
        let capture_required = recording_contract
            .as_deref()
            .is_some_and(|contract| contract.required_namespaces.contains(&surface.namespace));
        let presentation_visible =
            should_be_visible && (!capture_required || surface.presentation_ready);
        // Scene teardown and HUI template rebuilds can reset the root's Bevy
        // visibility without changing the engine exposure revision. The
        // revision remains the property/style fast path, but visibility is a
        // presentation lifecycle invariant and must be repaired independently.
        let visibility_matches = if presentation_visible {
            matches!(*visibility, Visibility::Visible)
        } else {
            matches!(*visibility, Visibility::Hidden)
        };
        let visibility_changed = !visibility_matches;
        let has_retained_presentation =
            surface.mounted || properties.is_some() || existing_style.is_some();
        if !should_be_visible && has_retained_presentation {
            // HUI builds the template root on this entity itself and keeps
            // private build-state components on it. Despawn the complete
            // presentation root whenever any retained HUI state exists, not
            // only when our lifecycle flag says it is mounted. The latter can
            // be false after a scene teardown or a deferred HUI rebuild while
            // the root is still present; leaving that tree alive is how a
            // completed terrain overlay remained visible after its exposure
            // had become false.
            //
            // Visibility is changed in the main world before the deferred
            // despawn is applied. Render extraction runs after Update, so a
            // root that is already in the render world's previous frame
            // cannot leak one more visible frame while the command queue is
            // being flushed.
            *visibility = Visibility::Hidden;
            commands.entity(entity).try_despawn();
            continue;
        }
        if surface.applied_revision == exposures.revision
            && !layout_changed
            && !gates_changed
            && !surface_layouts_changed
            && !workspace_changed
            && !theme_changed
            && !placement_changed
            && !visibility_changed
        {
            continue;
        }

        if let Some(placement) = placement {
            if placement_changed {
                apply_placement(&mut node, placement.rect);
            }
        }
        surface.applied_placement = placement;

        let Some(exposure) = exposure else {
            *visibility = Visibility::Hidden;
            surface.applied_revision = exposures.revision;
            continue;
        };

        *visibility = if presentation_visible {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };

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
        let default_theme;
        let theme = if let Some(theme) = theme.as_deref() {
            theme
        } else {
            default_theme = lunco_theme::Theme::dark();
            &default_theme
        };
        apply_runtime_theme(theme, &mut style, &mut style_changed);
        if surface.bindings.is_empty() {
            for (name, value) in &exposure.properties {
                if let Some(rendered) = value.scalar_render() {
                    apply_runtime_ui_property(
                        name,
                        rendered,
                        false,
                        &mut properties,
                        &mut style,
                        &mut properties_changed,
                        &mut style_changed,
                    );
                }
            }
        } else {
            for (target, binding) in &surface.bindings {
                let Some(value) = exposure.properties.get(&binding.source) else {
                    continue;
                };
                let Some(source_value) = value.scalar_render() else {
                    continue;
                };
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
                    binding.css,
                    &mut properties,
                    &mut style,
                    &mut properties_changed,
                    &mut style_changed,
                );
            }
        }

        if style_changed {
            commands.entity(entity).try_insert(style);
        }
        if properties_changed {
            commands.queue(move |world: &mut World| compile_runtime_ui_context(world, entity));
        }
        surface.has_projected_exposure = true;
        surface.applied_revision = exposures.revision;
    }
}

/// Reconcile every authored collection host against one typed exposure array.
/// This is the only collection mechanic: it knows neither programs nor
/// cameras, and it never serializes rows through JSON. Rhai supplies ordered
/// records and semantic action strings; HUI supplies the row template.
fn reconcile_runtime_ui_collections(
    mut commands: Commands,
    exposures: Res<EngineExposures>,
    roots: Query<(Entity, &RuntimeUiSurface, &Visibility)>,
    ids: Query<(Entity, &UiId), With<Node>>,
    parents: Query<&ChildOf>,
    rows: Query<(Entity, &RuntimeUiCollectionRow)>,
    hosts: Query<(), With<RuntimeUiCollectionHost>>,
    mut properties: Query<&mut TemplateProperties>,
    styles: Query<&InlineStyle>,
    scrollbars: Query<(), With<bevy_ui_widgets::Scrollbar>>,
    theme: Option<Res<lunco_theme::Theme>>,
) {
    for (surface_entity, surface, visibility) in &roots {
        if !surface.mounted || !matches!(*visibility, Visibility::Visible) {
            continue;
        }
        let Some(exposure) = exposures.surfaces.get(&surface.namespace) else {
            continue;
        };
        for collection in &surface.collections {
            let Some(host) = ids.iter().find_map(|(entity, id)| {
                (id.id() == &collection.container
                    && is_descendant_of_runtime_surface(
                        entity,
                        &HashSet::from([surface_entity]),
                        &parents,
                    ))
                .then_some(entity)
            }) else {
                continue;
            };
            if hosts.get(host).is_err() {
                commands.entity(host).insert((
                    RuntimeUiCollectionHost,
                    ScrollPosition::default(),
                    bevy_ui_widgets::ScrollArea,
                    Pickable::default(),
                ));
            }

            if let Some(track_id) = &collection.scrollbar {
                if let Some(track) = ids.iter().find_map(|(entity, id)| {
                    (id.id() == track_id
                        && is_descendant_of_runtime_surface(
                            entity,
                            &HashSet::from([surface_entity]),
                            &parents,
                        ))
                    .then_some(entity)
                }) && scrollbars.get(track).is_err()
                {
                    let default_theme;
                    let theme = match theme.as_deref() {
                        Some(theme) => theme,
                        None => {
                            default_theme = lunco_theme::Theme::default();
                            &default_theme
                        }
                    };
                    let color = theme.tokens.accent;
                    commands.entity(track).try_insert((
                        bevy_ui_widgets::Scrollbar {
                            target: host,
                            orientation: bevy_ui_widgets::ControlOrientation::Vertical,
                            min_thumb_length: theme.spacing.titlebar_control_size.y,
                        },
                        Pickable::default(),
                    ));
                    commands.spawn((
                        RuntimeUiScrollbarThumb,
                        bevy_ui_widgets::ScrollbarThumb {
                            border_radius: BorderRadius::all(px(theme.rounding.button)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba_u8(
                            color.r(),
                            color.g(),
                            color.b(),
                            color.a(),
                        )),
                        Pickable::default(),
                        ChildOf(track),
                    ));
                }
            }

            let values = match exposure.properties.get(&collection.source) {
                Some(lunco_exposure_core::ExposureValue::Array(values)) => values.as_slice(),
                Some(value) => {
                    warn!(
                        surface = surface.namespace,
                        source = collection.source,
                        value = ?value,
                        "[runtime-ui] collection source is not an array"
                    );
                    &[]
                }
                None => &[],
            };

            let mut existing = HashMap::new();
            for (entity, row) in &rows {
                if row.host == host {
                    existing.insert(row.key.clone(), entity);
                }
            }

            let mut ordered = Vec::with_capacity(values.len());
            let mut seen = HashSet::new();
            for value in values {
                let Some(fields) = collection_item_fields(value, &collection.key) else {
                    warn!(
                        surface = surface.namespace,
                        source = collection.source,
                        "[runtime-ui] collection item is not a keyed record; skipped"
                    );
                    continue;
                };
                let Some(key) = fields
                    .iter()
                    .find(|(name, _)| name == &collection.key)
                    .map(|(_, value)| value.clone())
                else {
                    continue;
                };
                if !seen.insert(key.clone()) {
                    warn!(
                        surface = surface.namespace,
                        source = collection.source,
                        key,
                        "[runtime-ui] collection contains a duplicate key; skipped"
                    );
                    continue;
                }

                let entity = if let Some(entity) = existing.remove(&key) {
                    if let Ok(mut state) = properties.get_mut(entity) {
                        let previous = rows
                            .get(entity)
                            .ok()
                            .map(|(_, row)| row.property_names.clone())
                            .unwrap_or_default();
                        let mut changed = false;
                        for name in previous {
                            if !fields.iter().any(|(field, _)| field == &name) {
                                changed |= state.remove(&name).is_some();
                            }
                        }
                        for (name, value) in &fields {
                            if state.get(name).map(String::as_str) != Some(value.as_str()) {
                                state.set(name, value);
                                changed = true;
                            }
                        }
                        if changed {
                            commands.queue(move |world: &mut World| {
                                compile_runtime_ui_context(world, entity)
                            });
                        }
                    }
                    commands.entity(entity).insert(RuntimeUiCollectionRow {
                        host,
                        key: key.clone(),
                        property_names: fields.iter().map(|(name, _)| name.clone()).collect(),
                    });
                    entity
                } else {
                    let mut state = TemplateProperties::default();
                    for (name, value) in &fields {
                        state.set(name, value);
                    }
                    commands
                        .spawn((
                            Node::default(),
                            HtmlNode(collection.template.clone()),
                            state,
                            RuntimeUiCollectionRow {
                                host,
                                key: key.clone(),
                                property_names: fields
                                    .iter()
                                    .map(|(name, _)| name.clone())
                                    .collect(),
                            },
                        ))
                        .id()
                };
                let mut style = styles.get(entity).cloned().unwrap_or_default();
                let mut style_changed = false;
                for name in &collection.css {
                    let css_name = format!("--ui-{}", name.replace('_', "-"));
                    match fields.iter().find(|(field, _)| field == name) {
                        Some((_, value)) if !value.trim().is_empty() => {
                            if style.get(&css_name) != Some(value.as_str()) {
                                style.set(css_name, value.clone());
                                style_changed = true;
                            }
                        }
                        _ => {
                            if style.get(&css_name).is_some() {
                                style.remove(&css_name);
                                style_changed = true;
                            }
                        }
                    }
                }
                if style_changed {
                    commands.entity(entity).try_insert(style);
                }
                ordered.push(entity);
            }

            for (_, entity) in existing {
                commands
                    .entity(entity)
                    .despawn_related::<Children>()
                    .try_despawn();
            }
            commands.entity(host).replace_children(&ordered);
        }
    }
}

/// Native thumb styling follows the same application theme as authored CSS.
fn sync_runtime_ui_scrollbar_theme(
    theme: Option<Res<lunco_theme::Theme>>,
    mut thumbs: Query<&mut BackgroundColor, With<RuntimeUiScrollbarThumb>>,
) {
    let Some(theme) = theme else {
        return;
    };
    if !theme.is_changed() {
        return;
    }
    let color = theme.tokens.accent;
    for mut background in &mut thumbs {
        background.0 = Color::srgba_u8(color.r(), color.g(), color.b(), color.a());
    }
}

pub fn collection_item_fields(
    value: &lunco_exposure_core::ExposureValue,
    key: &str,
) -> Option<Vec<(String, String)>> {
    let lunco_exposure_core::ExposureValue::Map(fields) = value else {
        return None;
    };
    let fields = fields
        .iter()
        .filter_map(|(name, value)| value.scalar_render().map(|value| (name.clone(), value)))
        .collect::<Vec<_>>();
    fields
        .iter()
        .any(|(name, value)| name == key && !value.is_empty())
        .then_some(fields)
}

fn apply_runtime_theme(theme: &lunco_theme::Theme, style: &mut InlineStyle, changed: &mut bool) {
    let tokens = &theme.tokens;
    let values = [
        ("--panel-background", css_color(tokens.overlay_backdrop)),
        ("--panel-border", css_color(tokens.overlay_border)),
        ("--panel-inner", css_color(tokens.surface_sunken)),
        ("--button-background", css_color(tokens.surface_raised)),
        ("--button-border", css_color(tokens.surface_raised_border)),
        ("--text-color", css_color(tokens.text)),
        ("--muted-color", css_color(tokens.text_subdued)),
        ("--accent-color", css_color(tokens.accent)),
        ("--active-text-color", css_color(tokens.surface_sunken)),
        ("--ok-color", css_color(tokens.success)),
        ("--caution-color", css_color(tokens.warning)),
        ("--warm-color", css_color(tokens.warning)),
        ("--danger-color", css_color(tokens.error)),
    ];
    for (name, value) in values {
        if style.get(name) != Some(value.as_str()) {
            style.set(name, value);
            *changed = true;
        }
    }
}

fn css_color(color: egui::Color32) -> String {
    format!(
        "rgba({}, {}, {}, {:.3})",
        color.r(),
        color.g(),
        color.b(),
        f32::from(color.a()) / 255.0
    )
}

/// Apply the manifest rectangle after HUI has finished replacing the template
/// root's [`Node`] and Flair has applied authored CSS.
///
/// This is change-detected on the root node. It runs for the initial HUI build
/// (and for an actual resize/style rebuild), then goes dormant; there is no
/// per-frame geometry write. The manifest owns the outer rectangle and the
/// stylesheet owns the contents of that rectangle.
fn apply_runtime_ui_placement_after_style(
    rects: Option<Res<PanelRects>>,
    surface_layouts: Res<RuntimeSurfaceLayouts>,
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
        let placement =
            resolve_surface_placement(&surface, &surface_layouts, rects.as_deref(), target, window);
        if surface.applied_placement != placement {
            surface.applied_placement = placement;
        }
        let Some(placement) = placement else {
            continue;
        };
        let rect = placement.rect;
        if !placement_is_applied(&node, rect) {
            apply_placement(&mut node, rect);
        }
    }
}

fn apply_runtime_ui_property(
    name: &str,
    rendered: String,
    css: bool,
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
    if !css || rendered.trim().is_empty() {
        // Text-only bindings stay outside Flair's token stream. Empty authored
        // CSS values also remove their override.
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

/// Apply pointer-driven movement to authored draggable window surfaces.
///
/// Picking events bubble from any HUI child to the surface root, so templates
/// remain data-only. The persisted position is logical, clamped to the live
/// target, and written through Workbench's per-Twin workspace state. A
/// primary-button double click removes the override and restores the authored
/// anchor/default.
fn apply_runtime_ui_surface_interactions(
    mut drags: MessageReader<RuntimeUiSurfaceDragged>,
    mut resets: MessageReader<RuntimeUiSurfaceReset>,
    mut layouts: ResMut<RuntimeSurfaceLayouts>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut roots: Query<(
        &mut RuntimeUiSurface,
        &mut Node,
        Option<&ComputedUiRenderTargetInfo>,
    )>,
) {
    let window = windows.iter().next();
    for drag in drags.read() {
        let Ok((mut surface, mut node, target)) = roots.get_mut(drag.entity) else {
            continue;
        };
        if !surface.draggable {
            continue;
        }
        let Some(current) = surface.applied_placement else {
            continue;
        };
        let Some((_, target_size)) = logical_target_dimensions(target, window) else {
            continue;
        };
        let rect = resolve_dragged_surface_rect(current.rect, drag.delta, target_size);
        layouts.set(
            surface.layout_id.clone(),
            RuntimeSurfaceLayout {
                left: rect.min.x,
                top: rect.min.y,
            },
        );
        apply_placement(&mut node, rect);
        surface.applied_placement = Some(ResolvedRuntimeUiPlacement { rect });
    }

    for reset in resets.read() {
        let Ok((mut surface, mut node, target)) = roots.get_mut(reset.entity) else {
            continue;
        };
        if !surface.draggable {
            continue;
        }
        layouts.reset(&surface.layout_id);
        let Some(placement) = resolve_surface_placement(&surface, &layouts, None, target, window)
        else {
            continue;
        };
        apply_placement(&mut node, placement.rect);
        surface.applied_placement = Some(placement);
    }
}

/// Resolve one pointer drag in the surface's logical coordinate space.
///
/// Bevy's pointer events report `Drag::delta` in the same logical window
/// pixels used by `ComputedUiRenderTargetInfo` and the authored placement. Do
/// not divide it by the render-target scale factor: doing so applies the DPI
/// conversion twice and makes a drag move only a fraction of the pointer
/// distance on high-DPI windows.
fn resolve_dragged_surface_rect(current: egui::Rect, delta: Vec2, target_size: Vec2) -> egui::Rect {
    let size = Vec2::new(current.width(), current.height()).min(target_size);
    let max_origin = (target_size - size).max(Vec2::ZERO);
    let current_origin = Vec2::new(current.min.x, current.min.y);
    let origin = (current_origin + delta).clamp(Vec2::ZERO, max_origin);
    egui::Rect::from_min_size(egui::pos2(origin.x, origin.y), egui::vec2(size.x, size.y))
}

fn runtime_ui_surface_ancestor(
    hit: Entity,
    roots: &Query<(Entity, &RuntimeUiSurface)>,
    parents: &Query<&ChildOf>,
) -> Option<Entity> {
    let mut current = hit;
    for _ in 0..64 {
        if roots.get(current).is_ok() {
            return Some(current);
        }
        current = parents.get(current).ok()?.parent();
    }
    None
}

/// HUI supplies callback names; the runtime owns event-driven press delivery.
/// A press and release in the same frame must still invoke the callback once.
#[derive(Component)]
struct RuntimeUiPressCallbacks(Vec<String>);

fn dispatch_runtime_ui_press(
    mut press: On<bevy::picking::events::PointerPress>,
    callbacks: Query<(&RuntimeUiPressCallbacks, Has<bevy::ui::InteractionDisabled>)>,
    bindings: Res<FunctionBindings>,
    mut commands: Commands,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    let Ok((callbacks, disabled)) = callbacks.get(press.entity) else {
        return;
    };
    press.propagate(false);
    if disabled {
        return;
    }
    for callback in &callbacks.0 {
        bindings.maybe_run(callback, press.entity, &mut commands);
    }
}

/// Add the visible interactive runtime controls to the existing workbench
/// scene/chrome gate. This is intentionally a bridge into `ScenePickGate`, not
/// a second hit-test implementation: egui dock cards and runtime UI controls
/// are resolved by the same press latch and pointer ownership state machine.
/// Native controls block the scene without asking egui's capture backend to
/// intercept the Bevy UI hit that must deliver their press.
///
/// The surface root is a layout container, not an input target. In particular,
/// a `viewport` surface is intentionally full-window, so registering its
/// placement rectangle would make the complete 3D scene non-interactive. HUI's
/// explicit `OnUiPress` marker is the input contract; Bevy's computed node
/// geometry supplies the actual child hit rectangle. These controls also
/// receive `Pickable` so Bevy's marker-filtered UI backend can ignore layout
/// nodes that do not own pointer input.
fn register_runtime_ui_input_regions(
    mut commands: Commands,
    roots: Query<(Entity, &RuntimeUiSurface, &Visibility)>,
    controls: Query<(
        Entity,
        &ComputedNode,
        &UiGlobalTransform,
        Option<&InheritedVisibility>,
        Option<&OnUiPress>,
        Has<RuntimeUiPressCallbacks>,
        Option<&Pickable>,
        Has<bevy_ui_widgets::ScrollArea>,
        Has<bevy_ui_widgets::Scrollbar>,
        Has<RuntimeUiScrollbarThumb>,
    )>,
    parents: Query<&ChildOf>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut gate: ResMut<ScenePickGate>,
) {
    let interactive_roots: HashSet<Entity> = roots
        .iter()
        .filter_map(|(entity, surface, visibility)| {
            (surface.interactive && matches!(*visibility, Visibility::Visible)).then_some(entity)
        })
        .collect();
    let draggable_roots: HashSet<Entity> = roots
        .iter()
        .filter_map(|(entity, surface, visibility)| {
            (surface.draggable && matches!(*visibility, Visibility::Visible)).then_some(entity)
        })
        .collect();

    if interactive_roots.is_empty() && draggable_roots.is_empty() {
        return;
    }

    if roots
        .iter()
        .any(|(_, surface, visibility)| surface.modal && matches!(*visibility, Visibility::Visible))
    {
        if let Some(window) = windows.iter().next() {
            let rect = egui::Rect::from_min_max(
                egui::pos2(0.0, 0.0),
                egui::pos2(window.width(), window.height()),
            );
            gate.record_chrome_panel(rect, rect);
        }
    }

    for (
        entity,
        node,
        transform,
        inherited_visibility,
        press,
        callbacks,
        pickable,
        scroll_area,
        scrollbar,
        thumb,
    ) in &controls
    {
        if !inherited_visibility.is_some_and(|visibility| visibility.get()) {
            continue;
        }

        let explicit_control = (press.is_some()
            || callbacks
            || scroll_area
            || scrollbar
            || thumb
            || pickable.is_some_and(|pickable| pickable.should_block_lower))
            && is_descendant_of_runtime_surface(entity, &interactive_roots, &parents);
        if explicit_control && let Some(press) = press {
            // Transfer callback ownership, so HUI's state-sampling callback
            // cannot dispatch a second invocation for a held press.
            commands
                .entity(entity)
                .try_insert(RuntimeUiPressCallbacks(press.0.clone()))
                .try_remove::<OnUiPress>();
        }
        let draggable_content =
            is_descendant_of_runtime_surface(entity, &draggable_roots, &parents);
        if (explicit_control || draggable_content) && pickable.is_none() {
            // Scene teardown may retire the surface before this queue applies.
            commands.entity(entity).try_insert(Pickable::default());
        }

        if !explicit_control {
            continue;
        }
        if let Some(rect) = runtime_ui_input_rect(node, transform) {
            gate.record_native_ui_region(rect);
        }
    }
}

/// Route Escape through the same authored semantic action path as a button.
/// The runtime owns only the key-to-dismiss primitive; Rhai owns the resulting
/// lifecycle decision.
fn dismiss_runtime_ui(
    keys: Res<ButtonInput<KeyCode>>,
    roots: Query<(Entity, &RuntimeUiSurface, &Visibility)>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    scene: Option<Res<lunco_core::SceneTransitionCoordinator>>,
    exposures: Res<EngineExposures>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    let Some((source, surface, _)) = roots.iter().find(|(_, surface, visibility)| {
        surface.dismiss_action.is_some() && matches!(*visibility, Visibility::Visible)
    }) else {
        return;
    };
    let Some(action) = surface.dismiss_action.clone() else {
        return;
    };
    let mut scope = RuntimeUiActionScope::capture(workspace.as_deref(), scene.as_deref());
    if exposures.is_application_surface(&surface.namespace) {
        scope.lifetime = lunco_core::InteractionLifetime::Owner;
    }
    commands.trigger(RuntimeUiAction {
        scope,
        action,
        source,
        parameters: HookValue::Map(Vec::new()),
    });
}

fn is_descendant_of_runtime_surface(
    entity: Entity,
    roots: &HashSet<Entity>,
    parents: &Query<&ChildOf>,
) -> bool {
    let mut current = entity;
    loop {
        if roots.contains(&current) {
            return true;
        }
        let Ok(child_of) = parents.get(current) else {
            return false;
        };
        current = child_of.parent();
    }
}

/// Computed node bounds in logical window coordinates, shared by input and
/// scene-label occlusion so DPI and retained layout use the same geometry.
pub fn runtime_ui_input_rect(
    node: &ComputedNode,
    transform: &UiGlobalTransform,
) -> Option<egui::Rect> {
    if node.is_empty() || !node.inverse_scale_factor.is_finite() || node.inverse_scale_factor <= 0.0
    {
        return None;
    }

    let local = node.border_box();
    let corners = [
        local.min,
        Vec2::new(local.max.x, local.min.y),
        local.max,
        Vec2::new(local.min.x, local.max.y),
    ];
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    for corner in corners {
        let point = transform.affine().transform_point2(corner) * node.inverse_scale_factor;
        if !point.is_finite() {
            return None;
        }
        min = min.min(point);
        max = max.max(point);
    }
    (max.x > min.x && max.y > min.y)
        .then(|| egui::Rect::from_min_max(egui::pos2(min.x, min.y), egui::pos2(max.x, max.y)))
}

fn resolve_surface_placement(
    surface: &RuntimeUiSurface,
    layouts: &RuntimeSurfaceLayouts,
    rects: Option<&PanelRects>,
    target: Option<&ComputedUiRenderTargetInfo>,
    window: Option<&Window>,
) -> Option<ResolvedRuntimeUiPlacement> {
    let placement = resolve_placement(&surface.placement, rects, target, window)?;
    let Some(layout) = layouts.get(&surface.layout_id) else {
        return Some(placement);
    };
    if !surface.draggable {
        return Some(placement);
    }
    let (_, target_size) = logical_target_dimensions(target, window)?;
    let size = Vec2::new(placement.rect.width(), placement.rect.height()).min(target_size);
    let max_origin = (target_size - size).max(Vec2::ZERO);
    let origin = Vec2::new(layout.left, layout.top).clamp(Vec2::ZERO, max_origin);
    Some(ResolvedRuntimeUiPlacement {
        rect: egui::Rect::from_min_size(egui::pos2(origin.x, origin.y), egui::vec2(size.x, size.y)),
    })
}

fn logical_target_dimensions(
    target: Option<&ComputedUiRenderTargetInfo>,
    window: Option<&Window>,
) -> Option<(f32, Vec2)> {
    // The UI target component is propagated before the camera has a live
    // viewport on the first frame, so its default is a valid-looking 1.0 scale
    // paired with a 0x0 physical size. Do not let that placeholder shadow the
    // already-valid window dimensions.
    let target_dimensions = target.and_then(|target| {
        let scale = target.scale_factor();
        let physical_size = target.physical_size();
        (scale.is_finite() && scale > 0.0 && physical_size.x > 0 && physical_size.y > 0)
            .then_some((scale, physical_size))
    });
    let (scale, physical_size) = target_dimensions
        .or_else(|| window.map(|window| (window.scale_factor(), window.physical_size())))?;
    if !scale.is_finite() || scale <= 0.0 || physical_size.x == 0 || physical_size.y == 0 {
        return None;
    }
    Some((scale, physical_size.as_vec2() / scale))
}

fn resolve_placement(
    placement: &RuntimeUiPlacement,
    rects: Option<&PanelRects>,
    target: Option<&ComputedUiRenderTargetInfo>,
    window: Option<&Window>,
) -> Option<ResolvedRuntimeUiPlacement> {
    let (scale, window_size) = logical_target_dimensions(target, window)?;
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
            // A manifest window is a preferred logical size, not permission to
            // escape the render target. Shrink it to the live target and clamp
            // its anchored origin so every authored control remains reachable
            // after a resize, on a small display, or under a larger DPI scale.
            let size = Vec2::new(width.max(1.0), height.max(1.0)).min(window_size);
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
            let max_origin = (window_size - size).max(Vec2::ZERO);
            let origin = (base + *offset).clamp(Vec2::ZERO, max_origin);
            Some(ResolvedRuntimeUiPlacement {
                rect: egui::Rect::from_min_size(egui_pos2(origin), egui_vec2(size)),
            })
        }
    }
}

fn apply_placement(node: &mut Node, rect: egui::Rect) {
    let width = rect.width();
    let height = rect.height();
    node.position_type = PositionType::Absolute;
    node.left = Val::Px(rect.min.x);
    node.top = Val::Px(rect.min.y);
    node.width = Val::Px(width);
    node.height = Val::Px(height);
    // HUI and Flair can replace the root node's ideal size while building a
    // template. The manifest rectangle is the outer-surface contract, so its
    // bounds must also override the root's intrinsic min-content size. Clip at
    // this boundary; child controls remain responsible for fitting their
    // authored profile inside the surface.
    node.min_width = Val::Px(width);
    node.max_width = Val::Px(width);
    node.min_height = Val::Px(height);
    node.max_height = Val::Px(height);
    node.overflow = Overflow::hidden();
}

fn placement_is_applied(node: &Node, rect: egui::Rect) -> bool {
    let width = Val::Px(rect.width());
    let height = Val::Px(rect.height());
    node.position_type == PositionType::Absolute
        && node.left == Val::Px(rect.min.x)
        && node.top == Val::Px(rect.min.y)
        && node.width == width
        && node.height == height
        && node.min_width == width
        && node.max_width == width
        && node.min_height == height
        && node.max_height == height
        && node.overflow == Overflow::hidden()
}

lunco_core::register_commands!(
    on_set_runtime_ui_form_values,
    on_set_runtime_ui_collection_data,
    on_set_runtime_ui_form_status,
    on_inspect_runtime_ui_form
);

fn register_runtime_ui_form_commands(app: &mut App) {
    register_all_commands(app);
}

#[cfg(test)]
mod tests {
    #[test]
    fn runtime_ui_schedule_builds_with_bevy_ui_and_flair() {
        use super::*;

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            bevy::window::WindowPlugin {
                primary_window: None,
                ..default()
            },
            bevy::ui::UiPlugin,
            RuntimeUiPlugin,
        ));
        assert!(
            app.world()
                .resource::<bevy_hui::prelude::ComponentBindings>()
                .contains_key("input")
        );
        app.finish();
        app.cleanup();
        app.world_mut()
            .schedule_scope(PostUpdate, |world, schedule| {
                schedule
                    .initialize(world)
                    .expect("runtime HTML and Flair PostUpdate schedule must build");
            });
    }

    #[test]
    fn authored_collection_publication_is_twin_scoped_typed_and_key_safe() {
        use super::*;

        let mut app = App::new();
        let mut workspace = WorkspaceResource::new();
        workspace.active_twin = Some(TwinId::new(41));
        app.insert_resource(workspace)
            .insert_resource(RuntimeUiForms::default())
            .insert_resource(EngineExposures::default())
            .init_resource::<lunco_core::CommandResults>()
            .init_resource::<lunco_core::ActiveCommandId>();
        register_all_commands(&mut app);

        let definition: RuntimeUiSurfaceDefinition = serde_json::from_str(
            r#"{
                "id":"factory-studio",
                "namespace":"factory-studio",
                "template":"ui/factory_studio.html",
                "stylesheet":"ui/factory_studio.css",
                "collections":[{
                    "source":"ingredient_rows",
                    "container":"ingredient-rows",
                    "template":"ui/factory_ingredient_row.html",
                    "key":"row_key"
                }],
                "placement":{"mode":"window","anchor":"top_left","offset":[0,0],"width":640,"height":900}
            }"#,
        )
        .unwrap();
        let mut manifest_state = RuntimeUiManifestState::default();
        manifest_state.scopes.insert(
            Some(TwinId::new(41)),
            Ok(Some(RuntimeUiManifest {
                _kind: RUNTIME_UI_MANIFEST_KIND.to_owned(),
                surfaces: vec![definition],
            })),
        );
        app.insert_resource(manifest_state);

        let row = |key: &str, field: &str| {
            BTreeMap::from([
                ("row_key".to_owned(), key.to_owned()),
                ("material_field".to_owned(), field.to_owned()),
            ])
        };
        app.world_mut().trigger(SetRuntimeUiCollectionData {
            namespace: "factory-studio".to_owned(),
            source: "ingredient_rows".to_owned(),
            items: vec![
                row("ingredient-1", "ingredient_1"),
                row("ingredient-2", "ingredient_2"),
            ],
        });
        let exposures = app.world().resource::<EngineExposures>();
        let Some(ExposureValue::Array(rows)) = exposures
            .surfaces
            .get("factory-studio")
            .and_then(|surface| surface.properties.get("ingredient_rows"))
        else {
            panic!("Rhai collection rows were not published as a typed exposure array");
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(
            collection_item_fields(&rows[1], "row_key").unwrap(),
            vec![
                ("material_field".to_owned(), "ingredient_2".to_owned()),
                ("row_key".to_owned(), "ingredient-2".to_owned()),
            ]
        );
        let published_revision = exposures.revision;

        app.world_mut().trigger(SetRuntimeUiCollectionData {
            namespace: "factory-studio".to_owned(),
            source: "ingredient_rows".to_owned(),
            items: vec![
                row("duplicate", "ingredient_1"),
                row("duplicate", "ingredient_2"),
            ],
        });
        app.world_mut().trigger(SetRuntimeUiCollectionData {
            namespace: "factory-studio".to_owned(),
            source: "undeclared_rows".to_owned(),
            items: Vec::new(),
        });
        assert_eq!(
            app.world().resource::<EngineExposures>().revision,
            published_revision,
            "invalid keys and undeclared sources must not change the live collection"
        );

        app.world_mut()
            .resource_mut::<WorkspaceResource>()
            .active_twin = Some(TwinId::new(42));
        app.world_mut().trigger(SetRuntimeUiFormValues {
            form_id: "another_form".to_owned(),
            values: BTreeMap::from([("name".to_owned(), "another twin".to_owned())]),
        });
        assert!(
            !app.world()
                .resource::<EngineExposures>()
                .surfaces
                .get("factory-studio")
                .is_some_and(|surface| surface.properties.contains_key("ingredient_rows")),
            "changing active Twin ownership must retire the previous Twin's rows"
        );
    }

    #[test]
    fn authored_collection_payload_limits_are_enforced_before_publication() {
        use super::*;

        let too_many_rows = (0..129)
            .map(|index| BTreeMap::from([("row_key".to_owned(), format!("row-{index}"))]))
            .collect::<Vec<_>>();
        assert!(
            runtime_ui_collection_value("row_key", &too_many_rows)
                .unwrap_err()
                .contains("at most 128")
        );

        let too_many_fields = BTreeMap::from_iter(
            std::iter::once(("row_key".to_owned(), "ingredient-1".to_owned()))
                .chain((0..32).map(|index| (format!("field-{index}"), "value".to_owned()))),
        );
        assert!(
            runtime_ui_collection_value("row_key", &[too_many_fields])
                .unwrap_err()
                .contains("1 to 32 fields")
        );

        let oversized = BTreeMap::from([
            ("row_key".to_owned(), "ingredient-1".to_owned()),
            ("value".to_owned(), "x".repeat(2049)),
        ]);
        assert!(
            runtime_ui_collection_value("row_key", &[oversized])
                .unwrap_err()
                .contains("exceeds 2048 characters")
        );
    }

    #[test]
    fn keyed_collection_reconciliation_reuses_rows_updates_fields_and_retires_removed_keys() {
        use super::*;

        let mut exposures = EngineExposures::default();
        let first_rows = runtime_ui_collection_value(
            "row_key",
            &[
                BTreeMap::from([
                    ("row_key".to_owned(), "ingredient-1".to_owned()),
                    ("material_value".to_owned(), "Sugar".to_owned()),
                ]),
                BTreeMap::from([
                    ("row_key".to_owned(), "ingredient-2".to_owned()),
                    ("material_value".to_owned(), "Vanilla".to_owned()),
                ]),
            ],
        )
        .unwrap();
        exposures
            .writer("factory-studio")
            .property("ingredient_rows", first_rows);

        let mut app = App::new();
        app.insert_resource(exposures);
        let definition: RuntimeUiSurfaceDefinition = serde_json::from_str(
            r#"{
                "id":"factory-studio",
                "namespace":"factory-studio",
                "template":"ui/factory_studio.html",
                "stylesheet":"ui/factory_studio.css",
                "collections":[{
                    "source":"ingredient_rows",
                    "container":"ingredient-rows",
                    "template":"ui/factory_ingredient_row.html",
                    "key":"row_key"
                }],
                "placement":{"mode":"window","anchor":"top_left","offset":[0,0],"width":640,"height":900}
            }"#,
        )
        .unwrap();
        let mut surface = RuntimeUiSurface::from_definition(
            &definition,
            Handle::default(),
            Handle::default(),
            None,
        )
        .unwrap();
        surface.mounted = true;
        let root = app
            .world_mut()
            .spawn((Node::default(), surface, Visibility::Visible))
            .id();
        let mut container_id = UiId::default();
        *container_id = "ingredient-rows".to_owned();
        let container = app
            .world_mut()
            .spawn((Node::default(), container_id, ChildOf(root)))
            .id();
        app.add_systems(Update, reconcile_runtime_ui_collections);
        app.update();

        let mut query = app
            .world_mut()
            .query::<(Entity, &RuntimeUiCollectionRow, &TemplateProperties)>();
        let initial = query
            .iter(app.world())
            .map(|(entity, row, properties)| {
                (
                    row.key.clone(),
                    (entity, properties.get("material_value").cloned()),
                )
            })
            .collect::<HashMap<_, _>>();
        assert_eq!(initial.len(), 2);
        let retained_second = initial["ingredient-2"].0;

        let next_rows = runtime_ui_collection_value(
            "row_key",
            &[
                BTreeMap::from([
                    ("row_key".to_owned(), "ingredient-2".to_owned()),
                    ("material_value".to_owned(), "Cocoa".to_owned()),
                ]),
                BTreeMap::from([
                    ("row_key".to_owned(), "ingredient-3".to_owned()),
                    ("material_value".to_owned(), "Milk".to_owned()),
                ]),
            ],
        )
        .unwrap();
        app.world_mut()
            .resource_mut::<EngineExposures>()
            .writer("factory-studio")
            .property("ingredient_rows", next_rows);
        app.update();

        let updated = query
            .iter(app.world())
            .map(|(entity, row, properties)| {
                (
                    row.key.clone(),
                    (entity, properties.get("material_value").cloned()),
                )
            })
            .collect::<HashMap<_, _>>();
        assert_eq!(updated.len(), 2);
        assert!(!updated.contains_key("ingredient-1"));
        assert_eq!(updated["ingredient-2"].0, retained_second);
        assert_eq!(updated["ingredient-2"].1.as_deref(), Some("Cocoa"));
        assert!(updated.contains_key("ingredient-3"));
        assert_eq!(
            app.world().get::<Children>(container).map(Children::len),
            Some(2),
            "the retained host should contain exactly the current ordered rows"
        );
    }

    #[test]
    fn authored_html_class_tags_update_flair_class_lists() {
        use super::*;

        let mut app = App::new();
        app.add_systems(Update, sync_runtime_ui_html_classes);
        let mut tags = Tags::default();
        tags.insert("class".to_owned(), "section-title muted".to_owned());
        let entity = app.world_mut().spawn(tags).id();

        app.update();
        assert_eq!(
            app.world().get::<ClassList>(entity),
            Some(&ClassList::new("section-title muted"))
        );

        app.world_mut()
            .get_mut::<Tags>(entity)
            .unwrap()
            .insert("class".to_owned(), "field-label".to_owned());
        app.update();
        assert_eq!(
            app.world().get::<ClassList>(entity),
            Some(&ClassList::new("field-label"))
        );

        app.world_mut()
            .get_mut::<Tags>(entity)
            .unwrap()
            .remove("class");
        app.update();
        assert!(app.world().get::<ClassList>(entity).is_none());
    }

    #[test]
    fn authored_action_includes_current_form_values_and_button_tags() {
        use super::*;
        let mut tags = Tags::default();
        tags.insert("action".to_owned(), "factory.recipe.choose".to_owned());
        tags.insert("ui_form".to_owned(), "factory_recipe".to_owned());
        tags.insert("field".to_owned(), "mixer".to_owned());
        tags.insert("value".to_owned(), "Paddle".to_owned());
        tags.insert("index".to_owned(), "4".to_owned());
        let values = BTreeMap::from([
            ("product_name".to_owned(), "Cocoa drink".to_owned()),
            ("ingredient_count".to_owned(), "3".to_owned()),
        ]);

        let parameters = runtime_ui_action_parameters(&tags, Some(&values));
        assert_eq!(
            parameters.get("ui_form").and_then(HookValue::as_str),
            Some("factory_recipe")
        );
        assert_eq!(
            parameters.get("field").and_then(HookValue::as_str),
            Some("mixer")
        );
        assert_eq!(
            parameters.get("value").and_then(HookValue::as_str),
            Some("Paddle")
        );
        assert_eq!(
            parameters.get("index").and_then(HookValue::as_str),
            Some("4")
        );
        assert!(parameters.get("action").is_none());
        let fields = parameters.get("fields").unwrap();
        assert_eq!(
            fields.get("product_name").and_then(HookValue::as_str),
            Some("Cocoa drink")
        );
        assert_eq!(
            fields.get("ingredient_count").and_then(HookValue::as_str),
            Some("3")
        );
    }

    #[test]
    fn authored_value_and_status_labels_follow_the_active_twins_form() {
        use super::*;
        let mut app = App::new();
        let mut workspace = WorkspaceResource::new();
        workspace.active_twin = Some(TwinId::new(23));
        app.insert_resource(workspace);
        let mut forms = RuntimeUiForms {
            owner_twin_id: Some(23),
            ..default()
        };
        forms.forms.insert(
            "factory_recipe".to_owned(),
            RuntimeUiFormState {
                values: BTreeMap::from([("mixer".to_owned(), "Paddle".to_owned())]),
                status: "Ready to run".to_owned(),
                needs_projection: false,
            },
        );
        app.insert_resource(forms);

        let mut value_tags = Tags::default();
        value_tags.insert("ui_form".to_owned(), "factory_recipe".to_owned());
        value_tags.insert("ui_value".to_owned(), "mixer".to_owned());
        let value_label = app
            .world_mut()
            .spawn((value_tags, Text::new("old mixer")))
            .id();
        let mut status_tags = Tags::default();
        status_tags.insert("ui_form".to_owned(), "factory_recipe".to_owned());
        status_tags.insert("ui_status".to_owned(), "true".to_owned());
        let status_label = app
            .world_mut()
            .spawn((status_tags, Text::new("old status")))
            .id();

        app.add_systems(Update, update_runtime_ui_form_text);
        app.update();

        assert_eq!(app.world().get::<Text>(value_label).unwrap().0, "Paddle");
        assert_eq!(
            app.world().get::<Text>(status_label).unwrap().0,
            "Ready to run"
        );
    }

    #[test]
    fn authored_text_inputs_round_trip_form_values_only_for_the_active_twin() {
        use super::*;
        let mut app = App::new();
        let mut workspace = WorkspaceResource::new();
        workspace.active_twin = Some(TwinId::new(23));
        app.insert_resource(workspace);
        let mut forms = RuntimeUiForms {
            owner_twin_id: Some(23),
            ..default()
        };
        forms.forms.insert(
            "factory_recipe".to_owned(),
            RuntimeUiFormState {
                values: BTreeMap::from([("fraction_1".to_owned(), "0.5".to_owned())]),
                ..default()
            },
        );
        app.insert_resource(forms);
        let input = RuntimeUiTextInput {
            owner_twin_id: 23,
            form_id: "factory_recipe".to_owned(),
            field_id: "fraction_1".to_owned(),
        };
        let entity = app
            .world_mut()
            .spawn((input, EditableText::new("0.6")))
            .id();
        app.add_systems(PostUpdate, store_runtime_ui_form_input_changes);

        app.update();
        assert_eq!(
            app.world().resource::<RuntimeUiForms>().forms["factory_recipe"].values["fraction_1"],
            "0.6"
        );

        app.world_mut()
            .resource_mut::<WorkspaceResource>()
            .active_twin = Some(TwinId::new(24));
        app.world_mut()
            .get_mut::<EditableText>(entity)
            .unwrap()
            .editor_mut()
            .set_text("0.9");
        app.update();
        assert_eq!(
            app.world().resource::<RuntimeUiForms>().forms["factory_recipe"].values["fraction_1"],
            "0.6",
            "a retained input from the outgoing Twin must not update the next Twin's form"
        );
    }

    #[test]
    fn authored_input_fields_receive_text_input_and_pointer_focus_components() {
        use super::*;

        let mut app = App::new();
        let mut workspace = WorkspaceResource::new();
        workspace.active_twin = Some(TwinId::new(23));
        app.insert_resource(workspace);
        let mut forms = RuntimeUiForms {
            owner_twin_id: Some(23),
            ..default()
        };
        forms.forms.insert(
            "factory_recipe".to_owned(),
            RuntimeUiFormState {
                values: BTreeMap::from([("ingredient_count".to_owned(), "2".to_owned())]),
                ..default()
            },
        );
        app.insert_resource(forms);

        let definition: RuntimeUiSurfaceDefinition = serde_json::from_str(
            r#"{
                "id":"factory-studio",
                "namespace":"factory-studio",
                "template":"ui/factory_studio.html",
                "stylesheet":"ui/factory_studio.css",
                "interactive":true,
                "placement":{"mode":"window","anchor":"top_left","offset":[18,64],"width":560,"height":880}
            }"#,
        )
        .unwrap();
        let mut surface = RuntimeUiSurface::from_definition(
            &definition,
            Handle::default(),
            Handle::default(),
            None,
        )
        .unwrap();
        surface.mounted = true;
        let root = app.world_mut().spawn(surface).id();
        let mut tags = Tags::default();
        tags.insert("ui_form".to_owned(), "factory_recipe".to_owned());
        tags.insert("ui_field".to_owned(), "ingredient_count".to_owned());
        tags.insert("ui_tab_index".to_owned(), "3".to_owned());
        let field = app
            .world_mut()
            .spawn((
                Text::default(),
                tags,
                TemplateProperties::default()
                    .with("type", "number")
                    .with("value", "2"),
                ChildOf(root),
            ))
            .id();
        app.add_systems(Update, attach_runtime_ui_text_inputs);

        app.update();

        assert!(
            app.world()
                .get::<bevy_ui_widgets::TextInput>(field)
                .is_some()
        );
        assert!(app.world().get::<Pickable>(field).is_some());
        assert_eq!(
            app.world()
                .get::<bevy::text::TextCursorStyle>(field)
                .unwrap()
                .color,
            Color::srgb(0.44, 0.85, 0.77),
            "native inputs need a high-contrast visible caret"
        );
        assert_eq!(app.world().get::<EditableText>(field).unwrap().value(), "2");
        assert_eq!(
            app.world()
                .get::<bevy::input_focus::tab_navigation::TabIndex>(field),
            Some(&bevy::input_focus::tab_navigation::TabIndex(3))
        );
    }

    #[test]
    fn form_value_commands_project_back_into_retained_text_inputs() {
        use super::*;
        let mut app = App::new();
        let mut workspace = WorkspaceResource::new();
        workspace.active_twin = Some(TwinId::new(23));
        app.insert_resource(workspace);
        let mut forms = RuntimeUiForms {
            owner_twin_id: Some(23),
            ..default()
        };
        forms.forms.insert(
            "factory_recipe".to_owned(),
            RuntimeUiFormState {
                values: BTreeMap::from([("batch_kg".to_owned(), "750.0".to_owned())]),
                needs_projection: true,
                ..default()
            },
        );
        app.insert_resource(forms);
        let entity = app
            .world_mut()
            .spawn((
                RuntimeUiTextInput {
                    owner_twin_id: 23,
                    form_id: "factory_recipe".to_owned(),
                    field_id: "batch_kg".to_owned(),
                },
                EditableText::new("1000.0"),
            ))
            .id();
        app.add_systems(Update, apply_runtime_ui_form_values);
        app.update();

        assert_eq!(
            app.world().get::<EditableText>(entity).unwrap().value(),
            "750.0"
        );
        assert!(!app.world().resource::<RuntimeUiForms>().forms["factory_recipe"].needs_projection);
    }

    #[test]
    fn a_pending_descendant_image_invalidates_a_previously_ready_surface() {
        use super::*;
        let mut app = App::new();
        app.init_resource::<lunco_status_core::status_bus::StatusBus>();
        app.init_resource::<RuntimeUiPresentationGeneration>();
        let mut exposures = EngineExposures::default();
        exposures.writer("camera-view").visible(true);
        app.insert_resource(exposures);
        app.insert_resource(RuntimeUiRecordingContract {
            required_namespaces: HashSet::from(["camera-view".to_owned()]),
            ..Default::default()
        });
        let definition: RuntimeUiSurfaceDefinition = serde_json::from_str(
            r#"{
            "id":"camera-view", "namespace":"camera-view", "template":"ui/a.html",
            "stylesheet":"ui/a.css", "placement":{"mode":"viewport"}
        }"#,
        )
        .unwrap();
        let mut surface = RuntimeUiSurface::from_definition(
            &definition,
            Handle::default(),
            Handle::default(),
            None,
        )
        .unwrap();
        surface.presentation_ready = true;
        let root = app.world_mut().spawn((surface, Visibility::Visible)).id();
        let container = app.world_mut().spawn(ChildOf(root)).id();
        app.world_mut().spawn((
            ChildOf(container),
            RuntimeUiImagePending("camera view 1 has no source".into()),
        ));
        app.add_systems(Update, report_runtime_ui_readiness);
        app.update();
        assert!(
            !app.world()
                .get::<RuntimeUiSurface>(root)
                .unwrap()
                .presentation_ready,
            "a ready latch must not admit a recording after a descendant image becomes unavailable"
        );
    }

    #[test]
    fn perspective_filter_accepts_one_or_many_and_rejects_ambiguous_lists() {
        use super::*;
        let mut layout = WorkbenchSnapshot::default();
        layout.replace(
            Some(lunco_workbench_core::PerspectiveId::new(
                "twin/report-analysis",
            )),
            None,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        );
        for json in [
            r#""twin/report-analysis""#,
            r#"["sandbox_view","twin/report-analysis"]"#,
        ] {
            let filter: RuntimeUiPerspectiveFilter = serde_json::from_str(json).unwrap();
            assert!(filter.validates().is_ok());
            assert!(filter.allows(Some(&layout)));
            assert!(!filter.allows(None));
        }
        let other: RuntimeUiPerspectiveFilter = serde_json::from_str(r#""sandbox_view""#).unwrap();
        assert!(!other.allows(Some(&layout)));
        for json in [
            r#"[]"#,
            r#"["", "sandbox_view"]"#,
            r#"["sandbox_view","sandbox_view"]"#,
        ] {
            let filter: RuntimeUiPerspectiveFilter = serde_json::from_str(json).unwrap();
            assert!(filter.validates().is_err());
        }
    }

    #[test]
    fn hui_rebuild_preserves_retained_runtime_styles_only() {
        use super::*;
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), bevy_hui::HuiPlugin));
        app.init_asset::<TextureAtlasLayout>();
        install_runtime_ui_style_handoff(&mut app);
        let definition: RuntimeUiSurfaceDefinition = serde_json::from_str(
            r#"{
            "id":"retained", "namespace":"retained", "template":"ui/a.html",
            "stylesheet":"ui/a.css", "placement":{"mode":"viewport"}
        }"#,
        )
        .unwrap();
        let root = app
            .world_mut()
            .spawn(
                RuntimeUiSurface::from_definition(
                    &definition,
                    Handle::default(),
                    Handle::default(),
                    None,
                )
                .unwrap(),
            )
            .id();
        let styled = Node {
            width: Val::Px(321.0),
            ..default()
        };
        let runtime = app
            .world_mut()
            .spawn((styled.clone(), HtmlStyle::default(), ChildOf(root)))
            .id();
        let unrelated = app.world_mut().spawn((styled, HtmlStyle::default())).id();
        app.update();
        assert_eq!(
            app.world().get::<Node>(runtime).unwrap().width,
            Val::Px(321.0)
        );
        assert!(app.world().get::<RuntimeUiTemplateStyle>(runtime).is_some());
        assert!(app.world().get::<HtmlStyle>(runtime).is_none());
        assert_eq!(app.world().get::<Node>(unrelated).unwrap().width, Val::Auto);
        assert!(app.world().get::<HtmlStyle>(unrelated).is_some());
    }

    #[test]
    fn deferred_action_scope_preserves_an_explicitly_empty_workspace() {
        let scope = super::RuntimeUiActionScope::capture(None, None);
        assert!(scope.validate(None, None).is_ok());
        let outgoing = super::RuntimeUiActionScope {
            owner_twin_id: Some(17),
            lifetime: lunco_core::InteractionLifetime::Owner,
        };
        assert!(outgoing.validate(None, None).is_err());
        let mut incoming = lunco_workspace::WorkspaceResource::new();
        incoming.active_twin = Some(lunco_workspace::TwinId::new(23));
        assert!(scope.validate(Some(&incoming), None).is_err());
        assert!(outgoing.validate(Some(&incoming), None).is_err());
    }

    use super::*;
    use bevy::camera::NormalizedRenderTarget;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::picking::pointer::{Location, PointerId};
    use lunco_workbench_core::scene_pick::EguiPointerState;
    use std::sync::Once;

    struct TestRuntimeUiRecordingPolicy;

    impl lunco_hooks::ScriptHook for TestRuntimeUiRecordingPolicy {
        fn invoke(&self, invocation: &lunco_hooks::HookInvocation<'_>) -> lunco_hooks::HookResult {
            let context = invocation.context;
            if context.route
                != Some(lunco_core::RuntimeRoute::application(
                    lunco_core::RuntimeCycle::Ui,
                ))
                || context.phase != lunco_core::RuntimePhase::Preparation
                || context.clock != lunco_core::RuntimeClock::Application
                || context.time_seconds.is_none()
                || context.delta_seconds.is_none()
                || context.sequence.is_some()
                || context.producer.is_some()
                || context.validate().is_err()
            {
                return Err(lunco_hooks::HookError(
                    "runtime UI recording policy requires Application/Ui/Preparation context"
                        .into(),
                ));
            }
            let facts = invocation
                .args
                .first()
                .ok_or_else(|| lunco_hooks::HookError("missing recording facts".into()))?;
            let recording_active = facts.get("recording_active").and_then(HookValue::as_bool);
            let visible_surface_ids = facts.get("visible_surface_ids");
            if recording_active != Some(true) {
                return Err(lunco_hooks::HookError(
                    "runtime UI recording facts require an active recording".into(),
                ));
            }
            match visible_surface_ids {
                Some(HookValue::Array(ids)) => Ok(HookValue::Array(ids.clone())),
                _ => Err(lunco_hooks::HookError(
                    "missing visible surface candidates".into(),
                )),
            }
        }
    }

    fn install_runtime_ui_recording_test_policy() {
        static INSTALLED: Once = Once::new();
        INSTALLED.call_once(|| {
            lunco_hooks::register(lunco_hooks::RegisteredHook {
                id: "runtime.ui.recording".into(),
                backend: "test".into(),
                deterministic: false,
                hook: std::sync::Arc::new(TestRuntimeUiRecordingPolicy),
            });
        });
    }

    #[test]
    fn runtime_ui_recording_policy_receives_owner_ui_context() {
        install_runtime_ui_recording_test_policy();

        let manifest: RuntimeUiManifest = serde_json::from_str(
            r#"{
                "kind": "lunco.runtime-ui-manifest.v1",
                "surfaces": [{
                    "id": "capture",
                    "template": "ui/capture.html",
                    "stylesheet": "ui/capture.css",
                    "namespace": "capture-surface",
                    "placement": {"mode": "viewport"}
                }]
            }"#,
        )
        .expect("inline runtime UI manifest should parse");

        let mut exposures = EngineExposures::default();
        exposures.writer("capture-surface").visible(true);

        let mut app = App::new();
        app.insert_resource(Time::<Real>::default())
            .insert_resource(exposures)
            .insert_resource(RuntimeUiManifestState {
                manifest: Some(manifest),
                ..Default::default()
            })
            .insert_resource(RuntimeUiCaptureState { active: true })
            .init_resource::<RuntimeUiRecordingContract>();

        app.world_mut()
            .run_system_once(update_runtime_ui_recording_contract)
            .expect("recording contract system should run");

        let contract = app.world().resource::<RuntimeUiRecordingContract>();
        assert_eq!(
            contract.required_namespaces,
            ["capture-surface".to_owned()].into()
        );
        assert!(contract.error.is_none());
    }

    #[test]
    fn recording_candidates_follow_perspective_without_an_exposure_change() {
        install_runtime_ui_recording_test_policy();
        let manifest: RuntimeUiManifest = serde_json::from_str(
            r#"{"surfaces":[{
                "id":"capture", "namespace":"capture-surface",
                "template":"ui/a.html", "stylesheet":"ui/a.css",
                "placement":{"mode":"viewport"}, "visible_in_perspective":"drive"
            }]}"#,
        )
        .unwrap();
        let mut exposures = EngineExposures::default();
        exposures.writer("capture-surface").visible(true);
        let revision = exposures.revision;
        let mut layout = WorkbenchSnapshot::default();
        layout.replace(
            Some(lunco_workbench_core::PerspectiveId::new("drive")),
            None,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let mut app = App::new();
        app.insert_resource(Time::<Real>::default())
            .insert_resource(exposures)
            .insert_resource(layout)
            .insert_resource(RuntimeUiManifestState {
                manifest: Some(manifest),
                ..Default::default()
            })
            .insert_resource(RuntimeUiCaptureState { active: true })
            .init_resource::<RuntimeUiRecordingContract>()
            .add_systems(Update, update_runtime_ui_recording_contract);
        for perspective in ["drive", "analysis", "drive"] {
            app.world_mut().resource_mut::<WorkbenchSnapshot>().replace(
                Some(lunco_workbench_core::PerspectiveId::new(perspective)),
                None,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
            );
            app.update();
            let contract = app.world().resource::<RuntimeUiRecordingContract>();
            assert!(contract.error.is_none(), "{:?}", contract.error);
            assert_eq!(
                contract.required_namespaces.contains("capture-surface"),
                perspective == "drive"
            );
            assert_eq!(app.world().resource::<EngineExposures>().revision, revision);
        }
    }

    #[test]
    fn recording_candidates_obey_authored_gates_and_settings_before_mounting() {
        let mut surface: RuntimeUiSurfaceDefinition = serde_json::from_str(
            r#"{"id":"capture", "namespace":"capture-surface",
                "template":"ui/a.html", "stylesheet":"ui/a.css",
                "placement":{"mode":"viewport"}, "gate":"ready",
                "setting":"ui.telemetry", "setting_default":true}"#,
        )
        .unwrap();
        let mut exposures = EngineExposures::default();
        exposures.writer("capture-surface").visible(true);
        let mut gates = RuntimeUiGates::default();
        assert!(!capture_surface_is_visible(
            &surface, &exposures, None, None, None
        ));
        assert!(!capture_surface_is_visible(
            &surface,
            &exposures,
            None,
            Some(&gates),
            None
        ));
        gates.set("ready", true);
        // An eligible surface remains required before its retained root mounts.
        assert!(capture_surface_is_visible(
            &surface,
            &exposures,
            None,
            Some(&gates),
            None
        ));
        surface.setting_default = false;
        assert!(!capture_surface_is_visible(
            &surface,
            &exposures,
            None,
            Some(&gates),
            None
        ));
        surface.setting_default = true;
        exposures.writer("capture-surface").visible(false);
        assert!(!capture_surface_is_visible(
            &surface,
            &exposures,
            None,
            Some(&gates),
            None
        ));
    }

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
                        "setting": "ui.camera_status",
                        "interactive": true,
                        "draggable": true,
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
        assert!(manifest.surfaces[2].draggable);
        assert_eq!(
            manifest.surfaces[2].setting.as_deref(),
            Some("ui.camera_status")
        );
    }

    #[test]
    fn draggable_surfaces_require_window_placement() {
        let manifest: RuntimeUiManifest = serde_json::from_str(
            r#"{
                "surfaces": [{
                    "id": "full-screen",
                    "template": "ui/a.html",
                    "stylesheet": "ui/a.css",
                    "namespace": "full-screen",
                    "draggable": true,
                    "placement": {"mode": "viewport"}
                }]
            }"#,
        )
        .expect("JSON shape should parse");

        let error = manifest
            .validate()
            .expect_err("only window surfaces can be draggable");
        assert!(error.contains("must use window placement"));
    }

    #[test]
    fn modal_surfaces_require_interactive_viewport_placement() {
        let manifest: RuntimeUiManifest = serde_json::from_str(
            r#"{
                "surfaces": [{
                    "id": "modal",
                    "template": "ui/a.html",
                    "stylesheet": "ui/a.css",
                    "namespace": "modal",
                    "modal": true,
                    "dismiss_action": "dismiss",
                    "placement": {"mode": "viewport"}
                }]
            }"#,
        )
        .expect("modal JSON should parse");
        assert!(manifest.validate().is_err(), "modal must be interactive");

        let manifest: RuntimeUiManifest = serde_json::from_str(
            r#"{
                "surfaces": [{
                    "id": "modal",
                    "template": "ui/a.html",
                    "stylesheet": "ui/a.css",
                    "namespace": "modal",
                    "interactive": true,
                    "modal": true,
                    "dismiss_action": "dismiss",
                    "placement": {
                        "mode": "window",
                        "anchor": "top_left",
                        "width": 200.0,
                        "height": 100.0
                    }
                }]
            }"#,
        )
        .expect("modal JSON should parse");
        assert!(
            manifest.validate().is_err(),
            "modal must use viewport placement"
        );
    }

    #[test]
    fn draggable_window_layout_is_clamped_to_live_target() {
        let manifest: RuntimeUiManifest = serde_json::from_str(
            r#"{
                "surfaces": [{
                    "id": "window",
                    "template": "ui/a.html",
                    "stylesheet": "ui/a.css",
                    "namespace": "window",
                    "draggable": true,
                    "placement": {
                        "mode": "window",
                        "anchor": "top_left",
                        "width": 240.0,
                        "height": 120.0
                    }
                }]
            }"#,
        )
        .expect("JSON shape should parse");
        manifest.validate().expect("window should validate");

        let surface = RuntimeUiSurface::from_definition(
            &manifest.surfaces[0],
            Handle::default(),
            Handle::default(),
            None,
        )
        .unwrap();
        let mut layouts = RuntimeSurfaceLayouts::default();
        layouts.set(
            "window",
            RuntimeSurfaceLayout {
                left: 10_000.0,
                top: 10_000.0,
            },
        );
        let window = Window::default();
        let placement = resolve_surface_placement(&surface, &layouts, None, None, Some(&window))
            .expect("window placement should resolve");

        assert_eq!(placement.rect.size(), egui::vec2(240.0, 120.0));
        assert!(placement.rect.min.x >= 0.0);
        assert!(placement.rect.min.y >= 0.0);
        assert!(placement.rect.max.x <= window.width());
        assert!(placement.rect.max.y <= window.height());
    }

    #[test]
    fn drag_delta_is_applied_once_in_logical_pixels() {
        let current = egui::Rect::from_min_size(egui::pos2(16.0, 819.0), egui::vec2(224.0, 184.0));
        let dragged = resolve_dragged_surface_rect(
            current,
            Vec2::new(500.0, -330.0),
            Vec2::new(1707.0, 1035.0),
        );

        assert_eq!(dragged.min, egui::pos2(516.0, 489.0));
        assert_eq!(dragged.size(), egui::vec2(224.0, 184.0));
    }

    #[test]
    fn only_explicit_drag_handle_moves_its_draggable_surface_root() {
        let mut app = App::new();
        app.add_message::<RuntimeUiSurfaceDragged>()
            .add_message::<RuntimeUiSurfaceReset>()
            .init_resource::<RuntimeSurfaceLayouts>()
            .add_systems(Update, apply_runtime_ui_surface_interactions);
        app.world_mut().spawn((Window::default(), PrimaryWindow));

        let root = app
            .world_mut()
            .spawn((
                Node::default(),
                RuntimeUiSurface {
                    layout_id: "view-switcher".to_owned(),
                    namespace: "view-switcher".to_owned(),
                    template: Handle::default(),
                    stylesheet: Handle::default(),
                    bindings: HashMap::new(),
                    collections: Vec::new(),
                    visible_in_perspective: None,
                    gate: None,
                    setting: None,
                    setting_default: false,
                    interactive: true,

                    modal: false,
                    dismiss_action: None,
                    draggable: true,
                    mounted: true,
                    presentation_ready: false,
                    presentation_visible: false,
                    presentation_generation: 0,
                    placement: RuntimeUiPlacement::Window {
                        anchor: RuntimeUiWindowAnchor::TopCenter,
                        offset: Vec2::ZERO,
                        width: 200.0,
                        height: 100.0,
                    },
                    has_projected_exposure: false,
                    applied_revision: 0,
                    applied_placement: Some(ResolvedRuntimeUiPlacement {
                        rect: egui::Rect::from_min_size(
                            egui::pos2(10.0, 20.0),
                            egui::vec2(200.0, 100.0),
                        ),
                    }),
                },
            ))
            .id();
        app.world_mut()
            .entity_mut(root)
            .observe(emit_runtime_ui_surface_drag);
        let button = app.world_mut().spawn(ChildOf(root)).id();
        let mut handle_tags = Tags::default();
        handle_tags.insert("ui_drag_handle".to_owned(), "true".to_owned());
        let handle = app.world_mut().spawn((handle_tags, ChildOf(root))).id();

        let drag = |entity| PointerDrag {
            entity,
            pointer: Pointer::new(
                PointerId::Mouse,
                Location {
                    target: NormalizedRenderTarget::None {
                        width: 0,
                        height: 0,
                    },
                    position: Vec2::ZERO,
                },
            ),
            button: PointerButton::Primary,
            distance: Vec2::new(12.0, 18.0),
            delta: Vec2::new(12.0, 18.0),
        };
        app.world_mut().trigger(drag(button));
        app.update();
        assert_eq!(
            app.world()
                .resource::<RuntimeSurfaceLayouts>()
                .get("view-switcher"),
            None,
            "a drag over a button or input must not move the whole surface"
        );

        app.world_mut().trigger(drag(handle));
        app.update();

        assert_eq!(
            app.world()
                .resource::<RuntimeSurfaceLayouts>()
                .get("view-switcher"),
            Some(RuntimeSurfaceLayout {
                left: 22.0,
                top: 38.0,
            })
        );
    }

    #[test]
    fn runtime_ui_input_rect_uses_computed_control_geometry() {
        let node = ComputedNode {
            size: Vec2::new(80.0, 20.0),
            inverse_scale_factor: 0.5,
            ..Default::default()
        };
        let transform = UiGlobalTransform::from_xy(100.0, 60.0);

        assert_eq!(
            runtime_ui_input_rect(&node, &transform),
            Some(egui::Rect::from_min_max(
                egui::pos2(30.0, 25.0),
                egui::pos2(70.0, 35.0)
            ))
        );
    }

    #[test]
    fn viewport_surface_does_not_blanket_scene_input() {
        let mut app = App::new();
        app.init_resource::<ScenePickGate>();
        let root = app
            .world_mut()
            .spawn((
                Node::default(),
                RuntimeUiSurface {
                    layout_id: "hud".to_owned(),
                    namespace: "hud".to_owned(),
                    template: Handle::default(),
                    stylesheet: Handle::default(),
                    bindings: HashMap::new(),
                    collections: Vec::new(),
                    visible_in_perspective: None,
                    gate: None,
                    setting: None,
                    setting_default: false,
                    interactive: true,

                    modal: false,
                    dismiss_action: None,
                    draggable: false,
                    mounted: true,
                    presentation_ready: false,
                    presentation_visible: false,
                    presentation_generation: 0,
                    placement: RuntimeUiPlacement::Viewport,
                    has_projected_exposure: false,
                    applied_revision: 0,
                    applied_placement: None,
                },
                Visibility::Visible,
            ))
            .id();
        let control = app
            .world_mut()
            .spawn((
                Node::default(),
                ComputedNode {
                    size: Vec2::new(80.0, 20.0),
                    inverse_scale_factor: 1.0,
                    ..Default::default()
                },
                UiGlobalTransform::from_xy(100.0, 60.0),
                InheritedVisibility::VISIBLE,
                OnUiPress(vec!["runtime_action".to_owned()]),
                ChildOf(root),
            ))
            .id();
        let decoration = app
            .world_mut()
            .spawn((
                Node::default(),
                ComputedNode {
                    size: Vec2::new(80.0, 20.0),
                    inverse_scale_factor: 1.0,
                    ..Default::default()
                },
                UiGlobalTransform::from_xy(200.0, 60.0),
                InheritedVisibility::VISIBLE,
                ChildOf(root),
            ))
            .id();
        app.add_systems(Update, register_runtime_ui_input_regions);
        app.update();

        assert_eq!(
            app.world().get::<Pickable>(control),
            Some(&Pickable::default())
        );
        assert!(app.world().get::<Pickable>(decoration).is_none());

        let mut gate = app.world_mut().remove_resource::<ScenePickGate>().unwrap();
        gate.mark_rendered();
        gate.resolve(EguiPointerState {
            hover_pos: Some(egui::pos2(0.0, 0.0)),
            ..Default::default()
        });
        assert!(gate.over_main_scene());

        gate.mark_rendered();
        gate.resolve(EguiPointerState {
            hover_pos: Some(egui::pos2(100.0, 60.0)),
            ..Default::default()
        });
        assert!(!gate.over_main_scene());
    }

    #[test]
    fn non_recording_surface_does_not_wait_for_render_acknowledgement() {
        let mut app = App::new();
        let mut exposures = EngineExposures::default();
        {
            let mut control = exposures.writer("control-hud");
            control.visible(true);
            control.property("value", "initial");
        }
        let initial_revision = exposures.revision;
        app.insert_resource(exposures)
            .init_resource::<RuntimeSurfaceLayouts>()
            .insert_resource(RuntimeUiManifestState {
                applied: Some(1),
                rebuild_pending: false,
                ..Default::default()
            });
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        let root = app
            .world_mut()
            .spawn((
                Node::default(),
                HtmlNode(Handle::default()),
                Styled::Inherited,
                InlineStyle::default(),
                TemplateProperties::default().with("value", "initial"),
                RuntimeUiSurface {
                    layout_id: "control-hud".to_owned(),
                    namespace: "control-hud".to_owned(),
                    template: Handle::default(),
                    stylesheet: Handle::default(),
                    bindings: HashMap::new(),
                    collections: Vec::new(),
                    visible_in_perspective: None,
                    gate: None,
                    setting: None,
                    setting_default: false,
                    interactive: false,

                    modal: false,
                    dismiss_action: None,
                    draggable: false,
                    mounted: true,
                    // This surface is not part of the offline recording
                    // contract, so it must be visible before any render-world
                    // acknowledgement exists.
                    presentation_ready: false,
                    presentation_visible: false,
                    presentation_generation: 0,
                    placement: RuntimeUiPlacement::Viewport,
                    has_projected_exposure: true,
                    applied_revision: initial_revision,
                    applied_placement: None,
                },
                Visibility::Visible,
            ))
            .id();

        app.add_systems(Update, apply_runtime_ui_exposures);
        app.update();
        assert!(matches!(
            app.world().get::<Visibility>(root),
            Some(Visibility::Visible)
        ));

        app.world_mut()
            .resource_mut::<EngineExposures>()
            .writer("control-hud")
            .property("value", "updated");
        app.update();
        assert!(matches!(
            app.world().get::<Visibility>(root),
            Some(Visibility::Visible)
        ));
    }

    #[test]
    fn presentation_generation_tracks_visibility_lifecycle_not_value_updates() {
        let mut surface = RuntimeUiSurface {
            layout_id: "recorded".to_owned(),
            namespace: "recorded".to_owned(),
            template: Handle::default(),
            stylesheet: Handle::default(),
            bindings: HashMap::new(),
            collections: Vec::new(),
            visible_in_perspective: None,
            gate: None,
            setting: None,
            setting_default: false,
            interactive: false,

            modal: false,
            dismiss_action: None,
            draggable: false,
            mounted: true,
            presentation_ready: true,
            presentation_visible: false,
            presentation_generation: 0,
            placement: RuntimeUiPlacement::Viewport,
            has_projected_exposure: true,
            applied_revision: 17,
            applied_placement: None,
        };
        let mut generation = RuntimeUiPresentationGeneration::default();

        generation.start_if_visible(&mut surface, false);
        assert_eq!(generation.next, 0);
        generation.start_if_visible(&mut surface, true);
        assert_eq!(generation.next, 1);
        assert_eq!(surface.presentation_generation, 1);

        // A live exposure update does not create a new render lifecycle.
        generation.start_if_visible(&mut surface, true);
        assert_eq!(generation.next, 1);
        generation.start_if_visible(&mut surface, false);
        generation.start_if_visible(&mut surface, true);
        assert_eq!(generation.next, 2);
        assert_eq!(surface.presentation_generation, 2);
    }

    #[test]
    fn mounted_surface_is_fully_despawned_when_exposure_turns_off() {
        let mut app = App::new();
        let mut exposures = EngineExposures::default();
        {
            let mut terrain = exposures.writer("terrain-progress");
            terrain.visible(false);
        }
        let revision = exposures.revision;
        app.insert_resource(exposures)
            .init_resource::<RuntimeSurfaceLayouts>()
            .insert_resource(RuntimeUiManifestState {
                applied: Some(1),
                rebuild_pending: false,
                ..Default::default()
            });
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        let root = app
            .world_mut()
            .spawn((
                Node::default(),
                HtmlNode(Handle::default()),
                Styled::Inherited,
                InlineStyle::default(),
                TemplateProperties::default(),
                RuntimeUiSurface {
                    layout_id: "terrain-progress".to_owned(),
                    namespace: "terrain-progress".to_owned(),
                    template: Handle::default(),
                    stylesheet: Handle::default(),
                    bindings: HashMap::new(),
                    collections: Vec::new(),
                    visible_in_perspective: None,
                    gate: None,
                    setting: None,
                    setting_default: false,
                    interactive: false,

                    modal: false,
                    dismiss_action: None,
                    draggable: false,
                    mounted: true,
                    presentation_ready: false,
                    presentation_visible: false,
                    presentation_generation: 0,
                    placement: RuntimeUiPlacement::Viewport,
                    has_projected_exposure: true,
                    applied_revision: revision,
                    applied_placement: None,
                },
                Visibility::Visible,
            ))
            .id();
        let child = app.world_mut().spawn((Node::default(), ChildOf(root))).id();
        let grandchild = app
            .world_mut()
            .spawn((Node::default(), ChildOf(child)))
            .id();

        app.add_systems(Update, apply_runtime_ui_exposures);
        app.update();

        assert!(app.world().get_entity(root).is_err());
        assert!(app.world().get_entity(child).is_err());
        assert!(app.world().get_entity(grandchild).is_err());
    }

    #[test]
    fn retained_unmounted_surface_is_fully_despawned_when_exposure_turns_off() {
        let mut app = App::new();
        let mut exposures = EngineExposures::default();
        {
            let mut terrain = exposures.writer("terrain-progress");
            terrain.visible(false);
        }
        let revision = exposures.revision;
        app.insert_resource(exposures)
            .init_resource::<RuntimeSurfaceLayouts>()
            .insert_resource(RuntimeUiManifestState {
                applied: Some(1),
                rebuild_pending: false,
                ..Default::default()
            });
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        let root = app
            .world_mut()
            .spawn((
                Node::default(),
                HtmlNode(Handle::default()),
                Styled::Inherited,
                InlineStyle::default(),
                TemplateProperties::default(),
                RuntimeUiSurface {
                    layout_id: "terrain-progress".to_owned(),
                    namespace: "terrain-progress".to_owned(),
                    template: Handle::default(),
                    stylesheet: Handle::default(),
                    bindings: HashMap::new(),
                    collections: Vec::new(),
                    visible_in_perspective: None,
                    gate: None,
                    setting: None,
                    setting_default: false,
                    interactive: false,

                    modal: false,
                    dismiss_action: None,
                    draggable: false,
                    mounted: false,
                    presentation_ready: false,
                    presentation_visible: false,
                    presentation_generation: 0,
                    placement: RuntimeUiPlacement::Viewport,
                    has_projected_exposure: true,
                    applied_revision: revision,
                    applied_placement: None,
                },
                Visibility::Visible,
            ))
            .id();

        app.add_systems(Update, apply_runtime_ui_exposures);
        app.update();

        assert!(
            app.world().get_entity(root).is_err(),
            "retained HUI state must not survive an exposure being withdrawn"
        );
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
    fn oversized_window_surface_is_shrunk_and_kept_reachable() {
        let window = Window {
            resolution: bevy::window::WindowResolution::new(320, 180),
            ..Default::default()
        };
        let placement = RuntimeUiPlacement::Window {
            anchor: RuntimeUiWindowAnchor::TopCenter,
            offset: Vec2::new(0.0, 34.0),
            width: 900.0,
            height: 232.0,
        };
        let resolved = resolve_placement(&placement, None, None, Some(&window)).unwrap();

        assert_eq!(
            resolved.rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(320.0, 180.0))
        );
    }

    #[test]
    fn authored_window_offset_cannot_push_surface_offscreen() {
        let window = Window {
            resolution: bevy::window::WindowResolution::new(320, 180),
            ..Default::default()
        };
        let placement = RuntimeUiPlacement::Window {
            anchor: RuntimeUiWindowAnchor::BottomRight,
            offset: Vec2::new(400.0, -400.0),
            width: 120.0,
            height: 80.0,
        };
        let resolved = resolve_placement(&placement, None, None, Some(&window)).unwrap();

        assert!(resolved.rect.min.x >= 0.0);
        assert!(resolved.rect.min.y >= 0.0);
        assert!(resolved.rect.max.x <= 320.0);
        assert!(resolved.rect.max.y <= 180.0);
    }

    #[test]
    fn applied_placement_makes_manifest_bounds_authoritative() {
        let rect = egui::Rect::from_min_size(egui::pos2(12.0, 24.0), egui::vec2(224.0, 184.0));
        let mut node = Node::default();

        apply_placement(&mut node, rect);

        assert!(placement_is_applied(&node, rect));
        assert_eq!(node.min_width, Val::Px(224.0));
        assert_eq!(node.max_height, Val::Px(184.0));
        assert_eq!(node.overflow, Overflow::hidden());
    }

    #[test]
    fn dock_placement_uses_authoritative_panel_rect() {
        let mut rects = PanelRects::default();
        let panel = PanelId("right_inspector");
        rects.record(
            panel,
            lunco_viewport_core::PanelRect {
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
    fn manifest_validation_rejects_duplicate_identity_and_accepts_authored_action() {
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

        let authored_action: RuntimeUiManifest = serde_json::from_str(
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
        authored_action
            .validate()
            .expect("Twin-authored semantic actions must be accepted");
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
            true,
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
