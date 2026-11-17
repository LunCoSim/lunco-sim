//! Shared finite canvas geometry for selection and clipped rendering.
use crate::{Pos, Rect};

/// Clip a segment to an axis-aligned rectangle, including boundary touches.
/// Invalid geometry is rejected. Intermediate f64 arithmetic prevents overflow
/// when finite authored f32 coordinates span the full canvas coordinate range.
pub fn clip_segment(a: Pos, b: Pos, r: Rect) -> Option<(Pos, Pos)> {
    if ![a.x, a.y, b.x, b.y, r.min.x, r.min.y, r.max.x, r.max.y]
        .iter()
        .all(|v| v.is_finite())
        || r.min.x > r.max.x
        || r.min.y > r.max.y
    {
        return None;
    }
    let (mut ax, mut ay) = (f64::from(a.x), f64::from(a.y));
    let (mut bx, mut by) = (f64::from(b.x), f64::from(b.y));
    let (left, top, right, bottom) = (
        f64::from(r.min.x),
        f64::from(r.min.y),
        f64::from(r.max.x),
        f64::from(r.max.y),
    );
    let code = |x: f64, y: f64| -> u8 {
        u8::from(x < left)
            | (u8::from(x > right) << 1)
            | (u8::from(y < top) << 2)
            | (u8::from(y > bottom) << 3)
    };
    // Cohen-Sutherland fixes a boundary coordinate exactly and recomputes the
    // remaining line from the clipped endpoint, avoiding tiny t-interval loss
    // when an enormous segment crosses a small viewport.
    for _ in 0..8 {
        let (ca, cb) = (code(ax, ay), code(bx, by));
        if ca | cb == 0 {
            // Explicit rendering/selection boundary: bounded f64 -> f32.
            return Some((
                Pos::new(ax as f32, ay as f32),
                Pos::new(bx as f32, by as f32),
            ));
        }
        if ca & cb != 0 {
            return None;
        }
        let out = if ca != 0 { ca } else { cb };
        let (x, y) = if out & 8 != 0 {
            (ax + (bx - ax) * ((bottom - ay) / (by - ay)), bottom)
        } else if out & 4 != 0 {
            (ax + (bx - ax) * ((top - ay) / (by - ay)), top)
        } else if out & 2 != 0 {
            (right, ay + (by - ay) * ((right - ax) / (bx - ax)))
        } else {
            (left, ay + (by - ay) * ((left - ax) / (bx - ax)))
        };
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        if out == ca {
            ax = x;
            ay = y;
        } else {
            bx = x;
            by = y;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipped_segments_are_bounded_and_invalid_geometry_is_rejected() {
        let r = Rect::from_min_max(Pos::new(0.0, 0.0), Pos::new(100.0, 100.0));
        let (a, b) = clip_segment(Pos::new(-1000.0, 50.0), Pos::new(1000.0, 50.0), r).unwrap();
        assert_eq!(a, Pos::new(0.0, 50.0));
        assert_eq!(b, Pos::new(100.0, 50.0));
        assert!(clip_segment(Pos::new(-2.0, -1.0), Pos::new(200.0, -1.0), r).is_none());
        assert!(clip_segment(Pos::new(f32::NAN, 0.0), b, r).is_none());
        let (a, b) = clip_segment(Pos::new(-f32::MAX, 50.0), Pos::new(f32::MAX, 50.0), r).unwrap();
        assert_eq!(a, Pos::new(0.0, 50.0));
        assert_eq!(b, Pos::new(100.0, 50.0));
        let (a, b) = clip_segment(
            Pos::new(-f32::MAX, -f32::MAX),
            Pos::new(f32::MAX, f32::MAX),
            r,
        )
        .unwrap();
        assert_eq!(a, r.min);
        assert_eq!(b, r.max);
    }
}
