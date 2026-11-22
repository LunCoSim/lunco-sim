//! Component and variable mutation helpers.
//!
//! Each helper splices the bytes it means to change and mutates the AST clone to
//! match. It must never re-emit a declaration that already exists in the source
//! so comments and formatting outside the claimed spans remain unchanged.

use rumoca_ir_ast::{ClassDef, Expression};
use std::ops::Range;

use super::clause;
use super::edit::Edit;
use super::errors::AstMutError;
use super::parsing::{
    FRAGMENT_CLASS_NAME, parse_component_fragment, parse_modification_list_fragment,
    parse_value_fragment,
};
use super::text;
use crate::pretty;

fn valid_parameter_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('.').all(|part| {
            let mut chars = part.chars();
            chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
                && chars.all(|c| c == '_' || c.is_alphanumeric())
        })
}

fn joined_modifier_path(prefix: &str, target: &str) -> String {
    if prefix.is_empty() {
        target.to_string()
    } else if target.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}.{target}")
    }
}

fn expression_range(expression: &Expression) -> Option<Range<usize>> {
    let span = expression.span();
    (span.end.0 > span.start.0).then_some(span.start.0..span.end.0)
}

/// Find a value expression for a full component-relative modifier path.
/// Nested and qualified modifier syntax resolve to the same target path.
fn modifier_value_range(
    modifiers: &[Expression],
    prefix: &str,
    target_path: &str,
) -> Option<Range<usize>> {
    for modifier in modifiers {
        match modifier {
            Expression::Modification { target, value, .. } => {
                let path = joined_modifier_path(prefix, &target.to_string());
                if path == target_path {
                    return expression_range(value);
                }
                if let Expression::ClassModification {
                    target: nested_target,
                    modifications,
                    ..
                } = value.as_ref()
                {
                    let nested_path = joined_modifier_path(&path, &nested_target.to_string());
                    if let Some(range) =
                        modifier_value_range(modifications, &nested_path, target_path)
                    {
                        return Some(range);
                    }
                }
            }
            Expression::ClassModification {
                target,
                modifications,
                ..
            } => {
                let path = joined_modifier_path(prefix, &target.to_string());
                if let Some(range) = modifier_value_range(modifications, &path, target_path) {
                    return Some(range);
                }
            }
            Expression::Binary {
                op: rumoca_core::OpBinary::Assign,
                lhs,
                rhs,
                ..
            } => {
                if let Expression::ClassModification {
                    target,
                    modifications,
                    ..
                } = lhs.as_ref()
                {
                    let path = joined_modifier_path(prefix, &target.to_string());
                    if path == target_path {
                        return expression_range(rhs);
                    }
                    if let Some(range) = modifier_value_range(modifications, &path, target_path) {
                        return Some(range);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

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

    let span_of = |e: &rumoca_ir_ast::Expression| {
        let span = e.span();
        let range = span.start.0..span.end.0;
        (range.end > range.start && source.get(range.clone()).is_some()).then_some(range)
    };

    if param.contains('.') {
        if !valid_parameter_path(param) {
            return Err(AstMutError::InvalidParameterPath(param.to_string()));
        }
        let modifier_group = text::component_modifier_group(source, comp);
        let existing_value = if let Some(group) = modifier_group.clone() {
            let modifier_text = &source[group.start + 1..group.end - 1];
            let (modifiers, prefix_len) = parse_modification_list_fragment(modifier_text)?;
            modifier_value_range(&modifiers, "", param).and_then(|range| {
                let offset = group.start as isize + 1 - prefix_len as isize;
                let start = range.start as isize + offset;
                let end = range.end as isize + offset;
                (start >= 0 && end >= start)
                    .then_some(start as usize..end as usize)
                    .filter(|range| source.get(range.clone()).is_some())
            })
        } else {
            None
        };
        if let Some(range) = existing_value {
            edit.replace(range, value_text);
        } else {
            let rendered = format!("{param} = {value_text}");
            match modifier_group {
                Some(group) => clause::append_arg(edit, group, &rendered),
                None => {
                    let at = text::component_after_name(source, comp);
                    edit.insert(at, format!("({rendered})"));
                }
            }
        }
    } else {
        match param {
            "" => match comp.binding.as_ref() {
                Some(binding) => {
                    let range = span_of(binding).ok_or_else(|| AstMutError::AnchorNotFound {
                        what: format!("binding of component `{component}`"),
                    })?;
                    edit.replace(range, value_text);
                }
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
            _ => match comp.modifications.get(param) {
                Some(value) => {
                    let range = span_of(value).ok_or_else(|| AstMutError::AnchorNotFound {
                        what: format!("`{param}` modifier of component `{component}`"),
                    })?;
                    edit.replace(range, value_text);
                }
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
            // Mirror Rumoca's binding-derived start only when no separate
            // start modifier is authored. An explicit start stays independent.
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
    insert_declaration(class, edit, &pretty::component_decl(decl));
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
    insert_declaration(class, edit, &body);
    class.components.insert(decl.name.clone(), new_component);
    Ok(())
}

/// Splice a rendered declaration in after the last existing component (or at the
/// top of the class body). `rendered` carries its own indent and trailing `;\n`.
fn insert_declaration(class: &ClassDef, edit: &mut Edit<'_>, rendered: &str) {
    let at = text::component_insert_point(edit.source(), class);
    edit.insert(at, format!("\n{}", rendered.trim_end_matches('\n')));
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
