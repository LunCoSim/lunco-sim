// Production-installed policy: compact placement, repeatability and bad facts.
let checks = [];
let nodes = [];
for i in 0..30 { nodes.push(#{ path: "/System/N" + i, role: "prim", width: 250.0, height: 96.0, rank: 0 }); }
nodes.push(#{ path: "/System/Sink", role: "prim", width: 250.0, height: 420.0, rank: 1 });
let facts = #{ nodes: nodes, edges: [], scope: "/System" };
let first = invoke_hook("diagram.layout", [facts]);
checks.push(t_true(first.ok, "production layout policy installed"));
if first.ok {
    let placements = first.value;
    checks.push(t_true(placements.len() == nodes.len(), "every USD node receives a placement"));
    let columns = #{};
    let right = 0.0;
    let bottom = 0.0;
    for i in 0..30 {
        let p = placements[i]; columns[p.x.to_string()] = true;
        if p.x > right { right = p.x; }
        if p.y > bottom { bottom = p.y; }
    }
    checks.push(t_true(columns.len() > 1 && bottom < 2000.0, "parallel nodes form a compact block"));
    checks.push(t_true(placements[30].x > right, "downstream layer remains to the right"));
    let second = invoke_hook("diagram.layout", [facts]);
    checks.push(t_true(second.ok && second.value == placements, "same graph has deterministic positions"));
}
let boundaries = invoke_hook("diagram.layout", [#{ nodes: [
    #{ path: "/System#outputs", role: "outputs", width: 250.0, height: 96.0, rank: 0 },
    #{ path: "/System/Model", role: "prim", width: 250.0, height: 96.0, rank: 0 },
    #{ path: "/System#inputs", role: "inputs", width: 250.0, height: 96.0, rank: 0 }
], edges: [], scope: "/System" }]);
checks.push(t_true(boundaries.ok, "system interface roles are admitted"));
if boundaries.ok {
    let p = boundaries.value;
    checks.push(t_true(p[0].path == "/System#inputs" && p[0].x < p[1].x && p[1].x < p[2].x && p[2].path == "/System#outputs", "authored policy places input and output boundaries around models"));
}
let mixed = [
    #{ path: "/A", role: "prim", width: 250.0, height: 96.0, rank: 0 },
    #{ path: "/B", role: "prim", width: 250.0, height: 96.0, rank: 1 },
    #{ path: "/Decoration", role: "prim", width: 250.0, height: 96.0, rank: 0 }
];
let shelf = invoke_hook("diagram.layout", [#{ nodes: mixed, edges: [#{ source: "/A", target: "/B", kind: "Dataflow" }], scope: "/" }]);
checks.push(t_true(shelf.ok && shelf.value.len() == 3 && shelf.value[2].path == "/Decoration" && shelf.value[2].y > shelf.value[0].y + 96.0 && shelf.value[2].y > shelf.value[1].y + 96.0, "disconnected prims remain visible below the connected graph"));
let branches = invoke_hook("diagram.layout", [#{ nodes: [
    #{ path: "/A1", role: "prim", width: 250.0, height: 96.0, rank: 0 },
    #{ path: "/A2", role: "prim", width: 250.0, height: 96.0, rank: 0 },
    #{ path: "/B1", role: "prim", width: 250.0, height: 96.0, rank: 1 },
    #{ path: "/B2", role: "prim", width: 250.0, height: 96.0, rank: 1 }
], edges: [#{ source: "/A1", target: "/B2", kind: "Dataflow" }, #{ source: "/A2", target: "/B1", kind: "Dataflow" }], scope: "/" }]);
let aligned = #{};
if branches.ok { for placement in branches.value { aligned[placement.path] = placement.y; } }
checks.push(t_true(branches.ok && aligned["/A1"] == aligned["/B2"] && aligned["/A2"] == aligned["/B1"], "neighbor ordering aligns parallel branches instead of crossing them"));
let invalid = invoke_hook("diagram.layout", [#{ nodes: [#{ path: "/Broken" }], edges: [], scope: "/" }]);
checks.push(t_true(!invalid.ok, "bad layout facts fail visibly"));
report_verdict(checks, "DIAGRAM LAYOUT", "DIAGRAM_LAYOUT");

