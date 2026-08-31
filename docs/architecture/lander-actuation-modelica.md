# Lander actuation and Modelica network architecture

> Status: Active · Audience: contributors on lander propulsion, actuation, and Modelica/Avian co-simulation

This is the canonical boundary for a powered lander. The airframe, control law,
propulsion, physical actuator placement, and physics engine are separate
projections of one composed USD design.

## Runtime ownership

| Concern | Owner | Contract |
|---|---|---|
| Mission composition and topology | USD | `inputs:*`, `outputs:*`, `connectors:*`, collection membership, transforms, actuator direction and limits; structural actuator command targets use USD relationships |
| Sensor frame conversion | USD sensor projection + Modelica sensor component | The control law receives body-local gyro and attitude error, never world-frame pose or wrench |
| Guidance and control equations | Modelica | `Lander.mo` produces a normalized main-valve request and body-local torque demand; PID tuning is exposed through authored inputs |
| Propellant hydraulics and engine performance | Modelica | Acausal `LunCo.Propulsion.FluidPort` connections carry pressure, conserved mass flow, and stream specific enthalpy; tanks, turbopumps, and chamber are reusable package members |
| Actuator allocation | Generic projection + reusable Modelica allocator | USD actuator geometry is factorized once; `LunCo.Actuation.WrenchAllocator` evaluates normalized valve demands at runtime |
| Applied force and torque | Avian projection | Each physical actuator applies force at its authored local mount and direction, so adding an actuator requires no vehicle-specific Rust code |
| Flame and RCS visuals | USD render wiring | Flame throttle/activity is connected to the corresponding Modelica output; no script mirrors actuator state |
| Mission sequencing | Rhai | Scenario orchestration and event policy; production uses task/events, while authored tests may use `on_tick` for bounded sampled verdicts |

Built-in network ownership is derived from typed USD roles. A collection whose
members apply `LunCoProgramAPI` is an acausal Modelica network; a collection
whose members apply `LunCoForceActuatorAPI` is the geometry-derived wrench
allocator. The lander therefore does not repeat `lunco:synthesizer` beside its
actuator list. `LunCoDomainSynthesisAPI` remains available only when an asset
deliberately selects a registered non-default policy. Mixed or unclassified
member roles are rejected as authoring errors.

## Signal and fluid paths

The main engine is a generated `CollectionAPI:components` network. USD connects:

```text
Lander.throttle
       │
       ├── valve opening ──► FuelPump ──FluidPort──► MainChamber
       │                         ▲                       │
       │                         │                       ├── thrust_n ──► Nozzle ──► Avian
       └── valve opening ──► OxidizerPump ─FluidPort──►  └── activity ──► plume/light
                                  ▲
                            FuelTank / OxidizerTank
```

The attitude path is intentionally generic:

```text
Lander.torque_x/y/z
       │
       ▼
USD actuator collection ──► WrenchAllocator ──► normalized valve outputs
                                                         │
                                       USD-authored physical RCS actuator prims
                                                         │
                                      Avian force-at-authored-mount projection
```

The lander model does not instantiate an RCS cluster and does not calculate a
world-frame wrench. The controller sees local sensors and asks for local body
torque. USD decides which physical devices provide that authority.

The accepted `landing_handoff` input is the single landed-state boundary at the
airframe actuator owner. `landing_engine_cutoff` closes the main-engine request
when the target-qualified contact event is accepted; the airframe retains only
measured body-rate damping while the suspension settles. The later accepted
handoff gates the normalized throttle and every body-torque output, so stale
filtered commands or late scenario writes cannot reopen the main engine or RCS.
The remaining motion is therefore resolved by the authored Avian bodies, the
native prismatic geometry, the authored passive crush cartridges, contact, and
damping rather than by a transform clamp or controller freeze. A passive
cartridge is not a Modelica actuator: its constitutive state belongs to the
`LunCoPrismaticSuspensionAPI` material projected by `lunco-usd-sim` and solved
in Avian's existing substep schedule.

## Visualization and live state

Every reusable propulsion component has a semantic Modelica `Icon` and
`Placement` is emitted for every generated component. A generated network also
receives a class-level assembly icon and diagram banner. The Modelica canvas:

- renders acausal `FluidPort` connections as physical network edges;
- renders scalar input/output equalities as directed signal edges;
- reads live node state for animated flow dots and edge tooltips;
- resolves `DynamicSelect` icon labels for tank mass, pump speed, chamber
  activity, and RCS activity/thrust.

Thus a diagram communicates both topology and current state. It does not infer
connections from filenames or draw a decorative copy of the network in Rust.

The geometry-derived `synth.actuator-wrench` policy follows the same source
contract for its single generated root: the `WrenchAllocator` is a real placed
Modelica component and the root uses standard `Icon`/`Diagram` annotations.
The physical USD actuators remain Avian members, so the UI does not invent a
fake Modelica unit or drill-down class for them.

Each force actuator targets its allocator output with
`rel lunco:forceActuator:commandSource = </...outputs:...>`. This is a
structural relationship because the actuator's real physical signal is the
typed `inputs:force_command` connection to its thrust model. The relationship
selects the allocator column without duplicating a value port or matching a
free-form output name.

## Parameter rule

Physical and controller constants are Modelica parameters or public Modelica
inputs. Scene-specific values are authored as USD `inputs:*` constants and are
therefore visible to the Inspector and included in the generated source. The
following are explicitly exposed in the lander scene:

- controller filter and touchdown transition parameters;
- tank mass, pressure, depletion, and smoothing parameters;
- pump flow, pressure, efficiency, spool, speed, and density parameters;
- chamber mixture ratio, c-star, throat, exhaust, efficiency, temperature, and
  energy parameters;
- all twelve RCS nozzle force, specific-impulse, and gravity parameters;
- PID gains, integral limits, acceleration limits, and mission target values.

Projection tolerances used to factor actuator geometry are named Rust constants;
they are numerical projection policy, not hidden vehicle design parameters.

## USD/Modelica port invariant

An unconnected USD `inputs:<name>` constant is a scene-authored value, but it is
still sent through the live co-simulation input surface. A Lander setting that
the Inspector must be able to change while the simulation is running is
therefore declared as `input Real` in `Lander.mo`, not as a compile-time
`parameter Real`. This keeps the two sides of the contract honest: the USD
constant is accepted by the compiled DAE, and a later Inspector edit reaches
the active solver through `set_input`.

Compile-time Modelica parameters remain appropriate for component structure
and generated-network construction, where USD values are inserted into the
generated source before compilation. They must not be presented as live
controller ports. Mixing those categories was the second runtime defect found
after the navigation fix: the model compiled, but readiness correctly rejected
the Lander because eight USD controller settings had no compiled input slot.

## Failure policy

An unresolved Modelica source is pending and then becomes an explicit source
resolution error. The projector never substitutes a class name derived from a
filesystem path. Unknown synthesizer names are errors. A changed or removed
network retires the previous runtime projection instead of keeping stale solver
state alive.

## Navigation/PID structural invariant

`LanderNavigation` exposes its integrated position and velocity states directly.
The lateral states satisfy `der(nav_pos) = nav_vel` and the velocity states
satisfy `der(nav_vel) = imu_accel`; mission-initialized values are applied by
Modelica `initial equation` clauses. Vertical position remains the live
altimeter measurement plus the authored mount offset, while vertical velocity
is still integrated from the IMU.

This shape is important. The previous implementation kept hidden `delta_*`
states and defined `nav_pos_*`/`nav_vel_*` as algebraic aliases. Rumoca could
compile each block independently, but when a PID consumed both aliases from
the same navigation block, its initial-condition incidence graph lost two
matches and reported a singular system. The physical equations were not
underdetermined; our state/output boundary was structurally ambiguous.

`PositionPID3D` also consumes the public saturated `PIDAxis.command` output.
It does not read `raw_command` or add a second lateral limiter. The regression
in `crates/lunco-modelica/tests/gnc_position_pid.rs` compiles and advances both
the reusable axis and the complete sensor-driven three-axis controller, so this
failure is guarded at the Modelica boundary rather than hidden by a solver
relaxation.
