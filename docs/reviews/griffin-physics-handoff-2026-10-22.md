# Griffin physics handoff — 2026-10-22

Owner: optimization worktree. User instruction: report physics errors here and continue Griffin authoring while physics is repaired. The Griffin task leaves Avian and its dependency pin unchanged; no dependency modification is authorized by this handoff.

## Reproduced loaded-support drift

The current USD production binary reproduces drift on a simple loaded slider without Griffin, flight control, terrain, or rover behavior. A separate unjointed contact body remains stationary on the same slope. The slider prevents relative rotation in pose while reporting substantial relative angular velocity.

- Execution checkout: `/home/rod/Documents/luncosim-workspace/usd`, HEAD `3844088aa` plus existing application changes.
- Binary: `/home/rod/Documents/luncosim-workspace/usd/target/debug/luncosim`, SHA256 `6d69f8af0be13fd0582f4dec3ad96ac5f9d52c0e71c42114f8858ecf0409f452`.
- Native Avian pin: `e4f073bb8914beb9fb0aa97795ae93d08a478528`.
- Scene: `/home/rod/Documents/luncosim-workspace/usd/assets/scenes/tests/loaded_slider_slope.usda` (authored through the production document API).
- Observer: `/home/rod/Documents/luncosim-workspace/usd/assets/scenarios/tests/loaded_slider_slope.rhai`.
- Scene SHA256: `86a8ba98c668442aab3f21c452f625562428bc6cac46b271a2623c556eff3217`.
- Observer SHA256: `1392ad11885533ee8c4635e576e97e50e4868215b3d3ae693fd7269920ede57a`.
- Evidence directory: `/home/rod/Documents/luncosim-workspace/usd/.cache/codex-evidence/griffin-contact-audit-20261022/`.

```sh
LUNCOSIM_EPHEMERAL_SETTINGS=1 ./target/debug/luncosim test \
  --scene assets/scenes/tests/loaded_slider_slope.usda \
  --max-ticks 500 --verdict-channel LOADED_SLIDER_SLOPE --api <free-owned-port>
```

Run from the USD checkout. `loaded-slider-slope.log` records terminal tick 360, exit 1, three passing checks and two failing checks. After one second settling, the observer samples five seconds at 60 Hz:

| Measurement | Result | Acceptance |
| --- | ---: | ---: |
| Unloaded control drift | 0 m | ≤ 0.002 m |
| Control final speed | 0.0000286 m/s | ≤ 0.001 m/s |
| Loaded body drift | 0.3670201381 m | ≤ 0.002 m |
| Maximum relative orientation change | 0.0000000730 rad | ≤ 0.0001 rad |
| Maximum relative angular velocity | 0.5323372837 rad/s | ≤ 0.001 rad/s |

The fixture uses lunar gravity 1.62 m/s², a 0.12 rad slope, static/dynamic friction 0.8 (well above tan(slope)), a 180 kg contact body coupled by a native prismatic joint to a 4000 kg body, and the standard eight substeps. The joint has coincident rest frames, force-drive stiffness 85000 N/m, damping 19000 N·s/m, cap 12000 N and limits [-0.05, 0.02] m. There is no control input. The control has the same contact geometry, friction and slope. Peak loaded penetration was approximately 0.007386 m; control penetration was 0.00000884 m.

## Griffin manifestation and release gates

`RunRhai.json`, `rest-state.json`, `leg-constraint-probe.rhai`, and `window.log` in the evidence directory retain the application observation. Post-landing main thrust was zero, requested RCS torque was [0,0,0], and landing handoff/cutoff were complete. A ten-second leg probe measured maximum leg/bus relative angular velocity 0.5447459141 rad/s with only 0.0000038055 rad relative orientation drift.

Current landing-stability gate fails ground speed (0.10715 versus 0.05 m/s), angular speed (0.01793 versus 0.01 rad/s) and sixty-second drift (2.16788 versus 0.35 m). Its finite-state, attitude and contact checks pass. Earlier evidence is retained under `/home/rod/Documents/luncosim-workspace/usd/.cache/codex-evidence/griffin-physics-followup-20261022/`.

Read-only schedule inspection suggests a joint/contact coupling issue: native XPBD joint correction and velocity projection occur after contact relaxation. Late joint load may not participate in the same friction solve. This is a diagnostic hypothesis, not a proven solver repair. Please investigate at the physics owner and qualify both the isolated fixture and Griffin's landing gate. Do not mask the error with pose/velocity writes, enlarged friction, altered mass, damping, weaker limits or fewer substeps.

The user-selected interactive target is 60 FPS at 1280 × 720, High quality and real-time physics. No clean qualifying full-mission window exists yet. Keep standard quality, fixed timestep and substeps when qualifying the repair; profiler timing alone is not FPS acceptance.

## Engine-off impact classification

The current sixty-metre engine-off fixture reaches about 14 m/s and roughly 500 kJ for the approximately 5390 kg vehicle. Four 12000 N drives through 0.07 m provide at most 3360 J of actuator work. This is an overload study, not a calibrated survivable landing. Its penetration, foot clearance, four-leg contact and hard-stop checks currently fail. A repair must distinguish solver/contact integrity from the physical overload classification; passing a settling assertion would not establish impact capacity.

## Separate shared guidance finding

The current `assets/models/LunCo/Mobility/RoverAutopilotGuidance.mo` skid/turn-in-place branch multiplies throttle by `approach_gate = clamp((distance-radius)/(2*radius),0,1)`. It tends to zero outside the arrival boundary, unlike the wheel branch's arrival envelope. Griffin's latest mission reaches the later route stage but stalls at distance 1.6764 m with throttle approximately 0.03148 (terminal tick 6881). This may be a shared guidance issue as well as the open support physics issue; it has not been isolated or repaired by the Griffin task. Do not describe the complete mission as currently passing.

The optimization worktree's existing dirty implementation and assets were preserved. This handoff adds only this report. Black diagnostic camera captures are not visual qualification; numerical fixture receipts and terminal verdicts are the evidence above.
