//! Inspector panel — `lunco-workbench::Panel` implementation.
//!
//! Migrates the old standalone egui window to use bevy_workbench docking.
//! Provides editable sliders for transform, physics, and wheel parameters.
//!
//! **WP-8 reactive shape:** `render` takes a capability-narrowed
//! [`PanelCtx`] — no raw `&mut World`. Reads go through
//! [`PanelCtx::resource`]/[`PanelCtx::get`] (and, for query-derived data
//! like the scene sun / camera / joint, the change-driven
//! [`InspectorView`] view-model produced by [`populate_inspector_view`]);
//! every mutation is emitted as a typed intent and applied by an observer after
//! the egui pass.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_core::ports::PortRegistry;
use lunco_cosim::{joint_angle_holder, JOINT_ANGLE_PORT};
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};
// Appearance INTENT. The Material (PBR) section edits this component, not the
// material asset — see `material_pbr_section`.
use lunco_materials::{ParamValue, ShaderLook};
use lunco_render::PbrLook;

use lunco_obstacle_field::{plugin::UpdateObstacleFieldSpec, ObstacleFieldSpec, Pattern};

use lunco_scene_commands::SelectedEntities;
// Doc resolution + material-binding walk: headless-safe, shared verbatim with the
// command layer (which is why they don't live in this panel — see `doc_resolve`).
use lunco_scene_commands::doc_resolve::{
    bound_shader_prim, geom_api_schemas, resolve_doc_for_entity,
};
use lunco_usd::commands::ApplyUsdOp;
use lunco_usd::document::{LayerId, UsdOp};
use lunco_usd_bevy::UsdPrimPath;

fn report_inspector_error(world: &mut World, message: impl Into<String>) {
    let message = message.into();
    warn!("[inspector] {message}");
    world.trigger(lunco_core::TelemetryEvent {
        name: "inspector-edit-failed".to_string(),
        source: 0,
        severity: lunco_core::Severity::Error,
        data: lunco_core::TelemetryValue::String(message),
        timestamp: 0.0,
    });
}

#[derive(Event, Clone, Copy, Debug)]
pub(crate) enum InspectorComponentEdit {
    Mass {
        entity: Entity,
        value: f32,
    },
    LinearDamping {
        entity: Entity,
        value: f64,
    },
    AngularDamping {
        entity: Entity,
        value: f64,
    },
    TerrainShader {
        entity: Entity,
        mode: lunco_terrain_surface::TerrainShaderMode,
    },
    JointSetpoint {
        holder: Entity,
        value: f64,
    },
    Anchor {
        entity: Entity,
        value: lunco_celestial::GeodeticAnchor,
    },
    Orbit {
        entity: Entity,
        value: lunco_celestial::KeplerOrbit,
    },
}

#[derive(Event, Clone, Debug)]
pub(crate) struct ProjectionEditRequested {
    entity: Entity,
    projection: Projection,
    fov: f32,
    near: f32,
    far: f32,
}

#[derive(Event, Clone, Debug)]
pub(crate) struct UsdAttributeEditRequested {
    entity: Entity,
    name: String,
    type_name: String,
    value: String,
}

#[derive(Event, Clone, Debug)]
pub(crate) struct UsdVariantEditRequested {
    entity: Entity,
    prim_path: String,
    set: String,
    variant: String,
}

#[derive(Event, Clone, Debug)]
pub(crate) struct MountSnapRequested {
    entity: Entity,
    part: String,
    joint: String,
    placement: [f64; 3],
    rotate: [f64; 3],
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Event, Clone, Debug)]
pub(crate) struct AttachAtSocketRequested {
    entity: Entity,
    host_path: String,
    name: String,
    asset: String,
    accepts: String,
    joint: lunco_usd::attach::AttachJoint,
    socket_frame: Transform,
}

#[derive(Event, Clone, Debug)]
pub(crate) struct ShaderSwapRequested {
    part: Entity,
    path: String,
}

#[derive(Event, Clone, Debug)]
pub(crate) struct ShaderCreateRequested {
    part: Entity,
    name: String,
    template: String,
}

#[derive(Event, Clone, Debug)]
pub(crate) struct ShaderImportRequested {
    part: Entity,
    source: String,
}

#[derive(Event, Clone, Debug)]
pub(crate) struct ShaderParametersRequested {
    entity: Entity,
    edits: Vec<(String, ParamValue)>,
    usd_prim_exists: bool,
}

#[derive(Event, Clone, Debug)]
pub(crate) struct PbrMaterialRequested {
    part: Entity,
    parts: Vec<Entity>,
    base: [f32; 3],
    alpha: f32,
    emissive: [f32; 3],
    metallic: f32,
    roughness: f32,
    ior: f32,
    double_sided: bool,
    base_changed: bool,
    emissive_changed: bool,
    metallic_changed: bool,
    roughness_changed: bool,
    ior_changed: bool,
}

#[derive(Event, Clone, Debug)]
pub(crate) struct ModelicaParameterRequested {
    entity: Entity,
    key: String,
    value: f64,
}

pub(crate) fn on_inspector_component_edit(
    trigger: On<InspectorComponentEdit>,
    mut commands: Commands,
    mut masses: Query<&mut avian3d::prelude::Mass>,
    mut linear: Query<&mut avian3d::prelude::LinearDamping>,
    mut angular: Query<&mut avian3d::prelude::AngularDamping>,
    mut terrain: Query<&mut lunco_terrain_surface::TerrainShaderMode>,
) {
    match *trigger.event() {
        InspectorComponentEdit::Mass { entity, value } => {
            if let Ok(mut mass) = masses.get_mut(entity) {
                mass.0 = value;
            }
        }
        InspectorComponentEdit::LinearDamping { entity, value } => {
            if let Ok(mut damping) = linear.get_mut(entity) {
                damping.0 = value;
            }
        }
        InspectorComponentEdit::AngularDamping { entity, value } => {
            if let Ok(mut damping) = angular.get_mut(entity) {
                damping.0 = value;
            }
        }
        InspectorComponentEdit::TerrainShader { entity, mode } => {
            if let Ok(mut current) = terrain.get_mut(entity) {
                *current = mode;
            }
        }
        InspectorComponentEdit::JointSetpoint { holder, value } => {
            commands.queue(move |world: &mut World| {
                let registry = world.resource::<PortRegistry>().clone();
                registry.write_port(world, holder, JOINT_ANGLE_PORT, value);
            });
        }
        InspectorComponentEdit::Anchor { entity, value } => {
            commands.queue(move |world: &mut World| {
                if let Some(mut current) = world.get_mut::<lunco_celestial::GeodeticAnchor>(entity)
                {
                    *current = value;
                }
                for (name, value) in [
                    ("lunco:anchor:lat", format!("{}", value.geodetic.lat_deg)),
                    ("lunco:anchor:lon", format!("{}", value.geodetic.lon_deg)),
                    (
                        "lunco:anchor:height",
                        format!("{}", value.geodetic.height_m),
                    ),
                ] {
                    apply_usd_attribute_change(world, entity, name, "double", value);
                }
                apply_usd_attribute_change(
                    world,
                    entity,
                    "lunco:anchor:body",
                    "int",
                    format!("{}", value.body),
                );
            });
        }
        InspectorComponentEdit::Orbit { entity, value } => {
            commands.queue(move |world: &mut World| {
                if let Some(mut current) = world.get_mut::<lunco_celestial::KeplerOrbit>(entity) {
                    *current = value;
                }
                for (name, value) in [
                    (
                        "lunco:orbit:semiMajorAxisM",
                        format!("{}", value.elements.semi_major_axis_m),
                    ),
                    (
                        "lunco:orbit:eccentricity",
                        format!("{}", value.elements.eccentricity),
                    ),
                    (
                        "lunco:orbit:inclinationDeg",
                        format!("{}", value.elements.inclination_deg),
                    ),
                    (
                        "lunco:orbit:raanDeg",
                        format!("{}", value.elements.raan_deg),
                    ),
                    (
                        "lunco:orbit:argPeriapsisDeg",
                        format!("{}", value.elements.arg_periapsis_deg),
                    ),
                    (
                        "lunco:orbit:meanAnomalyDeg",
                        format!("{}", value.elements.mean_anomaly_deg),
                    ),
                ] {
                    apply_usd_attribute_change(world, entity, name, "double", value);
                }
            });
        }
    }
}

pub(crate) fn on_projection_edit_requested(
    trigger: On<ProjectionEditRequested>,
    mut commands: Commands,
) {
    let request = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        if let Some(mut current) = world.get_mut::<Projection>(request.entity) {
            *current = request.projection;
        }
        if world.get::<UsdPrimPath>(request.entity).is_some() {
            const VERTICAL_APERTURE_MM: f32 = 15.2908;
            let focal_length = VERTICAL_APERTURE_MM / (2.0 * (request.fov * 0.5).tan());
            apply_usd_attribute_change(
                world,
                request.entity,
                "focalLength",
                "float",
                format!("{focal_length}"),
            );
            apply_usd_attribute_change(
                world,
                request.entity,
                "clippingRange",
                "float2",
                format!("({}, {})", request.near, request.far),
            );
        }
    });
}

pub(crate) fn on_usd_attribute_edit_requested(
    trigger: On<UsdAttributeEditRequested>,
    mut commands: Commands,
) {
    let request = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        apply_usd_attribute_change(
            world,
            request.entity,
            &request.name,
            &request.type_name,
            request.value,
        );
    });
}

pub(crate) fn on_usd_variant_edit_requested(
    trigger: On<UsdVariantEditRequested>,
    mut commands: Commands,
) {
    let request = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        apply_usd_variant_selection(
            world,
            request.entity,
            request.prim_path,
            request.set,
            request.variant,
        );
    });
}

pub(crate) fn on_mount_snap_requested(trigger: On<MountSnapRequested>, mut commands: Commands) {
    let request = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let ops = lunco_usd::attach::realign_component_ops(
            LayerId::root(),
            request.part,
            request.joint,
            request.placement,
            request.rotate,
        );
        apply_usd_ops(world, request.entity, ops);
    });
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn on_attach_at_socket_requested(
    trigger: On<AttachAtSocketRequested>,
    mut commands: Commands,
) {
    let request = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        attach_component_at_socket(
            world,
            request.entity,
            request.host_path,
            request.name,
            request.asset,
            request.accepts,
            request.joint,
            request.socket_frame,
        );
    });
}

pub(crate) fn on_shader_swap_requested(trigger: On<ShaderSwapRequested>, mut commands: Commands) {
    let request = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        swap_shader_on_entity(world, request.part, &request.path);
    });
}

pub(crate) fn on_shader_create_requested(
    trigger: On<ShaderCreateRequested>,
    mut commands: Commands,
) {
    let request = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        create_and_apply(world, request.part, &request.name, &request.template);
    });
}

pub(crate) fn on_shader_import_requested(
    trigger: On<ShaderImportRequested>,
    mut commands: Commands,
) {
    let request = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        import_and_apply(world, request.part, &request.source);
    });
}

pub(crate) fn on_shader_parameters_requested(
    trigger: On<ShaderParametersRequested>,
    mut commands: Commands,
) {
    let request = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        if let Some(mut look) = world.get_mut::<ShaderLook>(request.entity) {
            for (name, value) in &request.edits {
                look.values.insert(name.clone(), *value);
            }
        }
        if request.usd_prim_exists {
            for (name, value) in &request.edits {
                let usd_name = if name.starts_with("primvars:") {
                    name.clone()
                } else {
                    format!("primvars:{name}")
                };
                let (type_name, value_str) = match value {
                    ParamValue::F32(x) => ("float", format!("{x:.3}")),
                    ParamValue::I32(x) => ("int", format!("{x}")),
                    ParamValue::U32(x) => ("uint", format!("{x}")),
                    ParamValue::Vec2(arr) => ("float2", format!("({}, {})", arr[0], arr[1])),
                    ParamValue::Vec3(arr) => {
                        let name_lc = name.to_lowercase();
                        let ty = if name_lc.contains("color") || name_lc.contains("colour") {
                            "color3f"
                        } else {
                            "float3"
                        };
                        (ty, format!("({}, {}, {})", arr[0], arr[1], arr[2]))
                    }
                    ParamValue::Vec4(arr) => (
                        "float4",
                        format!("({}, {}, {}, {})", arr[0], arr[1], arr[2], arr[3]),
                    ),
                };
                apply_usd_attribute_change(world, request.entity, &usd_name, type_name, value_str);
            }
        }
    });
}

