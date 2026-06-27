//! Size-bucket quantization.
//!
//! Thousands of rocks would mean thousands of distinct meshes and colliders.
//! Instead we quantize sizes into a handful of geometric buckets: one shared
//! `Mesh` + one shared `Collider` per bucket, and every instance differs only by
//! a `Transform` scale (size / bucket-representative). Cheap to build, cheap to
//! batch.

/// Representative sizes for `n` geometric buckets spanning `[min, max]`.
///
/// Geometric (log) spacing matches the log-normal size distribution: more
/// resolution where rocks are common (small), less in the rare large tail.
pub fn bucket_sizes(min: f32, max: f32, n: usize) -> Vec<f32> {
    let n = n.max(1);
    if n == 1 || max <= min {
        return vec![(min + max) * 0.5];
    }
    let lmin = min.max(1e-4).ln();
    let lmax = max.max(min + 1e-4).ln();
    (0..n)
        .map(|i| {
            // Bucket representative = geometric midpoint of the i-th sub-interval.
            let t = (i as f32 + 0.5) / n as f32;
            (lmin + (lmax - lmin) * t).exp()
        })
        .collect()
}

/// Index of the bucket whose representative size is closest to `size`.
pub fn bucket_index(size: f32, buckets: &[f32]) -> usize {
    let mut best = 0;
    let mut best_d = f32::INFINITY;
    for (i, &b) in buckets.iter().enumerate() {
        let d = (b.ln() - size.max(1e-4).ln()).abs();
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_monotonic_and_in_range() {
        let b = bucket_sizes(0.2, 4.0, 5);
        assert_eq!(b.len(), 5);
        for w in b.windows(2) {
            assert!(w[1] > w[0], "buckets must increase");
        }
        assert!(*b.first().unwrap() >= 0.2 && *b.last().unwrap() <= 4.0);
    }

    #[test]
    fn index_picks_nearest() {
        let b = bucket_sizes(0.2, 4.0, 5);
        // Smallest size → first bucket; largest → last.
        assert_eq!(bucket_index(0.2, &b), 0);
        assert_eq!(bucket_index(4.0, &b), b.len() - 1);
    }

    #[test]
    fn single_bucket() {
        let b = bucket_sizes(1.0, 1.0, 1);
        assert_eq!(b.len(), 1);
        assert_eq!(bucket_index(99.0, &b), 0);
    }
}
