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


## Integrated packaging follow-up — 22 October 2026

The user requested a larger Griffin, more inward stowed ramps and correction of landing-gear/panel interference. Model commit `57b853580fbd1540c80a400a3f5a9a5bbf08c4e8` reconstructs a 4.50 m bus, compact FLIP and 120-degree root stow. Absolute dimensions and travel remain explicit reconstruction estimates. The geometry, collision profiles, body stations, hinge targets and fixed-latch anchors were reconciled through the production document API; no solver dependency, timestep, substep, friction, mass, spring cap or numerical acceptance tolerance was changed.

The final selected cohort passes sixteen cases: nine assembly/geometry cases, solar layout, five FLIP component cases and RCS runtime. Static native geometry inspection clears the stowed rover/ramps and panel/legs. These passes do not establish loaded touchdown, unfolding or full-mission performance. The portable model receipt is `twins/astrobotic-griffin-1/contracts/evidence/griffin-packaging-2026-10-22.json`, with 154 source hashes and the exact acceptance boundaries.

The final `GriffinPropulsionRequirements::Verify_GriffinPilotAttitude` run is **inconclusive**. Descent was armed at tick 1 with matching propagated assembly/guidance mass 5389.999761581421 kg. No pilot-trial samples or terminal authored verdict were emitted before the external harness terminated the process at 240 wall-clock seconds. A native clock observation during the run recorded tick 5534, admitted elapsed 92.23333333333333 simulation seconds, fixed dt 1/60 s and eight substeps. The observer was still awaiting settled touchdown. This is not a measured failure of its own 150-second simulation-time touchdown limit; the external fuse prevented a terminal verdict.

Reproduction from the USD checkout, using a free owned API port and a wall-clock allowance sufficient to reach the authored terminal verdict:

```sh
LUNCOSIM_EPHEMERAL_SETTINGS=1 LUNCO_ASSET_ROOT=/home/rod/Documents/luncosim-workspace/usd/assets \
  ./target/debug/luncosim test --api <free-owned-port> \
  --scene /home/rod/Documents/models/lunar-base-model/twins/astrobotic-griffin-1/tests/griffin_pilot_attitude.usda \
  --verification GriffinPropulsionRequirements::Verify_GriffinPilotAttitude \
  --source-revision 17798372683716093098
```

The final source revision is `17798372683716093098`; the binary SHA256 remains `6d69f8af0be13fd0582f4dec3ad96ac5f9d52c0e71c42114f8858ecf0409f452`. Final local logs and cohort receipts are under `/home/rod/Documents/luncosim-workspace/usd/.cache/codex-evidence/griffin-inward-stow-20261022/accepted-gates-{assembly,runtime,rcs}/`. The pilot log is `accepted-gates-runtime/griffin_pilot_attitude.log`. Both owned ports 4391 and 4393 were released after the checks.

An earlier wider/inward source (`1816606438305106943`, before final source-ownership and perimeter-collider reconciliation) produced a terminal pilot **FAIL** at tick 2584, 43.066667 simulation seconds. Eight of nine checks passed. The sole failure was ramp peak relative angular rate **4.276409768811721 rad/s** against **0.02 rad/s**; peak relative orientation error was only **0.001426457612820009 rad**, within **0.035 rad**. Other pilot bounds passed (minimum upright Y 0.9963134120821918; maximum hull angular speed 0.0857813692106911 rad/s). Evidence is `wide-gates/griffin_pilot_attitude.log` and `wide-gates/cohort.json` in the same directory. Do not present this earlier rate as a measurement on the final saved geometry. It is additional evidence for investigating the loaded constraint pose/rate discrepancy alongside the isolated slider, not proof of a common solver root cause.

Please retain the existing support-drift, impact and pilot acceptance limits while qualifying the physics repair. The Griffin task continues to leave backend repair with optimization. Whole-model mounted USD lint also reports nine unchanged solar direction-port/IMU gravity wiring findings; the model review records those separately and does not claim clean whole-model lint. This follow-up changes only this report and preserves optimization's dirty implementation files.

## IMU rotation-norm control after main synchronization

The USD worktree fast-forwarded to `a45cbca69f22427fb7a7d5f7652940c306efdb28` and rebuilt the normal production binary (SHA256 `7ac15fa0d9c5aafd0140be663b09b5e12ec1ec55fa2602106be065bdb712a6a3`). Its `body_acceleration` production gate fails 47 of 267 checks, all with the existing diagnostic `accelerometer magnitude must be invariant under body rotation`. The observer compares the norm of the three body-frame specific-force components with the reported proper-acceleration magnitude using the unchanged absolute limit `1.0e-9`. Supported, thrust and freefall phase checks pass.

