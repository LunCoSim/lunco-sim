// ── An authored never-collide pair really does not collide ──────────────────
//
// TEST-ONLY. Drives BOTH scenes that share this rig, and which one it is running
// is read from the scene, not from a second copy of this file:
//
//   scenes/tests/filtered_pairs.usda             rel authored → no contact
//   scenes/tests/filtered_pairs_unfiltered.usda  rel removed  → contact
//
// WHAT IT GUARDS. `PhysicsFilteredPairsAPI` fails SILENTLY in both directions.
// Unread, the rel is a comment: the parts collide and a vehicle spends every step
// pushing itself apart, which looks like bad tuning rather than a dropped schema.
// Over-applied, it filters pairs nobody asked it to and contacts a mechanism
// depends on quietly stop existing. Neither says anything on screen.
//
// So the CONTACT is read directly, off the Pad's `lunco:sensor:contact`, rather
// than inferred from how far anything moved. The Pad is authored 0.5 m inside the
// Hull and is two joints away from it, so `JointCollisionDisabled` does not reach
// the pair and nothing but the authored rel can filter it.
//
// The control scene is half the test: "no contact" passes trivially on a rig
// whose parts never touched. The unfiltered scene proves the rig overlaps and the
// sensor can see it.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("FILTERED_PAIRS", "PASS"|"FAIL")` — the telemetry verdict `scene_test`
// reads for its exit code. Helpers come from `prelude/auto_tests.rhai`.

// Hull half-extent 1.0 + Pad half-extent 0.3 = 1.3 m between centres at the
// moment they would just touch. Authored at 0.8, so they overlap by 0.5 m.
const TOUCH_DIST = 1.3;

fn on_start(me) {
    this.t = 0.0;
    this.settle_s = 4.0;
    this.sample_s = 1.0;
    this.next_sample = 0.0;

    this.hull = find("/SandboxScene/Rig/Hull");
    this.pad = find("/SandboxScene/Rig/Pad");

    // WHICH scene this is, DECLARED by the scene rather than sniffed. Reading it
    // back off the Pad's own schema would make the expectation depend on the very
    // authoring under test.
    this.filtered = param(me, "filtered", -1.0);

    // Was there ever a contact to filter? Held over the whole run, because a
    // contact that existed for one tick is still a contact the filter missed.
    this.contact_seen = 0.0;
    this.min_gap = 1.0e9;
    this.phase = "settle";

    print("[pairs] ── authored filtered pair ────────────────────────────");
    print("[pairs] hull gid=" + this.hull + " pad gid=" + this.pad +
          "  filtered=" + this.filtered);
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.hull == () || this.hull < 0 || this.pad == () || this.pad < 0 {
        fail_fast("rig prims not found (hull=" + this.hull + " pad=" + this.pad +
                  ") — wrong scene?", "FILTERED PAIRS", "FILTERED_PAIRS");
        this.phase = "done";
        return;
    }

    this.t += dt();

    let c = get(this.pad, "contact");
    if c != () && c > this.contact_seen { this.contact_seen = c; }

    let d = distance(this.hull, this.pad);
    if d != () && d < this.min_gap { this.min_gap = d; }

    if this.t >= this.next_sample {
        print("[pairs] t=" + r2(this.t) + "s  gap=" + r4(d) + "m  contact=" +
              r2(c) + "  seen=" + r2(this.contact_seen));
        this.next_sample = this.t + this.sample_s;
    }

    if this.t >= this.settle_s {
        this.phase = "done";
        verdict(#{
            filtered: this.filtered,
            contact: get(this.pad, "contact"),
            seen: this.contact_seen,
            gap: this.min_gap,
            pad_pos: world_pos(this.pad),
        });
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   port present     a contact sensor that was never projected publishes
//                    nothing, and an absent port reads exactly like "no contact"
//                    — which is the PASS condition here. Without this check the
//                    filtered case goes green on a dead sensor.
//   they overlap     the anti-trivial guard. Both cases assert something about a
//                    pair that must be in contact range; if the parts never came
//                    within touching distance there is no pair, and the rest of
//                    this verdict is about nothing.
//   pad held         the assembly must still be where it was authored. In the
//                    filtered case a runaway would ALSO end contact.
//   filtered ⇒ never touched   the whole point: not once in the run.
//   unfiltered ⇒ touched       the control. Same rig, same sensor, rel removed.
fn verdict(s) {
    let f = [];
    f.push(t_present(s.contact, "Pad contact port"));

    f.push(t_true(s.filtered >= 0.0,
        "the scene declared no `filtered` parameter on its program prim, so this " +
        "test does not know which case it is running and would assert the wrong one"));

    f.push(t_true(s.gap < TOUCH_DIST,
        "the pad and hull never came within touching distance (closest " +
        r4(s.gap) + " m against " + TOUCH_DIST + " m) — there is no pair to " +
        "filter, so neither case below means anything"));

    f.push(t_bounded(s.pad_pos, 100.0,
        "pad — the rig left the world, which ends contact for reasons that have " +
        "nothing to do with the filter"));

    if s.filtered > 0.5 {
        f.push(t_true(s.seen < 0.5,
            "the authored filtered pair did NOT stop the contact: the pad reported " +
            "contact " + r2(s.seen) + " against a hull it names in " +
            "physics:filteredPairs — the rel was read as a comment"));
    } else {
        f.push(t_true(s.seen > 0.5,
            "control: with no physics:filteredPairs the overlapping pad must " +
            "report contact, and it reported " + r2(s.seen) + " — either the pair " +
            "is being filtered when nothing asked for it, or the sensor cannot " +
            "see this contact at all and the filtered case proves nothing"));
    }

    print("[pairs] ── results ──────────────────────────────────────────");
    print("[pairs]   filtered      " + s.filtered);
    print("[pairs]   contact seen  " + r2(s.seen) + " (final " + r2(s.contact) + ")");
    print("[pairs]   closest gap   " + r4(s.gap) + " m (touch at " + TOUCH_DIST + ")");

    report_verdict(f, "FILTERED PAIRS", "FILTERED_PAIRS");
}
