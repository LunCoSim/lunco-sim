# Runtime UI capture visibility

The retained UI owner, `lunco-workbench-runtime-ui`, supplies candidate surface
IDs to the existing `runtime.ui.recording` hook. The hook remains the active
Twin's policy for choosing required surfaces. Its input is the typed manifest
ID list, the eligible visible ID list and recording activity; its output is
an array of required IDs. Unknown, hidden, duplicate and malformed selections
remain explicit errors.

Mounting, retained rendering and capture candidacy use one authored visibility
predicate: exposure visibility, perspective filter, named gate and active Twin
setting. Layout, gate, workspace and manifest changes invalidate the capture
contract independently of exposure revisions. Eligibility does not depend on
mounting, layout completion or render acknowledgement. Those remain readiness
fences for every required surface, so a visible HUD cannot be silently skipped
while its retained tree is preparing.

No hook, dependency, storage contract or Twin-specific Rust policy is added.
The policy keeps Application/Ui/Preparation context and remains optional in
ordinary interactive sessions. Stopping capture clears the required selection;
normal scene/Twin teardown retains its existing ownership.

The negative case is a surface with exposure visibility enabled but an authored
perspective filter excluding the current full-chart view. It must not become a
required hidden HUD and block recording. Returning to an allowed perspective
must require that HUD again without any exposure edit. Focused owner tests cover
that transition, closed gates, disabled settings and eligibility before mounting.
Production acceptance additionally requires a native Rhai-directed graph-only
take followed by a visible-HUD take, with clean sampled images and recorder
receipts. Source tests alone do not qualify that runtime boundary.

Validation on 24 October 2026: four focused owner tests and the normal debug
application build passed. The production Twin's Rhai director completed a
246-image launch with its HUD, a 60-image full-history view with that HUD hidden,
and a 60-image return to the visible HUD. The launch admitted one coupled 60 Hz
cycle per image; both holds kept their simulation tick unchanged. First and
last raw pictures from each take were reviewed without loading cards or hover
tooltips. The episode retains the exact binary hash, model input hashes and
receipts in `delivery/runtime_ui_visibility_review.json`. This qualifies the
visibility boundary, not the full film or a new renderer.
