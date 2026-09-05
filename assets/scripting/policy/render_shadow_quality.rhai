// ── Shadow-resource warning policy ─────────────────────────────────────────
//
// Rust publishes renderer facts. This policy decides whether configured shadow
// limits are unmet and owns the user-facing warning. It never changes a light,
// map size, cascade count, or quality setting.
//
// ctx = #{ directional_casters, point_casters, spot_casters,
//          max_directional_shadow_casters, max_point_shadow_casters,
//          max_spot_shadow_casters, estimated_bytes, budget_bytes }
// Return () when the facts are within the configured limits, otherwise return
// the warning text to publish once through the renderer status bus.
fn shadow_quality(ctx) {
    let reasons = [];
    if ctx.directional_casters > ctx.max_directional_shadow_casters {
        reasons.push(ctx.directional_casters.to_string() + " directional caster(s) exceed the configured limit of "
            + ctx.max_directional_shadow_casters.to_string());
    }
    if ctx.point_casters > ctx.max_point_shadow_casters {
        reasons.push(ctx.point_casters.to_string() + " point caster(s) exceed the configured limit of "
            + ctx.max_point_shadow_casters.to_string());
    }
    if ctx.spot_casters > ctx.max_spot_shadow_casters {
        reasons.push(ctx.spot_casters.to_string() + " spot caster(s) exceed the configured limit of "
            + ctx.max_spot_shadow_casters.to_string());
    }
    if ctx.estimated_bytes > ctx.budget_bytes {
        reasons.push("the live shadow estimate of " + ctx.estimated_bytes.to_string()
            + " bytes exceeds the explicit ceiling of " + ctx.budget_bytes.to_string() + " bytes");
    }
    if reasons.len() == 0 {
        return ();
    }
    let detail = "";
    for reason in reasons {
        if detail != "" {
            detail += "; ";
        }
        detail += reason;
    }
    "Shadow configuration is unmet: " + detail
        + ". All authored shadow maps remain enabled; no automatic quality change was applied."
}
