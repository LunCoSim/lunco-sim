//! Deterministic, incrementally evolved budgeted restricted-quadtree cover.
//! Projection and terrain-specific demand/error callbacks belong to the caller.

use std::collections::HashSet;

use crate::quadtree::{QuadCoord, Quadtree, Square};

/// Depth at which the cover covers `q`'s region from ABOVE: `Some(d ≤ q.depth)`
/// if `q` itself or an ancestor is in the cover, `None` if the region is covered
/// by strictly deeper nodes. Exactness (every point under exactly one cover node)
/// is what makes these three cases exhaustive.
pub fn covering_depth(cover: &HashSet<QuadCoord>, mut q: QuadCoord) -> Option<u8> {
    loop {
        if cover.contains(&q) {
            return Some(q.depth);
        }
        q = q.parent()?;
    }
}

/// Internal split nodes needed to preserve an admitted required leaf partition.
/// Ancestors are shared across every constrained leaf; each is recorded once.
pub fn required_split_nodes(leaves: impl IntoIterator<Item = QuadCoord>) -> HashSet<QuadCoord> {
    let mut splits = HashSet::new();
    for leaf in leaves {
        let mut ancestor = leaf.parent();
        while let Some(parent) = ancestor {
            if !splits.insert(parent) {
                break;
            }
            ancestor = parent.parent();
        }
    }
    splits
}

/// Would replacing `p`'s four children by `p` break the 2:1 edge restriction?
/// True iff some region edge-adjacent to `p` is covered at depth ≥ `p.depth + 2`.
/// Tested on the depth-`p.depth+1` rim coords just outside `p`: by exactness,
/// "neither the coord nor any ancestor is in the cover" ⟺ covered strictly
/// deeper than `p.depth + 1`.
fn merge_violates_restriction(cover: &HashSet<QuadCoord>, p: QuadCoord) -> bool {
    let d1 = p.depth + 1;
    let side = 1i64 << d1;
    let (x0, z0) = (2 * p.x as i64, 2 * p.z as i64);
    let rim = [
        (x0 - 1, z0),
        (x0 - 1, z0 + 1), // left
        (x0 + 2, z0),
        (x0 + 2, z0 + 1), // right
        (x0, z0 - 1),
        (x0 + 1, z0 - 1), // below
        (x0, z0 + 2),
        (x0 + 1, z0 + 2), // above
    ];
    rim.into_iter().any(|(x, z)| {
        x >= 0
            && z >= 0
            && x < side
            && z < side
            && covering_depth(
                cover,
                QuadCoord {
                    depth: d1,
                    x: x as u32,
                    z: z as u32,
                },
            )
            .is_none()
    })
}

/// Reused candidate collections for [`evolve_restricted_cover`].
#[derive(Default)]
pub struct CoverScratch {
    parents: HashSet<QuadCoord>,
    merges: Vec<(f64, QuadCoord)>,
    splits: Vec<(f64, QuadCoord)>,
    chain: Vec<QuadCoord>,
}

