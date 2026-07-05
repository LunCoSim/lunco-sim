//! Build a Scene — a guided tour of authoring the 3D world (the build-usd-scene skill).
//!
//! Coach-mark tour (advances on Next / Back / Skip). Loads a small world so the
//! panels it points at have content. Teaches the spawn palette, the gizmo, the
//! inspector, and the fact that everything here is OpenUSD underneath.

fn steps() {
    [
        #{ anchor: "",                      title: "Build a Scene",       body: "LunCoSim scenes are OpenUSD — the same format Omniverse and Blender use. Let's see how you compose one. Next / Back below, Skip anytime." },
        #{ anchor: "panel.bottom",          title: "The spawn palette",   body: "The palette (bottom dock) lists catalogued objects — rovers, props, primitives. Click one to drop it into the scene." },
        #{ anchor: "panel.center",          title: "Place with the gizmo",body: "Click an object in the viewport to select it; the transform gizmo appears. Drag to move, rotate, or scale it in the world." },
        #{ anchor: "panel.right_inspector", title: "Tune properties",     body: "The Inspector shows the selected object's properties — colour, mass, material. Edits write straight back to the USD." },
        #{ anchor: "panel.side_browser",    title: "It's all USD",        body: "Scenes live under assets/scenes as .usda files. Save your edits there to make them permanent — or drive all of this over the API (SpawnEntity / MoveEntity / SetObjectProperty)." },
        #{ anchor: "",                      title: "That's scene authoring", body: "You compose the world from parts, then attach behaviour and simulation. Next stop: scripting." },
    ]
}

fn show(s, i) { let step = s[i]; coach(i, s.len(), step.anchor, step.title, step.body); }

fn on_start(me) {
    load_scene("tutorials/sandbox/first_drive.usda");
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
        notify_kind("Build a Scene complete 🎉", "success");
        emit("MISSION_COMPLETE", 0);
    } else { show(s, this.i); }
}
