//! Backend-neutral typed assembly contracts.
//!
//! An authored vehicle is more than a bag of USD prim names.  It is a graph
//! of components, local frames, and typed interfaces.  This module keeps that
//! contract independent of USD, Bevy, and Rhai so the same plan can be used by
//! a SysML adapter, a USD authoring tool, and a runtime verifier.

use crate::DTransform;
use std::collections::HashSet;

/// Stable identity for an authored assembly component.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ComponentId(String);

impl ComponentId {
    /// Construct an identity.  Names are identifiers, not encoded numeric
    /// values; rejecting control characters prevents them from becoming
    /// ambiguous USD/SysML paths later.
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.trim().is_empty() && !value.chars().any(char::is_control)).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable identity for a component-local port or mount frame.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PortId(String);

impl PortId {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.trim().is_empty() && !value.chars().any(char::is_control)).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The semantic role of an assembly component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComponentKind {
    System,
    Subsystem,
    Structure,
    Mechanism,
    Payload,
    Avionics,
    Power,
    Thermal,
    Propulsion,
    Visual,
}

impl ComponentKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Subsystem => "Subsystem",
            Self::Structure => "Structure",
            Self::Mechanism => "Mechanism",
            Self::Payload => "Payload",
            Self::Avionics => "Avionics",
            Self::Power => "Power",
            Self::Thermal => "Thermal",
            Self::Propulsion => "Propulsion",
            Self::Visual => "Visual",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "System" => Self::System,
            "Subsystem" => Self::Subsystem,
            "Structure" => Self::Structure,
            "Mechanism" => Self::Mechanism,
            "Payload" => Self::Payload,
            "Avionics" => Self::Avionics,
            "Power" => Self::Power,
            "Thermal" => Self::Thermal,
            "Propulsion" => Self::Propulsion,
            "Visual" => Self::Visual,
            _ => return None,
        })
    }
}

/// The semantic role of a component port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortKind {
    Frame,
    Mechanical,
    Electrical,
    Thermal,
    Fluid,
    Data,
}

impl PortKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Frame => "Frame",
            Self::Mechanical => "Mechanical",
            Self::Electrical => "Electrical",
            Self::Thermal => "Thermal",
            Self::Fluid => "Fluid",
            Self::Data => "Data",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "Frame" => Self::Frame,
            "Mechanical" => Self::Mechanical,
            "Electrical" => Self::Electrical,
            "Thermal" => Self::Thermal,
            "Fluid" => Self::Fluid,
            "Data" => Self::Data,
            _ => return None,
        })
    }
}

/// One component in the assembly graph.
#[derive(Clone, Debug, PartialEq)]
pub struct AssemblyComponent {
    pub id: ComponentId,
    pub kind: ComponentKind,
    pub parent: Option<ComponentId>,
    pub local_pose: DTransform,
}

/// One component-local interface frame.
#[derive(Clone, Debug, PartialEq)]
pub struct AssemblyPort {
    pub component: ComponentId,
    pub id: PortId,
    pub kind: PortKind,
    pub local_pose: DTransform,
}

/// One typed connection between two component ports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssemblyLink {
    pub from_component: ComponentId,
    pub from_port: PortId,
    pub to_component: ComponentId,
    pub to_port: PortId,
}

/// A machine-checkable component/frame/port graph.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssemblyPlan {
    pub components: Vec<AssemblyComponent>,
    pub ports: Vec<AssemblyPort>,
    pub links: Vec<AssemblyLink>,
}

impl AssemblyPlan {
    pub fn add_component(
        &mut self,
        id: ComponentId,
        kind: ComponentKind,
        parent: Option<ComponentId>,
        local_pose: DTransform,
    ) -> Result<(), AssemblyError> {
        if self.components.iter().any(|component| component.id == id) {
            return Err(AssemblyError::new("duplicate_component", id.as_str()));
        }
        if parent.as_ref().is_some_and(|parent| parent == &id) {
            return Err(AssemblyError::new("self_parent", id.as_str()));
        }
        if !local_pose.is_finite() {
            return Err(AssemblyError::new("non_finite_component_pose", id.as_str()));
        }
        self.components.push(AssemblyComponent {
            id,
            kind,
            parent,
            local_pose,
        });
        Ok(())
    }

    pub fn add_port(
        &mut self,
        component: ComponentId,
        id: PortId,
        kind: PortKind,
        local_pose: DTransform,
    ) -> Result<(), AssemblyError> {
        if !self.components.iter().any(|item| item.id == component) {
            return Err(AssemblyError::new(
                "unknown_port_component",
                component.as_str(),
            ));
        }
        if self
            .ports
            .iter()
            .any(|port| port.component == component && port.id == id)
        {
            return Err(AssemblyError::new("duplicate_port", id.as_str()));
        }
        if !local_pose.is_finite() {
            return Err(AssemblyError::new("non_finite_port_pose", id.as_str()));
        }
        self.ports.push(AssemblyPort {
            component,
            id,
            kind,
            local_pose,
        });
        Ok(())
    }

