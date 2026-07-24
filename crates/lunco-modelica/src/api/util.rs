//! Shared utilities for API handlers.

use crate::pretty::PortRef;
use bevy::prelude::*;
use lunco_doc::DocumentId;

pub fn resolve_doc(world: &World, raw: DocumentId) -> Option<DocumentId> {
    if raw.is_unassigned() {
        world
            .get_resource::<lunco_workspace::WorkspaceResource>()
            .and_then(|ws| ws.active_document)
    } else {
        Some(raw)
    }
}

pub fn parse_port_ref(s: &str) -> Option<PortRef> {
    let (comp, port) = s.split_once('.')?;
    if comp.is_empty() || port.is_empty() {
        return None;
    }
    Some(PortRef::new(comp.to_string(), port.to_string()))
}

pub fn strip_same_package_prefix(class: &str, type_name: &str) -> String {
    let Some((class_pkg, _)) = class.rsplit_once('.') else {
        return type_name.to_string();
    };
    let prefix = format!("{class_pkg}.");
    if let Some(stripped) = type_name.strip_prefix(&prefix) {
        if !stripped.contains('.') {
            return stripped.to_string();
        }
    }
    type_name.to_string()
}
