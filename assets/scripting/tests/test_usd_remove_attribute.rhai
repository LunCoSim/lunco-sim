// Production Rhai/API coverage for the typed USD RemoveAttribute operation.
// `scripts/api/test_remove_attribute_rhai.sh` starts an isolated headless app
// and opens a scratch USD document before running this one-shot test.

import "assembly_edit" as assembly_edit;

const PATH = "/World";
const ATTRIBUTE = "test:removeAttributeProbe";

fn only_usd_document() {
    let documents = query("ListOpenDocuments");
    if documents == () || documents.open_documents == ()
        || documents.open_documents.len() != 1 {
        return -1;
    }
    let document = documents.open_documents[0];
    if document.kind == "usd" { return document.doc_id; }
    -1
}

fn attribute_value(doc, path, name) {
    let inspection = assembly_edit::inspect(doc, path);
    if inspection == () || inspection.prim == () || inspection.prim.attributes == () {
        return ();
    }
    for attribute in inspection.prim.attributes {
        if attribute.name.ends_with("." + name) { return attribute.value; }
    }
    ()
}

fn query_ok(report) {
    report != () && report.ok != false;
}

let checks = [];
let document_id = only_usd_document();
checks.push(t_true(document_id >= 0,
    "the isolated test app exposes exactly one scratch USD document"));

if document_id >= 0 {
    let initial = assembly_edit::describe(document_id);
    checks.push(t_true(query_ok(initial) && initial.generation != (),
        "scratch USD document exposes its generation"));

    if query_ok(initial) && initial.generation != () {
        let base_write = assembly_edit::attribute(document_id, "@root@", PATH,
            ATTRIBUTE, "double", "1.25", initial.generation);
        checks.push(t_true(base_write != () && base_write.ok,
            "Rhai authors the weaker root-layer value"));
        checks.push(t_eq(attribute_value(document_id, PATH, ATTRIBUTE), "1.25",
            "root-layer value is composed before the runtime override"));

        let after_base = assembly_edit::describe(document_id);
        let runtime_write = assembly_edit::attribute(document_id, "@runtime@", PATH,
            ATTRIBUTE, "double", "2.5", after_base.generation);
        checks.push(t_true(runtime_write != () && runtime_write.ok,
            "Rhai authors a stronger runtime-layer override"));
        checks.push(t_eq(attribute_value(document_id, PATH, ATTRIBUTE), "2.5",
            "runtime-layer override is the composed value"));

        let before_remove = assembly_edit::describe(document_id);
        let removed = assembly_edit::remove_attribute(document_id, "@runtime@", PATH,
            ATTRIBUTE, before_remove.generation);
        checks.push(t_true(removed != () && removed.ok,
            "typed RemoveAttribute clears the selected layer opinion"));
        checks.push(t_eq(attribute_value(document_id, PATH, ATTRIBUTE), "1.25",
            "removing the runtime opinion reveals the weaker root value"));

        if removed != () && removed.ok {
            let undone = assembly_edit::undo(document_id);
            checks.push(t_true(undone != () && undone.ok,
                "RemoveAttribute participates in document undo"));
            checks.push(t_eq(attribute_value(document_id, PATH, ATTRIBUTE), "2.5",
                "undo restores the stronger runtime value"));

            let redone = assembly_edit::redo(document_id);
            checks.push(t_true(redone != () && redone.ok,
                "RemoveAttribute participates in document redo"));
            checks.push(t_eq(attribute_value(document_id, PATH, ATTRIBUTE), "1.25",
                "redo removes the runtime opinion and reveals the root value"));

            let before_repeat = assembly_edit::describe(document_id);
            let repeated = assembly_edit::remove_attribute(document_id, "@runtime@", PATH,
                ATTRIBUTE, before_repeat.generation);
            checks.push(t_true(repeated != () && repeated.ok,
                "clearing an already-clear runtime layer remains a valid operation"));
            checks.push(t_eq(attribute_value(document_id, PATH, ATTRIBUTE), "1.25",
                "a repeated runtime clear preserves the weaker root value"));
        }
    }
}

t_report(checks);
