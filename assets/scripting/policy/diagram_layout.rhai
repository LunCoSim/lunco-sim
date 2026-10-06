fn quotient_edge(source,target,kind) {#{source:source,target:target,kind:kind}}
// Pure presentation policy over immutable USD graph facts; no scene writes.
fn program_accent(programs) {
    if programs == () || programs.len() == 0 { return ""; }
    let backends = #{};
    for backend in programs { backends[backend] = true; }
    if backends.contains("Invalid program") { return "warning"; }
    if backends.len() > 1 { return "package"; }
    if backends.contains("Modelica") { return "model"; }
    if backends.contains("Rhai") { return "block"; }
    if backends.contains("Python") { return "record"; }
    "class"
}
fn pack_layer(nodes, start_x, target) {
    let x = start_x; let y = 40.0; let width = 0.0; let positions = [];
    for node in nodes {
        if y > 40.0 && y + node.height > 40.0 + target {
            x += width + 100.0; y = 40.0; width = 0.0;
        }
        let placement = #{ path: node.path, x: x, y: y };
        if node.role == "inputs" { placement.label = "System inputs"; }
        if node.role == "outputs" { placement.label = "System outputs"; }
        if node.role == "connectors" { placement.label = "System connectors"; }
        let accent = program_accent(node.programs);
        if accent != "" { placement.accent = accent; }
        positions.push(placement);
        y += node.height + 48.0;
        if node.width > width { width = node.width; }
    }
    #{ positions: positions, next_x: x + width + 100.0 }
}
fn compare_order(a, b) {
    if a.order < b.order { return -1; }
    if a.order > b.order { return 1; }
    if a.path < b.path { return -1; }
    if a.path > b.path { return 1; }
    0
}
fn neighbor_order(path, rank, neighbors, rows, layer_of, forward) {
    let total = 0.0; let count = 0.0;
    for other in neighbors[path] {
        let other_rank = layer_of[other];
        let eligible = forward && other_rank < rank;
        if !forward { eligible = other_rank > rank; }
        if eligible { total += rows[other]; count += 1.0; }
    }
    if count > 0.0 { return total / count; }
    rows[path]
}
fn order_layer(layer, rank, neighbors, rows, layer_of, forward) {
    for index in 0..layer.len() {
        let node = layer[index];
        node.order = neighbor_order(node.path, rank, neighbors, rows, layer_of, forward);
        layer[index] = node;
    }
    layer.sort_by(Fn("compare_order"));
    layer
}
fn layer_rows(groups, ranks) {
    let rows = #{};
    for rank in ranks {
        let i = 0.0;
        for node in groups[rank.to_string()] { rows[node.path] = i; i += 1.0; }
    }
    rows
}
fn align_sweep(groups, ranks, neighbors, layer_of, forward) {
    let rows = layer_rows(groups, ranks);
    for rank in ranks {
        let key = rank.to_string();
        let layer = order_layer(groups[key], rank, neighbors, rows, layer_of, forward);
        let i = 0.0; for node in layer { rows[node.path] = i; i += 1.0; }
        groups[key] = layer;
    }
    groups
}
// Four indexed neighbor-order sweeps align branches without re-reading USD.
fn align_layers(groups, ranks, edges) {
    let neighbors = #{}; let layer_of = #{};
    for rank in ranks {
        for node in groups[rank.to_string()] { layer_of[node.path] = rank; neighbors[node.path] = []; }
    }
    for edge in edges {
        let known = neighbors.contains(edge.source) && neighbors.contains(edge.target);
        if known && edge.source != edge.target {
            neighbors[edge.source].push(edge.target); neighbors[edge.target].push(edge.source);
        }
    }
    for pass in 0..4 {
        let order = ranks; if pass % 2 == 1 { order.reverse(); }
        groups = align_sweep(groups, order, neighbors, layer_of, pass % 2 == 0);
    }
    groups
}
// Balance each causal layer into compact columns. Disconnected models remain
// visible, and real port contracts determine row heights.
fn layout_flat_diagram(facts) {
    let groups = #{}; let ranks = []; let detached = []; let linked = #{};
    for edge in facts.edges { linked[edge.source] = true; linked[edge.target] = true; }
    for node in facts.nodes {
        if facts.edges.len() > 0 && node.role == "prim" && !linked.contains(node.path) {
            detached.push(node); continue;
        }
        let rank = node.rank;
        if node.role == "inputs" { rank = -1; }
        if node.role == "outputs" { rank = 1000000; }
        let key = rank.to_string();
        if !groups.contains(key) { groups[key] = []; ranks.push(rank); }
        groups[key].push(node);
    }
    ranks.sort();
    groups = align_layers(groups, ranks, facts.edges);
    // Use one graph-wide column height. Independent per-rank packing widens
    // short layers and collapses the entire system into an unreadable strip.
    let area = 0.0; let tallest = 0.0;
    for node in facts.nodes { area += (node.width + 100.0) * (node.height + 48.0); if node.height > tallest { tallest = node.height; } }
    let target = sqrt(area / 1.6);
    if target < tallest + 48.0 { target = tallest + 48.0; }
    let result = []; let x = 40.0; let bottom = 40.0;
    for rank in ranks {
        let layer = pack_layer(groups[rank.to_string()], x, target);
        for i in 0..layer.positions.len() {
            let placement = layer.positions[i];
            let edge = placement.y + groups[rank.to_string()][i].height;
            if edge > bottom { bottom = edge; }
        }
        result += layer.positions; x = layer.next_x;
    }
    if detached.len() > 0 {
        let shelf_area = 0.0; let shelf_tallest = 0.0;
        for node in detached { shelf_area += (node.width + 100.0) * (node.height + 48.0); if node.height > shelf_tallest { shelf_tallest = node.height; } }
        let shelf_height = sqrt(shelf_area / 1.6); if shelf_height < shelf_tallest + 48.0 { shelf_height = shelf_tallest + 48.0; }
        let shelf = pack_layer(detached, 40.0, shelf_height);
        for placement in shelf.positions { placement.y += bottom + 80.0; result.push(placement); }
    }
    result
}

fn group_bounds(placements,nodes) {
    let sizes=#{};for node in nodes {sizes[node.path]=node;}
    let right=0.0;let bottom=0.0;
    for p in placements {let node=sizes[p.path];let x=p.x+node.width;let y=p.y+node.height;if x>right {right=x;}if y>bottom {bottom=y;}}
    #{right:right,bottom:bottom}
}
fn member_of(path,id,membership) {membership.contains(path) && membership[path]==id}
fn group_graph(group,facts,membership) {
    let nodes=[];let edges=[];let external=#{};let rank=1000000;
    for node in facts.nodes {
        if !member_of(node.path,group.id,membership) {continue;}
        nodes.push(node);if node.rank<rank {rank=node.rank;}
    }
    for edge in facts.edges {
        let from=member_of(edge.source,group.id,membership);let to=member_of(edge.target,group.id,membership);
        if from && to {edges.push(edge);}
        if from && !to {let key=edge.source+"."+edge.source_port;external[key]=true;}
        if to && !from {let key=edge.target+"."+edge.target_port;external[key]=true;}
    }
    #{nodes:nodes,edges:edges,external:external,rank:rank}
}
// Pack group interiors first, then their quotient graph. Collapsed summaries
// reserve only their external endpoint rows; every original member is placed.
fn layout_diagram(facts) {
    if !facts.contains("groups") || facts.groups.len() == 0 { return layout_flat_diagram(facts); }
    let membership = #{}; let local = #{}; let quotient = []; let result = [];
    for group in facts.groups { for path in group.members { membership[path] = group.id; } }
    for group in facts.groups {
        let graph=group_graph(group,facts,membership);let nodes=graph.nodes;let edges=graph.edges;let external=graph.external;let rank=graph.rank;
        let placements = layout_flat_diagram(#{nodes:nodes,edges:edges,scope:facts.scope});
        let bounds=group_bounds(placements,nodes);
        local[group.id] = placements;
        let width = bounds.right + 24.0; let height = bounds.bottom + 24.0;
        if group.collapsed { width = 250.0; height = 70.0 + external.len() * 19.0; if height < 96.0 { height = 96.0; } }
        quotient.push(#{path:group.id,role:"prim",programs:[],rank:rank,width:width,height:height});
    }
    for node in facts.nodes { if !membership.contains(node.path) { quotient.push(node); } }
    let edges = [];
    for edge in facts.edges {
        let source = edge.source; let target = edge.target;
        if membership.contains(source) { source = membership[source]; } if membership.contains(target) { target = membership[target]; }
        if source == target {continue;}edges.push(quotient_edge(source,target,edge.kind));
    }
    let outer = layout_flat_diagram(#{nodes:quotient,edges:edges,scope:facts.scope});
    for placement in outer {
        if !local.contains(placement.path) {result.push(placement);continue;}
        for member in local[placement.path] {member.x+=placement.x;member.y+=placement.y+24.0;result.push(member);}
    }
    result
}
