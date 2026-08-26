//! Build a Scene — a guided tour of authoring the 3D world (the build-usd-scene skill).
//!
//! Coach-mark tour (advances on Next / Back / Skip). Loads a small world so the
//! panels it points at have content. Teaches the spawn palette, the gizmo, the
//! inspector, and the fact that everything here is OpenUSD underneath.

fn steps() {
    [
        #{ anchor: "",                      title: "Build a Scene",       body: "LunCoSim scenes are OpenUSD — the same format Omniverse and Blender use. Let's see how you compose one. Next / Back below, Skip anytime." },
        #{ anchor: "panel.right_inspector", title: "The spawn palette",   body: "The right dock's Spawn panel lists catalogued objects — rovers, props, primitives. Click one to drop it into the scene." },
        #{ anchor: "panel.center",          title: "Place with the gizmo",body: "Select an object in the Entities panel or the USD prim tree; the transform gizmo appears in a build view. Drag to move, rotate, or scale it in the world. Help shows the viewport-selection gesture." },
        #{ anchor: "panel.right_inspector", title: "Tune properties",     body: "The Inspector shows the selected object's authored properties — colour, mass, material. Editing it creates a USD-backed authoring operation; the runtime ECS is only the projection." },
        #{ anchor: "panel.side_browser",    title: "It's all USD",        body: "The shipped assets under assets/scenes are a read-only library. Save authored changes through the active Twin/document Save workflow; the same authoring surface is available through SpawnEntity, MoveEntity and SetObjectProperty." },
        #{ anchor: "",                      title: "That's scene authoring", body: "You compose the world from parts, then attach behaviour and simulation. Next stop: scripting." },
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
