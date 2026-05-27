//! Implementation of operation-to-patch translation.

use std::ops::Range;
use lunco_doc::DocumentError;
use rumoca_compile::parsing::ast::StoredDefinition;

use super::ops::{ModelicaChange, ModelicaOp, FreshAst};
use super::core::AstCache;
use crate::pretty;

/// Map the op-layer's [`pretty::ClassKindSpec`] to the Index's
/// [`crate::index::ClassKind`].
/// Inverse of [`class_kind_spec_to_index_kind`]. Used when the
/// rebuild-time class diff emits `ClassAdded` for a class first
/// seen in the index — we need the op-layer kind keyword.
pub fn index_kind_to_class_kind_spec(kind: crate::index::ClassKind) -> pretty::ClassKindSpec {
    use crate::index::ClassKind;
    match kind {
        ClassKind::Model => pretty::ClassKindSpec::Model,
        ClassKind::Block => pretty::ClassKindSpec::Block,
        ClassKind::Connector => pretty::ClassKindSpec::Connector,
        ClassKind::Package => pretty::ClassKindSpec::Package,
        ClassKind::Record => pretty::ClassKindSpec::Record,
        ClassKind::Function => pretty::ClassKindSpec::Function,
        ClassKind::Type => pretty::ClassKindSpec::Type,
        // Class kinds without a direct pretty mapping fall back to
        // Model — the diff path only uses this for change-history
        // metadata, not source generation.
        _ => pretty::ClassKindSpec::Model,
    }
}

pub fn class_kind_spec_to_index_kind(spec: pretty::ClassKindSpec) -> crate::index::ClassKind {
    use crate::index::ClassKind;
    match spec {
        pretty::ClassKindSpec::Model => ClassKind::Model,
        pretty::ClassKindSpec::Block => ClassKind::Block,
        pretty::ClassKindSpec::Connector => ClassKind::Connector,
        pretty::ClassKindSpec::Package => ClassKind::Package,
        pretty::ClassKindSpec::Record => ClassKind::Record,
        pretty::ClassKindSpec::Function => ClassKind::Function,
        pretty::ClassKindSpec::Type => ClassKind::Type,
    }
}

pub fn ast_check_no_parse_error(ast: &AstCache) -> Result<(), DocumentError> {
    if let Some(msg) = ast.first_error() {
        return Err(DocumentError::ValidationFailed(format!(
            "cannot apply AST op while source has a parse error: {}",
            msg
        )));
    }
    Ok(())
}

pub fn ast_mut_to_doc_error(e: crate::ast_mut::AstMutError) -> DocumentError {
    DocumentError::ValidationFailed(e.to_string())
}

/// Translate a high-level [`ModelicaOp`] into the concrete text patch
/// and the structured change it represents.
pub fn op_to_patch(
    source: &str,
    ast: &AstCache,
    parsed: &StoredDefinition,
    op: ModelicaOp,
) -> Result<
    (
        Range<usize>,
        String,
        ModelicaChange,
        FreshAst,
    ),
    DocumentError,
