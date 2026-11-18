//! Equation mutation helpers.

use rumoca_ir_ast::ClassDef;

use super::connections::insert_equation;
use super::edit::Edit;
use super::errors::AstMutError;
use super::parsing::parse_equation_fragment;
use crate::pretty;

/// Append a generic equation to a class.
pub fn add_equation(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    eq: &pretty::EquationDecl,
) -> Result<(), AstMutError> {
    let body = pretty::equation_decl(eq);
    let new_eq = parse_equation_fragment(&body)?;
    insert_equation(class, edit, &body)?;
    class.equations.push(new_eq);
    Ok(())
}