pub(crate) fn on_pbr_material_requested(trigger: On<PbrMaterialRequested>, mut commands: Commands) {
    let request = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        for entity in &request.parts {
            let Some(mut look) = world.get_mut::<PbrLook>(*entity) else {
                continue;
            };
            look.base_color = LinearRgba::new(
                request.base[0],
                request.base[1],
                request.base[2],
                request.alpha,
            );
            look.emissive = LinearRgba::new(
                request.emissive[0],
                request.emissive[1],
                request.emissive[2],
                1.0,
            );
            look.metallic = request.metallic;
            look.perceptual_roughness = request.roughness;
            look.ior = request.ior;
            look.double_sided = request.double_sided;
            look.alpha = if request.alpha >= 1.0 {
                lunco_render::SurfaceAlpha::Opaque
            } else {
                lunco_render::SurfaceAlpha::Blend
            };
        }

        let Some(prim) = world.get::<UsdPrimPath>(request.part).cloned() else {
            return;
        };
        let shader_path = bound_shader_prim(world, &prim);
        let (mut ops, shader) = match &shader_path {
            Some(path) => (Vec::new(), path.clone()),
            None => {
                let schemas = geom_api_schemas(world, &prim);
                let Some((ops, shader)) =
                    lunco_usd::material::ensure_preview_surface_ops(&prim.path, &schemas)
                else {
                    return;
                };
                (ops, shader)
            }
        };
        let fresh = shader_path.is_none();
        let root = LayerId::root();
        let mut set = |name: &str, type_name: &str, value: String| {
            ops.push(UsdOp::SetAttribute {
                edit_target: root.clone(),
                path: shader.clone(),
                name: name.to_string(),
                type_name: type_name.to_string(),
                value,
            });
        };
        if request.base_changed || fresh {
            set(
                "inputs:diffuseColor",
                "color3f",
                format!(
                    "({}, {}, {})",
                    request.base[0], request.base[1], request.base[2]
                ),
            );
        }
        if request.emissive_changed || fresh {
            set(
                "inputs:emissiveColor",
                "color3f",
                format!(
                    "({}, {}, {})",
                    request.emissive[0], request.emissive[1], request.emissive[2]
                ),
            );
        }
        if request.metallic_changed || fresh {
            set(
                "inputs:metallic",
                "float",
                format!("{:.3}", request.metallic),
            );
        }
        if request.roughness_changed || fresh {
            set(
                "inputs:roughness",
                "float",
                format!("{:.3}", request.roughness),
            );
        }
        if request.ior_changed || fresh {
            set("inputs:ior", "float", format!("{:.3}", request.ior));
        }
        if let Some(doc) = resolve_doc_for_entity(world, request.part) {
            lunco_usd::commands::apply_ops_as_change_set(world, doc, "Edit material", ops);
        }
    });
}

pub(crate) fn on_modelica_parameter_requested(
    trigger: On<ModelicaParameterRequested>,
    mut commands: Commands,
) {
    let request = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        use lunco_modelica::document::ModelicaOp;
        use lunco_modelica::state::ModelicaDocumentRegistry;
        use lunco_modelica::ui::panels::canvas_diagram::apply_ops_public;
        use lunco_modelica::{ModelicaChannels, ModelicaCommand, ModelicaModel};

        let mut session_id = 0u64;
        let mut model_name = String::new();
        if let Some(mut model) = world.get_mut::<ModelicaModel>(request.entity) {
            if let Some(slot) = model.parameters.get_mut(&request.key) {
                *slot = request.value;
            }
            model.session_id += 1;
            session_id = model.session_id;
            model.is_stepping = true;
            model_name = model.model_name.clone();
        }

        let (doc_id, class_name) = {
            let registry = world.resource::<ModelicaDocumentRegistry>();
            let doc = registry.document_of(request.entity);
            let class = doc.and_then(|doc| registry.host(doc)).and_then(|host| {
                lunco_modelica::ast_extract::extract_model_name_from_ast(
                    host.document().syntax().ast(),
                )
            });
            (doc, class)
        };
        let (Some(doc_id), Some(class_name)) = (doc_id, class_name) else {
            return;
        };
        apply_ops_public(
            world,
            doc_id,
            vec![ModelicaOp::SetParameter {
                class: class_name,
                component: request.key.clone(),
                param: String::new(),
                value: format!("{}", request.value),
            }],
        );
        let new_source = world
            .resource::<ModelicaDocumentRegistry>()
            .host(doc_id)
            .map(|host| host.document().source().to_string());
        if let (Some(new_source), Some(channels)) =
            (new_source, world.get_resource::<ModelicaChannels>())
        {
            let _ = channels.tx.send(ModelicaCommand::UpdateParameters {
                entity: request.entity,
                session_id,
                model_name,
                source: new_source,
            });
        }
    });
}

// ─────────────────────────────────────────────────────────────────────
// View-model (WP-8) — query-derived inspector state.
// ─────────────────────────────────────────────────────────────────────

/// Live scene-sun readout for the Environment section.
#[derive(Clone)]
pub struct SunReadout {
    pub name: String,
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub illuminance: f32,
    pub shadow_maps_enabled: bool,
    pub rgb: [f32; 3],
    pub shadow_first: Option<f32>,
    pub shadow_max: Option<f32>,
}

/// Live joint readout for the selected entity's `angle` port.
#[derive(Clone, Copy)]
pub struct JointReadout {
    pub holder: Entity,
    pub measured: f64,
    pub commanded: f64,
    pub wired: bool,
}

/// Change-driven view-model for the Inspector (WP-8). The Environment,
/// Camera, and Joint sections read query-derived world state that
/// [`PanelCtx`] deliberately can't gather during paint (no `query`, no
/// `&World`); [`populate_inspector_view`] flattens it here each frame and
/// the panel reads it via `ctx.resource`.
#[derive(Resource, Default)]
pub struct InspectorView {
    /// The primary selection used to derive the joint readout.
    pub selected: Option<Entity>,
    /// The unique authored scene sun, if the scene has one.
    pub sun: Option<SunReadout>,
    /// Global ambient brightness, if the resource exists.
    pub ambient_brightness: Option<f32>,
    /// Earthshine fill-light illuminance, if present.
    pub earthshine_lux: Option<f32>,
    /// Active presentation camera's exposure EV100, if any.
    pub exposure_ev100: Option<f32>,
    /// Active presentation camera's bloom intensity, if any.
    pub bloom_intensity: Option<f32>,
    /// Joint readout for the primary-selected entity, if it drives one.
    pub joint: Option<JointReadout>,
}

/// Producer for [`InspectorView`]. Exclusive (needs `&mut World` for the
/// scans + `joint_angle_holder`); runs in `Update` before the egui pass,
/// gated by [`inspector_inputs_changed`] so the world scans are skipped on
/// a quiescent scene. All reads are bounded single-entity lookups or small
/// scans the panel used to do in-paint.
pub fn populate_inspector_view(world: &mut World) {
    use bevy::camera::visibility::RenderLayers;
    use bevy::camera::Exposure;
    use bevy::light::{CascadeShadowConfig, DirectionalLight, GlobalAmbientLight};
    use bevy::post_process::bloom::Bloom;

    // ── Scene sun (skip preview / earthshine lights, same rule as the
    // horizon system's pick_sun).
    let sun_entity = world
        .query_filtered::<Entity, (
            With<DirectionalLight>,
            Without<RenderLayers>,
            Without<lunco_environment::Earthshine>,
        )>()
        .single(world)
        .ok();
    let sun = sun_entity.map(|e| {
        let name = world
            .get::<Name>(e)
            .map(|n| n.as_str().to_string())
            .unwrap_or_default();
        let (yaw_deg, pitch_deg) = world
            .get::<Transform>(e)
            .map(|tf| {
                let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
                (yaw.to_degrees(), pitch.to_degrees())
            })
            .unwrap_or((0.0, 0.0));
        let (illuminance, shadow_maps_enabled, rgb) = world
            .get::<DirectionalLight>(e)
            .map(|l| {
                let lin = l.color.to_linear();
                (
                    l.illuminance,
                    l.shadow_maps_enabled,
                    [lin.red, lin.green, lin.blue],
                )
            })
            .unwrap_or((0.0, false, [1.0, 1.0, 1.0]));
        let (shadow_first, shadow_max) = world
            .get::<CascadeShadowConfig>(e)
            .map(|cfg| {
                (
                    Some(cfg.bounds.first().copied().unwrap_or(40.0)),
                    Some(cfg.bounds.last().copied().unwrap_or(1500.0)),
                )
            })
            .unwrap_or((None, None));
        SunReadout {
            name,
            yaw_deg,
            pitch_deg,
            illuminance,
            shadow_maps_enabled,
            rgb,
            shadow_first,
            shadow_max,
        }
    });

    let ambient_brightness = world
        .get_resource::<GlobalAmbientLight>()
        .map(|a| a.brightness);
    let earthshine_lux = world
        .query_filtered::<&DirectionalLight, With<lunco_environment::Earthshine>>()
        .single(world)
        .ok()
        .map(|l| l.illuminance);

    // ── Camera.
    let active_camera = world
        .get_resource::<lunco_core::SceneViewport>()
        .and_then(|viewport| viewport.active_camera);
    let exposure_ev100 = active_camera
        .and_then(|entity| world.get::<Exposure>(entity))
        .map(|exposure| exposure.ev100);
    let bloom_intensity = active_camera
        .and_then(|entity| world.get::<Bloom>(entity))
        .map(|bloom| bloom.intensity);

    // ── Joint for the primary-selected entity.
    let selected = world
        .get_resource::<SelectedEntities>()
        .and_then(|s| s.primary());
    let joint = if let Some(entity) = selected {
        if let Some(holder) = joint_angle_holder(world, entity) {
            let registry = world.resource::<PortRegistry>().clone();
            let measured = registry
                .read_output_port(world, holder, JOINT_ANGLE_PORT)
                .unwrap_or(0.0);
            let commanded = registry
                .read_input_port(world, holder, JOINT_ANGLE_PORT)
                .unwrap_or(0.0);
            let mut cq = world.query::<&lunco_cosim::SimConnection>();
            let wired = cq
                .iter(world)
                .any(|c| c.end_element == holder && c.end_connector == JOINT_ANGLE_PORT);
            Some(JointReadout {
                holder,
                measured,
                commanded,
                wired,
            })
        } else {
            None
        }
    } else {
        None
    };

    let mut view = world.resource_mut::<InspectorView>();
    view.selected = selected;
    view.sun = sun;
    view.ambient_brightness = ambient_brightness;
    view.earthshine_lux = earthshine_lux;
    view.exposure_ev100 = exposure_ev100;
    view.bloom_intensity = bloom_intensity;
    view.joint = joint;
}

/// Run condition for [`populate_inspector_view`]: skip the world scans on a
/// quiescent scene, the way the sibling [`super::entity_list::populate_entity_tree_view`]
/// gates on [`super::entity_list::scene_topology_changed`]. Runs when any
/// readout the Inspector shows could have changed — the selection moved, the
/// scene sun / camera exposure / bloom / ambient was edited, or a directional
/// light was removed (despawn) — and keeps running every frame while a joint
/// readout is live (`view.joint.is_some()`) so the measured angle stays fresh
/// during a sim. The `Local` flag forces one initial build (a freshly-added
/// system does not see pre-existing entities as `Changed`). On an idle scene
/// with nothing selected this returns `false` and every scan is skipped.
/// ⚠ **VALUE COMPARISON, NOT `Changed<…>`** — and that is forced, not stylistic.
///
/// This gate used to ask `Changed<Transform>` on the sun. It fired on 296 of 300
/// frames, i.e. it gated nothing while the Inspector paid for a full world scan
/// every frame. Celestial transforms are projection state and can legitimately
/// change as the shared epoch advances, so a component change tick is not the
/// same thing as a changed inspector value. The gate compares the displayed
/// values directly instead.
///
/// So the gate compares the handful of scalars the view actually holds against
/// the world's current values: a few single-entity component reads, versus the
/// producer's `&mut World` scans. When the sun's aim moves by a ULP and the
/// rendered readout would print the same degrees, nothing runs. Selection and
/// ambient use the same value comparison; their Bevy change ticks are not an
/// input because unrelated systems can borrow those resources while leaving
/// the displayed value unchanged.
pub(crate) fn inspector_inputs_changed(
    mut first: Local<bool>,
    mut joint_poll: Local<f32>,
    time: Res<Time>,
    view: Res<InspectorView>,
    selection: Res<lunco_scene_commands::SelectedEntities>,
    ambient: Option<Res<bevy::light::GlobalAmbientLight>>,
    // The SAME sun the producer reads (non-preview, non-fill), so the comparison
    // is against the value that would land in the view.
    viewport: Option<Res<lunco_core::SceneViewport>>,
    lights: Query<
        (
            &Transform,
            &bevy::light::DirectionalLight,
            Option<&bevy::light::CascadeShadowConfig>,
        ),
        (
            Without<lunco_environment::Earthshine>,
            Without<bevy::camera::visibility::RenderLayers>,
        ),
    >,
    exposures: Query<(Entity, &bevy::camera::Exposure)>,
    blooms: Query<(Entity, &bevy::post_process::bloom::Bloom)>,
    mut removed_lights: RemovedComponents<bevy::light::DirectionalLight>,
) -> bool {
    use bevy::math::EulerRot;

    // Drain the removal buffer every frame (so it doesn't accumulate) and note
    // whether a directional light despawned since last frame.
    let removed = removed_lights.read().count() > 0;

    // The readout is printed to a tenth of a degree / whole lux, so compare at
    // that resolution: a difference the panel cannot show is not a reason to
    // rebuild the view.
    let sun_moved = {
        let live = lights.single().ok().map(|(tf, light, cascades)| {
            let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
            let lin = light.color.to_linear();
            (
                yaw.to_degrees(),
                pitch.to_degrees(),
                light.illuminance,
                light.shadow_maps_enabled,
                cascades.map(|c| {
                    (
                        c.bounds.first().copied().unwrap_or(40.0),
                        c.bounds.last().copied().unwrap_or(1500.0),
                    )
                }),
                [lin.red, lin.green, lin.blue],
            )
        });
        match (&view.sun, live) {
            (None, None) => false,
            (Some(cached), Some((yaw, pitch, lux, shadows, shadow_bounds, rgb))) => {
                let shadow_changed = match ((cached.shadow_first, cached.shadow_max), shadow_bounds)
                {
                    ((None, None), None) => false,
                    ((Some(cached_first), Some(cached_max)), Some((live_first, live_max))) => {
                        (cached_first - live_first).abs() > 1.0e-3
                            || (cached_max - live_max).abs() > 1.0e-3
                    }
                    _ => true,
                };
                (cached.yaw_deg - yaw).abs() > 0.05
                    || (cached.pitch_deg - pitch).abs() > 0.05
                    || (cached.illuminance - lux).abs() > 1.0
                    || cached.shadow_maps_enabled != shadows
                    || shadow_changed
                    || cached
                        .rgb
                        .iter()
                        .zip(rgb)
                        .any(|(cached, live)| (cached - live).abs() > 1.0e-4)
            }
            // Appeared or disappeared — the view is stale either way.
            _ => true,
        }
    };

    let camera_changed = {
        let active_camera = viewport.as_deref().and_then(|vp| vp.active_camera);
        let live_ev = active_camera.and_then(|entity| {
            exposures
                .get(entity)
                .ok()
                .map(|(_, exposure)| exposure.ev100)
        });
        let live_bloom = active_camera
            .and_then(|entity| blooms.get(entity).ok().map(|(_, bloom)| bloom.intensity));
        let ev_moved = match (view.exposure_ev100, live_ev) {
            (Some(a), Some(b)) => (a - b).abs() > 1.0e-3,
            (None, None) => false,
            _ => true,
        };
        let bloom_moved = match (view.bloom_intensity, live_bloom) {
            (Some(a), Some(b)) => (a - b).abs() > 1.0e-4,
            (None, None) => false,
            _ => true,
        };
        ev_moved || bloom_moved
    };
    let viewport_changed = viewport.as_ref().is_some_and(|vp| vp.is_changed());

    // A joint's measured angle is a continuously changing Avian value, but the
    // Inspector is a human readout rather than a telemetry oscilloscope. Poll
    // it at 10 Hz so the producer remains genuinely gated while the displayed
    // value stays responsive. The old `view.joint.is_some()` clause made the
    // supposedly change-driven system unconditional for every selected joint.
    *joint_poll += time.delta_secs();
    let joint_due = view.joint.is_some() && *joint_poll >= 0.1;
    if joint_due {
        *joint_poll = 0.0;
    }

    let selection_changed = view.selected != selection.primary();
    let ambient_changed = match (
        view.ambient_brightness,
        ambient.as_ref().map(|a| a.brightness),
    ) {
        (None, None) => false,
        (Some(cached), Some(live)) => (cached - live).abs() > 1.0e-4,
        _ => true,
    };

    let run = !*first
        || selection_changed
        || ambient_changed
        || sun_moved
        || camera_changed
        || viewport_changed
        || removed
        || joint_due;
    *first = true;
    run
}

