//! Scene property and shader authoring commands.
//!
//! This module owns the generic `SetObjectProperty` surface, standard USD
//! property persistence, and live shader source/create/import/delete commands.
//! The command plugin is installed by `lunco-scene-commands` so mutation and
//! authoring remain separate production packages.

use bevy::prelude::*;
use lunco_core::{Command, EditIntent, on_command};
use lunco_materials::{ParamSchema, ParamValue, ShaderLook};
use lunco_render::{PbrLook, SurfaceAlpha};
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_usd_core::commands::ApplyUsdOps;
use lunco_usd_document::document::{LayerId, UsdOp};

/// Public wheel parameter, live setter, and authored USD property.
pub(crate) struct WheelParam {
    /// The single public `SetObjectProperty` name for this parameter.
    pub name: &'static str,
    /// Live setter on `WheelRaycast`. Non-capturing closures coerce to `fn`.
    pub set: fn(&mut lunco_mobility::WheelRaycast, f64),
    /// The USD attribute the loader reads back into this field (`float`).
    pub usd_attr: &'static str,
}

/// Every wheel-dynamics parameter `SetObjectProperty` can tune. Each row's
/// `usd_attr` is a name `lunco_usd_sim`'s wheel loader actually reads, so every
/// tune round-trips through the runtime layer on reload.
pub(crate) const WHEEL_PARAMS: &[WheelParam] = &[
    WheelParam {
        name: "brake_torque",
        set: |w, v| w.brake_torque_max = v,
        usd_attr: "physxVehicleWheel:maxBrakeTorque",
    },
    WheelParam {
        name: "slip_stiffness",
        set: |w, v| w.slip_stiffness = v,
        usd_attr: "physxVehicleTire:longitudinalStiffness",
    },
    WheelParam {
        name: "bearing_damping",
        set: |w, v| w.bearing_damping = v,
        usd_attr: "physxVehicleWheel:dampingRate",
    },
    WheelParam {
        name: "friction_mu",
        set: |w, v| w.friction_mu = v,
        usd_attr: "physics:dynamicFriction",
    },
    WheelParam {
        name: "mass",
        set: |w, v| w.mass = v,
        usd_attr: "physxVehicleWheel:mass",
    },
    WheelParam {
        name: "moi",
        set: |w, v| w.moment_of_inertia = v,
        usd_attr: "physxVehicleWheel:moi",
    },
    WheelParam {
        name: "wheel_radius",
        set: |w, v| w.wheel_radius = v,
        usd_attr: "physxVehicleWheel:radius",
    },
];

/// Look a canonical `SetObjectProperty` name up in [`WHEEL_PARAMS`], or `None`
/// if it isn't a wheel field. Both the live-mutation path and the USD-authoring
/// path go through this one lookup.
pub(crate) fn wheel_param(name: &str) -> Option<&'static WheelParam> {
    WHEEL_PARAMS.iter().find(|p| p.name == name)
}

/// Validate the target and value before routing a suspension or wheel edit to
/// the live component or to the USD document that projects its persisted value.
fn set_vehicle_property(
    world: &mut World,
    target: Entity,
    key: &str,
    text: &str,
    intent: EditIntent,
) -> Result<Option<ApplyUsdOps>, String> {
    let value: f64 = text
        .trim()
        .parse()
        .map_err(|_| format!("{key} requires a number"))?;
    if !value.is_finite() {
        return Err(format!("{key} requires a finite number"));
    }
    if value < 0.0 || (value == 0.0 && matches!(key, "wheel_radius" | "mass")) {
        return Err(format!("{value} is outside the physical range for {key}"));
    }
    let (attribute, attachment) = if let Some(param) = wheel_param(key) {
        let mut wheel = world
            .get_mut::<lunco_mobility::WheelRaycast>(target)
            .ok_or_else(|| format!("{key} requires a wheel target"))?;
        if !intent.is_persistent() {
            (param.set)(&mut wheel, value);
            return Ok(None);
        }
        (
            param.usd_attr,
            matches!(key, "slip_stiffness" | "friction_mu"),
        )
    } else {
        let mut suspension = world
            .get_mut::<lunco_mobility::Suspension>(target)
            .ok_or_else(|| format!("{key} requires a suspension target"))?;
        let (attribute, set): (&str, fn(&mut lunco_mobility::Suspension, f64)) = match key {
            "rest_length" => ("lunco:suspension:restLength", |s, v| s.rest_length = v),
            "spring_k" => ("physxVehicleSuspension:springStrength", |s, v| {
                s.spring_k = v
            }),
            "damping_c" => ("physxVehicleSuspension:springDamperRate", |s, v| {
                s.damping_c = v
            }),
            _ => return Err(format!("unknown suspension property {key}")),
        };
        if !intent.is_persistent() {
            set(&mut suspension, value);
            return Ok(None);
        }
        (attribute, true)
    };
    let (doc, mut path) = crate::doc_resolve::authorable_entity(world, target)
        .ok_or("persistent vehicle property requires an authored USD target")?;
    let prim = world
        .get::<UsdPrimPath>(target)
        .ok_or("vehicle target has no USD identity")?;
    let reader = crate::doc_resolve::composed_reader(world, prim)?;
    if attachment {
        let topology = lunco_usd_sim_authoring::collect_wheel_attachment_topology(&reader);
        let binding = topology
            .binding_for(&path)
            .ok_or("wheel attachment does not resolve its tire and suspension")?;
        path = if wheel_param(key).is_some() {
            binding.tire.clone()
        } else {
            binding.suspension.clone()
        };
    }
    let sdf = openusd::sdf::Path::new(&path).map_err(|error| error.to_string())?;
    let convention = lunco_usd_bevy_stage::stage_convention(&reader).map_err(|e| e.to_string())?;
    let value = value
        / lunco_usd_sim_authoring::wheel_params::wheel_parameter_unit_scale(
            attribute,
            convention.length(1.0),
        );
    let kind = lunco_usd_bevy_stage::UsdRead::attr_type_name(&reader, &sdf, attribute)
        .ok_or_else(|| format!("{path}.{attribute} has no declared type"))?;
    match kind.as_str() {
        "double" if value.is_finite() => (),
        "float" if (value as f32).is_finite() => (),
        _ => return Err(format!("{key} cannot be represented as USD {kind}")),
    }
    Ok(Some(ApplyUsdOps {
        doc_id: doc,
        parent_gen: None,
        label: format!("Edit {key}"),
        ops: vec![UsdOp::SetAttribute {
            edit_target: LayerId::runtime(),
            path,
            name: attribute.into(),
            type_name: kind,
            value: value.to_string(),
        }],
    }))
}

