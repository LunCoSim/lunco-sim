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
    pub moon_path_geocentric: Vec<bevy::math::DVec3>, // Relative to Earth Center (Meters)
    pub earth_path_heliocentric: Vec<bevy::math::DVec3>, // Relative to Sun Center (Meters)
}

pub fn draw_trajectories_system(
    mut gizmos: Gizmos,
    clock: Res<CelestialClock>,
    registry_resource: Res<EphemerisResource>,
    q_bodies: Query<(&CelestialBody, &GlobalTransform)>,
    mut cache: ResMut<TrajectoryCache>,
) {
    let current_epoch = clock.epoch;
    
    // Find centers in world-space (f32) AND their "true" current ephemeris centers (f64)
    let mut sun_world_pos = None;
    let mut earth_world_pos = None;
    
    for (body, gtf) in q_bodies.iter() {
        if body.ephemeris_id == 10 { sun_world_pos = Some(gtf.translation()); }
        if body.ephemeris_id == 399 { earth_world_pos = Some(gtf.translation()); }
    }
    
    // 1. Re-compute Relative Paths if epoch changed significantly
    if (cache.last_update_jd - current_epoch).abs() > 0.05 || cache.moon_path_geocentric.is_empty() {
        cache.last_update_jd = current_epoch;
        cache.moon_path_geocentric.clear();
        cache.earth_path_heliocentric.clear();
        
        for i in -28..28 {
            let jd = current_epoch + (i as f64) * 0.5;
            let m_au = registry_resource.provider.position(301, jd);
            let e_au = registry_resource.provider.position(399, jd);
            // Result is in ecliptic AU
            cache.moon_path_geocentric.push(m_au - e_au);
        }
        for i in -50..50 {
            let jd = current_epoch + (i as f64) * 5.0;
            let e_au = registry_resource.provider.position(399, jd);
            // Result is in heliocentric ecliptic AU
            cache.earth_path_heliocentric.push(e_au);
        }
    }
    
    // 2. High-Precision Drawing via Anchor-Relative offsets
    
    // Moon (Geocentric relative to Earth)
    if let Some(earth_w) = earth_world_pos {
        let earth_sim_now = registry_resource.provider.position(399, current_epoch);
        let moon_sim_now = registry_resource.provider.position(301, current_epoch);
        let rel_now = moon_sim_now - earth_sim_now;
        
        for i in 0..(cache.moon_path_geocentric.len() - 1) {
            let p1_rel = cache.moon_path_geocentric[i] - rel_now;
            let p2_rel = cache.moon_path_geocentric[i+1] - rel_now;
            
            // p_rel points are small AU values, converting to meters is safe in f32
            let p1 = ecliptic_to_bevy(p1_rel).as_vec3() + (gtf_translation_of(&q_bodies, 301).unwrap_or(earth_w));
            let p2 = ecliptic_to_bevy(p2_rel).as_vec3() + (gtf_translation_of(&q_bodies, 301).unwrap_or(earth_w));
            
            gizmos.line(p1, p2, Color::srgba(0.5, 0.7, 1.0, 0.4));
        }
    }
    
    // Earth (Heliocentric relative to Sun)
    if let Some(sun_w) = sun_world_pos {
        let sun_sim_now = registry_resource.provider.position(10, current_epoch);
        let earth_sim_now = registry_resource.provider.position(399, current_epoch);
        let rel_now = earth_sim_now - sun_sim_now;
        
        for i in 0..(cache.earth_path_heliocentric.len() - 1) {
            // helio position of Earth at track epoch
            let p_track_helio = cache.earth_path_heliocentric[i]; 
            // offset from current Earth helio
            let p1_rel = p_track_helio - rel_now;
            let p2_rel = cache.earth_path_heliocentric[i+1] - rel_now;

            let p1 = ecliptic_to_bevy(p1_rel).as_vec3() + (gtf_translation_of(&q_bodies, 399).unwrap_or(sun_w));
            let p2 = ecliptic_to_bevy(p2_rel).as_vec3() + (gtf_translation_of(&q_bodies, 399).unwrap_or(sun_w));
            
            gizmos.line(p1, p2, Color::srgba(1.0, 1.0, 0.6, 0.2));
        }
    }
}

fn gtf_translation_of(q_bodies: &Query<(&CelestialBody, &GlobalTransform)>, id: u32) -> Option<Vec3> {
    for (body, gtf) in q_bodies.iter() {
        if body.ephemeris_id == id { return Some(gtf.translation()); }
    }
    None
}
