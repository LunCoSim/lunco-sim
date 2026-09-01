//! Standard USD Physics joint authoring for the Editor perspective.
//!
//! The composed stage is the only source of joint facts. This view-model reads
//! the same `UsdRead` surface used by the physics projector, while edits go
//! through `UsdOp` and the document journal. It deliberately contains no
//! vehicle-specific rules and no runtime joint setter: a joint is a USD
//! relationship plus standard `UsdPhysics` attributes.

use std::collections::BTreeSet;

use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use bevy_egui::egui;
use lunco_doc::DocumentId;
use lunco_usd::document::{LayerId, UsdOp};
use lunco_usd::ui::viewport::UsdViewportState;
use lunco_usd_bevy::{
    author::normalize_value_literal, stage_convention, CanonicalStages, SdfPath, UsdPrimPath,
    UsdRead, UsdStageAsset,
};

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
#[derive(Resource, Default, Clone)]
pub struct UsdJointView {
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

impl UsdJointView {
    fn clear(&mut self) {
        *self = Self::default();
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

/// Rebuild the selected joint view from the focused Editor preview.
pub fn produce_usd_joint_view(
    selected: Option<Res<lunco_scene_commands::SelectedEntities>>,
    target: Option<Res<crate::InspectorTarget>>,
    q: Query<&UsdPrimPath>,
    q_parents: Query<&ChildOf>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    viewport: Option<Res<UsdViewportState>>,
    mut view: ResMut<UsdJointView>,
) {
    view.clear();

    let Some(viewport) = viewport else {
        return;
    };
    let Some(preview_root) = viewport.focused_scene_root() else {
        return;
    };
    let Some(doc) = viewport.focused_doc() else {
        return;
    };
    let Some(edit_target) = viewport.focused_edit_target().cloned() else {
        return;
    };
    let Some(entity) = target
        .as_deref()
        .and_then(|value| value.part)
        .filter(|entity| q.get(*entity).is_ok())
        .or_else(|| selected.as_deref().and_then(|value| value.primary()))
    else {
        return;
    };
    if !crate::ui::is_editor_preview_entity(entity, preview_root, &q_parents) {
        return;
    }
    let Ok(prim) = q.get(entity) else {
        return;
    };
    let Some(handle) = viewport.focused_stage_handle() else {
        return;
    };
    if prim.stage_handle.id() != handle.id() {
        return;
    }
    let stage_id = handle.id();
    if canonical.get(stage_id).is_none() {
        if let Some(recipe) = stages.get(handle).and_then(|asset| asset.recipe.clone()) {
            canonical.get_or_build(stage_id, &recipe);
        }
    }
    let Some(stage) = canonical.get(stage_id).map(|value| value.view()) else {
        return;
    };
    let Ok(path) = SdfPath::new(&prim.path) else {
        return;
    };
    let Some(type_name) = stage.type_name(&path).filter(|name| is_joint(name)) else {
        return;
    };
    let Ok(convention) = stage_convention(&stage) else {
        return;
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

    let mut booleans = Vec::new();
    for &(name, label) in JOINT_BOOL_ATTRIBUTES {
        if let Some(value) = stage.boolean(&path, name) {
            booleans.push(JointBoolean { name, label, value });
        }
    }

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
        scalars.push(JointScalar {
            type_name: schema_type(&name, "float"),
            label: scalar_label(&name),
            unit: scalar_unit(&schema_type(&name, "float"), &type_name, &name),
            name,
            value,
        });
    }
    scalars.sort_by(|a, b| a.name.cmp(&b.name));

    view.entity = Some(entity);
    view.doc = Some(doc);
    view.edit_target = Some(edit_target);
    view.generation = viewport
        .focused_session()
        .map(|session| session.projected_generation())
        .unwrap_or_default();
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

fn apply_attribute(
    ctx: &mut lunco_workbench::PanelCtx,
    view: &UsdJointView,
    name: &str,
    type_name: &str,
    value: String,
) {
    let (Some(doc), Some(edit_target)) = (view.doc, view.edit_target.clone()) else {
        return;
    };
    ctx.trigger(lunco_usd::commands::ApplyUsdOp {
        doc,
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
    ctx: &mut lunco_workbench::PanelCtx,
    view: &UsdJointView,
    name: &str,
    target: String,
) {
    let (Some(doc), Some(edit_target)) = (view.doc, view.edit_target.clone()) else {
        return;
    };
    ctx.trigger(lunco_usd::commands::ApplyUsdOp {
        doc,
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

/// Paint the authored standard joint editor for the selected prim.
pub fn authored_joint_section(
    ui: &mut egui::Ui,
    ctx: &mut lunco_workbench::PanelCtx,
    entity: Entity,
) {
    let Some(view) = ctx
        .resource::<UsdJointView>()
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
            if let Some(current) = view.local_pos0 {
                if let Some(value) = position_control(ui, "Local position 0", current) {
                    let type_name = schema_type("physics:localPos0", "point3f");
                    if let Ok(literal) = normalize_value_literal(
                        &type_name,
                        &format!("({}, {}, {})", value[0], value[1], value[2]),
                    ) {
                        apply_attribute(ctx, &view, "physics:localPos0", &type_name, literal);
                    }
                }
            }
            if let Some(current) = view.local_pos1 {
                if let Some(value) = position_control(ui, "Local position 1", current) {
                    let type_name = schema_type("physics:localPos1", "point3f");
                    if let Ok(literal) = normalize_value_literal(
                        &type_name,
                        &format!("({}, {}, {})", value[0], value[1], value[2]),
                    ) {
                        apply_attribute(ctx, &view, "physics:localPos1", &type_name, literal);
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
