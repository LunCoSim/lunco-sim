//! Build a Lunar Base — a coaching mission over the spawn palette.
//!
//! ## What it teaches
//! Composing a working surface base from the moonbase **Structures** kit
//! (extracted USD parts under `assets/structures/`), and — the real lesson —
//! WHERE each piece goes and why. Every objective is a siting rule made
//! physical: power first, crew habitat central, comms on high ground, ISRU by
//! the resource, and a landing pad set well back so descent plume can't
//! sandblast the base.
//!
//! ## One source, event-driven
//! The lesson IS this script. `on_start` loads the empty build plot
//! (`build_base.usda`); the declarative `mission(me)` below advances on REAL
//! spawns — an entity of each kind actually appearing in the world — never on a
//! timer. The engine auto-publishes the checklist to the HUD and emits
//! `MISSION_COMPLETE` when every objective is done.
//!
//! ## Why detection is a world-state read, not an event latch
//! `requires_event: "cmd:SpawnEntity"` would tick on ANY spawn — dropping a
//! second solar tower would satisfy "place the habitat". So each objective asks
//! the question it actually means ("is a habitat present?") by scanning the live
//! entity list for the part's name. The spawned root's Name is the catalog
//! display name (`habitat_fsh` → "Habitat Fsh", `solar_tower` → "Solar Tower"),
//! so a keyword match is stable however many parts the student places.

/// True if any entity in the world has `keyword` in its Name. The spawned root
/// of a catalog part carries its display name, so "Habitat" matches "Habitat
/// Fsh", "Solar" matches "Solar Tower", etc. Robust to duplicate placements.
fn has_structure(keyword) {
    for e in list_entities() {
        if e.name.contains(keyword) { return true; }
    }
    false
}

fn power_present(me) {
    has_structure("Solar") || has_structure("Fsps")
}

fn power_placed(me) {
    clear_spotlight();
    hint("Power down. Solar gives you daylight watts; a fission plant (FSPS) carries you through the long lunar night — and, being a reactor, it must sit well away from the crew for radiation standoff. Now the habitat.");
}

fn habitat_present(me) {
    has_structure("Habitat")
}

fn comms_present(me) {
    has_structure("Comms")
}

fn isru_present(me) {
    has_structure("Isru")
}

fn pad_present(me) {
    has_structure("Landing")
}

fn habitat_placed(me) {
    hint("Habitat placed. Keep it central and uphill of the landing pad; its radiators want a clear sky. Next, comms — your link home.");
}

fn comms_placed(me) {
    hint("Comms up. On a real base this goes on the highest ground you have — a ridge or crater rim — for Earth line-of-sight. Now the ISRU plant.");
}

fn isru_placed(me) {
    hint("ISRU placed. It wants to be near the resource (the cold-trap ice on a PSR-facing slope) yet within rover range of the pad and tanks. Last piece: the landing pad.");
}

fn pad_placed(me) {
    notify_kind("Base laid out — that's a working outpost!", "success");
}

fn on_start(me) {
    // The world this lesson teaches in is DECLARED in the curriculum layer
    // (a `payload` arc) and mounted by the launcher before this runs.
    notify_kind("Tutorial: Build a Lunar Base", "info");
    hint("Open the palette (bottom dock) and find the new Structures group. Pick a part and drop it onto the ground, then click-drag its gizmo to place it. Fly the camera with right-mouse + WASD.");
    spotlight("panel.bottom", "Your parts live here — the Structures group holds the habitat, power, comms, ISRU and the landing pad.");
}

fn mission(me) {
    [
        // 1 — POWER FIRST. Nothing else runs without it. Either source counts:
        // the solar tower for daylight watts, or the fission plant (FSPS) for
        // the 350-hour lunar night.
        objective("power", #{
            text: "Place a power source — a Solar Tower or a Fission plant (FSPS)",
            done: Fn("power_present"),
            on_complete: Fn("power_placed"),
        }),
        // 2 — CREW HABITAT. Central, so everything else lays out around it.
        objective("habitat", #{
            text: "Set down the crew Habitat, central to the base",
            done: Fn("habitat_present"),
            on_complete: Fn("habitat_placed"),
        }),
        // 3 — COMMS. Highest ground for Earth line-of-sight.
        objective("comms", #{
            text: "Raise a Comms Mast for the link back to Earth",
            done: Fn("comms_present"),
            on_complete: Fn("comms_placed"),
        }),
        // 4 — ISRU. By the resource, within rover reach of the pad and tanks.
        objective("isru", #{
            text: "Add an ISRU plant to mine water and propellant",
            done: Fn("isru_present"),
            on_complete: Fn("isru_placed"),
        }),
        // 5 — LANDING PAD. Set well back — plume ejecta is the hazard.
        objective("pad", #{
            text: "Lay a Landing Pad, set well back from the base",
            done: Fn("pad_present"),
            on_complete: Fn("pad_placed"),
        }),
    ]
}

fn on_event(me, evt) {
    // The engine emits MISSION_COMPLETE once every objective is done.
    if evt.name == "MISSION_COMPLETE" {
        clear_spotlight();
        hint("Your base is up: power, habitat, comms, ISRU and a pad. Keep building — add the second power source, park a rover, or open the Inspector to tune a part. Replay any lesson from the 🎓 Tutorials menu.");
        notify_kind("Build a Lunar Base complete 🎉", "success");
    }
}

// UNATTENDED self-completion — runs alongside the mission for CI/regression (the
// "if unattended, autopilot" pattern; see lander_mission.rhai). With no window it
// spawns the whole base itself, in a sensible layout that obeys the siting rules
// the lesson teaches, satisfying every objective WITHOUT a click. With a window
// open it returns () and a human builds the base.
fn spawn_part(id, x, y, z) {
    cmd("SpawnEntity", #{ entry_id: id, position: [x, y, z] });
    notify_kind(`[debug] placed ${id}`, "info");
}

fn task(me) {
    if !is_unattended() { return; }
    seq([
        // Power (open ground, near the habitat) + the crew module (central).
        once(|m| spawn_part("solar_tower", 20.0, 0.0, 0.0)),
        wait(0.5),
        once(|m| spawn_part("habitat_fsh", 0.0, 0.0, 0.0)),
        wait(0.5),
        // Comms offset onto "high ground"; ISRU out toward the resource.
        once(|m| spawn_part("comms_mast", -18.0, 0.0, 22.0)),
        wait(0.5),
        once(|m| spawn_part("isru_plant", -32.0, 0.0, -26.0)),
        wait(0.5),
        // Fission power at a radiation standoff, well off the habitat.
        once(|m| spawn_part("fsps", 60.0, 0.0, -45.0)),
        wait(0.5),
        // Landing pad ≥150 m down-sun so plume ejecta clears the base.
        once(|m| spawn_part("landing_pad", 0.0, 0.0, 180.0)),
        wait(1.5),
    ])
}
