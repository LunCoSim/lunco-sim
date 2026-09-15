// Pure contract test for the frame-tagged Rhai point helpers.
//
// Runtime scene tests additionally verify that pointer hits arrive in the
// active frame. This test keeps the data contract and same-frame arithmetic
// executable without needing a scene or a fake ECS world.

fn coordinate_check(condition, message) {
    if condition { return ""; }
    message
}

fn coordinate_equal(actual, expected, message) {
    if actual == expected { return ""; }
    message
}

fn coordinate_report(failures) {
    let real = [];
    for failure in failures {
        if failure != "" { real.push(failure); }
    }
    if real.is_empty() { print("TESTS_OK " + failures.len()); }
    else { print("TESTS_FAIL " + real.len() + "/" + failures.len()); }
}

let failures = [];
let a = point3([1.0, 2.0, 3.0], "active_physics");
let b = point3([4.0, 6.0, 3.0], "active_physics");
let render = point3([1.0, 2.0, 3.0], "render");

failures.push(coordinate_check(point3([1.0, 2.0], "active_physics") == (),
                               "point3 rejects a non-3D value"));
failures.push(coordinate_check(point3([1.0, 2.0, 0.0 / 0.0], "active_physics") == (),
                               "point3 rejects non-finite values"));
failures.push(coordinate_check(point_valid(a), "point3 creates a valid tagged point"));
failures.push(coordinate_equal(point_frame(a), "active_physics", "point frame is explicit"));
failures.push(coordinate_equal(point_values(a), [1.0, 2.0, 3.0], "point values round-trip"));
failures.push(coordinate_check(point_same_frame(a, b), "same-frame points compare as same frame"));
failures.push(coordinate_check(!point_same_frame(a, render), "different frames never compare as same"));
failures.push(coordinate_equal(point_delta(b, a), [3.0, 4.0, 0.0], "same-frame delta is frame-safe"));
failures.push(coordinate_check(point_delta(a, render) == (), "cross-frame delta is rejected"));
failures.push(coordinate_equal(point_values(point_offset(a, [2.0, 0.0, -1.0])),
                               [3.0, 2.0, 2.0], "offset preserves the point frame"));
failures.push(coordinate_equal(point_distance(a, b), 5.0, "same-frame distance is available"));
failures.push(coordinate_check(point_distance(a, render) == (), "cross-frame distance is rejected"));

let pointer = pointer_point(#{ world_position: a });
failures.push(coordinate_check(pointer.ok && pointer.point == a,
                               "pointer_point accepts an active-frame point"));
let missing = pointer_point(#{ position_error: "active frame is unavailable" });
failures.push(coordinate_check(missing.ok == false
                    && missing.error == "active frame is unavailable",
                    "pointer_point preserves an unavailable-frame error"));
let intents = #{ pointer_intents: ["route.add_point"] };
failures.push(coordinate_check(pointer_intent(intents, "route.add_point"),
                               "configured pointer intent is visible to Rhai"));
failures.push(coordinate_check(!pointer_intent(intents, "route.context"),
                               "unconfigured pointer intent is not inferred"));

coordinate_report(failures);
