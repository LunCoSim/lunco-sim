// ── Direct link policy ──────────────────────────────────────────────────────
//
// The generic link kernel owns range, elevation and line-of-sight geometry. This
// policy owns which authored endpoint roles may use that geometry. A rover's
// high-gain antenna may use an Earth station, a lunar base, or a relay;
// rover-to-rover is a different radio system (the authored Wi-Fi projection)
// and must not appear in this graph.
//
// `ctx.builtin` is the kernel's geometry verdict. Keep it as the final gate so
// this policy cannot turn an occluded or out-of-range pair back on.

fn station_role(role) {
    role == "earth" || role == "base" || role == "relay"
}

fn link_connected(ctx) {
    let a_rover = ctx.class_a == "rover";
    let b_rover = ctx.class_b == "rover";

    if a_rover {
        return ctx.builtin && station_role(ctx.class_b);
    }
    if b_rover {
        return ctx.builtin && station_role(ctx.class_a);
    }

    ctx.builtin
}