/// Set a property on a scene object through its component owner. Use
/// `Interactive` (default) for live state and `Persistent` to explicitly author a
/// document-backed setting.
///
/// ```jsonc
/// {"type":"ExecuteCommand","command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"shader","value":"shaders/balloon.wesl"}}
/// {"type":"ExecuteCommand","command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"wedge_count","value":"12"}}
/// {"type":"ExecuteCommand","command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"cell_a","value":"0.1,0.8,0.2"}}
/// ```
///
/// Recognised `property` values:
/// - `shader` → author a [`ShaderLook`] for that `.wesl` (asset path); the render
///   binder turns it into a material.
/// - any parameter named by the shader's `Material` struct (e.g. `albedo`,
///   `wedge_count`, `cell_a`) → set that named value on the entity's `ShaderLook`
///   (requires `shader` set first, or a USD shader material). The shader's
///   reflected schema resolves the type; colours are `r,g,b`.
/// - `visible` → `true`/`false` through the same namespace visibility owner as `SetSceneView`.
/// - Per-wheel tire-spin dynamics (target a single wheel entity by its `api_id`):
///   `brake_torque`, `slip_stiffness`, `bearing_damping`, `friction_mu`, `mass`,
///   `moi`, `wheel_radius`, `rest_length`, `spring_k`, `damping_c` → set that
///   `f64` field on the wheel's `WheelRaycast` live. Each wheel is its own entity,
///   so this gives independent per-wheel control. Motor torque and no-load speed
///   are owned by the composed Modelica motor prim; edit its authored
///   `inputs:stall_torque` / `inputs:no_load_speed` attributes instead of
///   addressing a wheel-local drive parameter.
#[Command(default)]
pub struct SetObjectProperty {
    /// API-stable global entity ID from `ListEntities`, resolved to the live
    /// Bevy entity by `ApiEntityRegistry`.
    pub entity_id: u64,
    /// Property name (see struct docs).
    pub property: String,
    /// Value; comma-separated `r,g,b` for colors, a single float for params,
    /// an asset path for `shader`, `true`/`false` for `visible`.
    pub value: String,
    /// `Interactive` previews in ECS. `Persistent` commits supported authored
    /// properties to the document.
    #[serde(default)]
    #[reflect(default)]
    pub intent: EditIntent,
}

/// The `SetObjectProperty` PBR keys [`PbrLook`] can express.
///
/// These go through the **appearance-intent component**, not a material asset:
/// mutating `PbrLook` is enough, because `lunco-render-bevy`'s `Changed<PbrLook>`
/// binder re-materialises the entity. Keeping material ownership in the render
/// binder prevents shared handles from leaking edits between entities and keeps
/// `bevy_pbr` out of the headless command layer.
const PBR_LOOK_KEYS: &[&str] = &[
    "base_color",
    "emissive",
    "metallic",
    "roughness",
    "ior",
    "alpha",
    "unlit",
    "double_sided",
];

/// Produce one USD material transaction for a selected layer. Creating a
/// surface seeds all inputs from the projected look. Existing materials only
/// receive the changed inputs. Geometry flags stay on the addressed prim.
pub fn appearance_ops(
    world: &World,
    prim: &UsdPrimPath,
    edit_target: LayerId,
    keys: &[&str],
    look: &PbrLook,
) -> Result<Vec<UsdOp>, String> {
    let mut ops = Vec::new();
    let mut surface_schemas = None;
    // Geometry-level appearance intents live on the gprim, not the surface.
    for &key in keys
        .iter()
        .filter(|key| matches!(**key, "double_sided" | "unlit"))
    {
        if key == "unlit" {
            let mut schemas = crate::doc_resolve::geom_api_schemas(world, prim)?;
            if !schemas.iter().any(|schema| schema == "LunCoSurfaceAPI") {
                schemas.push("LunCoSurfaceAPI".into());
                ops.push(UsdOp::SetApiSchemas {
                    edit_target: edit_target.clone(),
                    path: prim.path.clone(),
                    schemas: schemas.clone(),
                });
                surface_schemas = Some(schemas);
            }
        }
        let (name, value) = if key == "double_sided" {
            ("doubleSided", look.double_sided)
        } else {
            ("lunco:surface:unlit", look.unlit)
        };
        ops.push(UsdOp::SetAttribute {
            edit_target: edit_target.clone(),
            path: prim.path.clone(),
            name: name.into(),
            type_name: "bool".into(),
            value: value.to_string(),
        });
    }
    if keys
        .iter()
        .all(|key| matches!(*key, "double_sided" | "unlit"))
    {
        return Ok(ops);
    }
    if keys.iter().any(|key| !PBR_LOOK_KEYS.contains(key)) {
        return Err("Unsupported PBR property".into());
    }

    // An existing bound shader, else create the material.
    let existing = crate::doc_resolve::bound_shader_prim(world, prim)?;
    let (setup, shader, fresh) = match existing {
        Some(sp) => (Vec::new(), sp, false),
        None => {
            let schemas = match surface_schemas {
                Some(schemas) => schemas,
                None => crate::doc_resolve::geom_api_schemas(world, prim)?,
            };
            match lunco_usd_core::material::ensure_preview_surface_ops(
                edit_target.clone(),
                &prim.path,
                &schemas,
            ) {
                Some((ops, shader)) => (ops, shader, true),
                None => return Err("cannot create USD material at this prim".into()),
            }
        }
    };

    ops.extend(setup);
    let mut set = |attr: &str, ty: &str, value: String| {
        ops.push(UsdOp::SetAttribute {
            edit_target: edit_target.clone(),
            path: shader.clone(),
            name: attr.into(),
            type_name: ty.into(),
            value,
        });
    };
    let c = |c: LinearRgba| format!("({}, {}, {})", c.red, c.green, c.blue);
    for (k, ty, v) in [
        ("base_color", "color3f", c(look.base_color)),
        ("emissive", "color3f", c(look.emissive)),
        ("metallic", "float", look.metallic.to_string()),
        ("roughness", "float", look.perceptual_roughness.to_string()),
        ("opacity", "float", look.base_color.alpha.to_string()),
        ("ior", "float", look.ior.to_string()),
    ] {
        // A fresh material seeds every input; an existing one writes only what
        // changed (so an unrelated authored input is not clobbered).
        if !fresh && !keys.iter().any(|key| key_matches(key, k)) {
            continue;
        }
        if let Some((attr, _)) = lunco_usd_core::material::preview_surface_input(k) {
            set(attr, ty, v);
        }
    }
    Ok(ops)
}

