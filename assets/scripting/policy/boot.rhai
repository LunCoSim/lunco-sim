// ── Boot-entry policy ────────────────────────────────────────────────────────
//
// Decides what an app does at startup. Registered under `lunco_core::BOOT_HOOK`
// and consulted ONCE by each app's boot seam (`lunco_tutorial::consult_boot`).
//
// This is a PURE decision: read `ctx`, return either
//   #{ command: "<Name>", params: #{ ... } }   → the seam dispatches that command
//   ()                                          → take no action; the app loads its default
//
// So the whole "what shows on launch" policy is DATA, not Rust — rewrite it here
// (or hot-replace by id) with no rebuild. `ctx` is:
//   #{ onboarded, first_start_id, has_scene_arg, automated }
//     onboarded      : bool  — has first-run onboarding already happened?
//     first_start_id : str   — the app's onboarding tutorial id (() if none)
//     has_scene_arg  : bool  — an explicit `--scene`/deep-link was given
//     automated      : bool  — headless / `--api` / `--no-ui` session

fn boot_entry(ctx) {
    // Explicit intent or automation → the app knows exactly what to load; don't
    // override. (This is what keeps `--scene X` / the headless server direct.)
    if ctx.has_scene_arg == true || ctx.automated == true {
        return ();
    }

    // First genuine interactive run → onboard with the app's first-start tutorial
    // (which loads its OWN environment). This is the "load the tutorial, not the
    // default scene" rule — one load, no race.
    if ctx.onboarded != true && ctx.first_start_id != () {
        return #{ command: "StartTutorial", params: #{ id: ctx.first_start_id } };
    }

    // Otherwise: nothing to override — the app loads its default (scene / Welcome).
    // Future branches (resume last session, showcase-of-the-day, deep-link) go
    // here as data, no Rust change.
    ()
}
