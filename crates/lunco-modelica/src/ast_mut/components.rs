//! Component and variable mutation helpers.
//!
//! Each helper splices the bytes it means to change and mutates the AST clone to
//! match. It must never re-emit a declaration that already exists in the source
//! — see [`super::edit`] for what that used to cost.

use rumoca_compile::parsing::ast::ClassDef;

use super::clause;
use super::edit::Edit;
use super::errors::AstMutError;
use super::parsing::{parse_component_fragment, parse_value_fragment, FRAGMENT_CLASS_NAME};
use super::text;
use crate::pretty;

/// Set or replace a single parameter modification on a component.
///
/// `param` selects which slot is written:
/// * `""` — the declaration binding (`= value`)
/// * `"start"` — the start attribute, written where the source already keeps it
///   (as a modifier `x(start = …)`, which is also where a new one goes)
/// * anything else — a modifier `x(param = …)`
///
/// Only that one value's bytes move. A declaration like
/// `parameter Real m(start = 1, min = 0) = 5 "mass"` keeps its other modifiers,
/// its binding and its description regardless of which slot is written.
pub fn set_parameter(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    component: &str,
    param: &str,
    value_text: &str,
) -> Result<(), AstMutError> {
    let class_name = class.name.text.to_string();
    let comp = class
        .components
        .get(component)
        .ok_or_else(|| AstMutError::ComponentNotFound {
            class: class_name.clone(),
            component: component.to_string(),
        })?;
    let source = edit.source();

    // Parse first: a bad value must fail before any splice is recorded.
    let expr = parse_value_fragment(value_text)?;

    let span_of = |e: &rumoca_compile::parsing::ast::Expression| {
        e.get_location()
            .filter(|l| l.end > l.start)
            .map(|l| l.start as usize..l.end as usize)
    };

    match param {
        "" => match comp.binding.as_ref().and_then(&span_of) {
            Some(range) => edit.replace(range, value_text),
            None => {
                // No binding yet — add one after the modifier list (or the name).
                let at = text::component_modifier_group(source, comp)
                    .map(|g| g.end)
                    .unwrap_or_else(|| text::component_after_name(source, comp));
                edit.insert(at, format!(" = {value_text}"));
            }
        },
        "start" if comp.start_is_modification => {
            let range = span_of(&comp.start).ok_or_else(|| AstMutError::AnchorNotFound {
                what: format!("`start` modifier of component `{component}`"),
            })?;
            edit.replace(range, value_text);
        }
        _ => match comp.modifications.get(param).and_then(&span_of) {
            Some(range) => edit.replace(range, value_text),
            None => {
                let rendered = format!("{param} = {value_text}");
                match text::component_modifier_group(source, comp) {
                    Some(group) => clause::upsert_arg(edit, group, param, &rendered),
                    None => {
                        let at = text::component_after_name(source, comp);
                        edit.insert(at, format!("({rendered})"));
                    }
                }
            }
        },
    }

    // Mirror into the AST clone so the document's cache and index stay current.
    let comp = class
        .components
        .get_mut(component)
        .expect("component looked up above");
    match param {
        "" => {
            comp.binding = Some(expr.clone());
            comp.has_explicit_binding = true;
            // rumoca mirrors the binding into `start` only when the source
            // didn't give `start` its own modifier. Writing a binding must not
            // clobber an existing `start = …` — that conflation is the bug the
            // splice engine exists to stop reproducing in the AST too.
            if !comp.start_is_modification {
                comp.start = expr;
            }
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
    edit: &mut Edit<'_>,
    decl: &pretty::ComponentDecl,
) -> Result<(), AstMutError> {
    if class.components.contains_key(&decl.name) {
        return Err(AstMutError::DuplicateComponent {
            class: class.name.text.to_string(),
            component: decl.name.clone(),
        });
    }
    let new_component = parse_component_fragment(decl)?;
    insert_declaration(class, edit, &pretty::component_decl(decl))?;
    class.components.insert(decl.name.clone(), new_component);
    Ok(())
}

/// Add a new variable declaration to a class.
pub fn add_variable(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
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
    let parsed =
        super::parsing::parse_stub_cached(&stub).ok_or_else(|| AstMutError::ValueParseFailed {
            value: body.clone(),
        })?;
    let new_component = parsed
        .classes
        .get(FRAGMENT_CLASS_NAME)
        .and_then(|c| c.components.get(&decl.name))
        .cloned()
        .ok_or_else(|| AstMutError::ValueParseFailed {
            value: body.clone(),
        })?;
    insert_declaration(class, edit, &body)?;
    class.components.insert(decl.name.clone(), new_component);
    Ok(())
}

/// Splice a rendered declaration in after the last existing component (or at the
/// top of the class body). `rendered` carries its own indent and trailing `;\n`.
fn insert_declaration(
    class: &ClassDef,
    edit: &mut Edit<'_>,
    rendered: &str,
) -> Result<(), AstMutError> {
    let at = text::component_insert_point(edit.source(), class).ok_or_else(|| {
        AstMutError::AnchorNotFound {
            what: format!("component insertion point in class `{}`", class.name.text),
        }
    })?;
    edit.insert(at, format!("\n{}", rendered.trim_end_matches('\n')));
    Ok(())
}

/// Remove a component by name.
pub fn remove_component(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    name: &str,
) -> Result<(), AstMutError> {
    let class_name = class.name.text.to_string();
    let comp = class
        .components
        .get(name)
        .ok_or_else(|| AstMutError::ComponentNotFound {
            class: class_name.clone(),
            component: name.to_string(),
        })?;
    let stmt =
        text::component_extent(edit.source(), comp).ok_or_else(|| AstMutError::AnchorNotFound {
            what: format!("declaration of component `{name}`"),
        })?;
    edit.delete(text::line_extent(edit.source(), stmt));
    class.components.shift_remove(name);
    Ok(())
}

/// Remove a variable by name.
pub fn remove_variable(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    name: &str,
) -> Result<(), AstMutError> {
    remove_component(class, edit, name)
}