/// Whether the edited look key names the same `UsdPreviewSurface` input as `slot`
/// (`roughness` and `alpha` are the canonical command keys).
fn key_matches(key: &str, slot: &str) -> bool {
    lunco_usd_core::material::preview_surface_input(key)
        == lunco_usd_core::material::preview_surface_input(slot)
}

/// Validate an entire PBR patch against the selected surface before publishing it.
pub fn edited_pbr_look<'a>(
    world: &World,
    target: Entity,
    edits: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<PbrLook, String> {
    if let Some(failure) = world.get::<lunco_usd_bevy_scene::UsdSceneProjectionFailed>(target) {
        return Err(failure.0.clone());
    }
    let mut look = match world.get::<PbrLook>(target) {
        Some(look) => look.clone(),
        None if world.get::<Mesh3d>(target).is_some() => PbrLook::default(),
        None => return Err("entity has no PBR surface".into()),
    };
    for (key, value) in edits {
        if !apply_pbr_look(&mut look, key, value) {
            return Err(format!("invalid {key} value '{value}'"));
        }
    }
    Ok(look)
}

fn apply_pbr_look(look: &mut PbrLook, key: &str, value: &str) -> bool {
    if matches!(key, "unlit" | "double_sided") {
        let enabled = match value.trim() {
            "true" | "1" | "yes" | "on" => true,
            "false" | "0" | "no" | "off" => false,
            _ => return false,
        };
        if key == "unlit" {
            look.unlit = enabled;
        } else {
            look.double_sided = enabled;
        }
        return true;
    }
    let Some(f) = value
        .split(',')
        .map(|v| v.trim().parse::<f32>().ok().filter(|v| v.is_finite()))
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    if matches!(key, "base_color" | "emissive") {
        if !(3..=4).contains(&f.len()) {
            return false;
        }
    } else if f.len() != 1 {
        return false;
    }
    match key {
        "base_color" => {
            let a = f.get(3).copied().unwrap_or(look.base_color.alpha);
            look.base_color = LinearRgba::new(f[0], f[1], f[2], a);
        }
        "emissive" => {
            look.emissive = LinearRgba::new(f[0], f[1], f[2], f.get(3).copied().unwrap_or(1.0));
        }
        "metallic" => {
            let Some(v) = f.first() else { return false };
            look.metallic = v.clamp(0.0, 1.0);
        }
        "roughness" => {
            let Some(v) = f.first() else { return false };
            look.perceptual_roughness = v.clamp(0.0, 1.0);
        }
        // Index of refraction — `UsdPreviewSurface`'s `inputs:ior`. The specular knob;
        // Bevy's `reflectance` is derived from it (see `lunco-render-bevy`). 1.0 = no
        // Fresnel at all (vacuum); nothing physical goes below it.
        "ior" => {
            let Some(v) = f.first() else { return false };
            look.ior = v.max(1.0);
        }
        "alpha" => {
            let Some(v) = f.first() else { return false };
            let v = v.clamp(0.0, 1.0);
            look.base_color.alpha = v;
            look.alpha = if v >= 1.0 {
                SurfaceAlpha::Opaque
            } else {
                SurfaceAlpha::Blend
            };
        }
        _ => return false,
    }
    true
}

/// The reflected parameter schema of a shader **asset path**.
///
/// Read straight out of the loaded WESL source (`Material` struct + `//!@`
/// annotations) rather than off a material — the schema is a property of the
/// *asset*, and reading it this way keeps the shader-param paths render-free.
/// `None` while the shader is still loading (or if it declares no `Material`) is
/// an unavailable edit target, not permission to infer a type.
fn shader_schema(
    path: &str,
    asset_server: &AssetServer,
    shaders: &Assets<bevy::shader::Shader>,
) -> Option<ParamSchema> {
    let path = match lunco_assets_core::asset_path::load_asset_path(path, None, None, None) {
        Ok(path) => path,
        Err(error) => {
            warn!("invalid shader schema address: {error}");
            return None;
        }
    };
    let handle = asset_server.load::<bevy::shader::Shader>(path);
    let src = match &shaders.get(&handle)?.source {
        bevy::shader::Source::Wesl(s) => s.as_ref().to_string(),
        _ => return None,
    };
    ParamSchema::parse(&src)
}

