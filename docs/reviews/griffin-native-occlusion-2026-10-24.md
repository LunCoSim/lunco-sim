# Griffin native occlusion qualification

Bevy's native occlusion culling does not establish a graphics improvement for
the landed Griffin view. No application rendering change is retained.

The regular production build temporarily registered Bevy's reflected
`OcclusionCulling` component. The existing Rhai component API added and removed
it on the active window camera, resolved through its exact USD prim path.
The camera already had the native depth prepass. No camera, graphics-quality,
physics, timestep, or authored-source settings changed between phases.

Owned PID 1307879/API 5209 ran source revision
`78d308e4e4fa97ce3d0b07d3b65437f3c95a4de5`. Griffin completed its touchdown
and four-pad handoff before measurement. Each phase waited five seconds, then
measured at least ten seconds and 240 new coupled ticks, requiring scene and
terrain readiness throughout. The camera pose is identical in all three phases.

| Culling | Fresh frames | Median frame ms | p95 frame ms |
|---|---:|---:|---:|
| Off, before | 405 | 23.695 | 29.726 |
| On | 400 | 24.212 | 30.717 |
| Off, after | 424 | 21.789 | 33.824 |

The inspected images retain the lander, terrain and shadow. Final registered
diagnostics contain zero errors and warnings; source hashes are unchanged. The
owned process exits through the API and releases its port. Another Tesla
session, PID 1305842/API 4499, consumed about 299 percent CPU during qualification.
These are contention-affected application-frame measurements, not isolated GPU
pass times or clean performance acceptance. They do not justify enabling
occlusion culling by default. The temporary reflection registration is removed.

An earlier trial on API 5208 failed its camera-presence assertion before toggling
culling. It is excluded from qualification. SceneCameraAudit and the exact USD
path identify the production camera; an unregistered reflected component read
does not establish that camera's absence.

Receipts, pressure samples, logs and images remain ignored under
`.cache/codex-evidence/performance-20261023/griffin-native-occlusion-confirmed/`.
All original loading, coupled-tick and graphics targets remain open.
