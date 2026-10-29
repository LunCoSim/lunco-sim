// @timing presentation
//! Build a Scene — a guided tour of authoring the 3D world (the build-usd-scene skill).
//!
//! Coach-mark tour (advances on Next / Back / Skip). Loads a small world so the
//! panels it points at have content. Teaches the spawn palette, the gizmo, the
//! inspector, and the fact that everything here is OpenUSD underneath.

fn steps() {
    [
        #{ anchor: "",                      title: "Build a scene",        body: "Use this tour to place and edit objects. Press Next to move on; press Back to review a step." },
        #{ anchor: "panel.spawn_palette",   title: "Place an object",       body: "Open Spawn in the right dock. Click an object in the palette to add it to the scene." },
        #{ anchor: "panel.center",          title: "Move the object",       body: "Select the object in the Entities panel. Drag the gizmo to move, rotate, or scale it." },
        #{ anchor: "panel.sandbox_inspector", title: "Edit its properties",   body: "With the object selected, open Inspector. Change an authored property such as colour, mass, or material." },
        #{ anchor: "",                      title: "Save the scene",       body: "The shipped assets are read-only. Save your authored changes through the active Twin or document Save workflow." },
        #{ anchor: "",                      title: "Review your scene",           body: "This tour showed placement, transforms, properties, and saving; those actions were not assessed. If you made an edit, save and reopen your Twin to check it persists. Finish this tour to choose your next lesson." },
    ]
}

fn show(s, i) { let step = s[i]; coach(i, s.len(), step.anchor, step.title, step.body); }

fn on_start(me, ctx) {
    // The optional world is selected by the catalog and mounted by the generic
    // launcher before this runs.
    cmd("ActivatePerspective", #{ id: "rover_build" });
    this.i = 0;
    show(steps(), 0);
    notify_kind("Tutorial: Build a Scene", "info");
}

fn on_event(me, evt, ctx) {
    if tutorials::navigation(evt, ctx) { return; }
    this = tutorials::tour_event(this, evt, steps(), ctx);
}
