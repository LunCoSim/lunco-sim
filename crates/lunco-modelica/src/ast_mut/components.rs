//! Component and variable mutation helpers.

use rumoca_compile::parsing::ast::ClassDef;
use super::errors::AstMutError;
use super::parsing::{parse_value_fragment, parse_component_fragment, FRAGMENT_CLASS_NAME};
use crate::pretty;

/// Set or replace a single parameter modification on a component.
pub fn set_parameter(
    class: &mut ClassDef,
    component: &str,
    param: &str,
    value_text: &str,
) -> Result<(), AstMutError> {
    let class_name = class.name.text.to_string();
    let comp = class
        .components
        .get_mut(component)
        .ok_or_else(|| AstMutError::ComponentNotFound {
            class: class_name,
            component: component.to_string(),
        })?;
    let expr = parse_value_fragment(value_text)?;
    match param {
        "" => {
            comp.binding = Some(expr.clone());
            comp.start = expr;
            comp.has_explicit_binding = true;
            comp.start_is_modification = false;
        }
        "start" => {
            comp.start = expr;
            comp.start_is_modification = true;
        }
        _ => {
            comp.modifications.insert(param.to_string(), expr);
        }
    }
    Ok(())
}

/// Append a new component to a class.
pub fn add_component(
    class: &mut ClassDef,
    decl: &pretty::ComponentDecl,
) -> Result<(), AstMutError> {
    if class.components.contains_key(&decl.name) {
        return Err(AstMutError::DuplicateComponent {
            class: class.name.text.to_string(),
            component: decl.name.clone(),
        });
    }
    let new_component = parse_component_fragment(decl)?;
    class.components.insert(decl.name.clone(), new_component);
    Ok(())
}

/// Remove a component by name.
pub fn remove_component(class: &mut ClassDef, name: &str) -> Result<(), AstMutError> {
    let class_name = class.name.text.to_string();
    if class.components.shift_remove(name).is_none() {
        return Err(AstMutError::ComponentNotFound {
            class: class_name,
            component: name.to_string(),
        });
    }
    Ok(())
}

/// Add a new variable declaration to a class.
pub fn add_variable(
    class: &mut ClassDef,
    decl: &pretty::VariableDecl,
) -> Result<(), AstMutError> {
    if class.components.contains_key(&decl.name) {
        return Err(AstMutError::DuplicateComponent {
            class: class.name.text.to_string(),
            component: decl.name.clone(),
        });
    }
    let body = pretty::variable_decl(decl);
    let stub = format!("model {FRAGMENT_CLASS_NAME}\n{body}end {FRAGMENT_CLASS_NAME};\n");
    let parsed = super::parsing::parse_stub_cached(&stub)
        .ok_or_else(|| AstMutError::ValueParseFailed { value: body.clone() })?;
    let parsed_class = parsed
        .classes
        .get(FRAGMENT_CLASS_NAME)
        .ok_or_else(|| AstMutError::ValueParseFailed { value: body.clone() })?;
    let new_component = parsed_class
        .components
        .get(&decl.name)
        .cloned()
        .ok_or(AstMutError::ValueParseFailed { value: body })?;
    class.components.insert(decl.name.clone(), new_component);
    Ok(())
}

/// Remove a variable by name.
pub fn remove_variable(class: &mut ClassDef, name: &str) -> Result<(), AstMutError> {
    remove_component(class, name)
}
