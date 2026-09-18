//! Native Rhai bindings for the backend-neutral assembly graph.

use lunco_core::{
    AssemblyComponent, AssemblyError, AssemblyPlan, AssemblyPort, ComponentId, ComponentKind,
    PortId, PortKind,
};
use rhai::{Dynamic, Engine, EvalAltResult, Position};

fn script_error(error: AssemblyError) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(error.message().into(), Position::NONE).into()
}

fn component_id(value: String) -> Result<ComponentId, Box<EvalAltResult>> {
    ComponentId::new(value).ok_or_else(|| {
        EvalAltResult::ErrorRuntime(
            "component id must be a non-empty identifier".into(),
            Position::NONE,
        )
        .into()
    })
}

fn port_id(value: String) -> Result<PortId, Box<EvalAltResult>> {
    PortId::new(value).ok_or_else(|| {
        EvalAltResult::ErrorRuntime(
            "port id must be a non-empty identifier".into(),
            Position::NONE,
        )
        .into()
    })
}

fn component_kind(value: String) -> Result<ComponentKind, Box<EvalAltResult>> {
    ComponentKind::parse(&value).ok_or_else(|| {
        EvalAltResult::ErrorRuntime(
            format!("unknown component kind `{value}`").into(),
            Position::NONE,
        )
        .into()
    })
}

fn port_kind(value: String) -> Result<PortKind, Box<EvalAltResult>> {
    PortKind::parse(&value).ok_or_else(|| {
        EvalAltResult::ErrorRuntime(
            format!("unknown port kind `{value}`").into(),
            Position::NONE,
        )
        .into()
    })
}

fn optional_component(value: Dynamic) -> Result<Option<ComponentId>, Box<EvalAltResult>> {
    if value.is_unit() {
        return Ok(None);
    }
    value.try_cast::<ComponentId>().map(Some).ok_or_else(|| {
        EvalAltResult::ErrorRuntime(
            "assembly parent must be a ComponentId or ()".into(),
            Position::NONE,
        )
        .into()
    })
}

fn add_component(
    plan: &mut AssemblyPlan,
    id: ComponentId,
    kind: ComponentKind,
    parent: Dynamic,
    pose: lunco_core::DTransform,
) -> Result<(), Box<EvalAltResult>> {
    plan.add_component(id, kind, optional_component(parent)?, pose)
        .map_err(script_error)
}

fn add_port(
    plan: &mut AssemblyPlan,
    component: ComponentId,
    id: PortId,
    kind: PortKind,
    pose: lunco_core::DTransform,
) -> Result<(), Box<EvalAltResult>> {
    plan.add_port(component, id, kind, pose)
        .map_err(script_error)
}

fn connect(
    plan: &mut AssemblyPlan,
    from_component: ComponentId,
    from_port: PortId,
    to_component: ComponentId,
    to_port: PortId,
) -> Result<(), Box<EvalAltResult>> {
    plan.connect(from_component, from_port, to_component, to_port)
        .map_err(script_error)
}

