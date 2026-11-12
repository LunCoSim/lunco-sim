//! Certified camera-sphere sweeps over the immutable composed height source.
//!
//! A raster enclosure only rejects an empty interval. Actual terrain columns
//! supply collision witnesses; uncertain rectangles subdivide under one budget.
use crate::oracle::SurfaceOracle;
use bevy::math::DVec3;
use bevy::prelude::Entity;
use lunco_spatial::coords::GridPos;
use lunco_terrain_core::HeightPlane;
use lunco_terrain_core::HeightSource;

/// One witnessed camera-volume contact within the request's geometric
/// separation tolerance, expressed in the active physics frame.
#[derive(Clone, Copy, Debug)]
pub struct TerrainSphereHit {
    pub terrain: Entity,
    pub frame: Entity,
    pub source_key: u64,
    /// Conservative admitted fraction of the requested segment, in [0, 1].
    pub time: f64,
    pub point: GridPos,
    pub normal: DVec3,
}

/// A certified sphere sweep. Uncertainty never becomes a collision witness.
#[derive(Clone, Debug)]
pub enum TerrainSphereSweep {
    Clear,
    Hit(TerrainSphereHit),
    InitialOverlap {
        hit: TerrainSphereHit,
        penetration_m: f64,
        /// Only an explicitly requested, terrain-up translation may escape.
        escape_allowed: bool,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Clone, Copy, Debug)]
struct Candidate {
    cell: [usize; 2],
    rect: [f64; 4],
    lower: f64,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for Candidate {}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // BinaryHeap pops the least certified time first; every geometric tie
        // is resolved by canonical cell and rectangle coordinates.
        other
            .lower
            .total_cmp(&self.lower)
            .then_with(|| other.cell.cmp(&self.cell))
            .then_with(|| {
                for (a, b) in other.rect.iter().zip(self.rect.iter()) {
                    let order = a.total_cmp(b);
                    if !order.is_eq() {
                        return order;
                    }
                }
                std::cmp::Ordering::Equal
            })
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct LocalHit {
    pub time: f64,
    pub point: DVec3,
    pub normal: DVec3,
    pub penetration: f64,
}

/// Earliest intersection with the radius-expanded vertical column box.
/// Each piece of squared distance is quadratic in segment time.
fn column_box_entry(a: DVec3, delta: DVec3, rect: [f64; 4], top: f64, radius: f64) -> Option<f64> {
    let mut cuts = vec![0.0, 1.0];
    for (origin, speed, limits) in [
        (a.x, delta.x, [rect[0], rect[2]]),
        (a.z, delta.z, [rect[1], rect[3]]),
        (a.y, delta.y, [top, top]),
    ] {
        if speed != 0.0 {
            for limit in limits {
                let t = (limit - origin) / speed;
                if t > 0.0 && t < 1.0 {
                    cuts.push(t);
                }
            }
        }
    }
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    for span in cuts.windows(2) {
        let mid = (span[0] + span[1]) * 0.5;
        let mut quadratic = 0.0;
        let mut linear = 0.0;
        let mut constant = -radius * radius;
        for (origin, speed, low, high) in [
            (a.x, delta.x, rect[0], rect[2]),
            (a.z, delta.z, rect[1], rect[3]),
            (a.y, delta.y, f64::NEG_INFINITY, top),
        ] {
            let value = origin + speed * mid;
            let boundary = if value < low {
                low
            } else if value > high {
                high
            } else {
                continue;
            };
            let offset = origin - boundary;
            quadratic += speed * speed;
            linear += 2.0 * offset * speed;
            constant += offset * offset;
        }
        let evaluate = |t: f64| (quadratic * t + linear) * t + constant;
        if evaluate(span[0]) <= 0.0 {
            return Some(span[0]);
        }
        if quadratic > 0.0 {
            let discriminant = linear * linear - 4.0 * quadratic * constant;
            if discriminant >= 0.0 {
                let entry = (-linear - discriminant.sqrt()) / (2.0 * quadratic);
                if entry >= span[0] && entry <= span[1] {
                    return Some(entry);
                }
            }
        }
    }
    None
}

#[derive(Debug, Clone, Copy)]
enum SweepPhase {
    TubeStep,
    TubeCell,
    RootEnclosure,
    Candidate,
    Witness,
    WitnessNormal,
    ChildEnclosure,
}

#[derive(Debug, Default)]
struct SweepWork {
    phase: Option<SweepPhase>,
    tube_steps: usize,
    cell_visits: usize,
    root_enclosures: usize,
    candidate_visits: usize,
    witness_samples: usize,
    witness_normal_samples: usize,
    child_enclosures: usize,
    point_cache_hits: usize,
    normal_cache_hits: usize,
    last: Option<Candidate>,
    frontier: Option<Candidate>,
    best_time: Option<f64>,
    best_point: Option<DVec3>,
    best_distance_at_lower: Option<f64>,
    queue_len: usize,
}

/// Exact memoization lives only for one immutable source sweep. Entries are
/// inserted after charged computations, bounding memory by the same work lease.
#[derive(Default)]
struct SweepSamples {
    heights: std::collections::HashMap<[u64; 2], f64>,
    normals: std::collections::HashMap<[u64; 3], DVec3>,
}

impl SweepSamples {
    fn height(
        &mut self,
        oracle: &SurfaceOracle,
        x: f64,
        z: f64,
        remaining: &mut usize,
        work: &mut SweepWork,
    ) -> Result<f64, String> {
        let key = [x.to_bits(), z.to_bits()];
        if let Some(&height) = self.heights.get(&key) {
            work.point_cache_hits += 1;
            return Ok(height);
        }
        charge(remaining, work, SweepPhase::Witness)?;
        let height = oracle.height_at(x, z);
        if !height.is_finite() {
            return Err("terrain camera sweep sampled invalid height".into());
        }
        self.heights.insert(key, height);
        Ok(height)
    }

