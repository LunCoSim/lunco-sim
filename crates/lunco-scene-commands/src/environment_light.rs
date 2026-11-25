//! Route a validated environment edit to the live environment or its mounted USD document.
use bevy::prelude::*;
use lunco_core::{on_command, register_commands};
use lunco_environment::{EnvironmentLightTargets, SetEnvironmentLight};
use lunco_usd_bevy_scene::{UsdAnimated, UsdPrimPath};
use lunco_usd_bevy_stage::UsdRead;
use lunco_usd_core::commands::ApplyUsdOps;
use lunco_usd_document::document::{LayerId, UsdOp};
use openusd::sdf::Path;
use openusd_schemas::lux::tokens as lux;

#[on_command(SetEnvironmentLight)]
fn on_set_environment_light(
    trigger: On<SetEnvironmentLight>,
    active: Res<lunco_core::ActiveCommandId>,
    pending: Res<lunco_api::executor::PendingApiRequest>,
    mut commands: Commands,
) {
    let cmd = trigger.event().clone();
    let active = active.clone();
    let correlation = (pending.correlation_id != 0).then_some(pending.correlation_id);
    commands.queue(move |world: &mut World| {
        let result = world
            .run_system_cached_with(lunco_environment::update_environment_light, cmd.clone())
            .map_err(|error| error.to_string())
            .and_then(|targets| targets)
            .and_then(|targets| {
                if cmd.intent.is_persistent() {
                    plan_light_edit(world, &cmd, targets).map(Some)
                } else {
                    Ok(None)
                }
            });
        let outcome = match result {
            Ok(Some(edit)) => {
                lunco_api::executor::trigger_command_with_context(world, edit, active, correlation);
                return;
            }
            Ok(None) => Ok(lunco_command_contracts::Ack::default()),
            Err(message) => {
                warn!("SetEnvironmentLight: {message}");
                Err(message)
            }
        };
        lunco_api::executor::finish_command_result(
            world,
            active.get(),
            correlation,
            outcome,
            lunco_api_core::ApiErrorCode::CommandRejected,
        );
    });
}
register_commands!(on_set_environment_light);

pub(crate) fn install(app: &mut App) {
    use lunco_api::executor::DeferredCommandAppExt;
    register_all_commands(app);
    app.register_deferred_command::<SetEnvironmentLight>();
}

fn attribute(path: &str, name: &str, kind: &str, value: impl ToString) -> UsdOp {
    UsdOp::SetAttribute {
        edit_target: LayerId::runtime(),
        path: path.into(),
        name: name.into(),
        type_name: kind.into(),
        value: value.to_string(),
    }
}

fn require_static(reader: &impl UsdRead, path: &Path, names: &[&str]) -> Result<(), String> {
    for name in names {
        if reader.has_time_samples(path, name) || !reader.connections(path, name).is_empty() {
            return Err(format!("{path}.{name} is driven"));
        }
    }
    Ok(())
}

fn color(reader: &impl UsdRead, path: &Path, requested: [f32; 3]) -> Result<UsdOp, String> {
    require_static(
        reader,
        path,
        &[
            lux::INPUTS_COLOR,
            lux::INPUTS_ENABLE_COLOR_TEMPERATURE,
            lux::INPUTS_COLOR_TEMPERATURE,
        ],
    )?;
    let multiplier = lunco_usd_bevy_light::light::light_temperature_multiplier(reader, path)
        .map_err(|_| format!("Invalid light temperature at {path}"))?
        .to_array();
    let mut tint = [0.0; 3];
    for (i, component) in tint.iter_mut().enumerate() {
        if multiplier[i] == 0.0 && requested[i] == 0.0 {
            continue;
        }
        *component = requested[i] / multiplier[i];
        if !component.is_finite() || *component < 0.0 {
            return Err(format!(
                "Requested color is outside the light temperature gamut at {path}"
            ));
        }
    }
    let [r, g, b] = tint;
    Ok(attribute(
        path.as_str(),
        lux::INPUTS_COLOR,
        "color3f",
        format!("({r}, {g}, {b})"),
    ))
}

fn ensure_child(
    reader: &impl UsdRead,
    ops: &mut Vec<UsdOp>,
    parent: &str,
    name: &str,
    kind: &str,
) -> Result<String, String> {
    let path = Path::new(&format!("{parent}/{name}")).map_err(|e| e.to_string())?;
    if reader.has_prim(&path) {
        if reader.type_name(&path).as_deref() != Some(kind) || !reader.is_active(&path)? {
            return Err(format!("Expected an active {kind} at {path}"));
        }
    } else {
        ops.push(UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: parent.into(),
            name: name.into(),
            type_name: Some(kind.into()),
            reference: None,
            reference_prim_path: None,
        });
    }
    Ok(path.to_string())
}

