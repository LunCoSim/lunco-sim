let px = mission_site_camera_angles([1.0, 0.0, 0.0]);
let nx = mission_site_camera_angles([-1.0, 0.0, 0.0]);
let nz = mission_site_camera_angles([0.0, 0.0, -1.0]);
let vertical = mission_site_camera_angles([0.0, 1.0, 0.0]);

t_report([
    t_true(px != () && abs(px.yaw_rad - atan(1.0, 0.0)) < 0.0001,
        "positive X site direction maps to the positive yaw quadrant"),
    t_true(nx != () && abs(nx.yaw_rad - atan(-1.0, 0.0)) < 0.0001,
        "negative X site direction maps to the negative yaw quadrant"),
    t_true(nz != () && abs(nz.yaw_rad.abs() - atan(0.0, -1.0).abs()) < 0.0001,
        "negative Z site direction preserves the opposite yaw quadrant"),
    t_true(vertical != () && abs(vertical.pitch_rad + atan(1.0, 0.0)) < 0.0001,
        "positive Y site direction maps to the overhead orbit pitch"),
    t_true(mission_site_camera_angles([0.0, 0.0, 0.0]) == (),
        "zero site direction is rejected"),
    t_true(mission_site_camera_angles([0.0, 1.0]) == (),
        "malformed site direction is rejected"),
]);
