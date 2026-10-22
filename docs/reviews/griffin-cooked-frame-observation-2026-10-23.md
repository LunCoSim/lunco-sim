# Cooked contact dimensions in their native frame

The corrected collision query reports canonical-stage bounds. Griffin's
GRR-006 observer compared those rotated AABB dimensions with section-local
length and thickness. That produced 24 failures without a geometry change.
The observation now uses the native rigid cooking pose to recover dimensions
from the actual cooked vertices in the scaled cooking frame.

`lunco-scene-queries::usd_prim_query::collider_part_api_value` adds the existing
`collider_pose_api_value` representation to cooked convex hulls and triangle
meshes, as already provided for analytic shapes. Vertices remain in canonical
stage metres. `local_to_stage` carries f64 translation and scalar-first
quaternion rotation. It removes placement and orientation when inverted;
the scale and unit conversion already applied to cooked geometry remain.
The same canonical USD matrix reader and cooker supply both facts.

This is query metadata. It changes no runtime collider, body state, solver,
dependency, gain, mass, force, timestep or tolerance. Rust reports the existing
typed native geometry and pose through the query boundary; the Twin's Rhai
observer selects source dimensions and binds its SysML acceptance rule.
No changeable lifecycle or selection decision was added, so no policy hook
is needed. There is no new transform walker or renderer-pose reader.

The production `assembly_collision_clearance_builder` observer expands from
19 to 21 checks. Its two new translated/rotated hull-frame assertions fail on
binary `9b28c4be8f9e77e8bd1fe8e5ee6f21db4e83afbe2d3d9153cf0c10c64d1579eb`.
The normal rebuilt binary
`40ab82cb16ef016fd475d227e0739d301cd9769a28fc32dc0357d882e2200a7a`
passes all 21 checks and the existing expected, non-crashing malformed-collider
fault. Receipts are `cook-frame-baseline.log` and `cook-frame-repaired.log`
under `.cache/codex-evidence/griffin-root-section-20261022/` in this worktree.

Griffin's unchanged installed geometry now passes its ramp requirement gate:
1,257 checks. The three axis comparisons on each of 12 tracks are consolidated
into one formal cooked-dimension, placement and transform-contract check per
track, reducing the count from 1,281 by 24. All three dimensions retain the
source's 0.002 m limit; placement and exact local unit-scale/order checks are
also required. The native f64 pose is used directly, without a projection
through the renderer's f32 topology transform. Static geometry reads use an
explicit document owner, so static colliders do not need a dynamic physics
frame. The separate existing flight-stow gate passes 272 checks.

The detached longer-root component passes 119 checks. A fixture shortened by
an instance X scale of 0.8 must fail its physical-span constraint even though
its local mesh fields retain the correct dimensions. Component and fixture
USD preflight pass; typed provenance covers 222 requirements with 222 records
and 71 sources. The model retains 153 existing SysML warnings, with no new
root-section warnings.

These are source/component gates. The new root is not mounted in the vehicle;
middle/toe geometry, toe hinge stock and sweep, full installation clearance,
loaded deployment and clean 60 FPS real-time acceptance remain open. The
known transition interference is still present in the installed equal-length
mechanism. Native physics defects remain assigned to optimization.
