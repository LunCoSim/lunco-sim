//! Build a Lunar Base — a coaching mission over the spawn palette.
//!
//! ## What it teaches
//! Composing a working surface base from the moonbase **Structures** kit
//! (extracted USD parts under `assets/structures/`), and explaining the design
//! role of each part. The training plot is flat and contains no resource,
//! radio-shadow, terrain-height, or plume-ejecta model, so the completion check
//! verifies the five catalog identities only. The placement advice is a design
//! discussion, not a hidden geometric pass condition.
//!
//! ## One source, event-driven
//! The lesson IS this script. Its curriculum payload provides the empty build
//! plot (`build_base.usda`); the declarative `mission(me)` below advances on
//! REAL spawns — an entity of each kind actually appearing in the world — never
//! on a timer. The engine auto-publishes the checklist to the HUD and emits
//! `MISSION_COMPLETE` when every objective is done.
//!
//! ## Why detection is a world-state read, not an event latch
//! `requires_event: "cmd:SpawnEntity"` would tick on ANY spawn — dropping a
//! second solar tower would satisfy "place the habitat". So each objective asks
//! the question it actually means ("is a habitat present?") by scanning the live
//! entity list for the part's catalog ID. Display names are presentation text
//! and may change without changing what was spawned, so the lesson never
//! infers identity from them.

/// True if any spawned catalog root carries the exact content identity.
fn has_structure(entry_id) {
    for e in list_entities() {
        if e.catalog_id == entry_id { return true; }
    }
    false
}

fn power_present(me) {
    has_structure("solar_tower") || has_structure("fsps")
}

fn power_placed(me) {
    clear_spotlight();
    hint("Power added. Solar Tower supplies daylight power; FSPS covers the lunar night.\n\nNow add the crew Habitat.");
}

fn habitat_present(me) {
    has_structure("habitat_fsh")
}

fn comms_present(me) {
    has_structure("comms_mast")
}

fn isru_present(me) {
    has_structure("isru_plant")
}

fn pad_present(me) {
    has_structure("landing_pad")
}

fn habitat_placed(me) {
    hint("Habitat added. Place it where the other systems can service the crew.\n\nNow add a Comms Mast.");
}

fn comms_placed(me) {
    hint("Comms added. On real terrain, check elevation and line of sight.\n\nNow add the ISRU plant.");
}

fn isru_placed(me) {
    hint("ISRU added. On a resource-enabled base, place it near the resource.\n\nLast, add the Landing Pad.");
}

fn pad_placed(me) {
    notify_kind("Base components assembled — now review the layout as a design exercise.", "success");
}

fn on_start(me) {
    // The world this lesson teaches in is DECLARED in the curriculum layer
    // (a `payload` arc) and mounted by the launcher before this runs.
    notify_kind("Tutorial: Build a Lunar Base", "info");
    hint("Open Spawn in the right dock. Open Structures and add the requested part.\n\nDrag its gizmo to place it; use " + input_hint("look_button") + " to look around.");
    focus_panel("spawn_palette");
    spotlight("panel.spawn_palette", "Open Spawn, then choose a part from Structures.");
}

fn mission(me) {
    [
        // 1 — POWER FIRST. Nothing else runs without it. Either source counts:
        // the solar tower for daylight watts, or the fission plant (FSPS) for
        // the 350-hour lunar night.
        objective("power", #{
            text: "Open Structures and place a Solar Tower or FSPS",
            done: Fn("power_present"),
            on_complete: Fn("power_placed"),
        }),
        // 2 — CREW HABITAT. Identify the crew module in the catalog.
        objective("habitat", #{
            text: "Open Structures and add the crew Habitat",
            done: Fn("habitat_present"),
            on_complete: Fn("habitat_placed"),
        }),
        // 3 — COMMS. Discuss elevation and line-of-sight as a design concern;
        // this flat payload does not validate either one.
        objective("comms", #{
            text: "Open Structures and add a Comms Mast",
            done: Fn("comms_present"),
            on_complete: Fn("comms_placed"),
        }),
        // 4 — ISRU. Discuss resource and logistics placement; this payload has no
        // resource field or distance predicate.
        objective("isru", #{
            text: "Open Structures and add the ISRU plant",
            done: Fn("isru_present"),
            on_complete: Fn("isru_placed"),
        }),
        // 5 — LANDING PAD. Discuss plume clearance; this payload does not model it.
        objective("pad", #{
            text: "Open Structures and add the Landing Pad",
            done: Fn("pad_present"),
            on_complete: Fn("pad_placed"),
        }),
    ]
}

fn on_event(me, evt) {
    // The engine emits MISSION_COMPLETE once every objective is done.
    if evt.name == "MISSION_COMPLETE" {
        clear_spotlight();
        hint("Your base has power, habitat, comms, ISRU, and a landing pad.\n\nReview each part in Inspector and adjust the layout for your mission.");
        notify_kind("Build a Lunar Base complete", "success");
    }
}

// UNATTENDED self-completion — runs alongside the mission for CI/regression (the
// "if unattended, autopilot" pattern; see lander_mission.rhai). With no window it
// spawns the required catalog identities in an illustrative layout, satisfying
// every objective WITHOUT a click. With a window
// open it returns () and a human builds the base.
fn spawn_part(id, x, y, z) {
    cmd("SpawnEntity", #{ entry_id: id, position: [x, y, z] });
    notify_kind(`[debug] placed ${id}`, "info");
}

fn task(me) {
    if !is_unattended() { return; }
    seq([
        // Illustrative layout only: the lesson checks catalog identity, not siting.
        once(|m| spawn_part("solar_tower", 20.0, 0.0, 0.0)),
        wait(0.5),
        once(|m| spawn_part("habitat_fsh", 0.0, 0.0, 0.0)),
        wait(0.5),
        // Illustrative offsets for a readable layout; no terrain/resource
        // predicate is asserted by this training scene.
        once(|m| spawn_part("comms_mast", -18.0, 0.0, 22.0)),
        wait(0.5),
        once(|m| spawn_part("isru_plant", -32.0, 0.0, -26.0)),
        wait(0.5),
        // Another power option in an illustrative location.
        once(|m| spawn_part("fsps", 60.0, 0.0, -45.0)),
        wait(0.5),
        // Illustrative offset; no plume-clearance predicate is asserted here.
        once(|m| spawn_part("landing_pad", 0.0, 0.0, 180.0)),
        wait(1.5),
    ])
}