/// Parse one `SetObjectProperty` value into a typed [`ParamValue`] for `key`.
///
/// The field's type comes from the shader's reflected schema. Unknown fields,
/// unavailable schemas, engine-owned fields, and malformed component text are
/// rejected; RGB receives the explicit opaque-alpha convention only for a
/// reflected `vec4` field.
fn shader_param_value(schema: Option<&ParamSchema>, key: &str, value: &str) -> Option<ParamValue> {
    let schema = schema?;
    let field = schema.field(key)?;
    if schema.is_engine(key) {
        return None;
    }
    ParamValue::parse_authoring(field.ty, value)
}

/// Give `target` a [`ShaderLook`] for `shader_path`, carrying over any parameters
/// it already has so swapping the `.wesl` keeps tuned values.
///
/// Drops the [`PbrLook`] intent: an entity that carries both draws twice, because
/// each binder materialises its own. See `lunco-render-bevy`'s caller contract.
pub(crate) fn author_shader_look(
    commands: &mut Commands,
    target: Entity,
    existing: Option<&ShaderLook>,
    shader_path: &str,
) {
    let mut look = existing.cloned().unwrap_or_default();
    look.shader = shader_path.to_string();
    commands.entity(target).remove::<PbrLook>().try_insert(look);
}

/// Plan a WESL assignment on the bound UsdShade shader. The document stores
/// the public source identity, including for shaders in a mounted Twin.
pub fn shader_assignment_ops(
    world: &World,
    prim: &UsdPrimPath,
    edit_target: LayerId,
    source: &str,
) -> Result<Vec<UsdOp>, String> {
    let source = crate::shader_doc::canonical_shader_path(source);
    if !is_wesl_shader_path(&source) {
        return Err("Shader assignment requires a standalone .wesl asset".into());
    }
    let source =
        lunco_assets_core::asset_path::source_uri_for_address(&source, world.get_resource())
            .map_err(|error| error.to_string())?;
    let shader = crate::doc_resolve::bound_shader_prim(world, prim)?
        .ok_or("Shader assignment requires a bound USD shader")?;
    Ok([
        (
            "info:implementationSource",
            "token",
            r#""sourceAsset""#.to_owned(),
        ),
        ("info:wesl:sourceAsset", "asset", format!("@{source}@")),
    ]
    .into_iter()
    .map(|(name, kind, value)| UsdOp::SetAttribute {
        edit_target: edit_target.clone(),
        path: shader.clone(),
        name: name.into(),
        type_name: kind.into(),
        value,
    })
    .collect())
}

/// Observer for [`SetObjectProperty`].
#[on_command(SetObjectProperty)]
pub fn on_set_object_property(
    trigger: On<SetObjectProperty>,
    active: Res<lunco_core::ActiveCommandId>,
    pending: Res<lunco_api::executor::PendingApiRequest>,
    mut commands: Commands,
) {
    let cmd = trigger.event().clone();
    let active = active.clone();
    let correlation = (pending.correlation_id != 0).then_some(pending.correlation_id);
    commands.queue(move |world: &mut World| {
        let result = match apply_object_property(world, &cmd, active.origin()) {
            Ok(Some(edit)) => {
                lunco_api::executor::trigger_command_with_context(world, edit, active, correlation);
                return;
            }
            Ok(None) => Ok(lunco_command_contracts::Ack::default()),
            Err(message) => {
                warn!("SetObjectProperty: {message}");
                Err(message)
            }
        };
        lunco_api::executor::finish_command_result(
            world,
            active.get(),
            correlation,
            result,
            lunco_api_core::ApiErrorCode::CommandRejected,
        );
    });
}

