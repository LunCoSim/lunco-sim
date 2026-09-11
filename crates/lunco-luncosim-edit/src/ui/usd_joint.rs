//! Standard USD Physics joint authoring for the Editor perspective.
//!
//! The composed stage is the only source of joint facts. This view-model reads
//! the same `UsdRead` surface used by the physics projector, while edits go
//! through `UsdOp` and the document journal. It deliberately contains no
//! vehicle-specific rules and no runtime joint setter: a joint is a USD
//! relationship plus standard `UsdPhysics` attributes.

use std::collections::{BTreeSet, HashMap};

use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use bevy_egui::egui;
use lunco_doc::DocumentId;
use lunco_usd::document::{LayerId, UsdOp};
use lunco_usd_bevy::{
    author::normalize_value_literal, stage_convention, CanonicalStages, SdfPath, UsdPrimPath,
    UsdRead, UsdStageAsset,
};
use lunco_usd_ui::viewport::{UsdPreviewId, UsdViewportState};

const JOINT_TYPES: &[&str] = &[
    "PhysicsJoint",
    "PhysicsFixedJoint",
    "PhysicsRevoluteJoint",
    "PhysicsPrismaticJoint",
    "PhysicsSphericalJoint",
    "PhysicsDistanceJoint",
];

const JOINT_BOOL_ATTRIBUTES: &[(&str, &str)] = &[
    ("physics:jointEnabled", "Joint enabled"),
    ("physics:collisionEnabled", "Collision enabled"),
    (
        "physics:excludeFromArticulation",
        "Exclude from articulation",
    ),
];

const ANCHOR_COLOR: Color = Color::srgb(1.0, 0.85, 0.2);
const LINK_COLOR: Color = Color::srgb(0.55, 0.55, 0.55);
const ANCHOR_RADIUS: f32 = 0.06;
const AXIS_LEN: f32 = 0.4;

/// Gizmos for USD previews use the session's isolated render layer rather
/// than the live-scene layer used by the ordinary editor gizmo.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub(crate) struct UsdJointPreviewGizmoConfigGroup;

/// One standard scalar on a joint, retained in the USD-native value frame.
#[derive(Clone)]
pub struct JointScalar {
    pub name: String,
    pub label: String,
    pub value: f64,
    pub type_name: String,
    pub unit: String,
}

/// One standard boolean on a joint.
#[derive(Clone)]
pub struct JointBoolean {
    pub name: &'static str,
    pub label: &'static str,
    pub value: bool,
}

/// Render-ready authored joint view. It is derived state; USD remains
/// authoritative and every mutation is dispatched as a typed document command.
#[derive(Default, Clone)]
pub struct UsdJointSessionView {
    pub preview: UsdPreviewId,
    pub entity: Option<Entity>,
    pub doc: Option<DocumentId>,
    pub edit_target: Option<LayerId>,
    pub generation: u64,
    pub path: String,
    pub type_name: String,
    pub body0: Vec<String>,
    pub body1: Vec<String>,
    pub body_options: Vec<String>,
    pub axis: Option<String>,
    pub local_pos0: Option<[f64; 3]>,
    pub local_pos1: Option<[f64; 3]>,
    /// Quaternion components in Bevy order `(x, y, z, w)`, already in the
    /// canonical coordinate basis. USD literal emission reverses this to its
    /// standard `(w, x, y, z)` order.
    pub local_rot0: Option<[f64; 4]>,
    pub local_rot1: Option<[f64; 4]>,
    pub booleans: Vec<JointBoolean>,
    pub scalars: Vec<JointScalar>,
}