/// Evolve the PERSISTENT cover one bounded step toward what the metric wants.
///
/// The configured pixel-error metric stays fixed. Under budget pressure,
/// exchange less valuable complete sibling groups for more valuable splits,
/// preserving required refinement and the restricted cover.
///
/// Split priority is `dist / refine_range`. Voluntary merges and budget
/// exchanges have hysteresis; a stationary camera reaches a fixed point.
///
/// The cover stays an exact, disjoint REPLACE cover throughout: a split swaps one
/// node for its four children, a merge swaps four siblings for their parent, and
/// nothing else touches it.
///
/// It also stays RESTRICTED (edge-adjacent nodes differ by ≤ 1 depth — the CDLOD
/// morph contract; a 2-level seam has no band wide enough to blend it). The cover
/// starts at a restricted mandatory cover or `{ROOT}`, so the invariant holds by
/// induction: every merge is guarded ([`merge_violates_restriction`]) and every
/// split first force-splits any coarser edge-neighbour (which, ON a restricted
/// cover, is at most one level up — so the cascade is depth-1 per neighbour,
/// recursing only through the chain it builds).
///
/// A retain band
/// blocks voluntary merges of already-refined tiles, but never requests new
/// splits; it is therefore hysteresis rather than a larger detail radius.
///
/// `required_splits` identifies internal nodes of the admitted mandatory tree;
/// no merge may consume them. The caller must provide a feasible mandatory
/// restricted cover within the budget. `locked_leaves` prohibits direct and
/// cascading splits of an admitted stable leaf; its ancestors must be required.
/// Returns `(edits applied, splits refused by resource or locked-cover constraints)`.
/// Zero edits means
/// the cover is at its fixed point FOR THESE INPUTS — the caller can skip the
/// pass until an input changes.
pub fn evolve_restricted_cover(
    qt: &Quadtree,
    cover: &mut HashSet<QuadCoord>,
    foci: &[([f64; 2], f64)],
    node_error: &impl Fn(QuadCoord, Square) -> f64,
    node_heights: &impl Fn(QuadCoord, Square) -> [f64; 2],
    budget: usize,
    max_cover_edits: usize,
    hysteresis_ratio: f64,
    retain_refinement: &impl Fn(QuadCoord, Square) -> bool,
    required_splits: &HashSet<QuadCoord>,
    locked_leaves: &HashSet<QuadCoord>,
    scratch: &mut CoverScratch,
) -> (usize, usize) {
    let CoverScratch {
        parents,
        merges,
        splits,
        chain,
    } = scratch;
    if cover.is_empty() {
        cover.insert(QuadCoord::ROOT);
    }
    let range = |c: QuadCoord| qt.error_refine_range(node_error(c, qt.region(c)));
    let dist = |c: QuadCoord| {
        foci.iter()
            .map(|(focus, camera_y)| {
                qt.focus_distance(
                    c,
                    [focus[0], *camera_y, focus[1]],
                    node_heights(c, qt.region(c)),
                )
            })
            .reduce(f64::min)
            .unwrap_or(f64::INFINITY)
    };
    // How far past its refine range a node sits: < 1 wants to be finer, > 1 coarser.
    let slack = |c: QuadCoord| dist(c) / range(c).max(1e-9);

    // Full quads whose parent could take over, ranked by how far past the band.
    parents.clear();
    for c in cover.iter() {
        if let Some(p) = c.parent() {
            parents.insert(p);
        }
    }
    merges.clear();
    merges.extend(
        parents
            .drain()
            .filter(|p| {
                !required_splits.contains(p) && p.children().iter().all(|k| cover.contains(k))
            })
            .map(|p| {
                let slack = if retain_refinement(p, qt.region(p)) {
                    0.0
                } else {
                    slack(p)
                };
                (slack, p)
            }),
    );
    // Total order for peer determinism: slack, then coord (a slack-only sort
    // leaves equal keys in HashSet iteration order, so a budget truncation
    // would edit different nodes on different peers/runs).
    let merge_order = |a: &(f64, QuadCoord), b: &(f64, QuadCoord)| {
        b.0.total_cmp(&a.0)
            .then_with(|| (a.1.depth, a.1.x, a.1.z).cmp(&(b.1.depth, b.1.x, b.1.z)))
    };
    // Sorting the FULL merge list was one of the two per-frame O(n log n)
    // passes run to apply the configured cover-edit budget. Pass 1 only ever consumes
    // the candidates past the hysteresis band, so partition those to the
    // front and order just that (usually empty) prefix; the full ordering is
    // materialised only on the rare over-budget frame (pass 2 below).
    let mut band = 0;
    for i in 0..merges.len() {
        if merges[i].0 >= hysteresis_ratio {
            merges.swap(band, i);
            band += 1;
        }
    }
    merges[..band].sort_by(merge_order);

    let mut edits = 0usize;
    let mut budget_refused = 0usize;
    let merge_one = |cover: &mut HashSet<QuadCoord>, p: QuadCoord| {
        // Re-check: an earlier merge in this pass may have consumed these children.
        if !p.children().iter().all(|k| cover.contains(k)) {
            return false;
        }
        for k in p.children() {
            cover.remove(&k);
        }
        cover.insert(p);
        true
    };

    // 1. Voluntary merges — the node is genuinely past the hysteresis band
    //    (the sorted prefix holds exactly those, farthest first).
    for &(_, p) in merges.iter().take(band) {
        if edits >= max_cover_edits {
            break;
        }
        // Restriction guard — also what makes cascade splits STABLE: a node
        // force-split for a neighbour's sake drifts past the hysteresis band, but
        // merging it back would re-violate, so the guard refuses and the cover
        // keeps its fixed point instead of flip-flopping.
        if merge_violates_restriction(cover, p) {
            continue;
        }
        if merge_one(cover, p) {
            edits += 1;
        }
    }

    // 2. Forced merges restore the hard budget after required refinement or a
    //    budget reduction. Reclaim the least valuable non-retained groups first.
    //    Only if none can satisfy the budget may we touch retained tiles.
    //    Not bounded by the configured cover-edit budget: being over budget is a frame-rate problem,
    //    and unlike a global metric change this only touches the quads it drops.
    // Forced merges count toward the RETURNED edit total (a pass that shrank
    // the cover is not a fixed point) but not toward the cover-edit setting — being
    // over budget is a frame-rate problem the pass must fully resolve.
    let mut forced = 0usize;
    while cover.len() > budget {
        parents.clear();
        parents.extend(cover.iter().filter_map(|coord| coord.parent()));
        merges.clear();
        merges.extend(
            parents
                .drain()
                .filter(|parent| {
                    !required_splits.contains(parent)
                        && parent.children().iter().all(|child| cover.contains(child))
                })
                .map(|parent| {
                    let retained = retain_refinement(parent, qt.region(parent));
                    (if retained { 0.0 } else { slack(parent) }, parent)
                }),
        );
        merges.sort_by(merge_order);
        let before = cover.len();
        for retained in [false, true] {
            for &(_, parent) in merges.iter() {
                if cover.len() <= budget {
                    break;
                }
                if retain_refinement(parent, qt.region(parent)) == retained
                    && !merge_violates_restriction(cover, parent)
                    && merge_one(cover, parent)
                {
                    forced += 1;
                }
            }
            if cover.len() <= budget {
                break;
            }
        }
        // A valid restricted tree always has a mergeable deepest sibling group.
        if cover.len() == before {
            break;
        }
    }

    // 3. Splits by projected error, subject to budget and stable-leaf constraints.
    splits.clear();
    splits.extend(
        cover
            .iter()
            .copied()
            .filter(|c| c.depth < qt.max_depth && !locked_leaves.contains(c))
            .map(|c| (slack(c), c))
            .filter(|&(s, _)| s < 1.0),
    );
    let split_order = |a: &(f64, QuadCoord), b: &(f64, QuadCoord)| {
        a.0.total_cmp(&b.0)
            .then_with(|| (a.1.depth, a.1.x, a.1.z).cmp(&(b.1.depth, b.1.x, b.1.z)))
    };
    // The wanting-to-split set can approach the whole cover, but the
    // edit budget bounds accepted splits. Partition the best candidates
    // to the front (total order ⇒ the partition boundary is exact) and sort
    // only that prefix; the tail is ordered on demand ONLY if refusals or
    // consumed cascades leave edit budget unspent — so the visit order, the
    // applied edits and the `budget_refused` count are all identical to the
    // full sort.
    let prefix = max_cover_edits.saturating_sub(edits).min(splits.len());
    if prefix > 0 && prefix < splits.len() {
        splits.select_nth_unstable_by(prefix - 1, split_order);
    }
    splits[..prefix].sort_by(split_order);
    let mut sorted_upto = prefix;
    let mut next = 0;
    let mut exchange_candidates_dirty = true;
    while next < splits.len() {
        if edits >= max_cover_edits {
            break;
        }
        if next == sorted_upto {
            // Earlier candidates were refused/consumed without spending the
            // edit budget — order the tail before visiting it.
            splits[sorted_upto..].sort_by(split_order);
            sorted_upto = splits.len();
        }
        let (split_slack, c) = splits[next];
        next += 1;
        if !cover.contains(&c) {
            continue; // consumed by an earlier chain's cascade
        }
        // Splitting `c` puts depth `c.depth + 1` against every edge-adjacent
        // region, so any neighbour covered COARSER than `c` must split first —
        // the classic restricted-quadtree cascade. On a restricted cover each
        // such neighbour is exactly one level up, and its own cascade needs (the
        // ancestors the loop pushes transitively), so the chain stays short and
        // the walk terminates at the root.
        chain.clear();
        chain.push(c);
        let mut i = 0;
        while i < chain.len() {
            let n = chain[i];
            for nb in n.edge_neighbours() {
                if let Some(nd) = covering_depth(cover, nb) {
                    if nd < n.depth {
                        let shift = nb.depth - nd;
                        let a = QuadCoord {
                            depth: nd,
                            x: nb.x >> shift,
                            z: nb.z >> shift,
                        };
                        if !chain.contains(&a) {
                            chain.push(a);
                        }
                    }
                }
            }
            i += 1;
        }
        // A distant refinement may not force a locked radial leaf to split
        // through the restriction cascade, even when the direct candidate fits.
        if chain.iter().any(|coord| locked_leaves.contains(coord)) {
            budget_refused += 1;
            continue;
        }
        // The whole chain lands or none of it does — a partial cascade would leave
        // the violation it exists to prevent. `continue`, not `break`: a later,
        // cheaper split may still fit. Permit one indivisible cascade in an
        // empty cycle so a small edit allowance cannot falsely settle the cover.
        if edits > 0 && edits + chain.len() > max_cover_edits {
            continue; // transient: retried next pass once the edit budget frees up
        }
        if cover.len() + 3 * chain.len() > budget {
            // A full cover is not a fixed point when a more valuable refinement
            // can replace a less valuable sibling group. Exchange only complete
            // restricted groups, with hysteresis, and commit all or none.
            let needed = (cover.len() + 3 * chain.len() - budget).div_ceil(3);
            if edits > 0 && edits + chain.len() + needed > max_cover_edits {
                continue;
            }
            // Reuse the ranked donors while the cover stays unchanged. A
            // budget-starved fixed point must not sort the entire cover once
            // per refused split.
            if exchange_candidates_dirty {
                parents.clear();
                parents.extend(cover.iter().filter_map(|coord| coord.parent()));
                merges.clear();
                merges.extend(parents.drain().filter_map(|parent| {
                    if required_splits.contains(&parent)
                        || retain_refinement(parent, qt.region(parent))
                        || !parent.children().iter().all(|child| cover.contains(child))
                    {
                        return None;
                    }
                    Some((slack(parent), parent))
                }));
                merges.sort_by(merge_order);
                exchange_candidates_dirty = false;
            }
            let threshold = split_slack * hysteresis_ratio;
            let eligible = merges.partition_point(|(value, _)| *value > threshold);
            if eligible < needed {
                budget_refused += 1;
                continue;
            }
            let mut trial = cover.clone();
            let mut reclaimed = 0;
            for &(_, parent) in &merges[..eligible] {
                if parent.children().iter().any(|child| chain.contains(child)) {
                    continue;
                }
                if merge_violates_restriction(&trial, parent) {
                    continue;
                }
                if !merge_one(&mut trial, parent) {
                    continue;
                }
                // A merge may create a new neighbour cascade. Do not spend the
                // budget on that exchange unless the original chain remains valid.
                let chain_valid = chain.iter().all(|node| {
                    trial.contains(node)
                        && node.edge_neighbours().into_iter().all(|neighbour| {
                            covering_depth(&trial, neighbour).is_none_or(|depth| {
                                depth >= node.depth
                                    || chain.iter().any(|ancestor| {
                                        ancestor.depth == depth
                                            && neighbour.x >> (neighbour.depth - depth)
                                                == ancestor.x
                                            && neighbour.z >> (neighbour.depth - depth)
                                                == ancestor.z
                                    })
                            })
                        })
                });
                if !chain_valid {
                    trial.remove(&parent);
                    trial.extend(parent.children());
                    continue;
                }
                reclaimed += 1;
                if reclaimed == needed {
                    break;
                }
            }
            if reclaimed != needed {
                budget_refused += 1;
                continue;
            }
            *cover = trial;
            edits += reclaimed;
        }
        // Coarsest first, so every forced split lands on an already-conforming rim.
        chain.sort_unstable_by_key(|q| q.depth);
        exchange_candidates_dirty = true;
        for &q in chain.iter() {
            if !cover.remove(&q) {
                continue;
            }
            for k in q.children() {
                cover.insert(k);
            }
            edits += 1;
        }
    }
    (edits + forced, budget_refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(depth: u8, x: u32, z: u32) -> QuadCoord {
        QuadCoord { depth, x, z }
    }

    fn test_qt() -> Quadtree {
        Quadtree::new(1000.0, 6, 1.0, 100.0)
    }

    #[cfg(test)]
    const TEST_MAX_COVER_EDITS: usize = 64;
    #[cfg(test)]
    const TEST_LOD_HYSTERESIS_RATIO: f64 = 1.30;

    #[cfg(test)]
    fn evolve_cover(
        qt: &Quadtree,
        cover: &mut HashSet<QuadCoord>,
        focus: [f64; 2],
        camera_y: f64,
        node_error: &impl Fn(QuadCoord, Square) -> f64,
        budget: usize,
    ) -> usize {
        evolve_restricted_cover(
            qt,
            cover,
            &[(focus, camera_y)],
            node_error,
            &|_, _| [0.0, 0.0],
            budget,
            TEST_MAX_COVER_EDITS,
            TEST_LOD_HYSTERESIS_RATIO,
            &|_, _| false,
            &HashSet::new(),
            &HashSet::new(),
            &mut CoverScratch::default(),
        )
        .1
    }

    /// The cover must remain an exact, disjoint REPLACE cover of the root after any
    /// number of evolution steps: every point covered by exactly one node.
    fn assert_exact_cover(cover: &HashSet<QuadCoord>) {
        assert!(!cover.is_empty(), "cover went empty");
        // No node may be an ancestor of another (disjointness).
        for a in cover.iter() {
            let mut p = a.parent();
            while let Some(n) = p {
                assert!(
                    !cover.contains(&n),
                    "{a:?} lies under {n:?} — cover overlaps"
                );
                p = n.parent();
            }
        }
        // Total area must equal the root's: sum of 4^-depth == 1.
        let area: f64 = cover.iter().map(|c| 0.25f64.powi(c.depth as i32)).sum();
        assert!(
            (area - 1.0).abs() < 1e-9,
            "cover area {area} != 1 — holes or overlaps"
        );
    }

    /// A moving focus preserves exact coverage under the fixed tile budget.
    #[test]
    fn evolving_cover_stays_exact_and_within_budget() {
        let qt = test_qt();
        let err = |_c: QuadCoord, _r: Square| 120.0f64;
        let budget = 40;
        let mut cover = HashSet::new();

        // Sweep the focus across the terrain, then back out to a distance.
        for step in 0..60 {
            let x = -900.0 + (step as f64) * 30.0;
            evolve_cover(&qt, &mut cover, [x, 0.0], 2.0, &err, budget);
            assert_exact_cover(&cover);
            assert!(
                cover.len() <= budget,
                "cover {} exceeded budget {budget}",
                cover.len()
            );
        }
        for step in 0..30 {
            let h = 100.0 + (step as f64) * 400.0;
            evolve_cover(&qt, &mut cover, [0.0, 0.0], h, &err, budget);
            assert_exact_cover(&cover);
            assert!(cover.len() <= budget);
        }
    }

    /// Do two cover nodes share a boundary SEGMENT (not merely a corner)? Normalised
    /// `[0,1]` extents are exact powers of two, so the touch/overlap tests are exact
    /// — no epsilon.
    fn edge_adjacent(a: QuadCoord, b: QuadCoord) -> bool {
        let ext = |c: QuadCoord| {
            let s = 0.5f64.powi(c.depth as i32);
            (
                c.x as f64 * s,
                c.x as f64 * s + s,
                c.z as f64 * s,
                c.z as f64 * s + s,
            )
        };
        let (ax0, ax1, az0, az1) = ext(a);
        let (bx0, bx1, bz0, bz1) = ext(b);
        let touch_x = ax1 == bx0 || bx1 == ax0;
        let touch_z = az1 == bz0 || bz1 == az0;
        let over_x = ax0 < bx1 && bx0 < ax1;
        let over_z = az0 < bz1 && bz0 < az1;
        (touch_x && over_z) || (touch_z && over_x)
    }

    /// Worst neighbour depth gap in a cover, with the offending pair.
    fn worst_depth_gap(cover: &HashSet<QuadCoord>) -> (u32, Option<(QuadCoord, QuadCoord)>) {
        let v: Vec<QuadCoord> = cover.iter().copied().collect();
        let mut worst = 0u32;
        let mut pair = None;
        for i in 0..v.len() {
            for j in (i + 1)..v.len() {
                if !edge_adjacent(v[i], v[j]) {
                    continue;
                }
                let gap = (v[i].depth as i32 - v[j].depth as i32).unsigned_abs();
                if gap > worst {
                    worst = gap;
                    pair = Some((v[i], v[j]));
                }
            }
        }
        (worst, pair)
    }

    /// CONTROL for [`evolving_cover_stays_restricted`]: the checker itself must
    /// accept a hand-built restricted cover and flag a hand-built 2-level seam,
    /// or a pass over `evolve_cover` output proves nothing.
    #[test]
    fn the_restriction_checker_detects_seams() {
        let d1 = |x, z| QuadCoord { depth: 1, x, z };
        // (1,0,0) replaced by its four children: every edge crosses ≤ 1 level.
        let mut restricted: HashSet<QuadCoord> =
            [d1(1, 0), d1(0, 1), d1(1, 1)].into_iter().collect();
        restricted.extend((0..2).flat_map(|x| (0..2).map(move |z| QuadCoord { depth: 2, x, z })));
        let (gap, pair) = worst_depth_gap(&restricted);
        assert!(
            gap <= 1,
            "checker rejects a restricted cover: gap {gap} at {pair:?}"
        );
        // (1,0,0) replaced by its SIXTEEN depth-3 descendants: they touch the
        // depth-1 siblings directly — a 2-level seam the checker must flag.
        let mut seamed: HashSet<QuadCoord> = [d1(1, 0), d1(0, 1), d1(1, 1)].into_iter().collect();
        seamed.extend((0..4).flat_map(|x| (0..4).map(move |z| QuadCoord { depth: 3, x, z })));
        let (gap, _) = worst_depth_gap(&seamed);
        assert_eq!(gap, 2, "checker missed a 2-level seam");
    }

    /// THE CDLOD RESTRICTION: edge-adjacent selected nodes must differ by at most one
    /// level, because a single morph band is all that blends them — a 2-level jump has
    /// no band wide enough and the seam becomes a hard edge (visible as a staircase
    /// where sub-DEM detail switches on).
    ///
    /// `evolve_cover` enforces this by construction — guarded merges + cascade
    /// splits, from a trivially-restricted `{ROOT}` start (see its docs). This test
    /// is the proof it stays that way under a camera sweep; it FAILED (gap 2 at
    /// step 4) before the guards existed.
    #[test]
    fn evolving_cover_stays_restricted() {
        let qt = test_qt();
        // Depth-decaying error — see the control test for why a constant one is
        // degenerate here.
        let err = |c: QuadCoord, _r: Square| 120.0f64 * 0.5f64.powi(c.depth as i32);
        let budget = 40;
        let mut cover = HashSet::new();

        for step in 0..60 {
            let x = -900.0 + (step as f64) * 30.0;
            evolve_cover(&qt, &mut cover, [x, 0.0], 2.0, &err, budget);
            let (gap, pair) = worst_depth_gap(&cover);
            assert!(
                gap <= 1,
                "step {step}: neighbour depth gap {gap} > 1 at {pair:?}"
            );
        }
        for step in 0..30 {
            let h = 100.0 + (step as f64) * 400.0;
            evolve_cover(&qt, &mut cover, [0.0, 0.0], h, &err, budget);
            let (gap, pair) = worst_depth_gap(&cover);
            assert!(
                gap <= 1,
                "height step {step}: neighbour depth gap {gap} > 1 at {pair:?}"
            );
        }
    }

    /// A stationary camera reaches a fixed point and stops editing.
    #[test]
    fn a_still_camera_reaches_a_fixed_point() {
        let qt = test_qt();
        let err = |_c: QuadCoord, _r: Square| 120.0f64;
        let mut cover = HashSet::new();
        for _ in 0..200 {
            evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, 64);
        }
        let settled = cover.clone();
        for _ in 0..50 {
            evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, 64);
            assert_eq!(
                cover, settled,
                "cover still churning on a stationary camera — no fixed point"
            );
        }
    }

    /// The refusal counter is starvation, not fill progress: any budget that
    /// covers the metric's want reports zero at the fixed point, any budget
    /// below it reports the refused splits.
    #[test]
    fn budget_starvation_is_reported() {
        let qt = test_qt();
        let err = |_c: QuadCoord, _r: Square| 120.0f64;
        let settle = |budget: usize| {
            let mut cover = HashSet::new();
            for _ in 0..200 {
                evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, budget);
            }
            (
                evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, budget),
                cover,
            )
        };
        let (refused, unconstrained) = settle(usize::MAX);
        assert_eq!(
            refused, 0,
            "fixed point within budget must not report refusals"
        );
        let want = unconstrained.len();
        let (refused, _) = settle(want);
        assert_eq!(refused, 0, "budget == want must not report refusals");
        for budget in [want / 2, want / 8, 16] {
            let (refused, cover) = settle(budget);
            assert!(
                refused > 0,
                "budget {budget} < want {want} (cover {}) reached a fixed point \
                 without reporting refusals",
                cover.len()
            );
        }
    }

    /// Lowering the budget must be absorbed by merging, not by blowing past it.
    #[test]
    fn shrinking_the_budget_merges_down() {
        let qt = test_qt();
        let err = |_c: QuadCoord, _r: Square| 120.0f64;
        let mut cover = HashSet::new();
        for _ in 0..200 {
            evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, 256);
        }
        assert!(cover.len() > 16, "expected a refined cover to shrink from");
        evolve_cover(&qt, &mut cover, [0.0, 0.0], 5.0, &err, 16);
        assert_exact_cover(&cover);
        assert!(
            cover.len() <= 16,
            "cover {} did not shrink to budget 16",
            cover.len()
        );
    }

    /// The cover evolution must be a pure function of its inputs — reusing the
    /// scratch collections across calls (the per-frame hoist) must not leak
    /// state between passes or terrains.
    #[test]
    fn cover_scratch_reuse_matches_fresh_scratch() {
        let qt = test_qt();
        let err = |c: QuadCoord, _r: Square| 120.0f64 * 0.5f64.powi(c.depth as i32);
        let mut reused = CoverScratch::default();
        let mut a = HashSet::new();
        let mut b = HashSet::new();
        for step in 0..40 {
            let x = -900.0 + (step as f64) * 45.0;
            let ra = evolve_restricted_cover(
                &qt,
                &mut a,
                &[([x, 0.0], 2.0)],
                &err,
                &|_, _| [0.0, 0.0],
                64,
                TEST_MAX_COVER_EDITS,
                TEST_LOD_HYSTERESIS_RATIO,
                &|_, _| false,
                &HashSet::new(),
                &HashSet::new(),
                &mut reused,
            );
            let rb = evolve_restricted_cover(
                &qt,
                &mut b,
                &[([x, 0.0], 2.0)],
                &err,
                &|_, _| [0.0, 0.0],
                64,
                TEST_MAX_COVER_EDITS,
                TEST_LOD_HYSTERESIS_RATIO,
                &|_, _| false,
                &HashSet::new(),
                &HashSet::new(),
                &mut CoverScratch::default(),
            );
            assert_eq!(a, b, "step {step}: scratch reuse changed the cover");
            assert_eq!(
                ra, rb,
                "step {step}: scratch reuse changed the returned counts"
            );
        }
    }

    /// Zero edits means the caller can retain the cover and still report
    /// budget-starved refinement until demand changes.
    #[test]
    fn zero_edits_marks_the_fixed_point_even_when_budget_refused() {
        let qt = test_qt();
        let err = |_c: QuadCoord, _r: Square| 120.0f64;
        let mut scratch = CoverScratch::default();
        let mut cover = HashSet::new();
        let mut step = |cover: &mut HashSet<QuadCoord>| {
            evolve_restricted_cover(
                &qt,
                cover,
                &[([0.0, 0.0], 5.0)],
                &err,
                &|_, _| [0.0, 0.0],
                16,
                TEST_MAX_COVER_EDITS,
                TEST_LOD_HYSTERESIS_RATIO,
                &|_, _| false,
                &HashSet::new(),
                &HashSet::new(),
                &mut scratch,
            )
        };
        let mut last = (usize::MAX, 0);
        for _ in 0..200 {
            last = step(&mut cover);
        }
        assert_eq!(last.0, 0, "a settled cover must report zero edits");
        assert!(
            last.1 > 0,
            "budget 16 starves this metric — refusals must still be reported"
        );
        let settled = cover.clone();
        step(&mut cover);
        assert_eq!(
            cover, settled,
            "zero edits must mean the cover did not change"
        );
    }

    #[test]
    fn saturated_cover_exchanges_less_valuable_refinement_at_fixed_budget() {
        let qt = Quadtree::new(8.0, 3, 1.0, 8.0);
        let near = c(1, 0, 0);
        let far = c(1, 1, 1);
        let mut cover: HashSet<_> = QuadCoord::ROOT.children().into_iter().collect();
        cover.remove(&far);
        cover.extend(far.children());
        let budget = cover.len();
        let error = |coord: QuadCoord, _: Square| if coord.depth == 1 { 32.0 } else { 0.0 };
        let mut scratch = CoverScratch::default();
        let mut mandatory = cover.clone();
        let (edits, refused) = evolve_restricted_cover(
            &qt,
            &mut mandatory,
            &[([-4.0, -4.0], 1.0)],
            &error,
            &|_, _| [0.0, 0.0],
            budget,
            64,
            1.3,
            &|_, _| false,
            &required_split_nodes(far.children()),
            &HashSet::new(),
            &mut scratch,
        );
        assert_eq!(edits, 0);
        assert!(refused > 0);
        assert_eq!(
            mandatory, cover,
            "a mandatory split cannot fund another branch"
        );
        let (edits, _) = evolve_restricted_cover(
            &qt,
            &mut cover,
            &[([-4.0, -4.0], 1.0)],
            &error,
            &|_, _| [0.0, 0.0],
            budget,
            64,
            1.3,
            &|_, _| false,
            &HashSet::new(),
            &HashSet::new(),
            &mut scratch,
        );
        assert!(edits >= 2);
        assert!(near.children().iter().all(|child| cover.contains(child)));
        assert!(cover.contains(&far));
        assert!(cover.len() <= budget);
        assert_exact_cover(&cover);
        assert!(worst_depth_gap(&cover).0 <= 1);
        let settled = cover.clone();
        let (edits, _) = evolve_restricted_cover(
            &qt,
            &mut cover,
            &[([-4.0, -4.0], 1.0)],
            &error,
            &|_, _| [0.0, 0.0],
            budget,
            64,
            1.3,
            &|_, _| false,
            &HashSet::new(),
            &HashSet::new(),
            &mut scratch,
        );
        assert_eq!(edits, 0);
        assert_eq!(cover, settled);
    }

    #[test]
    fn locked_leaf_blocks_a_distant_restriction_cascade() {
        let qt = Quadtree::new(8.0, 3, 1.0, 8.0);
        let locked = c(1, 0, 0);
        let branch = c(1, 1, 0);
        let target = c(2, 2, 0);
        let mut cover: HashSet<_> = QuadCoord::ROOT.children().into_iter().collect();
        cover.remove(&branch);
        cover.extend(branch.children());
        let original = cover.clone();
        let required = required_split_nodes(cover.iter().copied());
        let err = |node, _: Square| if node == target { 100.0 } else { 0.0 };
        let (edits, refused) = evolve_restricted_cover(
            &qt,
            &mut cover,
            &[(qt.region(target).center, 1.0)],
            &err,
            &|_, _| [0.0, 0.0],
            64,
            64,
            1.3,
            &|_, _| false,
            &required,
            &HashSet::from([locked]),
            &mut CoverScratch::default(),
        );
        assert_eq!(edits, 0);
        assert!(refused > 0);
        assert_eq!(cover, original);
        assert_exact_cover(&cover);
        assert!(worst_depth_gap(&cover).0 <= 1);
    }

    #[test]
    fn budget_pressure_sheds_far_cover_before_a_retained_quad() {
        let qt = test_qt();
        let retained = c(1, 0, 0);
        let farthest = c(1, 1, 1);
        let mut cover = (0..4)
            .flat_map(|z| (0..4).map(move |x| c(2, x, z)))
            .collect::<HashSet<_>>();
        // Keep all quads inside the refine band so this exercises the forced,
        // not voluntary, merge path.
        let err = |_c: QuadCoord, _r: Square| 1_000_000.0f64;

        evolve_restricted_cover(
            &qt,
            &mut cover,
            &[([-700.0, -700.0], 5.0)],
            &err,
            &|_, _| [0.0, 0.0],
            13,
            TEST_MAX_COVER_EDITS,
            TEST_LOD_HYSTERESIS_RATIO,
            &|coord, _| coord == retained,
            &HashSet::new(),
            &HashSet::new(),
            &mut CoverScratch::default(),
        );

        assert!(
            retained.children().iter().all(|child| {
                covering_depth(&cover, *child).is_none_or(|depth| depth >= child.depth)
            }),
            "retained near-camera detail may refine further but must not coarsen"
        );
        assert!(
            cover.len() <= 13,
            "non-retained quads should absorb the budget cut"
        );
        assert!(cover.contains(&farthest));
        assert!(
            farthest
                .children()
                .iter()
                .all(|child| !cover.contains(child)),
            "the farthest eligible quad must merge before nearer non-retained cover"
        );
    }
}