fn apply_object_property(
    world: &mut World,
    cmd: &SetObjectProperty,
    origin: Option<lunco_core::CommandOrigin>,
) -> Result<Option<ApplyUsdOps>, String> {
    use lunco_usd_bevy_scene::visibility::{
        presentation_actor, reconcile_runtime_visibility, validate_runtime_visibility,
        write_runtime_visibility,
    };
    let target = world
        .resource::<lunco_api::registry::ApiEntityRegistry>()
        .resolve(&lunco_core::GlobalEntityId::from_raw(cmd.entity_id))
        .ok_or_else(|| format!("no entity with API id {}", cmd.entity_id))?;
    let key = cmd.property.as_str();
    if wheel_param(key).is_some() || matches!(key, "rest_length" | "spring_k" | "damping_c") {
        return set_vehicle_property(world, target, key, &cmd.value, cmd.intent);
    }
    let persistent = cmd.intent.is_persistent();
    let destination = if persistent {
        Some(
            crate::doc_resolve::authorable_entity(world, target)
                .ok_or("persistent property requires an authored USD target")?,
        )
    } else {
        None
    };
    let mut ops = Vec::new();
    match key {
        "visible" => {
            let visible = cmd
                .value
                .trim()
                .parse::<bool>()
                .map_err(|_| "visible requires true or false")?;
            validate_runtime_visibility(world, target)?;
            if let Some((_, path)) = &destination {
                let token = if visible { "inherited" } else { "invisible" };
                ops.push(UsdOp::SetAttribute {
                    edit_target: LayerId::runtime(),
                    path: path.clone(),
                    name: "visibility".into(),
                    type_name: "token".into(),
                    value: format!("\"{token}\""),
                });
            } else {
                let actor = presentation_actor(world, origin)?;
                if write_runtime_visibility(world, target, Some(visible), actor) {
                    reconcile_runtime_visibility(world, &[target]);
                }
            }
        }
        "shader" => {
            if persistent {
                let prim = world.get::<UsdPrimPath>(target).ok_or("missing USD prim")?;
                ops.extend(shader_assignment_ops(
                    world,
                    prim,
                    LayerId::runtime(),
                    &cmd.value,
                )?);
            } else {
                let source = crate::shader_doc::canonical_shader_path(&cmd.value);
                if !is_wesl_shader_path(&source) {
                    return Err("Shader assignment requires a standalone .wesl asset".into());
                }
                let path =
                    lunco_assets_core::asset_path::load_source_uri(&source, world.get_resource())
                        .map_err(|error| error.to_string())?
                        .to_string();
                let existing = world.get::<ShaderLook>(target).cloned();
                author_shader_look(&mut world.commands(), target, existing.as_ref(), &path);
            }
        }
        key if PBR_LOOK_KEYS.contains(&key) => {
            let look = edited_pbr_look(world, target, [(key, cmd.value.as_str())])?;
            if persistent {
                let prim = world.get::<UsdPrimPath>(target).ok_or("missing USD prim")?;
                ops.extend(appearance_ops(world, prim, LayerId::root(), &[key], &look)?);
            } else if world.get::<PbrLook>(target) != Some(&look) {
                world.entity_mut(target).insert(look);
            }
        }
        key => {
            let look = world
                .get::<ShaderLook>(target)
                .ok_or("target has no shader look")?;
            let schema = shader_schema(&look.shader, world.resource(), world.resource());
            let name = lunco_materials::to_snake_case(key);
            let value = shader_param_value(schema.as_ref(), &name, &cmd.value)
                .ok_or_else(|| format!("invalid or unavailable shader parameter {key}"))?;
            if persistent {
                let prim = world
                    .get::<UsdPrimPath>(target)
                    .cloned()
                    .ok_or("missing USD prim")?;
                let param = crate::doc_resolve::resolve_shader_parameter_usd_target(
                    world, &prim, &name, &value,
                )?;
                ops.push(UsdOp::SetAttribute {
                    edit_target: LayerId::runtime(),
                    path: param.shader_path,
                    name: param.attribute_name,
                    type_name: param.type_name,
                    value: param.literal,
                });
            } else if let Some(mut look) = world.get_mut::<ShaderLook>(target) {
                look.set_value(&name, value);
            }
        }
    }
    Ok(destination.map(|(doc_id, _)| ApplyUsdOps {
        doc_id,
        parent_gen: None,
        label: format!("Edit {key}"),
        ops,
    }))
}

/// Whether `path` is a standalone WESL asset path accepted by the shader
/// commands. Shader sub-assets are not valid command targets.
fn is_wesl_shader_path(path: &str) -> bool {
    let Ok(path) = lunco_assets_core::asset_path::load_asset_path(path, None, None, None) else {
        return false;
    };
    // Bevy's embedded source contains engine-internal WESL modules, not
    // standalone files a caller can reload or replace. Other registered
    // sources remain valid asset owners and are intentionally not enumerated
    // here; the live AssetServer identity is the authority.
    let editable_source = !matches!(path.source().as_str(), Some("embedded"));
    editable_source
        && path.label().is_none()
        && path
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension == "wesl")
}

/// Find the live asset identities matching one shader request. The returned
/// paths are exactly the paths held by [`AssetServer`], so reloading or
/// replacing one cannot accidentally create a second default-source asset.
fn active_shader_paths(
    asset_server: &AssetServer,
    shaders: &Assets<bevy::shader::Shader>,
    requested: &str,
) -> Vec<String> {
    if requested.is_empty() {
        let mut paths = std::collections::BTreeSet::new();
        for id in shaders.ids() {
            let Some(path) = asset_server.get_path(id) else {
                continue;
            };
            if is_wesl_shader_path(&path.to_string()) {
                paths.insert(path.to_string());
            }
        }
        return paths.into_iter().collect();
    }

    let path = crate::shader_doc::canonical_shader_path(requested);
    if is_wesl_shader_path(&path)
        && asset_server
            .get_path_ids(path.clone())
            .into_iter()
            .any(|id| {
                id.try_typed::<bevy::shader::Shader>()
                    .is_ok_and(|id| shaders.contains(id))
            })
    {
        vec![path]
    } else {
        Vec::new()
    }
}

