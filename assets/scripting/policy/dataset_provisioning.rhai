// ── Dataset provisioning policy ────────────────────────────────────────────
//
// Rust supplies facts about the engine or opened Twin and its missing declared
// datasets.
// This policy decides whether the interactive consent window is appropriate;
// the user-facing command is still the only thing that authorises a download.
//
// ctx = #{ scope: "engine"|"twin", owner, interactive, show_on_start,
//          datasets: [#{ id, key, name, processed }] }
// return "prompt" to show the consent window, or "skip" to continue without
// network access.
fn dataset_provisioning(ctx) {
    if ctx.interactive != true {
        return "skip";
    }
    if ctx.show_on_start != true {
        return "skip";
    }
    "prompt"
}
