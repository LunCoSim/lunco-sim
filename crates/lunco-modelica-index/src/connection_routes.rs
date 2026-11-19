//! Source-owned routes for diagram connections.

use lunco_modelica_ast::annotations::{LineRoute, extract_line_full};
use rumoca_compile::parsing::ast::{ComponentReference, Equation, StoredDefinition};
use std::collections::HashMap;

use crate::index::endpoint_from_component_ref;

type Endpoint = (String, String);

/// An undirected pair of diagram endpoints.
pub type ConnectEndpointKey = (Endpoint, Endpoint);

/// Collect the routes authored on the selected class's connection equations.
pub fn connect_line_routes(
    ast: &StoredDefinition,
    target_class: &str,
) -> HashMap<ConnectEndpointKey, LineRoute> {
    let mut routes = HashMap::new();
    if let Some(class) = ast.get_class_by_qualified_name(target_class) {
        for equation in &class.equations {
            if let Equation::Connect {
                lhs,
                rhs,
                annotation,
            } = equation
                && let Some(route) = extract_line_full(annotation)
            {
                routes
                    .entry(connection_key(endpoint(lhs), endpoint(rhs)))
                    .or_insert(route);
            }
        }
    }
    routes
}

/// Canonicalize either ordering of a diagram connection.
pub fn connection_key(lhs: Endpoint, rhs: Endpoint) -> ConnectEndpointKey {
    if lhs <= rhs { (lhs, rhs) } else { (rhs, lhs) }
}

fn endpoint(reference: &ComponentReference) -> Endpoint {
    let endpoint = endpoint_from_component_ref(reference);
    (endpoint.component_name, endpoint.port.unwrap_or_default())
}