/// Inspector panel — editable entity parameters.
pub struct Inspector;

impl Panel for Inspector {
    fn id(&self) -> PanelId {
        PanelId("sandbox_inspector")
    }
    fn title(&self) -> String {
        "Inspector".into()
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::RightInspector
    }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Scene
    }
    fn transparent_background(&self) -> bool {
        true
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        ctx.panel_content_frame().show(ui, |ui| {
            // The Inspector stacks many sections (Environment, Transform,
            // Physics, Wheel, Shader, Material, Modelica) and can exceed the
            // panel height — scroll so the lower sections stay reachable.
            egui::ScrollArea::vertical()
                .auto_shrink([false, true])
                .show(ui, |ui| inspector_content(self, ui, ctx));
        });
    }
}

/// Environment panel — global scene environment, lighting, terrain, LOD & obstacle field settings.
pub struct EnvironmentPanel;

impl Panel for EnvironmentPanel {
    fn id(&self) -> PanelId {
        PanelId("sandbox_environment")
    }
    fn title(&self) -> String {
        "Environment".into()
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::RightInspector
    }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Scene
    }
    fn transparent_background(&self) -> bool {
        true
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        ctx.panel_content_frame().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, true])
                .show(ui, |ui| environment_panel_content(self, ui, ctx));
        });
    }
}

fn environment_panel_content(_panel: &mut EnvironmentPanel, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    ui.heading("Environment");

    // ── Environment (sun + ambient) ──────────────────────────────
    egui::CollapsingHeader::new("Sun & Ambient")
        .default_open(true)
        .show(ui, |ui| environment_section(ui, ctx));
    ui.separator();

    // ── Animation transport (play/pause/scrub/rate) ──────────────
    egui::CollapsingHeader::new("Animation")
        .default_open(true)
        .show(ui, |ui| animation_transport_section(ui, ctx));
    ui.separator();

    // ── Camera (exposure + post-process) ─────────────────────────
    egui::CollapsingHeader::new("Camera")
        .default_open(true)
        .show(ui, |ui| camera_section(ui, ctx));
    ui.separator();

    // ── Terrain Overlay (slope-hazard analysis VIEW) ─────────────
    egui::CollapsingHeader::new("Terrain Overlay")
        .default_open(true)
        .show(ui, |ui| terrain_overlay_section(ui, ctx));
    ui.separator();

    // ── Obstacle Field (procedural craters + rocks) ──────────────
    egui::CollapsingHeader::new("Obstacle Field (Craters & Rocks)")
        .default_open(true)
        .show(ui, |ui| obstacle_field_section(ui, ctx));
}

/// Delete `entity` from the scene — the single delete path for both the Del
/// hotkey and the Delete button.
///
/// Authors the removal into the active document's runtime layer FIRST (so the
/// delete is a journaled, undoable, networked document op — the editor keeps no
/// private history), then performs the live despawn for immediate feedback and
/// drops it from the selection. A non-document entity (a palette spawn the doc
/// doesn't own) simply isn't authored — it just despawns.
/// NOTE: there is no local `delete_entity` helper any more. It did the same three things
/// the typed `commands::DeleteEntity` verb does (author the `RemovePrim`, despawn, drop
/// the selection), so it was a second delete path that the command bus — and hence the
/// API, the journal and networked peers — never saw. The Inspector triggers the command.
/// Delete the selected entity through the same typed command as the Inspector
/// button. The shortcut comes from `UserIntent::DeleteSelection`, so it remains
/// rebindable and never fires while an egui field or cursor tool owns input.
pub fn delete_selected_on_intent(
    delete: lunco_core::DeleteSelectionIntent,
    cursor_mode: lunco_core::CursorModeActive,
    selected: Res<SelectedEntities>,
    mut commands: Commands,
) {
    if !delete.just_pressed() || cursor_mode.any() {
        return;
    }
    if let Some(target) = selected.primary() {
        commands.trigger(crate::commands::DeleteEntity {
            target,
            intent: lunco_core::EditIntent::Persistent,
        });
    }
}

fn inspector_content(_panel: &mut Inspector, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    // Esc / Backspace deselection lives in the Bevy `handle_entity_selection`
    // system (the single mutation path), not here.

    ui.heading("Inspector");

    // Get current selection
    let Some(entity) = ctx.resource::<SelectedEntities>().and_then(|s| s.primary()) else {
        ui.label("No entity selected.");
        ui.label("Press Shift+Left-click on an object to select it.");
        return;
    };

    ui.label(format!("ID: {entity:?}"));

    // Name (read-only)
    if let Some(name) = ctx.get::<Name>(entity).map(|n| n.as_str().to_string()) {
        ui.label(format!("Name: {name}"));
    }

    if let Some(projection) = ctx.get::<Projection>(entity).cloned() {
        camera_projection_section(ui, ctx, entity, projection);
    }

    ui.separator();

    // ── Comms & Orbit (doc 43): geodetic anchor / Kepler orbit /
    //    antenna params + live link state ─────────────────────────
    comms_orbit_section(ui, ctx, entity);

    // ── USD parameters: data-driven bounded sliders for attributes that
    //    author a `customData {min,max,unit}` UI hint. ────────────────
    usd_parameters_section(ui, ctx, entity);

    // ── Variants: which configuration this prim composes with — a rover's
    //    drivetrain, a scenario scene's terrain site. ─────────────────
    usd_variants_section(ui, ctx, entity);

    // ── Mount: snap an attached part onto the socket it declares, re-deriving
    //    its placement + joint anchor from the mount frames (doc 48 §3.1). ──
    mount_section(ui, ctx, entity);

    // ── Transform component ──────────────────────────────────────
    // The sliders author a **document op**, they do not poke ECS: a committed
    // edit fires `MoveEntity`, whose observers both move the body (physics
    // seat + kinematic pulse — the old hand-copied CQ-510 block, now in ONE
    // place) and author `UsdOp::SetTranslate` into the runtime layer. So an
    // Inspector move survives reload, journals, syncs, and is undone by the
    // same Ctrl+Z as a gizmo drag. Committed = drag released or value typed —
    // per-frame firing during a drag would push one op per frame.
    if ctx.get::<Transform>(entity).is_some() {
        egui::CollapsingHeader::new("Transform")
            .default_open(true)
            .show(ui, |ui| {
                // GRID-ABSOLUTE, not `Transform.translation`: on a
                // grid-direct prim the raw local is only the cell
                // remainder, so the sliders showed a number that agreed with
                // neither the authored USD nor the object's actual place —
                // and committing it fed that short value to `MoveEntity`,
                // teleporting the object one cell. This is the same frame
                // the gizmo authors and the same one `MoveEntity` expects.
                if let Some(t) = grid_absolute_of(ctx, entity).map(|p| p.as_vec3()) {
                    let (mut x, mut y, mut z) = (t.x, t.y, t.z);
                    // `DragValue`, not a ±1000 `Slider`: a grid-absolute
                    // coordinate is unbounded (a moonbase prim sits well
                    // outside ±1000 m of the grid origin), and a slider would
                    // CLAMP it — merely showing the panel and nudging one axis
                    // would have hauled the object back inside the range.
                    let rx = ui.add(egui::DragValue::new(&mut x).speed(0.1).prefix("X: "));
                    let ry = ui.add(egui::DragValue::new(&mut y).speed(0.1).prefix("Y: "));
                    let rz = ui.add(egui::DragValue::new(&mut z).speed(0.1).prefix("Z: "));
                    // Author ONCE, on release — not on every `changed()` frame, which
                    // would flood the journal with an op per mouse-move for a single
                    // drag. Same rule as the gizmo's drag-end authoring.
                    let committed = [&rx, &ry, &rz]
                        .iter()
                        .any(|r| r.drag_stopped() || (r.changed() && !r.dragged()));
                    if committed {
                        let new_t = Vec3::new(x, y, z);
                        // Route through the typed `MoveEntity` verb; it owns
                        // physics pose seating and USD persistence.
                        let Some(gid) = ctx.get::<lunco_core::GlobalEntityId>(entity).copied()
                        else {
                            warn!("INSPECTOR: {entity:?} has no GlobalEntityId — not movable");
                            ctx.trigger(lunco_core::TelemetryEvent {
                                name: "inspector-move-failed".to_string(),
                                source: 0,
                                severity: lunco_core::Severity::Error,
                                data: lunco_core::TelemetryValue::String(
                                    "The selected object has no stable entity identity and cannot be moved"
                                        .to_string(),
                                ),
                                timestamp: 0.0,
                            });
                            return;
                        };
                        ctx.trigger(crate::commands::MoveEntity {
                            entity_id: gid.get(),
                            translation: new_t.to_array().map(f64::from),
                        });
                    }
                }
            });
    }

    // ── Physics component ────────────────────────────────────────
    let has_physics = ctx.get::<avian3d::prelude::RigidBody>(entity).is_some()
        || ctx.get::<avian3d::prelude::Mass>(entity).is_some()
        || ctx.get::<avian3d::prelude::LinearDamping>(entity).is_some()
        || ctx
            .get::<avian3d::prelude::AngularDamping>(entity)
            .is_some();
    if has_physics {
        egui::CollapsingHeader::new("Physics")
            .default_open(false)
            .show(ui, |ui| {
                if let Some(rb) = ctx
                    .get::<avian3d::prelude::RigidBody>(entity)
                    .map(|rb| format!("{rb:?}"))
                {
                    ui.label(format!("Type: {rb}"));
                }
                if let Some(cur) = ctx.get::<avian3d::prelude::Mass>(entity).map(|c| c.0) {
                    let mut m = cur;
                    if ui
                        .add(
                            egui::Slider::new(&mut m, 0.1..=100000.0)
                                .text("Mass (kg)")
                                .logarithmic(true),
                        )
                        .changed()
                    {
                        ctx.trigger(InspectorComponentEdit::Mass { entity, value: m });
                    }
                }
                if let Some(cur) = ctx
                    .get::<avian3d::prelude::LinearDamping>(entity)
                    .map(|c| c.0 as f32)
                {
                    let mut d = cur;
                    if ui
                        .add(egui::Slider::new(&mut d, 0.0..=10.0).text("Linear Damping"))
                        .changed()
                    {
                        ctx.trigger(InspectorComponentEdit::LinearDamping {
                            entity,
                            value: d as f64,
                        });
                    }
                }
                if let Some(cur) = ctx
                    .get::<avian3d::prelude::AngularDamping>(entity)
                    .map(|c| c.0 as f32)
                {
                    let mut d = cur;
                    if ui
                        .add(egui::Slider::new(&mut d, 0.0..=10.0).text("Angular Damping"))
                        .changed()
                    {
                        ctx.trigger(InspectorComponentEdit::AngularDamping {
                            entity,
                            value: d as f64,
                        });
                    }
                }
            });
    }

    // Wheel + suspension dynamics have NO hand-coded sections here: they are
    // USD-authored (`lunco:wheel:*`, `lunco:suspension:*`, `physxVehicle*`)
    // and surface as derived sliders via `usd_parameters_section` (customData
    // UI hints). Edits go through `ApplyUsdOp` and re-derive the spawned
    // components in place (`lunco_usd_sim::wheel_params::resync_wheels_for_stage`)
    // — the direct-ECS sliders that used to live here bypassed the document,
    // so their edits neither persisted, journaled, nor replicated, and the
    // resync would now overwrite them on the next document change.

    // ── Materials ────────────────────────────────────────────────
    let parts = editable_parts(ctx, entity);
    if !parts.is_empty() {
        let stored = ctx
            .resource::<crate::InspectorTarget>()
            .and_then(|t| t.part)
            .filter(|p| parts.iter().any(|(e, _)| e == p));
        let mut target = stored.or_else(|| default_part(ctx, &parts));
        if stored.is_none() {
            if let Some(t) = target {
                ctx.resource_scope::<crate::InspectorTarget, _>(|_, target| {
                    target.part = Some(t);
                });
            }
        }
        // Multi-part object → a dropdown to switch parts (may retarget).
        if parts.len() > 1 {
            target = parts_selector(ui, ctx, &parts, target);
        }

        if let Some(part) = target {
            shader_picker_for_part(ui, ctx, part);
            shader_tools_ui(ui, ctx, part);

            // One subtree pass yields both the shader holder and the
            // distinct PBR material handles (CQ-204: was two independent
            // `subtree` walks of the same part — `first_shader_holder` +
            // `collect_std_handles`).
            let (pbr_parts, shader_holder) = part_materials(ctx, part);
            if let Some(holder) = shader_holder {
                egui::CollapsingHeader::new("Shader Parameters")
                    .default_open(true)
                    .show(ui, |ui| {
                        shader_parameters_section(ui, ctx, holder);
                    });
            }
            if !pbr_parts.is_empty() {
                egui::CollapsingHeader::new("Material (PBR)")
                    .default_open(true)
                    .show(ui, |ui| {
                        material_pbr_section(ui, ctx, part, &pbr_parts);
                    });
            }
        }
    }

    // ── Terrain shader mode (streamed DEM terrain) ──────────────
    if let Some(mode) = ctx
        .get::<lunco_terrain_surface::TerrainShaderMode>(entity)
        .copied()
    {
        use lunco_terrain_surface::TerrainShaderMode as M;
        egui::CollapsingHeader::new("Terrain Shader")
            .default_open(true)
            .show(ui, |ui| {
                let label = |m: M| match m {
                    M::Lit => "Lit (regolith)",
                    M::DebugLod => "Debug LOD (colours)",
                    M::Plain => "Plain (no shader)",
                };
                let mut sel = mode;
                egui::ComboBox::from_label("Mode")
                    .selected_text(label(sel))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut sel, M::Lit, label(M::Lit));
                        ui.selectable_value(&mut sel, M::DebugLod, label(M::DebugLod));
                        ui.selectable_value(&mut sel, M::Plain, label(M::Plain));
                    });
                if sel != mode {
                    ctx.trigger(InspectorComponentEdit::TerrainShader { entity, mode: sel });
                }
            });
    }

    // ── Modelica parameters component ───────────────────────────
    let has_modelica = ctx.get::<lunco_modelica::ModelicaModel>(entity).is_some();
    if has_modelica {
        egui::CollapsingHeader::new("Modelica Parameters")
            .default_open(true)
            .show(ui, |ui| {
                modelica_parameters_section(ui, ctx, entity);
            });
    }

    // ── Joint control ───────────────────────────────────────────
    let joint = ctx.resource::<InspectorView>().and_then(|v| v.joint);
    if let Some(j) = joint {
        egui::CollapsingHeader::new("Joint")
            .default_open(true)
            .show(ui, |ui| {
                joint_control_section(ui, ctx, j);
            });
    }

    // Delete button
    ui.separator();
    if ui.button("Delete Entity (Del)").clicked() {
        ctx.trigger(crate::commands::DeleteEntity {
            target: entity,
            intent: lunco_core::EditIntent::Persistent,
        });
    }
}

