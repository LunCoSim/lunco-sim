// Production command-clock gate for native double-precision trim evaluation.
fn native_trim_path() { "/World/NativeTrimPrecision" }
fn attr(name, kind, value) {
    #{SetAttribute: #{edit_target: "@runtime@", path: native_trim_path(),
        name: name, type_name: kind, value: value}}
}
fn native_trim_prepare(doc) {
    let ops = [#{AddPrim: #{edit_target: "@runtime@", parent_path: "/World",
        name: "NativeTrimPrecision", type_name: "NurbsPatch", reference: (), reference_prim_path: ()}},
        attr("uVertexCount", "int", "3"), attr("vVertexCount", "int", "3"),
        attr("uOrder", "int", "2"), attr("vOrder", "int", "2"),
        attr("primvars:nativeEmpty", "double[]", "[]"),
        attr("points", "point3f[]", "[(0,0,0),(5,0,0),(10,0,0),(0,0,5),(5,0,5),(10,0,5),(0,0,10),(5,0,10),(10,0,10)]"),
        attr("uKnots", "double[]", "[100000000,100000000,100000000.5,100000001,100000001]"),
        attr("vKnots", "double[]", "[100000000,100000000,100000000.5,100000001,100000001]"),
        attr("uRange", "double2", "(100000000,100000001)"),
        attr("vRange", "double2", "(100000000,100000001)"),
        attr("trimCurve:counts", "int[]", "[1]"), attr("trimCurve:orders", "int[]", "[2]"),
        attr("trimCurve:vertexCounts", "int[]", "[5]"),
        attr("trimCurve:knots", "double[]", "[0,0,1,2,3,4,4]"),
        attr("trimCurve:ranges", "double2[]", "[(0,4)]"),
        attr("trimCurve:points", "double3[]", "[(100000000.01,100000000.01,1),(100000000.99,100000000.01,1),(100000000.99,100000000.99,1),(100000000.01,100000000.99,1),(100000000.01,100000000.01,1)]")];
    let ticket = cmd("ApplyUsdOps", #{doc_id: doc, parent_gen: (), label: "Native trim precision", ops: ops});
    if ticket == () || ticket.id == () { throw("trim fixture was not admitted"); }
    if ticket.ok != true || ticket.status != "applied" { throw("trim edit failed: " + ticket.to_string()); }
    print("TRIM_APPLIED");
}
fn plan(doc) {
    query("PlanNurbsCollisionProxy", #{doc_id: doc, path: native_trim_path(),
        proxy_name: "Proxy", approximation: "none", max_refinement_delta_m: 0.1})
}
fn native_trim_valid(doc) {
    let cooked = plan(doc);
    if cooked != () && cooked.ok == false && cooked.error.to_string().contains("projection is not current") {
        print("TRIM_PENDING"); return;
    }
    if cooked == () || cooked.ok == false {
        report_verdict([t_true(false, "native trim cook failed: " + cooked.to_string())],
            "NATIVE TRIM PRECISION", "NATIVE_TRIM_PRECISION"); return;
    }
    let points = cooked.points; let indices = cooked.face_vertex_indices;
    let outside = indices.len() > 0 && indices.len() % 3 == 0;
    for face in 0..indices.len()/3 {
        let a = points[indices[face*3]]; let b = points[indices[face*3+1]];
        let c = points[indices[face*3+2]];
        let x = (a[0]+b[0]+c[0])/3.0; let z = (a[2]+b[2]+c[2])/3.0;
        if x > 0.10001 && x < 9.89999 && z > 0.10001 && z < 9.89999 { outside = false; }
    }
    let source = query("QueryUsdPrim", #{doc_id: doc, path: native_trim_path(),
        attrs: ["trimCurve:points", "uVertexCount", "primvars:nativeEmpty"]});
    report_verdict([
        t_true(points.len() > 0 && cooked.face_vertex_counts.len() > 0, "trimmed surface is cooked"),
        t_true(outside, "no triangle survives inside the precise trim hole"),
        t_true(source.attrs["trimCurve:points"][0][0] == 100000000.01,
            "authored trim coordinates retain double precision"),
        t_true(source.attrs.uVertexCount == 3, "native integer attributes remain integers"),
        t_true(source.attrs["primvars:nativeEmpty"] == [], "authored empty arrays remain arrays")
    ], "NATIVE TRIM PRECISION", "NATIVE_TRIM_PRECISION");
}
fn native_trim_invalidate(doc) {
    let ticket = cmd("ApplyUsdOps", #{doc_id: doc, parent_gen: (), label: "Reject zero trim weight",
        ops: [attr("trimCurve:points", "double3[]", "[(100000000.01,100000000.01,0),(100000000.99,100000000.01,1),(100000000.99,100000000.99,1),(100000000.01,100000000.99,1),(100000000.01,100000000.01,0)]")]});
    if ticket.ok != true || ticket.status != "applied" { throw("trim edit failed: " + ticket.to_string()); }
    print("TRIM_APPLIED");
}
fn native_trim_invalid(doc) {
    let rejected = plan(doc);
    if rejected != () && rejected.ok == false && rejected.error.to_string().contains("projection is not current") {
        print("TRIM_PENDING"); return;
    }
    report_verdict([
        t_true(rejected != () && rejected.ok == false && rejected.error != (),
            "malformed trim data produces a terminal cook diagnostic"),
        t_true(query("ListOpenDocuments", #{}).open_documents.len() > 0,
            "the application remains queryable after invalid trim data")
    ], "NATIVE TRIM PRECISION NEGATIVE", "NATIVE_TRIM_PRECISION_NEGATIVE");
}
fn native_trim_incomplete(doc) {
    let ticket = cmd("ApplyUsdOps", #{doc_id: doc, parent_gen: (), label: "Reject truncated second trim loop", ops: [
        attr("trimCurve:counts", "int[]", "[1,1]"),
        attr("trimCurve:orders", "int[]", "[2,2]"),
        attr("trimCurve:vertexCounts", "int[]", "[5,5]"),
        attr("trimCurve:knots", "double[]", "[0,0,1,2,3,4,4,0,0,1,2,3,4,4]"),
        attr("trimCurve:ranges", "double2[]", "[(0,4),(0,4)]"),
        attr("trimCurve:points", "double3[]", "[(100000000.01,100000000.01,1),(100000000.99,100000000.01,1),(100000000.99,100000000.99,1),(100000000.01,100000000.99,1),(100000000.01,100000000.01,1)]")
    ]});
    if ticket.ok != true || ticket.status != "applied" { throw("trim edit failed: " + ticket.to_string()); }
    print("TRIM_APPLIED");
}
