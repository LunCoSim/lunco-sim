//! SS1 — The Site (Space School track).
//!
//! The mission briefing. A coach tour over the event traverse that frames the
//! specific task the teams run: start NW, reach the Point of Interest on the low
//! centre ground, come back — across a site with borderline slopes, deep shadow,
//! and a radio-shadow floor where the rover must run autonomously. Sets up the
//! three lessons that follow (route, radio-shadow, the scored run).
//!
//! Loads the placeholder traverse today; the real DEM relief (offline GeoTIFF,
//! task §3.1) drops in as a dem-layer twin without changing this script.
//! NEEDS (for real relief): the event DEM terrain site.

fn steps() {
    [
        #{ anchor: "",                title: "Your mission",        body: "This is the site your team will be scored on. The job is simple to state and hard to do: from the NW corner, reach the Point of Interest on the low centre ground, and return — without tipping, without stalling on a slope, and through a stretch where you lose the link home." },
        #{ anchor: "panel.center",    title: "Start and goal",      body: "You start at the NW corner. The POI marker sits at the centre, on the low ground. A full run is there AND back — returning is half the score." },
        #{ anchor: "panel.center",    title: "The hazard: slope",   body: "The way down to the centre crosses borderline grades — near the limit of what a rover can climb, and enough to tip a top-heavy one. The remote-sensing (ДЗЗ) team gives you a slope map and a route that stays inside the safe angle. Your job is to fly that route without exceeding your rover." },
        #{ anchor: "panel.center",    title: "The hazard: shadow",  body: "The SE highland and the low ESE sun throw deep shadow over the centre. Hard to see; and in the bonus rounds, cold and battery-hungry. Note where the shade lies." },
        #{ anchor: "panel.center",    title: "The hazard: radio",   body: "Earth is low on the E horizon, so the low centre near the POI is in radio-shadow — no link home. There you cannot tele-operate: the rover runs autonomously until it climbs back into line of sight. Plan for a stretch you can't directly control." },
        #{ anchor: "",                title: "Three lessons",       body: "Next you'll: import and fly the ДЗЗ route to the POI and back; handle the radio-shadow hand-off to autonomy; then put it together in the scored run, on three rover tiers. Let's go." },
    ]
}

fn show(s, i) {
    let step = s[i];
    coach(i, s.len(), step.anchor, step.title, step.body);
}

fn on_start(me) {
    load_scene("tutorials/school/traverse.usda");
    this.i = 0;
    notify_kind("Space School 1/4 · The Site", "info");
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
        notify_kind("Briefed — on to the route", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        show(s, this.i);
    }
}