This result also reproduces with the earlier binary (`6d69f8af0be13fd0582f4dec3ad96ac5f9d52c0e71c42114f8858ecf0409f452`) on optimization's untouched fixture, and on the USD fixture after removing its unused inactive `/SandboxScene/Box` branch. The active `/SandboxScene/Box50` body, mass, force phases and observer limits are unchanged. All three runs have the same ordered 47 failure messages. This establishes a pre-existing rotation/precision contract failure; it does not establish its mechanism or a common cause with the loaded slider.

Evidence under `/home/rod/Documents/luncosim-workspace/usd/.cache/codex-evidence/griffin-validation-20261022/`: `body-acceleration-baseline.log`, `body_acceleration-gate.log`, and `post-ff-gates/body_acceleration-gate.log`. Reproduce with the current normal binary using `luncosim test --api <free-owned-port> --scene assets/scenes/tests/body_acceleration.usda --verdict-channel BODY_ACCELERATION`. Please investigate at the physics/IMU owner without relaxing the rotation-norm limit. Initialization-policy, scene-transition-generation and standalone sensor gates pass on the rebuilt binary.

The separate Griffin interface cleanup now closes the former direction-alias and gravity-port lint errors through Rhai collection-aware policy and double gravity inputs. That preflight repair is not evidence that these physics gates are repaired. No solver, dependency, timestep, substep, friction, mass, spring cap or physics tolerance was changed by the Griffin task.

## Collision-query observation during internal FLIP packaging

The native document query has an inconsistent child bounds result worth retaining when investigating contact geometry. On FLIP document `119546362397782`, generation 151, stage generation 4, `/FLIP/Battery/Body` has geometry bounds min `[-0.842,0.158,-0.68]`, max `[0.842,0.358,0.68]` in canonical-stage metres. Its returned cooked convex hull occupies that allocation (vertex roundoff is about `1e-8` m). However, the same child query's `collision_bounds` reports the untransformed unit Cube at the origin and labels that result `canonical_stage`. Querying the parent `/FLIP/Battery` gives the expected transformed collision bounds. Receipt: `/home/rod/Documents/luncosim-workspace/usd/.cache/codex-evidence/griffin-validation-20261022/battery-final-readback.json`.

This is an observed query/frame discrepancy, not proof that runtime collision uses the wrong shape. The composed fixture was not spawned. The battery gate observes parent aggregate collision bounds and child visible bounds; the separate cooked vertices confirm the allocation. Keep shape, frame, and parent-transform provenance explicit when using these APIs to diagnose the loaded-support issue.

## IMU repair qualification — optimization

The optimization production run reproduced the same 47/267 failures before the
repair. `lunco-physics::kinematics::finish_interval` validated a native unit f32
quaternion, widened it, then used it as a unit f64 frame rotation. Native unit
tolerance does not imply unit length after widening. The measurement boundary
now normalizes the validated widened quaternion before transforming specific
force into the body frame; native body state and solver settings are unchanged.

The unchanged `body_acceleration` production scene now passes **267 checks**,
including the original **1e-9** norm limit, supported/thrust/freefall phases and
unavailable initial measurements (exit 0, terminal tick 175). Binary SHA256: `65ed1704dfca6220dcaaaff6dfc40439341978fefd7d30b58d6592da9f5ef898`.
Evidence: `.cache/codex-evidence/physics-followup-20261021/imu-roof-pause-integration/body-acceleration.log`;
the failing local baseline is `embedded-policy-trace/body-acceleration.log` in
the same evidence root. This closes the IMU rotation-norm finding; Griffin
landing, ramp unfolding and the sub-1-ms active-step target remain separate open gates.

USD also integrated the committed measurement repair without the worktree's
uncommitted solver/dependency changes. Its normal production binary SHA256 is
`0242a65cff83d1adce15b14a67e3f04aa3d69d760aa87f7e1fc19d048675e4e1`.
The original acceleration gate passes 267 checks, and sensor,
initialization-policy, scene-transition-generation, FLIP chassis and integrated
visual gates pass. Receipt:
`/home/rod/Documents/luncosim-workspace/usd/.cache/codex-evidence/griffin-materials-20261022/integration-gates.json`.
