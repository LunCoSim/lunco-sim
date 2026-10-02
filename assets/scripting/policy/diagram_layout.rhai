// Pure presentation policy over immutable USD graph facts; no scene writes.
fn pack_layer(nodes, start_x) {
    let total = 0.0; let tallest = 0.0;
    for node in nodes {
        total += node.height + 48.0;
        if node.height > tallest { tallest = node.height; }
    }
    let target = sqrt(total * 370.0 / 1.6);
    if target < tallest + 48.0 { target = tallest + 48.0; }
    let x = start_x; let y = 40.0; let width = 0.0; let positions = [];
    for node in nodes {
        if y > 40.0 && y + node.height > 40.0 + target {
            x += width + 100.0; y = 40.0; width = 0.0;
        }
        positions.push(#{ path: node.path, x: x, y: y });
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
fn layout_diagram(facts) {
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
    let result = []; let x = 40.0; let bottom = 40.0;
    for rank in ranks {
        let layer = pack_layer(groups[rank.to_string()], x);
        for i in 0..layer.positions.len() {
            let placement = layer.positions[i];
            let edge = placement.y + groups[rank.to_string()][i].height;
            if edge > bottom { bottom = edge; }
        }
        result += layer.positions; x = layer.next_x;
    }
    if detached.len() > 0 {
        let shelf = pack_layer(detached, 40.0);
        for placement in shelf.positions { placement.y += bottom + 80.0; result.push(placement); }
    }
    result
}