/// Edit the selected camera's live projection. `Projection` is the shared
/// camera contract: USD import, the free-flight camera, and the render binder
/// all use it, so the Inspector does not maintain a second FOV or clipping
/// state. The render pipeline never replaces this component.
fn camera_projection_section(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    entity: Entity,
    projection: Projection,
) {
    let Projection::Perspective(mut perspective) = projection else {
        return;
    };
    egui::CollapsingHeader::new("Camera Projection")
        .default_open(true)
        .show(ui, |ui| {
            let mut fov_deg = perspective.fov.to_degrees();
            let mut near = perspective.near;
            let mut far = perspective.far;
            let fov_changed = ui
                .add(egui::Slider::new(&mut fov_deg, 1.0..=179.0).text("Vertical FOV"))
                .changed();
            let near_changed = ui
                .add(egui::DragValue::new(&mut near).speed(0.01).prefix("Near: "))
                .changed();
            let far_changed = ui
                .add(egui::DragValue::new(&mut far).speed(100.0).prefix("Far: "))
                .changed();
            if !(fov_changed || near_changed || far_changed) {
                return;
            }
            perspective.fov = fov_deg.to_radians();
            perspective.near = near.max(0.001);
            perspective.far = far.max(perspective.near + 0.001);
            let authored_fov = perspective.fov;
            let authored_near = perspective.near;
            let authored_far = perspective.far;
            ctx.trigger(ProjectionEditRequested {
                entity,
                projection: Projection::Perspective(perspective),
                fov: authored_fov,
                near: authored_near,
                far: authored_far,
            });
        });
}

/// Live sun + ambient controls. Reads the change-driven [`InspectorView`]
/// snapshot and dispatches every edit through a single
/// Bounded sliders for the selected prim's `customData`-ranged attributes,
/// from the [`UsdParamView`](crate::ui::usd_params::UsdParamView) view-model. An
/// asset that authors `customData {min,max,unit}` on a scalar gets a clamped
/// slider here without any hand-coded range; edits write back through the same
/// `ApplyUsdOp(SetAttribute)` path as every other Inspector control.
/// Grid-absolute translation of `entity` — `cell × edge + local`, the frame USD
/// authors `xformOp:translate` in and the frame `MoveEntity` takes.
///
/// The `PanelCtx` (one-component-at-a-time) spelling of
/// [`lunco_core::coords::grid_absolute`], which needs `Query`s the panel doesn't
/// have. Same rule: no parent `Grid` ⇒ no cell ⇒ the local translation already
/// IS the authored value.
fn grid_absolute_of(ctx: &PanelCtx, entity: Entity) -> Option<bevy::math::DVec3> {
    let tf = ctx.get::<Transform>(entity)?;
    let Some(grid) = ctx
        .get::<ChildOf>(entity)
        .and_then(|c| ctx.get::<big_space::prelude::Grid>(c.parent()))
    else {
        return Some(tf.translation.as_dvec3());
    };
    let cell = ctx
        .get::<big_space::prelude::CellCoord>(entity)
        .copied()
        .unwrap_or_default();
    Some(grid.grid_position_double(&cell, tf))
}

fn usd_parameters_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, entity: Entity) {
    // The producer aims the view at the DRILLED prim-backed part when one is
    // set (see `produce_usd_param_view`), else at the primary. Render whenever
    // the view belongs to this inspector context — primary or its drill.
    let part = ctx
        .resource::<crate::InspectorTarget>()
        .and_then(|t| t.part);
    let (target, params): (Entity, Vec<crate::ui::usd_params::UsdParam>) =
        match ctx.resource::<crate::ui::usd_params::UsdParamView>() {
            Some(v)
                if !v.params.is_empty()
                    && (v.entity == Some(entity) || (v.entity.is_some() && v.entity == part)) =>
            {
                (v.entity.unwrap(), v.params.clone())
            }
            _ => return,
        };
    egui::CollapsingHeader::new("Parameters")
        .default_open(true)
        .show(ui, |ui| {
            // Breadcrumb when drilled into a subpart: name the part being
            // edited and offer the way back to the whole vehicle.
            if target != entity {
                ui.horizontal(|ui| {
                    let part_name = ctx
                        .get::<Name>(target)
                        .map(|n| n.as_str().to_string())
                        .unwrap_or_else(|| format!("{target:?}"));
                    ui.label(egui::RichText::new(format!("part: {part_name}")).italics());
                    if ui.small_button("⏶ back to root").clicked() {
                        ctx.resource_scope::<crate::InspectorTarget, _>(|_, target| {
                            target.part = None;
                        });
                    }
                });
                ui.separator();
            }
            let mut edits: Vec<(String, String, String)> = Vec::new();
            for p in &params {
                let mut v = p.value;
                let text = if p.unit.is_empty() {
                    p.label.clone()
                } else {
                    format!("{} ({})", p.label, p.unit)
                };
                if ui
                    .add(egui::Slider::new(&mut v, p.min..=p.max).text(text))
                    .changed()
                {
                    edits.push((p.name.clone(), p.type_name.clone(), format!("{v}")));
                }
            }
            for (name, type_name, value) in edits {
                ctx.trigger(UsdAttributeEditRequested {
                    entity: target,
                    name,
                    type_name,
                    value,
                });
            }
        });
}

/// The ⎇ Variants section — one row per variant set the selected prim ships,
/// from the [`UsdVariantView`](crate::ui::usd_variants::UsdVariantView)
/// view-model. Picking an option dispatches
/// [`UsdOp::SetVariantSelection`](lunco_usd::document::UsdOp), so it journals,
/// replicates and undoes like every other authoring edit.
///
/// This is how a scenario scene switches which real lunar site it composes
/// with (`terrain`), and how a rover instance switches drivetrain — the same
/// control, because they are the same USD mechanism.
///
/// Selecting the option already composed is skipped: it would author an
/// identical opinion, and each dispatch costs a whole-subtree rebuild.
fn usd_variants_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, entity: Entity) {
    let (prim_path, sets) = match ctx.resource::<crate::ui::usd_variants::UsdVariantView>() {
        Some(v) if v.entity == Some(entity) && !v.sets.is_empty() => {
            (v.prim_path.clone(), v.sets.clone())
        }
        _ => return,
    };
    egui::CollapsingHeader::new("⎇ Variants")
        .default_open(true)
        .show(ui, |ui| {
            let mut picked: Vec<(String, String)> = Vec::new();
            for set in &sets {
                ui.horizontal_wrapped(|ui| {
                    ui.label(&set.name);
                    for opt in &set.options {
                        let active = set.selection.as_deref() == Some(opt.as_str());
                        if ui.selectable_label(active, opt).clicked() && !active {
                            picked.push((set.name.clone(), opt.clone()));
                        }
                    }
                });
                // A set whose selection does not resolve composes its fallback
                // (or nothing) — say so rather than showing an empty row that
                // looks like a rendering bug.
                if set.selection.is_none() {
                    ui.weak("no selection composed — fallback in use");
                }
            }
            for (set, variant) in picked {
                ctx.trigger(UsdVariantEditRequested {
                    entity,
                    prim_path: prim_path.clone(),
                    set,
                    variant,
                });
            }
        });
}

/// Map a socket's `lunco:mount:joint` token (+ optional axis) to the typed
/// [`AttachJoint`](lunco_usd::attach::AttachJoint) the attach lowering wants.
/// Unknown tokens are rejected; mount metadata must not silently become a
/// different joint.
#[cfg(not(target_arch = "wasm32"))]
fn attach_joint_from(joint: &str, axis: Option<&str>) -> Option<lunco_usd::attach::AttachJoint> {
    use lunco_usd::attach::{AttachJoint, Axis};
    match joint {
        "fixed" if axis.is_none() => Some(AttachJoint::Fixed),
        "revolute" => Some(AttachJoint::Revolute {
            axis: match axis? {
                "X" => Axis::X,
                "Y" => Axis::Y,
                "Z" => Axis::Z,
                _ => return None,
            },
        }),
        "prismatic" => Some(AttachJoint::Prismatic {
            axis: match axis? {
                "X" => Axis::X,
                "Y" => Axis::Y,
                "Z" => Axis::Z,
                _ => return None,
            },
        }),
        _ => None,
    }
}

