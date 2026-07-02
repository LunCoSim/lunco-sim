// ── Control-authority takeover policy (spec 034) ─────────────────────────────
//
// Decides who may take a vessel from its CURRENT owner. Registered under
// `lunco_core::session::CONTROL_AUTHORITY_HOOK` and consulted by the possession
// path when a `PossessVessel` targets a vessel another session already owns.
// Returning `true` releases the prior owner so the taker's claim succeeds (even
// under the default Exclusive possession policy); `false` denies the takeover.
//
// This is DATA, not Rust — tune the control hand-off rules here without a rebuild.
// `ctx` is a map: #{ taker, taker_role, owner, owner_role, target }, where the
// *_role fields are the `AuthorityRole` names ("Owner" | "Operator" | "AiAgent" |
// "Observer" | "None").

fn may_take_control(ctx) {
    // An autopilot (AiAgent) yields to a human on takeover: a human actor may take
    // a vessel an autopilot is driving.
    if ctx.owner_role == "AiAgent" {
        return ctx.taker_role != "AiAgent";
    }

    // A vessel a human holds is NOT stealable by an autopilot, and (by default) not
    // by another human either — humans coordinate via explicit release. Deny.
    false
}
