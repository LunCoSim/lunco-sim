// Generic component authoring facade.
//
// Component recipes stay in the owning Twin or model package. This library
// only composes the shared authoring context, schema-driven property catalog,
// and topology-preserving bundle planner so a human-facing Editor assistant
// and an AI use one discoverable contract. It never guesses a recipe and
// never commits a USD edit.
import "assembly_builder" as builder;

fn component_error(message) {
    #{ ok: false, kind: "component_editor", error: message, ops: [] }
}

/// Return the complete read-only context needed before updating one component.
/// `requested == ()` discovers the editable standard USD fields; an explicit
/// array keeps an Inspector focused. The caller supplies the component bundle
/// to `update_plan`, because USD has no standard parametric-recipe schema.
fn update_context(doc, edit_target, path, requested) {
    if doc == () || edit_target == () || path == () {
        return component_error(
            "component_editor::update_context requires document, edit target, and exact component path",
        );
    }
    let context = builder::authoring_context(doc, path, edit_target);
    if context == () || context.ok != true {
        return context;
    }
    if context.prim == () || context.prim.type_name != "Xform" {
        return component_error(path + ": component editor requires an existing Xform root");
    }
    let catalog = builder::editable_property_catalog(doc, path, edit_target, requested);
    if catalog == () || catalog.ok != true {
        return catalog;
    }
    #{
        ok: true,
        kind: "component_update_context",
        doc_id: context.doc_id,
        path: context.path,
        edit_target: context.edit_target,
        generation: context.generation,
        component: context,
        properties: catalog,
        operation: #{
            name: "Update component",
            planner: "component_editor::update_plan",
            implementation: "assembly_builder::component_bundle_update_plan",
            mode: "dry_plan",
            inputs: ["bundle", "bindings"],
            commit: "assembly_edit::propose -> review_session -> commit_proposal",
            topology_preserved: true,
        },
    }
}

/// Read the currently selected component without copying a path from a tab or
/// display label. `preview == ()` uses the focused preview; an explicit
/// preview reads a hidden session without changing focus.
fn selected_update_context(preview, requested) {
    let context = builder::selected_authoring_context(preview);
    if context == () || context.ok != true {
        return context;
    }
    update_context(context.doc_id, context.edit_target, context.path, requested)
}

/// Build the existing topology-preserving component regeneration plan and add
/// the facade metadata consumed by Editor/AI clients. The returned `.ops` are
/// still dry; review and commit them through `assembly_edit`.
fn update_plan(doc, edit_target, path, bundle, bindings) {
    let plan = builder::component_bundle_update_plan(
        doc, edit_target, path, bundle, bindings,
    );
    if plan == () || plan.ok != true {
        return plan;
    }
    #{
        ok: true,
        kind: "component_update_plan",
        doc_id: doc,
        edit_target: edit_target,
        path: path,
        planner: "assembly_builder::component_bundle_update_plan",
        topology_preserved: plan.topology_preserved,
        bindings: plan.bindings,
        no_op: plan.no_op,
        ops: plan.ops,
    }
}
