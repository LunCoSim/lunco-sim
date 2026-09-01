// Production acceptance for the safe default avatar terrain-collision path.
// The Twin opt-out is intentionally absent: the setting/unit contract covers
// explicit opt-in, while this scene proves real movement against a projected
// standard USD collider through the production executable.

const HOLD_S = 1.5;

fn fail(message) {
    report_verdict([message], "AVATAR TERRAIN COLLISION", "AVATAR_TERRAIN_COLLISION");
}

fn on_start(me) {
    this.elapsed = 0.0;
    this.avatar = find("/AvatarTerrainCollision/Avatar");
    this.start = ();
    this.done = false;

    if this.avatar < 0 {
        this.done = true;
        fail("avatar did not compose");
        return;
    }

    this.start = world_pos(this.avatar);
    let result = cmd("SimulateIntent", #{
        intent: "yaw_left",
        held: true,
        target: this.avatar,
    });
    if !result.ok {
        this.done = true;
        fail("could not hold semantic down intent: " + result.error);
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < HOLD_S { return; }

    cmd("SimulateIntent", #{ intent: "yaw_left", held: false, target: this.avatar });
    let end = world_pos(this.avatar);
    let moved_down = this.start[1] - end[1];
    let checks = [];
    checks.push(t_true(moved_down > 1.0, "avatar did not move down through its control path"));
    checks.push(t_true(end[1] > 0.75, "avatar capsule entered the ground collider"));
    checks.push(t_true(end[1] < 1.2, "avatar did not settle against the ground collider"));
    checks.push(t_bounded(end, 100.0, "avatar left the bounded collision fixture"));
    print("[avatar-terrain-collision] start_y=" + r4(this.start[1]) +
        " end_y=" + r4(end[1]) + " moved_down=" + r4(moved_down));
    this.done = true;
    report_verdict(checks, "AVATAR TERRAIN COLLISION", "AVATAR_TERRAIN_COLLISION");
}
