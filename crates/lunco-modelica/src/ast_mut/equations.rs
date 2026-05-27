//! Equation mutation helpers.

use rumoca_compile::parsing::ast::ClassDef;
use super::errors::AstMutError;
use super::parsing::{parse_stub_cached, FRAGMENT_CLASS_NAME};
use crate::pretty;

/// Append a generic equation to a class.
pub fn add_equation(
    class: &mut ClassDef,
    eq: &pretty::EquationDecl,
) -> Result<(), AstMutError> {
    let body = pretty::equation_decl(eq);
    let stub = format!("model {FRAGMENT_CLASS_NAME}\nequation\n{body}end {FRAGMENT_CLASS_NAME};\n");
    let parsed = parse_stub_cached(&stub)
        .ok_or_else(|| AstMutError::ValueParseFailed { value: body.clone() })?;
    let parsed_class = parsed
        .classes
        .get(FRAGMENT_CLASS_NAME)
        .ok_or_else(|| AstMutError::ValueParseFailed { value: body.clone() })?;
    let new_eq = parsed_class
        .equations
        .first()
        .cloned()
        .ok_or(AstMutError::ValueParseFailed { value: body })?;
    class.equations.push(new_eq);
    Ok(())
}
