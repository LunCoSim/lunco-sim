//! Annotation graphics mutation helpers.

use std::sync::Arc;
use rumoca_compile::parsing::ast::{ClassDef, Expression};
use rumoca_compile::parsing::Span;
use super::errors::AstMutError;
use super::util::{is_annotation_entry_named, synth_token, is_graphic_entry_named, graphic_entry_arg, string_literal_value, point_pair, render_text_spec, read_text_spec, plot_node_signal_matches};
use super::parsing::{parse_graphics_entry, parse_experiment_expression, parse_placement_expression, parse_plot_node_record};
use crate::pretty;

/// Append a graphic to `class.annotation.<section>(graphics)`.
fn append_graphic_to_section(
    class: &mut ClassDef,
    section_name: &str,
    graphic_text: &str,
) -> Result<(), AstMutError> {
    let entry = parse_graphics_entry(graphic_text)?;
    let arr = graphics_array_mut(class, section_name);
    arr.push(entry);
    Ok(())
}

/// Get a mutable reference to the `graphics={...}` array inside the
/// class's `Diagram` or `Icon` annotation section.
fn graphics_array_mut<'a>(
    class: &'a mut ClassDef,
    section_name: &str,
) -> &'a mut Vec<Expression> {
    let section_idx = class
        .annotation
        .iter()
        .position(|e| is_annotation_entry_named(e, section_name));
    let section_idx = match section_idx {
        Some(i) => i,
        None => {
            class.annotation.push(Expression::ClassModification {
                target: rumoca_compile::parsing::ast::ComponentReference {
                    local: false,
                    parts: vec![rumoca_compile::parsing::ast::ComponentRefPart {
                        ident: synth_token(section_name.to_string()),
                        subs: None,
                    }],
                    span: Span::DUMMY,
                    def_id: None,
                },
                modifications: Vec::new(),
                each_flags: Vec::new(),
                final_flags: Vec::new(),
                redeclare_flags: Vec::new(),
                span: Span::DUMMY,
            });
            class.annotation.len() - 1
        }
    };
    let mods = match &mut class.annotation[section_idx] {
        Expression::ClassModification { modifications, .. } => modifications,
        _ => unreachable!("section was a ClassModification on insert/find"),
    };

    let graphics_idx = mods.iter().position(|m| {
        matches!(
            m,
            Expression::Modification { target, .. }
                if target.parts.len() == 1
                    && &*target.parts[0].ident.text == "graphics"
        )
    });
    let graphics_idx = match graphics_idx {
        Some(i) => i,
        None => {
            mods.push(Expression::Modification {
                target: rumoca_compile::parsing::ast::ComponentReference {
                    local: false,
                    parts: vec![rumoca_compile::parsing::ast::ComponentRefPart {
                        ident: synth_token("graphics".to_string()),
                        subs: None,
                    }],
                    span: Span::DUMMY,
                    def_id: None,
                },
                value: Arc::new(Expression::Array {
                    elements: Vec::new(),
                    is_matrix: false,
                    span: Span::DUMMY,
                }),
                span: Span::DUMMY,
            });
            mods.len() - 1
        }
    };
    let graphics_value = match &mut mods[graphics_idx] {
        Expression::Modification { value, .. } => Arc::make_mut(value),
        _ => unreachable!("graphics modification just inserted/found above"),
    };
    match graphics_value {
        Expression::Array { elements, .. } => elements,
        other => {
            *other = Expression::Array {
                elements: Vec::new(),
                is_matrix: false,
                span: Span::DUMMY,
            };
            match other {
                Expression::Array { elements, .. } => elements,
                _ => unreachable!("just assigned an Array variant"),
            }
        }
    }
}

