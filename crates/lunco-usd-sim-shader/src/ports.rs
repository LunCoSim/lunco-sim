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
//! `rewire_usd_connections` already turns any `inputs:foo.connect` on any prim
//! entity into a `SimConnection` with no check on what kind of thing the target
//! is, and `propagate_connections` routes every write through the shared port
//! registry. This backend resolves driven parameters to their ordered authored
//! slots once; steady-state propagation does not normalize or compare names.
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
//! private (see `unshared` in `lunco-usd-sim-shader`) rather than leaking one
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
//! convention `lunco-usd-sim-shader` already uses for `primvars:doNotCastShadows`. We do not
//! use it yet for one concrete reason: the binder resolves a SINGLE shader, not a
//! network (`read_shader_inputs` skips connected inputs and stops at the first hop),
//! so a `UsdPrimvarReader` has nothing to evaluate it, and per-instance primvars
//! would need a per-instance uniform path rather than today's per-material block.
//! That is a deliberate deferral with a known migration path — not an absence of a
//! standard. When the binder learns graphs, this should move to primvars.
//!
//! ## Why this lives beside the AUTHORING, not beside the renderer
//!
//! It used to live in `lunco-render-bevy`, and answered `has_port` by reflecting the
//! bound `ShaderMaterial`'s WGSL schema. That made a shader-driven port exist only in a
//! build with a render stack — so in the headless harness the backend was never even
//! registered, every shader wire in the scene dropped its value forever, and the
//! never-landed gate reported the descent lander's plume `throttle`, its legs'
//! `load_frac` and its light wires as authoring faults. Three green scenes went red for
//! a reason that was purely about which crates the binary linked.
//!
//! Nothing here needs the GPU. [`ShaderLook::driven`] is the authoring pass's own
//! answer to "which of this prim's wires name parameters its shader declares"
//! (`lunco-usd-sim-shader::driven_shader_inputs`) — computed from the WGSL source at author
//! time, present in every build. The write lands in [`ShaderLook::live`], a plain
//! component. So the backend belongs where that component is filled, which is this
//! crate, and it is registered unconditionally.
//!
//! This is the same rule the sun ports follow: a port backend belongs to the crate
//! that owns the component it reads, never to the crate that happens to consume the
//! value downstream.
//!
//! ## Naming
//!
//! `to_snake_case` is applied HERE rather than in the wiring pass, because
//! snake_case is a fact about WGSL struct-field reflection, not about connections.
//! `inputs:loadFrac` and `inputs:load_frac` both reach `load_frac`.

use bevy::prelude::*;
use lunco_materials::dyn_params::ParamValue;
use lunco_materials::look::ShaderLook;
use lunco_materials::naming::to_snake_case;
use lunco_port_core::ports::{
    PortBackend, PortDeclaration, PortDirection, PortMetadata, PortRegistry, PortTopologyRevision,
    PortTopologyState,
};

fn read_value(world: &World, entity: Entity, name: &str) -> Option<f32> {
    let key = to_snake_case(name);
    let look = world.get::<ShaderLook>(entity)?;
    // `live` (a landed wire) beats `values` (the authored constant). There is no
    // third fallback to the shader's declared default: that would need the reflected
    // material schema, which only exists in a render build, and reading a port
    // differently depending on whether a GPU is present is the defect this module
    // was moved here to remove. An undriven, unauthored parameter reads as absent.
    let v = look
        .live_value(&key)
        .or_else(|| look.values().get(&key).copied())?;
    match v {
        ParamValue::F32(v) => Some(v),
        ParamValue::I32(v) => Some(v as f32),
        ParamValue::U32(v) => Some(v as f32),
        // A vec parameter has no single scalar reading; a connection carries one
        // f64, so drive components individually (`inputs:tint_r`) if you need one.
        _ => None,
    }
}

fn resolve_shader_input(world: &World, entity: Entity, name: &str) -> Option<u64> {
    let key = to_snake_case(name);
    let look = world.get::<ShaderLook>(entity)?;
    if !look.driven().contains(&key) {
        return None;
    }
    look.live().resolve_slot(&key)
}

fn read_shader_slot(world: &World, entity: Entity, slot: u64) -> Option<f64> {
    let look = world.get::<ShaderLook>(entity)?;
    let (name, live) = look.live().get_slot_entry(slot)?;
    let value = live.as_ref().or_else(|| look.values().get(name))?;
    match *value {
        ParamValue::F32(value) => Some(f64::from(value)),
        ParamValue::I32(value) => Some(f64::from(value)),
        ParamValue::U32(value) => Some(f64::from(value)),
        _ => None,
    }
}

