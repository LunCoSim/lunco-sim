//! Scene property and shader authoring commands.
//!
//! This module owns the generic `SetObjectProperty` surface, standard USD
//! property persistence, and live shader source/create/import/delete commands.
//! The command plugin is installed by `lunco-scene-commands` so mutation and
//! authoring remain separate production packages.

use bevy::prelude::*;
use lunco_core::{on_command, Command};
use lunco_doc_bevy::DocumentRegistry;
use lunco_materials::{ParamSchema, ParamValue, ShaderLook};
use lunco_render::{PbrLook, SurfaceAlpha};
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_usd_core::commands::ApplyUsdOp;
use lunco_usd_core::document::{LayerId, UsdDocument, UsdOp};

/// One wheel-dynamics parameter — **the** single source of truth for it.
///
/// A wheel param has exactly three facets and they must never drift apart:
/// the names `SetObjectProperty` accepts for it, the live `WheelRaycast` field
/// it sets, and the USD attribute `lunco_usd_sim` reads back onto that field on
/// load. Two hand-synced tables (a `name → setter` match and a separate
/// `name → attr` match) had already drifted — `slip_stiffness` / `friction_mu`
/// were settable but not persistable, so tuning them was silently lost on
/// reload. One row per param makes that structurally impossible: a field cannot
/// exist in one table and not the other, because there is only one table.
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

/// Persist a `SetObjectProperty` **wheel-dynamics** or **visibility** tune into
/// the active USD document's runtime overlay — the
/// counterpart of the shader-parameter authoring path for the property classes
/// it skips. Fully decoupled + disjoint: it authors wheel-param names (via
/// [`wheel_param`]) or `visible` (standard USD `token visibility`). PBR intent
/// is authored by the command handler through the canonical UsdPreviewSurface
/// path below; keeping it out of this observer avoids two USD representations for
/// one property.
pub fn persist_wheel_to_runtime_layer(
    trigger: On<SetObjectProperty>,
    api_registry: Res<lunco_api::registry::ApiEntityRegistry>,
    usd_registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    mut commands: Commands,
) {
    let cmd = trigger.event();

    // Route the property to a USD attribute the loader reads back.
    let authored: Option<(String, &str, String)> = if let Some(param) = wheel_param(&cmd.property) {
        // Wheel dynamics → the single `WHEEL_PARAMS` row's USD attribute.
        cmd.value
            .trim()
            .parse::<f32>()
            .ok()
            .map(|v| (param.usd_attr.to_string(), "float", v.to_string()))
    } else if matches!(
        cmd.property.as_str(),
        "rest_length" | "spring_k" | "damping_c"
    ) {
        // `springStrength` / `springDamperRate` are NVIDIA's canonical
        // PhysxVehicleSuspensionAPI names; `restLength` has no PhysX
        // equivalent, so it lives under the lunco: namespace.
        let usd_attr = match cmd.property.as_str() {
            "rest_length" => "lunco:suspension:restLength",
            "spring_k" => "physxVehicleSuspension:springStrength",
            "damping_c" => "physxVehicleSuspension:springDamperRate",
            _ => unreachable!(),
        };
        cmd.value
            .trim()
            .parse::<f32>()
            .ok()
            .map(|v| (usd_attr.to_string(), "float", v.to_string()))
    } else if cmd.property == "visible" {
        // Visibility → standard USD `token visibility`, which the prim
        // instantiator already reads back (`inherited` / `invisible`), so a
        // hide survives reload instead of being a live-only ECS `Visibility`
        // write. A `token` literal is QUOTED in USD.
        let hidden = matches!(cmd.value.trim(), "false" | "0" | "hidden");
        let tok = if hidden { "invisible" } else { "inherited" };
        Some(("visibility".to_string(), "token", format!("\"{tok}\"")))
    } else {
        None
    };
    let Some((name, type_name, value)) = authored else {
        return;
    };

    let Some(workspace) = workspace else { return };
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = api_registry.resolve(&global_id) else {
        return;
    };
    let Some((doc, path)) =
        crate::doc_resolve::authorable_prim(target, &q_prim, &usd_registry, Some(&*workspace))
    else {
        return;
    };

    commands.trigger(ApplyUsdOp {
        doc_id: doc,
        parent_gen: None,
        op: UsdOp::SetAttribute {
            edit_target: LayerId::runtime(),
            path,
            name,
            type_name: type_name.to_string(),
            value,
        },
    });
}