/// The 🔩 Mount section — one row per socket the selected host advertises.
///
/// A socket **holding a part** offers **⟳ Snap**: re-author that part's transform
/// and joint anchor from the mount frames (`realign_component_ops`) so both follow
/// the socket. All frames are on the live stage.
///
/// An **empty socket** that names a default asset (`lunco:mount:asset`) offers
/// **⊕ Attach**: the *new-attach* flow — compose the not-yet-loaded asset, read its
/// plug frame, `from_mount` it onto the socket, and reference + joint it in via
/// `AttachComponent`.
///
/// Reads the pre-resolved [`UsdMountView`](crate::ui::usd_mount::UsdMountView) (the
/// socket frame math ran in the producer; it needs the `!Send` stage).
fn mount_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, entity: Entity) {
    let (host_path, items) = match ctx.resource::<crate::ui::usd_mount::UsdMountView>() {
        Some(v) if v.entity == Some(entity) && !v.items.is_empty() => {
            (v.host_path.clone(), v.items.clone())
        }
        _ => return,
    };

    egui::CollapsingHeader::new("Mount")
        .default_open(true)
        .show(ui, |ui| {
            let mut snap: Option<(String, String, [f64; 3], [f64; 3])> = None;
            // (asset, child name, host, accepted plug kind, joint token, axis, socket frame)
            let mut attach: Option<(
                String,
                String,
                String,
                String,
                String,
                Option<String>,
                Transform,
            )> = None;
            for item in &items {
                ui.horizontal(|ui| {
                    let joint = match &item.axis {
                        Some(ax) => format!("{} {}", item.joint, ax),
                        None => item.joint.clone(),
                    };
                    ui.label(format!("{} ({}, {joint})", item.socket, item.accepts));
                });
                match (
                    &item.part_path,
                    &item.part_leaf,
                    item.placement,
                    item.rotate_deg,
                ) {
                    (Some(part), Some(leaf), Some(placement), Some(rotate)) => {
                        ui.horizontal(|ui| {
                            let resp = ui
                                .add_enabled_ui(!item.aligned, |ui| {
                                    lunco_workbench::icon_text_button(
                                        ui,
                                        lunco_workbench::UiIcon::Refresh,
                                        &format!("Snap {leaf}"),
                                        "Align this part to its socket",
                                    )
                                })
                                .inner
                                .on_disabled_hover_text(
                                    "This part is already aligned to its socket",
                                );
                            if resp.clicked() {
                                snap = Some((
                                    part.clone(),
                                    item.joint_path.clone(),
                                    placement,
                                    rotate,
                                ));
                            }
                            if item.aligned {
                                ui.weak("aligned");
                            } else {
                                ui.weak(format!(
                                    "→ ({:.2}, {:.2}, {:.2})",
                                    placement[0], placement[1], placement[2]
                                ));
                            }
                        });
                    }
                    // Empty socket with a suggested asset → offer a new-attach.
                    _ => match &item.attach_asset {
                        Some(asset) => {
                            let leaf = asset.rsplit('/').next().unwrap_or(asset);
                            ui.horizontal(|ui| {
                                #[cfg(not(target_arch = "wasm32"))]
                                if ui.button(format!("⊕ Attach {leaf}")).clicked() {
                                    attach = Some((
                                        asset.clone(),
                                        item.socket.clone(),
                                        host_path.clone(),
                                        item.accepts.clone(),
                                        item.joint.clone(),
                                        item.axis.clone(),
                                        item.socket_frame,
                                    ));
                                }
                                #[cfg(target_arch = "wasm32")]
                                ui.add_enabled(
                                    false,
                                    egui::Button::new(format!("⊕ Attach {leaf} (native only)")),
                                );
                                ui.weak("empty");
                            });
                        }
                        None => {
                            ui.weak("  (empty)");
                        }
                    },
                }
            }
            if let Some((part, joint, placement, rotate)) = snap {
                ctx.trigger(MountSnapRequested {
                    entity,
                    part,
                    joint,
                    placement,
                    rotate,
                });
            }
            #[cfg(not(target_arch = "wasm32"))]
            if let Some((asset, name, host, accepts, joint_tok, axis, socket_frame)) = attach {
                if let Some(joint) = attach_joint_from(&joint_tok, axis.as_deref()) {
                    ctx.trigger(AttachAtSocketRequested {
                        entity,
                        host_path: host,
                        name,
                        asset,
                        accepts,
                        joint,
                        socket_frame,
                    });
                } else {
                    warn!("mount socket `{name}` has invalid joint metadata; attach skipped");
                }
            }
            #[cfg(target_arch = "wasm32")]
            let _ = &attach;
        });
}

/// Perform a new-attach: read the asset's plug frame off its (not-yet-loaded) file,
/// `from_mount` it onto `socket_frame`, and dispatch [`AttachComponent`] to the
/// host's document. The typed request observer performs the asset composition
/// work after the egui pass (a one-shot click, never per frame). Native-only.
#[cfg(not(target_arch = "wasm32"))]
fn attach_component_at_socket(
    world: &mut World,
    entity: Entity,
    host_path: String,
    name: String,
    asset: String,
    accepts: String,
    joint: lunco_usd::attach::AttachJoint,
    socket_frame: Transform,
) {
    use lunco_usd::attach::AttachSpec;
    // Ask `lunco-assets` where the reference lives — do NOT assume the shipped
    // library. A component authored by an open Twin is `twin://<name>/…`, which
    // has no path under `assets/` at all; joining one produced a path that never
    // existed and the attach was skipped with a "no plug frame" warning.
    let Some(schemes) = world
        .get_resource::<lunco_assets::SchemeRegistry>()
        .cloned()
    else {
        report_inspector_error(
            world,
            format!("Asset scheme registry is unavailable for mount asset `{asset}`"),
        );
        return;
    };
    let fs_path = match schemes.local_path(&asset) {
        Ok(Some(path)) => path,
        Ok(None) => {
            report_inspector_error(
                world,
                format!("Mount asset `{asset}` resolves to no local file; attach was skipped"),
            );
            return;
        }
        Err(error) => {
            report_inspector_error(
                world,
                format!("Asset scheme registry unavailable for `{asset}`: {error}"),
            );
            return;
        }
    };
    let Some(plug) = lunco_usd_bevy::mount::read_asset_plug(&fs_path) else {
        report_inspector_error(
            world,
            format!(
                "Mount asset `{asset}` ({}) has no plug frame; attach was skipped",
                fs_path.display()
            ),
        );
        return;
    };
    if plug.kind != accepts {
        report_inspector_error(
            world,
            format!(
                "Mount asset `{asset}` advertises plug `{}` but socket accepts `{accepts}`; attach was skipped",
                plug.kind
            ),
        );
        return;
    }
    let Some(doc) = resolve_doc_for_entity(world, entity) else {
        report_inspector_error(world, "Selected mount host is not document-backed");
        return;
    };
    let spec = AttachSpec::from_mount(
        LayerId::root(),
        host_path,
        name,
        asset,
        joint,
        socket_frame,
        plug.frame,
    );
    world.trigger(lunco_usd::commands::AttachComponent { doc, spec });
}

/// [`SetEnvironmentLight`](lunco_environment::SetEnvironmentLight) command
/// — the same mutation path the HTTP/MCP API uses.
/// Animation transport for the USD animation-preview domain (doc 19 — T7).
/// Reads the singleton [`lunco_time::AnimationPreview`]'s [`lunco_time::Playback`]
/// and drives it through the [`lunco_time::ControlAnimation`] command (the same
/// authority the API/MCP use), so play/pause/scrub/rate touch only animation —
/// never the physics clock.
fn animation_transport_section(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    use lunco_time::{AnimationPreview, ControlAnimation, Playback, TransportMode};

    let Some(domain) = ctx.resource::<AnimationPreview>().map(|p| p.domain) else {
        ui.label("Animation spine not active.");
        return;
    };
    let Some(pb) = ctx.get::<Playback>(domain).copied() else {
        ui.label("No animation timeline yet.");
        return;
    };
    let playing = matches!(pb.mode, TransportMode::Playing);

    ui.horizontal(|ui| {
        let (icon, label, tooltip) = if playing {
            (
                lunco_workbench::UiIcon::Pause,
                "Pause",
                "Pause the animation preview",
            )
        } else {
            (
                lunco_workbench::UiIcon::Play,
                "Play",
                "Play the animation preview",
            )
        };
        if lunco_workbench::icon_text_button(ui, icon, label, tooltip).clicked() {
            ctx.trigger(ControlAnimation {
                playing: Some(!playing),
                ..Default::default()
            });
        }
        if lunco_workbench::icon_text_button(
            ui,
            lunco_workbench::UiIcon::Back,
            "Rewind",
            "Rewind the animation preview",
        )
        .clicked()
        {
            ctx.trigger(ControlAnimation {
                seek_secs: Some(0.0),
                ..Default::default()
            });
        }
    });

    // Scrub the playhead (seconds) over the bound clips' authored span (set by
    // `bind_animated_to_preview`); fall back to a default window when no clip has
    // bound yet (so the bar is still usable). Pausing first lets the slider hold.
    let range = if pb.bounded() {
        pb.start..=pb.end
    } else {
        0.0..=120.0
    };
    let mut head = pb.head;
    if ui
        .add(egui::Slider::new(&mut head, range).text("Time (s)"))
        .changed()
    {
        ctx.trigger(ControlAnimation {
            seek_secs: Some(head),
            ..Default::default()
        });
    }

    // Playback rate (1× = realtime). 0 freezes without changing the play flag.
    let mut rate = pb.rate;
    if ui
        .add(egui::Slider::new(&mut rate, 0.0..=10.0).text("Rate ×"))
        .changed()
    {
        ctx.trigger(ControlAnimation {
            rate: Some(rate),
            ..Default::default()
        });
    }

    ui.label("Animation only — the physics clock is the toolbar pause control.");
}

fn environment_section(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    use lunco_environment::SetEnvironmentLight;

    let sun = ctx.resource::<InspectorView>().and_then(|v| v.sun.clone());
    let ambient = ctx
        .resource::<InspectorView>()
        .and_then(|v| v.ambient_brightness);
    let earthshine = ctx
        .resource::<InspectorView>()
        .and_then(|v| v.earthshine_lux);
    if sun.is_none() && ambient.is_none() && earthshine.is_none() {
        return;
    }

    let mut cmd = SetEnvironmentLight::default();
    let mut any_change = false;

    egui::CollapsingHeader::new("Environment")
        .default_open(true)
        .show(ui, |ui| {
            if let Some(s) = &sun {
                if !s.name.is_empty() {
                    ui.label(egui::RichText::new(&s.name).strong());
                }

                let mut yaw_deg = s.yaw_deg;
                let mut pitch_deg = s.pitch_deg;
                let yaw_changed = ui
                    .add(egui::Slider::new(&mut yaw_deg, -180.0..=180.0).text("Yaw (°)"))
                    .changed();
                let pitch_changed = ui
                    .add(egui::Slider::new(&mut pitch_deg, -90.0..=90.0).text("Pitch (°)"))
                    .changed();
                if yaw_changed {
                    cmd.sun_yaw = Some(yaw_deg.to_radians());
                }
                if pitch_changed {
                    cmd.sun_pitch = Some(pitch_deg.to_radians());
                }
                any_change |= yaw_changed || pitch_changed;

                let mut lux = s.illuminance;
                let mut shadows = s.shadow_maps_enabled;
                let mut rgb = s.rgb;
                if ui
                    .add(
                        egui::Slider::new(&mut lux, 100.0..=200_000.0)
                            .text("Illuminance (lx)")
                            .logarithmic(true),
                    )
                    .changed()
                {
                    cmd.illuminance = Some(lux);
                    any_change = true;
                }
                ui.horizontal(|ui| {
                    if ui.color_edit_button_rgb(&mut rgb).changed() {
                        cmd.sun_color = Some(rgb);
                        any_change = true;
                    }
                    ui.label("Color");
                });
                if ui.checkbox(&mut shadows, "Cast shadows").changed() {
                    cmd.shadow_maps_enabled = Some(shadows);
                    any_change = true;
                }

                if let (Some(f0), Some(m0)) = (s.shadow_first, s.shadow_max) {
                    let mut first = f0;
                    let mut max = m0;
                    if ui
                        .add(
                            egui::Slider::new(&mut first, 5.0..=200.0)
                                .text("Near shadow bound (m)")
                                .logarithmic(true),
                        )
                        .changed()
                    {
                        cmd.shadow_first_cascade_bound = Some(first);
                        any_change = true;
                    }
                    if ui
                        .add(
                            egui::Slider::new(&mut max, 50.0..=5000.0)
                                .text("Shadow max distance (m)")
                                .logarithmic(true),
                        )
                        .changed()
                    {
                        cmd.shadow_max_distance = Some(max);
                        any_change = true;
                    }
                }
                ui.separator();
            }

            if let Some(b0) = ambient {
                let mut b = b0;
                if ui
                    .add(egui::Slider::new(&mut b, 0.0..=400.0).text("Ambient (cd/m²)"))
                    .changed()
                {
                    cmd.ambient_brightness = Some(b);
                    any_change = true;
                }
            }

            // READOUT, not a knob. Earthshine brightness is derived from
            // Earth's phase (`lunco_environment::drive_earthshine_from_phase`),
            // which is its only writer — a slider here would be overwritten in
            // the same frame it was dragged. What moves it is the sim clock.
            if let Some(lux) = earthshine {
                ui.label(format!(
                    "Earthshine: {lux:.1} lx ({:.0}% of full Earth)",
                    100.0 * lux / lunco_environment::FULL_EARTH_EARTHSHINE_LUX
                ));
            }
        });

    if any_change {
        ctx.trigger(cmd);
    }
}

/// Camera section — physical exposure and bloom. Reads the
/// [`InspectorView`] snapshot; mutates via the same
/// [`SetEnvironmentLight`](lunco_environment::SetEnvironmentLight) command.
fn camera_section(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    use lunco_environment::SetEnvironmentLight;

    let exposure = ctx
        .resource::<InspectorView>()
        .and_then(|v| v.exposure_ev100);
    let bloom = ctx
        .resource::<InspectorView>()
        .and_then(|v| v.bloom_intensity);

    let mut cmd = SetEnvironmentLight::default();
    let mut any_change = false;

    if let Some(ev0) = exposure {
        let mut ev = ev0;
        if ui
            .add(egui::Slider::new(&mut ev, 5.0..=18.0).text("Exposure (EV100)"))
            .changed()
        {
            cmd.exposure_ev100 = Some(ev);
            any_change = true;
        }
    } else {
        ui.label("No camera Exposure component.");
    }

    if let Some(i0) = bloom {
        let mut i = i0;
        if ui
            .add(egui::Slider::new(&mut i, 0.0..=1.0).text("Bloom intensity"))
            .changed()
        {
            cmd.bloom_intensity = Some(i);
            any_change = true;
        }
    }

    if any_change {
        ctx.trigger(cmd);
    }
}