    fn normal(
        &mut self,
        oracle: &SurfaceOracle,
        x: f64,
        z: f64,
        eps: f64,
        half: f64,
        remaining: &mut usize,
        work: &mut SweepWork,
    ) -> Result<DVec3, String> {
        let key = [x.to_bits(), z.to_bits(), eps.to_bits()];
        if let Some(&normal) = self.normals.get(&key) {
            work.normal_cache_hits += 1;
            return Ok(normal);
        }
        for _ in 0..4 {
            charge(remaining, work, SweepPhase::WitnessNormal)?;
        }
        let normal = DVec3::from_array(lunco_terrain_core::normal_at_bounded(
            oracle, x, z, eps, half,
        ));
        if !normal.is_finite() || normal.y <= 0.0 {
            return Err("terrain camera witness has an invalid source normal".into());
        }
        self.normals.insert(key, normal);
        Ok(normal)
    }
}

fn record_frontier(work: &mut SweepWork, queue: &std::collections::BinaryHeap<Candidate>) {
    work.frontier = queue.peek().copied();
    work.queue_len = queue.len();
}

fn record_best(work: &mut SweepWork, best: Option<LocalHit>, eye: DVec3) {
    work.best_time = best.map(|hit| hit.time);
    work.best_point = best.map(|hit| hit.point);
    work.best_distance_at_lower = best.map(|hit| eye.distance(hit.point));
}

fn charge(remaining: &mut usize, work: &mut SweepWork, phase: SweepPhase) -> Result<(), String> {
    work.phase = Some(phase);
    match phase {
        SweepPhase::TubeStep => work.tube_steps += 1,
        SweepPhase::TubeCell => work.cell_visits += 1,
        SweepPhase::RootEnclosure => work.root_enclosures += 1,
        SweepPhase::Candidate => work.candidate_visits += 1,
        SweepPhase::Witness => work.witness_samples += 1,
        SweepPhase::WitnessNormal => work.witness_normal_samples += 1,
        SweepPhase::ChildEnclosure => work.child_enclosures += 1,
    }
    *remaining = remaining
        .checked_sub(1)
        .ok_or_else(|| format!("terrain camera sweep work budget exhausted during {phase:?}"))?;
    Ok(())
}

/// An upper affine surface bounds the solid terrain below it. Its infinite
/// half-space gives an independent, conservative sphere-entry lower bound,
/// retaining slope correlation along long near-tangent motion.
fn plane_entry(
    a: DVec3,
    delta: DVec3,
    plane: HeightPlane,
    residual_top: f64,
    radius: f64,
) -> Result<Option<f64>, String> {
    let [gx, gz] = plane.gradient;
    let normal_length = gx.hypot(gz).hypot(1.0);
    let height = a.y
        - plane.height
        - residual_top
        - gx * (a.x - plane.origin[0])
        - gz * (a.z - plane.origin[1]);
    let speed = delta.y - gx * delta.x - gz * delta.z;
    let reach = radius * normal_length;
    let magnitude = a.y.abs()
        + plane.height.abs()
        + residual_top.abs()
        + gx.abs() * (a.x.abs() + plane.origin[0].abs())
        + gz.abs() * (a.z.abs() + plane.origin[1].abs())
        + delta.y.abs()
        + (gx * delta.x).abs()
        + (gz * delta.z).abs()
        + reach;
    let pad = 128.0 * f64::EPSILON * magnitude.max(1.0);
    if !height.is_finite() || !speed.is_finite() || !reach.is_finite() || !pad.is_finite() {
        return Err("terrain camera affine enclosure exceeds finite coordinates".into());
    }
    let separation = height - reach - pad;
    if separation <= 0.0 {
        return Ok(Some(0.0));
    }
    if speed >= 0.0 {
        return Ok(None);
    }
    let time = -separation / speed;
    Ok((time <= 1.0).then_some(time))
}

fn candidate_lower(
    oracle: &SurfaceOracle,
    cell: [usize; 2],
    rect: [f64; 4],
    a: DVec3,
    delta: DVec3,
    radius: f64,
    remaining: &mut usize,
) -> Result<Option<f64>, String> {
    let enclosure = oracle.height_enclosure_in_cell(cell, rect, remaining)?;
    let plane = enclosure.plane;
    let residual = enclosure.residual;
    let Some(slope_lower) = plane_entry(a, delta, plane, residual[1], radius)? else {
        return Ok(None);
    };
    let top = enclosure.height[1].min(plane.height_bounds(rect)[1] + residual[1]);
    if !top.is_finite() {
        return Err("terrain camera affine height exceeds finite coordinates".into());
    }
    Ok(column_box_entry(a, delta, rect, top, radius).map(|box_lower| box_lower.max(slope_lower)))
}

/// A tangent plane proposes a surface location; it never certifies contact.
/// The caller samples the actual oracle there and casts its vertical column.
fn projected_witness(
    a: DVec3,
    delta: DVec3,
    lower: f64,
    sample: DVec3,
    normal: DVec3,
    rect: [f64; 4],
    radius: f64,
) -> Result<[f64; 2], String> {
    if !normal.is_finite() || normal.y <= 0.0 {
        return Err("terrain camera witness has an invalid source normal".into());
    }
    let plane = HeightPlane {
        origin: [sample.x, sample.z],
        height: sample.y,
        gradient: [-normal.x / normal.y, -normal.z / normal.y],
    };
    let time = plane_entry(a, delta, plane, 0.0, radius)?
        .unwrap_or(lower)
        .max(lower);
    let eye = a + delta * time;
    let point = eye - normal * (eye - sample).dot(normal);
    if !point.is_finite() {
        return Err("terrain camera projected witness exceeds finite coordinates".into());
    }
    Ok([
        point.x.clamp(rect[0], rect[2]),
        point.z.clamp(rect[1], rect[3]),
    ])
}

/// Restrict a terrain patch to the swept sphere's XZ footprint. A contacting
/// point requires the sphere centre to enter the radius-expanded rectangle;
/// only centre positions during that time interval can reach this patch.
pub(crate) fn clip_to_swept_tube(
    a: DVec3,
    delta: DVec3,
    rect: [f64; 4],
    radius: f64,
) -> Option<[f64; 4]> {
    let end = a + delta;
    let scale =
        a.x.abs()
            .max(a.z.abs())
            .max(end.x.abs())
            .max(end.z.abs())
            .max(rect.into_iter().map(f64::abs).fold(0.0, f64::max))
            .max(radius)
            .max(1.0);
    let reach = (radius + 64.0 * f64::EPSILON * scale).next_up();
    let mut near = 0.0_f64;
    let mut far = 1.0_f64;
    for (origin, speed, low, high) in [
        (a.x, delta.x, rect[0], rect[2]),
        (a.z, delta.z, rect[1], rect[3]),
    ] {
        if speed == 0.0 {
            if origin < low - reach || origin > high + reach {
                return None;
            }
        } else {
            let t0 = (low - reach - origin) / speed;
            let t1 = (high + reach - origin) / speed;
            near = near.max(t0.min(t1).next_down());
            far = far.min(t0.max(t1).next_up());
        }
    }
    if near > far {
        return None;
    }
    let p0 = a + delta * near;
    let p1 = a + delta * far;
    let clipped = [
        rect[0].max((p0.x.min(p1.x) - reach).next_down()),
        rect[1].max((p0.z.min(p1.z) - reach).next_down()),
        rect[2].min((p0.x.max(p1.x) + reach).next_up()),
        rect[3].min((p0.z.max(p1.z) + reach).next_up()),
    ];
    (clipped[0] <= clipped[2] && clipped[1] <= clipped[3]).then_some(clipped)
}

/// `lower` is the earliest remaining enclosure entry, so its preceding prefix
/// is certified clear. A real solid point within the geometric tolerance
/// supplies proximity even when nearly tangent travel spans a long interval.
fn admit_witness(
    a: DVec3,
    delta: DVec3,
    lower: f64,
    radius: f64,
    tolerance: f64,
    hit: LocalHit,
) -> Result<Option<LocalHit>, String> {
    if !lower.is_finite() || lower < 0.0 || lower > hit.time {
        return Err("terrain camera enclosure entry exceeds its actual contact witness".into());
    }
    let time = lower.next_down().max(0.0);
    let separation = a + delta * time - hit.point;
    let distance = separation.length();
    if !distance.is_finite() {
        return Err("terrain camera proximity witness exceeds finite coordinates".into());
    }
    if distance > radius + tolerance {
        return Ok(None);
    }
    Ok(Some(LocalHit {
        time,
        point: hit.point,
        normal: hit.normal,
        penetration: if time == 0.0 {
            (radius - distance).max(0.0)
        } else {
            0.0
        },
    }))
}

/// A proximity point need not be the closest point to the admitted eye. Use
/// the source's surface normal, so grazing proximity to a flat plane stays flat.
fn contact_with_source_normal(
    oracle: &SurfaceOracle,
    mut hit: LocalHit,
    radius: f64,
    tolerance: f64,
    half: f64,
    samples: &mut SweepSamples,
    remaining: &mut usize,
    work: &mut SweepWork,
) -> Result<LocalHit, String> {
    hit.normal = samples.normal(
        oracle,
        hit.point.x,
        hit.point.z,
        tolerance.max(radius * 0.125),
        half,
        remaining,
        work,
    )?;
    Ok(hit)
}

/// Broad and narrow phases share exact surface identity. The admitted prefix
/// precedes every possible contact; its endpoint is within `tolerance` metres
/// of the requested sphere radius from an actual solid terrain witness.
pub(super) fn sweep(
    oracle: &SurfaceOracle,
    a: DVec3,
    b: DVec3,
    radius: f64,
    tolerance: f64,
    remaining: &mut usize,
) -> Result<Option<LocalHit>, String> {
    let initial_work = *remaining;
    let mut work = SweepWork::default();
    let mut samples = SweepSamples::default();
    sweep_with_work(oracle, a, b, radius, tolerance, remaining, &mut work, &mut samples).map_err(|reason| {
        format!("{reason}; local_start={a:?}; local_end={b:?}; radius_m={radius:.17}; tolerance_m={tolerance:.17}; initial_work={initial_work}; remaining_work={remaining}; phases={work:?}")
    })
}

fn sweep_with_work(
    oracle: &SurfaceOracle,
    a: DVec3,
    b: DVec3,
    radius: f64,
    tolerance: f64,
    remaining: &mut usize,
    work: &mut SweepWork,
    samples: &mut SweepSamples,
) -> Result<Option<LocalHit>, String> {
    let delta = b - a;
    let length = delta.length();
    let half = f64::from(oracle.domain_half_extent());
    // Enclosure/root arithmetic needs a representable error budget. Reject a
    // requested tolerance smaller than the f64 frame can express, rather than
    // turning overflow or lost subtractions into a fabricated clear result.
    let scale = a
        .abs()
        .max(b.abs())
        .max_element()
        .max(half)
        .max(radius)
        .max(1.0);
    if !delta.is_finite() || !length.is_finite() || 64.0 * f64::EPSILON * scale > tolerance {
        return Err("terrain camera sweep exceeds its f64 frame precision".into());
    }
    let spacing = oracle.occlusion_spacing();
    let res = oracle.domain_resolution();
    if res < 2 || !spacing.is_finite() || spacing <= 0.0 {
        return Err("terrain camera sweep has invalid raster geometry".into());
    }
    let footprint_radius = (radius + 64.0 * f64::EPSILON * scale).next_up();
    let bounds = [
        (a.x.min(b.x) - footprint_radius).next_down().max(-half),
        (a.z.min(b.z) - footprint_radius).next_down().max(-half),
        (a.x.max(b.x) + footprint_radius).next_up().min(half),
        (a.z.max(b.z) + footprint_radius).next_up().min(half),
    ];
    if bounds[0] > bounds[2] || bounds[1] > bounds[3] {
        return Ok(None);
    }
    let index = |v: f64| (((v + half) / spacing).floor() as usize).min(res - 2);
    // Enumerate only a covered tube around the centre path. Midpoint discs
    // grow by half a path step, so this broad phase contains the entire swept
    // sphere without charging the area of a diagonal enclosing rectangle.
    let mut near = 0.0_f64;
    let mut far = 1.0_f64;
    for (origin, speed) in [(a.x, delta.x), (a.z, delta.z)] {
        if speed == 0.0 {
            if origin.abs() > half + footprint_radius {
                return Ok(None);
            }
        } else {
            let t0 = (-half - footprint_radius - origin) / speed;
            let t1 = (half + footprint_radius - origin) / speed;
            near = near.max(t0.min(t1).next_down());
            far = far.min(t0.max(t1).next_up());
        }
    }
    if near > far {
        return Ok(None);
    }
    let horizontal = bevy::math::DVec2::new(delta.x, delta.z).length() * (far - near);
    let steps = (horizontal / spacing).ceil().max(1.0) as usize;
    if steps > *remaining {
        return Err(format!(
            "terrain camera sweep work budget exhausted before tube enumeration: required_steps={steps}"
        ));
    }
    let reach = (footprint_radius + horizontal / (2.0 * steps as f64)).next_up();
    let mut cells = std::collections::BTreeSet::new();
    for step in 0..steps {
        charge(remaining, work, SweepPhase::TubeStep)?;
        let eye = a + delta * (near + (far - near) * (step as f64 + 0.5) / steps as f64);
        let footprint = [
            (eye.x - reach).next_down().max(-half),
            (eye.z - reach).next_down().max(-half),
            (eye.x + reach).next_up().min(half),
            (eye.z + reach).next_up().min(half),
        ];
        if footprint[0] > footprint[2] || footprint[1] > footprint[3] {
            continue;
        }
        for z in index(footprint[1])..=index(footprint[3]) {
            for x in index(footprint[0])..=index(footprint[2]) {
                charge(remaining, work, SweepPhase::TubeCell)?;
                cells.insert((z, x));
            }
        }
    }
    let mut queue = std::collections::BinaryHeap::new();
    for (z, x) in cells {
        charge(remaining, work, SweepPhase::RootEnclosure)?;
        let rect = [
            bounds[0].max(-half + x as f64 * spacing),
            bounds[1].max(-half + z as f64 * spacing),
            bounds[2].min(-half + (x + 1) as f64 * spacing),
            bounds[3].min(-half + (z + 1) as f64 * spacing),
        ];
        // Conservative midpoint discs also cover cells outside the exact
        // swept bounds. Only their nonempty intersections need an enclosure.
        if rect[0] > rect[2] || rect[1] > rect[3] {
            continue;
        }
        let Some(rect) = clip_to_swept_tube(a, delta, rect, radius) else {
            continue;
        };
        if let Some(lower) = candidate_lower(oracle, [x, z], rect, a, delta, radius, remaining)? {
            queue.push(Candidate {
                cell: [x, z],
                rect,
                lower,
            });
        }
        record_frontier(work, &queue);
    }
    let mut best: Option<LocalHit> = None;
    while let Some(node) = queue.pop() {
        work.last = Some(node);
        record_frontier(work, &queue);
        record_best(work, best, a + delta * node.lower);
        if let Some(hit) = best {
            if node.lower > hit.time {
                continue;
            }
            if let Some(admitted) = admit_witness(a, delta, node.lower, radius, tolerance, hit)? {
                return Ok(Some(contact_with_source_normal(
                    oracle, admitted, radius, tolerance, half, samples, remaining, work,
                )?));
            }
        }
        charge(remaining, work, SweepPhase::Candidate)?;
        let [x0, z0, x1, z1] = node.rect;
        let centre = (a + delta * node.lower).clamp(
            DVec3::new(x0, f64::NEG_INFINITY, z0),
            DVec3::new(x1, f64::INFINITY, z1),
        );
        let centre_height = samples.height(oracle, centre.x, centre.z, remaining, work)?;
        let normal = samples.normal(
            oracle,
            centre.x,
            centre.z,
            tolerance.max(radius * 0.125),
            half,
            remaining,
            work,
        )?;
        let projected = projected_witness(
            a,
            delta,
            node.lower,
            DVec3::new(centre.x, centre_height, centre.z),
            normal,
            node.rect,
            radius,
        )?;
        for (x, z, known_height) in [
            (projected[0], projected[1], None),
            (centre.x, centre.z, Some(centre_height)),
            ((x0 + x1) * 0.5, (z0 + z1) * 0.5, None),
            (x0, z0, None),
            (x1, z0, None),
            (x0, z1, None),
            (x1, z1, None),
        ] {
            let height = if let Some(height) = known_height {
                height
            } else {
                samples.height(oracle, x, z, remaining, work)?
            };
            if !height.is_finite() {
                return Err("terrain camera sweep sampled invalid height".into());
            }
            let eye = a + delta * node.lower;
            let proximity = LocalHit {
                time: node.lower,
                point: DVec3::new(x, eye.y.min(height), z),
                normal,
                penetration: 0.0,
            };
            if let Some(admitted) =
                admit_witness(a, delta, node.lower, radius, tolerance, proximity)?
            {
                return Ok(Some(contact_with_source_normal(
                    oracle, admitted, radius, tolerance, half, samples, remaining, work,
                )?));
            }
            if let Some(time) = column_box_entry(a, delta, [x, z, x, z], height, radius) {
                let eye = a + delta * time;
                let point = DVec3::new(x, eye.y.min(height), z);
                let separation = eye - point;
                let distance = separation.length();
                let normal = if distance > 0.0 {
                    separation / distance
                } else {
                    samples.normal(oracle, x, z, tolerance, half, remaining, work)?
                };
                let hit = LocalHit {
                    time,
                    point,
                    normal,
                    penetration: if time == 0.0 {
                        (radius - distance).max(0.0)
                    } else {
                        0.0
                    },
                };
                if time == 0.0 && hit.penetration > tolerance {
                    return Ok(Some(contact_with_source_normal(
                        oracle, hit, radius, tolerance, half, samples, remaining, work,
                    )?));
                }
                if best.is_none_or(|previous| time < previous.time) {
                    best = Some(hit);
                }
                record_best(work, best, a + delta * node.lower);
            }
            if let Some(hit) = best {
                if let Some(admitted) = admit_witness(a, delta, node.lower, radius, tolerance, hit)?
                {
                    return Ok(Some(contact_with_source_normal(
                        oracle, admitted, radius, tolerance, half, samples, remaining, work,
                    )?));
                }
            }
        }
        let split_x = x1 - x0 >= z1 - z0;
        let mid = if split_x {
            (x0 + x1) * 0.5
        } else {
            (z0 + z1) * 0.5
        };
        if (split_x && (mid == x0 || mid == x1)) || (!split_x && (mid == z0 || mid == z1)) {
            return Err(
                "terrain camera sweep cannot resolve contact within numeric precision".into(),
            );
        }
        let rects = if split_x {
            [[x0, z0, mid, z1], [mid, z0, x1, z1]]
        } else {
            [[x0, z0, x1, mid], [x0, mid, x1, z1]]
        };
        for rect in rects {
            charge(remaining, work, SweepPhase::ChildEnclosure)?;
            let Some(rect) = clip_to_swept_tube(a, delta, rect, radius) else {
                continue;
            };
            if let Some(lower) =
                candidate_lower(oracle, node.cell, rect, a, delta, radius, remaining)?
            {
                let lower = lower.max(node.lower);
                if best.is_none_or(|hit| lower <= hit.time) {
                    queue.push(Candidate {
                        cell: node.cell,
                        rect,
                        lower,
                    });
                }
                record_frontier(work, &queue);
            }
        }
    }
    match best {
        Some(hit) => {
            let admitted = admit_witness(a, delta, hit.time, radius, tolerance, hit)?
                .ok_or("terrain camera contact witness exceeds its distance tolerance")?;
            Ok(Some(contact_with_source_normal(
                oracle, admitted, radius, tolerance, half, samples, remaining, work,
            )?))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grazing_admission_uses_witnessed_distance_and_source_normal() {
        let radius = 0.027;
        let tolerance = 0.0001;
        let a = DVec3::new(-1.0, radius + 0.00005, 0.0);
        let delta = DVec3::new(2.0, -0.0001, 0.0);
        let hit = LocalHit {
            time: 0.5,
            point: DVec3::ZERO,
            normal: DVec3::Y,
            penetration: 0.0,
        };
        let lower = 0.499;
        assert!((hit.time - lower) * delta.length() > 10.0 * tolerance);
        let admitted = admit_witness(a, delta, lower, radius, tolerance, hit)
            .unwrap()
            .unwrap();
        let separation = a + delta * admitted.time - admitted.point;
        assert!(admitted.time <= lower && separation.length() <= radius + tolerance);
        let oracle = SurfaceOracle::bare(std::sync::Arc::new(
            lunco_obstacle_field::field::HeightGrid::new_flat(5, 2.0),
        ));
        let mut samples = SweepSamples::default();
        let mut work = SweepWork::default();
        let mut budget = 32;
        let admitted = contact_with_source_normal(
            &oracle,
            admitted,
            radius,
            tolerance,
            2.0,
            &mut samples,
            &mut budget,
            &mut work,
        )
        .unwrap();
        assert_eq!(admitted.normal, DVec3::Y);
        assert!((admitted.normal - separation.normalize()).length() > 0.01);
        assert!(
            admit_witness(a, delta, 0.49, radius, tolerance, hit)
                .unwrap()
                .is_none()
        );
        assert!(admit_witness(a, delta, 0.51, radius, tolerance, hit).is_err());
    }

    #[test]
    fn exact_sweep_memo_charges_only_point_and_normal_computations() {
        let oracle = SurfaceOracle::bare(std::sync::Arc::new(
            lunco_obstacle_field::field::HeightGrid::new_flat(5, 2.0),
        ));
        let mut samples = SweepSamples::default();
        let mut work = SweepWork::default();
        let mut budget = 32;
        let x = 0.13_f64;
        let z = 0.17;
        let first = samples
            .height(&oracle, x, z, &mut budget, &mut work)
            .unwrap();
        assert_eq!(
            samples
                .height(&oracle, x, z, &mut budget, &mut work)
                .unwrap()
                .to_bits(),
            first.to_bits()
        );
        samples
            .height(&oracle, x.next_up(), z, &mut budget, &mut work)
            .unwrap();
        let normal = samples
            .normal(&oracle, x, z, 0.01, 2.0, &mut budget, &mut work)
            .unwrap();
        assert_eq!(
            samples
                .normal(&oracle, x, z, 0.01, 2.0, &mut budget, &mut work)
                .unwrap(),
            normal
        );
        samples
            .normal(&oracle, x, z, 0.02, 2.0, &mut budget, &mut work)
            .unwrap();
        assert_eq!(budget, 22);
        assert_eq!(work.witness_samples, 2);
        assert_eq!(work.witness_normal_samples, 8);
        assert_eq!(work.point_cache_hits, 1);
        assert_eq!(work.normal_cache_hits, 1);
        assert_eq!(samples.heights.len(), 2);
        assert_eq!(samples.normals.len(), 2);
    }

    #[test]
    fn column_box_cast_handles_diagonal_miss_and_slope_trajectory() {
        let rect = [0.0, 9.0, 1.0, 10.0];
        assert_eq!(
            column_box_entry(
                DVec3::new(0.0, 1.0, 0.0),
                DVec3::new(10.0, 0.0, 10.0),
                rect,
                2.0,
                0.1
            ),
            None
        );
        let t = column_box_entry(
            DVec3::new(0.0, 2.0, 0.0),
            DVec3::new(0.0, -4.0, 0.0),
            [-1.0, -1.0, 1.0, 1.0],
            0.0,
            0.1,
        )
        .unwrap();
        assert!((t - 0.475).abs() < 1e-12);
    }
    #[test]
    fn affine_half_space_entry_accounts_for_sphere_slope() {
        let plane = HeightPlane {
            origin: [0.0; 2],
            height: 0.0,
            gradient: [1.0, 0.0],
        };
        let time = plane_entry(DVec3::Y * 2.0, DVec3::NEG_Y * 4.0, plane, 0.0, 0.1)
            .unwrap()
            .unwrap();
        assert!((time - (2.0 - 0.1 * 2.0_f64.sqrt()) / 4.0).abs() < 1e-10);
        assert!(
            plane_entry(DVec3::Y * 2.0, DVec3::Y, plane, 0.0, 0.1)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn projected_sphere_contact_is_a_noncentral_actual_column_witness() {
        let plane = HeightPlane {
            origin: [0.0; 2],
            height: 0.0,
            gradient: [0.7, -0.4],
        };
        let normal = DVec3::new(-0.7, 1.0, 0.4).normalize();
        let a = DVec3::new(0.13, 2.0, 0.17);
        let delta = DVec3::NEG_Y * 4.0;
        let radius = 0.1;
        let [x, z] = projected_witness(
            a,
            delta,
            0.0,
            DVec3::ZERO,
            normal,
            [-1.0, -1.0, 1.0, 1.0],
            radius,
        )
        .unwrap();
        assert!((x - a.x).abs() > 0.01 && (z - a.z).abs() > 0.01);
        let actual_height = plane.height_at(x, z);
        let time = column_box_entry(a, delta, [x, z, x, z], actual_height, radius).unwrap();
        let expected =
            (a.y - plane.height_at(a.x, a.z) - radius * (1.0_f64 + 0.7 * 0.7 + 0.4 * 0.4).sqrt())
                / 4.0;
        assert!((time - expected).abs() < 1e-10);
    }
    #[test]
    fn swept_tube_clipping_covers_off_axis_diagonal_and_stationary_footprints() {
        let rect = [-1.0, -1.0, 1.0, 1.0];
        let a = DVec3::new(-1.0, 3.0, -10.0);
        let delta = DVec3::new(2.0, -4.0, 20.0);
        let clipped = clip_to_swept_tube(a, delta, rect, 0.1).unwrap();
        assert!(
            clipped[2] - clipped[0] < 0.43,
            "off-path terrain must be excluded: {clipped:?}"
        );
        for (a, delta, radius) in [
            (a, delta, 0.1),
            (
                DVec3::new(-5.0, 2.0, -4.0),
                DVec3::new(10.0, -3.0, 8.0),
                0.27,
            ),
            (DVec3::new(0.2, 2.0, -0.3), DVec3::ZERO, 0.1),
        ] {
            let clipped = clip_to_swept_tube(a, delta, rect, radius).unwrap();
            for step in 0..=128 {
                for angle in 0..32 {
                    for fraction in [0.0, 0.5, 1.0] {
                        let eye = a + delta * (step as f64 / 128.0);
                        let theta = angle as f64 * std::f64::consts::TAU / 32.0;
                        let x = eye.x + radius * fraction * theta.cos();
                        let z = eye.z + radius * fraction * theta.sin();
                        if x >= rect[0] && x <= rect[2] && z >= rect[1] && z <= rect[3] {
                            assert!(
                                x >= clipped[0]
                                    && x <= clipped[2]
                                    && z >= clipped[1]
                                    && z <= clipped[3],
                                "({x},{z}) outside {clipped:?}"
                            );
                        }
                    }
                }
            }
        }
        assert!(clip_to_swept_tube(DVec3::new(5.0, 1.0, 5.0), DVec3::ZERO, rect, 0.1).is_none());
        assert!(
            clip_to_swept_tube(DVec3::new(-2.0, 1.0, 5.0), DVec3::X * 4.0, rect, 0.1).is_none()
        );
    }
    #[test]
    fn sphere_volume_checks_side_contact_and_initial_overlap() {
        assert_eq!(
            column_box_entry(
                DVec3::new(0.05, 0.0, 0.0),
                DVec3::X,
                [0.0, 0.0, 0.0, 0.0],
                1.0,
                0.1
            ),
            Some(0.0)
        );
        let t = column_box_entry(
            DVec3::new(-2.0, 0.0, 0.0),
            DVec3::X * 4.0,
            [0.0, 0.0, 0.0, 0.0],
            1.0,
            0.1,
        )
        .unwrap();
        assert!((t - 0.475).abs() < 1e-12);
    }
    #[test]
    fn oracle_sweep_uses_witnesses_for_low_flat_motion_and_overlap() {
        let oracle = SurfaceOracle::bare(std::sync::Arc::new(
            lunco_obstacle_field::field::HeightGrid::new_flat(9, 4.0),
        ));
        let mut budget = 4096;
        assert!(
            sweep(
                &oracle,
                DVec3::new(-2.0, 0.03, -2.0),
                DVec3::new(2.0, 0.03, 2.0),
                0.025,
                0.0001,
                &mut budget
            )
            .unwrap()
            .is_none()
        );
        let mut budget = 4096;
        let hit = sweep(&oracle, DVec3::Y, DVec3::NEG_Y, 0.025, 0.0001, &mut budget)
            .unwrap()
            .unwrap();
        assert!((hit.time - 0.4875).abs() < 0.0001);
        assert!(hit.normal.dot(DVec3::Y) > 0.999);
        let mut budget = 4096;
        let inside = sweep(
            &oracle,
            DVec3::Y * 0.01,
            DVec3::Y,
            0.025,
            0.0001,
            &mut budget,
        )
        .unwrap()
        .unwrap();
        assert_eq!(inside.time, 0.0);
        assert!(inside.penetration > 0.014);
    }

    #[test]
    fn off_path_relief_cannot_become_a_collision_and_uncertainty_exhausts_budget() {
        let oracle = SurfaceOracle::new(
            std::sync::Arc::new(lunco_obstacle_field::field::HeightGrid::new_flat(3, 4.0)),
            vec![crate::oracle::HeightContribution {
                modifier: std::sync::Arc::new(lunco_terrain_core::BrushModifier::new(
                    [-1.5, 1.5],
                    0.05,
                    2.0,
                )),
                content_key: 7,
            }],
        );
        let a = DVec3::new(-2.0, 0.05, -2.0);
        let b = DVec3::new(2.0, 0.05, 2.0);
        let mut budget = 32768;
        assert!(
            sweep(&oracle, a, b, 0.025, 0.0001, &mut budget)
                .unwrap()
                .is_none()
        );
        let mut budget = 1;
        assert!(sweep(&oracle, a, b, 0.025, 0.0001, &mut budget).is_err());
    }

    #[test]
    fn continuous_sweep_cannot_skip_a_thin_ridge() {
        let oracle = SurfaceOracle::new(
            std::sync::Arc::new(lunco_obstacle_field::field::HeightGrid::new_flat(9, 4.0)),
            vec![crate::oracle::HeightContribution {
                modifier: std::sync::Arc::new(lunco_terrain_core::BrushModifier::new(
                    [0.125, 0.0],
                    0.02,
                    2.0,
                )),
                content_key: 9,
            }],
        );
        let mut budget = 32768;
        let hit = sweep(
            &oracle,
            DVec3::new(-3.0, 1.0, 0.0),
            DVec3::new(3.0, 1.0, 0.0),
            0.025,
            0.0001,
            &mut budget,
        )
        .unwrap()
        .unwrap();
        assert!(hit.time > 0.45 && hit.time < 0.55);
        assert!(hit.normal.is_finite() && hit.normal.length_squared() > 0.99);
    }

    #[test]
    fn long_diagonal_work_tracks_path_footprint() {
        let oracle = SurfaceOracle::bare(std::sync::Arc::new(
            lunco_obstacle_field::field::HeightGrid::new_flat(257, 128.0),
        ));
        let mut budget = 4096;
        assert!(
            sweep(
                &oracle,
                DVec3::new(-100.0, 0.05, -100.0),
                DVec3::new(100.0, 0.05, 100.0),
                0.025,
                0.0001,
                &mut budget
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn tube_disc_cells_outside_swept_bounds_do_not_reach_modifiers() {
        let oracle = SurfaceOracle::new(
            std::sync::Arc::new(lunco_obstacle_field::field::HeightGrid::new_flat(
                512, 500.0,
            )),
            vec![crate::oracle::HeightContribution {
                modifier: std::sync::Arc::new(lunco_terrain_core::Overzoom {
                    relief_amp: 0.0,
                    crater_mean: 0.0,
                    ..Default::default()
                }),
                content_key: 11,
            }],
        );
        let mut budget = 4096;
        assert!(
            sweep(
                &oracle,
                DVec3::new(0.0, 1.0, 0.9),
                DVec3::new(3.0, 1.0, 0.9),
                0.026,
                0.0001,
                &mut budget
            )
            .expect("valid composed flat terrain must certify the swept tube")
            .is_none()
        );
    }

    #[test]
    fn long_near_tangent_sweep_preserves_sloped_raster_correlation() {
        let mut grid = lunco_obstacle_field::field::HeightGrid::new_flat(257, 128.0);
        let height = |x: f64, z: f64| -1967.0 + 0.3 * x + 0.1 * z;
        for z in 0..257 {
            for x in 0..257 {
                grid.heights[z * 257 + x] = height(x as f64 - 128.0, z as f64 - 128.0);
            }
        }
        let oracle = SurfaceOracle::new(
            std::sync::Arc::new(grid),
            vec![crate::oracle::HeightContribution {
                modifier: std::sync::Arc::new(lunco_terrain_core::Overzoom {
                    relief_amp: 0.0,
                    crater_mean: 0.0,
                    ..Default::default()
                }),
                content_key: 13,
            }],
        );
        let radius = 0.026;
        let clearance = radius * (1.0_f64 + 0.3 * 0.3 + 0.1 * 0.1).sqrt() + 0.001;
        let mut budget = 8192;
        assert!(
            sweep(
                &oracle,
                DVec3::new(-100.0, height(-100.0, -100.0) + clearance, -100.0),
                DVec3::new(100.0, height(100.0, 100.0) + clearance, 100.0),
                radius,
                0.0001,
                &mut budget
            )
            .expect(
                "near-tangent motion must preserve the source slope within the normal work budget"
            )
            .is_none()
        );
    }

    fn composed_boom_source() -> SurfaceOracle {
        let mut grid = lunco_obstacle_field::field::HeightGrid::new_flat(512, 500.0);
        let spacing = f64::from(grid.spacing());
        for z in 0..512 {
            for x in 0..512 {
                let wx = -500.0 + x as f64 * spacing;
                let wz = -500.0 + z as f64 * spacing;
                grid.heights[z * 512 + x] = -1918.0 + 0.015 * (wx + 376.0) + 0.035 * (wz + 374.0);
            }
        }
        SurfaceOracle::new(
            std::sync::Arc::new(grid),
            vec![
                crate::oracle::HeightContribution {
                    modifier: std::sync::Arc::new(lunco_terrain_core::Overzoom {
                        seed: 424242,
                        max_radius: 0.6499999761581421,
                        min_radius: 0.18000000715255737,
                        crater_mean: 0.3499999940395355,
                        relief_amp: 0.012000000104308128,
                        relief_scale: 4.0,
                        ..Default::default()
                    }),
                    content_key: 17,
                },
                crate::oracle::HeightContribution {
                    modifier: std::sync::Arc::new(lunco_terrain_core::BodyCurvature::new(
                        1737400.0, -1918.0,
                    )),
                    content_key: 19,
                },
            ],
        )
    }

    #[test]
    fn hundred_metre_downward_boom_certifies_composed_procedural_contact() {
        let oracle = composed_boom_source();
        let a = DVec3::new(-376.37325594981667, -1909.874939441681, -374.0574798533926);
        let b = DVec3::new(-379.8156164105749, -1927.7998397946358, -472.37761830782983);
        let radius = 0.027;
        let tolerance = 0.0001;
        let mut budget = 8192;
        let hit = sweep(&oracle,a,b,radius,tolerance,&mut budget)
            .expect("the composed source must certify the downward boom within its existing work budget")
            .expect("the boom crosses the composed terrain");
        assert!((0.4..0.7).contains(&hit.time), "{hit:?}");
        assert!((hit.point.y - oracle.height_at(hit.point.x, hit.point.z)).abs() < 1e-9);
        let separation = (a + (b - a) * hit.time - hit.point).length();
        assert!(
            separation >= radius - 1e-8 && separation <= radius + 2.0 * tolerance + 1e-8,
            "{separation}"
        );
    }

    #[test]
    fn physical_radius_oblique_boom_certifies_composed_source_with_strict_tolerance() {
        let oracle = composed_boom_source();
        let a = DVec3::new(-380.0278138117865, -1910.0133290290833, -380.00889278552495);
        let b = DVec3::new(-297.1677193434631, -1927.7050462505792, -433.12771913683235);
        let radius = 0.025;
        let tolerance = 0.0001;
        let mut budget = 8192;
        let hit = sweep(&oracle, a, b, radius, tolerance, &mut budget)
            .expect("the oblique physical-radius sweep must fit its unchanged source work budget")
            .expect("the oblique boom crosses the composed terrain");
        assert!(hit.time > 0.0 && hit.time < 1.0, "{hit:?}");
        assert!(hit.point.y <= oracle.height_at(hit.point.x, hit.point.z) + 1e-9);
        let separation = (a + (b - a) * hit.time - hit.point).length();
        assert!(
            separation >= radius - 1e-8 && separation <= radius + tolerance + 1e-8,
            "{separation}"
        );
    }

    #[test]
    fn physical_radius_sideways_boom_keeps_strict_source_separation() {
        let oracle = composed_boom_source();
        let a = DVec3::new(
            -380.05009174649604,
            -1910.0176320075989,
            -380.01737671135925,
        );
        let b = DVec3::new(
            -478.26784466496423,
            -1927.9412144443747,
            -374.32355894744234,
        );
        let radius = 0.025;
        let tolerance = 0.0001;
        let mut budget = 8192;
        let hit = sweep(&oracle, a, b, radius, tolerance, &mut budget)
            .expect("the sideways physical-radius boom must fit the unchanged source work budget")
            .expect("the sideways boom crosses the composed terrain");
        assert!(hit.time > 0.0 && hit.time < 1.0, "{hit:?}");
        assert!(hit.point.y <= oracle.height_at(hit.point.x, hit.point.z) + 1e-9);
        let separation = (a + (b - a) * hit.time - hit.point).length();
        assert!(
            separation >= radius - 1e-8 && separation <= radius + tolerance + 1e-8,
            "{separation}"
        );
    }

    #[test]
    fn grazing_composed_source_sweep_certifies_geometric_separation() {
        use lunco_terrain_core::HeightModifier;
        let a = DVec3::new(
            -414.16986083984375,
            -1918.0156707763672,
            -320.62322998046875,
        );
        let b = DVec3::new(-409.1048393249512, -1917.0786562561989, -346.49036026000977);
        let overzoom = lunco_terrain_core::Overzoom {
            seed: 424242,
            max_radius: 0.6499999761581421,
            min_radius: 0.18000000715255737,
            crater_mean: 0.3499999940395355,
            relief_amp: 0.012000000104308128,
            relief_scale: 4.0,
            ..Default::default()
        };
        let curvature = lunco_terrain_core::BodyCurvature::new(1737400.0, -1918.0);
        let delta_a = overzoom.delta_at(a.x, a.z) + curvature.apply(a.x, a.z, 0.0);
        let delta_b = overzoom.delta_at(b.x, b.z) + curvature.apply(b.x, b.z, 0.0);
        let anchor = a.y - 0.3 - delta_a;
        let gradient_z = (b.y - a.y + 0.35 - (delta_b - delta_a)) / (b.z - a.z);
        let mut grid = lunco_obstacle_field::field::HeightGrid::new_flat(512, 500.0);
        let spacing = f64::from(grid.spacing());
        for z in 0..512 {
            for x in 0..512 {
                grid.heights[z * 512 + x] =
                    anchor + gradient_z * (-500.0 + z as f64 * spacing - a.z);
            }
        }
        let oracle = SurfaceOracle::new(
            std::sync::Arc::new(grid),
            vec![
                crate::oracle::HeightContribution {
                    modifier: std::sync::Arc::new(overzoom),
                    content_key: 23,
                },
                crate::oracle::HeightContribution {
                    modifier: std::sync::Arc::new(curvature),
                    content_key: 29,
                },
            ],
        );
        let radius = 0.027;
        let tolerance = 0.0001;
        let mut budget = 8192;
        let hit = sweep(&oracle, a, b, radius, tolerance, &mut budget)
            .expect("grazing source proximity must fit the existing work budget")
            .expect("the shallow path reaches the composed terrain");
        assert!(hit.time > 0.0 && hit.time < 1.0, "{hit:?}");
        assert!(hit.point.y <= oracle.height_at(hit.point.x, hit.point.z) + 1e-9);
        let separation = a + (b - a) * hit.time - hit.point;
        assert!(
            separation.length() >= radius - 1e-8
                && separation.length() <= radius + tolerance + 1e-9,
            "{separation:?}"
        );
        let source_normal = DVec3::from_array(lunco_terrain_core::normal_at_bounded(
            &oracle,
            hit.point.x,
            hit.point.z,
            tolerance.max(radius * 0.125),
            500.0,
        ));
        assert!((hit.normal - source_normal).length() < 1e-12);
    }
}