/// Get a mutable reference to the `plotNodes={...}` array inside the
/// class's `__LunCo(...)` vendor annotation. Inserts the annotation
/// and/or the `plotNodes` modification if they don't yet exist.
fn lunco_plot_nodes_array_mut(class: &mut ClassDef) -> &mut Vec<Expression> {
    let section_idx = class
        .annotation
        .iter()
        .position(|e| is_annotation_entry_named(e, "__LunCo"));
    let section_idx = match section_idx {
        Some(i) => i,
        None => {
            class.annotation.push(Expression::ClassModification {
                target: rumoca_compile::parsing::ast::ComponentReference {
                    local: false,
                    parts: vec![rumoca_compile::parsing::ast::ComponentRefPart {
                        ident: synth_token("__LunCo".to_string()),
                        subs: None,
                    }],
                    span: Span::DUMMY,
                    def_id: None,
                },
                modifications: Vec::new(),
                each_flags: Vec::new(),
                final_flags: Vec::new(),
                redeclare_flags: Vec::new(),
                span: Span::DUMMY,
            });
            class.annotation.len() - 1
        }
    };
    let mods = match &mut class.annotation[section_idx] {
        Expression::ClassModification { modifications, .. } => modifications,
        _ => unreachable!("__LunCo entry was a ClassModification on insert/find"),
    };

    let plot_nodes_idx = mods.iter().position(|m| matches!(
        m,
        Expression::Modification { target, .. }
            if target.parts.len() == 1
                && &*target.parts[0].ident.text == "plotNodes"
    ));
    let plot_nodes_idx = match plot_nodes_idx {
        Some(i) => i,
        None => {
            mods.push(Expression::Modification {
                target: rumoca_compile::parsing::ast::ComponentReference {
                    local: false,
                    parts: vec![rumoca_compile::parsing::ast::ComponentRefPart {
                        ident: synth_token("plotNodes".to_string()),
                        subs: None,
                    }],
                    span: Span::DUMMY,
                    def_id: None,
                },
                value: Arc::new(Expression::Array {
                    elements: Vec::new(),
                    is_matrix: false,
                    span: Span::DUMMY,
                }),
                span: Span::DUMMY,
            });
            mods.len() - 1
        }
    };
    let value = match &mut mods[plot_nodes_idx] {
        Expression::Modification { value, .. } => Arc::make_mut(value),
        _ => unreachable!("plotNodes modification just inserted/found above"),
    };
    match value {
        Expression::Array { elements, .. } => elements,
        other => {
            *other = Expression::Array {
                elements: Vec::new(),
                is_matrix: false,
                span: Span::DUMMY,
            };
            match other {
                Expression::Array { elements, .. } => elements,
                _ => unreachable!("just assigned an Array variant"),
            }
        }
    }
}

