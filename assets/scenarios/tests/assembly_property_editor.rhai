// TEST-ONLY. Verifies the schema-driven property catalog and typed dry patch
// plan through the normal inspection, proposal, review, journal, and query
// boundary. It covers standard fields, metadata, variants, no-op edits, and
// visible rejection of custom names, wrong types, stale generations, and
// structural fields.

const ROOT = "/AssemblyPropertyEditorTest";
const PANEL = ROOT + "/Assembly/Panel";
const TEMP_KIND = "component_test";

fn query_ok(record) {
    record != () && record.ok != false
}

fn close_enough(actual, expected) {
    actual != () && (actual - expected).abs() <= 0.001
}

fn vector_close(actual, expected) {
    if actual == () || actual.len() != expected.len() { return false; }
    for i in 0..expected.len() {
        if !close_enough(actual[i], expected[i]) { return false; }
    }
    true
}

fn property(catalog, name) {
    if catalog == () || catalog.properties == () { return (); }
    for item in catalog.properties {
        if item.name == name { return item; }
    }
    ()
}

fn composed_kind(inspection) {
    if !query_ok(inspection) || inspection.prim == ()
        || inspection.prim.metadata == ()
        || inspection.prim.metadata.kind == () {
        return ();
    }
    let kind = inspection.prim.metadata.kind.composed;
    if kind == () || kind.present != true { return (); }
    kind.value
}

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "ASSEMBLY PROPERTY EDITOR", "ASSEMBLY_PROPERTY_EDITOR");
    }
}