/// Terrain analysis-overlay controls — slope hazard or LOD depth, the render VIEW
/// of the terrain. Edits the global `TerrainOverlayParams`; the tile shader colourises live
/// (no re-bake). Read a Copy, edit locally, write back ONLY on a real change so the
/// live-sync system stays change-driven instead of firing every frame.
fn terrain_overlay_section(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    use lunco_terrain_surface::overlay::TerrainOverlayParams;
    let Some(cur) = ctx.resource::<TerrainOverlayParams>().copied() else {
        ui.label("No streaming terrain in this scene.");
        return;
    };
    let mut p = cur;
    ui.checkbox(&mut p.enabled, "Analysis overlay")
        .on_hover_text("Composite an analysis view over the lit terrain.");
    ui.add_enabled_ui(p.enabled, |ui| {
        ui.checkbox(&mut p.lod_depth, "LOD depth view")
            .on_hover_text(
                "Colour tiles by quadtree depth (cycling palette) instead of slope — \
                 the streaming diagnostic, composited over the production look.",
            );
        ui.add(egui::Slider::new(&mut p.opacity, 0.0..=1.0).text("Opacity"));
        if !p.lod_depth {
            ui.add(egui::Slider::new(&mut p.safe_deg, 0.0..=45.0).text("Safe ≤ (°)"))
                .on_hover_text("Slopes at/below this stay green.");
            ui.add(egui::Slider::new(&mut p.cliff_deg, 0.0..=45.0).text("Cliff ≥ (°)"))
                .on_hover_text(
                    "The critical angle: slopes at/above this go red. Tunes live, no re-bake.",
                );
            // Keep the band ordered so the ramp never inverts.
            if p.safe_deg > p.cliff_deg {
                p.safe_deg = p.cliff_deg;
            }
            draw_slope_legend(ui, p.safe_deg, p.cliff_deg);
        }
    })
    .response
    .on_disabled_hover_text("Enable Analysis overlay to edit its visualization");
    if p != cur {
        ctx.resource_scope(|_c, r: &mut TerrainOverlayParams| *r = p);
    }
}

/// A green→amber→red gradient bar over slope angle `[0°, 45°]`, coloured by the SAME
/// transfer the shader runs (`TransferFn::SlopeHazard` — the one Transfer plane, see
/// `docs/architecture/terrain-layered-rendering.md`), so the legend swatch and the
/// terrain pixel agree by construction. Tick marks at the safe/cliff angles.
fn draw_slope_legend(ui: &mut egui::Ui, safe_deg: f32, cliff_deg: f32) {
    use lunco_terrain_surface::TransferFn;
    const MAX_DEG: f32 = 45.0;
    let hazard = TransferFn::SlopeHazard {
        safe_rad: safe_deg.to_radians(),
        cliff_rad: cliff_deg.to_radians(),
    };
    let w = ui.available_width().min(240.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 16.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let n = 96usize;
    for i in 0..n {
        let t = i as f32 / n as f32;
        let deg = t * MAX_DEG;
        let c = hazard.sample(deg.to_radians());
        let col = egui::Color32::from_rgb(
            (c[0] * 255.0) as u8,
            (c[1] * 255.0) as u8,
            (c[2] * 255.0) as u8,
        );
        let x0 = rect.left() + rect.width() * t;
        let x1 = rect.left() + rect.width() * ((i + 1) as f32 / n as f32);
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom())),
            0.0,
            col,
        );
    }
    // Tick marks at the two critical angles.
    for deg in [safe_deg, cliff_deg] {
        let x = rect.left() + rect.width() * (deg / MAX_DEG).clamp(0.0, 1.0);
        let sample = hazard.sample(deg.to_radians());
        let luminance = 0.2126 * sample[0] + 0.7152 * sample[1] + 0.0722 * sample[2];
        let tick = if luminance > 0.45 {
            egui::Color32::BLACK
        } else {
            egui::Color32::WHITE
        };
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(1.0, tick),
        );
    }
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("0°").weak().small());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(format!("{MAX_DEG:.0}°")).weak().small());
        });
    });
    ui.label(
        egui::RichText::new(format!(
            "safe ≤ {safe_deg:.0}°   ·   cliff ≥ {cliff_deg:.0}°"
        ))
        .weak()
        .small(),
    );
}

fn obstacle_field_section(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    let mut regen_spec: Option<ObstacleFieldSpec> = None;

    let had = ctx.resource_scope(|_ctx, spec: &mut ObstacleFieldSpec| {
        let mut regen = false;

        ui.horizontal(|ui| {
            ui.label(format!("Seed {:#x}", spec.seed));
            if ui.button("Reseed").clicked() {
                spec.seed = spec
                    .seed
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    .wrapping_add(0x2545_F491_4F6C_DD1D);
                regen = true;
            }
        });

        if ui
            .add(egui::Slider::new(&mut spec.region_half_extent, 50.0..=500.0).text("Region ½ (m)"))
            .drag_stopped()
        {
            regen = true;
        }
        if ui
            .add(egui::Slider::new(&mut spec.grid_resolution, 65u32..=513).text("Grid res"))
            .drag_stopped()
        {
            regen = true;
        }

        // Spatial pattern.
        let mut kind = match spec.pattern {
            Pattern::Uniform => 0usize,
            Pattern::PoissonDisk { .. } => 1,
            Pattern::Clustered { .. } => 2,
        };
        egui::ComboBox::from_label("Pattern")
            .selected_text(["Uniform", "Poisson disk", "Clustered"][kind])
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut kind, 0, "Uniform");
                ui.selectable_value(&mut kind, 1, "Poisson disk");
                ui.selectable_value(&mut kind, 2, "Clustered");
            });
        let chosen = match kind {
            0 => Pattern::Uniform,
            1 => match spec.pattern {
                p @ Pattern::PoissonDisk { .. } => p,
                _ => Pattern::PoissonDisk { min_spacing: 3.0 },
            },
            _ => match spec.pattern {
                p @ Pattern::Clustered { .. } => p,
                _ => Pattern::Clustered {
                    clusters: 8,
                    spread: 25.0,
                },
            },
        };
        if std::mem::discriminant(&spec.pattern) != std::mem::discriminant(&chosen) {
            spec.pattern = chosen;
            regen = true;
        }
        match &mut spec.pattern {
            Pattern::PoissonDisk { min_spacing } => {
                if ui
                    .add(egui::Slider::new(min_spacing, 0.5..=10.0).text("Min spacing (m)"))
                    .drag_stopped()
                {
                    regen = true;
                }
            }
            Pattern::Clustered { clusters, spread } => {
                if ui
                    .add(egui::Slider::new(clusters, 1u32..=32).text("Clusters"))
                    .drag_stopped()
                {
                    regen = true;
                }
                if ui
                    .add(egui::Slider::new(spread, 2.0..=80.0).text("Spread (m)"))
                    .drag_stopped()
                {
                    regen = true;
                }
            }
            Pattern::Uniform => {}
        }

        egui::CollapsingHeader::new("Craters")
            .default_open(true)
            .show(ui, |ui| {
                let s = &mut *spec;
                if ui
                    .checkbox(&mut s.craters.enabled, "Enabled (including crater detail)")
                    .changed()
                {
                    regen = true;
                }
                for (val, range, label) in [
                    (&mut s.craters.density, 0.0..=60.0, "Density /ha"),
                    (&mut s.craters.depth_ratio, 0.0..=0.8, "Depth ratio"),
                    (
                        &mut s.craters.rim_height_ratio,
                        0.0..=1.5,
                        "Wall height ratio",
                    ),
                    (&mut s.craters.size.min, 0.5..=20.0, "Radius min"),
                    (&mut s.craters.size.mode, 0.5..=20.0, "Radius mode"),
                    (&mut s.craters.size.max, 0.5..=40.0, "Radius max"),
                ] {
                    if ui
                        .add(egui::Slider::new(val, range).text(label))
                        .drag_stopped()
                    {
                        regen = true;
                    }
                }
                // Keep the size distribution valid: min ≤ mode ≤ max. If the sliders
                // invert (e.g. min > mode) the log-normal sampler clamps EVERY crater to
                // the high end → a dense field of oversized overlapping basins + rims that
                // reads as jagged spike noise from altitude (the "craters look worse").
                s.craters.size.min = s.craters.size.min.min(s.craters.size.mode);
                s.craters.size.max = s.craters.size.max.max(s.craters.size.mode);
            });

        egui::CollapsingHeader::new("Rocks")
            .default_open(true)
            .show(ui, |ui| {
                let s = &mut *spec;
                if ui.checkbox(&mut s.rocks.enabled, "Enabled").changed() {
                    regen = true;
                }
                for (val, range, label) in [
                    (&mut s.rocks.density, 0.0..=400.0, "Density /ha"),
                    (&mut s.rocks.size.min, 0.05..=5.0, "Radius min"),
                    (&mut s.rocks.size.mode, 0.05..=5.0, "Radius mode"),
                    (&mut s.rocks.size.max, 0.05..=8.0, "Radius max"),
                    (&mut s.rocks.dynamic_fraction, 0.0..=1.0, "Dynamic frac"),
                ] {
                    if ui
                        .add(egui::Slider::new(val, range).text(label))
                        .drag_stopped()
                    {
                        regen = true;
                    }
                }
                // Same validity clamp as craters: min ≤ mode ≤ max.
                s.rocks.size.min = s.rocks.size.min.min(s.rocks.size.mode);
                s.rocks.size.max = s.rocks.size.max.max(s.rocks.size.mode);
            });

        ui.separator();
        if ui.button("♻ Regenerate").clicked() {
            regen = true;
        }
        ui.label(
            egui::RichText::new("Field rebuilds on slider release.")
                .small()
                .weak(),
        );

        if regen {
            regen_spec = Some(spec.clone());
        }
    });

    if had.is_none() {
        ui.label("Obstacle field plugin not active.");
        return;
    }

    if let Some(spec) = regen_spec {
        ctx.trigger(UpdateObstacleFieldSpec { spec });
    }
}

/// The selected entity plus all of its descendants (subtree walk via
/// [`PanelCtx::get`]).
fn subtree(ctx: &PanelCtx, root: Entity) -> Vec<Entity> {
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        let e = out[i];
        i += 1;
        if let Some(children) = ctx.get::<Children>(e) {
            out.extend(children.iter());
        }
    }
    out
}

/// Joint control over a revolute joint's `angle` port. Reads the
/// [`InspectorView`] snapshot; the setpoint write is emitted through
/// [`lunco_cosim::write_port`].
fn joint_control_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, j: JointReadout) {
    let measured = j.measured;
    let mut commanded = j.commanded;
    let holder = j.holder;

    ui.label(format!(
        "measured: {:.3} rad  ({:.1}°)",
        measured,
        measured.to_degrees()
    ));

    let r = ui.add(
        egui::Slider::new(&mut commanded, -std::f64::consts::PI..=std::f64::consts::PI)
            .text("setpoint (rad)"),
    );
    ui.label(format!("{:.1}°", commanded.to_degrees()));
    if r.changed() {
        ctx.trigger(InspectorComponentEdit::JointSetpoint {
            holder,
            value: commanded,
        });
    }
    if j.wired {
        ui.label(
            egui::RichText::new("Driven by a wire — setpoint is transient")
                .small()
                .weak(),
        );
    }
}

/// Walk `root`'s subtree once, returning its PBR-surface **entities** and the
/// first [`ShaderLook`]-bearing entity. Replaces the former
/// `collect_std_handles` + `first_shader_holder`, which each ran an
/// independent `subtree` walk of the same root (CQ-204).
///
/// Surfaces are addressed by ENTITY and classified by their appearance **intent**
/// ([`PbrLook`] / [`ShaderLook`]), never by a bound material: the material is
/// derived from the intent (`lunco-render-bevy` re-binds on `Changed<…Look>`), it is
/// *shared* across every entity with the same look — so an in-place asset write would
/// bleed onto all of them — and naming it would drag `bevy_pbr` into this crate.
fn part_materials(ctx: &PanelCtx, root: Entity) -> (Vec<Entity>, Option<Entity>) {
    let mut parts: Vec<Entity> = Vec::new();
    let mut shader_holder: Option<Entity> = None;
    for e in subtree(ctx, root) {
        if ctx.get::<PbrLook>(e).is_some() {
            parts.push(e);
        }
        if shader_holder.is_none() && ctx.get::<ShaderLook>(e).is_some() {
            shader_holder = Some(e);
        }
    }
    (parts, shader_holder)
}

/// Material-bearing parts of `root`'s subtree, each labelled by its leaf name.
fn editable_parts(ctx: &PanelCtx, root: Entity) -> Vec<(Entity, String)> {
    let ents = subtree(ctx, root);
    let mut out = Vec::new();
    for e in ents {
        let has_shader = ctx.get::<ShaderLook>(e).is_some();
        let has_std = ctx.get::<PbrLook>(e).is_some();
        if has_shader || has_std {
            let label = ctx
                .get::<Name>(e)
                .map(|n| {
                    n.as_str()
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or(n.as_str())
                        .to_string()
                })
                .unwrap_or_else(|| format!("{e:?}"));
            out.push((e, label));
        }
    }
    out
}

/// Default part to edit: the first part WITHOUT a shader (the PBR body).
fn default_part(ctx: &PanelCtx, parts: &[(Entity, String)]) -> Option<Entity> {
    parts
        .iter()
        .map(|(e, _)| *e)
        .find(|e| ctx.get::<ShaderLook>(*e).is_none())
        .or_else(|| parts.first().map(|(e, _)| *e))
}

/// *Part* dropdown for a multi-part component. Writes the choice into
/// [`InspectorTarget`](crate::InspectorTarget) (through a scoped resource) and returns the
/// new target.
fn parts_selector(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    parts: &[(Entity, String)],
    current: Option<Entity>,
) -> Option<Entity> {
    let cur_label = current
        .and_then(|c| parts.iter().find(|(e, _)| *e == c).map(|(_, l)| l.clone()))
        .unwrap_or_else(|| "—".to_string());

    let mut chosen: Option<Entity> = None;
    egui::ComboBox::from_label("Part")
        .selected_text(cur_label)
        .show_ui(ui, |ui| {
            for (e, label) in parts {
                if ui.selectable_label(current == Some(*e), label).clicked() {
                    chosen = Some(*e);
                }
            }
        });
    if let Some(c) = chosen {
        ctx.resource_scope::<crate::InspectorTarget, _>(|_, target| {
            target.part = Some(c);
        });
        return Some(c);
    }
    current
}