fn write_shader_slot(world: &mut World, entity: Entity, slot: u64, value: f64) {
    let shader_value_f32 = value as f32;
    assert!(
        shader_value_f32.is_finite(),
        "validated shader scalar must fit f32"
    );
    let (original, current_live) = world
        .get::<ShaderLook>(entity)
        .and_then(|look| {
            let (name, live) = look.live().get_slot_entry(slot)?;
            let current_live = *live;
            let original = current_live.or_else(|| look.values().get(name).copied())?;
            Some((original, current_live))
        })
        .expect("prepared shader input slot remains live through commit");
    assert!(
        matches!(original, ParamValue::F32(_)),
        "only authored f32 shader parameters accept scalar input writes"
    );
    let live_value = ParamValue::F32(shader_value_f32);
    if current_live == Some(live_value) {
        return;
    }
    let mut look = world
        .get_mut::<ShaderLook>(entity)
        .expect("prepared shader input retains its ShaderLook");
    look.set_live_slot(slot, live_value)
        .expect("prepared shader input slot remains live through commit");
}

/// Shader parameters are **inputs**: a uniform is something the world writes into,
/// never a source another prim reads. Exposing them as readable inputs (and not as
/// outputs) is what keeps `read_output_port` from resolving a material parameter as
/// a connection SOURCE and silently forming a feedback wire.
pub const SHADER_PARAM_BACKEND: PortBackend = PortBackend {
    list_entities: |world, out| {
        out.extend(
            world
                .query_filtered::<Entity, With<ShaderLook>>()
                .iter(world),
        );
    },
    topology_key: |world, entity| {
        let Some(look) = world.get::<ShaderLook>(entity) else {
            return 0;
        };
        shader_topology_key(look)
    },
    list: |world, entity, out| {
        let Some(look) = world.get::<ShaderLook>(entity) else {
            return;
        };
        // The prim's DRIVEN parameters plus whatever it authored a value for — the
        // same set the resolved slot writer accepts, so listing and writing cannot disagree.
        //
        // It used to list every field the bound material's WGSL declares. That was a
        // strictly larger set (a shared shader's full surface, most of it irrelevant
        // to this prim) and it required the reflected schema, i.e. a GPU build. The
        // parameters a prim actually HAS are the ones it drives or authors.
        let mut names: std::collections::BTreeSet<&String> = look.driven().iter().collect();
        names.extend(look.values().keys());
        for name in names {
            out.push(PortDeclaration {
                name: name.clone(),
                direction: PortDirection::In,
            });
        }
    },
    metadata: |world, entity, name, direction| {
        let key = to_snake_case(name);
        let authored_scalar = world
            .get::<ShaderLook>(entity)
            .filter(|look| look.driven().contains(&key))
            .and_then(|look| {
                look.values()
                    .get(&key)
                    .copied()
                    .or_else(|| look.live_value(&key))
            });
        let writable = matches!(authored_scalar, Some(ParamValue::F32(_)));
        PortMetadata::scalar(
            direction,
            None,
            Some(-(f32::MAX as f64)),
            Some(f32::MAX as f64),
            "shader parameter",
            "material owner",
            writable,
            None,
        )
    },
    read_output: |_, _, _| None,
    read_input: |world, entity, name| read_value(world, entity, name).map(|v| v as f64),
    resolve_output: None,
    resolve_input: Some(resolve_shader_input),
    read_slot: None,
    read_input_slot: Some(read_shader_slot),
    write_slot: Some(write_shader_slot),
};

fn shader_topology_key(look: &ShaderLook) -> u64 {
    look.port_topology_key()
}

/// Detect an in-place change to the shader parameter surface. The key is cached
/// by authored-shape setters and reads the live-slot layout identity, so changing
/// samples does not walk or hash parameter names.
fn check_shader_port_structure(
    changed: Query<(Entity, &ShaderLook), Changed<ShaderLook>>,
    mut state: ResMut<PortTopologyState>,
    mut revision: ResMut<PortTopologyRevision>,
) {
    for (entity, look) in &changed {
        if state.changed::<ShaderLook>(entity, shader_topology_key(look)) {
            revision.bump();
        }
    }
}

