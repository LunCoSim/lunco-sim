// ── Selection toolkit (write targeting/selection algorithms in rhai) ─────────
//
// list_entities() returns #{ id, name, type, pos, catalog_id } per entity, so selection is
// idiomatic functional rhai over the built-in filter/map/reduce/sort_by. These
// are the reusable PRIMITIVES — compose them or write your own; all are policy,
// hot-reloadable, no Rust rebuild. Deeper per-entity reads use get(id, "Comp.f").
// Pass closures as predicates/metrics, e.g.
//   let weakest = min_by(all_of_type("rover"), |e| get(e.id, "Battery.charge"));

/// Vector × scalar.
fn vscale(a, s) { [a[0]*s, a[1]*s, a[2]*s] }

/// True if entity `id` currently has component `name` (e.g. "RoverVessel",
/// "LinearVelocity"). Built on get(), which returns () for an absent component.
fn has_component(id, name) { get(id, name) != () }

/// Kind tag for one entity ("rover"/"planet"/"unknown"), matching list_entities().type.
fn kind(id) {
    if has_component(id, "RoverVessel") { "rover" }
    else if has_component(id, "CelestialBody") { "planet" }
    else { "unknown" }
}

/// All entities of a kind, as list_entities() records (#{id,name,type,pos}).
fn all_of_type(t) { list_entities().filter(|e| e.type == t) }

/// Resolve the first entity authored from a catalog entry, or -1 when it is
/// not currently materialized. Catalog identity is stable across runtime
/// spawn paths; display names are not an identity contract.
fn find_catalog(catalog_id) {
    for entity in list_entities() {
        if entity.catalog_id == catalog_id { return entity.id; }
    }
    -1
}

/// The list element with the smallest `metric.call(e)`, or () if empty.
fn min_by(list, metric) {
    let best = ();
    let best_v = ();
    for e in list {
        let v = metric.call(e);
        if best == () || v < best_v { best = e; best_v = v; }
    }
    best
}

/// The list element with the largest `metric.call(e)`, or () if empty.
fn max_by(list, metric) {
    let best = ();
    let best_v = ();
    for e in list {
        let v = metric.call(e);
        if best == () || v > best_v { best = e; best_v = v; }
    }
    best
}

/// How many list elements satisfy `pred.call(e)`.
fn count_where(list, pred) { list.filter(|e| pred.call(e)).len() }

/// The entity record nearest to world point `point` matching `pred`, or () if
/// none. Operates on list_entities() records (uses e.pos).
fn nearest_where(point, pred) {
    let cands = list_entities().filter(|e| e.pos != () && pred.call(e));
    min_by(cands, |e| vlen(vsub(e.pos, point)))
}

/// The entity record farthest from `point` matching `pred`, or ().
fn farthest_where(point, pred) {
    let cands = list_entities().filter(|e| e.pos != () && pred.call(e));
    max_by(cands, |e| vlen(vsub(e.pos, point)))
}
