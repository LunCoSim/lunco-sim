// Additional control-claim admission. Normal interactive sessions allow claims;
// a Twin or deployment can reserve its experiment by returning false here.
// Ownership and takeover floors are enforced independently by the session owner.
fn allow_control_claim(ctx) { true }
