// The High preset's full-resolution shadow allocation must stay within its
// declared budget so a fresh graphics configuration does not allocate 4 GiB
// to directional cascades alone.
let checks = [];
let high_profile = invoke_hook("render.quality_profile", ["high"]);
checks.push(t_true(high_profile.ok == true
        && high_profile.value.directional_shadow_map_size == 4096
        && high_profile.value.directional_cascades == 4
        && high_profile.value.shadow_budget_bytes == 2 * 1024 * 1024 * 1024
        && high_profile.value.shadow_first_cascade_far_bound == 30.0
        && high_profile.value.shadow_maximum_distance == 3000.0
        && high_profile.value.shadow_filtering_quality == "hardware2x2"
        && high_profile.value.shadow_normal_bias == 2.0
        && high_profile.value.terrain_lod_max_depth == 9
        && high_profile.value.terrain_lod_tile_budget == 256
        && high_profile.value.horizon_march_steps == 96,
    "High shadow quality uses the authored high-detail values"));
t_report(checks);