/// Shader picker for a single part. Lists the [`ShaderCatalog`] entries and,
/// on pick, emits a typed `.wgsl` swap request for `part`.
fn shader_picker_for_part(ui: &mut egui::Ui, ctx: &mut PanelCtx, part: Entity) {
    let entries = ctx
        .resource::<lunco_materials::ShaderCatalog>()
        .map(|c| c.entries.clone())
        .unwrap_or_default();
    if entries.is_empty() {
        return;
    }
    let cur = current_shader_path(ctx, part).unwrap_or_default();
    let cur_label = entries
        .iter()
        .find(|e| e.path == cur)
        .map(|e| e.label.clone())
        .unwrap_or_else(|| "— (none)".to_string());

    let mut chosen: Option<String> = None;
    egui::ComboBox::from_label("Shader")
        .selected_text(cur_label)
        .show_ui(ui, |ui| {
            for e in &entries {
                if ui.selectable_label(e.path == cur, &e.label).clicked() {
                    chosen = Some(e.path.clone());
                }
            }
        });
    if let Some(path) = chosen {
        if path != cur {
            ctx.trigger(ShaderSwapRequested { part, path });
        }
    }
}

/// Point `part`'s [`ShaderLook`] at shader `path`, carrying over the params it
/// already had (the render binder swaps the material). Runs in the typed
/// request observer after the egui pass.
fn swap_shader_on_entity(world: &mut World, part: Entity, path: &str) {
    let Some(shader_prim) = bound_shader_prim_path(world, part) else {
        report_inspector_error(
            world,
            "This prim has no bound Material with a Shader child; the shader was not changed",
        );
        return;
    };
    let Some(doc) = resolve_doc_for_entity(world, part) else {
        report_inspector_error(world, "Selected shader target is not document-backed");
        return;
    };
    let mut look = world.get::<ShaderLook>(part).cloned().unwrap_or_default();
    look.shader = path.to_string();
    world
        .commands()
        .entity(part)
        // The `PbrLook` intent must go. Leaving it would have the PBR binder keep
        // re-inserting its own material alongside the shader one — two materials on
        // one mesh, drawn twice.
        .remove::<PbrLook>()
        .try_insert(look);
    // …and the material that binder ALREADY bound, or the same double-draw happens
    // once, statically. (Removed reflectively — this crate may not name `bevy_pbr`.)
    lunco_scene_commands::commands::drop_bound_pbr_material(world, part);

    // Propagate to USD — onto the `Shader` prim of the `Material` this geometry is
    // bound to. A shader is not a property of a mesh: it belongs to the material, and
    // the material is what the mesh binds. So the edit goes where the shader lives.
    //
    // The TYPE is the schema's, and writer and reader must agree on it, not just on the
    // name: an `asset` reads back as `Value::AssetPath`, and a loader asking for a
    // `String` gets `None`.
    world.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetAttribute {
            edit_target: LayerId::root(),
            path: shader_prim,
            name: "info:wgsl:sourceAsset".to_string(),
            type_name: "asset".to_string(),
            value: format!("@{}@", path),
        },
    });
}

/// The USD path of the `Shader` prim behind `part`'s bound material, if it has one:
/// `rel material:binding` → `Material` → `outputs:surface.connect` → `Shader`.
///
/// The same resolve the loader makes (`lunco_usd_bevy::resolve_bound_shader`), so
/// the Inspector edits exactly the prim the renderer read.
fn bound_shader_prim_path(world: &mut World, part: Entity) -> Option<String> {
    use lunco_usd_bevy::{CanonicalStages, SdfPath};

    let prim = world.get::<UsdPrimPath>(part)?.clone();
    let stage_id = prim.stage_handle.id();
    let sdf = SdfPath::new(&prim.path).ok()?;

    let canonical = world.get_non_send_mut::<CanonicalStages>()?;
    let cs = canonical.get(stage_id)?;
    let view = cs.view();

    lunco_usd_bevy::resolve_bound_shader(&view, &sdf).map(|p| p.to_string())
}

/// The asset path of `part`'s current shader (read via [`PanelCtx`]), or `None` if
/// it isn't using one. The path IS the intent — no material lookup.
fn current_shader_path(ctx: &PanelCtx, part: Entity) -> Option<String> {
    let look = ctx.get::<ShaderLook>(part)?;
    (!look.shader.is_empty()).then(|| look.shader.clone())
}

/// "Shader Tools" — GUI front-end for the live shader-authoring commands.
/// Create / Import apply the result to `part` by `Entity`. Commands are emitted
/// as typed events (their observers run via `world.trigger`).
fn shader_tools_ui(ui: &mut egui::Ui, ctx: &mut PanelCtx, part: Entity) {
    egui::CollapsingHeader::new("Shader Tools")
        .default_open(false)
        .show(ui, |ui| {
            let id = ui.make_persistent_id("shader_tools_state");
            #[derive(Clone, Default)]
            struct St {
                name: String,
                template: String,
                import: String,
            }
            let mut st: St = ui
                .memory_mut(|m| m.data.get_temp::<St>(id))
                .unwrap_or_default();
            if st.template.is_empty() {
                st.template = "solid".to_string();
            }

            // ── New from template ──
            ui.label("New shader from template:");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut st.name)
                        .hint_text("name")
                        .desired_width(110.0),
                );
                let cur_label = lunco_materials::shader_template_kinds()
                    .iter()
                    .find(|(k, _)| *k == st.template)
                    .map(|(_, l)| *l)
                    .unwrap_or("Solid");
                egui::ComboBox::from_id_salt("shader_template")
                    .selected_text(cur_label)
                    .show_ui(ui, |ui| {
                        for (k, l) in lunco_materials::shader_template_kinds() {
                            if ui.selectable_label(st.template == *k, *l).clicked() {
                                st.template = k.to_string();
                            }
                        }
                    });
            });
            if ui
                .add_enabled(
                    !st.name.trim().is_empty(),
                    egui::Button::new("Create & apply"),
                )
                .on_disabled_hover_text("Enter a shader name first")
                .clicked()
            {
                let name = st.name.clone();
                let template = st.template.clone();
                ctx.trigger(ShaderCreateRequested {
                    part,
                    name,
                    template,
                });
                st.name.clear();
            }

            ui.separator();
            // ── Import from disk ──
            ui.label("Import .wgsl from disk:");
            ui.add(
                egui::TextEdit::singleline(&mut st.import)
                    .hint_text("/path/to/shader.wgsl")
                    .desired_width(220.0),
            );
            if ui
                .add_enabled(
                    !st.import.trim().is_empty(),
                    egui::Button::new("Import & apply"),
                )
                .on_disabled_hover_text("Enter the path of a .wgsl file first")
                .clicked()
            {
                let src = st.import.trim().to_string();
                ctx.trigger(ShaderImportRequested { part, source: src });
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .button("Rescan twin folder")
                    .on_hover_text("Register any .wgsl dropped into the twin's shaders/ folder")
                    .clicked()
                {
                    ctx.trigger(crate::commands::RescanShaders {});
                }
                if let Some(path) = current_shader_path(ctx, part) {
                    if ui
                        .button("Delete current")
                        .on_hover_text(format!("Remove {path} (file + picker)"))
                        .clicked()
                    {
                        ctx.trigger(crate::commands::DeleteShader { path });
                    }
                }
            });

            ui.memory_mut(|m| m.data.insert_temp(id, st));
        });
}

/// Create a shader from `template` (registers it), then bind it to `part`.
fn create_and_apply(world: &mut World, part: Entity, name: &str, template: &str) {
    world.trigger(lunco_scene_commands::commands::CreateShader {
        name: name.to_string(),
        template: template.to_string(),
        source: String::new(),
        target: 0,
    });
    let stem = lunco_scene_commands::commands::sanitize_stem(name);
    apply_if_registered(world, part, &stem);
}

/// Import an external `.wgsl` (registers it), then bind it to `part`.
fn import_and_apply(world: &mut World, part: Entity, src_path: &str) {
    world.trigger(lunco_scene_commands::commands::ImportShader {
        source_path: src_path.to_string(),
        name: String::new(),
        target: 0,
    });
    let stem = std::path::Path::new(src_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(lunco_scene_commands::commands::sanitize_stem)
        .unwrap_or_default();
    if !stem.is_empty() {
        apply_if_registered(world, part, &stem);
    }
}

/// If a shader for `stem` is now registered, swap `part` onto it.
fn apply_if_registered(world: &mut World, part: Entity, stem: &str) {
    let shader_path = {
        let tr = world.get_resource::<lunco_assets::twin_source::TwinRoots>();
        lunco_scene_commands::commands::shader_asset_path_for(tr, stem)
    };
    let path = match shader_path {
        Ok(path) => path,
        Err(error) => {
            report_inspector_error(
                world,
                format!("Twin asset registry unavailable for shader `{stem}`: {error}"),
            );
            return;
        }
    };
    let registered = world
        .resource::<lunco_materials::ShaderCatalog>()
        .entries
        .iter()
        .any(|e| e.path == path);
    if registered {
        swap_shader_on_entity(world, part, &path);
    }
}

/// Editable PBR controls for the selected object's surfaces.
///
/// Edits the [`PbrLook`] **intent** component, not the material asset: the render
/// binder re-materialises on `Changed<PbrLook>`. This is not merely tidier — it is
/// required for correctness, because the binder shares one material across every
/// entity with the same look, so the old `Assets::get_mut(handle)` write would now
/// bleed onto unrelated entities that happen to look alike. (It is also what keeps
/// this crate off `bevy_pbr`.) A surface with no `PbrLook` is not listed as a part,
/// so there is nothing here to fall back to.
///
/// Reads a snapshot via [`PanelCtx`]; the component + USD writes are handled by
/// a typed observer.
fn material_pbr_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, part: Entity, parts: &[Entity]) {
    let Some(&first) = parts.first() else {
        return;
    };

    // Snapshot current values — no world borrow held while drawing widgets.
    let Some(look) = ctx.get::<PbrLook>(first) else {
        ui.label("Material still loading…");
        return;
    };
    let snap = {
        let b = look.base_color;
        let e = look.emissive;
        (
            [b.red, b.green, b.blue],
            b.alpha,
            [e.red, e.green, e.blue],
            look.metallic,
            look.perceptual_roughness,
            look.ior,
            look.double_sided,
        )
    };
    let (mut base, mut alpha, mut emissive, mut metallic, mut roughness, mut ior, mut double_sided) =
        snap;

    let mut changed = false;
    let mut base_changed = false;
    let mut emissive_changed = false;

    ui.horizontal(|ui| {
        let r = ui.color_edit_button_rgb(&mut base);
        changed |= r.changed();
        base_changed |= r.changed();
        ui.label("Base color");
    });
    let alpha_changed = ui
        .add(egui::Slider::new(&mut alpha, 0.0..=1.0).text("Alpha"))
        .changed();
    changed |= alpha_changed;
    base_changed |= alpha_changed;
    ui.horizontal(|ui| {
        let r = ui.color_edit_button_rgb(&mut emissive);
        changed |= r.changed();
        emissive_changed |= r.changed();
        ui.label("Emissive");
    });
    let metallic_changed = ui
        .add(egui::Slider::new(&mut metallic, 0.0..=1.0).text("Metallic"))
        .changed();
    changed |= metallic_changed;
    let roughness_changed = ui
        .add(egui::Slider::new(&mut roughness, 0.0..=1.0).text("Roughness"))
        .changed();
    changed |= roughness_changed;
    // Index of refraction — `UsdPreviewSurface`'s `inputs:ior`, and the ONLY specular
    // knob. This slider used to say "Reflectance" and author a private
    // `inputs:reflectance` that no other DCC reads; IOR is the standard spelling of the
    // same physical quantity. 1.0 = vacuum (no Fresnel), 1.5 = glass and most
    // silicates, 2.33 = where Bevy's derived reflectance saturates.
    //
    // There is no "Unlit" checkbox: `PbrLook::unlit` is render-only intent for overlay
    // geometry (trajectory lines, brush rings, labels) with no USD equivalent, so a
    // checkbox here could only edit a value that silently reverted on reload.
    let ior_changed = ui
        .add(egui::Slider::new(&mut ior, 1.0..=2.33).text("IOR"))
        .changed();
    changed |= ior_changed;
    changed |= ui.checkbox(&mut double_sided, "Double-sided").changed();
    if parts.len() > 1 {
        ui.label(egui::RichText::new(format!("applies to {} parts", parts.len())).weak());
    }

    if changed {
        ctx.trigger(PbrMaterialRequested {
            part,
            parts: parts.to_vec(),
            base,
            alpha,
            emissive,
            metallic,
            roughness,
            ior,
            double_sided,
            base_changed,
            emissive_changed,
            metallic_changed,
            roughness_changed,
            ior_changed,
        });
    }
}

/// Reflected schemas keyed by shader asset, so each loaded WGSL source is
/// parsed once — not once per frame while the shader section is open. The
/// `(ptr, len)` pair fingerprints the source `Cow`; a hot-reload swaps the
/// allocation, so the entry re-parses. This crate cannot name the render
/// side's `ShaderSchemas` cache (the Cargo.toml render gate), so it keeps
/// its own.
#[derive(Resource, Default)]
pub(crate) struct ShaderSchemaCache {
    #[allow(clippy::type_complexity)]
    map: std::collections::HashMap<
        bevy::asset::AssetId<bevy::shader::Shader>,
        (
            (usize, usize),
            Option<std::sync::Arc<lunco_materials::ParamSchema>>,
        ),
    >,
}

