//! Connection and port mutation helpers.
//!
//! rumoca's `Equation::Connect` carries no `annotation` field, so the routing of
//! a connection line (`annotation(Line(points={…}))`) does not survive a parse.
//! Under the old re-emit-the-class scheme that made `set_connection_line` a
//! **silent no-op** — the canvas let you drag a line, reported success, and
//! wrote nothing. Splicing works on the source text, where the annotation is
//! plainly there, so line routing works again without waiting on upstream.

use std::ops::Range;

use rumoca_compile::parsing::ast::{ClassDef, Equation};

use super::clause;
use super::edit::Edit;
use super::errors::AstMutError;
use super::parsing::parse_connect_equation_fragment;
use super::text;
use super::util::matches_port_ref;
use crate::pretty;

/// Locate the `connect(...)` equation matching `(from, to)`: its index in
/// `class.equations` and the byte extent of its statement.
///
/// `connect(a, b)` is symmetric, so endpoints match in either order.
fn find_connect(
    class: &ClassDef,
    source: &str,
    from: &pretty::PortRef,
    to: &pretty::PortRef,
) -> Result<(usize, Range<usize>), AstMutError> {
    let not_found = || AstMutError::ConnectionNotFound {
        class: class.name.text.to_string(),
        from: format!("{}.{}", from.component, from.port),
        to: format!("{}.{}", to.component, to.port),
    };
    let idx = class
        .equations
        .iter()
        .position(|eq| {
            matches!(
                eq,
                Equation::Connect { lhs, rhs }
                    if (matches_port_ref(lhs, from) && matches_port_ref(rhs, to))
                        || (matches_port_ref(lhs, to) && matches_port_ref(rhs, from))
            )
        })
        .ok_or_else(not_found)?;

    // An equation's span is only its first token — for `connect(a, b)` that is
    // `a`, which sits *inside* the parens. Anchor on the `connect` keyword
    // before scanning, or the scan starts at depth 1 and never sees the
    // statement's `;` at depth 0.
    let loc = class.equations[idx].get_location().ok_or_else(not_found)?;
    let kw = text::find_keyword(
        source,
        text::line_start(source, loc.start as usize)..source.len(),
        "connect",
    )
    .ok_or_else(|| AstMutError::AnchorNotFound {
        what: "`connect` keyword".to_string(),
    })?;
    let end = text::statement_end(source, kw).ok_or_else(|| AstMutError::AnchorNotFound {
        what: "terminating `;` of a connect equation".to_string(),
    })?;
    Ok((idx, kw..end))
}

/// The `(...)` of the `connect` call in a connect statement.
fn connect_args(source: &str, stmt: Range<usize>) -> Result<Range<usize>, AstMutError> {
    text::paren_group_at(source, stmt.start + "connect".len()).ok_or_else(|| {
        AstMutError::AnchorNotFound {
            what: "argument list of a `connect` equation".to_string(),
        }
    })
}

/// Append a `connect(...)` equation to a class.
pub fn add_connection(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    eq: &pretty::ConnectEquation,
) -> Result<(), AstMutError> {
    let new_eq = parse_connect_equation_fragment(eq)?;
    let rendered = pretty::connect_equation(eq);
    insert_equation(class, edit, &rendered)?;
    class.equations.push(new_eq);
    Ok(())
}

/// Splice a rendered equation into the class's `equation` section, creating the
/// section when the class doesn't have one yet. `rendered` carries its own
/// indent and trailing `;\n`.
pub(super) fn insert_equation(
    class: &ClassDef,
    edit: &mut Edit<'_>,
    rendered: &str,
) -> Result<(), AstMutError> {
    let source = edit.source();
    match text::equation_insert_point(source, class) {
        Some(at) => edit.insert(at, format!("\n{}", rendered.trim_end_matches('\n'))),
        None => {
            // No `equation` section yet. It must go before the class annotation
            // (if any) and before `end Name;`.
            let at = text::class_tail_anchor(source, class).ok_or_else(|| {
                AstMutError::AnchorNotFound {
                    what: format!("end of class `{}`", class.name.text),
                }
            })?;
            edit.insert(at, format!("equation\n{rendered}"));
        }
    }
    Ok(())
}

