//! Shared Modelica run-bound resolution for UI and API callers.

use bevy::prelude::World;
use lunco_doc::DocumentId;
use lunco_doc_bevy::DocumentRegistry;
use lunco_modelica_core::sim_default::ResourceRead;
use lunco_modelica_document::ModelicaDocument;

/// Read the `experiment(...)` annotation bounds for a model from live document
/// state. `None` means the class or annotation is absent.
pub fn bounds_from_annotation_in<R: ResourceRead>(
    ctx: &R,
    doc: DocumentId,
    model_ref: &lunco_experiments::ModelRef,
) -> Option<lunco_experiments::RunBounds> {
    let registry = ctx.read_resource::<DocumentRegistry<ModelicaDocument>>()?;
    let host = registry.host(doc)?;
    let class = host
        .document()
        .index()
        .classes
        .get(&model_ref.0)
        .or_else(|| {
            host.document()
                .index()
                .classes
                .values()
                .find(|c| c.name == model_ref.0)
        })?;
    let experiment = class.experiment.as_ref()?;
    lunco_modelica_core::sim_target::bounds_from_experiment(experiment)
}

/// `&World` reader for [`bounds_from_annotation_in`].
pub fn bounds_from_annotation(
    world: &World,
    doc: DocumentId,
    model_ref: &lunco_experiments::ModelRef,
) -> Option<lunco_experiments::RunBounds> {
    bounds_from_annotation_in(world, doc, model_ref)
}

/// Resolve the single run-bound precedence used by the Fast Run surfaces:
/// saved draft override, current document AST annotation, then the documented
/// one-second default.
pub fn resolve_setup_bounds_in<R: ResourceRead>(
    ctx: &R,
    doc: DocumentId,
    model_ref: &lunco_experiments::ModelRef,
) -> lunco_experiments::RunBounds {
    let draft = ctx
        .read_resource::<crate::runner::ExperimentDrafts>()
        .and_then(|drafts| {
            drafts
                .get(doc, model_ref)
                .and_then(|draft| draft.bounds_override.clone())
        });
    let annotation = bounds_from_annotation_in(ctx, doc, model_ref);
    lunco_modelica_core::sim_target::resolve_bounds(draft, annotation)
}

/// `&World` reader for [`resolve_setup_bounds_in`].
pub fn resolve_setup_bounds(
    world: &World,
    doc: DocumentId,
    model_ref: &lunco_experiments::ModelRef,
) -> lunco_experiments::RunBounds {
    resolve_setup_bounds_in(world, doc, model_ref)
}