/// Register the typed assembly surface. Identifiers are strings only at the
/// constructor edge; poses, kinds, ports, and links remain native Rust values.
pub fn register(engine: &mut Engine) {
    engine
        .register_type_with_name::<ComponentId>("ComponentId")
        .register_get("value", |value: &mut ComponentId| value.as_str().to_owned())
        .register_type_with_name::<PortId>("PortId")
        .register_get("value", |value: &mut PortId| value.as_str().to_owned())
        .register_type_with_name::<ComponentKind>("ComponentKind")
        .register_get("value", |value: &mut ComponentKind| value.as_str())
        .register_type_with_name::<PortKind>("PortKind")
        .register_get("value", |value: &mut PortKind| value.as_str())
        .register_type_with_name::<AssemblyComponent>("AssemblyComponent")
        .register_get("id", |value: &mut AssemblyComponent| value.id.clone())
        .register_get("kind", |value: &mut AssemblyComponent| value.kind.as_str())
        .register_get("parent", |value: &mut AssemblyComponent| {
            value
                .parent
                .clone()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("pose", |value: &mut AssemblyComponent| value.local_pose)
        .register_type_with_name::<AssemblyPort>("AssemblyPort")
        .register_get("component", |value: &mut AssemblyPort| {
            value.component.clone()
        })
        .register_get("id", |value: &mut AssemblyPort| value.id.clone())
        .register_get("kind", |value: &mut AssemblyPort| value.kind.as_str())
        .register_get("pose", |value: &mut AssemblyPort| value.local_pose)
        .register_type_with_name::<lunco_core::AssemblyLink>("AssemblyLink")
        .register_get("from_component", |value: &mut lunco_core::AssemblyLink| {
            value.from_component.clone()
        })
        .register_get("from_port", |value: &mut lunco_core::AssemblyLink| {
            value.from_port.clone()
        })
        .register_get("to_component", |value: &mut lunco_core::AssemblyLink| {
            value.to_component.clone()
        })
        .register_get("to_port", |value: &mut lunco_core::AssemblyLink| {
            value.to_port.clone()
        })
        .register_type_with_name::<AssemblyError>("AssemblyError")
        .register_get("code", |value: &mut AssemblyError| value.code.clone())
        .register_get("subject", |value: &mut AssemblyError| value.subject.clone())
        .register_get("message", |value: &mut AssemblyError| value.message())
        .register_type_with_name::<AssemblyPlan>("AssemblyPlan")
        .register_get("components", |value: &mut AssemblyPlan| {
            value
                .components
                .iter()
                .cloned()
                .map(Dynamic::from)
                .collect::<rhai::Array>()
        })
        .register_get("ports", |value: &mut AssemblyPlan| {
            value
                .ports
                .iter()
                .cloned()
                .map(Dynamic::from)
                .collect::<rhai::Array>()
        })
        .register_get("links", |value: &mut AssemblyPlan| {
            value
                .links
                .iter()
                .cloned()
                .map(Dynamic::from)
                .collect::<rhai::Array>()
        })
        .register_get("link_count", |value: &mut AssemblyPlan| {
            value.links.len() as i64
        })
        .register_fn("assembly_plan", AssemblyPlan::default)
        .register_fn("component_id", component_id)
        .register_fn("port_id", port_id)
        .register_fn("component_kind", component_kind)
        .register_fn("port_kind", port_kind)
        .register_fn("assembly_add_component", add_component)
        .register_fn("assembly_add_port", add_port)
        .register_fn("assembly_connect", connect)
        .register_fn("assembly_validate", |plan: &mut AssemblyPlan| {
            plan.validate()
                .into_iter()
                .map(Dynamic::from)
                .collect::<rhai::Array>()
        })
        .register_fn(
            "assembly_world_pose",
            |plan: &mut AssemblyPlan, id: ComponentId| {
                plan.world_pose(&id)
                    .map(Dynamic::from)
                    .unwrap_or(Dynamic::UNIT)
            },
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_rhai_assembly_plan_keeps_components_and_mount_ports_typed() {
        let mut engine = Engine::new();
        crate::rhai_math::register(&mut engine);
        let values: rhai::Array = engine
            .eval(
                r#"
                let plan = assembly_plan();
                let body = component_id("Body");
                let rail = component_id("PortRail");
                assembly_add_component(plan, body, component_kind("Structure"), (), transform_identity());
                assembly_add_component(plan, rail, component_kind("Mechanism"), body, transform_identity());
                assembly_add_port(plan, rail, port_id("BodyMount"), port_kind("Mechanical"), transform_identity());
                assembly_add_port(plan, body, port_id("RailMount"), port_kind("Mechanical"), transform_identity());
                assembly_connect(plan, rail, port_id("BodyMount"), body, port_id("RailMount"));
                [assembly_validate(plan).len(), plan.components.len(), plan.ports[0].kind, plan.links[0].from_component.value]
                "#,
            )
            .expect("typed assembly Rhai plan");
        assert_eq!(values[0].as_int().unwrap(), 0);
        assert_eq!(values[1].as_int().unwrap(), 2);
        assert_eq!(
            values[2].clone().into_immutable_string().unwrap(),
            "Mechanical"
        );
        assert_eq!(
            values[3].clone().into_immutable_string().unwrap(),
            "PortRail"
        );
    }
}
