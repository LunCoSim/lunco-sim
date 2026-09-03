//! Build a Scene — a guided tour of authoring the 3D world (the build-usd-scene skill).
//!
//! Coach-mark tour (advances on Next / Back / Skip). Loads a small world so the
//! panels it points at have content. Teaches the spawn palette, the gizmo, the
//! inspector, and the fact that everything here is OpenUSD underneath.

fn steps() {
    [
        #{ anchor: "",                      title: "Build a scene",        body: "Use this tour to place and edit objects. Press Next to move on; press Back to review a step." },
        #{ anchor: "panel.spawn_palette",   title: "Place an object",       body: "Open Spawn in the right dock. Click an object in the palette to add it to the scene." },
        #{ anchor: "panel.center",          title: "Move the object",       body: "Select the object in Entities or the USD prim tree. Drag the gizmo to move, rotate, or scale it." },
        #{ anchor: "panel.sandbox_inspector", title: "Edit its properties",   body: "With the object selected, open Inspector. Change an authored property such as colour, mass, or material." },
        #{ anchor: "",                      title: "Save the scene",       body: "The shipped assets are read-only. Save your authored changes through the active Twin or document Save workflow." },
        #{ anchor: "",                      title: "Scene ready",           body: "You placed an object, transformed it, and edited its properties. Click Continue to learn how to script a rover." },
    ]
}

fn show(s, i) { let step = s[i]; coach(i, s.len(), step.anchor, step.title, step.body); }

fn on_start(me) {
    // The world this lesson teaches in is DECLARED in the curriculum layer
    // (a `payload` arc) and mounted by the launcher before this runs.
    this.i = 0;
    show(steps(), 0);
    notify_kind("Tutorial: Build a Scene", "info");
}

fn on_event(me, evt) {
    let s = steps();
    if evt.name == "cmd:TutorialNext" { this.i += 1; }
    else if evt.name == "cmd:TutorialBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:TutorialSkip" { this.i = s.len(); }
    else { return; }

    if this.i >= s.len() {
        end_tour();
        notify_kind("Build a Scene complete", "success");
        emit("MISSION_COMPLETE", 0);
    } else { show(s, this.i); }
}
