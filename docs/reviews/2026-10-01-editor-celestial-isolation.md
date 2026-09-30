# Editor celestial projection isolation

The georeferenced Griffin-FLIP scene rendered normally as a local vehicle asset
but became rotated and badly distorted in its Editor scene preview. Framing
selected a target near (-51650.75,-1725515.25,195171.97) metres. The preview root
already carried UsdPreviewOnly, but project_celestial_comms_prims admitted its
prim and descendants and projected domain placement into that hierarchy.

The projector now reuses lunco_usd_bevy_scene::is_preview_only before reading
or retrying any queued prim. Render-only stages retain canonical local geometry;
ordinary live stage work is still admitted. No new policy hook, API, geometry
conversion, Twin special case or coordinate override was introduced.

The regression first failed with both preview entities and the live loading
entity queued, then passed with only the live entity queued. All ten tests in
lunco-usd-sim-celestial pass. Logs:
/tmp/griffin-preview-isolation-red.log and
/tmp/griffin-preview-isolation-green.log.

Built the production app from terrain-streaming with the shared existing target:
CARGO_TARGET_DIR=/home/rod/Documents/luncosim-workspace/optimization/target
cargo build -p lunco-luncosim --bin luncosim --features tracy -j4
Build passed; /tmp/griffin-preview-isolation-build.log. Tracy is not FPS evidence.

Restarted only the owned API49746 process after all USD owners reported saved.
Opened the Twin and exact scenes/griffin_flip_visual.usda in the fresh Editor:
document115591551527336, view8, projected generation0 ready,50 recipe layers.
The unchanged rear preset at target(0,1.65,0) now displays correct Griffin and
FLIP geometry. /tmp/griffin-flip-isolated-preview.png. Selection framing now
returns a local target approximately(0,.26218,0), not lunar-radius coordinates.
The full review remains focused. No USD geometry compensations were required.

Production GRIFFIN_FLIP_VISUAL fixture passed113checks at8ticks,60Hz,one thread,
seed6840157149251759617, from terrain cwd with the rebuilt binary:
/tmp/griffin-preview-isolation-visual-gate.log. This establishes visual
composition and live projection admission, not landing/FLIP egress acceptance.

Worktree terrain, branch terrain-streaming, initial HEAD6186c437b was clean.
Official Trello tools were unavailable and the tracking blocker was reported
before the non-trivial edit; no external messages or board writes occurred.
