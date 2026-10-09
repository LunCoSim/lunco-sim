# Wheel trail contact and terrain review

The contract and ownership are defined in
[USD-driven visuals](../architecture/50-usd-driven-visuals.md#motion-trails-are-bounded-physics-history).
Avian/mobility provide solved contacts and tire widths. The shared viewport and
recorder plugin retains bounded f64 contact history; airborne/inverted support
ends the stroke, and landing starts a disconnected one.

The producer assigns stable point/segment identities and coalesces only changed
head, appended and retired segments. DEM history is neither flattened nor hashed
for a movement update. Terrain-frame conversion consumes only those deltas.
An unavailable frame holds the affected source, reports its diagnostic through
`InspectVehicleTrail` and requires a frame reset before admission resumes.
Non-DEM supports retain contact-plane meshes.

Hook candidacy: contact math, history bounds and spatial/GPU updates are generic
continuous presentation mechanisms. Their typed inputs come from engine owners;
Rhai owns scenario verdicts and route policy. No changeable Twin decision is
introduced in Rust. Lifecycle is the active physics frame plus `SceneTeardown`.

Verification uses the production `vehicle_trail_contact.rhai` gate for motion,
width, bounded producer edits, retained publication, airborne/inverted rejection
and disconnected landing. Pure index rollback and local-work verification are
recorded in [the incremental review](incremental-surface-annotations-review.md).
