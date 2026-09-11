//! Workbench-context adapters for the headless Modelica core.
//!
//! `PanelCtx` and `BrowserCtx` are UI capabilities. Their implementations
//! live here so `lunco-modelica-core` remains usable by servers, workers, and
//! USD simulation without depending on the workbench crates.

use bevy::prelude::{Entity, Resource};
use lunco_doc::DocumentId;
use lunco_modelica_core::{
    class_metadata::{self, ClassMetadata},
    sim_default::{self, ResourceRead},
    state::ModelicaDocumentRegistry,
};
use lunco_workbench_core::PanelCtx;

struct PanelResources<'a, 'ctx>(&'a PanelCtx<'ctx>);

impl<'a, 'ctx> ResourceRead for PanelResources<'a, 'ctx> {
    fn read_resource<T: Resource>(&self) -> Option<&T> {
        self.0.resource::<T>()
    }
}

struct BrowserResources<'a, 'ctx, 'world>(&'a lunco_workbench::BrowserCtx<'ctx, 'world>);

impl<'a, 'ctx, 'world> ResourceRead for BrowserResources<'a, 'ctx, 'world> {
    fn read_resource<T: Resource>(&self) -> Option<&T> {
        self.0.resource::<T>()
    }
}

pub fn drilled_class_for_doc(ctx: &PanelCtx, doc: DocumentId) -> Option<String> {
    let resources = PanelResources(ctx);
    sim_default::drilled_class_for_doc_in(&resources, doc)
}

pub fn default_simulation_class(ctx: &PanelCtx, doc: DocumentId) -> Option<String> {
    let resources = PanelResources(ctx);
    sim_default::default_simulation_class_in(&resources, doc)
}

pub fn resolve_setup_bounds(
    ctx: &PanelCtx,
    doc: DocumentId,
    model_ref: &lunco_experiments::ModelRef,
) -> lunco_experiments::RunBounds {
    let resources = PanelResources(ctx);
    lunco_modelica_core::model_commands::resolve_setup_bounds_in(&resources, doc, model_ref)
}

pub fn detected_name_for(ctx: &PanelCtx, doc: DocumentId) -> Option<String> {
    default_simulation_class(ctx, doc)
}

pub fn read_only_for(ctx: &PanelCtx, doc: DocumentId) -> bool {
    ctx.resource::<ModelicaDocumentRegistry>()
        .and_then(|registry| registry.host(doc))
        .map(|host| host.document().is_read_only())
        .unwrap_or(false)
}

pub fn display_name_for(ctx: &PanelCtx, doc: DocumentId) -> Option<String> {
    ctx.resource::<ModelicaDocumentRegistry>()
        .and_then(|registry| registry.host(doc))
        .map(|host| host.document().origin().display_name())
}

pub fn simulator_for(ctx: &PanelCtx, doc: DocumentId) -> Option<Entity> {
    ctx.resource::<ModelicaDocumentRegistry>()
        .and_then(|registry| registry.simulator_for(doc))
}

pub fn resolve_metadata_for_doc(
    ctx: &PanelCtx,
    doc: DocumentId,
    drilled: Option<&str>,
) -> Option<ClassMetadata> {
    let resources = PanelResources(ctx);
    class_metadata::resolve_metadata_for_doc_in(&resources, doc, drilled)
}

pub fn drilled_class_for_browser_doc(
    ctx: &lunco_workbench::BrowserCtx<'_, '_>,
    doc: DocumentId,
) -> Option<String> {
    let resources = BrowserResources(ctx);
    sim_default::drilled_class_for_doc_in(&resources, doc)
}
