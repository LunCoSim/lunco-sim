//! Shader uniforms as connection targets — WGSL-defined parameters driven from
//! USD `.connect` wires.
//!
//! A custom shader declares its parameters in WGSL (`struct Material { … }`) and
//! a prim binds them in USD. Constants are authored inline
//! (`float inputs:glow = 0.2`); a value that must FOLLOW the simulation is
//! authored as a connection, exactly like every other port in the engine:
//!
//! ```usda
//! def Mesh "LegPX_Strut" (prepend apiSchemas = ["MaterialBindingAPI"])
//! {
//!     rel material:binding = </Looks/StrutMat>
//!     float inputs:glow.connect = </DescentLander/LegPX_Spring.outputs:force>
//! }
//! ```
//!
//! **This module adds no resolver and no per-frame system.** `rewire_usd_connections`
//! already turns any `inputs:foo.connect` on any prim entity into a `SimConnection`
//! with no check on what kind of thing the target is, and `propagate_connections`
//! already routes every write through [`PortRegistry::write_port`]. So making a
//! uniform drivable is one registered [`PortBackend`] — the extension point
//! `register_builtin_port_backends` documents for exactly this.
//!
//! The write lands in [`ShaderLook::live`], the intent field that sits OUTSIDE the
//! material sharing key, and `rebind_changed_shader_look` drains it to the GPU on
//! `Changed<ShaderLook>`. That keeps one home for the value: the material asset is
//! written by the one system that already owns that job, so a rebind can never
//! resurrect a stale uniform.
//!
//! ## Why the wire is authored on the GPRIM, and what that costs
//!
//! A `UsdShade` shader input is a property of the *material*, which is shared by
//! every prim bound to it. A driven value is the opposite — it is per-instance, and
//! four landing legs each report their own load. Authoring the connection on the
//! bound geometry is therefore where the meaning lives, and it makes the material
//! private (see `unshared` in `lunco-usd-sim`'s `shader.rs`) rather than leaking one
//! leg's glow onto its three siblings.
//!
//! **This is a LunCo-private convention, not standard USD — say so out loud.**
//! Attribute connections are a core Sdf feature, so `inputs:glow.connect` on a
//! `Mesh` is spec-LEGAL and round-trips through any USD tool. But `inputs:` is a
//! UsdShade convention, and since USD 20.11 connectability is gated by the
//! `UsdShadeConnectableAPIBehavior` plugin registry, which registers Shader,
//! NodeGraph and Material — never a Gprim. So `UsdShadeConnectableAPI(mesh)` is
//! false, Hydra's `HdMaterialNetwork` never walks this edge, and usdchecker's
//! shading validators skip it. The `material:binding` chain is portable; THIS WIRE
//! IS NOT. It is invisible to Omniverse, MaterialX (`<geompropvalue>`) and Blender,
//! and unvalidated — a typo'd name is caught by our own backend or not at all.
//!
//! The standard answer to "same material, varying per gprim" is
//! `primvars:` + a `UsdPrimvarReader` node in the material's network. It would
//! delete `unshared` and the private-material-per-prim cost outright, and it is the
//! convention `shader.rs` already uses for `primvars:doNotCastShadows`. We do not
//! use it yet for one concrete reason: the binder resolves a SINGLE shader, not a
//! network (`read_shader_inputs` skips connected inputs and stops at the first hop),
//! so a `UsdPrimvarReader` has nothing to evaluate it, and per-instance primvars
//! would need a per-instance uniform path rather than today's per-material block.
//! That is a deliberate deferral with a known migration path — not an absence of a
//! standard. When the binder learns graphs, this should move to primvars.
//!
//! ## Naming
//!
//! `to_snake_case` is applied HERE rather than in the wiring pass, because
//! snake_case is a fact about WGSL struct-field reflection, not about connections.
//! `inputs:loadFrac` and `inputs:load_frac` both reach `load_frac`.

use bevy::prelude::*;
use lunco_core::ports::{PortBackend, PortDirection, PortRef, PortRegistry};
use lunco_materials::dyn_params::ParamValue;
use lunco_materials::look::ShaderLook;
use lunco_materials::naming::to_snake_case;

use crate::shader_material::ShaderMaterial;

