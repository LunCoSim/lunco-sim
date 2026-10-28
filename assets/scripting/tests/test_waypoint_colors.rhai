// Run through scripts/api/run_rhai_test.sh in an owned production session.
let checks = [];
let blue = [0.03, 0.35, 1.0];
let amber = [1.0, 0.45, 0.03];
let plan = waypoint_editor::color_plan("@runtime@", "/Scene/Route", blue);
checks.push(t_eq(plan.ok, true, "route color has a typed authoring plan"));
checks.push(t_eq(plan.ops[0].SetAttribute.name, "primvars:displayColor", "color uses standard USD"));
checks.push(t_eq(plan.ops[0].SetAttribute.edit_target, "@runtime@", "color belongs to the persistent edit layer"));
for invalid in [[], [1.0, 0.0], [1.1, 0.0, 0.0], [-0.1, 0.0, 0.0], ["red", 0.0, 0.0]] {
    checks.push(t_eq(waypoint_editor::color_plan("@runtime@", "/Scene/Route", invalid).ok,
        false, "invalid color rejected before editing"));
}
let snapshot = #{paths:["/Scene/Route/P0"], points:[[1.0, 0.0, 2.0]],
    color:blue, marker_colors:[blue]};
let topology = waypoint_editor::route_view_key(snapshot);
let style = waypoint_editor::route_style_key(snapshot);
snapshot.color = amber;
checks.push(t_eq(waypoint_editor::route_view_key(snapshot), topology,
    "style changes never retarget navigation"));
checks.push(t_eq(waypoint_editor::route_style_key(snapshot) != style, true,
    "style changes invalidate route presentation"));
checks.push(t_eq(waypoint_editor::marker_state_color(blue, false), blue,
    "unvisited markers retain authored color"));
checks.push(t_eq(waypoint_editor::marker_state_color((), true), [0.55,0.55,0.55],
    "legacy visited markers remain gray"));
checks.push(t_eq(waypoint_editor::marker_state_color(blue, true) != blue, true,
    "visited markers distinguish progress without losing route hue"));
t_report(checks);