/// Remove a `connect(...)` equation matching `(from, to)`.
pub fn remove_connection(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    from: &pretty::PortRef,
    to: &pretty::PortRef,
) -> Result<(), AstMutError> {
    let (idx, stmt) = find_connect(class, edit.source(), from, to)?;
    edit.delete(text::line_extent(edit.source(), stmt));
    class.equations.remove(idx);
    Ok(())
}

/// Swap `lhs`/`rhs` of a matching `connect(...)` equation.
pub fn reverse_connection(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    from: &pretty::PortRef,
    to: &pretty::PortRef,
) -> Result<(), AstMutError> {
    let (idx, stmt) = find_connect(class, edit.source(), from, to)?;
    let source = edit.source();
    let group = connect_args(source, stmt)?;
    let args = text::split_args(source, group);
    if args.len() != 2 {
        return Err(AstMutError::AnchorNotFound {
            what: "the two endpoints of a `connect` equation".to_string(),
        });
    }
    // Swap the endpoints' text, keeping each endpoint's exact bytes.
    let lhs = source[args[0].clone()].trim().to_string();
    let rhs = source[args[1].clone()].trim().to_string();
    edit.replace(args[0].clone(), rhs);
    edit.replace(args[1].clone(), format!(" {lhs}"));

    if let Equation::Connect { lhs, rhs } = &mut class.equations[idx] {
        std::mem::swap(lhs, rhs);
    }
    Ok(())
}

/// Set the `annotation(Line(points={…}))` route of a `connect(...)` equation.
///
/// Splices `points` inside any existing `Line(...)`, so a hand-authored `color`
/// / `thickness` / `smooth` on the same Line survives a re-route.
pub fn set_connection_line(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    from: &pretty::PortRef,
    to: &pretty::PortRef,
    points: &[(f32, f32)],
) -> Result<(), AstMutError> {
    let (_, stmt) = find_connect(class, edit.source(), from, to)?;
    set_line_fields(edit, stmt, &[("points", pretty::fmt_points(points))])
}

/// Set individual `Line(...)` style fields on a `connect(...)` equation.
/// `None` leaves a field as authored.
pub fn set_connection_line_style(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    from: &pretty::PortRef,
    to: &pretty::PortRef,
    color: Option<[u8; 3]>,
    thickness: Option<f64>,
    smooth_bezier: Option<bool>,
) -> Result<(), AstMutError> {
    let (_, stmt) = find_connect(class, edit.source(), from, to)?;
    let mut fields: Vec<(&str, String)> = Vec::new();
    if let Some([r, g, b]) = color {
        fields.push(("color", format!("{{{r},{g},{b}}}")));
    }
    if let Some(t) = thickness {
        fields.push(("thickness", pretty::fmt_num_f64(t)));
    }
    if let Some(s) = smooth_bezier {
        let value = if s { "Smooth.Bezier" } else { "Smooth.None" };
        fields.push(("smooth", value.to_string()));
    }
    set_line_fields(edit, stmt, &fields)
}

/// Upsert `name=value` fields inside the `annotation(Line(...))` of a connect
/// statement, creating the `Line(...)` — and the `annotation(...)` around it —
/// when the connection has none yet. Fields the caller didn't name are left
/// exactly as authored.
fn set_line_fields(
    edit: &mut Edit<'_>,
    stmt: Range<usize>,
    fields: &[(&str, String)],
) -> Result<(), AstMutError> {
    if fields.is_empty() {
        return Ok(());
    }
    let source = edit.source();
    let line_group = text::annotation_clause(source, stmt.clone()).and_then(|(_, group)| {
        clause::call_group(source, group.clone(), "Line").map(|l| (group, l))
    });

    match line_group {
        // A `Line(...)` is already there: splice each field into its arg list.
        Some((_, line)) => {
            for (name, value) in fields {
                match clause::arg_value(source, line.clone(), name) {
                    Some(range) => edit.replace(range, value.clone()),
                    None => {
                        clause::upsert_arg(edit, line.clone(), name, &format!("{name}={value}"))
                    }
                }
            }
        }
        None => {
            let rendered = format!(
                "Line({})",
                fields
                    .iter()
                    .map(|(n, v)| format!("{n}={v}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            match text::annotation_clause(source, stmt.clone()) {
                // Annotation exists but has no Line — add one alongside.
                Some((_, group)) => clause::upsert_arg(edit, group, "Line", &rendered),
                // No annotation at all — add one before the `;`.
                None => edit.insert(stmt.end - 1, format!(" annotation({rendered})")),
            }
        }
    }
    Ok(())
}