/// The reflected schema for `entity`'s bound material, when one is available.
///
/// Returns `None` while the material or its shader is still loading — the schema
/// is reflected from WGSL source by `reflect_shader_schemas`, which cannot have run
/// before the asset exists.
fn schema_of(world: &World, entity: Entity) -> Option<std::sync::Arc<lunco_materials::dyn_params::ParamSchema>> {
    let handle = world.get::<MeshMaterial3d<ShaderMaterial>>(entity)?;
    let assets = world.get_resource::<Assets<ShaderMaterial>>()?;
    let mat = assets.get(&handle.0)?;
    if mat.schema.fields.is_empty() {
        return None;
    }
    Some(mat.schema.clone())
}

/// Does this entity drive a shader parameter called `key`?
///
/// Answered from [`ShaderLook::driven`], which the USD authoring pass filled by
/// intersecting the prim's connected `inputs:` with the parameters its bound shader
/// declares. That set is known at author time and does not depend on whether the
/// WGSL asset has finished loading, so this never has to guess: a name is a shader
/// drive or it is not, from the first tick.
///
/// Anything else is refused, which is what makes a mistyped parameter surface as
/// propagation's dangling-wire warning instead of a silently dead uniform, and what
/// keeps a simulation port sharing the prim from being swallowed here.
fn declares(world: &World, entity: Entity, key: &str) -> bool {
    world
        .get::<ShaderLook>(entity)
        .is_some_and(|look| look.driven.contains(key))
}

fn read_value(world: &World, entity: Entity, name: &str) -> Option<f32> {
    let key = to_snake_case(name);
    let look = world.get::<ShaderLook>(entity)?;
    let v = look
        .live
        .get(&key)
        .or_else(|| look.values.get(&key))
        .copied()
        .or_else(|| schema_of(world, entity)?.field(&key)?.default)?;
    match v {
        ParamValue::F32(v) => Some(v),
        ParamValue::I32(v) => Some(v as f32),
        ParamValue::U32(v) => Some(v as f32),
        // A vec parameter has no single scalar reading; a connection carries one
        // f64, so drive components individually (`inputs:tint_r`) if you need one.
        _ => None,
    }
}

/// Shader parameters are **inputs**: a uniform is something the world writes into,
/// never a source another prim reads. Exposing them as readable inputs (and not as
/// outputs) is what keeps `read_output_port` from resolving a material parameter as
/// a connection SOURCE and silently forming a feedback wire.
pub(crate) const SHADER_PARAM_BACKEND: PortBackend = PortBackend {
    list: |world, entity, out| {
        if world.get::<ShaderLook>(entity).is_none() {
            return;
        }
        let Some(schema) = schema_of(world, entity) else { return };
        for f in &schema.fields {
            // EVERY declared field is listed, whether or not it currently holds a
            // value — the shader's parameters are what the prim HAS, and a field
            // still at its default is not a missing one. Listing only the fields
            // that read back would hide half a shader's surface from `ListPorts`.
            out.push(PortRef {
                name: f.name.clone(),
                direction: PortDirection::In,
                value: read_value(world, entity, &f.name).unwrap_or(0.0) as f64,
            });
        }
    },
    read_output: |_, _, _| None,
    read_input: |world, entity, name| read_value(world, entity, name).map(|v| v as f64),
    write_input: |world, entity, name, value| {
        let key = to_snake_case(name);
        if !declares(world, entity, &key) {
            return false;
        }
        let Some(mut look) = world.get_mut::<ShaderLook>(entity) else {
            return false;
        };
        let v = value as f32;
        // Deref immutably first: touching `ShaderLook` mutably sets `Changed`, and
        // `rebind_changed_shader_look` does real work per change. A held value must
        // cost nothing, or a static scene re-packs a uniform block every tick.
        //
        // Compared by BITS, not by `==`: a NaN — which `src * scale + offset` in
        // propagation produces the moment a Modelica source diverges — is never
        // equal to itself, so a value comparison would dirty the look every tick
        // forever and rebuild the material behind it every tick forever.
        if matches!(look.live.get(&key), Some(ParamValue::F32(p)) if p.to_bits() == v.to_bits()) {
            return true;
        }
        look.live.insert(key, ParamValue::F32(v));
        true
    },
    resolve_output: None,
    resolve_input: None,
    read_slot: None,
    write_slot: None,
};

