//! Gap A — big_space cell+offset coordinates and per-client floating-origin
//! rebasing. Pure math, dependency-free (`[i64;3]` cells, `[f64;3]` offsets).
//!
//! Proves: a pose `(cell, offset)` denotes one absolute world position; each
//! client can rebase it into its *own* origin and still agree on the world; and
//! the within-cell offset is *bounded*, which is what makes quantization cheap.

/// Meters per big_space cell (illustrative).
pub const CELL_SIZE: f64 = 10_000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridPos {
    pub cell: [i64; 3],
    pub offset: [f64; 3],
}

impl GridPos {
    pub fn new(cell: [i64; 3], offset: [f64; 3]) -> Self {
        Self { cell, offset }
    }

    /// Absolute world position as f64 (fine for tests / small worlds; the real
    /// system never materializes this — it stays cell-relative).
    pub fn world(&self) -> [f64; 3] {
        let mut w = [0.0; 3];
        for i in 0..3 {
            w[i] = self.cell[i] as f64 * CELL_SIZE + self.offset[i];
        }
        w
    }

    /// Build from an absolute world position (normalized: offset ∈ [0, CELL_SIZE)).
    pub fn from_world(world: [f64; 3]) -> Self {
        let mut cell = [0i64; 3];
        let mut offset = [0.0f64; 3];
        for i in 0..3 {
            let c = (world[i] / CELL_SIZE).floor();
            cell[i] = c as i64;
            offset[i] = world[i] - c * CELL_SIZE;
        }
        Self { cell, offset }
    }

    /// Rebase into a client whose floating origin sits at `origin_cell`.
    /// Returns render-space coords *near that origin* (small, metric).
    pub fn rebase_to(&self, origin_cell: [i64; 3]) -> [f64; 3] {
        let mut r = [0.0; 3];
        for i in 0..3 {
            r[i] = (self.cell[i] - origin_cell[i]) as f64 * CELL_SIZE + self.offset[i];
        }
        r
    }

    /// Normalize so every offset component is bounded to [0, CELL_SIZE),
    /// carrying overflow into the cell. Bounded offset ⇒ cheap quantization.
    pub fn normalized(&self) -> GridPos {
        let mut cell = self.cell;
        let mut offset = self.offset;
        for i in 0..3 {
            let carry = (offset[i] / CELL_SIZE).floor();
            cell[i] += carry as i64;
            offset[i] -= carry * CELL_SIZE;
        }
        GridPos { cell, offset }
    }

    /// Is every offset component within [0, CELL_SIZE)?
    pub fn offset_is_bounded(&self) -> bool {
        self.offset
            .iter()
            .all(|&o| o >= 0.0 && o < CELL_SIZE)
    }
}
