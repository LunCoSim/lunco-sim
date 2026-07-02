//! Connection and port mutation helpers.

use rumoca_compile::parsing::ast::{ClassDef, Equation};
use super::errors::AstMutError;
use super::parsing::parse_connect_equation_fragment;
use super::util::matches_port_ref;
use crate::pretty;

/// Append a `connect(...)` equation to a class.
pub fn add_connection(
    class: &mut ClassDef,
    eq: &pretty::ConnectEquation,
) -> Result<(), AstMutError> {
    let new_eq = parse_connect_equation_fragment(eq)?;
    class.equations.push(new_eq);
    Ok(())
}

/// Remove a `connect(...)` equation matching `(from, to)` PortRefs.
pub fn remove_connection(
    class: &mut ClassDef,
    from: &pretty::PortRef,
    to: &pretty::PortRef,
) -> Result<(), AstMutError> {
    let class_name = class.name.text.to_string();
    let before = class.equations.len();
    // `connect(a, b)` is symmetric — the same connection whether authored
    // `a→b` or `b→a` — so match the endpoints in EITHER order.
    class.equations.retain(|eq| {
        !matches!(
            eq,
            Equation::Connect { lhs, rhs }
                if (matches_port_ref(lhs, from) && matches_port_ref(rhs, to))
                    || (matches_port_ref(lhs, to) && matches_port_ref(rhs, from))
        )
    });
    if class.equations.len() == before {
        return Err(AstMutError::ConnectionNotFound {
            class: class_name,
            from: format!("{}.{}", from.component, from.port),
            to: format!("{}.{}", to.component, to.port),
        });
    }
    Ok(())
}

/// Swap `lhs`/`rhs` of a matching `connect(...)` equation.
pub fn reverse_connection(
    class: &mut ClassDef,
    from: &pretty::PortRef,
    to: &pretty::PortRef,
) -> Result<(), AstMutError> {
    let class_name = class.name.text.to_string();
    let mut matched = false;
    for eq in class.equations.iter_mut() {
        if let Equation::Connect { lhs, rhs } = eq {
            if matches_port_ref(lhs, from) && matches_port_ref(rhs, to) {
                std::mem::swap(lhs, rhs);
                matched = true;
                break;
            }
        }
    }
    if !matched {
        return Err(AstMutError::ConnectionNotFound {
            class: class_name,
            from: format!("{}.{}", from.component, from.port),
            to: format!("{}.{}", to.component, to.port),
        });
    }
    Ok(())
}

/// Set or clear the `annotation(Line(points={...}))` on a
/// `connect(...)` equation matching `(from, to)`.
///
/// Note: rumoca main no longer carries `annotation` on `Equation::Connect`.
/// This function validates the connection exists but the annotation
/// mutation is a no-op until annotation support is restored upstream.
pub fn set_connection_line(
    class: &mut ClassDef,
    from: &pretty::PortRef,
    to: &pretty::PortRef,
    _points: &[(f32, f32)],
) -> Result<(), AstMutError> {
    let class_name = class.name.text.to_string();
    let found = class.equations.iter().any(|eq| {
        let Equation::Connect { lhs, rhs } = eq else { return false };
        matches_port_ref(lhs, from) && matches_port_ref(rhs, to)
    });
    if !found {
        return Err(AstMutError::ConnectionNotFound {
            class: class_name,
            from: format!("{}.{}", from.component, from.port),
            to: format!("{}.{}", to.component, to.port),
        });
    }
    // Annotation field removed from Equation::Connect in rumoca main
    Ok(())
}

/// Set or clear individual `Line(...)` annotation fields on a
/// `connect(...)` equation matching `(from, to)`.
///
/// Note: rumoca main no longer carries `annotation` on `Equation::Connect`.
/// This function validates the connection exists but the annotation
/// mutation is a no-op until annotation support is restored upstream.
pub fn set_connection_line_style(
    class: &mut ClassDef,
    from: &pretty::PortRef,
    to: &pretty::PortRef,
    _color: Option<[u8; 3]>,
    _thickness: Option<f64>,
    _smooth_bezier: Option<bool>,
) -> Result<(), AstMutError> {
    let class_name = class.name.text.to_string();
    let found = class.equations.iter().any(|eq| {
        let Equation::Connect { lhs, rhs } = eq else { return false };
        matches_port_ref(lhs, from) && matches_port_ref(rhs, to)
    });
    if !found {
        return Err(AstMutError::ConnectionNotFound {
            class: class_name,
            from: format!("{}.{}", from.component, from.port),
            to: format!("{}.{}", to.component, to.port),
        });
    }
    // Annotation field removed from Equation::Connect in rumoca main
    Ok(())
}