/// Set a property on a scene object at runtime (live override — not persisted
/// to USD). One general command instead of many narrow ones; new properties
/// just add a `match` arm. Drive it from curl after a screenshot to iterate:
///
/// ```jsonc
/// {"type":"ExecuteCommand","command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"shader","value":"shaders/balloon.wgsl"}}
/// {"type":"ExecuteCommand","command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"wedge_count","value":"12"}}
/// {"type":"ExecuteCommand","command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"cell_a","value":"0.1,0.8,0.2"}}
/// ```
///
/// Recognised `property` values:
/// - `shader` → author a [`ShaderLook`] for that `.wgsl` (asset path); the render
///   binder turns it into a material.
/// - any parameter named by the shader's `Material` struct (e.g. `albedo`,
///   `wedge_count`, `cell_a`) → set that named value on the entity's `ShaderLook`
///   (requires `shader` set first, or a USD shader material). The shader's
///   reflected schema resolves the type; colours are `r,g,b`.
/// - `visible` → `true`/`false` toggles `Visibility`.
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

/// Apply one PBR property addressed by `SetObjectProperty` to a [`PbrLook`] —
/// appearance **intent**, no material asset touched.
///
/// Value formats: colors are comma-separated **linear** `r,g,b[,a]` in 0..1 (so they
/// round-trip the Inspector's `color_edit_button_rgb`); scalars a single float;
/// booleans `true`/`1`/`yes`/`on`. Only the keys in [`PBR_LOOK_KEYS`] are understood;
/// anything else returns `false`.
/// Author a `PbrLook` edit into the USD document, so a material change persists,
/// journals, undoes and replicates like every other edit.
///
/// The look's USD home is a `UsdPreviewSurface` Shader reached through the geom's
/// `material:binding`. If the prim has no material yet, one is created
/// (`ensure_preview_surface_ops` — Looks scope + Material + Shader + binding) and
/// EVERY input is seeded from the current look, not just the edited one: a
/// freshly-created material must reproduce what is on screen, rather than snapping
/// the untouched channels to `UsdPreviewSurface`'s defaults.
///
/// `double_sided` is deliberately NOT a shader input — it is `uniform bool
/// doubleSided` on `UsdGeomGprim`, a property of the geometry — so it is authored
/// on the geom prim instead. `unlit` is render-only intent with no USD equivalent
/// (see [`lunco_usd_core::material::preview_surface_input`]) — it is the one knob a saved
/// scene will not carry, deliberately.
fn author_look_to_usd(commands: &mut Commands, target: Entity, key: &str, look: &PbrLook) {
    let look = look.clone();
    let key = key.to_string();
    commands.queue(move |world: &mut World| {
        let Some(doc) = crate::doc_resolve::resolve_doc_for_entity(world, target) else {
            return;
        };
        let Some(prim) = world.get::<UsdPrimPath>(target).cloned() else {
            return;
        };

        // `doubleSided` lives on the geometry, not the surface.
        if key == "double_sided" {
            world.trigger(ApplyUsdOp {
                doc_id: doc,
                parent_gen: None,
                op: UsdOp::SetAttribute {
                    edit_target: LayerId::root(),
                    path: prim.path.clone(),
                    name: "doubleSided".into(),
                    type_name: "bool".into(),
                    value: look.double_sided.to_string(),
                },
            });
            return;
        }
        if lunco_usd_core::material::preview_surface_input(&key).is_none() {
            return; // `unlit` — render-only intent, no USD surface input to write.
        }

        // An existing bound shader, else create the material.
        let existing = crate::doc_resolve::bound_shader_prim(world, &prim);
        let (mut ops, shader, fresh) = match existing {
            Some(sp) => (Vec::new(), sp, false),
            None => {
                let schemas = crate::doc_resolve::geom_api_schemas(world, &prim);
                match lunco_usd_core::material::ensure_preview_surface_ops(
                    LayerId::root(),
                    &prim.path,
                    &schemas,
                ) {
                    Some((ops, shader)) => (ops, shader, true),
                    None => return,
                }
            }
        };

        let mut set = |attr: &str, ty: &str, value: String| {
            ops.push(UsdOp::SetAttribute {
                edit_target: LayerId::root(),
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
            if !fresh && !key_matches(&key, k) {
                continue;
            }
            if let Some((attr, _)) = lunco_usd_core::material::preview_surface_input(k) {
                set(attr, ty, v);
            }
        }
        for op in ops {
            world.trigger(ApplyUsdOp {
                doc_id: doc,
                parent_gen: None,
                op: op.clone(),
            });
        }
    });
}

/// Whether the edited look key names the same `UsdPreviewSurface` input as `slot`
/// (`roughness` and `alpha` are the canonical command keys).
fn key_matches(key: &str, slot: &str) -> bool {
    lunco_usd_core::material::preview_surface_input(key)
        == lunco_usd_core::material::preview_surface_input(slot)
}

fn apply_pbr_look(look: &mut PbrLook, key: &str, value: &str) -> bool {
    let f: Vec<f32> = value
        .split(',')
        .filter_map(|s| s.trim().parse::<f32>().ok())
        .collect();
    let parse_bool = |v: &str| matches!(v.trim(), "true" | "1" | "yes" | "on");
    match key {
        "base_color" => {
            if f.len() < 3 {
                return false;
            }
            let a = f.get(3).copied().unwrap_or(look.base_color.alpha);
            look.base_color = LinearRgba::new(f[0], f[1], f[2], a);
        }
        "emissive" => {
            if f.len() < 3 {
                return false;
            }
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
        "unlit" => look.unlit = parse_bool(value),
        "double_sided" => look.double_sided = parse_bool(value),
        _ => return false,
    }
    true
}

/// The reflected parameter schema of a shader **asset path**.
///
/// Read straight out of the loaded WGSL source (`Material` struct + `//!@`
/// annotations) rather than off a material — the schema is a property of the
/// *asset*, and reading it this way keeps the shader-param paths render-free.
/// `None` while the shader is still loading (or if it declares no `Material`) is
/// an unavailable edit target, not permission to infer a type.
fn shader_schema(
    path: &str,
    asset_server: &AssetServer,
    shaders: &Assets<bevy::shader::Shader>,
) -> Option<ParamSchema> {
    let handle = asset_server.load::<bevy::shader::Shader>(path.to_string());
    let src = match &shaders.get(&handle)?.source {
        bevy::shader::Source::Wgsl(s) => s.as_ref().to_string(),
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

/// Queue the authored leg of a reflected shader-parameter edit.
///
/// `SetObjectProperty` owns the public command and immediate intent update;
/// this deferred leg resolves the entity's explicit USD document and bound
/// Shader, then submits the same typed USD operation used by the Inspector.
/// Entities without a document remain session-only because they have no USD
/// owner for persistence.
fn author_shader_parameter_to_usd(
    commands: &mut Commands,
    target: Entity,
    name: String,
    value: ParamValue,
) {
    commands.queue(move |world: &mut World| {
        let Some(prim) = world.get::<UsdPrimPath>(target).cloned() else {
            return;
        };
        let Some(doc) = crate::doc_resolve::resolve_doc_for_entity(world, target) else {
            return;
        };
        let target = match crate::doc_resolve::resolve_shader_parameter_usd_target(
            world, &prim, &name, &value,
        ) {
            Ok(target) => target,
            Err(error) => {
                warn!(
                    "SET_PROPERTY: shader parameter '{}' was not authored: {error}",
                    name
                );
                return;
            }
        };
        world.trigger(ApplyUsdOp {
            doc_id: doc,
            parent_gen: None,
            op: UsdOp::SetAttribute {
                edit_target: LayerId::runtime(),
                path: target.shader_path,
                name: target.attribute_name,
                type_name: target.type_name,
                value: target.literal,
            },
        });
    });
}

/// Give `target` a [`ShaderLook`] for `shader_path`, carrying over any parameters
/// it already has so swapping the `.wgsl` keeps tuned values.
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
    commands.queue(move |world: &mut World| drop_bound_pbr_material(world, target));
}

/// Drop the concrete PBR material a render build already bound to `e`.
///
/// Removing the [`PbrLook`] *intent* stops the binder re-materialising the entity,
/// but the `MeshMaterial3d<StandardMaterial>` it inserted earlier stays put — and a
/// mesh carrying that AND the shader material draws twice. That component is
/// `bevy_pbr`'s and this crate may not name it (render-decoupling rule), so it is
/// resolved out of the type registry instead (`MaterialPlugin` registers it, and it
/// is `#[reflect(Component)]`).
///
/// No-op headless and in tests, where nothing ever bound a material — and a no-op the
/// day `lunco-render-bevy` grows an `On<Remove, PbrLook>` observer that unbinds its
/// own material, which is where this really belongs.
pub fn drop_bound_pbr_material(world: &mut World, e: Entity) {
    let Some(registry) = world.get_resource::<AppTypeRegistry>().cloned() else {
        return;
    };
    let reflect_component = {
        let reg = registry.read();
        reg.get_with_short_type_path("MeshMaterial3d<StandardMaterial>")
            .and_then(|r| r.data::<bevy::ecs::reflect::ReflectComponent>())
            .cloned()
    };
    let Some(rc) = reflect_component else { return };
    if let Ok(mut entity) = world.get_entity_mut(e) {
        rc.remove(&mut entity);
    }
}

/// Observer for [`SetObjectProperty`].
#[on_command(SetObjectProperty)]
pub fn on_set_object_property(
    trigger: On<SetObjectProperty>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    asset_server: Res<AssetServer>,
    shaders: Res<Assets<bevy::shader::Shader>>,
    mut q_look: Query<&mut PbrLook>,
    mut q_shader_look: Query<&mut ShaderLook>,
    q_mesh: Query<(), With<Mesh3d>>,
    mut q_vis: Query<&mut Visibility>,
    mut q_wheel: Query<&mut lunco_mobility::WheelRaycast>,
    mut q_susp: Query<&mut lunco_mobility::Suspension>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("SET_PROPERTY: no api_id={} in registry", cmd.entity_id);
        return;
    };

    // Per-wheel suspension tuning (both joint-based and raycast).
    match cmd.property.as_str() {
        "rest_length" | "spring_k" | "damping_c" => {
            let Ok(value) = cmd.value.trim().parse::<f64>() else {
                warn!(
                    "SET_PROPERTY: '{}' expects a number, got '{}'",
                    cmd.property, cmd.value
                );
                return;
            };
            let Ok(mut susp) = q_susp.get_mut(target) else {
                warn!(
                    "SET_PROPERTY: entity {} has no Suspension component",
                    cmd.entity_id
                );
                return;
            };
            match cmd.property.as_str() {
                "rest_length" => {
                    susp.rest_length = value;
                }
                "spring_k" => {
                    susp.spring_k = value;
                }
                "damping_c" => {
                    susp.damping_c = value;
                }
                _ => {}
            }
            info!(
                "SET_PROPERTY: suspension {} {} = {}",
                cmd.entity_id, cmd.property, value
            );
            return;
        }
        _ => {}
    }

    // Per-wheel tire-spin dynamics. Each wheel is its own entity, so addressing
    // a single `api_id` sets the field on just that wheel — independent control.
    if let Some(param) = wheel_param(&cmd.property) {
        let Ok(value) = cmd.value.trim().parse::<f64>() else {
            warn!(
                "SET_PROPERTY: '{}' expects a number, got '{}'",
                cmd.property, cmd.value
            );
            return;
        };
        let Ok(mut wheel) = q_wheel.get_mut(target) else {
            warn!("SET_PROPERTY: entity {} has no WheelRaycast", cmd.entity_id);
            return;
        };
        (param.set)(&mut wheel, value);
        info!(
            "SET_PROPERTY: wheel {} {} = {}",
            cmd.entity_id, cmd.property, value
        );
        return;
    }

    match cmd.property.as_str() {
        "shader" => {
            // Preserve existing uniforms if the object already has a shader look,
            // so swapping the .wgsl keeps tuned params.
            let existing = q_shader_look.get(target).ok().cloned();
            author_shader_look(&mut commands, target, existing.as_ref(), &cmd.value);
            info!("SET_PROPERTY: {} shader = {}", cmd.entity_id, cmd.value);
        }
        "visible" => {
            let Ok(mut vis) = q_vis.get_mut(target) else {
                warn!("SET_PROPERTY: entity {} has no Visibility", cmd.entity_id);
                return;
            };
            let v = cmd.value.trim();
            *vis = if matches!(v, "false" | "0" | "hidden") {
                Visibility::Hidden
            } else {
                Visibility::Visible
            };
        }
        // PBR properties — for props/rovers on a plain surface rather than a custom
        // shader. Explicit arm ([`PBR_LOOK_KEYS`]) so these names never get stolen by
        // the shader-param fallback below.
        //
        // The edit is a mutation of the entity's `PbrLook` *intent* component: the
        // render binder's `Changed<PbrLook>` system re-materialises it, so "edit the
        // material" is just "mutate a component" — no asset handles, and it works
        // headless (the intent is in the world; nothing binds it). A mesh with no
        // intent yet (a glTF import that brought its own material) is ADOPTED into an
        // intent, which is the only render-free way to keep this command working on
        // it; note that adoption starts from `PbrLook::default()`, so the import's own
        // textures are not carried over.
        key if PBR_LOOK_KEYS.contains(&key) => {
            if let Ok(mut look) = q_look.get_mut(target) {
                if apply_pbr_look(&mut look, key, &cmd.value) {
                    // ALSO author it into USD. Mutating `PbrLook` alone updates the
                    // screen and nothing else — the edit would never reach the
                    // document, so it would not save, journal, undo, or replicate.
                    // Every edit goes through `ApplyUsdOp`; this one was quietly
                    // exempt.
                    author_look_to_usd(&mut commands, target, key, &look);
                    info!(
                        "SET_PROPERTY: {} look {} = {}",
                        cmd.entity_id, cmd.property, cmd.value
                    );
                } else {
                    warn!(
                        "SET_PROPERTY: bad value '{}' for pbr '{}'",
                        cmd.value, cmd.property
                    );
                }
                return;
            }
            if q_mesh.get(target).is_err() {
                warn!(
                    "SET_PROPERTY: entity {} has no PbrLook / mesh",
                    cmd.entity_id
                );
                return;
            }
            let mut look = PbrLook::default();
            if apply_pbr_look(&mut look, key, &cmd.value) {
                author_look_to_usd(&mut commands, target, key, &look);
                commands.entity(target).try_insert(look);
                info!(
                    "SET_PROPERTY: {} adopted a PbrLook, {} = {}",
                    cmd.entity_id, cmd.property, cmd.value
                );
            } else {
                warn!(
                    "SET_PROPERTY: bad value '{}' for pbr '{}'",
                    cmd.value, cmd.property
                );
            }
        }
        key => {
            // param/color → set the named value on the entity's shader look. The
            // binder swaps in the material for the new look (`Changed<ShaderLook>`).
            let Ok(mut look) = q_shader_look.get_mut(target) else {
                warn!(
                    "SET_PROPERTY: entity {} has no shader look — set 'shader' first",
                    cmd.entity_id
                );
                return;
            };
            // USD authors params camelCase, WGSL declares them snake_case.
            let name = lunco_materials::to_snake_case(key);
            let schema = shader_schema(&look.shader, &asset_server, &shaders);
            match shader_param_value(schema.as_ref(), &name, &cmd.value) {
                Some(v) => {
                    look.values.insert(name.clone(), v);
                    drop(look);
                    author_shader_parameter_to_usd(&mut commands, target, name, v);
                }
                None => warn!("SET_PROPERTY: unknown property '{}'", key),
            }
        }
    }
}

/// Force-reload shader assets from disk so live WGSL edits apply without
/// restarting the app. Bypasses the file watcher (unreliable in this build):
/// calls [`AssetServer::reload`], which re-runs the loader and triggers
/// dependent material pipelines to rebuild. Empty `path` → reload the standard
/// `assets/shaders/*` set; otherwise reload just that path (e.g.
/// `"shaders/wheel.wgsl"`).
#[Command(default)]
pub struct ReloadShader {
    pub path: String,
}

/// Observer for [`ReloadShader`].
#[on_command(ReloadShader)]
pub fn on_reload_shader(trigger: On<ReloadShader>, asset_server: Res<AssetServer>) {
    let p = trigger.event().path.trim().to_string();
    let paths: Vec<String> = if p.is_empty() {
        [
            "shaders/wheel.wgsl",
            "shaders/balloon.wgsl",
            "shaders/solar_panel.wgsl",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    } else {
        vec![p]
    };
    for path in paths {
        // Owned `String` → `AssetPath<'static>`, so the queued reload doesn't
        // borrow the (short-lived) trigger.
        asset_server.reload(path.clone());
        info!("RELOAD_SHADER: {}", path);
    }
}

/// Replace a shader asset's WGSL **source in place** from text sent over the
/// API, recompiling it live without touching disk or restarting. Overwrites the
/// `Shader` asset currently at `path` (e.g. `"shaders/wheel.wgsl"`), so every
/// material using it re-specializes its pipeline next frame. Compile/validation
/// outcome surfaces in the render log (naga errors on a bad shader). Pairs with
/// [`ReloadShader`] (disk) — this one is for pushing edits directly.
#[Command(default)]
pub struct SetShaderSource {
    /// Asset path of the shader to overwrite, e.g. `"shaders/wheel.wgsl"`.
    pub path: String,
    /// New WGSL source text.
    pub source: String,
}

/// Observer for [`SetShaderSource`].
#[on_command(SetShaderSource)]
pub fn on_set_shader_source(
    trigger: On<SetShaderSource>,
    asset_server: Res<AssetServer>,
    mut shaders: ResMut<Assets<bevy::shader::Shader>>,
    mut registry: ResMut<crate::shader_doc::ShaderRegistry>,
    guard: Option<Res<lunco_core::session::SyncApplyGuard>>,
) {
    let ev = trigger.event();
    if ev.path.is_empty() || ev.source.is_empty() {
        warn!("SET_SHADER_SOURCE: empty path or source");
        return;
    }
    // Record the edit into the Twin journal (`DomainKind::Shader`) via the shader
    // document registry — so it SYNCS + PERSISTS like a rhai/Modelica edit, not
    // just a local `Assets<Shader>` poke. Skip recording when this arrived from the
    // wire (`SyncApplyGuard` set): the originating peer already journaled it, and
    // the journal replay leg applies + hot-reloads it here — re-recording would
    // duplicate the entry.
    if guard.is_none_or(|g| g.0.is_none()) {
        registry.apply_source(&ev.path, ev.source.clone());
    }
    // Hot-reload: `load` returns the handle every material already holds, so
    // overwriting that asset id propagates the recompile to them.
    let handle = asset_server.load::<bevy::shader::Shader>(ev.path.clone());
    let shader = bevy::shader::Shader::from_wgsl(ev.source.clone(), ev.path.clone());
    let _ = shaders.insert(handle.id(), shader);
    info!(
        "SET_SHADER_SOURCE: recompiled {} from {} bytes of WGSL",
        ev.path,
        ev.source.len()
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Live shader authoring — create from a template, import any `.wgsl` from the
// computer into the open Twin, and discover shaders dropped in the Twin folder.
// All persist into `<twin>/shaders/<name>.wgsl` (fallback `assets/shaders/`),
// register into the picker [`ShaderCatalog`], and can apply to an entity — no
// restart. The created/imported shaders are PBR-compatible self-describing
// shaders (see [`lunco_materials::shader_template`]).
// ─────────────────────────────────────────────────────────────────────────

/// The asset path a shader named `stem` would be installed at: under the
/// primary open Twin (`twin://<name>/shaders/<stem>.wgsl`) or the engine library
/// (`shaders/<stem>.wgsl`) when no Twin is open. Mirrors [`install_shader`]'s
/// destination logic so callers (e.g. the Inspector) can predict the path.
pub fn shader_asset_path_for(
    twin_roots: Option<&lunco_assets::twin_source::TwinRoots>,
    stem: &str,
) -> Result<String, lunco_assets::TwinRootsError> {
    Ok(
        match twin_roots.map(|t| t.primary()).transpose()?.flatten() {
            Some((name, _)) => lunco_assets::twin_uri(&name, format!("shaders/{stem}.wgsl")),
            None => format!("shaders/{stem}.wgsl"),
        },
    )
}

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

/// Core of [`CreateShader`]/[`ImportShader`]: validate the WGSL is a
/// prop-pickable dynamic shader, persist it into the open Twin (fallback
/// `assets/shaders/`), insert it live into [`Assets<Shader>`] so it renders
/// this frame, register it in the picker [`ShaderCatalog`], and optionally bind
/// it to `target` (API id; 0 = none). Returns the asset path on success.
#[allow(clippy::too_many_arguments)]
fn install_shader(
    stem: &str,
    source: &str,
    target: u64,
    twin_roots: Option<&lunco_assets::twin_source::TwinRoots>,
    asset_server: &AssetServer,
    shaders: &mut Assets<bevy::shader::Shader>,
    catalog: &mut lunco_materials::ShaderCatalog,
    registry: &lunco_api::registry::ApiEntityRegistry,
    q_look: &Query<&ShaderLook>,
    commands: &mut Commands,
) -> Option<String> {
    // Gate: must be a self-describing `Material` shader, and every `//!@engine`
    // field it declares must be one a plain prop entity actually receives —
    // `prop_fillable` in the engine-param registry. Otherwise it would render
    // black (e.g. terrain-only inputs) / can't be driven.
    if !lunco_materials::is_prop_pickable_source(source) {
        warn!(
            "INSTALL_SHADER: '{stem}' is not a prop-pickable dynamic shader \
             (needs a `Material` struct; every `//!@engine` field must be \
             prop-fillable per the engine-param registry) — skipped"
        );
        return None;
    }

    // Destination: the primary open Twin's `shaders/` dir (portable, persists
    // with the Twin under a `twin://` asset path), else the engine library.
    let primary = match twin_roots
        .map(|t| t.primary())
        .transpose()
        .map(|primary| primary.flatten())
    {
        Ok(primary) => primary,
        Err(error) => {
            error!("INSTALL_SHADER: Twin registry unavailable: {error}");
            return None;
        }
    };
    let (asset_path, disk_path): (String, std::path::PathBuf) = match primary {
        Some((name, root)) => (
            lunco_assets::twin_uri(&name, format!("shaders/{stem}.wgsl")),
            root.join("shaders").join(format!("{stem}.wgsl")),
        ),
        None => (
            format!("shaders/{stem}.wgsl"),
            lunco_assets::assets_dir_abs()
                .join("shaders")
                .join(format!("{stem}.wgsl")),
        ),
    };

    // Persist to disk (native). Non-fatal on failure — the in-memory insert
    // below still makes it usable this session.
    #[cfg(not(target_arch = "wasm32"))]
    {
        match lunco_storage::write_file_sync(&disk_path, source.as_bytes()) {
            Ok(()) => info!("INSTALL_SHADER: wrote {}", disk_path.display()),
            Err(e) => warn!("INSTALL_SHADER: write {} failed: {e}", disk_path.display()),
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = &disk_path;

    // Insert the compiled source live under the asset path, so any material
    // bound to it renders immediately (no disk round-trip / watcher wait).
    let shader_handle = asset_server.load::<bevy::shader::Shader>(asset_path.clone());
    let shader = bevy::shader::Shader::from_wgsl(source.to_string(), asset_path.clone());
    let _ = shaders.insert(shader_handle.id(), shader);

    // Make it pickable.
    catalog.add(asset_path.clone());

    // Optionally apply to a target entity (preserve any existing shader params).
    if target != 0 {
        let gid = lunco_core::GlobalEntityId::from_raw(target);
        match registry.resolve(&gid) {
            Some(ent) => {
                // Intent, not material: the binder loads the same `asset_path` we
                // just inserted the compiled source under, so it renders at once.
                author_shader_look(commands, ent, q_look.get(ent).ok(), &asset_path);
                info!("INSTALL_SHADER: applied {asset_path} to entity {target}");
            }
            None => warn!("INSTALL_SHADER: target id {target} not in registry"),
        }
    }

    info!("INSTALL_SHADER: registered {asset_path}");
    Some(asset_path)
}

/// Create a new dynamic shader from a built-in template (or supplied WGSL),
/// persist it into the open Twin (`<twin>/shaders/<name>.wgsl`, or
/// `assets/shaders/` when no Twin is open), register it in the picker, and
/// optionally bind it to a target entity — all live, no restart.
///
/// ```json
/// {"type":"ExecuteCommand","command":"CreateShader","params":{"name":"my_panel","template":"checker","target":42}}
/// {"type":"ExecuteCommand","command":"CreateShader","params":{"name":"custom","source":"<wgsl...>"}}
/// ```
#[Command(default)]
pub struct CreateShader {
    /// Display name / file stem, e.g. `"my_panel"` (sanitised to `[a-z0-9_]`).
    pub name: String,
    /// Template id when `source` is empty: `"solid"` (default) or `"checker"`.
    pub template: String,
    /// Full WGSL source. Empty → generate from `template`.
    pub source: String,
    /// API id of an entity to apply the new shader to. `0` = create only.
    pub target: u64,
}

/// Observer for [`CreateShader`].
#[allow(clippy::too_many_arguments)]
#[on_command(CreateShader)]
pub fn on_create_shader(
    trigger: On<CreateShader>,
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    asset_server: Res<AssetServer>,
    mut shaders: ResMut<Assets<bevy::shader::Shader>>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    q_look: Query<&ShaderLook>,
    mut commands: Commands,
) {
    let ev = trigger.event();
    let stem = sanitize_stem(&ev.name);
    let source = if ev.source.trim().is_empty() {
        lunco_materials::shader_template(&ev.template, &stem)
    } else {
        ev.source.clone()
    };
    install_shader(
        &stem,
        &source,
        ev.target,
        twin_roots.as_deref(),
        &asset_server,
        &mut shaders,
        &mut catalog,
        &registry,
        &q_look,
        &mut commands,
    );
}

/// Import an existing `.wgsl` file from anywhere on disk INTO the open Twin
/// (copies it to `<twin>/shaders/<name>.wgsl`), registers it in the picker, and
/// optionally binds it to a target entity. The file must be a prop-pickable
/// dynamic shader: a `Material` struct, and every `//!@engine` field it declares
/// must be prop-fillable per the engine-param registry.
///
/// ```json
/// {"type":"ExecuteCommand","command":"ImportShader","params":{"source_path":"/home/me/cool.wgsl","name":"cool","target":42}}
/// ```
#[Command(default)]
pub struct ImportShader {
    /// Filesystem path of the `.wgsl` to import (absolute or cwd-relative).
    pub source_path: String,
    /// Optional new stem; empty → keep the source file's own stem.
    pub name: String,
    /// API id of an entity to apply the imported shader to. `0` = import only.
    pub target: u64,
}

/// Observer for [`ImportShader`].
#[allow(clippy::too_many_arguments, unused_variables, unused_mut)]
#[on_command(ImportShader)]
pub fn on_import_shader(
    trigger: On<ImportShader>,
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    asset_server: Res<AssetServer>,
    mut shaders: ResMut<Assets<bevy::shader::Shader>>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    q_look: Query<&ShaderLook>,
    mut commands: Commands,
) {
    let ev = trigger.event();
    #[cfg(target_arch = "wasm32")]
    {
        warn!("IMPORT_SHADER: importing from a local file is native-only");
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let src = match lunco_assets::read_asset_file_string(std::path::Path::new(&ev.source_path))
        {
            Ok(s) => s,
            Err(e) => {
                warn!("IMPORT_SHADER: read '{}' failed: {e}", ev.source_path);
                return;
            }
        };
        let stem = if ev.name.trim().is_empty() {
            std::path::Path::new(&ev.source_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(sanitize_stem)
                .unwrap_or_else(|| "shader".to_string())
        } else {
            sanitize_stem(&ev.name)
        };
        install_shader(
            &stem,
            &src,
            ev.target,
            twin_roots.as_deref(),
            &asset_server,
            &mut shaders,
            &mut catalog,
            &registry,
            &q_look,
            &mut commands,
        );
    }
}

/// Delete a shader: unregister it from the picker [`ShaderCatalog`] and remove
/// its `.wgsl` from disk (the twin's `shaders/` folder, or `assets/shaders`).
/// Entities currently using it keep their in-memory material for the session.
///
/// ```json
/// {"type":"ExecuteCommand","command":"DeleteShader","params":{"path":"twin://moonbase/shaders/old.wgsl"}}
/// ```
#[Command(default)]
pub struct DeleteShader {
    /// Asset path to remove (`twin://name/shaders/x.wgsl` or `shaders/x.wgsl`).
    pub path: String,
}

/// Observer for [`DeleteShader`].
#[allow(unused_variables)]
#[on_command(DeleteShader)]
pub fn on_delete_shader(
    trigger: On<DeleteShader>,
    schemes: Option<Res<lunco_assets::SchemeRegistry>>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
) {
    let path = trigger.event().path.trim().to_string();
    if path.is_empty() {
        warn!("DELETE_SHADER: empty path");
        return;
    }
    let removed = catalog.remove(&path);
    // `twin://<name>/<rel>` → the Twin root, a bare `shaders/foo.wgsl` → the
    // shipped library: both are the registry's job, so this crate re-derives
    // neither root (a copy here once joined a bare relative `"assets"`, resolving
    // against the CWD instead of the library path the loader uses).
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(schemes) = schemes.as_ref() {
        match schemes.local_path(&path) {
            Ok(Some(disk)) => match lunco_storage::delete_file_sync(&disk) {
                Ok(()) => info!("DELETE_SHADER: removed {path} ({})", disk.display()),
                Err(e) => warn!("DELETE_SHADER: unregistered {path}, file remove failed: {e}"),
            },
            Ok(None) => {}
            Err(error) => error!("DELETE_SHADER: asset scheme registry unavailable: {error}"),
        }
    }
    if !removed {
        warn!("DELETE_SHADER: '{path}' was not in the catalog");
    }
}
