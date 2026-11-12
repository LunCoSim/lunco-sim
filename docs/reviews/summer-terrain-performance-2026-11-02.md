# Summer terrain correctness and performance qualification

The current terrain and camera contracts are recorded in
[the camera terrain review](2026-11-12-camera-terrain-architecture.md).
Native physics retains its authored collider lattice. Each consumed camera owns
its projected visual cover, independently of rover support residency.

## Verification ownership

`scripts/api/test_terrain_contact_visual.py` checks independent physical support,
camera cover readiness, and terrain-bound route/trail registration through authored
Rhai verdicts. The camera motion and simultaneous-view drivers qualify camera
clearance, cover publication and independent mounted outputs. Their snapshots and
images require production inspection; frame timings do not establish correctness.

`scripts/api/test_physical_presentation_time.py --windowed` runs the existing
13-check authored clock contract through the native host. Its default mode
uses the production scene-test host. Pause, manual steps, fractional and
accelerated rates, fixed quanta, epoch mapping and celestial clocks retain
one authoritative test. The native deadline allows paused unfocused frames;
it is a liveness bound rather than a frame-time gate.

## Performance boundary

The camera and terrain correctness gates do not establish a frame-time target.
Qualify performance separately with the production scene, unchanged authored
quality and physics cadence, and the intended concurrent workloads. Use the adjacent Tracy collector and a separate uninstrumented acceptance run.
Record actual output dimensions, camera poses, source readiness, and work budgets
so a quality or sampling change cannot be reported as a scheduling improvement.