impl UsdJointSessionView {
    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Session-keyed authored joint views. Runtime joint state remains a separate
/// live physics view; this resource contains only composed USD authoring data.
#[derive(Resource, Default)]
pub struct UsdJointView {
    sessions: HashMap<UsdPreviewId, UsdJointSessionView>,
}

impl UsdJointView {
    pub(crate) fn focused(&self, viewport: &UsdViewportState) -> Option<&UsdJointSessionView> {
        viewport
            .focused_preview_id()
            .and_then(|preview| self.sessions.get(&preview))
    }
}

fn is_joint(type_name: &str) -> bool {
    JOINT_TYPES.contains(&type_name)
}

fn schema_type(name: &str, fallback: &str) -> String {
    lunco_usd::schema::SchemaRegistry::global()
        .read()
        .ok()
        .and_then(|registry| {
            registry
                .property(name)
                .map(|property| property.type_name.clone())
        })
        .unwrap_or_else(|| fallback.to_owned())
}

fn canonical_position<R: UsdRead>(
    stage: &R,
    path: &SdfPath,
    name: &str,
    convention: &lunco_usd_bevy::ConventionTransform,
) -> Option<[f64; 3]> {
    stage
        .vec3_f64(path, name)
        .map(DVec3::from_array)
        .map(|value| convention.point_d(value).to_array())
}

fn canonical_rotation<R: UsdRead>(
    stage: &R,
    path: &SdfPath,
    name: &str,
    convention: &lunco_usd_bevy::ConventionTransform,
) -> Option<[f64; 4]> {
    stage
        .quat_d(path, name)
        .map(|value| convention.rotation_d(value).to_array())
}

fn authored_scalar_name(name: &str) -> bool {
    matches!(
        name,
        "physics:breakForce" | "physics:breakTorque" | "physics:lowerLimit" | "physics:upperLimit"
    ) || (name.starts_with("limit:")
        && (name.ends_with(":physics:low") || name.ends_with(":physics:high")))
        || (name.starts_with("drive:")
            && (name.ends_with(":physics:damping")
                || name.ends_with(":physics:maxForce")
                || name.ends_with(":physics:stiffness")
                || name.ends_with(":physics:targetPosition")
                || name.ends_with(":physics:targetVelocity")))
}

fn scalar_label(name: &str) -> String {
    let leaf = name.rsplit(':').next().unwrap_or(name);
    let mut label = String::with_capacity(leaf.len() + 8);
    for (index, character) in leaf.chars().enumerate() {
        if character.is_uppercase() && index != 0 {
            label.push(' ');
        }
        label.push(if index == 0 {
            character.to_ascii_uppercase()
        } else {
            character
        });
    }
    label
}

fn scalar_unit(type_name: &str, joint_type: &str, name: &str) -> String {
    if type_name != "float" && type_name != "double" {
        return String::new();
    }
    if matches!(name, "physics:lowerLimit" | "physics:upperLimit")
        && joint_type == "PhysicsRevoluteJoint"
    {
        return "deg".into();
    }
    if name.contains(":angular:")
        && (name.ends_with(":physics:targetPosition") || name.ends_with(":physics:targetVelocity"))
    {
        return if name.ends_with(":physics:targetVelocity") {
            "deg/s".into()
        } else {
            "deg".into()
        };
    }
    if matches!(name, "physics:lowerLimit" | "physics:upperLimit") {
        return "stage units".into();
    }
    if name.contains(":linear:")
        && (name.ends_with(":physics:targetPosition") || name.ends_with(":physics:targetVelocity"))
    {
        return if name.ends_with(":physics:targetVelocity") {
            "stage units/s".into()
        } else {
            "stage units".into()
        };
    }
    String::new()
}

/// Rebuild authored joint state for every open preview lease.
pub fn produce_usd_joint_view(
    selected: Option<Res<lunco_scene_commands::SelectedEntities>>,
    target: Option<Res<crate::InspectorTarget>>,
    q: Query<&UsdPrimPath>,
    q_parents: Query<&ChildOf>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    viewport: Option<Res<UsdViewportState>>,
    mut views: ResMut<UsdJointView>,
) {
    let Some(viewport) = viewport else {
        views.sessions.clear();
        return;
    };
    let open: std::collections::HashSet<_> = viewport.sessions().map(|s| s.id()).collect();
    views.sessions.retain(|preview, _| open.contains(preview));

    for session in viewport.sessions() {
        let view = views
            .sessions
            .entry(session.id())
            .or_insert_with(|| UsdJointSessionView {
                preview: session.id(),
                doc: Some(session.doc()),
                edit_target: Some(session.edit_target().clone()),
                ..Default::default()
            });
        view.clear();
        view.preview = session.id();
        view.doc = Some(session.doc());
        view.edit_target = Some(session.edit_target().clone());
        view.generation = session.projected_generation();
        if !session.projection_ready() {
            continue;
        }

        let Some(entity) = crate::ui::selected_entity_in_preview(
            session,
            selected.as_deref(),
            target.as_deref(),
            &q,
            &q_parents,
        ) else {
            continue;
        };
        let Ok(prim) = q.get(entity) else {
            continue;
        };
        let handle = session.stage_handle();
        if prim.stage_handle.id() != handle.id() {
            continue;
        }
        let stage_id = handle.id();
        if canonical.get(stage_id).is_none() {
            if let Some(recipe) = stages.get(handle).and_then(|asset| asset.recipe.clone()) {
                canonical.get_or_build(stage_id, &recipe);
            }
        }
        let Some(stage) = canonical.get(stage_id).map(|value| value.view()) else {
            continue;
        };
        let Ok(path) = SdfPath::new(&prim.path) else {
            continue;
        };
        let Some(type_name) = stage.type_name(&path).filter(|name| is_joint(name)) else {
            continue;
        };
        let Ok(convention) = stage_convention(&stage) else {
            continue;
        };

        let mut body_options = vec![String::new()];
        let mut seen = BTreeSet::new();
        seen.insert(String::new());
        for candidate in stage.prim_paths() {
            if stage.has_api_schema(&candidate, "PhysicsRigidBodyAPI") {
                let candidate = candidate.as_str().to_owned();
                if seen.insert(candidate.clone()) {
                    body_options.push(candidate);
                }
            }
        }
        let body0 = stage
            .rel_targets(&path, "physics:body0")
            .into_iter()
            .map(|value| value.as_str().to_owned())
            .collect::<Vec<_>>();
        let body1 = stage
            .rel_targets(&path, "physics:body1")
            .into_iter()
            .map(|value| value.as_str().to_owned())
            .collect::<Vec<_>>();
        for current in body0.iter().chain(body1.iter()) {
            if seen.insert(current.clone()) {
                body_options.push(current.clone());
            }
        }
        body_options.sort();

        let axis = matches!(
            type_name.as_str(),
            "PhysicsRevoluteJoint" | "PhysicsPrismaticJoint"
        )
        .then(|| stage.text(&path, "physics:axis"))
        .flatten();

        let booleans = JOINT_BOOL_ATTRIBUTES
            .iter()
            .filter_map(|&(name, label)| {
                stage
                    .boolean(&path, name)
                    .map(|value| JointBoolean { name, label, value })
            })
            .collect();

        let mut scalars = Vec::new();
        for name in stage.attr_names(&path) {
            if !authored_scalar_name(&name) {
                continue;
            }
            let Some(value) = stage.real(&path, &name) else {
                continue;
            };
            if !value.is_finite() {
                continue;
            }
            let type_name_for_attr = schema_type(&name, "float");
            scalars.push(JointScalar {
                type_name: type_name_for_attr.clone(),
                label: scalar_label(&name),
                unit: scalar_unit(&type_name_for_attr, &type_name, &name),
                name,
                value,
            });
        }
        scalars.sort_by(|a, b| a.name.cmp(&b.name));

        view.entity = Some(entity);
        view.path = prim.path.clone();
        view.type_name = type_name;
        view.body0 = body0;
        view.body1 = body1;
        view.body_options = body_options;
        view.axis = axis;
        view.local_pos0 = canonical_position(&stage, &path, "physics:localPos0", &convention);
        view.local_pos1 = canonical_position(&stage, &path, "physics:localPos1", &convention);
        view.local_rot0 = canonical_rotation(&stage, &path, "physics:localRot0", &convention);
        view.local_rot1 = canonical_rotation(&stage, &path, "physics:localRot1", &convention);
        view.booleans = booleans;
        view.scalars = scalars;
    }
}

fn body_text(targets: &[String]) -> String {
    match targets {
        [] => "World / unconnected".into(),
        [target] => target.clone(),
        _ => "Invalid: multiple targets".into(),
    }
}

fn body_value(targets: &[String]) -> Option<String> {
    targets.first().cloned().filter(|_| targets.len() == 1)
}

fn preview_body_path(targets: &[String]) -> Option<&str> {
    match targets {
        [] => Some(""),
        [target] => Some(target),
        _ => None,
    }
}

fn preview_body_transform(
    path: &str,
    session: &lunco_usd_ui::viewport::UsdPreviewSession,
    q_prims: &Query<(Entity, &UsdPrimPath, &GlobalTransform)>,
    q_globals: &Query<&GlobalTransform>,
    q_parents: &Query<&ChildOf>,
) -> Option<GlobalTransform> {
    if path.is_empty() {
        return q_globals.get(session.scene_root()).ok().copied();
    }

    q_prims
        .iter()
        .find(|(entity, prim, _)| {
            prim.stage_handle.id() == session.stage_handle().id()
                && prim.path == path
                && crate::ui::is_editor_preview_entity(*entity, session.scene_root(), q_parents)
        })
        .map(|(_, _, transform)| *transform)
}

fn preview_joint_frame(
    position: Option<[f64; 3]>,
    rotation: Option<[f64; 4]>,
    body: &GlobalTransform,
) -> Option<(Vec3, Quat)> {
    let position = position?;
    if position.iter().any(|value| !value.is_finite()) {
        return None;
    }
    let local_position = Vec3::new(position[0] as f32, position[1] as f32, position[2] as f32);
    let anchor = body.transform_point(local_position);
    if !anchor.is_finite() {
        return None;
    }

    let local_rotation = match rotation {
        None => DQuat::IDENTITY,
        Some(value) => {
            if value.iter().any(|component| !component.is_finite()) {
                return None;
            }
            let quaternion = DQuat::from_xyzw(value[0], value[1], value[2], value[3]);
            let length_squared = quaternion.length_squared();
            if !length_squared.is_finite() || length_squared <= 1.0e-24 {
                return None;
            }
            quaternion.normalize()
        }
    };
    let world_rotation = body.rotation().as_dquat() * local_rotation;
    world_rotation
        .is_finite()
        .then(|| (anchor, world_rotation.as_quat()))
}

fn draw_preview_joint_frame(
    gizmos: &mut Gizmos<UsdJointPreviewGizmoConfigGroup>,
    anchor: Vec3,
    rotation: Quat,
) {
    const X_COLOR: Color = Color::srgb(0.95, 0.2, 0.2);
    const Y_COLOR: Color = Color::srgb(0.2, 0.9, 0.3);
    const Z_COLOR: Color = Color::srgb(0.25, 0.55, 1.0);
    let length = AXIS_LEN * 1.5;
    gizmos.sphere(anchor, ANCHOR_RADIUS, ANCHOR_COLOR);
    for (axis, color) in [(Vec3::X, X_COLOR), (Vec3::Y, Y_COLOR), (Vec3::Z, Z_COLOR)] {
        let direction = rotation * axis * length;
        gizmos.arrow(anchor, anchor + direction, color);
    }
}

/// Draw the selected joint's authored frames in an isolated USD preview.
///
/// The preview is intentionally inert: it reads the composed joint view and
/// projected body transforms, but never creates or mutates an Avian joint.
pub(crate) fn sync_usd_joint_preview_gizmo_config(
    viewport: Option<Res<UsdViewportState>>,
    mut store: ResMut<bevy::gizmos::config::GizmoConfigStore>,
) {
    let (config, _) = store.config_mut::<UsdJointPreviewGizmoConfigGroup>();
    let Some(layer) = viewport
        .as_deref()
        .and_then(UsdViewportState::focused_session)
        .map(|session| bevy::camera::visibility::RenderLayers::layer(session.render_layer()))
    else {
        config.enabled = false;
        return;
    };

    config.enabled = true;
    config.render_layers = layer;
    config.depth_bias = -1.0;
    config.line.width = 4.0;
}

pub(crate) fn draw_usd_joint_preview_viz(
    mut gizmos: Gizmos<UsdJointPreviewGizmoConfigGroup>,
    viewport: Option<Res<UsdViewportState>>,
    views: Option<Res<UsdJointView>>,
    q_prims: Query<(Entity, &UsdPrimPath, &GlobalTransform)>,
    q_globals: Query<&GlobalTransform>,
    q_parents: Query<&ChildOf>,
) {
    let (Some(viewport), Some(views)) = (viewport, views) else {
        return;
    };
    let Some(session) = viewport.focused_session() else {
        return;
    };
    let Some(view) = views.focused(&viewport) else {
        return;
    };
    let Some(entity) = view.entity else {
        return;
    };
    if !crate::ui::is_editor_preview_entity(entity, session.scene_root(), &q_parents) {
        return;
    }

    let body0 = preview_body_path(&view.body0)
        .and_then(|path| preview_body_transform(path, session, &q_prims, &q_globals, &q_parents));
    let body1 = preview_body_path(&view.body1)
        .and_then(|path| preview_body_transform(path, session, &q_prims, &q_globals, &q_parents));
    let frame0 = body0
        .as_ref()
        .and_then(|body| preview_joint_frame(view.local_pos0, view.local_rot0, body));
    let frame1 = body1
        .as_ref()
        .and_then(|body| preview_joint_frame(view.local_pos1, view.local_rot1, body));

    if let Some((anchor, rotation)) = frame0 {
        draw_preview_joint_frame(&mut gizmos, anchor, rotation);
    }
    if let Some((anchor, rotation)) = frame1 {
        draw_preview_joint_frame(&mut gizmos, anchor, rotation);
    }
    if let (Some((anchor0, _)), Some((anchor1, _))) = (frame0, frame1) {
        gizmos.line(anchor0, anchor1, LINK_COLOR);
    }
}

fn apply_attribute(
    ctx: &mut lunco_workbench_core::PanelCtx,
    view: &UsdJointSessionView,
    name: &str,
    type_name: &str,
    value: String,
) {
    let (Some(doc), Some(edit_target)) = (view.doc, view.edit_target.clone()) else {
        return;
    };
    ctx.trigger(lunco_usd::commands::ApplyUsdOp {
        doc_id: doc,
        parent_gen: Some(view.generation),
        op: UsdOp::SetAttribute {
            edit_target,
            path: view.path.clone(),
            name: name.to_owned(),
            type_name: type_name.to_owned(),
            value,
        },
    });
}

fn apply_relationship(
    ctx: &mut lunco_workbench_core::PanelCtx,
    view: &UsdJointSessionView,
    name: &str,
    target: String,
) {
    let (Some(doc), Some(edit_target)) = (view.doc, view.edit_target.clone()) else {
        return;
    };
    ctx.trigger(lunco_usd::commands::ApplyUsdOp {
        doc_id: doc,
        parent_gen: Some(view.generation),
        op: UsdOp::SetRelationship {
            edit_target,
            path: view.path.clone(),
            name: name.to_owned(),
            targets: (!target.is_empty()).then_some(target).into_iter().collect(),
        },
    });
}

fn edit_released(response: &egui::Response) -> bool {
    response.drag_stopped() || (response.changed() && !response.dragged())
}

fn position_control(ui: &mut egui::Ui, label: &str, current: [f64; 3]) -> Option<[f64; 3]> {
    let mut value = current;
    let mut committed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        for (index, component) in value.iter_mut().enumerate() {
            let response = ui.add(egui::DragValue::new(component).speed(0.01).prefix(
                match index {
                    0 => "x ",
                    1 => "y ",
                    _ => "z ",
                },
            ));
            committed |= edit_released(&response);
        }
    });
    committed.then_some(value)
}

