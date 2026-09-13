// TEST-ONLY. Verifies the authored standard PointInstancer contract used by
// the production projection fixture. The Rust reader owns the composed USD
// interpretation; this scenario owns the observable scene verdict.
const WAIT_S = 3.0;
const ROOT = "/PointInstancerTest/Blades";

fn on_start(me) {
    this.elapsed = 0.0;
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

    let record = query("QueryUsdPrim", #{
        path: ROOT,
        attrs: ["positions", "protoIndices", "orientations", "scales", "ids", "invisibleIds"],
        rels: ["prototypes"],
    });
    if record == () || record.ok == false {
        if this.elapsed < WAIT_S { return; }
        report_verdict(["PointInstancer was not readable"], "POINT INSTANCER", "POINT_INSTANCER");
        this.done = true;
        return;
    }

    let attrs = record.attrs;
    let relationships = record.relationships;
    let failures = [];
    failures.push(t_true(record.type_name == "PointInstancer", "USD type is not PointInstancer"));
    failures.push(t_true(relationships.prototypes.len() == 1
        && relationships.prototypes[0] == "/PointInstancerTest/BladePrototype",
        "prototype relationship was not preserved"));
    failures.push(t_true(attrs.positions.len() == 2
        && attrs.protoIndices == [0, 0]
        && attrs.ids == [101, 202],
        "required or identity arrays were altered"));
    failures.push(t_true(attrs.invisibleIds == [202], "invisibleIds mask was altered"));

    report_verdict(failures, "POINT INSTANCER", "POINT_INSTANCER");
    this.done = true;
}