/// Drop the class's `__LunCo` annotation entirely if its `plotNodes`
/// array is now empty. Keeps source clean after removing the last
/// plot node — no `annotation(__LunCo(plotNodes={}))` clutter.
fn prune_empty_lunco_annotation(class: &mut ClassDef) {
    let Some(idx) = class
        .annotation
        .iter()
        .position(|e| is_annotation_entry_named(e, "__LunCo"))
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

/// Add or replace a plot tile in the class's
/// `annotation(__LunCo(plotNodes={LunCoAnnotations.PlotNode(...)}))`.
pub fn add_plot_node(
    class: &mut ClassDef,
    plot: &pretty::LunCoPlotNodeSpec,
) -> Result<(), AstMutError> {
    let new_entry = parse_plot_node_record(&pretty::lunco_plot_node_inner(plot))?;
    let arr = lunco_plot_nodes_array_mut(class);
    let signal = plot.signal.clone();
    if let Some(slot) = arr
        .iter_mut()
        .find(|e| plot_node_signal_matches(e, &signal))
    {
        *slot = new_entry;
    } else {
        arr.push(new_entry);
    }
    Ok(())
}

/// Remove the `LunCoAnnotations.PlotNode(...)` entry whose `signal=`
/// matches. Drops the enclosing `__LunCo` annotation if it becomes
/// empty as a result.
pub fn remove_plot_node(class: &mut ClassDef, signal_path: &str) -> Result<(), AstMutError> {
    let class_name = class.name.text.to_string();
    let arr = lunco_plot_nodes_array_mut(class);
    let before = arr.len();
    arr.retain(|e| !plot_node_signal_matches(e, signal_path));
    if arr.len() == before {
        return Err(AstMutError::PlotNodeNotFound {
            class: class_name,
            signal: signal_path.to_string(),
        });
    }
    prune_empty_lunco_annotation(class);
    Ok(())
}

/// Update the `extent={{x1,y1},{x2,y2}}` of a plot node by signal.
pub fn set_plot_node_extent(
    class: &mut ClassDef,
    signal_path: &str,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
) -> Result<(), AstMutError> {
    update_plot_node_by_signal(class, signal_path, |spec| {
        spec.x1 = x1;
        spec.y1 = y1;
        spec.x2 = x2;
        spec.y2 = y2;
    })
}

/// Update the `title=` of a plot node by signal.
pub fn set_plot_node_title(
    class: &mut ClassDef,
    signal_path: &str,
    title: &str,
) -> Result<(), AstMutError> {
    update_plot_node_by_signal(class, signal_path, |spec| {
        spec.title = title.to_string();
    })
}

fn update_plot_node_by_signal<F>(
    class: &mut ClassDef,
    signal_path: &str,
    update: F,
) -> Result<(), AstMutError>
where
    F: FnOnce(&mut pretty::LunCoPlotNodeSpec),
{
    let class_name = class.name.text.to_string();
    let arr = lunco_plot_nodes_array_mut(class);
    let entry = arr
        .iter_mut()
        .find(|e| plot_node_signal_matches(e, signal_path))
        .ok_or_else(|| AstMutError::PlotNodeNotFound {
            class: class_name,
            signal: signal_path.to_string(),
        })?;
    let mut spec = read_plot_node_spec(entry);
    update(&mut spec);
    *entry = parse_plot_node_record(&pretty::lunco_plot_node_inner(&spec))?;
    Ok(())
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
                if let (Some((x1, y1)), Some((x2, y2))) =
                    (point_pair(&outer[0]), point_pair(&outer[1]))
                {
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

/// Set or replace the `extent` of the i-th `Text(...)` entry in `Diagram(graphics)`.
pub fn set_diagram_text_extent(
    class: &mut ClassDef,
    index: usize,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
) -> Result<(), AstMutError> {
    update_diagram_text_at(class, index, |spec| {
        spec.x1 = x1;
        spec.y1 = y1;
        spec.x2 = x2;
        spec.y2 = y2;
    })
}

/// Set or replace the `textString=` of the i-th `Text(...)` entry.
pub fn set_diagram_text_string(
    class: &mut ClassDef,
    index: usize,
    text: &str,
) -> Result<(), AstMutError> {
    update_diagram_text_at(class, index, |spec| {
        spec.text = text.to_string();
    })
}

/// Remove the i-th `Text(...)` entry from `Diagram(graphics)`.
pub fn remove_diagram_text(class: &mut ClassDef, index: usize) -> Result<(), AstMutError> {
    let class_name = class.name.text.to_string();
    let arr = graphics_array_mut(class, "Diagram");
    let mut text_seen = 0usize;
    let mut target_idx = None;
    for (i, e) in arr.iter().enumerate() {
        if is_graphic_entry_named(e, "Text") {
            if text_seen == index {
                target_idx = Some(i);
                break;
            }
            text_seen += 1;
        }
    }
    let i = target_idx.ok_or(AstMutError::DiagramTextIndexOutOfRange {
        class: class_name,
        index,
    })?;
    arr.remove(i);
    Ok(())
}

fn update_diagram_text_at<F>(
    class: &mut ClassDef,
    index: usize,
    update: F,
) -> Result<(), AstMutError>
where
    F: FnOnce(&mut super::util::TextSpec),
{
    let class_name = class.name.text.to_string();
    let arr = graphics_array_mut(class, "Diagram");
    let mut text_seen = 0usize;
    let mut target_idx = None;
    for (i, e) in arr.iter().enumerate() {
        if is_graphic_entry_named(e, "Text") {
            if text_seen == index {
                target_idx = Some(i);
                break;
            }
            text_seen += 1;
        }
    }
    let i = target_idx.ok_or(AstMutError::DiagramTextIndexOutOfRange {
        class: class_name,
        index,
    })?;
    let mut spec = read_text_spec(&arr[i]);
    update(&mut spec);
    arr[i] = parse_graphics_entry(&render_text_spec(&spec))?;
    Ok(())
}

/// Append a graphic to `Icon(graphics)` or `Diagram(graphics)`.
pub fn add_named_graphic(
    class: &mut ClassDef,
    section_name: &str,
    graphic_text: &str,
) -> Result<(), AstMutError> {
    append_graphic_to_section(class, section_name, graphic_text)
}

/// Set or replace the class-level `experiment(...)` annotation.
pub fn set_experiment(
    class: &mut ClassDef,
    start_time: f64,
    stop_time: f64,
    tolerance: f64,
    interval: f64,
) -> Result<(), AstMutError> {
    let new_expr = parse_experiment_expression(start_time, stop_time, tolerance, interval)?;
    if let Some(slot) = class
        .annotation
        .iter_mut()
        .find(|expr| is_annotation_entry_named(expr, "experiment"))
    {
        *slot = new_expr;
    } else {
        class.annotation.push(new_expr);
    }
    Ok(())
}

/// Set or replace the `Placement(...)` annotation on a component.
pub fn set_placement(
    class: &mut ClassDef,
    component: &str,
    placement: &pretty::Placement,
) -> Result<(), AstMutError> {
    let class_name = class.name.text.to_string();
    let comp = class
        .components
        .get_mut(component)
        .ok_or_else(|| AstMutError::ComponentNotFound {
            class: class_name,
            component: component.to_string(),
        })?;
    let new_placement_expr = parse_placement_expression(placement)?;
    if let Some(slot) = comp
        .annotation
        .iter_mut()
        .find(|expr| is_annotation_entry_named(expr, "Placement"))
    {
        *slot = new_placement_expr;
    } else {
        comp.annotation.push(new_placement_expr);
    }
    Ok(())
}