> {
    match op {
        ModelicaOp::ReplaceSource { new } => Ok((
            0..source.len(),
            new,
            ModelicaChange::TextReplaced,
            FreshAst::TextEdit,
        )),
        ModelicaOp::EditText { range, replacement } => {
            Ok((range, replacement, ModelicaChange::TextReplaced, FreshAst::TextEdit))
        }
        ModelicaOp::AddComponent { class, decl } => {
            ast_check_no_parse_error(ast)?;
            let added_name = decl.name.clone();
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::add_component(c, &decl),
            )
            .map_err(ast_mut_to_doc_error)?;
            let change = ModelicaChange::ComponentAdded {
                class,
                name: added_name,
            };
            Ok((r, rp, change, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::AddConnection { class, eq } => {
            ast_check_no_parse_error(ast)?;
            let from = eq.from.clone();
            let to = eq.to.clone();
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::add_connection(c, &eq),
            )
            .map_err(ast_mut_to_doc_error)?;
            let change = ModelicaChange::ConnectionAdded { class, from, to };
            Ok((r, rp, change, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::RemoveComponent { class, name } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::remove_component(c, &name),
            )
            .map_err(ast_mut_to_doc_error)?;
            let change = ModelicaChange::ComponentRemoved { class, name };
            Ok((r, rp, change, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::RemoveConnection { class, from, to } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::remove_connection(c, &from, &to),
            )
            .map_err(ast_mut_to_doc_error)?;
            let change = ModelicaChange::ConnectionRemoved { class, from, to };
            Ok((r, rp, change, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::SetConnectionLine { class, from, to, points } => {
            ast_check_no_parse_error(ast)?;
            let from_c = from.clone();
            let to_c = to.clone();
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::set_connection_line(c, &from_c, &to_c, &points),
            )
            .map_err(ast_mut_to_doc_error)?;
            let change = ModelicaChange::ConnectionLineChanged { class, from, to };
            Ok((r, rp, change, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::SetConnectionLineStyle {
            class,
            from,
            to,
            color,
            thickness,
            smooth_bezier,
        } => {
            ast_check_no_parse_error(ast)?;
            let from_c = from.clone();
            let to_c = to.clone();
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::set_connection_line_style(
                    c, &from_c, &to_c, color, thickness, smooth_bezier,
                ),
            )
            .map_err(ast_mut_to_doc_error)?;
            let change = ModelicaChange::ConnectionLineStyleChanged { class, from, to };
            Ok((r, rp, change, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::ReverseConnection { class, from, to } => {
            ast_check_no_parse_error(ast)?;
            let from_c = from.clone();
            let to_c = to.clone();
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::reverse_connection(c, &from_c, &to_c),
            )
            .map_err(ast_mut_to_doc_error)?;
            let change = ModelicaChange::ConnectionReversed {
                class,
                from: to,
                to: from,
            };
            Ok((r, rp, change, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::SetPlacement { class, name, placement } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::set_placement(c, &name, &placement),
            )
            .map_err(ast_mut_to_doc_error)?;
            let change = ModelicaChange::PlacementChanged {
                class,
                component: name,
                placement,
            };
            Ok((r, rp, change, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::SetParameter { class, component, param, value } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::set_parameter(c, &component, &param, &value),
            )
            .map_err(ast_mut_to_doc_error)?;
            let change = ModelicaChange::ParameterChanged {
                class,
                component,
                param,
                value,
            };
            Ok((r, rp, change, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::AddPlotNode { class, plot } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::add_plot_node(c, &plot),
            )
            .map_err(ast_mut_to_doc_error)?;
            Ok((r, rp, ModelicaChange::TextReplaced, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::RemovePlotNode { class, signal_path } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::remove_plot_node(c, &signal_path),
            )
            .map_err(ast_mut_to_doc_error)?;
            Ok((r, rp, ModelicaChange::TextReplaced, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::SetPlotNodeExtent { class, signal_path, x1, y1, x2, y2 } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::set_plot_node_extent(c, &signal_path, x1, y1, x2, y2),
            )
            .map_err(ast_mut_to_doc_error)?;
            Ok((r, rp, ModelicaChange::TextReplaced, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::SetPlotNodeTitle { class, signal_path, title } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::set_plot_node_title(c, &signal_path, &title),
            )
            .map_err(ast_mut_to_doc_error)?;
            Ok((r, rp, ModelicaChange::TextReplaced, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::SetDiagramTextExtent { class, index, x1, y1, x2, y2 } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::set_diagram_text_extent(c, index, x1, y1, x2, y2),
            )
            .map_err(ast_mut_to_doc_error)?;
            Ok((r, rp, ModelicaChange::TextReplaced, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::SetDiagramTextString { class, index, text } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::set_diagram_text_string(c, index, &text),
            )
            .map_err(ast_mut_to_doc_error)?;
            Ok((r, rp, ModelicaChange::TextReplaced, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::RemoveDiagramText { class, index } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::remove_diagram_text(c, index),
            )
            .map_err(ast_mut_to_doc_error)?;
            Ok((r, rp, ModelicaChange::TextReplaced, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::AddClass { parent, name, kind, description, partial } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_document_patch(source, parsed, |sd| {
                crate::ast_mut::add_class(sd, &parent, &name, kind, &description, partial)
            })
            .map_err(ast_mut_to_doc_error)?;
            let qualified = if parent.is_empty() {
                name
            } else {
                format!("{}.{}", parent, name)
            };
            Ok((r, rp, ModelicaChange::ClassAdded { qualified, kind }, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::RemoveClass { qualified } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_document_patch(source, parsed, |sd| {
                crate::ast_mut::remove_class(sd, &qualified)
            })
            .map_err(ast_mut_to_doc_error)?;
            Ok((r, rp, ModelicaChange::ClassRemoved { qualified }, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::AddShortClass { parent, name, kind, base, prefixes, modifications } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_document_patch(source, parsed, |sd| {
                crate::ast_mut::add_short_class(
                    sd, &parent, &name, kind, &base, &prefixes, &modifications,
                )
            })
            .map_err(ast_mut_to_doc_error)?;
            let qualified = if parent.is_empty() {
                name
            } else {
                format!("{}.{}", parent, name)
            };
            Ok((r, rp, ModelicaChange::ClassAdded { qualified, kind }, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::AddVariable { class, decl } => {
            ast_check_no_parse_error(ast)?;
            let added_name = decl.name.clone();
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::add_variable(c, &decl),
            )
            .map_err(ast_mut_to_doc_error)?;
            let change = ModelicaChange::ComponentAdded {
                class,
                name: added_name,
            };
            Ok((r, rp, change, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::RemoveVariable { class, name } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::remove_variable(c, &name),
            )
            .map_err(ast_mut_to_doc_error)?;
            let change = ModelicaChange::ComponentRemoved { class, name };
            Ok((r, rp, change, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::AddEquation { class, eq } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::add_equation(c, &eq),
            )
            .map_err(ast_mut_to_doc_error)?;
            Ok((r, rp, ModelicaChange::TextReplaced, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::AddIconGraphic { class, graphic } => {
            ast_check_no_parse_error(ast)?;
            let graphic_text = crate::pretty::graphic_inner(&graphic);
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::add_named_graphic(c, "Icon", &graphic_text),
            )
            .map_err(ast_mut_to_doc_error)?;
            Ok((r, rp, ModelicaChange::TextReplaced, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::AddDiagramGraphic { class, graphic } => {
            ast_check_no_parse_error(ast)?;
            let graphic_text = crate::pretty::graphic_inner(&graphic);
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::add_named_graphic(c, "Diagram", &graphic_text),
            )
            .map_err(ast_mut_to_doc_error)?;
            Ok((r, rp, ModelicaChange::TextReplaced, FreshAst::Mutated(fresh_ast)))
        }
        ModelicaOp::SetExperimentAnnotation { class, start_time, stop_time, tolerance, interval } => {
            ast_check_no_parse_error(ast)?;
            let (r, rp, fresh_ast) = crate::ast_mut::regenerate_class_patch(
                source,
                parsed,
                &class,
                |c| crate::ast_mut::set_experiment(c, start_time, stop_time, tolerance, interval),
            )
            .map_err(ast_mut_to_doc_error)?;
            Ok((r, rp, ModelicaChange::TextReplaced, FreshAst::Mutated(fresh_ast)))
        }
    }
}

