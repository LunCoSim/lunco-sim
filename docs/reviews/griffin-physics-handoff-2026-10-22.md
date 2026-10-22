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

## Collision-query repair qualification — optimization

The child-bound discrepancy reproduced independently on the shared Cylinder
fixture: the child reported collision bounds around X=0 while its geometry
bounds and single-shape parent correctly reported X=3. The collision-envelope
reader began with the selected prim's local transform instead of its composed
stage transform. It now uses the shared f64 USD world-transform reader and
honors descendant reset stacks. The duplicate geometry transform walker was
removed; geometry and cooked-collider placement use that same stage owner.
No runtime collider geometry or solver setting was changed by this repair.

The regular production build passes the expanded
`assembly_collision_clearance_builder` gate: 19 checks, including single/batch
child bounds through translated and rotated ancestors, followed by the expected
non-crashing `usd-avian-collider-invalid` negative fixture (exit 0, tick 2).
The same observer failed exactly the two added checks on the previous binary.
The reported FLIP source was also queried directly: `/FLIP/Battery/Body` now
agrees with both its geometry bounds and its parent, with min
`[-0.842, 0.158, -0.68]` and max `[0.842, 0.358, 0.68]` metres in canonical
stage coordinates. This is static query qualification, not physical or visual
acceptance of FLIP. Source hashes, coherent readback, baseline and passing logs
are under `.cache/codex-evidence/physics-followup-20261021/collision-child-frame/`.
Binary SHA256: `685886df6ecbca5cd58ce25ea6f40dae61c46b55f9e24eb647dbea54412355d9`.
All owned sessions used for this query check have exited.

## Loaded-support and mission qualification — optimization

The native Avian branch `fix-joint-velocity-projection`, published at
`9097ce92e5d236c747a68567368575014c28fff5`, couples hard Fixed, Prismatic and
Revolute joint velocities with native contact relaxation. It also corrects
physical contact-impulse accumulation and applies native joint/custom collision
exclusions during swept CCD. The isolated loaded-slider fixture passes all
seven checks, including the original drift/relative-rate limits and supported
weight reactions. Eight substeps and the authored mechanical parameters remain
unchanged. The broader production cohort and its exact source receipts are in
`prepared-velocity-integration/` under the same evidence root.

The Griffin cohort at source revision `3062506949441219699` still fails the
landing gate: peak ground speed is **0.0679792 m/s**, above **0.05 m/s**.
Peak angular speed **0.00744409 rad/s** and sixty-second horizontal drift
**0.0144244 m** pass their existing **0.01 rad/s** and **0.35 m** limits.
Finite state, upright attitude and four-leg contact also pass. These are a
different authored source cohort from the original USD baseline; they are not
an identical-input performance or solver A/B comparison.

Ramp unfolding remains blocked. At tick 2665, both middle hinges target zero
but measure approximately 1.7933 rad with enabled 5000 N·m drives. Native
contacts identify both middle tracks against their fixed transition plates,
with approximately 0.0173 m penetration. Root and toe flight latches are still
installed, as required by the current sequence. Joint coordinates include the
authored -90-degree child-frame offset; a scalar angle cannot be interpreted
as the physical section angle without that frame. A subsequent diagnostic
reverse command also produces toe/root/plate collisions; it does not establish
a collision-free alternate path. Contact snapshots and coherent hinge/drive
reads are in `prepared-velocity-performance/`. The mechanism geometry and
deployment path need qualification; disabling contacts or increasing drive
force would not establish a valid repair.

The sub-1-ms target remains open. The contention-affected flight sample has
2.123 ms median and 3.201 ms p95 full native step time; touchdown has 2.868 ms
median. Concurrent applications were active, so these are diagnostic figures,
not clean acceptance. Sleeping-body samples do not qualify active physics.

## USD authored ramp geometry follow-up

USD independently confirms the obstruction in the saved model `4dc4d17`,
using the canonical query repair: all four middle tracks intersect their
matching fixed transitions at 120-degree inward stow. Exact-box SAT gives
approximately -0.22070535 m signed axis margin; this is not runtime contact
penetration. Source hashes and six collider snapshots are in the model's
`twins/astrobotic-griffin-1/contracts/evidence/griffin-ramp-interference-2026-10-22.json`.
The companion Markdown contract records a provisional unequal-length,
opposite-toe-sweep study and its limited coverage. These live in
`/home/rod/Documents/models/lunar-base-model`.

The existing source mechanism and common toe-unfold direction remain
unqualified. USD owns the detailed Griffin component/assembly repair. Please
keep the authored obstruction separate from the native landing-speed and
active-step performance findings; no solver, dependency, contact policy,
mass or torque change was made by this inspection.

## Native integration checkpoint — optimization

The published Avian revision `9097ce9` prepares hard joint velocity gradients,
mass factorization and responses once per substep. A matched diagnostic window
of flight steps 60–1000 gives a 2.756 ms median full native step and 3.416 ms
p95; joint/contact velocity relaxation alone has a 0.970 ms median. Standard
eight substeps are retained. Concurrent graphical workloads and Tracy overhead
make these attribution measurements, not clean performance acceptance.
The capture `scripts/perf/captures/griffin-native-9097-20261022.tracy` passes
framing validation and native CSV export. A further ECS borrow-cache prototype
showed no meaningful improvement in a matched short window and was discarded.

The final published-pin production binary
`e323b6c0279ee421791507c2a7a237698a6b05aaf4fdcc9cf55295e9d8201ae5`
passes loaded-slider (7), CCD-filter (6), native drive (24), contact (12),
drivetrain parity (20), coupled pause (11), paused wheel effort (16), suspension
travel (3), Summer Lunokhod (102), IMU (267) and FLIP route (25) checks. FLIP
reaches all three waypoints at tick 1192; its shared Modelica guidance retains
approach control until arrival. Receipts are in
`.cache/codex-evidence/physics-followup-20261022/published-native-final-regression/`.
This does not close Griffin ramp deployment or the sub-1-ms active-step target.

USD also recorded a baseline-physics default-scene witness while refreshing
the Twin source list for the detached root-section work. Binary SHA256
`9b28c4be8f9e77e8bd1fe8e5ee6f21db4e83afbe2d3d9153cf0c10c64d1579eb`
has none of optimization's uncommitted solver changes. The vehicle USD is
unchanged from `4dc4d17`; the new root component was not mounted in it.
At simulation time 121.916667 s, `/Griffin1SurfaceOps/FLIP` exited the finite
world at (153.449657, 1050.063833, -195.252691) m, with velocity
(20.96518, 66.85105, -13.631392) m/s. The native `physics-body-escaped`
policy paused its 14 dynamic bodies. Receipt:
`/home/rod/Documents/luncosim-workspace/usd/.cache/codex-evidence/griffin-root-section-20261022/window.log`,
event timestamp `2026-10-22T21:24:42.313807Z`. This is a baseline witness,
not a regression comparison against the repaired solver or landing acceptance.
USD continues the detached component and static geometry work separately.
