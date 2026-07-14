//! Annotation graphics mutation helpers.
//!
//! All of these edit a nested argument list —
//! `annotation(Icon(graphics={…}))`, `annotation(__LunCo(plotNodes={…}))`,
//! `annotation(experiment(...))`. Each op splices the one argument it owns and
//! creates only the levels that are genuinely missing, so an unrelated sibling
//! (`Documentation(info="…")`, a hand-tuned `coordinateSystem`) is never
//! rewritten by, say, moving a text label.

use std::ops::Range;

use rumoca_compile::parsing::ast::{ClassDef, Expression};

use super::clause;
use super::edit::Edit;
use super::errors::AstMutError;
use super::text;
use super::util::{graphic_entry_arg, is_graphic_entry_named, read_text_spec, render_text_spec, string_literal_value};
use crate::pretty;

// ---------------------------------------------------------------------------
// Component placement
// ---------------------------------------------------------------------------

/// Set or replace the `Placement(...)` annotation on a component.
///
/// Touches only the component's `Placement` argument. The declaration's
/// modifiers, binding and description — and any other annotation entry such as
/// `Dialog(...)` — keep their exact bytes.
pub fn set_placement(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    component: &str,
    placement: &pretty::Placement,
) -> Result<(), AstMutError> {
    let class_name = class.name.text.to_string();
    let comp = class
        .components
        .get(component)
        .ok_or_else(|| AstMutError::ComponentNotFound {
            class: class_name,
            component: component.to_string(),
        })?;
    let source = edit.source();
    let stmt =
        text::component_extent(source, comp).ok_or_else(|| AstMutError::AnchorNotFound {
            what: format!("declaration of component `{component}`"),
        })?;
    let rendered = pretty::placement_inner(placement);

    match text::annotation_clause(source, stmt.clone()) {
        Some((_, group)) => clause::upsert_arg(edit, group, "Placement", &rendered),
        None => edit.insert(stmt.end - 1, format!(" annotation({rendered})")),
    }

    // Mirror into the AST clone.
    let new_expr = super::parsing::parse_placement_expression(placement)?;
    let comp = class
        .components
        .get_mut(component)
        .expect("component looked up above");
    match comp
        .annotation
        .iter_mut()
        .find(|e| super::util::is_annotation_entry_named(e, "Placement"))
    {
        Some(slot) => *slot = new_expr,
        None => comp.annotation.push(new_expr),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Class annotation plumbing
// ---------------------------------------------------------------------------

/// Resolve `annotation(<section>(<key>={…}))` down to the array's byte range,
/// creating whatever levels are missing.
///
/// Returns `None` when a level had to be created — the array now exists in the
/// pending splice but not in `source`, so there is nothing to splice *into*.
/// Callers pass the entry they wanted to add as `seed`, and it is written as
/// part of the created level.
fn resolve_array(
    class: &ClassDef,
    edit: &mut Edit<'_>,
    section: &str,
    key: &str,
    seed: &str,
) -> Result<Option<Range<usize>>, AstMutError> {
    let source = edit.source();

    let Some((_, annotation)) = text::class_annotation_clause(source, class) else {
        // No class annotation at all: create the whole path.
        let at = text::class_tail_anchor(source, class).ok_or_else(|| {
            AstMutError::AnchorNotFound {
                what: format!("end of class `{}`", class.name.text),
            }
        })?;
        let indent = pretty::options().indent;
        edit.insert(
            at,
            format!("{indent}annotation({section}({key}={{{seed}}}));\n"),
        );
        return Ok(None);
    };

    let Some(section_group) = clause::call_group(source, annotation.clone(), section) else {
        // Annotation exists but not this section.
        clause::upsert_arg(
            edit,
            annotation,
            section,
            &format!("{section}({key}={{{seed}}})"),
        );
        return Ok(None);
    };

    match clause::arg_value(source, section_group.clone(), key) {
        Some(value) => {
            // `key = {…}` is there — hand back the array (including braces).
            let bytes = source.as_bytes();
            if bytes.get(value.start) != Some(&b'{') {
                return Err(AstMutError::AnchorNotFound {
                    what: format!("`{key}` array in `{section}` annotation"),
                });
            }
            Ok(Some(value))
        }
        None => {
            clause::upsert_arg(
                edit,
                section_group,
                key,
                &format!("{key}={{{seed}}}"),
            );
            Ok(None)
        }
    }
}

/// The class-level `annotation(...)` argument list, creating it when absent.
/// `None` means it was just created around `seed`.
fn resolve_class_annotation(
    class: &ClassDef,
    edit: &mut Edit<'_>,
    seed: &str,
) -> Result<Option<Range<usize>>, AstMutError> {
    let source = edit.source();
    if let Some((_, group)) = text::class_annotation_clause(source, class) {
        return Ok(Some(group));
    }
    let at = text::class_tail_anchor(source, class).ok_or_else(|| AstMutError::AnchorNotFound {
        what: format!("end of class `{}`", class.name.text),
    })?;
    let indent = pretty::options().indent;
    edit.insert(at, format!("{indent}annotation({seed});\n"));
    Ok(None)
}

// ---------------------------------------------------------------------------
// Icon / Diagram graphics
// ---------------------------------------------------------------------------

/// Append a graphic to `Icon(graphics={…})` or `Diagram(graphics={…})`.
pub fn add_named_graphic(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    section_name: &str,
    graphic_text: &str,
) -> Result<(), AstMutError> {
    let entry = super::parsing::parse_graphics_entry(graphic_text)?;
    if let Some(array) = resolve_array(class, edit, section_name, "graphics", graphic_text)? {
        clause::append_entry(edit, array, graphic_text);
    }
    graphics_array_mut(class, section_name).push(entry);
    Ok(())
}

/// Set or replace the `extent` of the i-th `Text(...)` entry in `Diagram`.
pub fn set_diagram_text_extent(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    index: usize,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
) -> Result<(), AstMutError> {
    update_diagram_text_at(class, edit, index, |spec| {
        spec.x1 = x1;
        spec.y1 = y1;
        spec.x2 = x2;
        spec.y2 = y2;
    })
}

/// Set or replace the `textString=` of the i-th `Text(...)` entry.
pub fn set_diagram_text_string(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    index: usize,
    text: &str,
) -> Result<(), AstMutError> {
    update_diagram_text_at(class, edit, index, |spec| {
        spec.text = text.to_string();
    })
}

/// Remove the i-th `Text(...)` entry from `Diagram(graphics)`.
pub fn remove_diagram_text(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    index: usize,
) -> Result<(), AstMutError> {
    let class_name = class.name.text.to_string();
    let (array, entry_idx, args) = diagram_text_entry(class, edit, index)?;
    let _ = array;
    clause::remove_entry(edit, &args, entry_idx);

    let arr = graphics_array_mut(class, "Diagram");
    let ast_idx = nth_text_index(arr, index).ok_or(AstMutError::DiagramTextIndexOutOfRange {
        class: class_name,
        index,
    })?;
    arr.remove(ast_idx);
    Ok(())
}

fn update_diagram_text_at<F>(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    index: usize,
    update: F,
) -> Result<(), AstMutError>
where
    F: FnOnce(&mut super::util::TextSpec),
{
    let class_name = class.name.text.to_string();
    let (_, entry_idx, args) = diagram_text_entry(class, edit, index)?;

    // Read the spec from the AST (already parsed), apply, re-render. A `Text`
    // graphic is entirely machine-authored, so re-rendering the one entry is
    // safe — its siblings in the array keep their bytes.
    let arr = graphics_array_mut(class, "Diagram");
    let ast_idx = nth_text_index(arr, index).ok_or(AstMutError::DiagramTextIndexOutOfRange {
        class: class_name,
        index,
    })?;
    let mut spec = read_text_spec(&arr[ast_idx]);
    update(&mut spec);
    let rendered = render_text_spec(&spec);
    arr[ast_idx] = super::parsing::parse_graphics_entry(&rendered)?;

    edit.replace(args[entry_idx].clone(), rendered);
    Ok(())
}

/// Locate the i-th `Text(...)` entry inside `Diagram(graphics={…})` in the
/// source: the array range, the entry's index among the array's args, and the
/// arg ranges.
fn diagram_text_entry(
    class: &ClassDef,
    edit: &mut Edit<'_>,
    index: usize,
) -> Result<(Range<usize>, usize, Vec<Range<usize>>), AstMutError> {
    let out_of_range = || AstMutError::DiagramTextIndexOutOfRange {
        class: class.name.text.to_string(),
        index,
    };
    let source = edit.source();
    let (_, annotation) =
        text::class_annotation_clause(source, class).ok_or_else(out_of_range)?;
    let section = clause::call_group(source, annotation, "Diagram").ok_or_else(out_of_range)?;
    let array = clause::arg_value(source, section, "graphics").ok_or_else(out_of_range)?;
    let args = text::split_args(source, array.clone());

    let mut seen = 0usize;
    for (i, arg) in args.iter().enumerate() {
        if text::arg_head(source, arg.clone()) != "Text" {
            continue;
        }
        if seen == index {
            return Ok((array, i, args));
        }
        seen += 1;
    }
    Err(out_of_range())
}

fn nth_text_index(arr: &[Expression], index: usize) -> Option<usize> {
    let mut seen = 0usize;
    for (i, e) in arr.iter().enumerate() {
        if is_graphic_entry_named(e, "Text") {
            if seen == index {
                return Some(i);
            }
            seen += 1;
        }
    }
    None
}

// ---------------------------------------------------------------------------
// LunCo plot nodes
// ---------------------------------------------------------------------------

/// Add or replace a plot tile in
/// `annotation(__LunCo(plotNodes={LunCoAnnotations.PlotNode(...)}))`.
pub fn add_plot_node(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    plot: &pretty::LunCoPlotNodeSpec,
) -> Result<(), AstMutError> {
    let rendered = pretty::lunco_plot_node_inner(plot);
    let entry = super::parsing::parse_plot_node_record(&rendered)?;

    if let Some(array) = resolve_array(class, edit, "__LunCo", "plotNodes", &rendered)? {
        let source = edit.source();
        let args = text::split_args(source, array.clone());
        match plot_node_arg(source, &args, &plot.signal) {
            Some(i) => edit.replace(args[i].clone(), rendered.clone()),
            None => clause::append_entry(edit, array, &rendered),
        }
    }

    let arr = lunco_plot_nodes_array_mut(class);
    let signal = plot.signal.clone();
    match arr
        .iter_mut()
        .find(|e| super::util::plot_node_signal_matches(e, &signal))
    {
        Some(slot) => *slot = entry,
        None => arr.push(entry),
    }
    Ok(())
}

/// Remove the `PlotNode(signal="…")` entry, dropping the `__LunCo` annotation
/// when it becomes empty.
pub fn remove_plot_node(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    signal_path: &str,
) -> Result<(), AstMutError> {
    let not_found = || AstMutError::PlotNodeNotFound {
        class: class.name.text.to_string(),
        signal: signal_path.to_string(),
    };
    let source = edit.source();
    let (_, annotation) = text::class_annotation_clause(source, class).ok_or_else(not_found)?;
    let section = clause::call_group(source, annotation.clone(), "__LunCo").ok_or_else(not_found)?;
    let array = clause::arg_value(source, section.clone(), "plotNodes").ok_or_else(not_found)?;
    let args = text::split_args(source, array.clone());
    let i = plot_node_arg(source, &args, signal_path).ok_or_else(not_found)?;

    // Removing the last plot node leaves `__LunCo(plotNodes={})`, which is just
    // clutter — drop the whole `__LunCo` section instead.
    if args.len() == 1 {
        let sibling_args = text::split_args(source, annotation.clone());
        match text::find_arg(source, &sibling_args, "__LunCo") {
            Some(idx) if sibling_args.len() == 1 => {
                // `__LunCo` was the only thing in the annotation: remove the
                // whole `annotation(...);` statement.
                let stmt_start = text::class_annotation_clause(source, class)
                    .map(|(kw, _)| kw)
                    .ok_or_else(not_found)?;
                let stmt_end = text::statement_end(source, annotation.end).ok_or_else(|| {
                    AstMutError::AnchorNotFound {
                        what: "terminating `;` of the class annotation".to_string(),
                    }
                })?;
                edit.delete(text::line_extent(source, stmt_start..stmt_end));
                let _ = idx;
            }
            Some(idx) => clause::remove_entry(edit, &sibling_args, idx),
            None => return Err(not_found()),
        }
    } else {
        clause::remove_entry(edit, &args, i);
    }

    let arr = lunco_plot_nodes_array_mut(class);
    arr.retain(|e| !super::util::plot_node_signal_matches(e, signal_path));
    prune_empty_lunco_annotation(class);
    Ok(())
}

/// Update the `extent=` of a plot node by signal.
pub fn set_plot_node_extent(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    signal_path: &str,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
) -> Result<(), AstMutError> {
    update_plot_node_by_signal(class, edit, signal_path, |spec| {
        spec.x1 = x1;
        spec.y1 = y1;
        spec.x2 = x2;
        spec.y2 = y2;
    })
}

/// Update the `title=` of a plot node by signal.
pub fn set_plot_node_title(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    signal_path: &str,
    title: &str,
) -> Result<(), AstMutError> {
    update_plot_node_by_signal(class, edit, signal_path, |spec| {
        spec.title = title.to_string();
    })
}

fn update_plot_node_by_signal<F>(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    signal_path: &str,
    update: F,
) -> Result<(), AstMutError>
where
    F: FnOnce(&mut pretty::LunCoPlotNodeSpec),
{
    let not_found = || AstMutError::PlotNodeNotFound {
        class: class.name.text.to_string(),
        signal: signal_path.to_string(),
    };
    let source = edit.source();
    let (_, annotation) = text::class_annotation_clause(source, class).ok_or_else(not_found)?;
    let section = clause::call_group(source, annotation, "__LunCo").ok_or_else(not_found)?;
    let array = clause::arg_value(source, section, "plotNodes").ok_or_else(not_found)?;
    let args = text::split_args(source, array);
    let i = plot_node_arg(source, &args, signal_path).ok_or_else(not_found)?;

    let arr = lunco_plot_nodes_array_mut(class);
    let slot = arr
        .iter_mut()
        .find(|e| super::util::plot_node_signal_matches(e, signal_path))
        .ok_or_else(|| AstMutError::PlotNodeNotFound {
            class: String::new(),
            signal: signal_path.to_string(),
        })?;
    let mut spec = read_plot_node_spec(slot);
    update(&mut spec);
    let rendered = pretty::lunco_plot_node_inner(&spec);
    *slot = super::parsing::parse_plot_node_record(&rendered)?;

    edit.replace(args[i].clone(), rendered);
    Ok(())
}

/// Index of the `LunCoAnnotations.PlotNode(signal="…")` arg matching `signal`.
fn plot_node_arg(source: &str, args: &[Range<usize>], signal: &str) -> Option<usize> {
    let needle = format!("signal=\"{}\"", signal.replace('\\', "\\\\").replace('"', "\\\""));
    let squished = |s: &str| s.split_whitespace().collect::<String>();
    args.iter()
        .position(|r| squished(&source[r.clone()]).contains(&squished(&needle)))
}

fn read_plot_node_spec(expr: &Expression) -> pretty::LunCoPlotNodeSpec {
    let mut spec = pretty::LunCoPlotNodeSpec {
        x1: 0.0,
        y1: 0.0,
        x2: 0.0,
        y2: 0.0,
        signal: String::new(),
        title: String::new(),
    };
    if let Some(v) = graphic_entry_arg(expr, "signal") {
        if let Some(s) = string_literal_value(v) {
            spec.signal = s;
        }
    }
    if let Some(v) = graphic_entry_arg(expr, "title") {
        if let Some(s) = string_literal_value(v) {
            spec.title = s;
        }
    }
    if let Some(v) = graphic_entry_arg(expr, "extent") {
        if let Expression::Array { elements: outer, .. } = v {
            if outer.len() == 2 {
                if let (Some((x1, y1)), Some((x2, y2))) = (
                    super::util::point_pair(&outer[0]),
                    super::util::point_pair(&outer[1]),
                ) {
                    spec.x1 = x1;
                    spec.y1 = y1;
                    spec.x2 = x2;
                    spec.y2 = y2;
                }
            }
        }
    }
    spec
}

// ---------------------------------------------------------------------------
// experiment(...)
// ---------------------------------------------------------------------------

/// Set or replace the class-level `experiment(...)` annotation.
pub fn set_experiment(
    class: &mut ClassDef,
    edit: &mut Edit<'_>,
    start_time: f64,
    stop_time: f64,
    tolerance: f64,
    interval: f64,
) -> Result<(), AstMutError> {
    let rendered = pretty::experiment_inner(start_time, stop_time, tolerance, interval);
    if let Some(group) = resolve_class_annotation(class, edit, &rendered)? {
        clause::upsert_arg(edit, group, "experiment", &rendered);
    }

    let new_expr =
        super::parsing::parse_experiment_expression(start_time, stop_time, tolerance, interval)?;
    match class
        .annotation
        .iter_mut()
        .find(|e| super::util::is_annotation_entry_named(e, "experiment"))
    {
        Some(slot) => *slot = new_expr,
        None => class.annotation.push(new_expr),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// AST-side mirrors
//
// These keep the cloned AST in step with the splices above. They are *not* the
// source of truth for the text — nothing here is ever emitted back to source.
// ---------------------------------------------------------------------------

/// The `graphics={…}` array inside the class's `Icon`/`Diagram` annotation.
fn graphics_array_mut<'a>(class: &'a mut ClassDef, section_name: &str) -> &'a mut Vec<Expression> {
    nested_array_mut(class, section_name, "graphics")
}

/// The `plotNodes={…}` array inside the class's `__LunCo` annotation.
fn lunco_plot_nodes_array_mut(class: &mut ClassDef) -> &mut Vec<Expression> {
    nested_array_mut(class, "__LunCo", "plotNodes")
}

/// `annotation(<section>(<key>={…}))` in the AST, inserting missing levels.
fn nested_array_mut<'a>(
    class: &'a mut ClassDef,
    section_name: &str,
    key: &str,
) -> &'a mut Vec<Expression> {
    use rumoca_compile::parsing::ast::{ComponentRefPart, ComponentReference};
    use rumoca_compile::parsing::Span;
    use std::sync::Arc;

    let cref = |name: &str| ComponentReference {
        local: false,
        parts: vec![ComponentRefPart {
            ident: super::util::synth_token(name.to_string()),
            subs: None,
        }],
        span: Span::DUMMY,
        def_id: None,
    };

    let section_idx = class
        .annotation
        .iter()
        .position(|e| super::util::is_annotation_entry_named(e, section_name))
        .unwrap_or_else(|| {
            class.annotation.push(Expression::ClassModification {
                target: cref(section_name),
                modifications: Vec::new(),
                each_flags: Vec::new(),
                final_flags: Vec::new(),
                redeclare_flags: Vec::new(),
                span: Span::DUMMY,
            });
            class.annotation.len() - 1
        });

    let mods = match &mut class.annotation[section_idx] {
        Expression::ClassModification { modifications, .. } => modifications,
        _ => unreachable!("section is a ClassModification on insert/find"),
    };

    let key_idx = mods
        .iter()
        .position(|m| {
            matches!(
                m,
                Expression::Modification { target, .. }
                    if target.parts.len() == 1 && &*target.parts[0].ident.text == key
            )
        })
        .unwrap_or_else(|| {
            mods.push(Expression::Modification {
                target: cref(key),
                value: Arc::new(Expression::Array {
                    elements: Vec::new(),
                    is_matrix: false,
                    span: Span::DUMMY,
                }),
                span: Span::DUMMY,
            });
            mods.len() - 1
        });

    let value = match &mut mods[key_idx] {
        Expression::Modification { value, .. } => Arc::make_mut(value),
        _ => unreachable!("key modification just inserted/found above"),
    };
    if !matches!(value, Expression::Array { .. }) {
        *value = Expression::Array {
            elements: Vec::new(),
            is_matrix: false,
            span: Span::DUMMY,
        };
    }
    match value {
        Expression::Array { elements, .. } => elements,
        _ => unreachable!("just ensured an Array variant"),
    }
}

/// Drop the `__LunCo` annotation when its `plotNodes` array is empty.
fn prune_empty_lunco_annotation(class: &mut ClassDef) {
    let Some(idx) = class
        .annotation
        .iter()
        .position(|e| super::util::is_annotation_entry_named(e, "__LunCo"))
    else {
        return;
    };
    let is_empty = match &class.annotation[idx] {
        Expression::ClassModification { modifications, .. } => modifications.iter().all(|m| {
            matches!(
                m,
                Expression::Modification { target, value, .. }
                    if target.parts.len() == 1
                        && &*target.parts[0].ident.text == "plotNodes"
                        && matches!(value.as_ref(), Expression::Array { elements, .. } if elements.is_empty())
            )
        }),
        _ => false,
    };
    if is_empty {
        class.annotation.remove(idx);
    }
}
