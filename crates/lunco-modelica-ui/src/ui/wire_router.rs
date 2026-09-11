//! Obstacle-aware orthogonal wire router.
//!
//! Replaces the per-edge `route_orthogonal` Z/L heuristic for cases
//! where a straight L is wrong because an icon body sits between the
//! ports. Runs A* on a sparse Manhattan grid:
//!
//! * Cells are `GRID_STEP` world units across.
//! * Each cell has up to four neighbours (N/S/E/W).
//! * Obstacles are the icon `visual_rect`s passed in by the caller,
//!   inflated by `CLEARANCE` so wires don't graze the icon body.
//! * Cost = step distance + bend penalty when the direction changes.
//!   Bend penalty (~step × 4) is the knob that picks fewer-bend routes
//!   over slightly-longer-straight ones.
//! * Heuristic = Manhattan distance to goal — admissible on a grid
//!   with bend penalty so A* is optimal.
//!
//! The returned polyline is in world coords, includes endpoints,
//! and has collinear runs collapsed (so a horizontal-then-horizontal
//! sequence becomes a single segment).
//!
//! Cost: ~0.5 ms per edge for a typical Modelica model on a 4-unit
//! grid with 4 obstacles. Caller runs this once per projection, not
//! per frame.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

/// Direction of travel when entering a cell. `None` = the start cell
/// before any movement; bend penalty is suppressed against `None`.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
enum Dir {
    N,
    S,
    E,
    W,
    None,
}

impl Dir {
    fn delta(self) -> (i32, i32) {
        match self {
            Dir::N => (0, -1),
            Dir::S => (0, 1),
            Dir::E => (1, 0),
            Dir::W => (-1, 0),
            Dir::None => (0, 0),
        }
    }
}

