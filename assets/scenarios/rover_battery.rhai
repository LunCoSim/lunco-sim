// Battery ENFORCEMENT bridge — the actuation half of the `power = "battery"`
// variant (`components/power/rover_battery.usda`).
//
// The PHYSICS lives in `models/RoverBattery.mo`: an energy budget integrating
// on the solver clock, whose `alive` output is a smooth 0..1 low-charge
// cutoff (1.0 until the last ~2% of charge). This script owns NO dynamics —
// it makes `alive` MATTER by scaling the rover's `drive_left`/`drive_right`
// ports as the battery dies.
//
// WRITE-ORDER CARE. Another program owns those ports every tick — the built-in
// drive kernel or a Modelica drive-law bridge. Two writers with unspecified
// ordering is a race, so this bridge does NOT touch the ports at all while
// `alive >= 0.999` (the entire life of the charge — zero interference in the
// common case). The override below is a DELIBERATE last-writer clamp that only
// engages as the battery dies: once `alive` drops, being last (or racing the
// drive writer for a tick) is fine — both orderings converge to a browned-out
// command within a tick, and at alive = 0 the rover is dead either way.
//
// PORT READ CHOICE: `get(me, "drive_left")` on the rover root reads the port's
// current value — the same value the wheels consume — regardless of WHICH
// writer produced it (kernel or drive-law bridge), so this works under both
// `driveLaw` variants. If the port has no value yet (first ticks), `()` is
// returned and we leave the port alone rather than inventing a 0 write.
//
// Mounted as `def LunCoProgram "PowerBridge"` on the rover root, so `me` IS
// the rover — sibling lookup finds the battery model by the port it publishes,
// no name inference (the leg_spring.rhai predicate style).

fn sibling_out(me, name) {
    for c in children(me) {
        let v = get(c, name);
        if v != () { return v; }
    }
    ()
}

fn on_tick(me) {
    let alive = sibling_out(me, "alive");
    // No battery output yet (model still compiling on first ticks): do nothing.
    if alive == () { return; }
    // Healthy battery: never rewrite ports another program owns.
    if alive >= 0.999 { return; }

    let dl = get(me, "drive_left");
    if dl != () { set(me, "drive_left", dl * alive); }
    let dr = get(me, "drive_right");
    if dr != () { set(me, "drive_right", dr * alive); }
}
