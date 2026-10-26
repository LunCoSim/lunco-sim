# Roadster fan/skirt landing observation

Status: unaccepted physical picture; investigation handed off to optimization.
No physics code or model equations changed by the film task.

The canonical Twin was exercised in the regular tutorials debug binary, built
from `f736291e4` plus the recorded render/capture prototype. Binary SHA-256:
`7d3430befe8c4626cdc1919f3a8b97ce606a162c0879889ddd933256fd25b695`.
Per-take `source_inputs.json` records all model and scene source hashes; the
model checkout contains other ongoing changes. This is not a clean-commit-only
reproduction claim.

Film mode `fan_lift` starts at rest, turns the fans on at 1 s and requests lift
at 5 s. The normal valve input closes after measured upward velocity crosses
3.5 m/s (observed at 5.2667 s and 3.7806 m/s). The first hop peaks near 2.4 m.
The initial airborne pressure/power response is plausible, but landing produces
an unexpected much larger rebound. The later peak is 17.0822 m.

| Capture frame | Body Y (m) | Measured vertical speed (m/s) | Jet force Y (N) | Fan seal | Suction (N) | Fan power (kW) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 360 | 2.262 | -1.649 | 0 | 0 | 0 | 80 |
| 390 | 1.355 | 18.163 | 0 | 0 | 0 | 80 |
| 420 | 9.004 | 12.951 | 0 | 0 | 0 | 80 |
| 839 | 0.863 | -0.936 | 0 | 0 | 0 | 80 |

Inspect the contact/adhesion transition around frames 375–395. These partial
channels establish an unexpected trajectory, not a complete energy diagnosis.
The native frame/solver checks pass, but that is not physical or visual
acceptance. Both the motion take and its paused-analysis follow-up are marked
rejected for the film. Do not relax a verifier or crop away the rebound to
claim a clean completed demonstration.

Evidence is retained locally under
`/home/rod/Documents/lunco/lunco-marketing/campaigns/general/roadster_real_physics_90s/renders/sources/report_shared_fan_lift_v22/`:
`shot_01/take.mp4`, `telemetry.json`, `verification.json`, `source_inputs.json`
and `acceptance.json`. Full native log:
`/home/rod/.cache/roadster-shared-report-v18-session.log`.

Separately, `standing_roll` v20 is a film setup issue: the script selects the
normal course pilot, whose phase-2/3 schedule requests 35 m/s. Its old
`roadster.pilot.controls` subscription no longer matches continuous native
pilot coupling. Do not report that setup as a physics-engine failure. A proper
stationary guidance request is needed on the same physical plant.

The requested agent-only Astra relay to `optimization` was rejected because
that name did not match one registered team member. No message was delivered;
this in-tree review is the local handoff.