/// World-space rectangle obstacle.
#[derive(Copy, Clone, Debug)]
pub struct Obstacle {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

/// Run obstacle-aware A* between two points and return a clean
/// orthogonal polyline including the endpoints.
///
/// `from_outward` / `to_outward` are unit vectors pointing *out of*
/// the source / target ports — used to seed the wire's first move so
/// it leaves the port from the expected side.
///
/// `obstacles` are inflated by `clearance` so wires don't graze
/// icon edges. Cells overlapping the obstacles are blocked except
/// the start and goal cells (which inevitably sit on or inside the
/// owning icon's rect).
///
/// Falls back to a single direct segment if the grid search fails to
/// find a path (e.g. start fully boxed in by obstacles).
pub fn route(
    from: (f32, f32),
    from_outward: (f32, f32),
    to: (f32, f32),
    to_outward: (f32, f32),
    obstacles: &[Obstacle],
    grid_step: f32,
    bend_penalty: f32,
    clearance: f32,
) -> Vec<(f32, f32)> {
    let grid = grid_step.max(1.0);

    // Bounding box of from + to + a generous margin so the A* has
    // room to wrap around obstacles between them.
    let margin = (grid * 20.0).max(60.0);
    let min_x = from.0.min(to.0) - margin;
    let min_y = from.1.min(to.1) - margin;
    let max_x = from.0.max(to.0) + margin;
    let max_y = from.1.max(to.1) + margin;

    let cells_x = ((max_x - min_x) / grid).ceil() as i32 + 1;
    let cells_y = ((max_y - min_y) / grid).ceil() as i32 + 1;

    let to_cell = |x: f32, y: f32| -> (i32, i32) {
        (
            ((x - min_x) / grid).round() as i32,
            ((y - min_y) / grid).round() as i32,
        )
    };
    let cell_world =
        |cx: i32, cy: i32| -> (f32, f32) { (min_x + cx as f32 * grid, min_y + cy as f32 * grid) };

    let start = to_cell(from.0, from.1);
    let goal = to_cell(to.0, to.1);

    if start == goal {
        return vec![from, to];
    }

    // Inflated obstacles. The blocker test uses cell *centres* against
    // these expanded rects.
    let inflated: Vec<Obstacle> = obstacles
        .iter()
        .map(|o| Obstacle {
            min_x: o.min_x - clearance,
            min_y: o.min_y - clearance,
            max_x: o.max_x + clearance,
            max_y: o.max_y + clearance,
        })
        .collect();
    let blocked = |cx: i32, cy: i32| -> bool {
        if (cx, cy) == start || (cx, cy) == goal {
            return false;
        }
        let (wx, wy) = cell_world(cx, cy);
        inflated
            .iter()
            .any(|o| wx >= o.min_x && wx <= o.max_x && wy >= o.min_y && wy <= o.max_y)
    };

    // Initial direction: snap from_outward to the dominant axis.
    let start_dir = outward_to_dir(from_outward);
    // Goal direction the wire SHOULD enter from = opposite of to_outward.
    // If to is to the LEFT of you, you must travel WEST to reach it; the
    // outward vector points west, so incoming = east. We don't enforce
    // this strictly (A* halts on cell hit), but we add a small penalty
    // for the wrong incoming direction in the goal so the wire
    // approaches from the natural side.
    let goal_in_dir = outward_to_dir((-to_outward.0, -to_outward.1));

    // A*.
    type State = (i32, i32, Dir);
    let h = |s: State| -> i64 { ((s.0 - goal.0).abs() + (s.1 - goal.1).abs()) as i64 };
    let mut g_score: HashMap<State, i64> = HashMap::new();
    let mut came_from: HashMap<State, State> = HashMap::new();
    let mut open: BinaryHeap<(Reverse<i64>, State)> = BinaryHeap::new();
    let start_state: State = (start.0, start.1, start_dir);
    g_score.insert(start_state, 0);
    open.push((Reverse(h(start_state)), start_state));

    let bend_cost = bend_penalty.max(0.0) as i64;
    let step_cost = 1_i64;
    let mut best_goal: Option<State> = None;

    while let Some((_, current)) = open.pop() {
        if (current.0, current.1) == goal {
            best_goal = Some(current);
            break;
        }
        let g_curr = *g_score.get(&current).unwrap();
        for ndir in [Dir::N, Dir::S, Dir::E, Dir::W] {
            let (dx, dy) = ndir.delta();
            let nx = current.0 + dx;
            let ny = current.1 + dy;
            if nx < 0 || ny < 0 || nx > cells_x || ny > cells_y {
                continue;
            }
            if blocked(nx, ny) {
                continue;
            }
            // Disallow immediate U-turn from start (wire shouldn't
            // exit the port the wrong way) — only enforced when the
            // start has a meaningful outward.
            if current == start_state && start_dir != Dir::None && opposite(ndir) == start_dir {
                continue;
            }
            let bend = if current.2 != Dir::None && current.2 != ndir {
                bend_cost
            } else {
                0
            };
            let approach_penalty =
                if (nx, ny) == goal && goal_in_dir != Dir::None && ndir != goal_in_dir {
                    bend_cost
                } else {
                    0
                };
            let tentative = g_curr + step_cost + bend + approach_penalty;
            let next: State = (nx, ny, ndir);
            if g_score.get(&next).map(|&g| tentative < g).unwrap_or(true) {
                g_score.insert(next, tentative);
                came_from.insert(next, current);
                let f = tentative + h(next);
                open.push((Reverse(f), next));
            }
        }
    }

    let Some(goal_state) = best_goal else {
        // Couldn't route; degenerate two-point wire.
        return vec![from, to];
    };

    // Reconstruct cells from goal back to start.
    let mut cells: Vec<(i32, i32)> = Vec::new();
    let mut cur = goal_state;
    cells.push((cur.0, cur.1));
    while cur != start_state {
        let Some(&prev) = came_from.get(&cur) else {
            break;
        };
        cur = prev;
        cells.push((cur.0, cur.1));
    }
    cells.reverse();

    // Build the polyline as bend-only points so every segment is
    // strictly horizontal or vertical. We walk cell transitions, find
    // every direction change, and project each bend onto the outgoing
    // segment's axis using the previous waypoint's coordinate. The
    // start point is the actual port position (not the snapped grid
    // cell), so the first segment leaves the port cleanly without the
    // half-grid diagonal that earlier "first cell as waypoint"
    // versions produced.
    let mut pts: Vec<(f32, f32)> = vec![from];
    let mut current_dir: Option<(i32, i32)> = None;
    for w in cells.windows(2) {
        let (ax, ay) = w[0];
        let (bx, by) = w[1];
        let step_dir = (bx - ax, by - ay);
        if step_dir == (0, 0) {
            continue;
        }
        if let Some(prev_dir) = current_dir {
            if prev_dir != step_dir {
                // Bend point: keep continuity with the previous
                // segment's axis.
                let (last_x, last_y) = *pts.last().unwrap();
                let (next_world_x, next_world_y) = cell_world(ax, ay);
                let bend = if prev_dir.0 != 0 {
                    // Previous segment was horizontal — bend's y
                    // matches the last point, x advances to the
                    // cell where the turn happens.
                    (next_world_x, last_y)
                } else {
                    // Previous segment was vertical.
                    (last_x, next_world_y)
                };
                pts.push(bend);
            }
        }
        current_dir = Some(step_dir);
    }
    // Final segment: a bend toward `to`. Use the last segment's axis
    // to derive the corner so the run into the goal port is clean.
    if let Some(prev_dir) = current_dir {
        let (last_x, last_y) = *pts.last().unwrap();
        let bend = if prev_dir.0 != 0 {
            (to.0, last_y)
        } else {
            (last_x, to.1)
        };
        if (bend.0 - last_x).abs() > 0.01 || (bend.1 - last_y).abs() > 0.01 {
            pts.push(bend);
        }
    }
    pts.push(to);

    // Collapse collinear runs (rare after the bend-only construction
    // but possible when consecutive bends collapse to a straight line).
    let eps = grid * 0.25;
    let mut compact: Vec<(f32, f32)> = Vec::with_capacity(pts.len());
    for p in pts {
        if compact.len() >= 2 {
            let a = compact[compact.len() - 2];
            let b = compact[compact.len() - 1];
            let collinear_x = (a.0 - b.0).abs() < eps && (b.0 - p.0).abs() < eps;
            let collinear_y = (a.1 - b.1).abs() < eps && (b.1 - p.1).abs() < eps;
            if collinear_x || collinear_y {
                let last = compact.len() - 1;
                compact[last] = p;
                continue;
            }
        }
        compact.push(p);
    }
    // Drop near-zero segments.
    let mut deduped: Vec<(f32, f32)> = Vec::with_capacity(compact.len());
    for p in compact {
        match deduped.last() {
            Some(&q) if (p.0 - q.0).abs() < eps && (p.1 - q.1).abs() < eps => continue,
            _ => deduped.push(p),
        }
    }

    // String-pulling: for each triple (a, b, c), try to drop the
    // middle bend `b` by replacing the two segments a→b→c with a
    // single L-shape a→corner→c whose corner is the unobstructed one
    // of (a.x, c.y) or (c.x, a.y). When such a corner exists, b was
    // an unnecessary detour the grid search introduced (typical when
    // bend penalty plus grid quantization let A* take a redundant zig
    // through an open area). Run repeatedly until no more reductions.
    let mut simplified = deduped;
    // Hard cap: each pass can only reduce the polyline (replacements
    // that don't reduce are filtered above), so 16 iterations is far
    // more than any real-world wire ever needs. The cap is purely a
    // safety net against floating-point chatter creating a livelock.
    for _ in 0..16 {
        let mut changed = false;
        let mut i = 0;
        while i + 2 < simplified.len() {
            let a = simplified[i];
            let c = simplified[i + 2];
            let candidates = [(a.0, c.1), (c.0, a.1)];
            let mut replacement: Option<(f32, f32)> = None;
            for cand in candidates {
                if segment_clear(a, cand, &inflated) && segment_clear(cand, c, &inflated) {
                    replacement = Some(cand);
                    break;
                }
            }
            if let Some(r) = replacement {
                let cur = simplified[i + 1];
                if (cur.0 - r.0).abs() > eps || (cur.1 - r.1).abs() > eps {
                    simplified[i + 1] = r;
                    changed = true;
                }
            }
            i += 1;
        }
        // Collapse collinear runs again after replacement.
        let mut next: Vec<(f32, f32)> = Vec::with_capacity(simplified.len());
        for p in simplified.into_iter() {
            if next.len() >= 2 {
                let a = next[next.len() - 2];
                let b = next[next.len() - 1];
                let collinear_x = (a.0 - b.0).abs() < eps && (b.0 - p.0).abs() < eps;
                let collinear_y = (a.1 - b.1).abs() < eps && (b.1 - p.1).abs() < eps;
                if collinear_x || collinear_y {
                    let last = next.len() - 1;
                    next[last] = p;
                    continue;
                }
            }
            match next.last() {
                Some(&q) if (p.0 - q.0).abs() < eps && (p.1 - q.1).abs() < eps => continue,
                _ => next.push(p),
            }
        }
        simplified = next;
        if !changed {
            break;
        }
    }
    simplified
}

/// Test whether the axis-aligned segment between `a` and `b` is clear
/// of every obstacle in `obs` (already inflated). Diagonal segments
/// are rejected — this router only emits orthogonals — so callers
/// should only invoke this for horizontal or vertical pairs.
fn segment_clear(a: (f32, f32), b: (f32, f32), obs: &[Obstacle]) -> bool {
    if (a.0 - b.0).abs() > 0.01 && (a.1 - b.1).abs() > 0.01 {
        return false;
    }
    let xmin = a.0.min(b.0);
    let xmax = a.0.max(b.0);
    let ymin = a.1.min(b.1);
    let ymax = a.1.max(b.1);
    // Treat segment as a thin rect; intersect against each obstacle.
    obs.iter()
        .all(|o| xmax < o.min_x || xmin > o.max_x || ymax < o.min_y || ymin > o.max_y)
}

fn outward_to_dir(v: (f32, f32)) -> Dir {
    let ax = v.0.abs();
    let ay = v.1.abs();
    if ax < 0.05 && ay < 0.05 {
        return Dir::None;
    }
    if ax >= ay {
        if v.0 >= 0.0 {
            Dir::E
        } else {
            Dir::W
        }
    } else if v.1 >= 0.0 {
        Dir::S
    } else {
        Dir::N
    }
}

fn opposite(d: Dir) -> Dir {
    match d {
        Dir::N => Dir::S,
        Dir::S => Dir::N,
        Dir::E => Dir::W,
        Dir::W => Dir::E,
        Dir::None => Dir::None,
    }
}
