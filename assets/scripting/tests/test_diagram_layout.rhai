// Production-installed policy: compact placement, repeatability and bad facts.
let checks = [];
let nodes = [];
for i in 0..30 { nodes.push(#{ path: "/System/N" + i, width: 250.0, height: 96.0, rank: 0 }); }
nodes.push(#{ path: "/System/Sink", width: 250.0, height: 420.0, rank: 1 });
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
let invalid = invoke_hook("diagram.layout", [#{ nodes: [#{ path: "/Broken" }], edges: [], scope: "/" }]);
checks.push(t_true(!invalid.ok, "bad layout facts fail visibly"));
report_verdict(checks, "DIAGRAM LAYOUT", "DIAGRAM_LAYOUT");