fn rotation_control(ui: &mut egui::Ui, label: &str, current: [f64; 4]) -> Option<[f64; 4]> {
    let mut value = current;
    let mut committed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        for (index, component) in value.iter_mut().enumerate() {
            let response = ui.add(egui::DragValue::new(component).speed(0.01).prefix(
                match index {
                    0 => "x ",
                    1 => "y ",
                    2 => "z ",
                    _ => "w ",
                },
            ));
            committed |= edit_released(&response);
        }
    });
    if !committed {
        return None;
    }
    let quaternion = DQuat::from_xyzw(value[0], value[1], value[2], value[3]);
    let length_squared = quaternion.length_squared();
    if !length_squared.is_finite() || length_squared <= 1.0e-24 {
        ui.colored_label(egui::Color32::RED, "Rotation must be a non-zero quaternion");
        return None;
    }
    Some(quaternion.normalize().to_array())
}

fn usd_quat_literal(type_name: &str, value: [f64; 4]) -> Option<String> {
    normalize_value_literal(
        type_name,
        &format!("({}, {}, {}, {})", value[3], value[0], value[1], value[2]),
    )
    .ok()
}

fn usd_point_literal(type_name: &str, value: [f64; 3]) -> Result<String, String> {
    if value.iter().any(|component| !component.is_finite()) {
        return Err("Position must contain only finite values".into());
    }
    normalize_value_literal(
        type_name,
        &format!("({}, {}, {})", value[0], value[1], value[2]),
    )
    .map_err(|error| error.to_string())
}