fn static_intensity(reader: &impl UsdRead, path: &Path, effective: f64) -> Result<f32, String> {
    require_static(reader, path, &[lux::INPUTS_INTENSITY, lux::INPUTS_EXPOSURE])?;
    let exposure = reader
        .real_f32(path, lux::INPUTS_EXPOSURE)
        .ok_or_else(|| format!("{path} has no valid light exposure"))?;
    let angular_scale = if reader.type_name(path).as_deref() == Some(lux::DISTANT_LIGHT) {
        require_static(reader, path, &[lux::INPUTS_ANGLE, lux::INPUTS_NORMALIZE])?;
        let angle = reader
            .real_f32(path, lux::INPUTS_ANGLE)
            .filter(|angle| (0.0..=360.0).contains(angle))
            .ok_or_else(|| format!("{path} has invalid angular diameter"))?;
        let normalized = reader
            .boolean(path, lux::INPUTS_NORMALIZE)
            .ok_or_else(|| format!("{path} has invalid normalization"))?;
        f64::from(lunco_usd_bevy_light::light::distant_light_illuminance(
            1.0, normalized, angle,
        ))
    } else {
        1.0
    };
    let raw = effective / (f64::from(exposure).exp2() * angular_scale);
    let schema_float = raw as f32;
    if !raw.is_finite()
        || !schema_float.is_finite()
        || raw < 0.0
        || (raw > 0.0 && schema_float == 0.0)
    {
        return Err(format!("{path} intensity is outside the USD float range"));
    }
    Ok(schema_float)
}