/// Register the shader-parameter backend.
///
/// Registration order is resolution precedence, and plugin order is not a contract —
/// `LuncoRenderPlugin` is added before `CoSimPlugin`, so this backend sits ahead of
/// the simulation ones. That is safe because it claims a name only when
/// [`ShaderLook::driven`] names it: a prim's simulation wires are never in that set,
/// so there is nothing for it to shadow.
pub fn build(app: &mut App) {
    app.init_resource::<PortRegistry>()
        .init_resource::<lunco_port_core::ports::PortTopologyRevision>()
        .init_resource::<lunco_port_core::ports::PortTopologyState>()
        .add_observer(lunco_port_core::ports::bump_port_topology_on_add::<ShaderLook>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<ShaderLook>)
        .add_systems(PostUpdate, check_shader_port_structure)
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
        ShaderLook::default()
            .with_driven([name.to_string()].into_iter().collect())
            .unshared()
    }

    /// The whole point: a value arriving through the ordinary port graph lands on a
    /// WGSL uniform, and the USD spelling (`inputs:loadFrac`) reaches the WGSL
    /// spelling (`load_frac`) without the author having to know the difference.
    #[test]
    fn a_port_write_drives_a_uniform_and_snake_cases_the_name() {
        let mut app = app();
        let e = app.world_mut().spawn(driving("load_frac")).id();

        let reg = app.world().resource::<PortRegistry>().clone();
        assert!(reg.write_port(app.world_mut(), e, "loadFrac", 0.5).is_ok());

        let look = app.world().get::<ShaderLook>(e).unwrap();
        assert_eq!(look.live_value("load_frac"), Some(ParamValue::F32(0.5)));
        // It reads back as an INPUT...
        assert_eq!(reg.read_input_port(app.world(), e, "load_frac"), Some(0.5));
        // ...and never as an output. A material parameter resolving as a connection
        // SOURCE would let a wire feed back from the renderer into the simulation.
        assert_eq!(reg.read_output_port(app.world(), e, "load_frac"), None);
    }

    #[test]
    fn resolved_shader_slot_writes_without_re_normalizing_the_port_name() {
        let mut app = app();
        let e = app.world_mut().spawn(driving("load_frac")).id();
        app.update();
        let topology_revision = app.world().resource::<PortTopologyRevision>().0;
        let reg = app.world().resource::<PortRegistry>().clone();
        let slot = reg
            .resolve_input(app.world(), e, "loadFrac")
            .expect("the authored driven field resolves at wiring time");

        assert!(reg.write_resolved(app.world_mut(), e, &slot, 0.5).is_ok());
        assert_eq!(reg.read_resolved(app.world(), e, &slot), Some(0.5));
        app.update();
        assert_eq!(
            app.world().resource::<PortTopologyRevision>().0,
            topology_revision,
            "a live sample must not rescan/rebuild the shader port topology"
        );
        app.world_mut().clear_trackers();
        assert!(reg.write_resolved(app.world_mut(), e, &slot, 0.5).is_ok());
        assert!(
            !app.world()
                .entity(e)
                .get_ref::<ShaderLook>()
                .unwrap()
                .is_changed(),
            "an unchanged slot write must not schedule a shader rebind"
        );
        assert!(reg.write_resolved(app.world_mut(), e, &slot, 0.75).is_ok());
        assert_eq!(reg.read_resolved(app.world(), e, &slot), Some(0.75));
    }

    /// A prim with no shader is not this backend's business. Returning an error is what
    /// lets the next backend claim the name and, failing that, what makes
    /// `propagate_connections` report the wire as dangling instead of eating it.
    #[test]
    fn an_entity_without_a_shader_look_is_refused() {
        let mut app = app();
        let e = app.world_mut().spawn_empty().id();
        let reg = app.world().resource::<PortRegistry>().clone();
        assert!(
            reg.write_port(app.world_mut(), e, "load_frac", 0.5)
                .is_err()
        );
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
        assert!(
            reg.write_port(app.world_mut(), e, "altitude", 12.0)
                .is_err()
        );
        assert!(!app.world().get::<ShaderLook>(e).unwrap().has_live_values());
    }

    /// Holding a value must not mark `ShaderLook` changed: `rebind_changed_shader_look`
    /// re-packs a 256-byte uniform block per change, so a constant wire would cost a
    /// GPU upload every tick for the lifetime of the scene.
    #[test]
    fn rewriting_the_same_value_does_not_dirty_the_look() {
        let mut app = app();
        let e = app.world_mut().spawn(driving("glow")).id();
        let reg = app.world().resource::<PortRegistry>().clone();

        assert!(reg.write_port(app.world_mut(), e, "glow", 0.25).is_ok());
        app.world_mut().clear_trackers();

        assert!(reg.write_port(app.world_mut(), e, "glow", 0.25).is_ok());
        assert!(
            !app.world()
                .entity(e)
                .get_ref::<ShaderLook>()
                .unwrap()
                .is_changed()
        );

        assert!(reg.write_port(app.world_mut(), e, "glow", 0.75).is_ok());
        assert!(
            app.world()
                .entity(e)
                .get_ref::<ShaderLook>()
                .unwrap()
                .is_changed()
        );
    }
}
