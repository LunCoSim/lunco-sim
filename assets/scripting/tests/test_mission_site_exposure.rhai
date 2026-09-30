// Run against a lunar Twin scene with a valid SiteAnchor.
// The orbital-view gate follows the current body and scene site, not rover possession.

let position = get_exposure("celestial-view", "site_orbit_position_m");
let active_body = get_exposure("celestial-view", "active_body_id");
let mission_available = get_exposure("celestial-view", "mission_view_available");
let site_valid = position != () && position.len() == 3;
let site_finite = false;
if site_valid {
    site_finite = position[0].is_finite() && position[1].is_finite() && position[2].is_finite();
}

t_report([
    t_true(site_valid, "the lunar site position is exposed as a three-vector"),
    t_true(site_finite, "the lunar site position is finite"),
    t_eq(mission_available, active_body == 301.0 && site_valid,
        "Mission visibility follows the Moon orbital pin and valid scene site"),
]);