/// Replace the live `Shader` asset(s) identified by `requested`.
///
/// This is shared by the local command and journal replay. A bare request
/// resolves to the canonical engine asset identity; if it is inactive, it loads
/// the canonical `lunco://` handle so a
/// replayed source edit is still available to the renderer. It does not journal
/// the edit — the caller that owns document persistence does that separately.
pub fn apply_shader_source_live(
    asset_server: &AssetServer,
    shaders: &Assets<bevy::shader::Shader>,
    pending: &mut crate::shader_sources::PendingShaderSources,
    roots: Option<&lunco_assets_core::twin_source::TwinRoots>,
    requested: &str,
    source: &str,
) -> Result<Vec<String>, String> {
    let requested = requested.trim();
    if requested.is_empty() || !is_wesl_shader_path(requested) {
        return Err(format!(
            "shader path must name a standalone .wesl asset, got '{requested}'"
        ));
    }
    if source.is_empty() {
        return Err("shader source must not be empty".to_string());
    }

    let load_address = lunco_assets_core::asset_path::load_source_uri(requested, roots)
        .map_err(|error| error.to_string())?
        .to_string();
    let paths = {
        let active = active_shader_paths(asset_server, shaders, &load_address);
        if active.is_empty() {
            vec![crate::shader_doc::canonical_shader_path(&load_address)]
        } else {
            active
        }
    };

    lunco_materials::validate_shader_syntax(source)?;
    let sources = paths
        .iter()
        .map(|path| {
            lunco_assets_core::asset_path::source_uri_for_address(path, roots)
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    pending.queue(asset_server, roots, &paths, source)?;
    Ok(sources)
}

/// Force-reload live WESL assets from disk so edits apply without restarting
/// the app. Calls [`AssetServer::reload`], which re-runs the loader and lets
/// dependent material pipelines rebuild.
///
/// A supplied bare path such as `"shaders/wheel.wesl"` resolves against the
/// active engine-library identity, including its explicit `lunco://` spelling.
/// A stable `twin://…` source is resolved to its current private load lease. An empty path
/// reloads every currently loaded WESL asset. The command fails visibly when
/// the requested source is not active instead of reporting a successful no-op.
#[Command(default)]
pub struct ReloadShader {
    pub path: String,
}

/// Observer for [`ReloadShader`].
#[on_command(ReloadShader)]
pub fn on_reload_shader(
    trigger: On<ReloadShader>,
    asset_server: Res<AssetServer>,
    shaders: Res<Assets<bevy::shader::Shader>>,
    mut pending: ResMut<crate::shader_sources::PendingShaderSources>,
    roots: Option<Res<lunco_assets_core::TwinRoots>>,
) -> Result<lunco_command_contracts::Ack, String> {
    let requested = trigger.event().path.trim().to_string();
    if !requested.is_empty() && !is_wesl_shader_path(&requested) {
        return Err(format!(
            "shader path must name a standalone .wesl asset, got '{requested}'"
        ));
    }
    let load_address = if requested.is_empty() {
        String::new()
    } else {
        lunco_assets_core::asset_path::load_source_uri(&requested, roots.as_deref())
            .map_err(|error| error.to_string())?
            .to_string()
    };
    let paths = active_shader_paths(&asset_server, &shaders, &load_address);
    let sources = paths
        .iter()
        .map(|path| {
            lunco_assets_core::asset_path::source_uri_for_address(path, roots.as_deref())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if paths.is_empty() {
        return Err(if requested.is_empty() {
            "no loaded WESL shader assets are available to reload".to_string()
        } else {
            format!(
                "no active WESL shader asset matches '{}'; use the exact loaded source path",
                requested
            )
        });
    }

    pending.cancel(&paths);
    for path in &paths {
        // Owned `String` → `AssetPath<'static>`, so the queued reload doesn't
        // borrow the short-lived command trigger.
        asset_server.reload(path.clone());
    }
    info!(
        "RELOAD_SHADER: queued {} active asset(s) for '{}'",
        paths.len(),
        if requested.is_empty() {
            "all"
        } else {
            requested.as_str()
        }
    );
    Ok(lunco_command_contracts::Ack::with_data(
        lunco_command_contracts::OpId::new(),
        lunco_api_core::api_value!({
            "accepted": true,
            "requested": requested,
            "queued_paths": sources,
        }),
    ))
}

/// Replace a shader asset's WESL **source in place** from text sent over the
/// API, recompiling it live without touching disk or restarting. Overwrites the
/// active `Shader` asset(s) at `path` (e.g. `"shaders/wheel.wesl"`), so every
/// material using them re-specializes its pipeline next frame. Bare engine
/// paths resolve to the canonical `lunco://` identity used by [`ReloadShader`].
/// Compile/validation outcome surfaces in the render log (naga errors on a bad
/// shader). Pairs with [`ReloadShader`] (disk) — this one is for pushing edits
/// directly.
#[Command(default)]
pub struct SetShaderSource {
    /// Stable source URI or engine-library path, e.g. `"shaders/wheel.wesl"`.
    pub path: String,
    /// New WESL source text.
    pub source: String,
}

/// Observer for [`SetShaderSource`].
#[on_command(SetShaderSource)]
pub fn on_set_shader_source(
    trigger: On<SetShaderSource>,
    asset_server: Res<AssetServer>,
    shaders: Res<Assets<bevy::shader::Shader>>,
    mut pending: ResMut<crate::shader_sources::PendingShaderSources>,
    roots: Option<Res<lunco_assets_core::twin_source::TwinRoots>>,
    mut registry: ResMut<crate::shader_doc::ShaderRegistry>,
    guard: Option<Res<lunco_core_session::SyncApplyGuard>>,
) -> Result<lunco_command_contracts::Ack, String> {
    let ev = trigger.event();
    let requested = crate::shader_doc::canonical_shader_path(ev.path.trim());
    // Validate and admit the source before recording the edit. Preparation
    // resolves native imports asynchronously; GPU compilation follows publication.
    let paths = apply_shader_source_live(
        &asset_server,
        &shaders,
        &mut pending,
        roots.as_deref(),
        &requested,
        &ev.source,
    )?;
    // Record the edit into the Twin journal (`DomainKind::Shader`) via the shader
    // document registry — so it SYNCS + PERSISTS like a rhai/Modelica edit, not
    // just a local `Assets<Shader>` poke. Skip recording when this arrived from the
    // wire (`SyncApplyGuard` set): the originating peer already journaled it, and
    // the journal replay leg applies + hot-reloads it here — re-recording would
    // duplicate the entry.
    if guard.is_none_or(|g| g.0.is_none()) {
        registry.apply_source(&requested, ev.source.clone(), roots.as_deref())?;
    }
    info!(
        "SET_SHADER_SOURCE: queued {} active asset(s) from {} bytes of WESL",
        paths.len(),
        ev.source.len()
    );
    Ok(lunco_command_contracts::Ack::with_data(
        lunco_command_contracts::OpId::new(),
        lunco_api_core::api_value!({
            "accepted": true,
            "requested": requested,
            "queued_paths": paths,
        }),
    ))
}

// ─────────────────────────────────────────────────────────────────────────
// Live shader authoring — create from a template, import any `.wesl` from the
// computer into the open Twin, and discover shaders dropped in the Twin folder.
// All persist into the owning Twin or engine asset library,
// register into the picker [`ShaderCatalog`], and can apply to an entity — no
// restart. The created/imported shaders are PBR-compatible self-describing
// shaders (see [`lunco_materials::shader_template`]).
// ─────────────────────────────────────────────────────────────────────────

/// Sanitise a free-text name into a safe lowercase file stem (`[a-z0-9_]`,
/// trimmed of leading/trailing `_`). Empty input → `"shader"`.
pub fn sanitize_stem(s: &str) -> String {
    let out: String = s
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "shader".to_string()
    } else {
        out
    }
}

/// Core of [`CreateShader`]/[`ImportShader`]: validate the WESL is a
/// prop-pickable dynamic shader, create a new file in the open Twin (or engine
/// asset root when no Twin is open), register it with the native asset loader
/// and picker, and optionally bind it to `target` (API id; 0 = none).
/// Existing destinations reject. Success returns the stable source URI.
#[allow(clippy::too_many_arguments)]
fn install_shader(
    stem: &str,
    source: &str,
    target: u64,
    twin_roots: Option<&lunco_assets_core::twin_source::TwinRoots>,
    asset_server: &AssetServer,
    pending: &mut crate::shader_sources::PendingShaderSources,
    catalog: &mut lunco_materials::ShaderCatalog,
    registry: &lunco_api::registry::ApiEntityRegistry,
    q_look: &Query<Option<&ShaderLook>>,
    commands: &mut Commands,
) -> Result<String, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (
            stem,
            source,
            target,
            twin_roots,
            asset_server,
            pending,
            catalog,
            registry,
            q_look,
            commands,
        );
        Err("Creating shader files requires a native filesystem".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        lunco_assets_path::validate_portable_file_name(&format!("{stem}.wesl"))
            .map_err(|error| format!("Invalid shader filename: {error}"))?;
        lunco_materials::validate_shader_syntax(source)?;
        if !lunco_materials::is_prop_pickable_source(source) {
            return Err(
                "Shader must declare a Material struct with only prop-fillable engine fields"
                    .into(),
            );
        }
        let target = if target == 0 {
            None
        } else {
            let entity = registry
                .resolve(&lunco_core::GlobalEntityId::from_raw(target))
                .ok_or_else(|| format!("Shader target {target} is not a live entity"))?;
            q_look
                .get(entity)
                .map_err(|_| format!("Shader target {target} is no longer live"))?;
            Some(entity)
        };
        let primary = twin_roots
            .map(|roots| roots.primary())
            .transpose()
            .map_err(|error| error.to_string())?
            .flatten();
        let (load_address, disk_path) = match primary {
            Some((mount, root)) => (
                lunco_assets_core::twin_uri(
                    &mount,
                    lunco_assets_core::engine_shader_asset_rel(stem),
                ),
                root.join(lunco_assets_core::engine_shader_asset_rel(stem)),
            ),
            None => (
                lunco_assets_core::engine_asset_uri(&lunco_assets_core::engine_shader_asset_rel(
                    stem,
                )),
                lunco_assets_core::engine_shader_path(stem).map_err(|error| error.to_string())?,
            ),
        };
        let source_uri =
            lunco_assets_core::asset_path::source_uri_for_address(&load_address, twin_roots)
                .map_err(|error| error.to_string())?;
        let load_path =
            lunco_assets_core::asset_path::load_asset_path(&load_address, None, None, None)
                .map_err(|error| error.to_string())?;
        {
            use lunco_storage::Storage;
            lunco_storage::FileStorage::new()
            .write_new_sync(&lunco_storage::StorageHandle::File(disk_path), source.as_bytes())
            .map_err(|error| format!("Cannot create shader {source_uri}: {error}. Choose a new name or edit the existing file."))?;
        }
        pending.cancel(&[load_address.clone()]);
        let _shader_handle = asset_server.load::<bevy::shader::Shader>(load_path);
        asset_server.reload(load_address.clone());
        catalog.add(load_address.clone());
        if let Some(target) = target {
            author_shader_look(
                commands,
                target,
                q_look.get(target).ok().flatten(),
                &load_address,
            );
        }
        info!("INSTALL_SHADER: registered {source_uri}");
        Ok(source_uri)
    }
}

/// Create a new dynamic shader from a built-in template (or supplied WESL),
/// persist it into the open Twin (`<twin>/shaders/<name>.wesl`, or
/// `assets/shaders/` when no Twin is open), register it in the picker, and
/// optionally bind it to a target entity — all live, no restart.
/// Existing destination files reject without replacement. Success returns
/// `source_uri`, the stable identity of the newly installed source.
///
/// ```json
/// {"type":"ExecuteCommand","command":"CreateShader","params":{"name":"my_panel","template":"checker","target":42}}
/// {"type":"ExecuteCommand","command":"CreateShader","params":{"name":"custom","source":"<wesl...>"}}
/// ```
#[Command(default)]
pub struct CreateShader {
    /// Display name / file stem, e.g. `"my_panel"` (sanitised to `[a-z0-9_]`).
    pub name: String,
    /// Template id when `source` is empty: `"solid"` (default) or `"checker"`.
    pub template: String,
    /// Full WESL source. Empty → generate from `template`.
    pub source: String,
    /// API id of an entity to apply the new shader to. `0` = create only.
    pub target: u64,
}

/// Observer for [`CreateShader`].
#[allow(clippy::too_many_arguments)]
#[on_command(CreateShader)]
pub fn on_create_shader(
    trigger: On<CreateShader>,
    twin_roots: Option<Res<lunco_assets_core::twin_source::TwinRoots>>,
    asset_server: Res<AssetServer>,
    mut pending: ResMut<crate::shader_sources::PendingShaderSources>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    q_look: Query<Option<&ShaderLook>>,
    mut commands: Commands,
) -> Result<lunco_command_contracts::Ack, String> {
    let ev = trigger.event();
    let stem = sanitize_stem(&ev.name);
    let source = if ev.source.trim().is_empty() {
        lunco_materials::shader_template(&ev.template, &stem)
    } else {
        ev.source.clone()
    };
    let source_uri = install_shader(
        &stem,
        &source,
        ev.target,
        twin_roots.as_deref(),
        &asset_server,
        &mut pending,
        &mut catalog,
        &registry,
        &q_look,
        &mut commands,
    )
    .inspect_err(|error| warn!("CREATE_SHADER: {error}"))?;
    Ok(lunco_api::executor::ack_with_string_field(
        lunco_command_contracts::OpId::new(),
        "source_uri",
        source_uri,
    ))
}
/// Import an existing `.wesl` file from anywhere on disk INTO the open Twin
/// (copies it to `<twin>/shaders/<name>.wesl`), registers it in the picker, and
/// optionally binds it to a target entity. The file must be a prop-pickable
/// dynamic shader: a `Material` struct, and every `//!@engine` field it declares
/// must be prop-fillable per the engine-param registry. Existing destination
/// files reject without replacement. Success returns the stable `source_uri`.
///
/// ```json
/// {"type":"ExecuteCommand","command":"ImportShader","params":{"source_path":"/home/me/cool.wesl","name":"cool","target":42}}
/// ```
#[Command(default)]
pub struct ImportShader {
    /// Filesystem path of the `.wesl` to import (absolute or cwd-relative).
    pub source_path: String,
    /// Optional new stem; empty → keep the source file's own stem.
    pub name: String,
    /// API id of an entity to apply the imported shader to. `0` = import only.
    pub target: u64,
}

/// Observer for [`ImportShader`].
#[allow(clippy::too_many_arguments)]
#[on_command(ImportShader)]
pub fn on_import_shader(
    trigger: On<ImportShader>,
    #[cfg(not(target_arch = "wasm32"))] twin_roots: Option<
        Res<lunco_assets_core::twin_source::TwinRoots>,
    >,
    #[cfg(not(target_arch = "wasm32"))] asset_server: Res<AssetServer>,
    #[cfg(not(target_arch = "wasm32"))] mut pending: ResMut<
        crate::shader_sources::PendingShaderSources,
    >,
    #[cfg(not(target_arch = "wasm32"))] mut catalog: ResMut<lunco_materials::ShaderCatalog>,
    #[cfg(not(target_arch = "wasm32"))] registry: Res<lunco_api::registry::ApiEntityRegistry>,
    #[cfg(not(target_arch = "wasm32"))] q_look: Query<Option<&ShaderLook>>,
    #[cfg(not(target_arch = "wasm32"))] mut commands: Commands,
) -> Result<lunco_command_contracts::Ack, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = trigger;
        return Err("Importing shader files requires a native filesystem".into());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let ev = trigger.event();
        let src = lunco_assets_core::read_asset_file_string(std::path::Path::new(&ev.source_path))
            .map_err(|error| format!("Cannot import shader {}: {error}", ev.source_path))?;
        let stem = if ev.name.trim().is_empty() {
            let stem = std::path::Path::new(&ev.source_path)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or("Imported shader path has no filename")?;
            sanitize_stem(stem)
        } else {
            sanitize_stem(&ev.name)
        };
        let source_uri = install_shader(
            &stem,
            &src,
            ev.target,
            twin_roots.as_deref(),
            &asset_server,
            &mut pending,
            &mut catalog,
            &registry,
            &q_look,
            &mut commands,
        )
        .inspect_err(|error| warn!("IMPORT_SHADER: {error}"))?;
        Ok(lunco_api::executor::ack_with_string_field(
            lunco_command_contracts::OpId::new(),
            "source_uri",
            source_uri,
        ))
    }
}

