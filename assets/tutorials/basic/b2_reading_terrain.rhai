//! B2 — Reading the Terrain (Basic tutorial track).
//!
//! A coach-mark tour over the event traverse scene. The lesson the ДЗЗ
//! collaboration hinges on: before committing a wheel, read the ground for the
//! three hazards the brief names — SLOPE (tip-over / can't-climb), SUN-SHADOW
//! (cold, dark), and RADIO-SHADOW (no Earth link → autonomy only). It frames the
//! scene the teams will actually drive and hands off to the hands-on slope lesson.
//!
//! Visual/conceptual today; when the live slope/shadow rasters land (roadmap
//! P1.4) this tour can spotlight the actual coloured overlays. NEEDS (for the live
//! overlay step, optional): slope/shadow raster zones.

fn steps() {
    [
        #{ anchor: "",                   title: "Read before you drive",   body: "This is the site your team will cross: start at the NW corner, reach the Point of Interest on the low centre ground, and come back. First, learn to read its three hazards — that reading IS the job the remote-sensing (ДЗЗ) team does for you." },
        #{ anchor: "panel.center",       title: "Slope",                   body: "The killer hazard. Too steep to climb and the wheels just spin; too steep across the fall-line and a top-heavy rover rolls. The ДЗЗ slope map colours every metre by steepness — you plan a line that stays under the rover's safe angle (~20–25°)." },
        #{ anchor: "panel.center",       title: "Sun-shadow",              body: "The sun is low from the ESE, so the SE highland throws a long shadow across the centre. Shadow means cold and dark — hard to see, and (bonus rounds) your rover cools and loses battery faster in it. Note where the shade falls before you route through it." },
        #{ anchor: "panel.center",       title: "Radio-shadow",            body: "Earth sits low on the E horizon. Anywhere the terrain hides that horizon, your link home is blocked — that's radio-shadow, and the low centre near the POI is in it. There, you cannot tele-operate: the rover must run autonomously. Plan for a stretch you can't directly control." },
        #{ anchor: "panel.right_inspector", title: "Match the rover",       body: "Reading the ground is half of it — the other half is knowing your rover. Select it and check which variant you have; the safe slope and climb power differ between Explorer LT, Standard and Hauler. A route that's fine for one may tip another." },
        #{ anchor: "",                   title: "Now feel a slope",         body: "You can spot slope, shadow and radio-shadow. Next lesson you'll actually climb a borderline grade — and feel exactly why a top-heavy rover tips." },
    ]
}

fn show(s, i) {
    let step = s[i];
    coach(i, s.len(), step.anchor, step.title, step.body);
}

fn on_start(me) {
    load_scene("tutorials/school/traverse.usda");
    this.i = 0;
    notify_kind("Basic 2/5 · Reading the Terrain", "info");
    show(steps(), 0);
}

fn on_event(me, evt) {
    let s = steps();
    if evt.name == "cmd:TutorialNext" { this.i += 1; }
    else if evt.name == "cmd:TutorialBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:TutorialSkip" { this.i = s.len(); }
    else { return; }

    if this.i >= s.len() {
        end_tour();
        notify_kind("Terrain read — on to Slopes & Tip-Over", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        show(s, this.i);
    }
}
