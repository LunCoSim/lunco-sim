# Avatar input and terrain movement performance

Status: pointer dispatch and idle-click costs are corrected; remaining terrain
movement spikes require unprofiled acceptance without competing workloads.

## Ownership

The generic scene pointer adapter owns document-scoped subscriptions and
coalesces raw movement before resolving coordinates or terrain. Authored Rhai
owns registration, placement, and cancellation. An idle document sends no
movement hooks; an idle primary click performs no route queries. This bound
does not depend on the model's hierarchy size. Active preview transforms update
the render entity without authoring USD. Placement performs the document edit.
The complete contract is in [Rhai integration](../architecture/rhai-integration.md)
and [waypoints](../architecture/waypoints-in-usd.md).

The controller's native automation boundary emits absolute cursor events and
relative mouse motion as distinct streams. Camera look uses the existing input
map and configured look button. The drivers in `scripts/perf/` measure actual
avatar displacement and yaw through the production API.

## Evidence from September 30

Exact model: Summer Space School `sim/scenes/traverse_apollo15.usda`, High quality,
normal shadows and substeps, no vsync or throttling. All sessions were launched
from the optimization checkout on owned explicit API and Tracy ports.

| Measurement | Evidence | Result |
| --- | --- | --- |
| Idle primary-click hook before correction | `target/perf/sss-calibrated-click-before.tracy` | 24 handler calls; mean 100.90 ms, max 188.32 ms |
| Idle primary-click hook after correction | `target/perf/sss-calibrated-click-after.tracy` | 19 handler calls; mean 1.25 ms, max 5.11 ms |
| Scene revisions during movement and forced camera rotation | `target/perf/sss-motion-click-isolated-before-20260930.tracy` and input log | Document 13 and stage 2 stayed unchanged; passive input ran no pointer-move Rhai hooks |
| Native movement and rotation after 30 seconds of warmup | `target/perf/sss-native-warm-motion.tracy` and input log | 232.15 m movement; measured 360-degree native mouse turn |
| GPU submission in that warmed movement / rotation window | `target/perf/sss-native-warm-submit.csv` | Maximum 3.94 / 2.81 ms |
| Largest warmed movement Main schedule | `target/perf/sss-native-warm-schedules.csv` | 77.93 ms; includes 44.21 ms for three fixed ticks, 11.77 ms Update, and 13.97 ms PostUpdate |

The click captures have different numbers of admitted handler calls; they do
not establish a matching 24-click frame comparison. Tracy numbers are diagnostic
CPU spans, not product FPS. The warmed run had other simulator sessions and a
sibling build active. Its early revision query preceded full scene projection,
so that before/after pair is not movement-rebuild evidence.

The production editor `route_interaction` gate passes 36 checks, including
native look, invalid raw-motion and subscription rejection, idle clicks, coalesced subscribed
preview, stable preview document generation, placement, delete/undo, and ribbon
states. `route_lifecycle` passes 125 checks. Both touched route tests use named
behavior-tree actions and sequencing.

## Remaining work

The native-input unprofiled run (`target/perf/sss-native-unprofiled.summary.json`)
observed 355 movement frames: p50 13.32 ms, p99 24.40 ms, maximum 52.77 ms,
and three samples over 40 ms. Its 183 rotation samples stayed below 26.52 ms.
The avatar moved 228.81 m and turned 360 degrees; document 12 and stage 2 stayed
unchanged throughout. Physics at the three slow movement samples was 0.29–0.35 ms.
Another simulator was active, and polling did not observe every frame. Remaining
movement spikes need attribution and uncontended acceptance before claiming the
goal is met. Preserve simulation ticks, rendering quality, and terrain detail.
