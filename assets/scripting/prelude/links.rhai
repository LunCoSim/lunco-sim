// ── Connectivity routing (PURE rhai over the query("Links") snapshot) ────────
//
// The Rust kernel computes link GEOMETRY at the LinkConfig cadence (range,
// elevation, body + terrain occlusion) and publishes the live graph + AOS/LOS
// events. Routing is POLICY, so it lives here — composed ON DEMAND (when a rover
// decides whether it can talk to Earth), never in a per-tick loop. Rust exposes a
// single data primitive, `query("Links")`; everything below is authored over it
// with no core rebuild. Tune or replace freely.

/// The live connectivity graph snapshot: `#{ nodes, adj, edges }`. `adj[key]` is
/// the list of currently-connected peer keys; a node's key is its authored `class`
/// (else its Name). () when the provider is absent.
fn links() { query("Links", #{}) }

/// True if `to` is reachable from `from` over currently-up links — directly or
/// multi-hop through relays. Breadth-first over the `links()` snapshot; call it at
/// decision time (e.g. in `on_event` on `link.los`), not every tick.
fn reachable(from, to) {
    if from == to { return true; }
    let g = links();
    if g == () { return false; }
    let adj = g.adj;
    let seen = #{};
    seen[from] = true;
    let frontier = [from];
    while frontier.len() > 0 {
        let cur = frontier.shift();
        if !(cur in adj) { continue; }
        for p in adj[cur] {
            if p == to { return true; }
            if !(p in seen) { seen[p] = true; frontier.push(p); }
        }
    }
    false
}

/// The shortest hop path `from → to` as node keys (both ends inclusive), or `[]`
/// when unreachable. Same BFS as `reachable`, tracking predecessors — use it to
/// name the relay chain a message would take.
fn link_path(from, to) {
    if from == to { return [from]; }
    let g = links();
    if g == () { return []; }
    let adj = g.adj;
    let prev = #{};
    prev[from] = from;
    let frontier = [from];
    while frontier.len() > 0 {
        let cur = frontier.shift();
        if !(cur in adj) { continue; }
        for p in adj[cur] {
            if p in prev { continue; }
            prev[p] = cur;
            if p == to {
                let path = [to];
                let node = to;
                while node != from { node = prev[node]; path.push(node); }
                path.reverse();
                return path;
            }
            frontier.push(p);
        }
    }
    []
}

/// Convenience for the common autonomy check "can this rover talk home?" — pass
/// the ground-station key your scene authored (e.g. `"earth"` or `"DSS_Goldstone"`).
fn can_reach(from, station) { reachable(from, station) }