    pub fn connect(
        &mut self,
        from_component: ComponentId,
        from_port: PortId,
        to_component: ComponentId,
        to_port: PortId,
    ) -> Result<(), AssemblyError> {
        if self.links.iter().any(|link| {
            link.from_component == from_component
                && link.from_port == from_port
                && link.to_component == to_component
                && link.to_port == to_port
        }) {
            return Err(AssemblyError::new("duplicate_link", from_port.as_str()));
        }
        self.links.push(AssemblyLink {
            from_component,
            from_port,
            to_component,
            to_port,
        });
        Ok(())
    }

    /// Check references, finite poses, and parent topology without lowering
    /// the graph to USD or Bevy.
    pub fn validate(&self) -> Vec<AssemblyError> {
        let mut errors = Vec::new();
        let component_ids: HashSet<&ComponentId> =
            self.components.iter().map(|item| &item.id).collect();
        for component in &self.components {
            if let Some(parent) = &component.parent {
                if !component_ids.contains(parent) {
                    errors.push(AssemblyError::new("unknown_parent", parent.as_str()));
                }
            }
            if !component.local_pose.is_finite() {
                errors.push(AssemblyError::new(
                    "non_finite_component_pose",
                    component.id.as_str(),
                ));
            }
            let mut seen = HashSet::new();
            let mut cursor = Some(&component.id);
            while let Some(id) = cursor {
                if !seen.insert(id) {
                    errors.push(AssemblyError::new("parent_cycle", component.id.as_str()));
                    break;
                }
                cursor = self
                    .components
                    .iter()
                    .find(|item| &item.id == id)
                    .and_then(|item| item.parent.as_ref());
            }
        }

        for port in &self.ports {
            if !component_ids.contains(&port.component) {
                errors.push(AssemblyError::new(
                    "unknown_port_component",
                    port.component.as_str(),
                ));
            }
            if !port.local_pose.is_finite() {
                errors.push(AssemblyError::new("non_finite_port_pose", port.id.as_str()));
            }
        }

        for link in &self.links {
            if !has_port(self, &link.from_component, &link.from_port) {
                errors.push(AssemblyError::new(
                    "unknown_link_source",
                    link.from_port.as_str(),
                ));
            }
            if !has_port(self, &link.to_component, &link.to_port) {
                errors.push(AssemblyError::new(
                    "unknown_link_target",
                    link.to_port.as_str(),
                ));
            }
        }
        errors
    }

    pub fn world_pose(&self, id: &ComponentId) -> Option<DTransform> {
        let component = self.components.iter().find(|item| &item.id == id)?;
        match &component.parent {
            Some(parent) => self.world_pose(parent)?.compose(component.local_pose),
            None => Some(component.local_pose),
        }
    }
}

fn has_port(plan: &AssemblyPlan, component: &ComponentId, port: &PortId) -> bool {
    plan.ports
        .iter()
        .any(|item| &item.component == component && &item.id == port)
}

/// A stable machine-readable validation finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssemblyError {
    pub code: String,
    pub subject: String,
}

impl AssemblyError {
    fn new(code: &str, subject: &str) -> Self {
        Self {
            code: code.to_owned(),
            subject: subject.to_owned(),
        }
    }

    pub fn message(&self) -> String {
        format!("{}: {}", self.code, self.subject)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::{DQuat, DVec3};

    #[test]
    fn griffin_style_component_graph_resolves_typed_mount_frames() {
        let mut plan = AssemblyPlan::default();
        let body = ComponentId::new("Body").unwrap();
        let engine = ComponentId::new("EngineCluster").unwrap();
        let rail = ComponentId::new("PortRail").unwrap();
        let identity = DTransform::new(DVec3::ZERO, DQuat::IDENTITY, DVec3::ONE).unwrap();
        plan.add_component(body.clone(), ComponentKind::Structure, None, identity)
            .unwrap();
        plan.add_component(
            engine.clone(),
            ComponentKind::Propulsion,
            Some(body.clone()),
            DTransform::new(DVec3::new(0.0, -0.8, 0.0), DQuat::IDENTITY, DVec3::ONE).unwrap(),
        )
        .unwrap();
        plan.add_component(
            rail.clone(),
            ComponentKind::Mechanism,
            Some(body.clone()),
            DTransform::new(DVec3::new(0.0, 0.9, 0.0), DQuat::IDENTITY, DVec3::ONE).unwrap(),
        )
        .unwrap();
        let thrust = PortId::new("ThrustFrame").unwrap();
        let rail_mount = PortId::new("BodyMount").unwrap();
        plan.add_port(engine.clone(), thrust.clone(), PortKind::Frame, identity)
            .unwrap();
        plan.add_port(
            rail.clone(),
            rail_mount.clone(),
            PortKind::Mechanical,
            identity,
        )
        .unwrap();
        plan.connect(engine.clone(), thrust, rail.clone(), rail_mount)
            .unwrap();

        assert!(plan.validate().is_empty());
        assert_eq!(
            plan.world_pose(&engine).unwrap().translation,
            DVec3::new(0.0, -0.8, 0.0)
        );
    }
}
