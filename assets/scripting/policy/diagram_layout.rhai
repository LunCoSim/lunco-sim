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
// Balance each causal layer into compact columns. Disconnected models remain
// visible, and real port contracts determine row heights.
fn layout_diagram(facts) {
    let groups = #{}; let ranks = [];
    for node in facts.nodes {
        let key = node.rank.to_string();
        if !groups.contains(key) { groups[key] = []; ranks.push(node.rank); }
        groups[key].push(node);
    }
    ranks.sort();
    let result = []; let x = 40.0;
    for rank in ranks {
        let layer = pack_layer(groups[rank.to_string()], x);
        result += layer.positions; x = layer.next_x;
    }
    result
}