/// Paint the authored standard joint editor for the selected prim.
pub fn authored_joint_section(
    ui: &mut egui::Ui,
    ctx: &mut lunco_workbench_core::PanelCtx,
    entity: Entity,
) {
    let Some(view) = ctx
        .resource::<UsdViewportState>()
        .and_then(|viewport| {
            ctx.resource::<UsdJointView>()
                .and_then(|views| views.focused(viewport))
        })
        .filter(|view| view.entity == Some(entity))
        .cloned()
    else {
        return;
    };

    egui::CollapsingHeader::new("USD Joint")
        .default_open(true)
        .show(ui, |ui| {
            ui.label(format!("Type: {}", view.type_name));
            ui.label(format!("Path: {}", view.path));
            if view.body0.len() > 1 || view.body1.len() > 1 {
                ui.colored_label(
                    egui::Color32::RED,
                    "The joint has multiple relationship targets; choose a replacement.",
                );
            }

            for (name, label, targets) in [
                ("physics:body0", "Body 0", view.body0.clone()),
                ("physics:body1", "Body 1", view.body1.clone()),
            ] {
                let mut selected = body_value(&targets).unwrap_or_default();
                egui::ComboBox::from_label(label)
                    .selected_text(body_text(&targets))
                    .show_ui(ui, |ui| {
                        for option in &view.body_options {
                            ui.selectable_value(
                                &mut selected,
                                option.clone(),
                                if option.is_empty() {
                                    "World / unconnected"
                                } else {
                                    option
                                },
                            );
                        }
                    });
                if selected != body_value(&targets).unwrap_or_default() {
                    apply_relationship(ctx, &view, name, selected);
                }
            }

            if let Some(axis) = &view.axis {
                let mut selected = axis.clone();
                egui::ComboBox::from_label("Axis")
                    .selected_text(&selected)
                    .show_ui(ui, |ui| {
                        for option in ["X", "Y", "Z"] {
                            ui.selectable_value(&mut selected, option.to_owned(), option);
                        }
                    });
                if selected != *axis {
                    if let Ok(value) = normalize_value_literal("token", &format!("\"{selected}\""))
                    {
                        apply_attribute(ctx, &view, "physics:axis", "token", value);
                    }
                }
            }

            ui.separator();
            ui.label(egui::RichText::new("Joint flags").strong());
            for flag in &view.booleans {
                let mut value = flag.value;
                if ui.checkbox(&mut value, flag.label).changed() {
                    if let Ok(literal) = normalize_value_literal("bool", &value.to_string()) {
                        apply_attribute(ctx, &view, flag.name, "bool", literal);
                    }
                }
            }

            ui.separator();
            ui.label(egui::RichText::new("Frames (canonical metres / basis)").strong());
            ui.weak("Amber anchors and XYZ arrows in the preview show the edited joint frames.");
            if let Some(current) = view.local_pos0 {
                if let Some(value) = position_control(ui, "Local position 0", current) {
                    let type_name = schema_type("physics:localPos0", "point3f");
                    match usd_point_literal(&type_name, value) {
                        Ok(literal) => {
                            apply_attribute(ctx, &view, "physics:localPos0", &type_name, literal)
                        }
                        Err(error) => {
                            ui.colored_label(egui::Color32::RED, error);
                        }
                    }
                }
            }
            if let Some(current) = view.local_pos1 {
                if let Some(value) = position_control(ui, "Local position 1", current) {
                    let type_name = schema_type("physics:localPos1", "point3f");
                    match usd_point_literal(&type_name, value) {
                        Ok(literal) => {
                            apply_attribute(ctx, &view, "physics:localPos1", &type_name, literal)
                        }
                        Err(error) => {
                            ui.colored_label(egui::Color32::RED, error);
                        }
                    }
                }
            }
            if let Some(current) = view.local_rot0 {
                if let Some(value) = rotation_control(ui, "Local rotation 0", current) {
                    let type_name = schema_type("physics:localRot0", "quatf");
                    if let Some(literal) = usd_quat_literal(&type_name, value) {
                        apply_attribute(ctx, &view, "physics:localRot0", &type_name, literal);
                    }
                }
            }
            if let Some(current) = view.local_rot1 {
                if let Some(value) = rotation_control(ui, "Local rotation 1", current) {
                    let type_name = schema_type("physics:localRot1", "quatf");
                    if let Some(literal) = usd_quat_literal(&type_name, value) {
                        apply_attribute(ctx, &view, "physics:localRot1", &type_name, literal);
                    }
                }
            }

            if !view.scalars.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new("Limits and drives").strong());
                for scalar in &view.scalars {
                    let mut value = scalar.value;
                    let response = ui.add(
                        egui::DragValue::new(&mut value)
                            .speed(0.1)
                            .prefix(format!("{}: ", scalar.label)),
                    );
                    if edit_released(&response) {
                        if let Ok(literal) =
                            normalize_value_literal(&scalar.type_name, &value.to_string())
                        {
                            apply_attribute(ctx, &view, &scalar.name, &scalar.type_name, literal);
                        }
                    }
                    if !scalar.unit.is_empty() {
                        ui.label(&scalar.unit);
                    }
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::usd_point_literal;

    #[test]
    fn joint_position_literal_rejects_non_finite_values() {
        let error = usd_point_literal("point3f", [f64::NAN, 0.0, 0.0]).unwrap_err();
        assert_eq!(error, "Position must contain only finite values");
    }

    #[test]
    fn joint_position_literal_preserves_the_authored_type() {
        let literal = usd_point_literal("point3d", [1.0, -2.5, 3.0]).unwrap();
        assert_eq!(literal, "(1.0, -2.5, 3.0)");
    }
}
