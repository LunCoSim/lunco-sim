//! Pure tile-grid math — world coordinates ↔ integer tile coordinates.
//!
//! The terrain plane is partitioned into square tiles of `tile_size_m`. Tiles
//! are addressed by integer [`TileCoord`] `(x, z)`; tile `(0,0)` covers world
//! `[0, tile_size) × [0, tile_size)` on the XZ plane. Tile size must be ≤ the
//! big_space cell edge so a tile never straddles a cell boundary (see crate
//! docs). No Bevy, no allocation on the hot path — this is the deterministic
//! foundation the streaming plugin and the cache keys build on.

use serde::{Deserialize, Serialize};

/// Integer address of a tile on the XZ plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TileCoord {
    pub x: i32,
    pub z: i32,
}

impl TileCoord {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// Chebyshev (chessboard) distance in tiles — the ring metric used for
    /// load/unload radius.
    pub fn chebyshev(self, other: TileCoord) -> i32 {
        (self.x - other.x).abs().max((self.z - other.z).abs())
    }
}

/// A square tiling of the XZ plane with edge `tile_size_m` (world metres).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TileGrid {
    pub tile_size_m: f64,
}

impl TileGrid {
    /// `tile_size_m` must be > 0 and ≤ the big_space cell edge.
    pub fn new(tile_size_m: f64) -> Self {
        debug_assert!(tile_size_m > 0.0, "tile_size_m must be positive");
        Self { tile_size_m }
    }

    /// The tile containing world position `(x, z)`. Uses floor division so it is
    /// correct for negative coordinates.
    pub fn tile_of(&self, x: f64, z: f64) -> TileCoord {
        TileCoord::new(
            (x / self.tile_size_m).floor() as i32,
            (z / self.tile_size_m).floor() as i32,
        )
    }

    /// World position of a tile's minimum (−X, −Z) corner.
    pub fn tile_origin(&self, c: TileCoord) -> (f64, f64) {
        (c.x as f64 * self.tile_size_m, c.z as f64 * self.tile_size_m)
    }

    /// World position of a tile's centre.
    pub fn tile_center(&self, c: TileCoord) -> (f64, f64) {
        let (ox, oz) = self.tile_origin(c);
        let h = self.tile_size_m * 0.5;
        (ox + h, oz + h)
    }

    /// All tiles within Chebyshev `radius` of `center` (a `(2r+1)²` block) — the
    /// set to keep resident around the viewer. `radius = 0` yields just `center`.
    pub fn ring(&self, center: TileCoord, radius: i32) -> Vec<TileCoord> {
        let r = radius.max(0);
        let mut out = Vec::with_capacity(((2 * r + 1) * (2 * r + 1)) as usize);
        for dz in -r..=r {
            for dx in -r..=r {
                out.push(TileCoord::new(center.x + dx, center.z + dz));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_of_floors_including_negatives() {
        let g = TileGrid::new(128.0);
        assert_eq!(g.tile_of(0.0, 0.0), TileCoord::new(0, 0));
        assert_eq!(g.tile_of(127.9, 0.1), TileCoord::new(0, 0));
        assert_eq!(g.tile_of(128.0, 0.0), TileCoord::new(1, 0));
        // negative side must floor toward −inf, not truncate toward 0
        assert_eq!(g.tile_of(-0.1, -200.0), TileCoord::new(-1, -2));
    }

    #[test]
    fn origin_center_consistent() {
        let g = TileGrid::new(256.0);
        let c = TileCoord::new(-3, 4);
        let (ox, oz) = g.tile_origin(c);
        assert_eq!((ox, oz), (-768.0, 1024.0));
        let (cx, cz) = g.tile_center(c);
        assert_eq!((cx, cz), (-640.0, 1152.0));
        // a point inside the tile maps back to it
        assert_eq!(g.tile_of(cx, cz), c);
    }

    #[test]
    fn ring_size_and_membership() {
        let g = TileGrid::new(100.0);
        let c = TileCoord::new(10, -10);
        assert_eq!(g.ring(c, 0), vec![c]);
        let r2 = g.ring(c, 2);
        assert_eq!(r2.len(), 25); // (2*2+1)^2
        assert!(r2.contains(&c));
        // every tile in the ring is within Chebyshev radius 2
        assert!(r2.iter().all(|t| c.chebyshev(*t) <= 2));
        // and a tile just outside is absent
        assert!(!r2.contains(&TileCoord::new(13, -10)));
    }
}
