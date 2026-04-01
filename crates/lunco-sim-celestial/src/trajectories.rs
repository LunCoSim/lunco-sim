use bevy::prelude::*;
use crate::registry::CelestialBody;
use crate::clock::CelestialClock;
use crate::ephemeris::EphemerisResource;
use crate::coords::ecliptic_to_bevy;

pub struct TrajectoryPlugin;

impl Plugin for TrajectoryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TrajectoryCache>();
        app.add_systems(Startup, configure_gizmos_system);
        // Run in PostUpdate AFTER Transform propagation to avoid 1-frame lag 
        // during high-speed time compression.
        app.add_systems(PostUpdate, draw_trajectories_system.after(bevy::transform::TransformSystems::Propagate));
    }
}

pub fn configure_gizmos_system(mut config_store: ResMut<GizmoConfigStore>) {
    let (config, _) = config_store.config_mut::<DefaultGizmoConfigGroup>();
    config.depth_bias = 0.0;
}

#[derive(Resource, Default)]
pub struct TrajectoryCache {
    pub last_update_jd: f64,
    pub moon_path_geocentric: Vec<bevy::math::DVec3>, 
    pub earth_path_heliocentric: Vec<bevy::math::DVec3>,
}

fn catmull_rom(p0: bevy::math::DVec3, p1: bevy::math::DVec3, p2: bevy::math::DVec3, p3: bevy::math::DVec3, t: f64) -> bevy::math::DVec3 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * (
        (2.0 * p1) +
        (-p0 + p2) * t +
        (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2 +
        (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3
    )
}

fn evaluate_spline(path: &[bevy::math::DVec3], start_jd: f64, spacing: f64, jd: f64) -> bevy::math::DVec3 {
    if path.is_empty() { return bevy::math::DVec3::ZERO; }
    
    let idx_f = (jd - start_jd) / spacing;
    let idx = idx_f.floor() as isize;
    let t = idx_f - (idx as f64);
    
    let get_p = |i: isize| {
        let clamped = i.clamp(0, path.len() as isize - 1) as usize;
        path[clamped]
    };
    
    let p0 = get_p(idx - 1);
    let p1 = get_p(idx);
    let p2 = get_p(idx + 1);
    let p3 = get_p(idx + 2);
    
    catmull_rom(p0, p1, p2, p3, t)
}

fn generate_fixed_jds(current_epoch: f64, max_days: f64, base_step: f64) -> Vec<f64> {
    let mut jds = Vec::new();
    
    // Base step is the coarsest step (far away)
    let step1 = base_step;
    let step2 = base_step / 10.0;
    let step3 = base_step / 100.0;
    let step4 = base_step / 1000.0;
    
    let mut add_zone = |start_rel: f64, end_rel: f64, step: f64| {
        let start_jd = current_epoch + start_rel;
        let end_jd = current_epoch + end_rel;
        
        let mut jd = (start_jd / step).ceil() * step;
        while jd <= end_jd + 1e-9 {
            jds.push(jd);
            jd += step;
        }
    };
    
    let z4 = max_days * 0.005;
    let z3 = max_days * 0.05;
    let z2 = max_days * 0.2;
    let z1 = max_days;
    
    add_zone(-z1, -z2, step1);
    add_zone(-z2, -z3, step2);
    add_zone(-z3, -z4, step3);
    add_zone(-z4, z4, step4);
    add_zone(z4, z3, step3);
    add_zone(z3, z2, step2);
    add_zone(z2, z1, step1);
    
    jds.push(current_epoch);
    
    jds.sort_by(|a, b| a.partial_cmp(b).unwrap());
    jds.dedup_by(|a, b| (*a - *b).abs() < step4 * 0.1);
    
    jds
}

pub fn draw_trajectories_system(
    mut gizmos: Gizmos,
    clock: Res<CelestialClock>,
    registry_resource: Res<EphemerisResource>,
    q_bodies: Query<(&CelestialBody, &GlobalTransform)>,
    mut cache: ResMut<TrajectoryCache>,
) {
    let current_epoch = clock.epoch;
    
    let earth_sim_now = registry_resource.provider.position(399, current_epoch);
    let moon_sim_now = registry_resource.provider.position(301, current_epoch);
    
    let moon_spacing = 0.05;
    let moon_half = 300; // 300 * 0.05 = 15.0 days
    
    let earth_spacing = 1.0;
    let earth_half = 210; // 210 * 1.0 = 210.0 days
    
    // 1. Re-compute Coarse Paths if epoch changed significantly
    if (cache.last_update_jd - current_epoch).abs() > 2.0 || cache.moon_path_geocentric.is_empty() {
        // Align to grid to prevent any possibility of popping during cache refresh!
        let aligned_epoch = (current_epoch / 2.0).round() * 2.0;
        cache.last_update_jd = aligned_epoch;
        
        cache.moon_path_geocentric.clear();
        cache.earth_path_heliocentric.clear();
        
        for i in -moon_half..=moon_half {
            let jd = aligned_epoch + (i as f64) * moon_spacing;
            let m_au = registry_resource.provider.position(301, jd);
            let e_au = registry_resource.provider.position(399, jd);
            cache.moon_path_geocentric.push(m_au - e_au);
        }
        for i in -earth_half..=earth_half {
            let jd = aligned_epoch + (i as f64) * earth_spacing;
            let e_au = registry_resource.provider.position(399, jd);
            cache.earth_path_heliocentric.push(e_au);
        }
    }
    
    // 2. Determine Local Rendering Anchor (f64 Precision Origin)
    let mut best_ref_id = 10;
    let mut best_ref_world = Vec3::ZERO;
    let mut min_dist = f32::MAX;
    
    for (body, gtf) in q_bodies.iter() {
        if matches!(body.ephemeris_id, 10 | 399 | 301) {
            let pos = gtf.translation();
            let dist = pos.length_squared();
            if dist < min_dist {
                min_dist = dist;
                best_ref_id = body.ephemeris_id;
                best_ref_world = pos;
            }
        }
    }
    
    let ref_au = registry_resource.provider.position(best_ref_id, current_epoch);
    let ref_meters = ecliptic_to_bevy(ref_au);
    
    let au_to_world = |p_au: bevy::math::DVec3| -> Vec3 {
        let p_meters = ecliptic_to_bevy(p_au);
        let offset_meters = p_meters - ref_meters;
        best_ref_world + offset_meters.as_vec3()
    };

    // 3. Draw with Adaptive Fixed-JD Spline Sampling
    let earth_jds = generate_fixed_jds(current_epoch, 200.0, 2.0);
    let moon_jds = generate_fixed_jds(current_epoch, 14.0, 0.2);
    
    let mut draw_spline_path = |path: &[bevy::math::DVec3], jds: &[f64], spacing: f64, half_count: isize, is_geocentric: bool, exact_helio_now: bevy::math::DVec3, color: Color| {
        if path.is_empty() { return; }
        let start_jd = cache.last_update_jd - (half_count as f64) * spacing;
        
        let spline_now = evaluate_spline(path, start_jd, spacing, current_epoch);
        let true_local_now = if is_geocentric { exact_helio_now - earth_sim_now } else { exact_helio_now };
        let error_offset = true_local_now - spline_now;
        
        let fade_duration = if is_geocentric { 0.5 } else { 5.0 };
        
        let mut world_pts = Vec::with_capacity(jds.len());
        
        for &jd in jds {
            let offset = jd - current_epoch;
            let mut p_spline = evaluate_spline(path, start_jd, spacing, jd);
            
            // Seamlessly blend the exact ephemeris position into the spline
            if offset.abs() < fade_duration {
                let fade = 1.0 - (offset.abs() / fade_duration);
                let smooth_fade = fade * fade * (3.0 - 2.0 * fade);
                p_spline += error_offset * smooth_fade;
            }
            
            let p_helio = if is_geocentric { earth_sim_now + p_spline } else { p_spline };
            world_pts.push(au_to_world(p_helio));
        }
        
        for i in 0..(world_pts.len() - 1) {
            gizmos.line(world_pts[i], world_pts[i+1], color);
        }
    };
    
    draw_spline_path(&cache.earth_path_heliocentric, &earth_jds, earth_spacing, earth_half, false, earth_sim_now, Color::srgba(1.0, 1.0, 0.6, 0.2));
    draw_spline_path(&cache.moon_path_geocentric, &moon_jds, moon_spacing, moon_half, true, moon_sim_now, Color::srgba(0.5, 0.7, 1.0, 0.4));
}
