// ── Connectivity routing (PURE rhai over the query("Links") snapshot) ────────
//
// The Rust kernel computes link GEOMETRY at the LinkConfig cadence (range,
// elevation, body + terrain + occluder blocking) and publishes the live graph +
// AOS/LOS events. Routing is POLICY, so it lives here — composed ON DEMAND (when a
// rover decides whether it can talk to Earth), never in a per-tick loop. Rust
// exposes a single data primitive, `query("Links")`; everything below is authored
// over it with no core rebuild. Tune or replace freely.
//
// ── Identity: GIDs, not names ────────────────────────────────────────────────
// A node IS its GID — the same id `find("/Scene/Rover/Comms")` returns and the API
// speaks. Names and classes are LABELS: a class is a shared role (every DSN dish
// authors `class = "earth"`), and a name is unique only within its parent. So the
// helpers below take either:
//
//   • a GID          → that one node, or every node CONTAINED by it
//   • a class string → the GROUP of nodes with that role ("any Earth station")
//
// which is why `can_reach(rover_gid, "earth")` still reads the way it always did,
// while each complex is now separately addressable.
//
// ── Containment: the prim you name is rarely the prim that links ─────────────
// A link node sits where the link geometry physically is — the dish's feed phase
// centre — so the beam, the RF state and the range verdict share one transform.
// That prim is several levels inside the mechanism:
// `Comms/YawHead/DishGimbal/DishHead/LinkAperture`. What a scene NAMES is the
// antenna: `find("/Scene/Rover/Comms")`. Both must denote the same node, or every
// scenario has to spell out an articulation it does not own — and a change to the
// dish's internals would silently break every script that addressed it.
//
// `query("Links").owners` maps each container GID to the nodes beneath it, so the
// resolution below is the hierarchy the scene authored, not a name match.

/// The live connectivity graph snapshot:
/// `#{ nodes: [#{id, name, class}], adj: #{ "<gid>": [gid] }, edges: [...],
///    groups: #{ class: [gid] }, owners: #{ "<gid>": [gid] } }`.
/// `()` when the provider is absent.
fn links() { query("Links", #{}) }

/// The GIDs a selector denotes: a class string → every node with that role; a GID →
/// that node, or — when the GID names something CONTAINING nodes rather than a node
/// itself — the nodes inside it. `[]` if it names nothing in the current graph.
fn link_ids(g, sel) {
    if g == () { return []; }
    // A `find()` that resolved nothing denotes nothing — never the whole graph,
    // and never a stringified "()" key that would quietly miss.
    if sel == () { return []; }
    if type_of(sel) == "string" {
        if sel in g.groups { return g.groups[sel]; }
        return [];
    }
    let k = "" + sel;
    // A node in its own right.
    if k in g.adj { return [sel]; }
    // Otherwise a container: the antenna, vessel or assembly a scene names.
    if k in g.owners { return g.owners[k]; }
    []
}

/// The currently-connected peers of `sel` — a node GID, a GID containing nodes, or
/// a class. Peers of every node the selector denotes, deduplicated: asking a rover
/// for its peers must answer the same list whether you name the vehicle, its
/// antenna, or the aperture. JSON object keys are strings, so the adjacency is
/// keyed by the stringified GID — converted here, once.
fn neighbours(g, sel) {
    if g == () { return []; }
    let out = [];
    for id in link_ids(g, sel) {
        let k = "" + id;
        if !(k in g.adj) { continue; }
        for p in g.adj[k] {
            if !(p in out) { out.push(p); }
        }
    }
    out
}

/// True when `a` and `b` have a DIRECT link up right now. Both ends take the same
/// three forms as `link_ids`: a node GID, a GID containing nodes, or a class.
///
/// Use this instead of `b in neighbours(g, a)`. A peer list holds NODE gids, so
/// testing a container gid — the antenna or vessel a scene actually names — against
/// it answers "no" for a link that is up, and an assertion written that way passes
/// for the wrong reason rather than failing loudly.
fn linked(g, a, b) {
    if g == () { return false; }
    let peers = neighbours(g, a);
    for id in link_ids(g, b) {
        if id in peers { return true; }
    }
    false
}

/// True if `to` is reachable from `from` over currently-up links — directly or
/// multi-hop through relays. Either end may be a GID or a class ("earth").
/// Breadth-first over the `links()` snapshot; call it at decision time (e.g. in
/// `on_event` on `link.los`), not every tick.
fn reachable(from, to) {
    let g = links();
    if g == () { return false; }
    let starts = link_ids(g, from);
    let goals = link_ids(g, to);
    if starts.is_empty() || goals.is_empty() { return false; }

    let seen = #{};
    let frontier = [];
    for s in starts {
        if s in goals { return true; }   // already there
        let k = "" + s;
        seen[k] = true;
        frontier.push(s);
    }
    while frontier.len() > 0 {
        let cur = frontier.shift();
        for p in neighbours(g, cur) {
            if p in goals { return true; }
            let k = "" + p;
            if k in seen { continue; }
            seen[k] = true;
            frontier.push(p);
        }
    }
    false
}

/// The shortest hop path `from → to` as GIDs (both ends inclusive), or `[]` when
/// unreachable. Same BFS as `reachable`, tracking predecessors — use it to name the
/// relay chain a message would take (`name(id)` renders each hop).
fn link_path(from, to) {
    let g = links();
    if g == () { return []; }
    let starts = link_ids(g, from);
    let goals = link_ids(g, to);
    if starts.is_empty() || goals.is_empty() { return []; }

    let prev = #{};
    let frontier = [];
    for s in starts {
        if s in goals { return [s]; }
        let k = "" + s;
        prev[k] = s;
        frontier.push(s);
    }
    while frontier.len() > 0 {
        let cur = frontier.shift();
        for p in neighbours(g, cur) {
            let k = "" + p;
            if k in prev { continue; }
            prev[k] = cur;
            if p in goals { return walk_back(prev, p); }
            frontier.push(p);
        }
    }
    []
}

/// Rebuild the path to `to` from a BFS predecessor map, ends inclusive. A start node
/// is its own predecessor, which is where the walk stops.
fn walk_back(prev, to) {
    let path = [to];
    let node = to;
    loop {
        let up = prev["" + node];
        if up == node { break; }
        node = up;
        path.push(node);
    }
    path.reverse();
    path
}

/// Convenience for the common autonomy check "can this rover talk home?" — pass the
/// rover's GID and the station's GID, or its authored class (e.g. `"earth"`).
fn can_reach(from, station) { reachable(from, station) }

/// The labels of a `link_path`, for a message a human reads
/// (`["Comms", "Relay", "DSS_Madrid"]`).
fn link_path_names(from, to) {
    let out = [];
    for id in link_path(from, to) { out.push(name(id)); }
    out
}
