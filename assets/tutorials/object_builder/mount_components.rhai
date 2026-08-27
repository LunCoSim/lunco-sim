//! Build from Parts — guided tour of the USD mount/socket workflow.
//!
//! The payload is the authored sandbox scene, which contains a Base host with
//! an Arm already attached and a second empty socket for the generic component
//! attach flow. This is a tour because the current lesson API does not expose a
//! public mount-completed event; the actual authoring operation still goes
//! through ApplyUsdOps/AttachComponent and persists through the active Twin.

fn steps() {
    [
        #{ anchor: "",                      focus: (),                title: "Build from parts",      body: "Object Builder authors USD parts, sockets, and joints. Press Next to follow one mount." },
        #{ anchor: "panel.side_browser",    focus: "usd_prim_tree",   title: "Select the host",       body: "In the USD prim tree, select Base. Inspector shows its authored structure and mount sockets." },
        #{ anchor: "panel.right_inspector", focus: "sandbox_inspector", title: "Snap a mounted arm",     body: "Open Mount in Inspector. Click Snap Arm. The plug and socket frames set the part's position and joint anchor." },
        #{ anchor: "panel.right_inspector", focus: "sandbox_inspector", title: "Fill an empty socket",         body: "Find the empty probe socket in Mount. On native builds, click Attach demo_probe.usda to create the child and its typed joint." },
        #{ anchor: "panel.side_browser",    focus: "usd_prim_tree",   title: "Inspect and save",       body: "Select the attached part or joint in the USD tree. Save the active Twin or document, then reload to verify the edit survives." },
        #{ anchor: "",                      focus: (),                title: "Remember the contract", body: "A hierarchy is not an attachment. A movable part needs a rigid body, a plug/socket contract, and a typed joint." },
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
