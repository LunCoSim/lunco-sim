# Route surface annotation review

Terrain fragments own the rendered surface; route policy supplies ordered f64
points and width through `UpdateUsdCurveView`. The disposable USD curve retains
its authored identity and pointer policy while its independent mesh is hidden
on DEM terrain. No presentation update edits an authored USD layer.

The contract is defined in
[USD-driven visuals](../architecture/50-usd-driven-visuals.md#route-ribbons-are-derived-annotations).
`lunco-luncosim-edit-ui` prepares and validates the request frame, retains
unchanged leg identities and sends segment edits to the terrain index owner.
Coverage, shader compatibility, bounded allocation and checked GPU narrowing
are mechanism invariants. Rhai owns route selection and gesture meaning.

The production `route_surface_annotation.rhai` gate pauses an explicit view
writer and verifies long-path publication, retained texture identity, bounded
local index/GPU updates, missing-coverage rejection, unchanged authored waypoints
and restoration of writer state. `route_interaction` owns the editing contract.
See [the incremental review](incremental-surface-annotations-review.md) for the
index/render boundary and current evidence.