/// Register the shader-parameter backend.
///
/// Registration order is resolution precedence, and plugin order is not a contract —
/// `LuncoRenderPlugin` is added before `CoSimPlugin`, so this backend sits ahead of
/// the simulation ones. That is safe because it claims a name only when
/// [`ShaderLook::driven`] names it: a prim's simulation wires are never in that set,
/// so there is nothing for it to shadow.
pub(crate) fn build(app: &mut App) {
    app.init_resource::<PortRegistry>()
        .world_mut()
        .resource_mut::<PortRegistry>()
        .register(SHADER_PARAM_BACKEND);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        build(&mut app);
        app
    }

    /// A look as the USD pass authors it for a prim driving `name`.
    fn driving(name: &str) -> ShaderLook {
        ShaderLook {
            driven: [name.to_string()].into_iter().collect(),
            unshared: true,
            ..Default::default()
        }
    }

    /// The whole point: a value arriving through the ordinary port graph lands on a
    /// WGSL uniform, and the USD spelling (`inputs:loadFrac`) reaches the WGSL
    /// spelling (`load_frac`) without the author having to know the difference.
    #[test]
    fn a_port_write_drives_a_uniform_and_snake_cases_the_name() {
        let mut app = app();
        let e = app.world_mut().spawn(driving("load_frac")).id();

        let reg = app.world().resource::<PortRegistry>().clone();
        assert!(reg.write_port(app.world_mut(), e, "loadFrac", 0.5));

        let look = app.world().get::<ShaderLook>(e).unwrap();
        assert_eq!(look.live.get("load_frac"), Some(&ParamValue::F32(0.5)));
        // It reads back as an INPUT...
        assert_eq!(reg.read_input_port(app.world(), e, "load_frac"), Some(0.5));
        // ...and never as an output. A material parameter resolving as a connection
        // SOURCE would let a wire feed back from the renderer into the simulation.
        assert_eq!(reg.read_output_port(app.world(), e, "load_frac"), None);
    }

    /// A prim with no shader is not this backend's business. Returning false is what
    /// lets the next backend claim the name and, failing that, what makes
    /// `propagate_connections` report the wire as dangling instead of eating it.
    #[test]
    fn an_entity_without_a_shader_look_is_refused() {
        let mut app = app();
        let e = app.world_mut().spawn_empty().id();
        let reg = app.world().resource::<PortRegistry>().clone();
        assert!(!reg.write_port(app.world_mut(), e, "load_frac", 0.5));
    }

    /// `inputs:` is the engine's spelling for every port, and a landing leg carries
    /// its Modelica wires on the same prim that binds a material. Only names the USD
    /// pass resolved as shader drives may be claimed here — this backend registers
    /// ahead of the simulation ones, so claiming a name it does not own would
    /// swallow that write and report nothing.
    #[test]
    fn a_simulation_port_sharing_the_prim_is_not_claimed() {
        let mut app = app();
        let e = app.world_mut().spawn(driving("load_frac")).id();
        let reg = app.world().resource::<PortRegistry>().clone();
        assert!(!reg.write_port(app.world_mut(), e, "altitude", 12.0));
        assert!(app.world().get::<ShaderLook>(e).unwrap().live.is_empty());
    }

    /// Holding a value must not mark `ShaderLook` changed: `rebind_changed_shader_look`
    /// re-packs a 256-byte uniform block per change, so a constant wire would cost a
    /// GPU upload every tick for the lifetime of the scene.
    #[test]
    fn rewriting_the_same_value_does_not_dirty_the_look() {
        let mut app = app();
        let e = app.world_mut().spawn(driving("glow")).id();
        let reg = app.world().resource::<PortRegistry>().clone();

        assert!(reg.write_port(app.world_mut(), e, "glow", 0.25));
        app.world_mut().clear_trackers();

        assert!(reg.write_port(app.world_mut(), e, "glow", 0.25));
        assert!(!app.world().entity(e).get_ref::<ShaderLook>().unwrap().is_changed());

        assert!(reg.write_port(app.world_mut(), e, "glow", 0.75));
        assert!(app.world().entity(e).get_ref::<ShaderLook>().unwrap().is_changed());
    }
}
