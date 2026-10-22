# Plume opaque occlusion review — 2026-10-19

A leg inside a plume bound must hide gas behind its surface while retaining gas
in front. Proxy-face depth testing cannot enforce that interval. The renderer
now supplies Bevy's native depth prepass to source-declared volume consumers;
WESL reconstructs the opaque surface and clips each integration interval there.
An inside-volume camera also needs the proxy to bypass surface depth rejection.
The bound does not write depth.

`lunco-render-bevy` owns native camera resources and pipeline state. The existing
shader capability parser and reflected material schema carry the requirement;
`plume.wesl` is the real consumer. No new USD schema, custom depth target,
render graph, dependency patch, Modelica equation, or Rhai visibility driver is
needed. Rhai policy hooks are inappropriate for native render-resource lifetime
and per-fragment integration. Source, mesh/material presence and camera changes
invalidate one reconciliation system; scalar uniform changes do not.
Only automatically supplied prepasses are removed. Pre-existing explicit
prepasses remain owned by their caller. Demand is derived from live mesh
consumers rather than stored across scene replacement.

The supported optical behavior is optically thin additive emission clipped by
opaque and alpha-masked prepass geometry. Transparent extinction and plume-solid
physical interaction are separate models. Authored volume bounds are double
sided. Lens bloom remains a camera postprocess and may spread across a silhouette.

## Production evidence

The regression scene was authored and saved through the USD document API, then
read back. It uses the production plume shader with a uniform positive field
and a movable opaque bar. `scripts/api/test_plume_depth.py` controls the real
active camera, verifies its actual pose, and captures a visible window.
Bloom is disabled only in this geometric pixel test.

- The unchanged baseline shader failed: an embedded occluder retained the same
  central red signal as the unoccluded field (0.0517429), proving hidden gas was
  still integrated. The updated shader passes that assertion.
- Perspective and orthographic projections each pass foreground, embedded,
  background and inside-volume cases. With the bright regression field, the
  central red signals are respectively 0, 0.592157, 0.917647 and 0.552941.
- High quality (native MSAA x2) and low quality (MSAA off) each pass three
  authored native-resource assertions and eight image assertions.
- The renderer lifecycle unit passes source validity changes, draw/material
  removal, mesh removal, camera creation and camera-type removal, consumer
  despawn and preservation of pre-existing explicit prepasses.
- The rebuilt optimization production binary passes the unchanged Griffin
  engine-out gate (20 checks), nominal/partial/off main engine and RCS on/off
  observations. Fresh main and RCS captures retain authored bloom. The main
  view shows the foreground landing leg and foot as solid silhouettes against
  the emitting plume; the camera pose was verified through the live API.
- GPU compilation/validation logs are clean; each owned session exits through
  API Exit and releases port 4187. User port 3743 was not controlled.
- Focused Rust test/build, Python syntax checks and the skill catalogue check
  pass. No physics or Modelica source changed in this repair.

Logs, numerical results and inspected images are retained locally under
`/tmp/griffin-plume-depth-20261019`. The runtime build used optimization's
existing target and the task checkout's explicit asset root. Both the renderer
binary and WESL must be updated together; an asset-only reload cannot install
the new pipeline/resource behavior.