fn find_document() {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return -1; }
    for item in docs.open_documents {
        // The scene test launches one explicit USD document. Do not gate
        // discovery on a transient composed query while a prior scoped edit
        // is still being projected; the exact prim is checked below through
        // InspectUsdDocument before any authoring command is issued.
        if item.kind == "usd" { return item.doc_id; }
    }
    -1
}

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ListOpenDocuments", "QueryUsdPrim"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.doc = -1;
    this.phase = 0;
    this.elapsed = 0.0;
    this.wait_generation = -1;
    this.kind_override_seen = false;
    this.kind_restore_seen = false;
    this.done = false;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.doc < 0 {
        this.doc = find_document();
        if this.doc < 0 {
            if this.elapsed > 8.0 { fail(this, "property-editor fixture was not discoverable"); }
            return;
        }
    }

    let current = assembly_edit::describe(this.doc);
    if !query_ok(current) || current.generation == () {
        fail(this, "property-editor fixture did not expose a document generation");
        return;
    }

    // SetPrimKind is an authored USD metadata edit, not an asset-free Rust
    // fixture. Exercise the complete Rhai -> typed command -> composition
    // path against a runtime override, then clear it and verify that the
    // weaker component kind is visible again. A restart can leave the
    // temporary override in flight, so resume that explicit phase rather
    // than reporting a false fixture failure.
    if this.phase == 0 {
        let before_kind = composed_kind(assembly_edit::inspect(this.doc, PANEL));
        if before_kind == TEMP_KIND {
            this.wait_generation = current.generation - 1;
            this.phase = 2;
            return;
        }
        if before_kind != "component" {
            fail(this, "fixture panel did not expose its authored component kind");
            return;
        }
        let authored = assembly_edit::prim_kind(
            this.doc, "@runtime@", PANEL, TEMP_KIND, current.generation,
        );
        if !query_ok(authored) {
            fail(this, "runtime kind override was rejected: " + authored.error);
            return;
        }
        this.wait_generation = current.generation;
        this.phase = 2;
        return;
    }

    if this.phase == 2 {
        if current.generation <= this.wait_generation {
            if this.elapsed > 10.0 { fail(this, "runtime kind override did not reach its committed generation"); }
            return;
        }
        let overridden = composed_kind(assembly_edit::inspect(this.doc, PANEL));
        if overridden != TEMP_KIND {
            fail(this, "runtime kind override was not visible through composed USD");
            return;
        }
        this.kind_override_seen = true;
        let cleared = assembly_edit::prim_kind(
            this.doc, "@runtime@", PANEL, (), current.generation,
        );
        if !query_ok(cleared) {
            fail(this, "runtime kind override could not be cleared: " + cleared.error);
            return;
        }
        this.wait_generation = current.generation;
        this.phase = 3;
        return;
    }

    if this.phase == 3 {
        if current.generation <= this.wait_generation {
            if this.elapsed > 10.0 { fail(this, "cleared runtime kind did not reach its committed generation"); }
            return;
        }
        let restored = composed_kind(assembly_edit::inspect(this.doc, PANEL));
        if restored != "component" {
            fail(this, "clearing runtime kind did not reveal the authored component kind");
            return;
        }
        this.kind_restore_seen = true;
        this.phase = 4;
    }

    if this.phase == 1 {
        if current.generation <= this.wait_generation {
            if this.elapsed > 10.0 { fail(this, "property patch did not reach its committed generation"); }
            return;
        }
        let values = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: PANEL,
            attrs: ["xformOp:translate", "size", "physics:collisionEnabled", "inputs:surface_area"],
            children: false,
        });
        if !query_ok(values) {
            if this.elapsed > 10.0 { fail(this, "patched property values were not queryable"); }
            return;
        }
        let same = assembly_builder::editable_property_patch_plan(
            this.doc, "@root@", PANEL,
            [
                #{ name: "xformOp:translate", value: [2.0, 0.0, 0.0] },
                #{ name: "size", type_name: "double", value: 2.0 },
                #{ name: "physics:collisionEnabled", type_name: "bool", value: false },
                #{ name: "inputs:surface_area", type_name: "float", value: 3.5 },
                #{ name: "variant:configuration", value: "wide" },
            ], current.generation,
        );
        let stale = assembly_builder::editable_property_patch_plan(
            this.doc, "@root@", PANEL,
            [#{ name: "size", type_name: "double", value: 2.0 }], this.wait_generation,
        );
        let structural = assembly_builder::editable_property_patch_plan(
            this.doc, "@root@", PANEL,
            [#{ name: "xformOpOrder", value: ["xformOp:translate"] }], current.generation,
        );
        let wrong_type = assembly_builder::editable_property_patch_plan(
            this.doc, "@root@", PANEL,
            [#{ name: "size", type_name: "float", value: 2.0 }], current.generation,
        );
        this.done = true;
        report_verdict([
            t_true(this.kind_override_seen && this.kind_restore_seen,
                "SetPrimKind runtime override and clear restore composed USD metadata"),
            t_true(vector_close(values.attrs["xformOp:translate"], [2.0, 0.0, 0.0])
                && close_enough(values.attrs.size, 2.0)
                && values.attrs["physics:collisionEnabled"] == false
                && close_enough(values.attrs["inputs:surface_area"], 3.5),
                "typed property patch values are visible through composed USD queries"),
            t_true(query_ok(same) && same.no_op && same.ops.len() == 0,
                "repeating the inspected values produces a true no-op"),
            t_true(!stale.ok && !structural.ok && !wrong_type.ok,
                "stale generations, structural fields, and wrong types fail visibly"),
        ], "ASSEMBLY PROPERTY EDITOR", "ASSEMBLY_PROPERTY_EDITOR");
        return;
    }

    let requested = [
        "kind", "xformOp:translate", "size", "xformOpOrder",
        "physics:collisionEnabled", "inputs:surface_area", "outputs:observed_area",
        "variant:configuration",
    ];
    let catalog = assembly_builder::editable_property_catalog(
        this.doc, PANEL, "@root@", requested,
    );
    let custom = assembly_builder::editable_property_catalog(
        this.doc, PANEL, "@root@", ["lunco:component:units"],
    );
    let automatic = assembly_builder::editable_property_catalog(
        this.doc, PANEL, "@root@", (),
    );
    if !query_ok(catalog) || !query_ok(automatic) || custom.ok
        || property(automatic, "variant:configuration") == () {
        fail(this, "schema-driven property catalog did not validate requested fields");
        return;
    }
    let translate = property(catalog, "xformOp:translate");
    let size = property(catalog, "size");
    let kind = property(catalog, "kind");
    let order = property(catalog, "xformOpOrder");
    let variant = property(catalog, "variant:configuration");
    let input = property(catalog, "inputs:surface_area");
    if translate == () || size == () || kind == () || order == ()
        || variant == () || input == () {
        fail(this, "schema-driven catalog omitted a requested standard field");
        return;
    }
    let collision = property(catalog, "physics:collisionEnabled");
    let already_patched = vector_close(translate.value, [2.0, 0.0, 0.0])
        && close_enough(size.value, 2.0)
        && collision.value == false
        && close_enough(input.value, 3.5)
        && variant.value == "wide";
    if already_patched {
        // The scene runner can replace the file-backed script after its first
        // attach while the document remains live. Preserve the current
        // document as the authoritative state and continue with the final
        // no-op/rejection checks instead of replaying setup.
        this.wait_generation = current.generation - 1;
        this.phase = 1;
        return;
    }
    if !vector_close(translate.value, [11.0, 0.0, 0.0])
        || !close_enough(size.value, 13.0)
        || collision.value != true
        || !close_enough(input.value, 17.0)
        || variant.value != "base" {
        fail(this, "fixture defaults did not match the authored standard USD values");
        return;
    }
    let patch = assembly_builder::editable_property_patch_plan(
        this.doc, "@root@", PANEL,
        [
            // The kind was already verified through the dedicated runtime
            // override/clear cycle above, so this mixed patch keeps it as a
            // deliberate no-op while exercising the other standard fields.
            #{ name: "kind", value: "component" },
            #{ name: "xformOp:translate", value: [2.0, 0.0, 0.0] },
            #{ name: "size", type_name: "double", value: 2.0 },
            #{ name: "physics:collisionEnabled", type_name: "bool", value: false },
            #{ name: "inputs:surface_area", type_name: "float", value: 3.5 },
            #{ name: "variant:configuration", value: "wide" },
        ], current.generation,
    );
    if !query_ok(patch) {
        fail(this, "schema-driven patch returned an error");
        return;
    }
    if patch.no_op {
        fail(this, "schema-driven patch unexpectedly became a no-op");
        return;
    }
    if patch.ops.len() != 5 {
        fail(this, "schema-driven patch emitted an unexpected operation count");
        return;
    }
    if patch.changed != ["xformOp:translate", "size", "physics:collisionEnabled",
        "inputs:surface_area", "variant:configuration"] {
        fail(this, "schema-driven patch changed the wrong field names");
        return;
    }
    let proposal = assembly_edit::propose(
        this.doc, #{ Assembly: () }, "Apply schema-driven property patch",
        patch.ops, current.generation,
    );
    if !query_ok(proposal) {
        fail(this, "schema-driven property proposal was rejected: " + proposal.error);
        return;
    }
    let review = assembly_edit::review_session(this.doc);
    if !query_ok(review) || review.proposals.len() != 1
        || review.proposals[0].diagnostics.len() != 0 {
        fail(this, "schema-driven property proposal did not validate cleanly");
        return;
    }
    let committed = assembly_edit::commit_proposal(review.proposals[0].id);
    if !query_ok(committed) {
        fail(this, "schema-driven property proposal could not be committed: " + committed.error);
        return;
    }
    this.wait_generation = current.generation;
    this.phase = 1;
}
