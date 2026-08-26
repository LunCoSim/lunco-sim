//! Build from Parts — guided tour of the USD mount/socket workflow.
//!
//! The payload is the authored sandbox scene, which contains a Base host with
//! an Arm already attached and a second empty socket for the generic component
//! attach flow. This is a tour because the current lesson API does not expose a
//! public mount-completed event; the actual authoring operation still goes
//! through ApplyUsdOps/AttachComponent and persists through the active Twin.

fn steps() {
    [
        #{ anchor: "",                      focus: (),                title: "Build from Parts",      body: "The Object Builder is a USD authoring surface: the prim tree, Inspector, viewport, and mount contracts all describe one object. Next / Back below, Skip anytime." },
        #{ anchor: "panel.side_browser",    focus: "usd_prim_tree",   title: "Select the host",       body: "In the USD prim tree, select Base. Its Inspector shows the host's authored structure and mount sockets." },
        #{ anchor: "panel.right_inspector", focus: "sandbox_inspector", title: "Snap by contract",     body: "Open Mount in the Inspector. The arm socket already holds Arm, but the part is deliberately misaligned. Click Snap Arm: the plug frame and socket frame determine the placement and joint anchor." },
        #{ anchor: "panel.right_inspector", focus: "sandbox_inspector", title: "Empty sockets",         body: "The probe socket is empty and advertises a default component asset. On native builds, Attach demo_probe.usda resolves its authored plug and creates the referenced child plus typed joint through the USD command path. Browser builds show this action as native-only because local asset-file inspection is not available there yet." },
        #{ anchor: "panel.side_browser",    focus: "usd_prim_tree",   title: "Inspect and save",       body: "Select the attached part or joint in the USD tree to inspect the composed result. Save the active Twin/document to persist the authored base-layer edit; reload to verify it survives." },
        #{ anchor: "",                      focus: (),                title: "One authoring contract", body: "A hierarchy alone is not an attachment: a movable part needs a rigid body, a plug/socket contract, and a typed joint. Object Builder composes those USD facts without a vehicle-specific builder." },
    ]
}

fn show(s, i) { coach(i, s.len(), s[i].anchor, s[i].title, s[i].body); }

fn on_start(me) {
    this.i = 0;
    show(steps(), 0);
    notify_kind("Tutorial: Build from Parts", "info");
}

fn on_event(me, evt) {
    let s = steps();
    if evt.name == "cmd:TutorialNext" { this.i += 1; }
    else if evt.name == "cmd:TutorialBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:TutorialSkip" { this.i = s.len(); }
    else { return; }

    if this.i >= s.len() {
        end_tour();
        notify_kind("Build from Parts complete", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        show(s, this.i);
    }
}