fn plan_light_edit(
    world: &World,
    cmd: &SetEnvironmentLight,
    targets: EnvironmentLightTargets,
) -> Result<ApplyUsdOps, String> {
    let root = world
        .get_resource::<lunco_core::SceneMountState>()
        .and_then(|m| m.active_root())
        .ok_or("No active scene")?;
    let prim = world
        .get::<UsdPrimPath>(root)
        .ok_or("The mounted scene has no USD prim")?;
    let doc = lunco_scene_authoring::doc_resolve::resolve_doc_for_entity(world, root)
        .ok_or("The mounted scene has no USD document")?;
    let generation = world
        .resource::<lunco_doc_bevy::DocumentRegistry<lunco_usd_document::document::UsdDocument>>()
        .host(doc)
        .ok_or("Lighting document is closed")?
        .generation();
    let stage = prim.stage_handle.id();
    if world
        .resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
        .applied_stage_generation(doc, stage)
        != Some(generation)
    {
        return Err("The mounted lighting revision is out of date".into());
    }
    let reader = lunco_scene_authoring::doc_resolve::composed_reader(world, prim)?;
    let path_for = |entity| {
        let target = world
            .get::<UsdPrimPath>(entity)
            .ok_or("Light has no USD identity")?;
        if target.stage_handle.id() != stage {
            return Err("Light belongs to another stage".into());
        }
        Path::new(&target.path).map_err(|e| e.to_string())
    };
    let mut ops = Vec::new();
    if let Some(entity) = targets.sun {
        let path = path_for(entity)?;
        let p = path.as_str();
        if cmd.shadow_maps_enabled.is_some()
            || cmd.shadow_max_distance.is_some()
            || cmd.shadow_first_cascade_bound.is_some()
        {
            let mut schemas = reader.api_schemas(&path);
            let initial = schemas.len();
            for (schema, needed) in [
                (
                    "ShadowAPI",
                    cmd.shadow_maps_enabled.is_some() || cmd.shadow_max_distance.is_some(),
                ),
                ("LunCoShadowAPI", cmd.shadow_first_cascade_bound.is_some()),
            ] {
                if needed && !schemas.iter().any(|applied| applied == schema) {
                    schemas.push(schema.into());
                }
            }
            if schemas.len() != initial {
                ops.push(UsdOp::SetApiSchemas {
                    edit_target: LayerId::runtime(),
                    path: p.into(),
                    schemas,
                });
            }
        }
        if let Some(v) = cmd.illuminance {
            ops.push(attribute(
                p,
                lux::INPUTS_INTENSITY,
                "float",
                static_intensity(&reader, &path, f64::from(v))?,
            ));
        }
        if let Some(value) = cmd.sun_color {
            ops.push(color(&reader, &path, value)?);
        }
        if let Some(value) = cmd.shadow_maps_enabled {
            require_static(&reader, &path, &[lux::INPUTS_SHADOW_ENABLE])?;
            ops.push(attribute(p, lux::INPUTS_SHADOW_ENABLE, "bool", value));
        }
        for (name, metres) in [
            (lux::INPUTS_SHADOW_DISTANCE, cmd.shadow_max_distance),
            (
                "lunco:shadow:firstCascadeFarBound",
                cmd.shadow_first_cascade_bound,
            ),
        ] {
            if let Some(value) = metres {
                require_static(&reader, &path, &[name])?;
                let convention =
                    lunco_usd_bevy_stage::stage_convention(&reader).map_err(|e| e.to_string())?;
                let schema_float = (f64::from(value) / convention.length(1.0)) as f32;
                if !schema_float.is_finite() || schema_float <= 0.0 {
                    return Err(format!("{name} is outside the USD float range"));
                }
                ops.push(attribute(p, name, "float", schema_float));
            }
        }
        if let Some(frame_rotation) = targets.rotation {
            let time = world
                .get::<UsdAnimated>(entity)
                .map_or(0.0, |a| a.pose_time_code);
            let reset = reader
                .resets_xform_stack(&path)
                .map_err(|error| error.to_string())?;
            // A reset stack uses the frame above the stage root's authored transform.
            let basis_owner = if reset { root } else { entity };
            let parent = world
                .get::<ChildOf>(basis_owner)
                .ok_or_else(|| format!("No mounted parent for light {path}"))?
                .parent();
            let (_, parent_rotation) = lunco_physics::SimulationPoseReadState::try_new(world)
                .and_then(|mut poses| poses.pose(parent))
                .ok_or_else(|| {
                    format!(
                        "Cannot resolve the mounted parent of {path} in the active physics frame"
                    )
                })?;
            let local_rotation = (parent_rotation.0.inverse() * frame_rotation).normalize();
            ops.extend(lunco_scene_authoring::local_pose::local_pose_ops(
                &reader,
                &LayerId::runtime(),
                p,
                time,
                None,
                Some(local_rotation),
                None,
            )?);
        }
    }
    if let (Some(entity), Some(rgb)) = (targets.earthshine, cmd.earthshine_color) {
        ops.push(color(&reader, &path_for(entity)?, rgb)?);
    }
    if cmd.bloom_intensity.is_some()
        || cmd.exposure_ev100.is_some()
        || cmd.ambient_brightness.is_some()
    {
        let environment_kind = lunco_environment::LUNCO_ENVIRONMENT_PRIM_TYPE;
        let stages = world
            .get_non_send::<lunco_usd_bevy_stage::canonical::CanonicalStages>()
            .ok_or("USD projection registry unavailable")?;
        let asset = world
            .resource::<Assets<lunco_usd_bevy_stage::UsdStageAsset>>()
            .get(stage)
            .ok_or("Mounted lighting stage is unavailable")?;
        let environment_paths = stages.paths_of_type(stage, asset, &prim.path, environment_kind)?;
        let env = match environment_paths.as_slice() {
            [path] => path.to_string(),
            [] => ensure_child(
                &reader,
                &mut ops,
                &prim.path,
                "Environment",
                environment_kind,
            )?,
            _ => return Err("Multiple environment settings prims in the lighting document".into()),
        };
        for (name, v) in [
            ("lunco:env:exposureEv100", cmd.exposure_ev100),
            ("lunco:env:bloomIntensity", cmd.bloom_intensity),
        ] {
            if let Some(v) = v {
                let path = Path::new(&env).map_err(|e| e.to_string())?;
                if reader.has_prim(&path) {
                    require_static(&reader, &path, &[name])?;
                }
                ops.push(attribute(&env, name, "float", v));
            }
        }
        if let Some(requested) = cmd.ambient_brightness {
            let fill = ensure_child(&reader, &mut ops, &env, "AmbientFill", lux::DOME_LIGHT)?;
            let path = Path::new(&fill).map_err(|e| e.to_string())?;
            let domes = stages.paths_of_type(stage, asset, &prim.path, lux::DOME_LIGHT)?;
            let others = lunco_usd_bevy_light::light::untextured_dome_intensity_sum(
                &reader,
                domes,
                Some(&path),
            )
            .map_err(|_| "Cannot resolve static ambient lights")?;
            if f64::from(requested) < others {
                return Err(format!(
                    "Other DomeLights already contribute {others} to the requested total {requested}"
                ));
            }
            let remaining = f64::from(requested) - others;
            let intensity = if reader.has_prim(&path) {
                require_static(&reader, &path, &[lux::INPUTS_TEXTURE_FILE])?;
                if reader
                    .asset_identifier(&path, lux::INPUTS_TEXTURE_FILE)
                    .map_err(|e| e.to_string())?
                    .is_some_and(|value| !value.is_empty())
                {
                    return Err("AmbientFill must be an untextured DomeLight".into());
                }
                static_intensity(&reader, &path, remaining)?
            } else {
                remaining as f32
            };
            ops.push(attribute(&fill, lux::INPUTS_INTENSITY, "float", intensity));
        }
    }
    if ops.is_empty() {
        return Err("Lighting edit has no authored fields".into());
    }
    Ok(ApplyUsdOps {
        doc_id: doc,
        parent_gen: Some(generation),
        label: "Edit environment lighting".into(),
        ops,
    })
}