/// Delete a shader: unregister it from the picker [`ShaderCatalog`] and remove
/// its `.wesl` from disk (the twin's `shaders/` folder, or `assets/shaders`).
/// Entities currently using it keep their in-memory material for the session.
///
/// ```json
/// {"type":"ExecuteCommand","command":"DeleteShader","params":{"path":"twin://moonbase/shaders/old.wesl"}}
/// ```
#[Command(default)]
pub struct DeleteShader {
    /// Asset path to remove (`twin://name/shaders/x.wesl` or `shaders/x.wesl`).
    pub path: String,
}

/// Observer for [`DeleteShader`].
#[on_command(DeleteShader)]
pub fn on_delete_shader(
    trigger: On<DeleteShader>,
    #[cfg(not(target_arch = "wasm32"))] schemes: Option<Res<lunco_assets_core::SchemeRegistry>>,
    roots: Option<Res<lunco_assets_core::TwinRoots>>,
    mut pending: ResMut<crate::shader_sources::PendingShaderSources>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
) -> Result<lunco_command_contracts::Ack, String> {
    let source = trigger.event().path.trim();
    if source.is_empty() || !is_wesl_shader_path(source) {
        return Err("DeleteShader requires a standalone .wesl source".into());
    }
    let path = lunco_assets_core::asset_path::load_source_uri(source, roots.as_deref())
        .map_err(|error| error.to_string())?
        .to_string();
    #[cfg(target_arch = "wasm32")]
    return Err("Deleting shader files requires a native filesystem".into());
    #[cfg(not(target_arch = "wasm32"))]
    {
        let disk = schemes
            .ok_or("asset scheme registry unavailable")?
            .local_path(&path)
            .map_err(|error| error.to_string())?
            .ok_or("shader source has no local file")?;
        lunco_storage::delete_file_sync(&disk).map_err(|error| error.to_string())?;
        pending.cancel(&[crate::shader_doc::canonical_shader_path(&path)]);
        catalog.remove(&crate::shader_doc::canonical_shader_path(&path));
        info!("DELETE_SHADER: removed {source}");
        Ok(lunco_command_contracts::Ack::default())
    }
}