/// The reflected [`ParamSchema`](lunco_materials::ParamSchema) of a shader asset
/// path — parsed from the loaded WGSL source, so the editor derives its widgets from
/// the *shader* rather than from a material (or a hardcoded table).
///
/// `None` while the shader is still loading, or if it declares no `Material` struct.
fn shader_schema_of(
    ctx: &mut PanelCtx,
    path: &str,
) -> Option<std::sync::Arc<lunco_materials::ParamSchema>> {
    if path.is_empty() {
        return None;
    }
    let handle = ctx
        .resource::<AssetServer>()?
        .load::<bevy::shader::Shader>(path.to_string());
    let id = handle.id();
    let cached = ctx.resource_scope(|ctx, cache: &mut ShaderSchemaCache| {
        let shaders = ctx.resource::<Assets<bevy::shader::Shader>>()?;
        let src = match &shaders.get(id)?.source {
            bevy::shader::Source::Wgsl(s) => s.as_ref(),
            _ => return None,
        };
        let key = (src.as_ptr() as usize, src.len());
        if let Some((k, schema)) = cache.map.get(&id) {
            if *k == key {
                return schema.clone();
            }
        }
        let schema = lunco_materials::ParamSchema::parse(src).map(std::sync::Arc::new);
        cache.map.insert(id, (key, schema.clone()));
        schema
    });
    cached.unwrap_or_default()
}

/// Render named, range-bounded controls for the selected entity's [`ShaderLook`]
/// parameters.
///
/// The rows are DERIVED — from the shader's own `ParamSchema` (reflected out of its
/// WGSL `Material` struct), never from a hand-written list; the current values come
/// from `ShaderLook::values` (falling back to each field's declared default). An
/// edit mutates the component and the binder re-materialises on `Changed<ShaderLook>`,
/// so nothing here touches a material asset.
///
/// Reads a snapshot via [`PanelCtx`]; the component + USD writes are handled by
/// a typed observer.
fn shader_parameters_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, entity: Entity) {
    use lunco_materials::{ParamType, ParamValue, UiKind};

    let Some(look) = ctx.get::<ShaderLook>(entity).cloned() else {
        return;
    };

    struct Row {
        name: String,
        label: String,
        ui: UiKind,
        ty: ParamType,
        scalar: f32,
        int: i32,
        color: [f32; 3],
        /// The engine owns this value — show it, never let the user fight it.
        locked: bool,
    }
    let rows: Vec<Row> = {
        // The schema is a property of the ASSET: read it from the loaded WGSL
        // source. `None` = the shader hasn't loaded yet.
        let Some(schema) = shader_schema_of(ctx, &look.shader) else {
            ui.label("Shader still loading…");
            return;
        };
        schema
            .fields
            .iter()
            .map(|f| {
                // A parameter is ENGINE-OWNED when it is filled from simulation state
                // rather than authored — either annotated `//!@engine` in the WGSL
                // (Rust fills it, e.g. `sun_vis`) or currently driven by a USD
                // connection (a wire writes it every tick).
                //
                // `ShaderLook::live` IS that fact — it is the engine-owned half of the
                // look, the map that sits outside the material sharing key precisely
                // because the engine mutates it. So membership in `live` is the test;
                // there is no second registry of "which fields are driven" to keep in
                // step, and this covers every writer (a `.connect` wire and
                // `horizon_shade`'s engine fields alike) without naming any of them.
                let locked = matches!(f.ui, UiKind::Engine) || look.live.contains_key(&f.name);
                // Show the value the engine is actually pushing, not the authored one
                // it is overriding — a locked row displaying a stale `values` entry
                // would misreport the frame on screen.
                let floats = look
                    .live
                    .get(&f.name)
                    .or_else(|| look.values.get(&f.name))
                    .copied()
                    .or(f.default)
                    .map(|v| v.as_floats())
                    .unwrap_or_default();
                Row {
                    name: f.name.clone(),
                    label: f.label.clone(),
                    ui: f.ui.clone(),
                    ty: f.ty,
                    locked,
                    scalar: floats.first().copied().unwrap_or(0.0),
                    int: floats.first().copied().unwrap_or(0.0).round() as i32,
                    color: [
                        floats.first().copied().unwrap_or(0.5),
                        floats.get(1).copied().unwrap_or(0.5),
                        floats.get(2).copied().unwrap_or(0.5),
                    ],
                }
            })
            .collect()
    };

    if rows.is_empty() {
        ui.label("No editable parameters.");
        return;
    }

    let mut edits: Vec<(String, ParamValue)> = Vec::new();
    for mut row in rows {
        // An engine-owned parameter is SHOWN but not editable. Showing it is the
        // point — watching `load_frac` climb as the strut compresses is what makes
        // the wire legible. Editing it would be a lie: the next propagation tick
        // overwrites whatever the user typed, so an enabled control would read as a
        // broken slider rather than as a value under someone else's authority.
        let locked = row.locked;
        ui.add_enabled_ui(!locked, |ui| match row.ui {
            UiKind::Slider { min, max } => {
                if ui
                    .add(egui::Slider::new(&mut row.scalar, min..=max).text(&row.label))
                    .changed()
                {
                    edits.push((row.name, ParamValue::F32(row.scalar)));
                }
            }
            UiKind::Int { min, max } => {
                if ui
                    .add(egui::Slider::new(&mut row.int, min..=max).text(&row.label))
                    .changed()
                {
                    let v = match row.ty {
                        ParamType::U32 => ParamValue::U32(row.int.max(0) as u32),
                        ParamType::F32 => ParamValue::F32(row.int as f32),
                        _ => ParamValue::I32(row.int),
                    };
                    edits.push((row.name, v));
                }
            }
            UiKind::Color => {
                ui.horizontal(|ui| {
                    if ui.color_edit_button_rgb(&mut row.color).changed() {
                        let v = if row.ty == ParamType::Vec3 {
                            ParamValue::Vec3(row.color)
                        } else {
                            ParamValue::Vec4([row.color[0], row.color[1], row.color[2], 1.0])
                        };
                        edits.push((row.name, v));
                    }
                    ui.label(&row.label);
                });
            }
            UiKind::Free | UiKind::Engine => {
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::DragValue::new(&mut row.scalar).speed(0.01))
                        .changed()
                    {
                        edits.push((row.name, ParamValue::F32(row.scalar)));
                    }
                    ui.label(&row.label);
                });
            }
        })
        .response
        .on_disabled_hover_text(
            "Driven by the simulation — this value comes from a USD connection or \
             from the engine, so it cannot be edited here.",
        );
    }

    if !edits.is_empty() {
        let usd_prim_exists = ctx.get::<UsdPrimPath>(entity).is_some();
        ctx.trigger(ShaderParametersRequested {
            entity,
            edits,
            usd_prim_exists,
        });
    }
}

/// Render editable sliders for every tunable `parameter Real` in the
/// entity's Modelica model. Reads params via [`PanelCtx::get`]; the op
/// dispatch + recompile signal run in the typed request observer.
fn modelica_parameters_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, entity: Entity) {
    use lunco_modelica::ModelicaModel;

    // Snapshot the current params so we can render stable sliders.
    let params = match ctx.get::<ModelicaModel>(entity) {
        Some(m) => m.parameters.clone(),
        None => return,
    };
    if params.is_empty() {
        ui.label(
            egui::RichText::new("(no tunable parameters)")
                .weak()
                .small(),
        );
        return;
    }

    let mut keys: Vec<String> = params.keys().cloned().collect();
    keys.sort();

    let mut changed_pair: Option<(String, f64)> = None;
    for key in &keys {
        let current = params.get(key).copied().unwrap_or(0.0);
        let mut v = current;
        ui.horizontal(|ui| {
            ui.label(format!("{key:14}"));
            if ui
                .add(egui::DragValue::new(&mut v).speed(0.01).fixed_decimals(3))
                .changed()
            {
                changed_pair = Some((key.clone(), v));
            }
        });
    }

    let Some((changed_key, new_value)) = changed_pair else {
        return;
    };

    ctx.trigger(ModelicaParameterRequested {
        entity,
        key: changed_key,
        value: new_value,
    });
}

/// Dispatch a `UsdOp::SetAttribute` for a specific prim path from the typed
/// request observer.
/// Comms & Orbit (doc 43): position ground stations (geodetic anchor) and
/// satellites (Kepler elements) realistically, tune antenna range/mask, and
/// watch live link state. Edits update the live component (the USD bridge
/// runs once per prim) AND persist as journaled `SetAttribute` ops.
fn comms_orbit_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, entity: Entity) {
    use lunco_celestial::{GeodeticAnchor, KeplerOrbit};

    let anchor = ctx.get::<GeodeticAnchor>(entity).copied();
    let orbit = ctx.get::<KeplerOrbit>(entity).copied();
    if anchor.is_none() && orbit.is_none() {
        return;
    }

    egui::CollapsingHeader::new("Anchor & Orbit")
        .default_open(true)
        .show(ui, |ui| {
            if let Some(a) = anchor {
                ui.label("Ground anchor (lat/lon °, height m):");
                let mut lat = a.geodetic.lat_deg;
                let mut lon = a.geodetic.lon_deg;
                let mut height = a.geodetic.height_m;
                let mut body = a.body;
                let changed = ui
                    .horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut lat)
                                .speed(0.01)
                                .range(-90.0..=90.0)
                                .prefix("lat "),
                        )
                        .changed()
                            | ui.add(
                                egui::DragValue::new(&mut lon)
                                    .speed(0.01)
                                    .range(-180.0..=180.0)
                                    .prefix("lon "),
                            )
                            .changed()
                            | ui.add(egui::DragValue::new(&mut height).speed(1.0).prefix("h "))
                                .changed()
                    })
                    .inner
                    | ui.add(egui::DragValue::new(&mut body).prefix("body NAIF "))
                        .changed();
                if changed {
                    let mut value = a;
                    value.body = body;
                    value.geodetic.lat_deg = lat;
                    value.geodetic.lon_deg = lon;
                    value.geodetic.height_m = height;
                    ctx.trigger(InspectorComponentEdit::Anchor { entity, value });
                }
            }

            if let Some(o) = orbit {
                ui.label("Kepler orbit (a m, e, angles °):");
                let mut a_m = o.elements.semi_major_axis_m;
                let mut e = o.elements.eccentricity;
                let mut inc = o.elements.inclination_deg;
                let mut raan = o.elements.raan_deg;
                let mut argp = o.elements.arg_periapsis_deg;
                let mut m0 = o.elements.mean_anomaly_deg;
                let changed = ui
                    .horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut a_m).speed(10_000.0).prefix("a "))
                            .changed()
                            | ui.add(
                                egui::DragValue::new(&mut e)
                                    .speed(0.005)
                                    .range(0.0..=0.95)
                                    .prefix("e "),
                            )
                            .changed()
                            | ui.add(
                                egui::DragValue::new(&mut inc)
                                    .speed(0.1)
                                    .range(-180.0..=180.0)
                                    .prefix("i "),
                            )
                            .changed()
                    })
                    .inner
                    | ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut raan).speed(0.1).prefix("Ω "))
                            .changed()
                            | ui.add(egui::DragValue::new(&mut argp).speed(0.1).prefix("ω "))
                                .changed()
                            | ui.add(egui::DragValue::new(&mut m0).speed(0.1).prefix("M₀ "))
                                .changed()
                    })
                    .inner;
                if changed {
                    let mut value = o;
                    value.elements.semi_major_axis_m = a_m;
                    value.elements.eccentricity = e;
                    value.elements.inclination_deg = inc;
                    value.elements.raan_deg = raan;
                    value.elements.arg_periapsis_deg = argp;
                    value.elements.mean_anomaly_deg = m0;
                    ctx.trigger(InspectorComponentEdit::Orbit { entity, value });
                }
            }
        });
    ui.separator();
}

/// Apply a sequence of typed [`UsdOp`]s to `entity`'s backing document, in order —
/// each journals and inverts on its own. Used by the mount snap, which re-authors a
/// part's transform + joint anchor as four ops.
fn apply_usd_ops(world: &mut World, entity: Entity, ops: Vec<UsdOp>) {
    let Some(doc) = resolve_doc_for_entity(world, entity) else {
        return;
    };
    for op in ops {
        world.trigger(ApplyUsdOp { doc, op });
    }
}

fn apply_usd_path_attribute_change(
    world: &mut World,
    entity: Entity,
    prim_path: String,
    name: &str,
    type_name: &str,
    value: String,
) {
    if let Some(doc) = resolve_doc_for_entity(world, entity) {
        let op = UsdOp::SetAttribute {
            edit_target: LayerId::root(),
            path: prim_path,
            name: name.to_string(),
            type_name: type_name.to_string(),
            value,
        };
        world.trigger(ApplyUsdOp { doc, op });
    }
}

/// Dispatch a `UsdOp::SetVariantSelection` — choose which variant of `set` the
/// prim composes with from the typed request observer.
///
/// Coarse by nature: value resolution re-composes the prim's whole subtree, so
/// the projection rebuilds instead of replaying incrementally
/// (`op_needs_rebuild`). That is the point — a variant can add and remove
/// prims, not just change values. It still journals, replicates and undoes like
/// every other op, and the author is read-modify-write so selecting one set
/// never drops a sibling set's selection.
fn apply_usd_variant_selection(
    world: &mut World,
    entity: Entity,
    prim_path: String,
    set: String,
    variant: String,
) {
    if let Some(doc) = resolve_doc_for_entity(world, entity) {
        let op = UsdOp::SetVariantSelection {
            edit_target: LayerId::root(),
            path: prim_path,
            variant_set: set,
            variant,
        };
        world.trigger(ApplyUsdOp { doc, op });
    }
}

/// Dispatch a `UsdOp::SetAttribute` to write changes back to the USD
/// document from the typed request observer.
fn apply_usd_attribute_change(
    world: &mut World,
    entity: Entity,
    name: &str,
    type_name: &str,
    value: String,
) {
    if let Some(prim) = world.get::<UsdPrimPath>(entity).cloned() {
        apply_usd_path_attribute_change(world, entity, prim.path, name, type_name, value);
    }
}
